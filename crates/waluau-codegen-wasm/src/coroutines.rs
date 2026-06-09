use std::collections::{BTreeSet, HashMap};

use waluau_diagnostics::Diagnostic;
use waluau_ir::{Instruction as IrInstruction, Module, Terminator};
use wasm_encoder::{ConstExpr, GlobalSection, GlobalType, HeapType, RefType, ValType};

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
pub(crate) struct CoroutinePlan {
    active_global: Option<u32>,
    pc_fields: HashMap<String, u32>,
    yielding_functions: BTreeSet<String>,
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
                yielding_functions,
            };
        }

        let mut pc_fields = HashMap::new();
        for (index, name) in directly_yielding.into_iter().enumerate() {
            pc_fields.insert(name, STATE_PC_FIELD_BASE + index as u32);
        }

        Self {
            active_global: Some(imported_global_count),
            pc_fields,
            yielding_functions,
        }
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
