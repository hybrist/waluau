use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use waluau_ast::{BinaryOp, NumberLiteral, NumericType, Type};
use waluau_diagnostics::Diagnostic;
use waluau_ir::{
    BasicBlock, Function as IrFunction, Instruction as IrInstruction, MathIntrinsic, Module,
    Terminator, ValueId,
};
use wasm_encoder::{
    AbstractHeapType, BlockType, CodeSection, ConstExpr, CustomSection, ElementSection, Elements,
    EntityType, ExportKind, ExportSection, Function, FunctionSection, HeapType, ImportSection,
    Instruction, Module as WasmModule, RefType, StartSection, StorageType, TableSection, TableType,
    TypeSection, ValType,
};
use wasmparser::{Validator, WasmFeatures};

pub mod host;

pub fn emit(module: &Module) -> Result<Vec<u8>, Diagnostic> {
    let array_types = collect_array_types(module);
    let string_constants = host::collect_string_constants(module);
    let start_thunk = module.start;
    let host_type_base = array_types.len() as u32;
    let user_type_base = host_type_base + host::HOST_TYPE_COUNT;
    // Array types come first in the type section (indices 0..N-1).
    let array_registry = ArrayTypeRegistry::with_function_type_offset(&array_types, 0);

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
    // Emit array types first so function types can reference them.
    for array_ty in &array_types {
        let element_ty = array_ty
            .element_type()
            .expect("array type must have element type");
        let storage = array_storage_type(&element_ty, &array_registry)?;
        types.ty().array(&storage, true);
    }
    // Host import function types for wasm:js-string builtins.
    types.ty().function(
        vec![externref_val_type(), externref_val_type()],
        vec![ValType::I32],
    );
    types.ty().function(
        vec![externref_val_type(), externref_val_type()],
        vec![externref_val_type()],
    );
    // Now emit user function types.
    for function in &module.functions {
        let params = function
            .params
            .iter()
            .map(|(_, ty)| wasm_type(ty, &array_registry))
            .collect::<Result<Vec<_>, _>>()?;
        let results = match &function.return_type {
            Type::Multi(multi_types) => multi_types
                .iter()
                .map(|ty| wasm_type(ty, &array_registry))
                .collect::<Result<Vec<_>, _>>()?,
            other => vec![wasm_type(other, &array_registry)?],
        };
        types.ty().function(params, results);
    }
    if start_thunk.is_some() {
        types
            .ty()
            .function(Vec::<ValType>::new(), Vec::<ValType>::new());
    }

    let mut imports = ImportSection::new();
    imports.import(
        host::JS_STRING_BUILTINS_MODULE,
        host::IMPORT_JS_STRING_EQ,
        EntityType::Function(host_type_base),
    );
    imports.import(
        host::JS_STRING_BUILTINS_MODULE,
        host::IMPORT_JS_STRING_CONCAT,
        EntityType::Function(host_type_base + 1),
    );
    for string in &string_constants {
        imports.import(
            host::IMPORTED_STRING_CONSTANTS_MODULE,
            string,
            EntityType::Global(wasm_encoder::GlobalType {
                val_type: externref_val_type(),
                mutable: false,
                shared: false,
            }),
        );
    }

    let mut functions = FunctionSection::new();
    let mut tables = TableSection::new();
    let mut elements = ElementSection::new();
    let mut exports = ExportSection::new();
    let mut codes = CodeSection::new();
    for (index, function) in module.functions.iter().enumerate() {
        // User function type indices come after array and host types in the type section.
        functions.function(user_type_base + index as u32);
        if function.name != "__waluau_top_level_init" {
            exports.export(
                &function.name,
                ExportKind::Func,
                host::defined_func_index(index as u32),
            );
        }
        codes.function(&emit_function(
            function,
            &signatures,
            &array_registry,
            &string_constants,
            user_type_base,
        )?);
    }
    if let Some(start) = start_thunk {
        let thunk_index = module.functions.len() as u32;
        functions.function(user_type_base + thunk_index);
        let mut thunk = Function::new(Vec::new());
        thunk.instruction(&Instruction::Call(host::defined_func_index(start as u32)));
        let n_returns = match &module.functions[start].return_type {
            Type::Multi(types) => types.len(),
            _ => 1,
        };
        for _ in 0..n_returns {
            thunk.instruction(&Instruction::Drop);
        }
        thunk.instruction(&Instruction::End);
        codes.function(&thunk);
    }
    let defined_func_count = module.functions.len() as u64;
    let table_size = host::HOST_IMPORT_COUNT as u64 + defined_func_count;
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        table64: false,
        minimum: table_size,
        maximum: Some(table_size),
        shared: false,
    });
    let table_inits = (0..module.functions.len() as u32)
        .map(host::defined_func_index)
        .collect::<Vec<_>>();
    elements.active(
        Some(0),
        &ConstExpr::i32_const(0),
        Elements::Functions(Cow::Owned(table_inits)),
    );

    wasm.section(&types);
    wasm.section(&imports);
    wasm.section(&functions);
    wasm.section(&tables);
    wasm.section(&exports);
    if start_thunk.is_some() {
        wasm.section(&StartSection {
            function_index: host::defined_func_index(module.functions.len() as u32),
        });
    }
    wasm.section(&elements);
    wasm.section(&codes);
    wasm.section(&CustomSection {
        name: host::CUSTOM_SECTION_NAME.into(),
        data: Cow::Owned(host::encode_string_constants_section(&string_constants)),
    });

    let bytes = wasm.finish();
    let features = WasmFeatures::all();
    Validator::new_with_features(features)
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
        Type::String => Ok(StorageType::Val(externref_val_type())),
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
        Type::Function { .. } | Type::Record(_) | Type::TypeParam(_) => unreachable!(),
    }
}

