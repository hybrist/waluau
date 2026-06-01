use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use waluau_ast::{BinaryOp, NumberLiteral, NumericType, Type};
use waluau_diagnostics::Diagnostic;
use waluau_ir::{
    BasicBlock, Function as IrFunction, Instruction as IrInstruction, MathIntrinsic, Module,
    Terminator, ValueId,
};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, CustomSection, ElementSection, Elements, EntityType,
    ExportKind, ExportSection, FieldType, Function, FunctionSection, GlobalSection, HeapType,
    ImportSection, Instruction, Module as WasmModule, RefType, StartSection, StorageType,
    TableSection, TableType, TypeSection, ValType,
};
use wasmparser::{Validator, WasmFeatures};

mod arrays;
mod coroutines;
pub mod host;
mod signatures;
mod wasm_types;

use arrays::{ArrayTypeRegistry, array_storage_type, collect_array_types};
use coroutines::{
    CoroutinePlan, STATE_CONT_FIELD, STATE_TAG_FIELD, STATE_YIELDED_FIELD, TAG_ERROR, TAG_FINISHED,
    TAG_SUSPENDED, coroutine_body_ref_type, coroutine_state_ref_type,
};
use signatures::{SignatureRegistry, collect_user_signatures, find_function_type_index};
use wasm_types::{compress_locals, externref_nonnull_val_type, externref_val_type, wasm_type};

