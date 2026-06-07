use std::collections::{BTreeMap, BTreeSet, HashMap};

use waluau_ast::{BinaryOp, NumericType, Type};
use waluau_diagnostics::Diagnostic;
use waluau_ir::{
    BasicBlock, Function as IrFunction, Instruction as IrInstruction, Terminator, ValueId,
};
use wasm_encoder::{Function, Instruction, ValType};

use crate::FunctionSignature;
use crate::arrays::ArrayTypeRegistry;
use crate::coroutines::coroutine_state_ref_type;
use crate::wasm_types::wasm_type;

pub(crate) struct LocalPlan {
    pub(crate) slots: BTreeMap<ValueId, u32>,
    pub(crate) multi_slots: BTreeMap<ValueId, Vec<u32>>,
    pub(crate) extra_locals: Vec<ValType>,
    pub(crate) stack_values: BTreeSet<ValueId>,
    pub(crate) unit_values: BTreeSet<ValueId>,
    pub(crate) pc_local: u32,
    /// Scratch `(ref null $coroutine_state)` local for saving/restoring the active
    /// instance across a `coroutine.resume` (nested-coroutine support).
    pub(crate) coroutine_save_local: Option<u32>,
    /// Scratch i32 local for spilling a yielded value before mutating the state struct.
    pub(crate) coroutine_yield_tmp: Option<u32>,
    /// Scratch i32 local for spilling the i32 continuation result in tagged resume.
    pub(crate) tagged_resume_value_tmp: Option<u32>,
    /// Scratch i32 local for reading the post-continuation state tag in tagged resume.
    pub(crate) tagged_resume_state_tmp: Option<u32>,
}

#[derive(Clone)]
struct LiveInterval {
    value: ValueId,
    start: usize,
    end: usize,
    ty: Type,
}

pub(crate) fn build_local_plan(
    function: &IrFunction,
    value_types: &BTreeMap<ValueId, Type>,
    array_registry: &ArrayTypeRegistry,
) -> Result<LocalPlan, Diagnostic> {
    let mut slots = BTreeMap::new();
    let unit_values = value_types
        .iter()
        .filter_map(|(value, ty)| matches!(ty, Type::Unit).then_some(*value))
        .collect::<BTreeSet<_>>();
    let phi_copy_sources = collect_phi_copy_sources(function);
    let mut stack_values = BTreeSet::new();

    let mut captured_values = BTreeSet::new();
    let mut global_uses = BTreeMap::new();
    for b in function.blocks.values() {
        for (_, inst) in &b.instructions {
            if let IrInstruction::Closure { captures, .. } = inst {
                captured_values.extend(captures.iter().copied());
            }
            for operand in instruction_operands(inst) {
                *global_uses.entry(operand).or_default() += 1;
            }
            if let IrInstruction::Phi(incoming) = inst {
                for (_, incoming_value) in incoming {
                    *global_uses.entry(*incoming_value).or_default() += 1;
                }
            }
        }
        for operand in terminator_operands(&b.terminator) {
            *global_uses.entry(operand).or_default() += 1;
        }
    }

    for block in function.blocks.values() {
        let block_stack_values = compute_stack_values(
            block,
            phi_copy_sources.get(&block.id).cloned().unwrap_or_default(),
            &captured_values,
            &global_uses,
        );
        stack_values.extend(block_stack_values);
    }
    stack_values.retain(|v| !matches!(value_types.get(v), Some(Type::Multi(_)) | Some(Type::Unit)));

    let intervals = compute_live_intervals(function, value_types, &stack_values)?;
    let (local_slots, mut extra_locals) =
        assign_locals_by_live_range(&intervals, function.params.len() as u32, array_registry)?;
    slots.extend(local_slots);

    for block in function.blocks.values() {
        for (value, instruction) in &block.instructions {
            if matches!(instruction, IrInstruction::Param(_)) {
                let IrInstruction::Param(index) = instruction else {
                    unreachable!()
                };
                slots.insert(*value, *index as u32);
            }
        }
    }

    let mut multi_slots = BTreeMap::new();
    for block in function.blocks.values() {
        for (value, _) in &block.instructions {
            let Some(Type::Multi(types)) = value_types.get(value) else {
                continue;
            };
            let mut value_slots = Vec::new();
            for ty in types {
                let val_type = wasm_type(ty, array_registry)?;
                let slot = function.params.len() as u32 + extra_locals.len() as u32;
                extra_locals.push(val_type);
                value_slots.push(slot);
            }
            multi_slots.insert(*value, value_slots);
        }
    }

    let pc_local = function.params.len() as u32 + extra_locals.len() as u32;
    extra_locals.push(ValType::I32);

    let has_resume = function.blocks.values().any(|block| {
        block.instructions.iter().any(|(_, instruction)| {
            matches!(
                instruction,
                IrInstruction::CoroutineResume { .. } | IrInstruction::CoroutineResumeTagged { .. }
            )
        })
    });
    let has_tagged_resume = function.blocks.values().any(|block| {
        block.instructions.iter().any(|(_, instruction)| {
            matches!(instruction, IrInstruction::CoroutineResumeTagged { .. })
        })
    });
    let coroutine_save_local = if has_resume {
        let slot = function.params.len() as u32 + extra_locals.len() as u32;
        extra_locals.push(coroutine_state_ref_type(
            array_registry.coroutine_state_type()?,
        ));
        Some(slot)
    } else {
        None
    };

    let has_yield = function
        .blocks
        .values()
        .any(|block| matches!(block.terminator, Terminator::CoroutineYield { .. }));
    let coroutine_yield_tmp = if has_yield {
        let slot = function.params.len() as u32 + extra_locals.len() as u32;
        extra_locals.push(ValType::I32);
        Some(slot)
    } else {
        None
    };

    let tagged_resume_value_tmp = if has_tagged_resume {
        let slot = function.params.len() as u32 + extra_locals.len() as u32;
        extra_locals.push(ValType::I32);
        Some(slot)
    } else {
        None
    };
    let tagged_resume_state_tmp = if has_tagged_resume {
        let slot = function.params.len() as u32 + extra_locals.len() as u32;
        extra_locals.push(ValType::I32);
        Some(slot)
    } else {
        None
    };

    Ok(LocalPlan {
        slots,
        multi_slots,
        extra_locals,
        stack_values,
        unit_values,
        pc_local,
        coroutine_save_local,
        coroutine_yield_tmp,
        tagged_resume_value_tmp,
        tagged_resume_state_tmp,
    })
}