#[derive(Clone)]
struct FunctionSignature {
    index: u32,
    params: Vec<Type>,
    result: Type,
}

struct EmissionContext<'a> {
    signatures: &'a HashMap<String, FunctionSignature>,
    array_registry: &'a ArrayTypeRegistry,
    string_constants: &'a [String],
    user_type_base: u32,
}

impl EmissionContext<'_> {
    fn wasm_func_index(&self, user_index: u32) -> u32 {
        host::defined_func_index(user_index)
    }
}

fn emit_function(
    function: &IrFunction,
    signatures: &HashMap<String, FunctionSignature>,
    array_registry: &ArrayTypeRegistry,
    string_constants: &[String],
    user_type_base: u32,
) -> Result<Function, Diagnostic> {
    let ctx = EmissionContext {
        signatures,
        array_registry,
        string_constants,
        user_type_base,
    };
    let value_types = infer_value_types(function, signatures)?;
    let local_plan = build_local_plan(function, &value_types, array_registry)?;
    let value_defs = build_value_definition_map(function);
    let locals = compress_locals(local_plan.extra_locals.clone());
    let mut out = Function::new(locals);
    if try_emit_structured_fast_path(
        &mut out,
        function,
        &ctx,
        &value_types,
        &local_plan,
        &value_defs,
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
            &ctx,
            &value_types,
            &local_plan,
            &value_defs,
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
    ctx: &EmissionContext<'_>,
    value_types: &BTreeMap<ValueId, Type>,
    local_plan: &LocalPlan,
    value_defs: &HashMap<ValueId, IrInstruction>,
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
            emit_block_instructions(out, entry, ctx, local_plan, value_defs)?;
            emit_value_operand(out, local_plan, condition)?;
            out.instruction(&Instruction::If(BlockType::Empty));
            emit_phi_copies(out, function, entry.id, then_block, local_plan)?;
            emit_block(
                out,
                function,
                then_bb.unwrap(),
                ctx,
                value_types,
                local_plan,
                value_defs,
            )?;
            out.instruction(&Instruction::Else);
            emit_phi_copies(out, function, entry.id, else_block, local_plan)?;
            emit_block(
                out,
                function,
                else_bb.unwrap(),
                ctx,
                value_types,
                local_plan,
                value_defs,
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
                emit_block_instructions(out, entry, ctx, local_plan, value_defs)?;
                emit_phi_copies(out, function, entry.id, second.id, local_plan)?;
                out.instruction(&Instruction::Block(BlockType::Empty));
                out.instruction(&Instruction::Loop(BlockType::Empty));
                emit_block_instructions(out, second, ctx, local_plan, value_defs)?;
                emit_value_operand(out, local_plan, condition)?;
                out.instruction(&Instruction::I32Eqz);
                out.instruction(&Instruction::BrIf(1));
                emit_phi_copies(out, function, second.id, then_block, local_plan)?;
                emit_block_instructions(
                    out,
                    then_bb.expect("checked above"),
                    ctx,
                    local_plan,
                    value_defs,
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
                    ctx,
                    value_types,
                    local_plan,
                    value_defs,
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
                emit_block_instructions(out, entry, ctx, local_plan, value_defs)?;
                emit_phi_copies(out, function, entry.id, body.id, local_plan)?;
                out.instruction(&Instruction::Block(BlockType::Empty));
                out.instruction(&Instruction::Loop(BlockType::Empty));
                emit_block_instructions(out, body, ctx, local_plan, value_defs)?;
                emit_phi_copies(out, function, body.id, second.id, local_plan)?;
                emit_block_instructions(out, second, ctx, local_plan, value_defs)?;
                emit_value_operand(out, local_plan, condition)?;
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
                    ctx,
                    value_types,
                    local_plan,
                    value_defs,
                )?;
                return Ok(true);
            }
        }
    }
    Ok(false)
}

