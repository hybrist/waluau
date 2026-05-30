use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use waluau_ast::{BinaryOp, NumberLiteral, NumericType, Type};
use waluau_diagnostics::Diagnostic;
use waluau_ir::{
    BasicBlock, Function as IrFunction, Instruction as IrInstruction, Module, Terminator, ValueId,
};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, ElementSection, Elements, ExportKind, ExportSection,
    Function, FunctionSection, HeapType, Instruction, Module as WasmModule, RefType, StartSection,
    StorageType, TableSection, TableType, TypeSection, ValType,
};
use wasmparser::Validator;

pub fn emit(module: &Module) -> Result<Vec<u8>, Diagnostic> {
    let array_types = collect_array_types(module);
    let start_thunk = module.start;
    let function_type_count = module.functions.len() as u32 + u32::from(start_thunk.is_some());
    let array_registry =
        ArrayTypeRegistry::with_function_type_offset(&array_types, function_type_count);

    let signatures = module
        .functions
        .iter()
        .enumerate()
        .map(|(index, function)| {
            (
                function.name.clone(),
                FunctionSignature {
                    index: index as u32,
                    params: function.params.iter().map(|(_, ty)| ty.clone()).collect(),
                    result: function.return_type.clone(),
                },
            )
        })
        .collect::<HashMap<_, _>>();

    let mut wasm = WasmModule::new();
    let mut types = TypeSection::new();
    for function in &module.functions {
        let params = function
            .params
            .iter()
            .map(|(_, ty)| wasm_type(ty, &array_registry))
            .collect::<Result<Vec<_>, _>>()?;
        let results = [wasm_type(&function.return_type, &array_registry)?];
        types.ty().function(params, results);
    }
    if start_thunk.is_some() {
        types
            .ty()
            .function(Vec::<ValType>::new(), Vec::<ValType>::new());
    }
    for array_ty in &array_types {
        let element_ty = array_ty
            .element_type()
            .expect("array type must have element type");
        let storage = array_storage_type(&element_ty, &array_registry)?;
        types.ty().array(&storage, true);
    }

    let mut functions = FunctionSection::new();
    let mut tables = TableSection::new();
    let mut elements = ElementSection::new();
    let mut exports = ExportSection::new();
    let mut codes = CodeSection::new();
    for (index, function) in module.functions.iter().enumerate() {
        functions.function(index as u32);
        if function.name != "__waluau_top_level_init" {
            exports.export(&function.name, ExportKind::Func, index as u32);
        }
        codes.function(&emit_function(function, &signatures, &array_registry)?);
    }
    if let Some(start) = start_thunk {
        let thunk_index = module.functions.len() as u32;
        functions.function(thunk_index);
        let mut thunk = Function::new(Vec::new());
        thunk.instruction(&Instruction::Call(start as u32));
        thunk.instruction(&Instruction::Drop);
        thunk.instruction(&Instruction::End);
        codes.function(&thunk);
    }
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        table64: false,
        minimum: module.functions.len() as u64,
        maximum: Some(module.functions.len() as u64),
        shared: false,
    });
    let table_inits = (0..module.functions.len() as u32).collect::<Vec<_>>();
    elements.active(
        Some(0),
        &ConstExpr::i32_const(0),
        Elements::Functions(Cow::Owned(table_inits)),
    );

    wasm.section(&types);
    wasm.section(&functions);
    wasm.section(&tables);
    wasm.section(&exports);
    if start_thunk.is_some() {
        wasm.section(&StartSection {
            function_index: module.functions.len() as u32,
        });
    }
    wasm.section(&elements);
    wasm.section(&codes);

    let bytes = wasm.finish();
    Validator::new()
        .validate_all(&bytes)
        .map_err(|err| Diagnostic::new(format!("emitted invalid wasm: {err}")))?;
    Ok(bytes)
}

struct ArrayTypeRegistry {
    indices: HashMap<String, u32>,
}

impl ArrayTypeRegistry {
    fn with_function_type_offset(array_types: &[Type], function_type_count: u32) -> Self {
        let indices = array_types
            .iter()
            .enumerate()
            .map(|(offset, array_ty)| (type_key(array_ty), function_type_count + offset as u32))
            .collect();
        Self { indices }
    }

    fn index(&self, array_ty: &Type) -> Result<u32, Diagnostic> {
        self.indices
            .get(&type_key(array_ty))
            .copied()
            .ok_or_else(|| Diagnostic::new(format!("missing wasm array type for {array_ty}")))
    }
}

fn type_key(ty: &Type) -> String {
    ty.to_string()
}

fn collect_array_types(module: &Module) -> Vec<Type> {
    let mut seen = BTreeSet::new();
    let mut types = Vec::new();
    for function in &module.functions {
        for (_, ty) in &function.params {
            insert_array_type(ty, &mut seen, &mut types);
        }
        insert_array_type(&function.return_type, &mut seen, &mut types);
        for block in function.blocks.values() {
            for (_, instruction) in &block.instructions {
                collect_array_types_from_instruction(instruction, &mut seen, &mut types);
            }
        }
    }
    types.sort_by_key(array_type_depth);
    types
}

fn array_type_depth(ty: &Type) -> usize {
    match ty {
        Type::Array(element) => 1 + array_type_depth(element),
        _ => 0,
    }
}

fn insert_array_type(ty: &Type, seen: &mut BTreeSet<String>, out: &mut Vec<Type>) {
    if let Type::Array(element) = ty {
        insert_array_type(element, seen, out);
        if seen.insert(type_key(ty)) {
            out.push(ty.clone());
        }
    }
}

fn collect_array_types_from_instruction(
    instruction: &IrInstruction,
    seen: &mut BTreeSet<String>,
    out: &mut Vec<Type>,
) {
    match instruction {
        IrInstruction::ArrayNew { element_ty, .. } => {
            insert_array_type(&Type::Array(Box::new(element_ty.clone())), seen, out);
        }
        IrInstruction::ArrayGet { element_ty, .. } | IrInstruction::ArraySet { element_ty, .. } => {
            insert_array_type(&Type::Array(Box::new(element_ty.clone())), seen, out);
        }
        IrInstruction::ArrayLen { .. } => {}
        _ => {}
    }
}