pub(crate) fn build_value_definition_map(function: &IrFunction) -> HashMap<ValueId, IrInstruction> {
    let mut defs = HashMap::new();
    for block in function.blocks.values() {
        for (value, instruction) in &block.instructions {
            defs.insert(*value, instruction.clone());
        }
    }
    defs
}

pub(crate) fn infer_value_types(
    function: &IrFunction,
    signatures: &HashMap<String, FunctionSignature>,
) -> Result<BTreeMap<ValueId, Type>, Diagnostic> {
    let mut types = BTreeMap::new();

    for block in function.blocks.values() {
        for (value, instruction) in &block.instructions {
            let ty = match instruction {
                IrInstruction::Param(index) => {
                    let ty = &function.params[*index].1;
                    if matches!(ty, Type::TaggedUnion(_) | Type::TaggedVariant(_)) {
                        Type::canonical_tagged_union_record()
                    } else {
                        ty.clone()
                    }
                }
                IrInstruction::Number { ty, .. } => Type::Numeric(*ty),
                IrInstruction::Unit => Type::Unit,
                IrInstruction::Bool(_) => Type::Bool,
                IrInstruction::Null { ty } => ty.clone(),
                IrInstruction::String(_) => Type::String,
                IrInstruction::Bytes(_) => Type::Bytes,
                IrInstruction::Cast { to, .. } => to.clone(),
                IrInstruction::Binary { result_ty, .. } => result_ty.clone(),
                IrInstruction::IsNull { .. } => Type::Bool,
                IrInstruction::MathIntrinsic { result_ty, .. } => result_ty.clone(),
                IrInstruction::ToString { .. } => Type::String,
                IrInstruction::Print { .. } => Type::Unit,
                IrInstruction::Call { name, .. } => signatures
                    .get(name)
                    .ok_or_else(|| {
                        Diagnostic::new(format!("unknown function '{name}' during wasm emission"))
                    })?
                    .result
                    .clone(),
                IrInstruction::CallValue { return_type, .. } => return_type.clone(),
                IrInstruction::CoroutineCreate { .. } => Type::Thread,
                IrInstruction::CoroutineResume { .. } => {
                    Type::Multi(vec![Type::Bool, Type::Numeric(NumericType::I32)])
                }
                IrInstruction::CoroutineResumeTagged { .. } => {
                    Type::canonical_tagged_union_record()
                }
                IrInstruction::CoroutineClose { .. } => Type::Bool,
                IrInstruction::Closure {
                    params,
                    return_type,
                    ..
                } => Type::Function {
                    params: params.clone(),
                    return_type: Box::new(return_type.clone()),
                },
                IrInstruction::ArrayNew { element_ty, .. } => {
                    Type::Array(Box::new(element_ty.clone()))
                }
                IrInstruction::ArrayGet { element_ty, .. } => element_ty.clone(),
                IrInstruction::ArraySet { .. } => Type::Numeric(NumericType::I32),
                IrInstruction::ArrayLen { .. } => Type::Numeric(NumericType::I32),
                IrInstruction::BytesGet { .. } => Type::Numeric(NumericType::I32),
                IrInstruction::BytesLen { .. } => Type::Numeric(NumericType::I32),
                IrInstruction::StructNew { struct_ty, .. } => struct_ty.clone(),
                IrInstruction::StructGet { field_ty, .. } => field_ty.clone(),
                IrInstruction::StructSet { .. } => Type::Unit,
                IrInstruction::PackMulti { types, .. } => Type::Multi(types.clone()),
                IrInstruction::MultiGet { ty, .. } => ty.clone(),
                IrInstruction::Phi(_) => continue,
            };
            types.insert(*value, ty);
        }
    }

    loop {
        let mut progress = false;
        for block in function.blocks.values() {
            for (value, instruction) in &block.instructions {
                if types.contains_key(value) {
                    continue;
                }

                let IrInstruction::Phi(incoming) = instruction else {
                    continue;
                };
                let mut candidate: Option<Type> = None;
                for (_, incoming) in incoming {
                    if incoming == value {
                        continue;
                    }
                    let Some(ty) = types.get(incoming).cloned() else {
                        continue;
                    };
                    if let Some(ref expected) = candidate {
                        if expected != &ty {
                            return Err(Diagnostic::new(format!(
                                "phi {:?} has inconsistent incoming types during wasm emission",
                                value
                            )));
                        }
                    } else {
                        candidate = Some(ty);
                    }
                }
                if let Some(ty) = candidate {
                    types.insert(*value, ty);
                    progress = true;
                }
            }
        }

        if !progress {
            break;
        }
    }

    for block in function.blocks.values() {
        for (value, instruction) in &block.instructions {
            if matches!(instruction, IrInstruction::Phi(_)) && !types.contains_key(value) {
                return Err(Diagnostic::new(format!(
                    "could not infer phi type for value {:?}",
                    value
                )));
            }
        }
    }

    Ok(types)
}

