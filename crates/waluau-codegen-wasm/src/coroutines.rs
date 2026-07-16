use std::collections::{BTreeMap, BTreeSet, HashMap};

use waluau_ast::Type;
use waluau_diagnostics::Diagnostic;
use waluau_ir::{BlockId, Instruction as IrInstruction, Module, Terminator, ValueId};
use wasm_encoder::{ConstExpr, GlobalSection, GlobalType, HeapType, RefType, ValType};

use crate::FunctionSignature;
use crate::arrays::ArrayTypeRegistry;
use crate::locals::{build_local_plan, infer_value_types};
use crate::wasm_types::wasm_type;

pub(crate) const STATE_TAG_FIELD: u32 = 0;
pub(crate) const STATE_YIELDED_FIELD: u32 = 1;
pub(crate) const STATE_CONT_FIELD: u32 = 2;
pub(crate) const STATE_AWAIT_STATUS_FIELD: u32 = 3;
pub(crate) const STATE_PC_FIELD_BASE: u32 = 4;
pub(crate) const TAG_SUSPENDED: i32 = 0;
pub(crate) const TAG_FINISHED: i32 = 1;
pub(crate) const TAG_ERROR: i32 = 2;
pub(crate) const TAG_AWAITING_PROMISE: i32 = 3;
pub(crate) const AWAIT_STATUS_NONE: i32 = 0;
pub(crate) const AWAIT_STATUS_FULFILLED: i32 = 1;
pub(crate) const AWAIT_STATUS_REJECTED: i32 = 2;

#[derive(Clone, Debug)]
pub(crate) struct CoroutineSpillSlot {
    pub(crate) local: u32,
    pub(crate) field: u32,
    pub(crate) ty: Type,
}

#[derive(Clone, Debug)]
pub(crate) struct CoroutineCallResumePoint {
    pub(crate) value: ValueId,
    pub(crate) block: BlockId,
    pub(crate) instruction_index: usize,
    pub(crate) pc: i32,
}

#[derive(Clone, Debug)]
pub(crate) struct CoroutinePlan {
    active_global: Option<u32>,
    pc_fields: HashMap<String, u32>,
    pc_initial_values: Vec<i32>,
    yielding_functions: BTreeSet<String>,
    call_resume_points: HashMap<String, Vec<CoroutineCallResumePoint>>,
    spill_slots: HashMap<String, Vec<CoroutineSpillSlot>>,
    spill_field_types: Vec<Type>,
}

impl CoroutinePlan {
    pub(crate) fn new(module: &Module, imported_global_count: u32) -> Self {
        let mut directly_yielding = BTreeSet::new();
        for function in &module.functions {
            if function.blocks.values().any(|block| {
                matches!(
                    block.terminator,
                    Terminator::CoroutineYield { .. } | Terminator::CoroutineAwaitPromise { .. }
                )
            }) {
                directly_yielding.insert(function.name.clone());
            }
        }

        let mut yielding_functions = directly_yielding.clone();
        loop {
            let mut changed = false;
            for function in &module.functions {
                if yielding_functions.contains(&function.name) {
                    continue;
                }
                let calls_yielding = function.blocks.values().any(|block| {
                    block.instructions.iter().any(|(_, instruction)| {
                        matches!(instruction, IrInstruction::Call { name, .. } if yielding_functions.contains(name))
                    })
                });
                if calls_yielding {
                    changed |= yielding_functions.insert(function.name.clone());
                }
            }
            if !changed {
                break;
            }
        }

        let has_coroutine_ops = module.functions.iter().any(|function| {
            function.blocks.values().any(|block| {
                block.instructions.iter().any(|(_, instruction)| {
                    matches!(
                        instruction,
                        IrInstruction::CoroutineCreate { .. }
                            | IrInstruction::CoroutineResume { .. }
                            | IrInstruction::CoroutineClose { .. }
                    )
                })
            })
        });

        let has_state = has_coroutine_ops || !yielding_functions.is_empty();
        if !has_state {
            return Self {
                active_global: None,
                pc_fields: HashMap::new(),
                pc_initial_values: Vec::new(),
                yielding_functions,
                call_resume_points: HashMap::new(),
                spill_slots: HashMap::new(),
                spill_field_types: Vec::new(),
            };
        }

        let mut pc_fields = HashMap::new();
        let mut pc_initial_values = Vec::new();
        for (index, name) in yielding_functions.iter().enumerate() {
            pc_fields.insert(name.clone(), STATE_PC_FIELD_BASE + index as u32);
            let entry = module
                .functions
                .iter()
                .find(|function| function.name == *name)
                .map(|function| function.entry.0 as i32)
                .unwrap_or(0);
            pc_initial_values.push(entry);
        }

        let mut call_resume_points = HashMap::new();
        for function in &module.functions {
            if !yielding_functions.contains(&function.name) {
                continue;
            }
            let mut next_pc = function
                .blocks
                .keys()
                .map(|block| block.0 as i32)
                .max()
                .unwrap_or(0)
                + 1;
            let mut points = Vec::new();
            for block in function.blocks.values() {
                for (instruction_index, (value, instruction)) in
                    block.instructions.iter().enumerate()
                {
                    if matches!(instruction, IrInstruction::Call { name, .. } if yielding_functions.contains(name))
                    {
                        points.push(CoroutineCallResumePoint {
                            value: *value,
                            block: block.id,
                            instruction_index,
                            pc: next_pc,
                        });
                        next_pc += 1;
                    }
                }
            }
            call_resume_points.insert(function.name.clone(), points);
        }

        Self {
            active_global: Some(imported_global_count),
            pc_fields,
            pc_initial_values,
            yielding_functions,
            call_resume_points,
            spill_slots: HashMap::new(),
            spill_field_types: Vec::new(),
        }
    }