fn array_storage_type(
    element_ty: &Type,
    registry: &ArrayTypeRegistry,
) -> Result<StorageType, Diagnostic> {
    match element_ty {
        Type::Numeric(NumericType::I32 | NumericType::U32) => Ok(StorageType::Val(ValType::I32)),
        Type::Numeric(NumericType::I64 | NumericType::U64) => Ok(StorageType::Val(ValType::I64)),
        Type::Numeric(NumericType::F32) => Ok(StorageType::Val(ValType::F32)),
        Type::Numeric(NumericType::F64) => Ok(StorageType::Val(ValType::F64)),
        Type::Bool => Ok(StorageType::Val(ValType::I32)),
        Type::Array(_) => {
            let index = registry.index(element_ty)?;
            Ok(StorageType::Val(ValType::Ref(RefType {
                nullable: true,
                heap_type: HeapType::Concrete(index),
            })))
        }
        Type::Multi(_) => Err(Diagnostic::new(
            "multi-value types are not supported in array storage yet",
        )),
        Type::Function { .. } => unreachable!(),
    }
}

#[derive(Clone)]
struct FunctionSignature {
    index: u32,
    params: Vec<Type>,
    result: Type,
}

fn emit_function(
    function: &IrFunction,
    signatures: &HashMap<String, FunctionSignature>,
    array_registry: &ArrayTypeRegistry,
) -> Result<Function, Diagnostic> {
    let value_types = infer_value_types(function, signatures)?;
    let local_plan = build_local_plan(function, &value_types)?;
    let locals = compress_locals(
        local_plan
            .extra_locals
            .iter()
            .map(|(_, ty)| wasm_type(ty, array_registry))
            .collect::<Result<Vec<_>, _>>()?,
    );
    let mut out = Function::new(locals);
    if try_emit_structured_fast_path(
        &mut out,
        function,
        signatures,
        &value_types,
        &local_plan,
        array_registry,
    )? {
        out.instruction(&Instruction::End);
        return Ok(out);
    }

    let pc_local = local_plan.pc_local;
    out.instruction(&Instruction::I32Const(function.entry.0 as i32));
    out.instruction(&Instruction::LocalSet(pc_local));
    out.instruction(&Instruction::Loop(BlockType::Empty));

    for block in function.blocks.values() {
        out.instruction(&Instruction::LocalGet(pc_local));
        out.instruction(&Instruction::I32Const(block.id.0 as i32));
        out.instruction(&Instruction::I32Eq);
        out.instruction(&Instruction::If(BlockType::Empty));
        emit_block(
            &mut out,
            function,
            block,
            signatures,
            &value_types,
            &local_plan,
            array_registry,
        )?;
        out.instruction(&Instruction::End);
    }

    out.instruction(&Instruction::Unreachable);
    out.instruction(&Instruction::End);
    out.instruction(&Instruction::Unreachable);
    out.instruction(&Instruction::End);
    Ok(out)
}

fn try_emit_structured_fast_path(
    out: &mut Function,
    function: &IrFunction,
    signatures: &HashMap<String, FunctionSignature>,
    value_types: &BTreeMap<ValueId, Type>,
    local_plan: &LocalPlan,
    array_registry: &ArrayTypeRegistry,
) -> Result<bool, Diagnostic> {
    if function.blocks.len() == 3 {
        let entry = function
            .blocks
            .get(&function.entry)
            .ok_or_else(|| Diagnostic::new("missing entry block"))?;
        let Terminator::Branch {
            condition,
            then_block,
            else_block,
        } = entry.terminator
        else {
            return Ok(false);
        };
        let then_bb = function.blocks.get(&then_block);
        let else_bb = function.blocks.get(&else_block);
        if then_bb.is_some_and(|b| matches!(b.terminator, Terminator::Return(_)))
            && else_bb.is_some_and(|b| matches!(b.terminator, Terminator::Return(_)))
        {
            emit_block_instructions(out, entry, signatures, local_plan, array_registry)?;
            out.instruction(&Instruction::LocalGet(local(local_plan, condition)?));
            out.instruction(&Instruction::If(BlockType::Empty));
            emit_phi_copies(out, function, entry.id, then_block, local_plan)?;
            emit_block(
                out,
                function,
                then_bb.unwrap(),
                signatures,
                value_types,
                local_plan,
                array_registry,
            )?;
            out.instruction(&Instruction::Else);
            emit_phi_copies(out, function, entry.id, else_block, local_plan)?;
            emit_block(
                out,
                function,
                else_bb.unwrap(),
                signatures,
                value_types,
                local_plan,
                array_registry,
            )?;
            out.instruction(&Instruction::End);
            return Ok(true);
        }
    }

    if function.blocks.len() == 4 {
        let entry = function
            .blocks
            .get(&function.entry)
            .ok_or_else(|| Diagnostic::new("missing entry block"))?;
        let Terminator::Jump(first_target) = entry.terminator else {
            return Ok(false);
        };
        let second = function
            .blocks
            .get(&first_target)
            .ok_or_else(|| Diagnostic::new("missing loop header/check block"))?;
        if let Terminator::Branch {
            condition,
            then_block,
            else_block,
        } = second.terminator
        {
            let then_bb = function.blocks.get(&then_block);
            let else_bb = function.blocks.get(&else_block);
            // while: header -> body/exit with body jumping back to header.
            if then_bb
                .is_some_and(|b| matches!(b.terminator, Terminator::Jump(t) if t == second.id))
                && else_bb.is_some_and(|b| matches!(b.terminator, Terminator::Return(_)))
            {
                emit_block_instructions(out, entry, signatures, local_plan, array_registry)?;
                emit_phi_copies(out, function, entry.id, second.id, local_plan)?;
                out.instruction(&Instruction::Block(BlockType::Empty));
                out.instruction(&Instruction::Loop(BlockType::Empty));
                emit_block_instructions(out, second, signatures, local_plan, array_registry)?;
                out.instruction(&Instruction::LocalGet(local(local_plan, condition)?));
                out.instruction(&Instruction::I32Eqz);
                out.instruction(&Instruction::BrIf(1));
                emit_phi_copies(out, function, second.id, then_block, local_plan)?;
                emit_block_instructions(
                    out,
                    then_bb.expect("checked above"),
                    signatures,
                    local_plan,
                    array_registry,
                )?;
                emit_phi_copies(out, function, then_block, second.id, local_plan)?;
                out.instruction(&Instruction::Br(0));
                out.instruction(&Instruction::End);
                out.instruction(&Instruction::End);
                emit_phi_copies(out, function, second.id, else_block, local_plan)?;
                emit_block(
                    out,
                    function,
                    else_bb.expect("checked above"),
                    signatures,
                    value_types,
                    local_plan,
                    array_registry,
                )?;
                return Ok(true);
            }
            // repeat-until: check -> exit/body with body eventually jumping to check.
            if then_bb.is_some_and(|b| matches!(b.terminator, Terminator::Return(_)))
                && else_bb
                    .is_some_and(|b| matches!(b.terminator, Terminator::Jump(t) if t == second.id))
            {
                let body = function
                    .blocks
                    .values()
                    .find(|b| {
                        matches!(b.terminator, Terminator::Jump(t) if t == second.id)
                            && b.id != second.id
                    })
                    .ok_or_else(|| {
                        Diagnostic::new(
                            "unsupported repeat-until CFG shape for structured wasm emission",
                        )
                    })?;
                emit_block_instructions(out, entry, signatures, local_plan, array_registry)?;
                emit_phi_copies(out, function, entry.id, body.id, local_plan)?;
                out.instruction(&Instruction::Block(BlockType::Empty));
                out.instruction(&Instruction::Loop(BlockType::Empty));
                emit_block_instructions(out, body, signatures, local_plan, array_registry)?;
                emit_phi_copies(out, function, body.id, second.id, local_plan)?;
                emit_block_instructions(out, second, signatures, local_plan, array_registry)?;
                out.instruction(&Instruction::LocalGet(local(local_plan, condition)?));
                out.instruction(&Instruction::BrIf(1));
                emit_phi_copies(out, function, second.id, body.id, local_plan)?;
                out.instruction(&Instruction::Br(0));
                out.instruction(&Instruction::End);
                out.instruction(&Instruction::End);
                emit_phi_copies(out, function, second.id, then_block, local_plan)?;
                emit_block(
                    out,
                    function,
                    then_bb.expect("checked above"),
                    signatures,
                    value_types,
                    local_plan,
                    array_registry,
                )?;
                return Ok(true);
            }
        }
    }
    Ok(false)
}