fn collect_phi_copy_sources(
    function: &IrFunction,
) -> BTreeMap<waluau_ir::BlockId, BTreeSet<ValueId>> {
    let mut phi_copy_sources = BTreeMap::new();
    for block in function.blocks.values() {
        for (_, instruction) in &block.instructions {
            let IrInstruction::Phi(incoming) = instruction else {
                continue;
            };
            for (source, value) in incoming {
                phi_copy_sources
                    .entry(*source)
                    .or_insert_with(BTreeSet::new)
                    .insert(*value);
            }
        }
    }
    phi_copy_sources
}

fn compute_stack_values(
    block: &BasicBlock,
    phi_copy_sources: BTreeSet<ValueId>,
    captured_values: &BTreeSet<ValueId>,
    global_uses: &BTreeMap<ValueId, usize>,
) -> BTreeSet<ValueId> {
    let mut uses = BTreeMap::<ValueId, Vec<usize>>::new();
    for (index, (_, instruction)) in block.instructions.iter().enumerate() {
        for operand in instruction_operands(instruction) {
            uses.entry(operand).or_default().push(index);
        }
    }
    let terminator_use_index = block.instructions.len();
    for operand in terminator_operands(&block.terminator) {
        uses.entry(operand).or_default().push(terminator_use_index);
    }

    let mut stack_values = BTreeSet::new();
    for (index, (value, instruction)) in block.instructions.iter().enumerate() {
        if matches!(instruction, IrInstruction::Param(_) | IrInstruction::Phi(_)) {
            continue;
        }
        if matches!(instruction, IrInstruction::CoroutineCreate { .. }) {
            continue;
        }
        if phi_copy_sources.contains(value) {
            continue;
        }
        if captured_values.contains(value) {
            continue;
        }
        if global_uses.get(value).copied().unwrap_or(0) != 1 {
            continue;
        }
        let Some(use_sites) = uses.get(value) else {
            continue;
        };
        if use_sites.len() != 1 {
            continue;
        }
        let use_index = use_sites[0];
        if use_index != index + 1 {
            continue;
        }
        if use_index < block.instructions.len() {
            let consumer = &block.instructions[use_index].1;
            if instruction_use_requires_local(consumer)
                || !instruction_can_consume_stack_value(consumer, *value)
            {
                continue;
            }
        }
        stack_values.insert(*value);
    }

    stack_values
}