pub fn emit(module: &Module) -> Result<Vec<u8>, Diagnostic> {
    let array_types = collect_array_types(module);
    let string_constants = host::collect_string_constants(module);
    let coroutine_plan = CoroutinePlan::new(module, string_constants.len() as u32);
    let start_thunk = module.start;
    let host_type_base = array_types.len() as u32;
    // When the module uses coroutines, two GC types sit between the host types and the
    // user function types: the body signature `() -> i32` and the `$coroutine_state` struct.
    // They must precede user function types so `thread` params can reference the struct.
    let coroutine_types_base = host_type_base + host::HOST_TYPE_COUNT;
    let coroutine_body_sig_type = coroutine_plan.has_state().then_some(coroutine_types_base);
    let coroutine_state_type = coroutine_plan
        .has_state()
        .then_some(coroutine_types_base + 1);
    let coroutine_type_count = if coroutine_plan.has_state() { 2 } else { 0 };
    let user_type_base = coroutine_types_base + coroutine_type_count;
    // Array types come first in the type section (indices 0..N-1).
    let mut array_registry = ArrayTypeRegistry::with_function_type_offset(&array_types, 0);
    array_registry.coroutine_state_type = coroutine_state_type;

    let signature_registry = collect_user_signatures(module, start_thunk.is_some());

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
        vec![externref_nonnull_val_type()],
    );
    types
        .ty()
        .function(vec![ValType::I32], vec![externref_val_type()]);
    types
        .ty()
        .function(vec![ValType::I64], vec![externref_val_type()]);
    types
        .ty()
        .function(vec![ValType::F32], vec![externref_val_type()]);
    types
        .ty()
        .function(vec![ValType::F64], vec![externref_val_type()]);
    types.ty().function(vec![externref_val_type()], vec![]);
    // Coroutine GC types (before user function types so `thread` params can reference them).
    if let (Some(body_sig), Some(state_type)) = (coroutine_body_sig_type, coroutine_state_type) {
        // Body signature: () -> i32 (the continuation funcref type).
        types
            .ty()
            .function(Vec::<ValType>::new(), vec![ValType::I32]);
        debug_assert_eq!(body_sig, coroutine_types_base);
        // State struct: { tag:i32, yielded:i32, continuation:(ref null body_sig), pc_*:i32 }.
        let mut fields = vec![
            FieldType {
                element_type: StorageType::Val(ValType::I32),
                mutable: true,
            },
            FieldType {
                element_type: StorageType::Val(ValType::I32),
                mutable: true,
            },
            FieldType {
                element_type: StorageType::Val(coroutine_body_ref_type(body_sig)),
                mutable: true,
            },
        ];
        for _ in 0..coroutine_plan.pc_field_count() {
            fields.push(FieldType {
                element_type: StorageType::Val(ValType::I32),
                mutable: true,
            });
        }
        let _ = state_type;
        types.ty().struct_(fields);
    }
    // Now emit user function types.
    for (params, return_type) in &signature_registry.unique_signatures {
        let params = params
            .iter()
            .map(|ty| wasm_type(ty, &array_registry))
            .collect::<Result<Vec<_>, _>>()?;
        let results = match return_type {
            Type::Multi(multi_types) => multi_types
                .iter()
                .map(|ty| wasm_type(ty, &array_registry))
                .collect::<Result<Vec<_>, _>>()?,
            Type::Unit => Vec::new(),
            other => vec![wasm_type(other, &array_registry)?],
        };
        types.ty().function(params, results);
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
    imports.import(
        host::IMPORT_MODULE,
        host::IMPORT_PRINT,
        EntityType::Function(host_type_base + 6),
    );
    imports.import(
        host::IMPORT_MODULE,
        host::IMPORT_JS_TOSTRING_I32,
        EntityType::Function(host_type_base + 2),
    );
    imports.import(
        host::IMPORT_MODULE,
        host::IMPORT_JS_TOSTRING_U32,
        EntityType::Function(host_type_base + 2),
    );
    imports.import(
        host::IMPORT_MODULE,
        host::IMPORT_JS_TOSTRING_I64,
        EntityType::Function(host_type_base + 3),
    );
    imports.import(
        host::IMPORT_MODULE,
        host::IMPORT_JS_TOSTRING_U64,
        EntityType::Function(host_type_base + 3),
    );
    imports.import(
        host::IMPORT_MODULE,
        host::IMPORT_JS_TOSTRING_F32,
        EntityType::Function(host_type_base + 4),
    );
    imports.import(
        host::IMPORT_MODULE,
        host::IMPORT_JS_TOSTRING_F64,
        EntityType::Function(host_type_base + 5),
    );
    imports.import(
        host::IMPORT_MODULE,
        host::IMPORT_JS_TOSTRING_BOOL,
        EntityType::Function(host_type_base + 2),
    );

    let mut functions = FunctionSection::new();
    let mut tables = TableSection::new();
    let mut elements = ElementSection::new();
    let mut globals = GlobalSection::new();
    if let Some(state_type) = coroutine_state_type {
        coroutine_plan.emit_globals(&mut globals, state_type);
    }
    let mut exports = ExportSection::new();
    let mut codes = CodeSection::new();
    for (index, function) in module.functions.iter().enumerate() {
        // User function type indices come after array, host, and coroutine types.
        let params = function
            .params
            .iter()
            .map(|(_, ty)| ty.clone())
            .collect::<Vec<_>>();
        let sig_index = signature_registry
            .get(&params, &function.return_type)
            .unwrap();
        functions.function(user_type_base + sig_index);
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
            &signature_registry,
            &array_registry,
            &string_constants,
            user_type_base,
            &coroutine_plan,
            coroutine_body_sig_type,
        )?);
    }
    if let Some(start) = start_thunk {
        let thunk_sig_index = signature_registry.get(&[], &Type::Unit).unwrap();
        functions.function(user_type_base + thunk_sig_index);
        let mut thunk = Function::new(Vec::new());
        thunk.instruction(&Instruction::Call(host::defined_func_index(start as u32)));
        let n_returns = match &module.functions[start].return_type {
            Type::Multi(types) => types.len(),
            Type::Unit => 0,
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
    if coroutine_plan.has_state() {
        wasm.section(&globals);
    }
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

#[derive(Clone)]
struct FunctionSignature {
    index: u32,
    params: Vec<Type>,
    result: Type,
}

struct EmissionContext<'a> {
    signatures: &'a HashMap<String, FunctionSignature>,
    signature_registry: &'a SignatureRegistry,
    array_registry: &'a ArrayTypeRegistry,
    string_constants: &'a [String],
    user_type_base: u32,
    coroutine_plan: &'a CoroutinePlan,
    /// Type index of the coroutine body signature `() -> i32` (continuation funcref type).
    coroutine_body_sig_type: Option<u32>,
}

impl EmissionContext<'_> {
    fn wasm_func_index(&self, user_index: u32) -> u32 {
        host::defined_func_index(user_index)
    }

    fn coroutine_state_type(&self) -> Result<u32, Diagnostic> {
        self.array_registry.coroutine_state_type()
    }

    fn coroutine_body_sig_type(&self) -> Result<u32, Diagnostic> {
        self.coroutine_body_sig_type
            .ok_or_else(|| Diagnostic::new("missing coroutine body signature type"))
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_function(
    function: &IrFunction,
    signatures: &HashMap<String, FunctionSignature>,
    signature_registry: &SignatureRegistry,
    array_registry: &ArrayTypeRegistry,
    string_constants: &[String],
    user_type_base: u32,
    coroutine_plan: &CoroutinePlan,
    coroutine_body_sig_type: Option<u32>,
) -> Result<Function, Diagnostic> {
    let ctx = EmissionContext {
        signatures,
        signature_registry,
        array_registry,
        string_constants,
        user_type_base,
        coroutine_plan,
        coroutine_body_sig_type,
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
    if let Some(pc_field) = ctx.coroutine_plan.pc_field(&function.name) {
        // A directly-yielding function resumes at the program counter saved in the
        // active instance's state struct.
        emit_active_state_field_get(&mut out, &ctx, pc_field)?;
    } else {
        out.instruction(&Instruction::I32Const(function.entry.0 as i32));
    }
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

/// Push the currently-running coroutine instance `(ref null $coroutine_state)`.
fn emit_active_state_ref(out: &mut Function, ctx: &EmissionContext<'_>) -> Result<(), Diagnostic> {
    out.instruction(&Instruction::GlobalGet(ctx.coroutine_plan.active_global()?));
    Ok(())
}

/// Load an i32 field from the active instance's state struct.
fn emit_active_state_field_get(
    out: &mut Function,
    ctx: &EmissionContext<'_>,
    field_index: u32,
) -> Result<(), Diagnostic> {
    emit_active_state_ref(out, ctx)?;
    out.instruction(&Instruction::StructGet {
        struct_type_index: ctx.coroutine_state_type()?,
        field_index,
    });
    Ok(())
}

/// Store a constant i32 into a field of the active instance's state struct.
fn emit_active_state_field_set_const(
    out: &mut Function,
    ctx: &EmissionContext<'_>,
    field_index: u32,
    value: i32,
) -> Result<(), Diagnostic> {
    emit_active_state_ref(out, ctx)?;
    out.instruction(&Instruction::I32Const(value));
    out.instruction(&Instruction::StructSet {
        struct_type_index: ctx.coroutine_state_type()?,
        field_index,
    });
    Ok(())
}

fn try_emit_structured_fast_path(
    out: &mut Function,
    function: &IrFunction,
    ctx: &EmissionContext<'_>,
    value_types: &BTreeMap<ValueId, Type>,
    local_plan: &LocalPlan,
    value_defs: &HashMap<ValueId, IrInstruction>,
) -> Result<bool, Diagnostic> {
    if ctx.coroutine_plan.function_yields(&function.name) {
        return Ok(false);
    }

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
            emit_block_instructions(out, function, entry, ctx, local_plan, value_defs)?;
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
                emit_block_instructions(out, function, entry, ctx, local_plan, value_defs)?;
                emit_phi_copies(out, function, entry.id, second.id, local_plan)?;
                out.instruction(&Instruction::Block(BlockType::Empty));
                out.instruction(&Instruction::Loop(BlockType::Empty));
                emit_block_instructions(out, function, second, ctx, local_plan, value_defs)?;
                emit_value_operand(out, local_plan, condition)?;
                out.instruction(&Instruction::I32Eqz);
                out.instruction(&Instruction::BrIf(1));
                emit_phi_copies(out, function, second.id, then_block, local_plan)?;
                emit_block_instructions(
                    out,
                    function,
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
                emit_block_instructions(out, function, entry, ctx, local_plan, value_defs)?;
                emit_phi_copies(out, function, entry.id, body.id, local_plan)?;
                out.instruction(&Instruction::Block(BlockType::Empty));
                out.instruction(&Instruction::Loop(BlockType::Empty));
                emit_block_instructions(out, function, body, ctx, local_plan, value_defs)?;
                emit_phi_copies(out, function, body.id, second.id, local_plan)?;
                emit_block_instructions(out, function, second, ctx, local_plan, value_defs)?;
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
    unit_values: BTreeSet<ValueId>,
    pc_local: u32,
    /// Scratch `(ref null $coroutine_state)` local for saving/restoring the active
    /// instance across a `coroutine_resume` (nested-coroutine support).
    coroutine_save_local: Option<u32>,
    /// Scratch i32 local for spilling a yielded value before mutating the state struct.
    coroutine_yield_tmp: Option<u32>,
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

    // Reserve a scratch ref local for save/restore around `coroutine_resume`.
    let has_resume = function.blocks.values().any(|block| {
        block
            .instructions
            .iter()
            .any(|(_, instruction)| matches!(instruction, IrInstruction::CoroutineResume { .. }))
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

    // Reserve a scratch i32 for spilling the yielded value (which may be a stack value)
    // before the state-struct writes reorder the wasm operand stack.
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

    Ok(LocalPlan {
        slots,
        multi_slots,
        extra_locals,
        stack_values,
        unit_values,
        pc_local,
        coroutine_save_local,
        coroutine_yield_tmp,
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
                IrInstruction::Unit => Type::Unit,
                IrInstruction::Bool(_) => Type::Bool,
                IrInstruction::String(_) => Type::String,
                IrInstruction::Cast { to, .. } => to.clone(),
                IrInstruction::Binary { result_ty, .. } => result_ty.clone(),
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
    emit_block_instructions(out, function, block, ctx, local_plan, value_defs)?;
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
        Terminator::CoroutineYield {
            value,
            resume_block,
        } => {
            let value_ty = value_types.get(value).ok_or_else(|| {
                Diagnostic::new(format!(
                    "missing type for coroutine yield value {:?}",
                    value
                ))
            })?;
            if !matches!(value_ty, Type::Numeric(NumericType::I32)) {
                return Err(Diagnostic::new(format!(
                    "coroutine_yield currently supports i32 values during wasm emission, got {}",
                    value_ty
                )));
            }
            let pc_field = ctx.coroutine_plan.pc_field(&function.name).ok_or_else(|| {
                Diagnostic::new(format!(
                    "missing coroutine pc field for yielding function '{}'",
                    function.name
                ))
            })?;
            let state_ty = ctx.coroutine_state_type()?;
            let yield_tmp = local_plan
                .coroutine_yield_tmp
                .ok_or_else(|| Diagnostic::new("missing coroutine yield scratch local"))?;

            // Spill the yielded value (it may be a stack value) before the struct writes
            // reorder the operand stack.
            emit_value_operand(out, local_plan, *value)?;
            out.instruction(&Instruction::LocalSet(yield_tmp));

            // Runtime check (design 0007): yielding with no coroutine on the stack traps.
            emit_active_state_ref(out, ctx)?;
            out.instruction(&Instruction::RefIsNull);
            out.instruction(&Instruction::If(BlockType::Empty));
            out.instruction(&Instruction::Unreachable);
            out.instruction(&Instruction::End);

            // Save the resume point so re-entry dispatches to the right block.
            emit_active_state_ref(out, ctx)?;
            out.instruction(&Instruction::I32Const(resume_block.0 as i32));
            out.instruction(&Instruction::StructSet {
                struct_type_index: state_ty,
                field_index: pc_field,
            });
            // Deliver the yielded value and mark the instance suspended.
            emit_active_state_ref(out, ctx)?;
            out.instruction(&Instruction::LocalGet(yield_tmp));
            out.instruction(&Instruction::StructSet {
                struct_type_index: state_ty,
                field_index: STATE_YIELDED_FIELD,
            });
            emit_active_state_field_set_const(out, ctx, STATE_TAG_FIELD, TAG_SUSPENDED)?;
            // Unwind the call stack back to `coroutine_resume`, propagating the i32 value
            // for functions that return one.
            if !matches!(function.return_type, Type::Unit) {
                out.instruction(&Instruction::LocalGet(yield_tmp));
            }
            out.instruction(&Instruction::Return);
        }
        Terminator::Return(value) => {
            let return_ty = value_types.get(value).ok_or_else(|| {
                Diagnostic::new(format!("missing type for return value {:?}", value))
            })?;
            // A normal return needs no coroutine bookkeeping: `coroutine_resume` marks the
            // instance finished tentatively before the call, and only `coroutine_yield`
            // flips it back to suspended. So a return that is *not* a yield leaves the
            // finished tag in place, and `coroutine_resume` reads the body's result directly.
            if !matches!(return_ty, Type::Unit) {
                emit_value_operand(out, local_plan, *value)?;
            }
            out.instruction(&Instruction::Return);
        }
        Terminator::Unreachable { .. } => {
            out.instruction(&Instruction::Unreachable);
        }
    }
    Ok(())
}

fn emit_block_instructions(
    out: &mut Function,
    function: &IrFunction,
    block: &BasicBlock,
    ctx: &EmissionContext<'_>,
    local_plan: &LocalPlan,
    value_defs: &HashMap<ValueId, IrInstruction>,
) -> Result<(), Diagnostic> {
    for (value, instruction) in &block.instructions {
        match instruction {
            IrInstruction::Param(_) | IrInstruction::Phi(_) => {}
            IrInstruction::Unit => {
                emit_value_store(out, local_plan, *value)?;
            }
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
            IrInstruction::Print { value: printed } => {
                emit_value_operand(out, local_plan, *printed)?;
                out.instruction(&Instruction::Call(host::IMPORT_PRINT_FUNC));
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::ToString {
                value: source,
                from,
            } => {
                emit_value_operand(out, local_plan, *source)?;
                match from {
                    Type::Numeric(NumericType::I32) => {
                        out.instruction(&Instruction::Call(host::IMPORT_JS_TOSTRING_I32_FUNC));
                    }
                    Type::Numeric(NumericType::U32) => {
                        out.instruction(&Instruction::Call(host::IMPORT_JS_TOSTRING_U32_FUNC));
                    }
                    Type::Numeric(NumericType::I64) => {
                        out.instruction(&Instruction::Call(host::IMPORT_JS_TOSTRING_I64_FUNC));
                    }
                    Type::Numeric(NumericType::U64) => {
                        out.instruction(&Instruction::Call(host::IMPORT_JS_TOSTRING_U64_FUNC));
                    }
                    Type::Numeric(NumericType::F32) => {
                        out.instruction(&Instruction::Call(host::IMPORT_JS_TOSTRING_F32_FUNC));
                    }
                    Type::Numeric(NumericType::F64) => {
                        out.instruction(&Instruction::Call(host::IMPORT_JS_TOSTRING_F64_FUNC));
                    }
                    Type::Bool => {
                        out.instruction(&Instruction::Call(host::IMPORT_JS_TOSTRING_BOOL_FUNC));
                    }
                    Type::String => {}
                    other => {
                        return Err(Diagnostic::new(format!(
                            "tostring is not supported for {} during wasm emission",
                            other
                        )));
                    }
                }
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
                if ctx.coroutine_plan.function_yields(name) {
                    emit_return_if_coroutine_yielded(out, function, ctx)?;
                }
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
                    ctx.signature_registry,
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
            IrInstruction::CoroutineCreate { callee } => {
                let state_ty = ctx.coroutine_state_type()?;
                let body_sig = ctx.coroutine_body_sig_type()?;
                // struct.new $coroutine_state { tag=suspended, yielded=0, continuation, pc*=0 }
                out.instruction(&Instruction::I32Const(TAG_SUSPENDED));
                out.instruction(&Instruction::I32Const(0));
                // Continuation: turn the callee's table index into a typed funcref.
                emit_value_operand(out, local_plan, *callee)?;
                out.instruction(&Instruction::TableGet(0));
                out.instruction(&Instruction::RefCastNonNull(HeapType::Concrete(body_sig)));
                for _ in 0..ctx.coroutine_plan.pc_field_count() {
                    out.instruction(&Instruction::I32Const(0));
                }
                out.instruction(&Instruction::StructNew(state_ty));
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::CoroutineResume { coroutine } => {
                let state_ty = ctx.coroutine_state_type()?;
                let body_sig = ctx.coroutine_body_sig_type()?;
                let active = ctx.coroutine_plan.active_global()?;
                let save_local = local_plan
                    .coroutine_save_local
                    .ok_or_else(|| Diagnostic::new("missing coroutine save local for resume"))?;
                let slots = local_plan.multi_slots.get(value).ok_or_else(|| {
                    Diagnostic::new("coroutine_resume result has no multi-value slots")
                })?;
                let ok_slot = slots[0];
                let value_slot = slots[1];

                // Suspended? Otherwise the coroutine is dead/errored → (false, 0).
                emit_value_operand(out, local_plan, *coroutine)?;
                out.instruction(&Instruction::StructGet {
                    struct_type_index: state_ty,
                    field_index: STATE_TAG_FIELD,
                });
                out.instruction(&Instruction::I32Const(TAG_SUSPENDED));
                out.instruction(&Instruction::I32Eq);
                out.instruction(&Instruction::If(BlockType::Empty));

                // Tentatively mark finished; `coroutine_yield` flips it back to suspended.
                emit_value_operand(out, local_plan, *coroutine)?;
                out.instruction(&Instruction::I32Const(TAG_FINISHED));
                out.instruction(&Instruction::StructSet {
                    struct_type_index: state_ty,
                    field_index: STATE_TAG_FIELD,
                });
                // Save the outer active instance (nested resume), then switch to this one.
                out.instruction(&Instruction::GlobalGet(active));
                out.instruction(&Instruction::LocalSet(save_local));
                emit_value_operand(out, local_plan, *coroutine)?;
                out.instruction(&Instruction::GlobalSet(active));
                // Run the continuation; its i32 result is the yielded or returned value.
                emit_value_operand(out, local_plan, *coroutine)?;
                out.instruction(&Instruction::StructGet {
                    struct_type_index: state_ty,
                    field_index: STATE_CONT_FIELD,
                });
                out.instruction(&Instruction::CallRef(body_sig));
                out.instruction(&Instruction::LocalSet(value_slot));
                // Restore the outer active instance.
                out.instruction(&Instruction::LocalGet(save_local));
                out.instruction(&Instruction::GlobalSet(active));
                // ok = tag != error.
                emit_value_operand(out, local_plan, *coroutine)?;
                out.instruction(&Instruction::StructGet {
                    struct_type_index: state_ty,
                    field_index: STATE_TAG_FIELD,
                });
                out.instruction(&Instruction::I32Const(TAG_ERROR));
                out.instruction(&Instruction::I32Ne);
                out.instruction(&Instruction::LocalSet(ok_slot));

                out.instruction(&Instruction::Else);
                out.instruction(&Instruction::I32Const(0));
                out.instruction(&Instruction::LocalSet(ok_slot));
                out.instruction(&Instruction::I32Const(0));
                out.instruction(&Instruction::LocalSet(value_slot));
                out.instruction(&Instruction::End);
            }
            IrInstruction::CoroutineClose { coroutine } => {
                let state_ty = ctx.coroutine_state_type()?;
                let body_sig = ctx.coroutine_body_sig_type()?;
                // result = (tag != error); if not errored, transition to dead and drop the
                // continuation.
                emit_value_operand(out, local_plan, *coroutine)?;
                out.instruction(&Instruction::StructGet {
                    struct_type_index: state_ty,
                    field_index: STATE_TAG_FIELD,
                });
                out.instruction(&Instruction::I32Const(TAG_ERROR));
                out.instruction(&Instruction::I32Eq);
                out.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
                out.instruction(&Instruction::I32Const(0));
                out.instruction(&Instruction::Else);
                emit_value_operand(out, local_plan, *coroutine)?;
                out.instruction(&Instruction::I32Const(TAG_FINISHED));
                out.instruction(&Instruction::StructSet {
                    struct_type_index: state_ty,
                    field_index: STATE_TAG_FIELD,
                });
                emit_value_operand(out, local_plan, *coroutine)?;
                out.instruction(&Instruction::RefNull(HeapType::Concrete(body_sig)));
                out.instruction(&Instruction::StructSet {
                    struct_type_index: state_ty,
                    field_index: STATE_CONT_FIELD,
                });
                out.instruction(&Instruction::I32Const(1));
                out.instruction(&Instruction::End);
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
                    ctx.signature_registry,
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

/// After calling a function that may yield, unwind toward `coroutine_resume` if the
/// active instance is now suspended (i.e. a yield happened transitively).
fn emit_return_if_coroutine_yielded(
    out: &mut Function,
    function: &IrFunction,
    ctx: &EmissionContext<'_>,
) -> Result<(), Diagnostic> {
    if !matches!(
        function.return_type,
        Type::Unit | Type::Numeric(NumericType::I32)
    ) {
        return Err(Diagnostic::new(format!(
            "delegated coroutine_yield currently supports i32 or unit returns, got {}",
            function.return_type
        )));
    }
    let state_ty = ctx.coroutine_state_type()?;
    // Skip entirely when no coroutine is running (the callee was invoked directly).
    emit_active_state_ref(out, ctx)?;
    out.instruction(&Instruction::RefIsNull);
    out.instruction(&Instruction::If(BlockType::Empty));
    out.instruction(&Instruction::Else);
    emit_active_state_field_get(out, ctx, STATE_TAG_FIELD)?;
    out.instruction(&Instruction::I32Const(TAG_SUSPENDED));
    out.instruction(&Instruction::I32Eq);
    out.instruction(&Instruction::If(BlockType::Empty));
    if matches!(function.return_type, Type::Numeric(NumericType::I32)) {
        emit_active_state_ref(out, ctx)?;
        out.instruction(&Instruction::StructGet {
            struct_type_index: state_ty,
            field_index: STATE_YIELDED_FIELD,
        });
    }
    out.instruction(&Instruction::Return);
    out.instruction(&Instruction::End);
    out.instruction(&Instruction::End);
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
        | IrInstruction::String(_) => Vec::new(),
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
    )
}

fn instruction_can_consume_stack_value(instruction: &IrInstruction, value: ValueId) -> bool {
    match instruction {
        IrInstruction::Param(_)
        | IrInstruction::Number { .. }
        | IrInstruction::Unit
        | IrInstruction::Bool(_)
        | IrInstruction::String(_) => false,
        IrInstruction::ToString { .. } => false,
        IrInstruction::Cast { value: source, .. } => *source == value,
        IrInstruction::Binary { left, .. } => *left == value,
        IrInstruction::MathIntrinsic { args, .. } => args.first().copied() == Some(value),
        IrInstruction::Print { value: printed } => *printed == value,
        IrInstruction::Call { args, .. } => args.first().copied() == Some(value),
        IrInstruction::CallValue { .. } => false,
        IrInstruction::CoroutineCreate { .. } => false,
        IrInstruction::CoroutineResume { coroutine, .. }
        | IrInstruction::CoroutineClose { coroutine, .. } => *coroutine == value,
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

fn emit_value_store(
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
            Type::Function { .. } | Type::Record(_) | Type::TypeParam(_) | Type::Thread => {
                unreachable!()
            }
            Type::Unit => unreachable!(),
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
            Type::Function { .. } | Type::Record(_) | Type::TypeParam(_) | Type::Thread => {
                unreachable!()
            }
            Type::Unit => unreachable!(),
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
            Type::Function { .. } | Type::Record(_) | Type::TypeParam(_) | Type::Thread => {
                unreachable!()
            }
            Type::Unit => unreachable!(),
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
            Type::Function { .. } | Type::Record(_) | Type::TypeParam(_) | Type::Thread => {
                unreachable!()
            }
            Type::Unit => unreachable!(),
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
            Type::Function { .. } | Type::Record(_) | Type::TypeParam(_) | Type::Thread => {
                unreachable!()
            }
            Type::Unit => unreachable!(),
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
            Type::Function { .. } | Type::Record(_) | Type::TypeParam(_) | Type::Thread => {
                unreachable!()
            }
            Type::Unit => unreachable!(),
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
            Type::Function { .. } | Type::Record(_) | Type::TypeParam(_) | Type::Thread => {
                unreachable!()
            }
            Type::Unit => unreachable!(),
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

#[cfg(test)]
mod tests;