struct LocalPlan {
    slots: BTreeMap<ValueId, u32>,
    multi_slots: BTreeMap<ValueId, Vec<u32>>,
    extra_locals: Vec<ValType>,
    stack_values: BTreeSet<ValueId>,
    pc_local: u32,
}

#[derive(Clone)]
struct LiveInterval {
    value: ValueId,
    start: usize,
    end: usize,
    ty: Type,
}

fn build_local_plan(
    function: &IrFunction,
    value_types: &BTreeMap<ValueId, Type>,
    array_registry: &ArrayTypeRegistry,
) -> Result<LocalPlan, Diagnostic> {
    let mut slots = BTreeMap::new();
    let phi_copy_sources = collect_phi_copy_sources(function);
    let mut stack_values = BTreeSet::new();

    for block in function.blocks.values() {
        let block_stack_values = compute_stack_values(
            block,
            phi_copy_sources.get(&block.id).cloned().unwrap_or_default(),
        );
        stack_values.extend(block_stack_values);
    }
    stack_values.retain(|v| !matches!(value_types.get(v), Some(Type::Multi(_))));

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
    Ok(LocalPlan {
        slots,
        multi_slots,
        extra_locals,
        stack_values,
        pc_local,
    })
}

fn build_value_definition_map(function: &IrFunction) -> HashMap<ValueId, IrInstruction> {
    let mut defs = HashMap::new();
    for block in function.blocks.values() {
        for (value, instruction) in &block.instructions {
            defs.insert(*value, instruction.clone());
        }
    }
    defs
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
                IrInstruction::String(_) => Type::String,
                IrInstruction::Cast { to, .. } => to.clone(),
                IrInstruction::Binary { result_ty, .. } => result_ty.clone(),
                IrInstruction::MathIntrinsic { result_ty, .. } => result_ty.clone(),
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

fn emit_block(
    out: &mut Function,
    function: &IrFunction,
    block: &BasicBlock,
    ctx: &EmissionContext<'_>,
    value_types: &BTreeMap<ValueId, Type>,
    local_plan: &LocalPlan,
    value_defs: &HashMap<ValueId, IrInstruction>,
) -> Result<(), Diagnostic> {
    emit_block_instructions(out, block, ctx, local_plan, value_defs)?;
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
            emit_value_operand(out, local_plan, *condition)?;
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
            emit_value_operand(out, local_plan, *value)?;
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
    ctx: &EmissionContext<'_>,
    local_plan: &LocalPlan,
    value_defs: &HashMap<ValueId, IrInstruction>,
) -> Result<(), Diagnostic> {
    for (value, instruction) in &block.instructions {
        match instruction {
            IrInstruction::Param(_) | IrInstruction::Phi(_) => {}
            IrInstruction::Number { ty, literal } => {
                emit_numeric_const(out, *ty, literal)?;
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::Bool(flag) => {
                out.instruction(&Instruction::I32Const(i32::from(*flag)));
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::String(literal) => {
                let index = host::string_constant_index(ctx.string_constants, literal)?;
                out.instruction(&Instruction::GlobalGet(index));
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::Cast {
                value: source,
                from,
                to,
            } => {
                emit_value_operand(out, local_plan, *source)?;
                emit_cast(out, from.clone(), to.clone())?;
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::Binary {
                op,
                left,
                right,
                operand_ty,
                result_ty,
            } => {
                if matches!(op, BinaryOp::FloorDiv | BinaryOp::Mod) {
                    let left_local = local(local_plan, *left)?;
                    let right_local = local(local_plan, *right)?;
                    emit_floor_or_mod(out, *op, operand_ty.clone(), left_local, right_local)?;
                } else {
                    emit_value_operand(out, local_plan, *left)?;
                    emit_value_operand(out, local_plan, *right)?;
                    emit_binary(out, ctx, *op, operand_ty.clone(), result_ty.clone())?;
                }
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::MathIntrinsic {
                intrinsic,
                args,
                operand_ty,
                ..
            } => {
                for arg in args {
                    emit_value_operand(out, local_plan, *arg)?;
                }
                emit_math_intrinsic(out, *intrinsic, operand_ty.clone())?;
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::Call { name, args } => {
                for arg in args {
                    emit_value_operand(out, local_plan, *arg)?;
                }
                let callee = ctx.signatures.get(name).ok_or_else(|| {
                    Diagnostic::new(format!("unknown function '{name}' during wasm emission"))
                })?;
                out.instruction(&Instruction::Call(ctx.wasm_func_index(callee.index)));
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::CallValue {
                callee,
                args,
                params,
                return_type,
            } => {
                if let Some(IrInstruction::Closure {
                    name,
                    captures,
                    params: closure_params,
                    return_type: closure_return_type,
                }) = value_defs.get(callee)
                {
                    if params != closure_params || return_type != closure_return_type {
                        return Err(Diagnostic::new(
                            "indirect-call signature mismatch for closure value",
                        ));
                    }
                    let target = ctx.signatures.get(name).ok_or_else(|| {
                        Diagnostic::new(format!(
                            "unknown closure target function '{name}' during wasm emission"
                        ))
                    })?;
                    for (i, capture) in captures.iter().enumerate() {
                        // Determine what the callee expects for this capture slot.
                        let expected = target
                            .params
                            .get(i)
                            .ok_or_else(|| Diagnostic::new("closure target param missing"))?
                            .clone();
                        if let Type::Array(_) = expected {
                            // Callee expects an array/ref - pass the capture by reference.
                            emit_value_operand(out, local_plan, *capture)?;
                        } else {
                            // Callee expects the element value. If the capture is a cell
                            // (ArrayNew), pass its stored element instead.
                            if let Some(IrInstruction::ArrayNew { elements, .. }) =
                                value_defs.get(capture)
                            {
                                let elem = elements.first().copied().ok_or_else(|| {
                                    Diagnostic::new("empty array capture during wasm emission")
                                })?;
                                emit_value_operand(out, local_plan, elem)?;
                            } else {
                                emit_value_operand(out, local_plan, *capture)?;
                            }
                        }
                    }
                    for arg in args {
                        emit_value_operand(out, local_plan, *arg)?;
                    }
                    out.instruction(&Instruction::Call(ctx.wasm_func_index(target.index)));
                    emit_value_store(out, local_plan, *value)?;
                    continue;
                }
                for arg in args {
                    emit_value_operand(out, local_plan, *arg)?;
                }
                emit_value_operand(out, local_plan, *callee)?;
                let type_index = find_function_type_index(
                    ctx.signatures,
                    ctx.user_type_base,
                    params,
                    return_type,
                )?;
                out.instruction(&Instruction::CallIndirect {
                    type_index,
                    table_index: 0,
                });
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::Closure {
                name,
                captures: _,
                params,
                return_type,
            } => {
                let callee = ctx.signatures.get(name).ok_or_else(|| {
                    Diagnostic::new(format!("unknown function '{name}' during wasm emission"))
                })?;
                let _ = find_function_type_index(
                    ctx.signatures,
                    ctx.user_type_base,
                    params,
                    return_type,
                )?;
                // Indirect calls use table slot indices, not module function indices.
                out.instruction(&Instruction::I32Const(callee.index as i32));
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::ArrayNew {
                element_ty,
                elements,
            } => {
                for element in elements {
                    emit_value_operand(out, local_plan, *element)?;
                }
                let array_ty = Type::Array(Box::new(element_ty.clone()));
                let array_type_index = ctx.array_registry.index(&array_ty)?;
                out.instruction(&Instruction::ArrayNewFixed {
                    array_type_index,
                    array_size: elements.len() as u32,
                });
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::ArrayGet {
                array,
                index,
                element_ty,
            } => {
                let array_local = local(local_plan, *array)?;
                let index_local = local(local_plan, *index)?;
                let array_ty = Type::Array(Box::new(element_ty.clone()));
                let array_type_index = ctx.array_registry.index(&array_ty)?;
                emit_bounds_check(out, array_local, index_local);
                out.instruction(&Instruction::LocalGet(array_local));
                out.instruction(&Instruction::LocalGet(index_local));
                out.instruction(&Instruction::ArrayGet(array_type_index));
                emit_value_store(out, local_plan, *value)?;
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
                let array_type_index = ctx.array_registry.index(&array_ty)?;
                emit_bounds_check(out, array_local, index_local);
                out.instruction(&Instruction::LocalGet(array_local));
                out.instruction(&Instruction::LocalGet(index_local));
                out.instruction(&Instruction::LocalGet(local(local_plan, *stored)?));
                out.instruction(&Instruction::ArraySet(array_type_index));
            }
            IrInstruction::ArrayLen { array } => {
                emit_value_operand(out, local_plan, *array)?;
                out.instruction(&Instruction::ArrayLen);
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::PackMulti { values, .. } => {
                for v in values {
                    emit_value_operand(out, local_plan, *v)?;
                }
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::MultiGet {
                value: source,
                index,
                ..
            } => {
                let slots = local_plan.multi_slots.get(source).ok_or_else(|| {
                    Diagnostic::new(format!(
                        "multi-get source {:?} has no multi-value slots",
                        source
                    ))
                })?;
                let slot = slots.get(*index).copied().ok_or_else(|| {
                    Diagnostic::new(format!(
                        "multi-get index {} out of range (multi has {} slots)",
                        index,
                        slots.len()
                    ))
                })?;
                out.instruction(&Instruction::LocalGet(slot));
                emit_value_store(out, local_plan, *value)?;
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
        if phi_copy_sources.contains(value) {
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
        if matches!(ty, Type::Multi(_)) {
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
        | IrInstruction::Bool(_)
        | IrInstruction::String(_) => Vec::new(),
        IrInstruction::Cast { value, .. } => vec![*value],
        IrInstruction::Binary { left, right, .. } => vec![*left, *right],
        IrInstruction::MathIntrinsic { args, .. } => args.clone(),
        IrInstruction::Call { args, .. } => args.clone(),
        IrInstruction::CallValue { callee, args, .. } => {
            let mut out = Vec::with_capacity(args.len() + 1);
            out.extend(args.iter().copied());
            out.push(*callee);
            out
        }
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
        IrInstruction::PackMulti { values, .. } => values.clone(),
        IrInstruction::MultiGet { value, .. } => vec![*value],
        IrInstruction::Phi(_) => Vec::new(),
    }
}

fn terminator_operands(terminator: &Terminator) -> Vec<ValueId> {
    match terminator {
        Terminator::Jump(_) | Terminator::Unreachable => Vec::new(),
        Terminator::Branch { condition, .. } => vec![*condition],
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
    )
}

fn instruction_can_consume_stack_value(instruction: &IrInstruction, value: ValueId) -> bool {
    match instruction {
        IrInstruction::Param(_)
        | IrInstruction::Number { .. }
        | IrInstruction::Bool(_)
        | IrInstruction::String(_) => false,
        IrInstruction::Cast { value: source, .. } => *source == value,
        IrInstruction::Binary { left, .. } => *left == value,
        IrInstruction::MathIntrinsic { args, .. } => args.first().copied() == Some(value),
        IrInstruction::Call { args, .. } => args.first().copied() == Some(value),
        IrInstruction::CallValue { .. } => false,
        IrInstruction::Closure { captures, .. } => captures.first().copied() == Some(value),
        IrInstruction::ArrayNew { elements, .. } => elements.first().copied() == Some(value),
        IrInstruction::ArrayGet { .. } | IrInstruction::ArraySet { .. } => false,
        IrInstruction::ArrayLen { array } => *array == value,
        IrInstruction::PackMulti { values, .. } => values.first().copied() == Some(value),
        IrInstruction::MultiGet { value: source, .. } => *source == value,
        IrInstruction::Phi(_) => false,
    }
}

fn emit_value_operand(
    out: &mut Function,
    local_plan: &LocalPlan,
    value: ValueId,
) -> Result<(), Diagnostic> {
    if local_plan.stack_values.contains(&value) {
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

fn emit_value_store(
    out: &mut Function,
    local_plan: &LocalPlan,
    value: ValueId,
) -> Result<(), Diagnostic> {
    if local_plan.stack_values.contains(&value) {
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
    _ctx: &EmissionContext<'_>,
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
            Type::String => {
                return Err(Diagnostic::new(
                    "string add is not supported during wasm emission",
                ));
            }
            Type::Array(_) => unreachable!(),
            Type::Multi(_) => {
                return Err(Diagnostic::new(
                    "multi-value add is not supported during wasm emission",
                ));
            }
            Type::Function { .. } | Type::Record(_) | Type::TypeParam(_) => unreachable!(),
        },
        BinaryOp::Concat => match operand_ty {
            Type::String => {
                out.instruction(&Instruction::Call(host::IMPORT_JS_STRING_CONCAT_FUNC));
            }
            _ => {
                return Err(Diagnostic::new(
                    "concat is only supported for strings during wasm emission",
                ));
            }
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
            Type::String => {
                return Err(Diagnostic::new(
                    "string sub is not supported during wasm emission",
                ));
            }
            Type::Array(_) => unreachable!(),
            Type::Multi(_) => {
                return Err(Diagnostic::new(
                    "multi-value sub is not supported during wasm emission",
                ));
            }
            Type::Function { .. } | Type::Record(_) | Type::TypeParam(_) => unreachable!(),
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
            Type::String => {
                return Err(Diagnostic::new(
                    "string mul is not supported during wasm emission",
                ));
            }
            Type::Array(_) => unreachable!(),
            Type::Multi(_) => {
                return Err(Diagnostic::new(
                    "multi-value mul is not supported during wasm emission",
                ));
            }
            Type::Function { .. } | Type::Record(_) | Type::TypeParam(_) => unreachable!(),
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
            Type::String => {
                return Err(Diagnostic::new(
                    "string div is not supported during wasm emission",
                ));
            }
            Type::Array(_) => unreachable!(),
            Type::Multi(_) => {
                return Err(Diagnostic::new(
                    "multi-value div is not supported during wasm emission",
                ));
            }
            Type::Function { .. } | Type::Record(_) | Type::TypeParam(_) => unreachable!(),
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
            Type::String => {
                out.instruction(&Instruction::Call(host::IMPORT_JS_STRING_EQ_FUNC));
            }
            Type::Array(_) => unreachable!(),
            Type::Multi(_) => {
                return Err(Diagnostic::new(
                    "multi-value equality is not supported during wasm emission",
                ));
            }
            Type::Function { .. } | Type::Record(_) | Type::TypeParam(_) => unreachable!(),
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
            Type::String => {
                return Err(Diagnostic::new(
                    "string comparison is not supported during wasm emission",
                ));
            }
            Type::Array(_) => unreachable!(),
            Type::Multi(_) => {
                return Err(Diagnostic::new(
                    "multi-value comparison is not supported during wasm emission",
                ));
            }
            Type::Function { .. } | Type::Record(_) | Type::TypeParam(_) => unreachable!(),
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
            Type::String => {
                return Err(Diagnostic::new(
                    "string comparison is not supported during wasm emission",
                ));
            }
            Type::Array(_) => unreachable!(),
            Type::Multi(_) => {
                return Err(Diagnostic::new(
                    "multi-value comparison is not supported during wasm emission",
                ));
            }
            Type::Function { .. } | Type::Record(_) | Type::TypeParam(_) => unreachable!(),
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

fn emit_math_intrinsic(
    out: &mut Function,
    intrinsic: MathIntrinsic,
    operand_ty: Type,
) -> Result<(), Diagnostic> {
    match (intrinsic, operand_ty) {
        (MathIntrinsic::Abs, Type::Numeric(NumericType::F32)) => {
            out.instruction(&Instruction::F32Abs);
        }
        (MathIntrinsic::Abs, Type::Numeric(NumericType::F64)) => {
            out.instruction(&Instruction::F64Abs);
        }
        (MathIntrinsic::Min, Type::Numeric(NumericType::F32)) => {
            out.instruction(&Instruction::F32Min);
        }
        (MathIntrinsic::Min, Type::Numeric(NumericType::F64)) => {
            out.instruction(&Instruction::F64Min);
        }
        (MathIntrinsic::Max, Type::Numeric(NumericType::F32)) => {
            out.instruction(&Instruction::F32Max);
        }
        (MathIntrinsic::Max, Type::Numeric(NumericType::F64)) => {
            out.instruction(&Instruction::F64Max);
        }
        (MathIntrinsic::Sqrt, Type::Numeric(NumericType::F32)) => {
            out.instruction(&Instruction::F32Sqrt);
        }
        (MathIntrinsic::Sqrt, Type::Numeric(NumericType::F64)) => {
            out.instruction(&Instruction::F64Sqrt);
        }
        (MathIntrinsic::Floor, Type::Numeric(NumericType::F32)) => {
            out.instruction(&Instruction::F32Floor);
        }
        (MathIntrinsic::Floor, Type::Numeric(NumericType::F64)) => {
            out.instruction(&Instruction::F64Floor);
        }
        (MathIntrinsic::Ceil, Type::Numeric(NumericType::F32)) => {
            out.instruction(&Instruction::F32Ceil);
        }
        (MathIntrinsic::Ceil, Type::Numeric(NumericType::F64)) => {
            out.instruction(&Instruction::F64Ceil);
        }
        (MathIntrinsic::Trunc, Type::Numeric(NumericType::F32)) => {
            out.instruction(&Instruction::F32Trunc);
        }
        (MathIntrinsic::Trunc, Type::Numeric(NumericType::F64)) => {
            out.instruction(&Instruction::F64Trunc);
        }
        (MathIntrinsic::Nearest, Type::Numeric(NumericType::F32)) => {
            out.instruction(&Instruction::F32Nearest);
        }
        (MathIntrinsic::Nearest, Type::Numeric(NumericType::F64)) => {
            out.instruction(&Instruction::F64Nearest);
        }
        (MathIntrinsic::Copysign, Type::Numeric(NumericType::F32)) => {
            out.instruction(&Instruction::F32Copysign);
        }
        (MathIntrinsic::Copysign, Type::Numeric(NumericType::F64)) => {
            out.instruction(&Instruction::F64Copysign);
        }
        (intrinsic, ty) => {
            return Err(Diagnostic::new(format!(
                "math intrinsic {intrinsic:?} does not support {ty} during wasm emission"
            )));
        }
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

fn externref_val_type() -> ValType {
    // Long-form `(ref null extern)` (0x63 0x6f) so Wasmtime host imports type-check.
    ValType::Ref(RefType {
        nullable: false,
        heap_type: HeapType::Abstract {
            shared: false,
            ty: AbstractHeapType::Extern,
        },
    })
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
        Type::String => Ok(externref_val_type()),
        Type::Multi(_) => Err(Diagnostic::new(
            "multi-value types are not supported in Wasm signatures yet",
        )),
        Type::Function { .. } => Ok(ValType::I32),
        Type::Record(_) => unreachable!("namespace types are not stored in wasm locals"),
        Type::TypeParam(_) => {
            unreachable!("generic type parameters must be specialized before codegen")
        }
    }
}

fn find_function_type_index(
    signatures: &HashMap<String, FunctionSignature>,
    user_type_base: u32,
    params: &[Type],
    return_type: &Type,
) -> Result<u32, Diagnostic> {
    signatures
        .values()
        .find(|signature| signature.params == params && signature.result == *return_type)
        .map(|signature| user_type_base + signature.index)
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
    use waluau_ast::BinaryOp;
    use waluau_ir::Instruction as IrInstruction;
    use wasmparser::{Operator, Parser, Payload, Validator};
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
    fn emits_valid_wasm_for_capturing_closure_values() {
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
        let wasm = super::emit(&ir).expect("capturing closures should compile");
        Validator::new()
            .validate_all(&wasm)
            .expect("emitted module should validate");
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

    #[test]
    fn keeps_immediate_return_value_on_stack() {
        let source = r#"
            function entry(x: i32): i32
                return x + 1
            end
        "#;
        let program = waluau_parser::parse(source).expect("parse should succeed");
        let ir = waluau_ir::build(&program).expect("ir should succeed");
        let wasm = super::emit(&ir).expect("emit should succeed");
        let mut saw_add_then_return = false;
        for payload in Parser::new(0).parse_all(&wasm) {
            let payload = payload.expect("wasm should parse");
            if let Payload::CodeSectionEntry(body) = payload {
                let mut reader = body.get_operators_reader().expect("ops should decode");
                let mut prev_was_add = false;
                while !reader.eof() {
                    let op = reader.read().expect("op should decode");
                    match op {
                        Operator::I32Add => prev_was_add = true,
                        Operator::Return if prev_was_add => {
                            saw_add_then_return = true;
                            break;
                        }
                        _ => prev_was_add = false,
                    }
                }
                break;
            }
        }
        assert!(saw_add_then_return);
    }

    #[test]
    fn emits_valid_wasm_for_multi_return() {
        let source = r#"
            function pair(x: i32, y: i32): i32, i32
                return x, y
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
    fn emits_valid_wasm_for_multi_let_binding() {
        let source = r#"
            function swap(x: i32, y: i32): i32, i32
                return y, x
            end
            function entry(a: i32, b: i32): i32
                local x: i32, y: i32 = swap(a, b)
                return x + y
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
    fn emits_valid_wasm_for_multi_assign() {
        let source = r#"
            function swap(x: i32, y: i32): i32, i32
                return y, x
            end
            function entry(a: i32, b: i32): i32
                local x: i32, y: i32 = a, b
                x, y = swap(x, y)
                return x + y
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
    fn reuses_i32_local_slots_for_disjoint_live_ranges() {
        let source = r#"
            function reuse(x: i32): i32
                local a: i32 = x + x
                local b: i32 = a + a
                local c: i32 = x - x
                local d: i32 = c + c
                return b + d
            end
        "#;
        let program = waluau_parser::parse(source).expect("parse should succeed");
        let ir = waluau_ir::build(&program).expect("ir should succeed");
        let function = &ir.functions[0];
        let signatures = std::iter::once((
            function.name.clone(),
            super::FunctionSignature {
                index: 0,
                params: function.params.iter().map(|(_, ty)| ty.clone()).collect(),
                result: function.return_type.clone(),
            },
        ))
        .collect::<std::collections::HashMap<_, _>>();
        let value_types =
            super::infer_value_types(function, &signatures).expect("types should infer");
        let array_types = super::collect_array_types(&ir);
        let array_registry = super::ArrayTypeRegistry::with_function_type_offset(
            &array_types,
            ir.functions.len() as u32 + u32::from(ir.start.is_some()),
        );
        let local_plan = super::build_local_plan(function, &value_types, &array_registry)
            .expect("plan should build");

        let block = function
            .blocks
            .get(&function.entry)
            .expect("entry block should exist");
        let param = block
            .instructions
            .iter()
            .find_map(|(value, instruction)| match instruction {
                IrInstruction::Param(_) => Some(*value),
                _ => None,
            })
            .expect("param should exist");
        let a = block
            .instructions
            .iter()
            .find_map(|(value, instruction)| match instruction {
                IrInstruction::Binary {
                    op: BinaryOp::Add,
                    left,
                    right,
                    ..
                } if *left == param && *right == param => Some(*value),
                _ => None,
            })
            .expect("a should exist");
        let c = block
            .instructions
            .iter()
            .find_map(|(value, instruction)| match instruction {
                IrInstruction::Binary {
                    op: BinaryOp::Sub,
                    left,
                    right,
                    ..
                } if *left == param && *right == param => Some(*value),
                _ => None,
            })
            .expect("c should exist");

        assert_eq!(local_plan.slots.get(&a), local_plan.slots.get(&c));
    }
}