fn compute_live_intervals(
    function: &IrFunction,
    value_types: &BTreeMap<ValueId, Type>,
    stack_values: &BTreeSet<ValueId>,
) -> Result<Vec<LiveInterval>, Diagnostic> {
    let mut block_bases = BTreeMap::new();
    let mut block_end_positions = BTreeMap::new();
    let mut cursor = 0usize;
    for block in function.blocks.values() {
        block_bases.insert(block.id, cursor);
        let end = cursor + block.instructions.len();
        block_end_positions.insert(block.id, end);
        cursor = end + 1;
    }

    let mut def_positions = BTreeMap::<ValueId, usize>::new();
    let mut last_use_positions = BTreeMap::<ValueId, usize>::new();
    for block in function.blocks.values() {
        let base = block_bases[&block.id];
        for (index, (value, instruction)) in block.instructions.iter().enumerate() {
            if stack_values.contains(value) {
                continue;
            }
            match instruction {
                IrInstruction::Param(_) => {}
                IrInstruction::Phi(_) => {
                    def_positions.insert(*value, base);
                    last_use_positions.entry(*value).or_insert(base);
                }
                _ => {
                    let pos = base + index;
                    def_positions.insert(*value, pos);
                    last_use_positions.entry(*value).or_insert(pos);
                }
            }
        }
    }

    for block in function.blocks.values() {
        let base = block_bases[&block.id];
        for (index, (_, instruction)) in block.instructions.iter().enumerate() {
            let pos = base + index;
            for operand in instruction_operands(instruction) {
                if stack_values.contains(&operand) {
                    continue;
                }
                if let Some(last_use) = last_use_positions.get_mut(&operand) {
                    *last_use = (*last_use).max(pos);
                }
            }
        }
        let term_pos = block_end_positions[&block.id];
        for operand in terminator_operands(&block.terminator) {
            if stack_values.contains(&operand) {
                continue;
            }
            if let Some(last_use) = last_use_positions.get_mut(&operand) {
                *last_use = (*last_use).max(term_pos);
            }
        }
    }

    for block in function.blocks.values() {
        for (_, instruction) in &block.instructions {
            let IrInstruction::Phi(incoming) = instruction else {
                continue;
            };
            for (source, incoming_value) in incoming {
                if stack_values.contains(incoming_value) {
                    continue;
                }
                if let Some(last_use) = last_use_positions.get_mut(incoming_value) {
                    *last_use = (*last_use).max(block_end_positions[source]);
                }
            }
        }
    }

    let mut intervals = Vec::new();
    for (value, start) in def_positions {
        let ty = value_types
            .get(&value)
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("missing type for value {:?}", value)))?;
        if matches!(ty, Type::Multi(_) | Type::Unit) {
            continue;
        }
        let end = last_use_positions.get(&value).copied().unwrap_or(start);
        intervals.push(LiveInterval {
            value,
            start,
            end,
            ty,
        });
    }
    intervals.sort_by_key(|interval| (interval.start, interval.end, interval.value));
    Ok(intervals)
}