    pub(crate) fn configure_spills(
        &mut self,
        module: &Module,
        signatures: &HashMap<String, FunctionSignature>,
        array_registry: &ArrayTypeRegistry,
    ) -> Result<(), Diagnostic> {
        let mut next_field = STATE_PC_FIELD_BASE + self.pc_field_count();
        for function in &module.functions {
            if !self.function_yields(&function.name) {
                continue;
            }
            let value_types = infer_value_types(function, signatures)?;
            let local_plan = build_local_plan(function, &value_types, array_registry, true)?;
            let mut local_types = BTreeMap::<u32, Type>::new();
            for (value, local) in &local_plan.slots {
                let Some(ty) = value_types.get(value) else {
                    continue;
                };
                if matches!(ty, Type::Unit | Type::Multi(_)) {
                    continue;
                }
                if let Some(existing) = local_types.get(local) {
                    if wasm_type(existing, array_registry)? != wasm_type(ty, array_registry)? {
                        return Err(Diagnostic::new(format!(
                            "coroutine local {local} in '{}' is reused across incompatible types {existing} and {ty}",
                            function.name
                        )));
                    }
                } else {
                    local_types.insert(*local, ty.clone());
                }
            }
            for (value, slots) in &local_plan.multi_slots {
                let Some(Type::Multi(types)) = value_types.get(value) else {
                    continue;
                };
                for (local, ty) in slots.iter().zip(types) {
                    local_types.insert(*local, ty.clone());
                }
            }

            let mut spills = Vec::with_capacity(local_types.len());
            for (local, ty) in local_types {
                spills.push(CoroutineSpillSlot {
                    local,
                    field: next_field,
                    ty: ty.clone(),
                });
                self.spill_field_types.push(ty);
                next_field += 1;
            }
            self.spill_slots.insert(function.name.clone(), spills);
        }
        Ok(())
    }

    pub(crate) fn has_state(&self) -> bool {
        self.active_global.is_some()
    }

    pub(crate) fn active_global(&self) -> Result<u32, Diagnostic> {
        self.active_global
            .ok_or_else(|| Diagnostic::new("missing coroutine active-instance global"))
    }

    pub(crate) fn pc_field(&self, name: &str) -> Option<u32> {
        self.pc_fields.get(name).copied()
    }

    pub(crate) fn pc_field_count(&self) -> u32 {
        self.pc_fields.len() as u32
    }

    pub(crate) fn pc_initial_values(&self) -> &[i32] {
        &self.pc_initial_values
    }

    pub(crate) fn call_resume_points(&self, name: &str) -> &[CoroutineCallResumePoint] {
        self.call_resume_points
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(crate) fn call_resume_point(&self, name: &str, value: ValueId) -> Option<i32> {
        self.call_resume_points(name)
            .iter()
            .find(|point| point.value == value)
            .map(|point| point.pc)
    }

    pub(crate) fn spill_slots(&self, name: &str) -> &[CoroutineSpillSlot] {
        self.spill_slots.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    pub(crate) fn spill_field_types(&self) -> &[Type] {
        &self.spill_field_types
    }

    pub(crate) fn emit_globals(&self, globals: &mut GlobalSection, state_type_index: u32) {
        if !self.has_state() {
            return;
        }
        globals.global(
            GlobalType {
                val_type: coroutine_state_ref_type(state_type_index),
                mutable: true,
                shared: false,
            },
            &ConstExpr::ref_null(HeapType::Concrete(state_type_index)),
        );
    }

    pub(crate) fn function_yields(&self, name: &str) -> bool {
        self.yielding_functions.contains(name)
    }
}

pub(crate) fn coroutine_state_ref_type(state_type_index: u32) -> ValType {
    ValType::Ref(RefType {
        nullable: true,
        heap_type: HeapType::Concrete(state_type_index),
    })
}