struct LocalPlan {
    slots: BTreeMap<ValueId, u32>,
    extra_locals: Vec<(ValueId, Type)>,
    pc_local: u32,
}

fn build_local_plan(
    function: &IrFunction,
    value_types: &BTreeMap<ValueId, Type>,
) -> Result<LocalPlan, Diagnostic> {
    let mut slots = BTreeMap::new();
    let mut extra_locals = Vec::new();
    let mut next_local = function.params.len() as u32;

    for block in function.blocks.values() {
        for (value, instruction) in &block.instructions {
            if let IrInstruction::Param(index) = instruction {
                slots.insert(*value, *index as u32);
                continue;
            }

            let ty = value_types
                .get(value)
                .cloned()
                .ok_or_else(|| Diagnostic::new(format!("missing type for value {:?}", value)))?;
            slots.insert(*value, next_local);
            extra_locals.push((*value, ty));
            next_local += 1;
        }
    }

    let pc_local = next_local;
    extra_locals.push((ValueId(usize::MAX), Type::Numeric(NumericType::I32)));
    Ok(LocalPlan {
        slots,
        extra_locals,
        pc_local,
    })
}

fn infer_value_types(
    function: &IrFunction,
    signatures: &HashMap<String, FunctionSignature>,
) -> Result<BTreeMap<ValueId, Type>, Diagnostic> {
    let mut types = BTreeMap::new();

    for block in function.blocks.values() {
        for (value, instruction) in &block.instructions {
            let ty = match instruction {
                IrInstruction::Param(index) => function.params[*index].1.clone(),
                IrInstruction::Number { ty, .. } => Type::Numeric(*ty),
                IrInstruction::Bool(_) => Type::Bool,
                IrInstruction::Cast { to, .. } => to.clone(),
                IrInstruction::Binary { result_ty, .. } => result_ty.clone(),
                IrInstruction::Call { name, .. } => signatures
                    .get(name)
                    .ok_or_else(|| {
                        Diagnostic::new(format!("unknown function '{name}' during wasm emission"))
                    })?
                    .result
                    .clone(),
                IrInstruction::CallValue { return_type, .. } => return_type.clone(),
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
                let Some((_, first)) = incoming.first() else {
                    return Err(Diagnostic::new(format!(
                        "phi {:?} has no incoming values",
                        value
                    )));
                };
                let Some(first_ty) = types.get(first).cloned() else {
                    continue;
                };
                if incoming
                    .iter()
                    .all(|(_, incoming)| types.get(incoming).cloned() == Some(first_ty.clone()))
                {
                    types.insert(*value, first_ty);
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

fn emit_block(
    out: &mut Function,
    function: &IrFunction,
    block: &BasicBlock,
    signatures: &HashMap<String, FunctionSignature>,
    value_types: &BTreeMap<ValueId, Type>,
    local_plan: &LocalPlan,
    array_registry: &ArrayTypeRegistry,
) -> Result<(), Diagnostic> {
    emit_block_instructions(out, block, signatures, local_plan, array_registry)?;
    match &block.terminator {
        Terminator::Jump(target) => {
            emit_phi_copies(out, function, block.id, *target, local_plan)?;
            out.instruction(&Instruction::I32Const(target.0 as i32));
            out.instruction(&Instruction::LocalSet(local_plan.pc_local));
            out.instruction(&Instruction::Br(1));
        }
        Terminator::Branch {
            condition,
            then_block,
            else_block,
        } => {
            out.instruction(&Instruction::LocalGet(local(local_plan, *condition)?));
            out.instruction(&Instruction::If(BlockType::Empty));
            emit_phi_copies(out, function, block.id, *then_block, local_plan)?;
            out.instruction(&Instruction::I32Const(then_block.0 as i32));
            out.instruction(&Instruction::LocalSet(local_plan.pc_local));
            out.instruction(&Instruction::Else);
            emit_phi_copies(out, function, block.id, *else_block, local_plan)?;
            out.instruction(&Instruction::I32Const(else_block.0 as i32));
            out.instruction(&Instruction::LocalSet(local_plan.pc_local));
            out.instruction(&Instruction::End);
            out.instruction(&Instruction::Br(1));
        }
        Terminator::Return(value) => {
            let _ = value_types.get(value).ok_or_else(|| {
                Diagnostic::new(format!("missing type for return value {:?}", value))
            })?;
            out.instruction(&Instruction::LocalGet(local(local_plan, *value)?));
            out.instruction(&Instruction::Return);
        }
        Terminator::Unreachable => {
            out.instruction(&Instruction::Unreachable);
        }
    }
    Ok(())
}

fn emit_block_instructions(
    out: &mut Function,
    block: &BasicBlock,
    signatures: &HashMap<String, FunctionSignature>,
    local_plan: &LocalPlan,
    array_registry: &ArrayTypeRegistry,
) -> Result<(), Diagnostic> {
    for (value, instruction) in &block.instructions {
        match instruction {
            IrInstruction::Param(_) | IrInstruction::Phi(_) => {}
            IrInstruction::Number { ty, literal } => {
                emit_numeric_const(out, *ty, literal)?;
                out.instruction(&Instruction::LocalSet(local(local_plan, *value)?));
            }
            IrInstruction::Bool(flag) => {
                out.instruction(&Instruction::I32Const(i32::from(*flag)));
                out.instruction(&Instruction::LocalSet(local(local_plan, *value)?));
            }
            IrInstruction::Cast {
                value: source,
                from,
                to,
            } => {
                out.instruction(&Instruction::LocalGet(local(local_plan, *source)?));
                emit_cast(out, from.clone(), to.clone())?;
                out.instruction(&Instruction::LocalSet(local(local_plan, *value)?));
            }
            IrInstruction::Binary {
                op,
                left,
                right,
                operand_ty,
                result_ty,
            } => {
                let left_local = local(local_plan, *left)?;
                let right_local = local(local_plan, *right)?;
                if matches!(op, BinaryOp::FloorDiv | BinaryOp::Mod) {
                    emit_floor_or_mod(out, *op, operand_ty.clone(), left_local, right_local)?;
                } else {
                    out.instruction(&Instruction::LocalGet(left_local));
                    out.instruction(&Instruction::LocalGet(right_local));
                    emit_binary(out, *op, operand_ty.clone(), result_ty.clone())?;
                }
                out.instruction(&Instruction::LocalSet(local(local_plan, *value)?));
            }
            IrInstruction::Call { name, args } => {
                for arg in args {
                    out.instruction(&Instruction::LocalGet(local(local_plan, *arg)?));
                }
                let callee = signatures.get(name).ok_or_else(|| {
                    Diagnostic::new(format!("unknown function '{name}' during wasm emission"))
                })?;
                out.instruction(&Instruction::Call(callee.index));
                out.instruction(&Instruction::LocalSet(local(local_plan, *value)?));
            }
            IrInstruction::CallValue {
                callee,
                args,
                params,
                return_type,
            } => {
                for arg in args {
                    out.instruction(&Instruction::LocalGet(local(local_plan, *arg)?));
                }
                out.instruction(&Instruction::LocalGet(local(local_plan, *callee)?));
                let type_index = find_function_type_index(signatures, params, return_type)?;
                out.instruction(&Instruction::CallIndirect {
                    type_index,
                    table_index: 0,
                });
                out.instruction(&Instruction::LocalSet(local(local_plan, *value)?));
            }
            IrInstruction::Closure {
                name,
                captures,
                params,
                return_type,
            } => {
                if !captures.is_empty() {
                    return Err(Diagnostic::new(
                        "wasm backend does not yet support closures with captures",
                    ));
                }
                let callee = signatures.get(name).ok_or_else(|| {
                    Diagnostic::new(format!("unknown function '{name}' during wasm emission"))
                })?;
                let _ = find_function_type_index(signatures, params, return_type)?;
                out.instruction(&Instruction::I32Const(callee.index as i32));
                out.instruction(&Instruction::LocalSet(local(local_plan, *value)?));
            }
            IrInstruction::ArrayNew {
                element_ty,
                elements,
            } => {
                for element in elements {
                    out.instruction(&Instruction::LocalGet(local(local_plan, *element)?));
                }
                let array_ty = Type::Array(Box::new(element_ty.clone()));
                let array_type_index = array_registry.index(&array_ty)?;
                out.instruction(&Instruction::ArrayNewFixed {
                    array_type_index,
                    array_size: elements.len() as u32,
                });
                out.instruction(&Instruction::LocalSet(local(local_plan, *value)?));
            }
            IrInstruction::ArrayGet {
                array,
                index,
                element_ty,
            } => {
                let array_local = local(local_plan, *array)?;
                let index_local = local(local_plan, *index)?;
                let array_ty = Type::Array(Box::new(element_ty.clone()));
                let array_type_index = array_registry.index(&array_ty)?;
                emit_bounds_check(out, array_local, index_local);
                out.instruction(&Instruction::LocalGet(array_local));
                out.instruction(&Instruction::LocalGet(index_local));
                out.instruction(&Instruction::ArrayGet(array_type_index));
                out.instruction(&Instruction::LocalSet(local(local_plan, *value)?));
            }
            IrInstruction::ArraySet {
                array,
                index,
                value: stored,
                element_ty,
            } => {
                let array_local = local(local_plan, *array)?;
                let index_local = local(local_plan, *index)?;
                let array_ty = Type::Array(Box::new(element_ty.clone()));
                let array_type_index = array_registry.index(&array_ty)?;
                emit_bounds_check(out, array_local, index_local);
                out.instruction(&Instruction::LocalGet(array_local));
                out.instruction(&Instruction::LocalGet(index_local));
                out.instruction(&Instruction::LocalGet(local(local_plan, *stored)?));
                out.instruction(&Instruction::ArraySet(array_type_index));
            }
            IrInstruction::ArrayLen { array } => {
                out.instruction(&Instruction::LocalGet(local(local_plan, *array)?));
                out.instruction(&Instruction::ArrayLen);
                out.instruction(&Instruction::LocalSet(local(local_plan, *value)?));
            }
            IrInstruction::PackMulti { .. } | IrInstruction::MultiGet { .. } => {
                return Err(Diagnostic::new(
                    "wasm backend does not yet support lowering multi-value tuple instructions",
                ));
            }
        }
    }

    Ok(())
}

fn emit_phi_copies(
    out: &mut Function,
    function: &IrFunction,
    source: waluau_ir::BlockId,
    target: waluau_ir::BlockId,
    local_plan: &LocalPlan,
) -> Result<(), Diagnostic> {
    let target_block = function
        .blocks
        .get(&target)
        .ok_or_else(|| Diagnostic::new(format!("unknown target block {:?}", target)))?;

    let mut copies = Vec::new();
    for (value, instruction) in &target_block.instructions {
        let IrInstruction::Phi(incoming) = instruction else {
            continue;
        };
        let incoming_value = incoming
            .iter()
            .find_map(|(pred, value)| (*pred == source).then_some(*value))
            .ok_or_else(|| {
                Diagnostic::new(format!(
                    "missing phi input from {:?} to {:?}",
                    source, target
                ))
            })?;
        copies.push((
            local(local_plan, *value)?,
            local(local_plan, incoming_value)?,
        ));
    }

    for (_, src) in &copies {
        out.instruction(&Instruction::LocalGet(*src));
    }
    for (dst, _) in copies.iter().rev() {
        out.instruction(&Instruction::LocalSet(*dst));
    }
    Ok(())
}

fn emit_numeric_const(
    out: &mut Function,
    ty: NumericType,
    literal: &NumberLiteral,
) -> Result<(), Diagnostic> {
    match ty {
        NumericType::U32 => {
            out.instruction(&Instruction::I32Const(
                parse_numeric_literal::<u32>(literal, "u32")? as i32,
            ));
        }
        NumericType::I32 => {
            out.instruction(&Instruction::I32Const(parse_numeric_literal::<i32>(
                literal, "i32",
            )?));
        }
        NumericType::U64 => {
            out.instruction(&Instruction::I64Const(
                parse_numeric_literal::<u64>(literal, "u64")? as i64,
            ));
        }
        NumericType::I64 => {
            out.instruction(&Instruction::I64Const(parse_numeric_literal::<i64>(
                literal, "i64",
            )?));
        }
        NumericType::F32 => {
            out.instruction(&Instruction::F32Const(parse_numeric_literal::<f32>(
                literal, "f32",
            )?));
        }
        NumericType::F64 => {
            out.instruction(&Instruction::F64Const(parse_numeric_literal::<f64>(
                literal, "f64",
            )?));
        }
    }

    Ok(())
}

fn parse_numeric_literal<T>(literal: &NumberLiteral, ty_name: &str) -> Result<T, Diagnostic>
where
    T: std::str::FromStr,
{
    literal.raw.parse::<T>().map_err(|_| {
        Diagnostic::new(format!(
            "invalid {ty_name} numeric literal during wasm emission"
        ))
    })
}

fn emit_cast(out: &mut Function, from: Type, to: Type) -> Result<(), Diagnostic> {
    if from == to {
        return Ok(());
    }

    let (Type::Numeric(from), Type::Numeric(to)) = (from, to) else {
        return Err(Diagnostic::new(
            "only numeric casts are supported during wasm emission",
        ));
    };

    match (from, to) {
        (NumericType::U32 | NumericType::I32, NumericType::U32 | NumericType::I32)
        | (NumericType::F32, NumericType::F32)
        | (NumericType::F64, NumericType::F64)
        | (NumericType::U64 | NumericType::I64, NumericType::U64 | NumericType::I64) => {}
        (NumericType::U32, NumericType::U64) => {
            out.instruction(&Instruction::I64ExtendI32U);
        }
        (NumericType::I32, NumericType::I64) => {
            out.instruction(&Instruction::I64ExtendI32S);
        }
        (NumericType::U32, NumericType::I64) => {
            out.instruction(&Instruction::I64ExtendI32U);
        }
        (NumericType::I32, NumericType::U64) => {
            out.instruction(&Instruction::I64ExtendI32S);
        }
        (NumericType::U64 | NumericType::I64, NumericType::U32 | NumericType::I32) => {
            out.instruction(&Instruction::I32WrapI64);
        }
        (NumericType::F32, NumericType::F64) => {
            out.instruction(&Instruction::F64PromoteF32);
        }
        (NumericType::F64, NumericType::F32) => {
            out.instruction(&Instruction::F32DemoteF64);
        }
        (NumericType::U32, NumericType::F32) => {
            out.instruction(&Instruction::F32ConvertI32U);
        }
        (NumericType::I32, NumericType::F32) => {
            out.instruction(&Instruction::F32ConvertI32S);
        }
        (NumericType::U64, NumericType::F32) => {
            out.instruction(&Instruction::F32ConvertI64U);
        }
        (NumericType::I64, NumericType::F32) => {
            out.instruction(&Instruction::F32ConvertI64S);
        }
        (NumericType::U32, NumericType::F64) => {
            out.instruction(&Instruction::F64ConvertI32U);
        }
        (NumericType::I32, NumericType::F64) => {
            out.instruction(&Instruction::F64ConvertI32S);
        }
        (NumericType::U64, NumericType::F64) => {
            out.instruction(&Instruction::F64ConvertI64U);
        }
        (NumericType::I64, NumericType::F64) => {
            out.instruction(&Instruction::F64ConvertI64S);
        }
        (NumericType::F32, NumericType::U32) => {
            out.instruction(&Instruction::I32TruncF32U);
        }
        (NumericType::F32, NumericType::I32) => {
            out.instruction(&Instruction::I32TruncF32S);
        }
        (NumericType::F64, NumericType::U32) => {
            out.instruction(&Instruction::I32TruncF64U);
        }
        (NumericType::F64, NumericType::I32) => {
            out.instruction(&Instruction::I32TruncF64S);
        }
        (NumericType::F32, NumericType::U64) => {
            out.instruction(&Instruction::I64TruncF32U);
        }
        (NumericType::F32, NumericType::I64) => {
            out.instruction(&Instruction::I64TruncF32S);
        }
        (NumericType::F64, NumericType::U64) => {
            out.instruction(&Instruction::I64TruncF64U);
        }
        (NumericType::F64, NumericType::I64) => {
            out.instruction(&Instruction::I64TruncF64S);
        }
    }

    Ok(())
}

fn emit_binary(
    out: &mut Function,
    op: BinaryOp,
    operand_ty: Type,
    _result_ty: Type,
) -> Result<(), Diagnostic> {
    match op {
        BinaryOp::Add => match operand_ty {
            Type::Numeric(NumericType::U32 | NumericType::I32) => {
                out.instruction(&Instruction::I32Add);
            }
            Type::Numeric(NumericType::U64 | NumericType::I64) => {
                out.instruction(&Instruction::I64Add);
            }
            Type::Numeric(NumericType::F32) => {
                out.instruction(&Instruction::F32Add);
            }
            Type::Numeric(NumericType::F64) => {
                out.instruction(&Instruction::F64Add);
            }
            Type::Bool => {
                return Err(Diagnostic::new(
                    "bool add is not supported during wasm emission",
                ));
            }
            Type::Array(_) => unreachable!(),
            Type::Multi(_) => {
                return Err(Diagnostic::new(
                    "multi-value add is not supported during wasm emission",
                ));
            }
            Type::Function { .. } => unreachable!(),
        },
        BinaryOp::Sub => match operand_ty {
            Type::Numeric(NumericType::U32 | NumericType::I32) => {
                out.instruction(&Instruction::I32Sub);
            }
            Type::Numeric(NumericType::U64 | NumericType::I64) => {
                out.instruction(&Instruction::I64Sub);
            }
            Type::Numeric(NumericType::F32) => {
                out.instruction(&Instruction::F32Sub);
            }
            Type::Numeric(NumericType::F64) => {
                out.instruction(&Instruction::F64Sub);
            }
            Type::Bool => {
                return Err(Diagnostic::new(
                    "bool sub is not supported during wasm emission",
                ));
            }
            Type::Array(_) => unreachable!(),
            Type::Multi(_) => {
                return Err(Diagnostic::new(
                    "multi-value sub is not supported during wasm emission",
                ));
            }
            Type::Function { .. } => unreachable!(),
        },
        BinaryOp::Mul => match operand_ty {
            Type::Numeric(NumericType::U32 | NumericType::I32) => {
                out.instruction(&Instruction::I32Mul);
            }
            Type::Numeric(NumericType::U64 | NumericType::I64) => {
                out.instruction(&Instruction::I64Mul);
            }
            Type::Numeric(NumericType::F32) => {
                out.instruction(&Instruction::F32Mul);
            }
            Type::Numeric(NumericType::F64) => {
                out.instruction(&Instruction::F64Mul);
            }
            Type::Bool => {
                return Err(Diagnostic::new(
                    "bool mul is not supported during wasm emission",
                ));
            }
            Type::Array(_) => unreachable!(),
            Type::Multi(_) => {
                return Err(Diagnostic::new(
                    "multi-value mul is not supported during wasm emission",
                ));
            }
            Type::Function { .. } => unreachable!(),
        },
        BinaryOp::Div => match operand_ty {
            Type::Numeric(NumericType::U32) => {
                out.instruction(&Instruction::I32DivU);
            }
            Type::Numeric(NumericType::I32) => {
                out.instruction(&Instruction::I32DivS);
            }
            Type::Numeric(NumericType::U64) => {
                out.instruction(&Instruction::I64DivU);
            }
            Type::Numeric(NumericType::I64) => {
                out.instruction(&Instruction::I64DivS);
            }
            Type::Numeric(NumericType::F32) => {
                out.instruction(&Instruction::F32Div);
            }
            Type::Numeric(NumericType::F64) => {
                out.instruction(&Instruction::F64Div);
            }
            Type::Bool => {
                return Err(Diagnostic::new(
                    "bool div is not supported during wasm emission",
                ));
            }
            Type::Array(_) => unreachable!(),
            Type::Multi(_) => {
                return Err(Diagnostic::new(
                    "multi-value div is not supported during wasm emission",
                ));
            }
            Type::Function { .. } => unreachable!(),
        },
        BinaryOp::FloorDiv | BinaryOp::Mod => unreachable!("handled before stack binary emission"),
        BinaryOp::Eq => match operand_ty {
            Type::Numeric(NumericType::U32 | NumericType::I32) | Type::Bool => {
                out.instruction(&Instruction::I32Eq);
            }
            Type::Numeric(NumericType::U64 | NumericType::I64) => {
                out.instruction(&Instruction::I64Eq);
            }
            Type::Numeric(NumericType::F32) => {
                out.instruction(&Instruction::F32Eq);
            }
            Type::Numeric(NumericType::F64) => {
                out.instruction(&Instruction::F64Eq);
            }
            Type::Array(_) => unreachable!(),
            Type::Multi(_) => {
                return Err(Diagnostic::new(
                    "multi-value equality is not supported during wasm emission",
                ));
            }
            Type::Function { .. } => unreachable!(),
        },
        BinaryOp::Less => match operand_ty {
            Type::Numeric(NumericType::U32) => {
                out.instruction(&Instruction::I32LtU);
            }
            Type::Numeric(NumericType::I32) => {
                out.instruction(&Instruction::I32LtS);
            }
            Type::Numeric(NumericType::U64) => {
                out.instruction(&Instruction::I64LtU);
            }
            Type::Numeric(NumericType::I64) => {
                out.instruction(&Instruction::I64LtS);
            }
            Type::Numeric(NumericType::F32) => {
                out.instruction(&Instruction::F32Lt);
            }
            Type::Numeric(NumericType::F64) => {
                out.instruction(&Instruction::F64Lt);
            }
            Type::Bool => {
                return Err(Diagnostic::new(
                    "bool comparison is not supported during wasm emission",
                ));
            }
            Type::Array(_) => unreachable!(),
            Type::Multi(_) => {
                return Err(Diagnostic::new(
                    "multi-value comparison is not supported during wasm emission",
                ));
            }
            Type::Function { .. } => unreachable!(),
        },
        BinaryOp::Greater => match operand_ty {
            Type::Numeric(NumericType::U32) => {
                out.instruction(&Instruction::I32GtU);
            }
            Type::Numeric(NumericType::I32) => {
                out.instruction(&Instruction::I32GtS);
            }
            Type::Numeric(NumericType::U64) => {
                out.instruction(&Instruction::I64GtU);
            }
            Type::Numeric(NumericType::I64) => {
                out.instruction(&Instruction::I64GtS);
            }
            Type::Numeric(NumericType::F32) => {
                out.instruction(&Instruction::F32Gt);
            }
            Type::Numeric(NumericType::F64) => {
                out.instruction(&Instruction::F64Gt);
            }
            Type::Bool => {
                return Err(Diagnostic::new(
                    "bool comparison is not supported during wasm emission",
                ));
            }
            Type::Array(_) => unreachable!(),
            Type::Multi(_) => {
                return Err(Diagnostic::new(
                    "multi-value comparison is not supported during wasm emission",
                ));
            }
            Type::Function { .. } => unreachable!(),
        },
        BinaryOp::And => {
            out.instruction(&Instruction::I32And);
        }
        BinaryOp::Or => {
            out.instruction(&Instruction::I32Or);
        }
    }
    Ok(())
}

fn emit_floor_or_mod(
    out: &mut Function,
    op: BinaryOp,
    operand_ty: Type,
    left_local: u32,
    right_local: u32,
) -> Result<(), Diagnostic> {
    match (op, operand_ty) {
        (BinaryOp::FloorDiv, Type::Numeric(NumericType::U32)) => {
            emit_integer_div(out, left_local, right_local, false, 32)
        }
        (BinaryOp::FloorDiv, Type::Numeric(NumericType::U64)) => {
            emit_integer_div(out, left_local, right_local, false, 64)
        }
        (BinaryOp::FloorDiv, Type::Numeric(NumericType::I32)) => {
            emit_float_floor_div(out, left_local, right_local, NumericType::I32)
        }
        (BinaryOp::FloorDiv, Type::Numeric(NumericType::I64)) => {
            emit_float_floor_div(out, left_local, right_local, NumericType::I64)
        }
        (BinaryOp::FloorDiv, Type::Numeric(NumericType::F32)) => {
            emit_float_floor_div(out, left_local, right_local, NumericType::F32)
        }
        (BinaryOp::FloorDiv, Type::Numeric(NumericType::F64)) => {
            emit_float_floor_div(out, left_local, right_local, NumericType::F64)
        }
        (BinaryOp::Mod, Type::Numeric(NumericType::U32)) => {
            emit_integer_rem(out, left_local, right_local, false, 32)
        }
        (BinaryOp::Mod, Type::Numeric(NumericType::I32)) => {
            emit_float_mod(out, left_local, right_local, NumericType::I32)
        }
        (BinaryOp::Mod, Type::Numeric(NumericType::U64)) => {
            emit_integer_rem(out, left_local, right_local, false, 64)
        }
        (BinaryOp::Mod, Type::Numeric(NumericType::I64)) => {
            emit_float_mod(out, left_local, right_local, NumericType::I64)
        }
        (BinaryOp::Mod, Type::Numeric(NumericType::F32)) => {
            emit_float_mod(out, left_local, right_local, NumericType::F32)
        }
        (BinaryOp::Mod, Type::Numeric(NumericType::F64)) => {
            emit_float_mod(out, left_local, right_local, NumericType::F64)
        }
        (_, Type::Bool) => {
            return Err(Diagnostic::new(
                "bool floor/mod is not supported during wasm emission",
            ));
        }
        (_, Type::Array(_)) => unreachable!(),
        _ => unreachable!(),
    }
    Ok(())
}

fn emit_integer_div(out: &mut Function, left_local: u32, right_local: u32, signed: bool, bits: u8) {
    out.instruction(&Instruction::LocalGet(left_local));
    out.instruction(&Instruction::LocalGet(right_local));
    match (signed, bits) {
        (false, 32) => out.instruction(&Instruction::I32DivU),
        (true, 32) => out.instruction(&Instruction::I32DivS),
        (false, 64) => out.instruction(&Instruction::I64DivU),
        (true, 64) => out.instruction(&Instruction::I64DivS),
        _ => unreachable!(),
    };
}

fn emit_integer_rem(out: &mut Function, left_local: u32, right_local: u32, signed: bool, bits: u8) {
    out.instruction(&Instruction::LocalGet(left_local));
    out.instruction(&Instruction::LocalGet(right_local));
    match (signed, bits) {
        (false, 32) => out.instruction(&Instruction::I32RemU),
        (true, 32) => out.instruction(&Instruction::I32RemS),
        (false, 64) => out.instruction(&Instruction::I64RemU),
        (true, 64) => out.instruction(&Instruction::I64RemS),
        _ => unreachable!(),
    };
}

fn emit_float_floor_div(out: &mut Function, left_local: u32, right_local: u32, ty: NumericType) {
    emit_as_float(out, left_local, ty);
    emit_as_float(out, right_local, ty);
    match ty {
        NumericType::F32 => {
            out.instruction(&Instruction::F32Div);
            out.instruction(&Instruction::F32Floor);
        }
        _ => {
            out.instruction(&Instruction::F64Div);
            out.instruction(&Instruction::F64Floor);
        }
    }
    emit_from_float(out, ty);
}

fn emit_float_mod(out: &mut Function, left_local: u32, right_local: u32, ty: NumericType) {
    emit_as_float(out, left_local, ty);
    emit_as_float(out, left_local, ty);
    emit_as_float(out, right_local, ty);
    match ty {
        NumericType::F32 => {
            out.instruction(&Instruction::F32Div);
            out.instruction(&Instruction::F32Floor);
            emit_as_float(out, right_local, ty);
            out.instruction(&Instruction::F32Mul);
            out.instruction(&Instruction::F32Sub);
        }
        _ => {
            out.instruction(&Instruction::F64Div);
            out.instruction(&Instruction::F64Floor);
            emit_as_float(out, right_local, ty);
            out.instruction(&Instruction::F64Mul);
            out.instruction(&Instruction::F64Sub);
        }
    }
    emit_from_float(out, ty);
}

fn emit_as_float(out: &mut Function, local: u32, ty: NumericType) {
    out.instruction(&Instruction::LocalGet(local));
    match ty {
        NumericType::U32 => {
            out.instruction(&Instruction::F64ConvertI32U);
        }
        NumericType::I32 => {
            out.instruction(&Instruction::F64ConvertI32S);
        }
        NumericType::U64 => {
            out.instruction(&Instruction::F64ConvertI64U);
        }
        NumericType::I64 => {
            out.instruction(&Instruction::F64ConvertI64S);
        }
        NumericType::F32 | NumericType::F64 => {}
    }
}

fn emit_from_float(out: &mut Function, ty: NumericType) {
    match ty {
        NumericType::U32 => {
            out.instruction(&Instruction::I32TruncF64U);
        }
        NumericType::I32 => {
            out.instruction(&Instruction::I32TruncF64S);
        }
        NumericType::U64 => {
            out.instruction(&Instruction::I64TruncF64U);
        }
        NumericType::I64 => {
            out.instruction(&Instruction::I64TruncF64S);
        }
        NumericType::F32 | NumericType::F64 => {}
    }
}

fn emit_bounds_check(out: &mut Function, array_local: u32, index_local: u32) {
    out.instruction(&Instruction::Block(BlockType::Empty));
    out.instruction(&Instruction::LocalGet(index_local));
    out.instruction(&Instruction::I32Const(0));
    out.instruction(&Instruction::I32LtS);
    out.instruction(&Instruction::If(BlockType::Empty));
    out.instruction(&Instruction::Unreachable);
    out.instruction(&Instruction::End);
    out.instruction(&Instruction::LocalGet(index_local));
    out.instruction(&Instruction::LocalGet(array_local));
    out.instruction(&Instruction::ArrayLen);
    out.instruction(&Instruction::I32GeU);
    out.instruction(&Instruction::If(BlockType::Empty));
    out.instruction(&Instruction::Unreachable);
    out.instruction(&Instruction::End);
    out.instruction(&Instruction::End);
}

fn local(local_plan: &LocalPlan, value: ValueId) -> Result<u32, Diagnostic> {
    local_plan
        .slots
        .get(&value)
        .copied()
        .ok_or_else(|| Diagnostic::new(format!("missing local slot for value {:?}", value)))
}

fn wasm_type(ty: &Type, array_registry: &ArrayTypeRegistry) -> Result<ValType, Diagnostic> {
    match ty {
        Type::Bool | Type::Numeric(NumericType::U32 | NumericType::I32) => Ok(ValType::I32),
        Type::Numeric(NumericType::U64 | NumericType::I64) => Ok(ValType::I64),
        Type::Numeric(NumericType::F32) => Ok(ValType::F32),
        Type::Numeric(NumericType::F64) => Ok(ValType::F64),
        Type::Array(_) => {
            let index = array_registry.index(ty)?;
            Ok(ValType::Ref(RefType {
                nullable: true,
                heap_type: HeapType::Concrete(index),
            }))
        }
        Type::Multi(_) => Err(Diagnostic::new(
            "multi-value types are not supported in Wasm signatures yet",
        )),
        Type::Function { .. } => Ok(ValType::I32),
    }
}

fn find_function_type_index(
    signatures: &HashMap<String, FunctionSignature>,
    params: &[Type],
    return_type: &Type,
) -> Result<u32, Diagnostic> {
    signatures
        .values()
        .find(|signature| signature.params == params && signature.result == *return_type)
        .map(|signature| signature.index)
        .ok_or_else(|| {
            Diagnostic::new(format!(
                "no wasm function type found for indirect call signature ({}) -> {}",
                params
                    .iter()
                    .map(|ty| ty.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                return_type
            ))
        })
}

fn compress_locals(locals: Vec<ValType>) -> Vec<(u32, ValType)> {
    let mut compressed = Vec::new();
    for ty in locals {
        if let Some((count, last_ty)) = compressed.last_mut() {
            if *last_ty == ty {
                *count += 1;
                continue;
            }
        }

        compressed.push((1, ty));
    }
    compressed
}

#[cfg(test)]
mod tests {
    use wasmparser::Validator;
    use wasmprinter::print_bytes;

    #[test]
    fn emits_valid_wasm_for_scalar_program() {
        let source = r#"
            function entry(x: i32): i32
                return x + 1
            end
        "#;
        let program = waluau_parser::parse(source).expect("parse should succeed");
        let ir = waluau_ir::build(&program).expect("ir should succeed");
        let wasm = super::emit(&ir).expect("emit should succeed");
        Validator::new()
            .validate_all(&wasm)
            .expect("emitted module should validate");
    }

    #[test]
    fn emits_valid_wasm_for_array_program() {
        let source = r#"
            function score_count(): i32
                local scores: {number} = {100, 250, 300}
                local first: number = scores[0]
                scores[1] = first + 1
                return #scores
            end
        "#;
        let program = waluau_parser::parse(source).expect("parse should succeed");
        let ir = waluau_ir::build(&program).expect("ir should succeed");
        let wasm = super::emit(&ir).expect("emit should succeed");
        Validator::new()
            .validate_all(&wasm)
            .expect("emitted module should validate");
    }

    #[test]
    fn emits_valid_wasm_for_non_capturing_indirect_call() {
        let source = r#"
            function entry(x: i32): i32
                local f: (i32) -> i32 = function(y: i32): i32
                    return y + 1
                end
                return f(x)
            end
        "#;
        let program = waluau_parser::parse(source).expect("parse should succeed");
        let ir = waluau_ir::build(&program).expect("ir should succeed");
        let wasm = super::emit(&ir).expect("emit should succeed");
        Validator::new()
            .validate_all(&wasm)
            .expect("emitted module should validate");
    }

    #[test]
    fn rejects_capturing_closure_values() {
        let source = r#"
            function entry(x: i32): i32
                local f: (i32) -> i32 = function(y: i32): i32
                    return x + y
                end
                return f(1)
            end
        "#;
        let program = waluau_parser::parse(source).expect("parse should succeed");
        let ir = waluau_ir::build(&program).expect("ir should succeed");
        let err = super::emit(&ir).expect_err("capturing closures should be unsupported");
        assert_eq!(
            err.to_string(),
            "wasm backend does not yet support closures with captures"
        );
    }

    #[test]
    fn emits_structured_if_for_simple_branch() {
        let source = r#"
            function choose(x: i32, y: i32): i32
                if x > y then
                    return x
                else
                    return y
                end
            end
        "#;
        let program = waluau_parser::parse(source).expect("parse should succeed");
        let ir = waluau_ir::build(&program).expect("ir should succeed");
        let wasm = super::emit(&ir).expect("emit should succeed");
        let wat = print_bytes(&wasm).expect("wat should print");
        assert!(wat.contains(" if"));
        assert!(!wat.contains("i32.eq\n    if"));
    }

    #[test]
    fn emits_structured_loop_for_simple_while() {
        let source = r#"
            function sum_to(n: i32): i32
                local acc: i32 = 0
                local i: i32 = n
                while i > 0 do
                    acc = acc + i
                    i = i - 1
                end
                return acc
            end
        "#;
        let program = waluau_parser::parse(source).expect("parse should succeed");
        let ir = waluau_ir::build(&program).expect("ir should succeed");
        let wasm = super::emit(&ir).expect("emit should succeed");
        let wat = print_bytes(&wasm).expect("wat should print");
        assert!(wat.contains(" loop"));
        assert!(!wat.contains("i32.eq\n    if"));
    }
}