fn assign_locals_by_live_range(
    intervals: &[LiveInterval],
    first_local: u32,
    array_registry: &ArrayTypeRegistry,
) -> Result<(BTreeMap<ValueId, u32>, Vec<ValType>), Diagnostic> {
    #[derive(Clone)]
    struct ActiveInterval {
        end: usize,
        slot: u32,
        val_type: ValType,
    }

    let mut slots = BTreeMap::new();
    let mut declared_locals = Vec::new();
    let mut free_slots: Vec<(ValType, Vec<u32>)> = Vec::new();
    let mut active: Vec<ActiveInterval> = Vec::new();
    let mut next_local = first_local;

    for interval in intervals {
        let val_type = wasm_type(&interval.ty, array_registry)?;
        let mut next_active = Vec::new();
        for live in active {
            if live.end < interval.start {
                let mut found = false;
                for (bucket_ty, bucket) in &mut free_slots {
                    if *bucket_ty == live.val_type {
                        bucket.push(live.slot);
                        found = true;
                        break;
                    }
                }
                if !found {
                    free_slots.push((live.val_type, vec![live.slot]));
                }
            } else {
                next_active.push(live);
            }
        }
        active = next_active;

        let mut reused_slot = None;
        for (bucket_ty, bucket) in &mut free_slots {
            if *bucket_ty == val_type {
                reused_slot = bucket.pop();
                break;
            }
        }
        let slot = if let Some(slot) = reused_slot {
            slot
        } else {
            let slot = next_local;
            next_local += 1;
            declared_locals.push(val_type);
            slot
        };
        slots.insert(interval.value, slot);
        active.push(ActiveInterval {
            end: interval.end,
            slot,
            val_type,
        });
    }

    Ok((slots, declared_locals))
}

fn instruction_operands(instruction: &IrInstruction) -> Vec<ValueId> {
    match instruction {
        IrInstruction::Param(_)
        | IrInstruction::Number { .. }
        | IrInstruction::Unit
        | IrInstruction::Bool(_)
        | IrInstruction::Null { .. }
        | IrInstruction::String(_)
        | IrInstruction::Bytes(_) => Vec::new(),
        IrInstruction::IsNull { value, .. } => vec![*value],
        IrInstruction::ToString { value, .. } => vec![*value],
        IrInstruction::Cast { value, .. } => vec![*value],
        IrInstruction::Binary { left, right, .. } => vec![*left, *right],
        IrInstruction::MathIntrinsic { args, .. } => args.clone(),
        IrInstruction::Print { value } => vec![*value],
        IrInstruction::Call { args, .. } => args.clone(),
        IrInstruction::CallValue { callee, args, .. } => {
            let mut out = Vec::with_capacity(args.len() + 1);
            out.extend(args.iter().copied());
            out.push(*callee);
            out
        }
        IrInstruction::CoroutineCreate { callee, .. } => vec![*callee],
        IrInstruction::CoroutineResume { coroutine, .. }
        | IrInstruction::CoroutineResumeTagged { coroutine, .. }
        | IrInstruction::CoroutineClose { coroutine, .. } => vec![*coroutine],
        IrInstruction::Closure { captures, .. } => captures.clone(),
        IrInstruction::ArrayNew { elements, .. } => elements.clone(),
        IrInstruction::ArrayGet { array, index, .. } => vec![*array, *index],
        IrInstruction::ArraySet {
            array,
            index,
            value,
            ..
        } => vec![*array, *index, *value],
        IrInstruction::ArrayLen { array } => vec![*array],
        IrInstruction::BytesGet { bytes, index } => vec![*bytes, *index],
        IrInstruction::BytesLen { bytes } => vec![*bytes],
        IrInstruction::StructNew { fields, .. } => fields.clone(),
        IrInstruction::StructGet { base, .. } => vec![*base],
        IrInstruction::StructSet { base, value, .. } => vec![*base, *value],
        IrInstruction::PackMulti { values, .. } => values.clone(),
        IrInstruction::MultiGet { value, .. } => vec![*value],
        IrInstruction::Phi(_) => Vec::new(),
    }
}

fn terminator_operands(terminator: &Terminator) -> Vec<ValueId> {
    match terminator {
        Terminator::Jump(_) | Terminator::Unreachable { .. } => Vec::new(),
        Terminator::Branch { condition, .. } => vec![*condition],
        Terminator::CoroutineYield { value, .. } => vec![*value],
        Terminator::Return(value) => vec![*value],
    }
}

fn instruction_use_requires_local(instruction: &IrInstruction) -> bool {
    matches!(
        instruction,
        IrInstruction::Binary {
            op: BinaryOp::FloorDiv | BinaryOp::Mod,
            ..
        } | IrInstruction::ArrayGet { .. }
            | IrInstruction::ArraySet { .. }
            | IrInstruction::StructGet { .. }
            | IrInstruction::StructSet { .. }
    )
}

fn instruction_can_consume_stack_value(instruction: &IrInstruction, value: ValueId) -> bool {
    match instruction {
        IrInstruction::Param(_)
        | IrInstruction::Number { .. }
        | IrInstruction::Unit
        | IrInstruction::Bool(_)
        | IrInstruction::Null { .. }
        | IrInstruction::String(_)
        | IrInstruction::Bytes(_) => false,
        IrInstruction::IsNull { value: tested, .. } => *tested == value,
        IrInstruction::ToString { .. } => false,
        IrInstruction::Cast { value: source, .. } => *source == value,
        IrInstruction::Binary { left, .. } => *left == value,
        IrInstruction::MathIntrinsic { args, .. } => args.first().copied() == Some(value),
        IrInstruction::Print { value: printed } => *printed == value,
        IrInstruction::Call { args, .. } => args.first().copied() == Some(value),
        IrInstruction::CallValue { .. } => false,
        IrInstruction::CoroutineCreate { .. } => false,
        IrInstruction::CoroutineResume { coroutine, .. }
        | IrInstruction::CoroutineResumeTagged { coroutine, .. }
        | IrInstruction::CoroutineClose { coroutine, .. } => *coroutine == value,
        IrInstruction::Closure { captures, .. } => captures.first().copied() == Some(value),
        IrInstruction::ArrayNew { elements, .. } => elements.first().copied() == Some(value),
        IrInstruction::ArrayGet { .. } | IrInstruction::ArraySet { .. } => false,
        IrInstruction::ArrayLen { array } => *array == value,
        IrInstruction::BytesGet { .. } => false,
        IrInstruction::BytesLen { bytes } => *bytes == value,
        IrInstruction::StructNew { fields, .. } => fields.first().copied() == Some(value),
        IrInstruction::StructGet { base, .. } => *base == value,
        IrInstruction::StructSet { base, .. } => *base == value,
        IrInstruction::PackMulti { values, .. } => values.first().copied() == Some(value),
        IrInstruction::MultiGet { value: source, .. } => *source == value,
        IrInstruction::Phi(_) => false,
    }
}

pub(crate) fn emit_value_operand(
    out: &mut Function,
    local_plan: &LocalPlan,
    value: ValueId,
) -> Result<(), Diagnostic> {
    if local_plan.stack_values.contains(&value) {
        return Ok(());
    }
    if local_plan.unit_values.contains(&value) {
        return Ok(());
    }
    if let Some(slots) = local_plan.multi_slots.get(&value) {
        for &slot in slots {
            out.instruction(&Instruction::LocalGet(slot));
        }
        return Ok(());
    }
    out.instruction(&Instruction::LocalGet(local(local_plan, value)?));
    Ok(())
}

pub(crate) fn emit_value_store(
    out: &mut Function,
    local_plan: &LocalPlan,
    value: ValueId,
) -> Result<(), Diagnostic> {
    if local_plan.stack_values.contains(&value) {
        return Ok(());
    }
    if local_plan.unit_values.contains(&value) {
        return Ok(());
    }
    if let Some(slots) = local_plan.multi_slots.get(&value) {
        for &slot in slots.iter().rev() {
            out.instruction(&Instruction::LocalSet(slot));
        }
        return Ok(());
    }
    out.instruction(&Instruction::LocalSet(local(local_plan, value)?));
    Ok(())
}

pub(crate) fn local(local_plan: &LocalPlan, value: ValueId) -> Result<u32, Diagnostic> {
    local_plan
        .slots
        .get(&value)
        .copied()
        .ok_or_else(|| Diagnostic::new(format!("missing local slot for value {:?}", value)))
}
