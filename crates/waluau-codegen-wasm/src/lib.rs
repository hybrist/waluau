use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

use waluau_ast::{BinaryOp, NumberLiteral, NumericType, SymbolId, Type};
use waluau_diagnostics::Diagnostic;
use waluau_ir::{
    BasicBlock, BitwiseIntrinsic, Function as IrFunction, Instruction as IrInstruction,
    MathIntrinsic, Module, Terminator, ValueId,
};
use wasm_encoder::{
    AbstractHeapType, BlockType, CodeSection, ConstExpr, CustomSection, ElementSection, Elements,
    EntityType, ExportKind, ExportSection, FieldType, Function, FunctionSection, GlobalSection,
    HeapType, ImportSection, Instruction, Module as WasmModule, RefType, StartSection, StorageType,
    TableSection, TableType, TypeSection, ValType,
};
use wasmparser::{Validator, WasmFeatures};

mod arrays;
mod coroutines;
pub mod host;
mod locals;
mod signatures;
mod wasm_types;

use arrays::{
    ArrayTypeRegistry, array_storage_type, collect_array_types, collect_record_types,
    record_storage_type,
};
use coroutines::{
    AWAIT_STATUS_FULFILLED, AWAIT_STATUS_NONE, AWAIT_STATUS_REJECTED, CoroutinePlan,
    STATE_AWAIT_STATUS_FIELD, STATE_CONT_FIELD, STATE_TAG_FIELD, STATE_YIELDED_FIELD,
    TAG_AWAITING_PROMISE, TAG_ERROR, TAG_FINISHED, TAG_SUSPENDED, coroutine_state_ref_type,
};
use locals::{
    LocalPlan, build_local_plan, build_value_definition_map, emit_value_operand, emit_value_store,
    infer_value_types, local,
};
use signatures::{SignatureRegistry, collect_user_signatures};
use wasm_types::{
    anyref_val_type, compress_locals, externref_nonnull_val_type, externref_val_type, wasm_type,
};

const CALLBACK_EVENT_UNIT_TRAMPOLINE_EXPORT: &str = "__waluau_call_callback_event_unit";
const CALLBACK_UNIT_EXTERN_TRAMPOLINE_EXPORT: &str = "__waluau_call_callback_unit_extern";
const PROMISE_RESUME_TRAMPOLINE_EXPORT: &str = "__waluau_resume_promise_await";
const PROMISE_RESET_ACTIVE_EXPORT: &str = "__waluau_reset_active_coroutine";

/// Collect the ordered set of function names that appear as `Closure` targets in the module.
/// These each need a wrapper function with the env-based calling convention.
fn collect_closure_targets(module: &Module) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut names = Vec::new();
    for function in &module.functions {
        for block in function.blocks.values() {
            for (_, inst) in &block.instructions {
                if let IrInstruction::Closure { name, .. } = inst {
                    if seen.insert(name.clone()) {
                        names.push(name.clone());
                    }
                }
            }
        }
    }
    names
}

fn type_contains_function_value(ty: &Type) -> bool {
    match ty {
        Type::Function { .. } => true,
        Type::Array(inner) | Type::Nullable(inner) => type_contains_function_value(inner),
        Type::Multi(types) => types.iter().any(type_contains_function_value),
        Type::Record(fields) => fields.values().any(type_contains_function_value),
        _ => false,
    }
}

fn is_event_unit_callback_type(ty: &Type) -> bool {
    let Type::Function {
        params,
        return_type,
    } = ty
    else {
        return false;
    };

    matches!(return_type.as_ref(), Type::Unit) && matches!(params.as_slice(), [Type::Extern])
}

fn is_unit_extern_callback_type(ty: &Type) -> bool {
    let Type::Function {
        params,
        return_type,
    } = ty
    else {
        return false;
    };

    params.is_empty() && is_promise_like_extern_type(return_type)
}

fn is_promise_like_extern_type(ty: &Type) -> bool {
    match ty {
        Type::Extern | Type::ExternSubtype(_) => true,
        Type::Opaque { ty, .. } => is_promise_like_extern_type(ty),
        _ => false,
    }
}

fn needs_callback_event_unit_trampoline(module: &Module) -> bool {
    module
        .declared_imports
        .iter()
        .any(|declared| declared.params.iter().any(is_event_unit_callback_type))
}

fn needs_callback_unit_extern_trampoline(module: &Module) -> bool {
    module
        .declared_imports
        .iter()
        .any(|declared| declared.params.iter().any(is_unit_extern_callback_type))
}

fn needs_promise_resume_trampoline(module: &Module) -> bool {
    module.functions.iter().any(|function| {
        function
            .blocks
            .values()
            .any(|block| matches!(block.terminator, Terminator::CoroutineAwaitPromise { .. }))
    })
}

/// Returns `true` if the module uses any features that require the closure GC types
/// (`$anyref_array`, `$func_val`, `$boxed_f64`) to be declared in the type section.
fn needs_closure_gc_types(module: &Module) -> bool {
    for import in &module.declared_imports {
        if import.params.iter().any(type_contains_function_value)
            || type_contains_function_value(&import.return_type)
        {
            return true;
        }
    }
    for function in &module.functions {
        // A function-typed parameter or return references `$func_val` via `wasm_type`.
        if function
            .params
            .iter()
            .any(|(_, ty)| matches!(ty, Type::Function { .. }))
            || matches!(function.return_type, Type::Function { .. })
        {
            return true;
        }
        for block in function.blocks.values() {
            for (_, instruction) in &block.instructions {
                match instruction {
                    // Closure creation and indirect calls require all three GC types.
                    IrInstruction::Closure { .. } | IrInstruction::CallValue { .. } => {
                        return true;
                    }
                    // Casting to/from `unknown` with f64 uses `$boxed_f64`.
                    IrInstruction::Cast { from, to, .. } => {
                        if matches!(
                            (from, to),
                            (Type::Numeric(waluau_ast::NumericType::F64), Type::Unknown)
                                | (Type::Unknown, Type::Numeric(waluau_ast::NumericType::F64))
                        ) {
                            return true;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    false
}

#[derive(Debug)]
pub struct EmitResult {
    pub wasm: Vec<u8>,
    pub record_type_indices: HashMap<String, u32>,
}

pub fn emit(module: &Module) -> Result<EmitResult, Diagnostic> {
    let array_types = collect_array_types(module);
    let record_types = collect_record_types(module);
    let string_constants = host::collect_string_constants(module);
    let bytes_constants = host::collect_bytes_constants(module);
    let coroutine_plan = CoroutinePlan::new(module, string_constants.len() as u32);
    let start_thunk = module.start;
    let host_type_base = array_types.len() as u32;

    // Determine which host imports the module actually uses, and build the
    // index remapping so callers can use canonical slot numbers.
    let used_imports = host::collect_used_host_imports(module);
    let import_map = used_imports.to_import_map();

    // Only emit host function type entries for the slots that are actually used.
    // Build a map from canonical slot index (0–8) to the actual type-section index.
    let needed_host_slots = host::needed_host_type_slots(&used_imports);
    let mut host_slot_type_index = [None::<u32>; host::HOST_TYPE_COUNT as usize];
    let mut actual_host_type_count = 0u32;
    for (slot, &needed) in needed_host_slots.iter().enumerate() {
        if needed {
            host_slot_type_index[slot] = Some(host_type_base + actual_host_type_count);
            actual_host_type_count += 1;
        }
    }

    // Closure GC types ($anyref_array, $func_val, $boxed_f64) are only needed when
    // the program uses closures, function-typed values, or f64 boxing into unknown.
    let callback_event_unit_trampoline = needs_callback_event_unit_trampoline(module);
    let callback_unit_extern_trampoline = needs_callback_unit_extern_trampoline(module);
    let promise_resume_trampoline = needs_promise_resume_trampoline(module);
    let closure_gc_needed = needs_closure_gc_types(module);
    // Two closure GC types sit after host types (only when needed):
    //   $anyref_array = (array (ref null any) mutable)
    //   $func_val = (struct { func_idx: i32, env: ref null $anyref_array })
    let closure_gc_base = host_type_base + actual_host_type_count;
    let (anyref_array_type, func_val_struct_type, boxed_f64_struct_type) = if closure_gc_needed {
        (closure_gc_base, closure_gc_base + 1, closure_gc_base + 2)
    } else {
        // Dummy values — never referenced when closure GC types are absent.
        (0, 0, 0)
    };
    let closure_gc_count = if closure_gc_needed { 3 } else { 0 };
    // Coroutine GC types sit after the closure GC types.
    let coroutine_types_base = closure_gc_base + closure_gc_count;
    let coroutine_state_type = coroutine_plan.has_state().then_some(coroutine_types_base);
    let coroutine_type_count = if coroutine_plan.has_state() { 1 } else { 0 };
    let record_types_base = coroutine_types_base + coroutine_type_count;
    let user_type_base = record_types_base + record_types.len() as u32;
    // Array types come first in the type section (indices 0..N-1).
    let mut array_registry = ArrayTypeRegistry::with_function_type_offset(
        &array_types,
        &record_types,
        0,
        record_types_base,
        anyref_array_type,
        func_val_struct_type,
        boxed_f64_struct_type,
    );
    array_registry.coroutine_state_type = coroutine_state_type;

    let signature_registry = collect_user_signatures(
        module,
        start_thunk.is_some(),
        callback_event_unit_trampoline,
        callback_unit_extern_trampoline,
        promise_resume_trampoline,
    );
    let coroutine_body_wrapper_type = if coroutine_plan.has_state() {
        Some(
            signature_registry
                .get_wrapper_type_index(user_type_base, &[], &Type::Numeric(NumericType::I32))
                .ok_or_else(|| Diagnostic::new("missing () -> i32 coroutine wrapper signature"))?,
        )
    } else {
        None
    };

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
    // Emit only the host function type entries that are actually used by this module.
    // The canonical slots and their signatures are documented in `host::needed_host_type_slots`.
    let host_type_specs: [(&[ValType], &[ValType]); host::HOST_TYPE_COUNT as usize] = [
        (
            &[externref_val_type(), externref_val_type()],
            &[ValType::I32],
        ),
        (
            &[externref_val_type(), externref_val_type()],
            &[externref_nonnull_val_type()],
        ),
        (&[ValType::I32], &[externref_val_type()]),
        (&[ValType::I64], &[externref_val_type()]),
        (&[ValType::F32], &[externref_val_type()]),
        (&[ValType::F64], &[externref_val_type()]),
        (&[externref_val_type()], &[]),
        (&[externref_val_type(), ValType::I32], &[ValType::I32]),
        (&[externref_val_type()], &[ValType::I32]),
        (&[anyref_val_type()], &[externref_val_type()]),
        (&[ValType::F64, ValType::F64], &[ValType::F64]),
    ];
    for (slot, (params, results)) in host_type_specs.iter().enumerate() {
        if needed_host_slots[slot] {
            types
                .ty()
                .function(params.iter().copied(), results.iter().copied());
        }
    }
    // Closure GC types: only emitted when the program actually uses closures,
    // function-typed values, or f64 boxing into unknown.
    if closure_gc_needed {
        // $anyref_array = (array (ref null any) mutable)
        let anyref_storage = StorageType::Val(ValType::Ref(RefType {
            nullable: true,
            heap_type: HeapType::Abstract {
                shared: false,
                ty: AbstractHeapType::Any,
            },
        }));
        types.ty().array(&anyref_storage, true);
        // $func_val = (struct {
        //   func_idx: i32 (mut)    — original function's table slot (for coroutine use)
        //   env: ref null $anyref_array (mut) — capture-cell env for wrapper calls
        //   wrapper_idx: i32 (mut) — wrapper table slot (for call_indirect)
        // })
        types.ty().struct_(vec![
            FieldType {
                element_type: StorageType::Val(ValType::I32),
                mutable: true,
            },
            FieldType {
                element_type: StorageType::Val(ValType::Ref(RefType {
                    nullable: true,
                    heap_type: HeapType::Concrete(anyref_array_type),
                })),
                mutable: true,
            },
            FieldType {
                element_type: StorageType::Val(ValType::I32),
                mutable: true,
            },
        ]);
        // $boxed_f64 = (struct (field f64)) — immutable box for f64 → anyref.
        types.ty().struct_(vec![FieldType {
            element_type: StorageType::Val(ValType::F64),
            mutable: false,
        }]);
    }
    // Coroutine GC types (before user function types so `thread` params can reference them).
    if let Some(state_type) = coroutine_state_type {
        // State struct: { tag:i32, yielded:anyref, continuation:func_val,
        // await_status:i32, pc_*:i32 }.
        let mut fields = vec![
            FieldType {
                element_type: StorageType::Val(ValType::I32),
                mutable: true,
            },
            FieldType {
                element_type: StorageType::Val(anyref_val_type()),
                mutable: true,
            },
            FieldType {
                element_type: StorageType::Val(ValType::Ref(RefType {
                    nullable: true,
                    heap_type: HeapType::Concrete(func_val_struct_type),
                })),
                mutable: true,
            },
            FieldType {
                element_type: StorageType::Val(ValType::I32),
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
    // Record struct types used by sealed tables/records.
    for record_ty in &record_types {
        let Type::Record(fields) = record_ty else {
            continue;
        };
        let mut wasm_fields = Vec::with_capacity(fields.len());
        for field_ty in fields.values() {
            wasm_fields.push(FieldType {
                element_type: record_storage_type(field_ty, &array_registry)?,
                mutable: true,
            });
        }
        types.ty().struct_(wasm_fields);
    }
    // Emit user function types (logical signatures).
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
    // Emit wrapper function types for closure call_indirect: (env, logical_params...) -> returns.
    let env_val_type = ValType::Ref(RefType {
        nullable: true,
        heap_type: HeapType::Concrete(anyref_array_type),
    });
    for (params, return_type) in &signature_registry.wrapper_sigs {
        let mut wrapper_params = vec![env_val_type];
        for ty in params {
            wrapper_params.push(wasm_type(ty, &array_registry)?);
        }
        let results = match return_type {
            Type::Multi(multi_types) => multi_types
                .iter()
                .map(|ty| wasm_type(ty, &array_registry))
                .collect::<Result<Vec<_>, _>>()?,
            Type::Unit => Vec::new(),
            other => vec![wasm_type(other, &array_registry)?],
        };
        types.ty().function(wrapper_params, results);
    }

    // Record type helper functions signatures and indices mapping.
    let unique_sig_count = signature_registry.unique_signatures.len() as u32;
    let wrapper_sig_count = signature_registry.wrapper_sigs.len() as u32;
    let mut type_idx_counter = user_type_base + unique_sig_count + wrapper_sig_count;
    let callback_event_unit_trampoline_type_idx = if callback_event_unit_trampoline {
        let callback_type = Type::Function {
            params: vec![Type::Extern],
            return_type: Box::new(Type::Unit),
        };
        types.ty().function(
            vec![
                wasm_type(&callback_type, &array_registry)?,
                externref_val_type(),
            ],
            Vec::<ValType>::new(),
        );
        let type_idx = type_idx_counter;
        type_idx_counter += 1;
        Some(type_idx)
    } else {
        None
    };
    let callback_unit_extern_trampoline_type_idx = if callback_unit_extern_trampoline {
        let callback_type = Type::Function {
            params: Vec::new(),
            return_type: Box::new(Type::Extern),
        };
        types.ty().function(
            vec![wasm_type(&callback_type, &array_registry)?],
            vec![externref_val_type()],
        );
        let type_idx = type_idx_counter;
        type_idx_counter += 1;
        Some(type_idx)
    } else {
        None
    };
    let promise_resume_trampoline_type_idx = if promise_resume_trampoline {
        Some(
            signature_registry
                .get(
                    &[Type::Thread, Type::Extern, Type::Numeric(NumericType::I32)],
                    &Type::Unit,
                )
                .map(|sig| user_type_base + sig)
                .ok_or_else(|| Diagnostic::new("missing promise resume trampoline signature"))?,
        )
    } else {
        None
    };
    let attach_promise_import_type_idx = if used_imports.attach_promise {
        Some(type_idx_counter)
    } else {
        None
    };
    if let Some(type_idx) = attach_promise_import_type_idx {
        types.ty().function(
            vec![
                coroutine_state_ref_type(coroutine_state_type.ok_or_else(|| {
                    Diagnostic::new("missing coroutine state type for Promise attach import")
                })?),
                externref_val_type(),
            ],
            Vec::<ValType>::new(),
        );
        type_idx_counter = type_idx + 1;
    }
    let promise_reset_active_type_idx = if promise_resume_trampoline {
        Some(
            signature_registry
                .get(&[], &Type::Unit)
                .map(|sig| user_type_base + sig)
                .ok_or_else(|| Diagnostic::new("missing promise reset trampoline signature"))?,
        )
    } else {
        None
    };

    struct RecordHelpersInfo {
        record_idx: u32,
        constructor_type_idx: u32,
        getter_type_indices: Vec<u32>,
    }

    let mut record_helpers = Vec::new();
    for (i, record_ty) in record_types.iter().enumerate() {
        let Type::Record(fields) = record_ty else {
            continue;
        };
        let record_idx = record_types_base + i as u32;

        // Constructor signature: (field0, field1...) -> ref null record_idx
        let mut constructor_params = Vec::new();
        for field_ty in fields.values() {
            constructor_params.push(wasm_type(field_ty, &array_registry)?);
        }
        let constructor_result = ValType::Ref(RefType {
            nullable: true,
            heap_type: HeapType::Concrete(record_idx),
        });
        types
            .ty()
            .function(constructor_params, vec![constructor_result]);
        let constructor_type_idx = type_idx_counter;
        type_idx_counter += 1;

        // Getters signatures: for each field, (ref null record_idx) -> field_type
        let mut getter_type_indices = Vec::new();
        for field_ty in fields.values() {
            let getter_param = ValType::Ref(RefType {
                nullable: true,
                heap_type: HeapType::Concrete(record_idx),
            });
            let getter_result = wasm_type(field_ty, &array_registry)?;
            types.ty().function(vec![getter_param], vec![getter_result]);
            getter_type_indices.push(type_idx_counter);
            type_idx_counter += 1;
        }

        record_helpers.push(RecordHelpersInfo {
            record_idx,
            constructor_type_idx,
            getter_type_indices,
        });
    }

    let mut imports = ImportSection::new();
    // Emit only the host function imports that the module actually uses.
    if used_imports.js_string_eq {
        imports.import(
            host::JS_STRING_BUILTINS_MODULE,
            host::IMPORT_JS_STRING_EQ,
            EntityType::Function(host_slot_type_index[0].unwrap()),
        );
    }
    if used_imports.js_string_concat {
        imports.import(
            host::JS_STRING_BUILTINS_MODULE,
            host::IMPORT_JS_STRING_CONCAT,
            EntityType::Function(host_slot_type_index[1].unwrap()),
        );
    }
    if used_imports.js_string_compare {
        imports.import(
            host::JS_STRING_BUILTINS_MODULE,
            host::IMPORT_JS_STRING_COMPARE,
            EntityType::Function(host_slot_type_index[0].unwrap()),
        );
    }
    if used_imports.bytes_literal {
        imports.import(
            host::IMPORT_MODULE,
            host::IMPORT_BYTES_LITERAL,
            EntityType::Function(host_slot_type_index[2].unwrap()),
        );
    }
    if used_imports.bytes_get {
        imports.import(
            host::IMPORT_MODULE,
            host::IMPORT_BYTES_GET,
            EntityType::Function(host_slot_type_index[7].unwrap()),
        );
    }
    if used_imports.bytes_len {
        imports.import(
            host::IMPORT_MODULE,
            host::IMPORT_BYTES_LEN,
            EntityType::Function(host_slot_type_index[8].unwrap()),
        );
    }
    if used_imports.bytes_concat {
        imports.import(
            host::IMPORT_MODULE,
            host::IMPORT_BYTES_CONCAT,
            EntityType::Function(host_slot_type_index[1].unwrap()),
        );
    }
    if used_imports.bytes_eq {
        imports.import(
            host::IMPORT_MODULE,
            host::IMPORT_BYTES_EQ,
            EntityType::Function(host_slot_type_index[0].unwrap()),
        );
    }
    if used_imports.bytes_compare {
        imports.import(
            host::IMPORT_MODULE,
            host::IMPORT_BYTES_COMPARE,
            EntityType::Function(host_slot_type_index[0].unwrap()),
        );
    }
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
    if used_imports.print {
        imports.import(
            host::IMPORT_MODULE,
            host::IMPORT_PRINT,
            EntityType::Function(host_slot_type_index[6].unwrap()),
        );
    }
    if used_imports.js_tostring_i32 {
        imports.import(
            host::IMPORT_MODULE,
            host::IMPORT_JS_TOSTRING_I32,
            EntityType::Function(host_slot_type_index[2].unwrap()),
        );
    }
    if used_imports.js_tostring_u32 {
        imports.import(
            host::IMPORT_MODULE,
            host::IMPORT_JS_TOSTRING_U32,
            EntityType::Function(host_slot_type_index[2].unwrap()),
        );
    }
    if used_imports.js_tostring_i64 {
        imports.import(
            host::IMPORT_MODULE,
            host::IMPORT_JS_TOSTRING_I64,
            EntityType::Function(host_slot_type_index[3].unwrap()),
        );
    }
    if used_imports.js_tostring_u64 {
        imports.import(
            host::IMPORT_MODULE,
            host::IMPORT_JS_TOSTRING_U64,
            EntityType::Function(host_slot_type_index[3].unwrap()),
        );
    }
    if used_imports.js_tostring_f32 {
        imports.import(
            host::IMPORT_MODULE,
            host::IMPORT_JS_TOSTRING_F32,
            EntityType::Function(host_slot_type_index[4].unwrap()),
        );
    }
    if used_imports.js_tostring_f64 {
        imports.import(
            host::IMPORT_MODULE,
            host::IMPORT_JS_TOSTRING_F64,
            EntityType::Function(host_slot_type_index[5].unwrap()),
        );
    }
    if used_imports.js_tostring_bool {
        imports.import(
            host::IMPORT_MODULE,
            host::IMPORT_JS_TOSTRING_BOOL,
            EntityType::Function(host_slot_type_index[2].unwrap()),
        );
    }
    if used_imports.js_tostring_unknown {
        imports.import(
            host::IMPORT_MODULE,
            host::IMPORT_JS_TOSTRING_UNKNOWN,
            EntityType::Function(host_slot_type_index[9].unwrap()),
        );
    }
    if used_imports.extern_is {
        imports.import(
            host::IMPORT_MODULE,
            host::IMPORT_EXTERN_IS,
            EntityType::Function(host_slot_type_index[0].unwrap()),
        );
    }
    if used_imports.attach_promise {
        imports.import(
            host::IMPORT_MODULE,
            host::IMPORT_ATTACH_PROMISE,
            EntityType::Function(
                attach_promise_import_type_idx
                    .ok_or_else(|| Diagnostic::new("missing Promise attach import type index"))?,
            ),
        );
    }
    if used_imports.math_pow {
        imports.import(
            host::IMPORT_MODULE,
            host::IMPORT_MATH_POW,
            EntityType::Function(host_slot_type_index[10].unwrap()),
        );
    }
    let mut declared_import_indices = HashMap::new();
    for (offset, declared) in module.declared_imports.iter().enumerate() {
        let sig_index = signature_registry
            .get(&declared.params, &declared.return_type)
            .ok_or_else(|| {
                Diagnostic::new(format!(
                    "missing signature for declared host function '{}'",
                    declared.name
                ))
            })?;
        imports.import(
            &declared.module,
            &declared.host_name,
            EntityType::Function(user_type_base + sig_index),
        );
        declared_import_indices.insert(declared.symbol_id, import_map.count + offset as u32);
    }
    let import_func_count = import_map.count + module.declared_imports.len() as u32;

    // Build wrapper slot map: function name → table slot index for its wrapper.
    // Wrappers are placed in table slots N..N+W-1 (after the N user-defined functions).
    let closure_targets = collect_closure_targets(module);
    let closure_wrapper_slots: HashMap<String, u32> = closure_targets
        .iter()
        .enumerate()
        .map(|(i, name)| (name.clone(), module.functions.len() as u32 + i as u32))
        .collect();

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
        if should_export_function(&function.name) {
            exports.export(
                &function.name,
                ExportKind::Func,
                import_func_count + index as u32,
            );
        }
        codes.function(&emit_function(
            function,
            &signatures,
            &signature_registry,
            &array_registry,
            &string_constants,
            &bytes_constants,
            user_type_base,
            &coroutine_plan,
            coroutine_body_wrapper_type,
            &closure_wrapper_slots,
            &import_map,
            &declared_import_indices,
            import_func_count,
        )?);
    }
    if let Some(start) = start_thunk {
        let thunk_sig_index = signature_registry.get(&[], &Type::Unit).unwrap();
        functions.function(user_type_base + thunk_sig_index);
        let mut thunk = Function::new(Vec::new());
        thunk.instruction(&Instruction::Call(import_func_count + start as u32));
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
    // Emit closure wrapper functions (after user functions and optional start thunk).
    // Each wrapper has signature (env: ref null $anyref_array, logical_params...) -> logical_returns
    // and dispatches to the original function, extracting captures from the env array.
    let thunk_offset = if start_thunk.is_some() { 1u32 } else { 0 };
    for (wrapper_idx, name) in closure_targets.iter().enumerate() {
        let target_fn = module
            .functions
            .iter()
            .find(|f| f.name == *name)
            .ok_or_else(|| {
                Diagnostic::new(format!("closure target '{name}' not found in module"))
            })?;
        let target_sig = signatures.get(name).ok_or_else(|| {
            Diagnostic::new(format!("missing signature for closure target '{name}'"))
        })?;
        let logical_params: Vec<Type> = target_fn.params[target_fn.capture_count..]
            .iter()
            .map(|(_, ty)| ty.clone())
            .collect();
        let wrapper_type_idx = signature_registry
            .get_wrapper_type_index(user_type_base, &logical_params, &target_fn.return_type)
            .ok_or_else(|| Diagnostic::new(format!("missing wrapper type for closure '{name}'")))?;
        functions.function(wrapper_type_idx);
        let wrapper_fn = emit_closure_wrapper(
            target_fn,
            target_sig,
            &logical_params,
            &array_registry,
            import_func_count,
        )?;
        codes.function(&wrapper_fn);
        let _ = wrapper_idx;
    }

    let mut helper_func_idx_counter =
        module.functions.len() as u32 + thunk_offset + closure_targets.len() as u32;
    if let Some(type_idx) = callback_event_unit_trampoline_type_idx {
        functions.function(type_idx);
        exports.export(
            CALLBACK_EVENT_UNIT_TRAMPOLINE_EXPORT,
            ExportKind::Func,
            import_func_count + helper_func_idx_counter,
        );
        helper_func_idx_counter += 1;
        codes.function(&emit_callback_event_unit_trampoline(
            &signature_registry,
            &array_registry,
            user_type_base,
        )?);
    }
    if let Some(type_idx) = callback_unit_extern_trampoline_type_idx {
        functions.function(type_idx);
        exports.export(
            CALLBACK_UNIT_EXTERN_TRAMPOLINE_EXPORT,
            ExportKind::Func,
            import_func_count + helper_func_idx_counter,
        );
        helper_func_idx_counter += 1;
        codes.function(&emit_callback_unit_extern_trampoline(
            &signature_registry,
            &array_registry,
            user_type_base,
        )?);
    }
    if let Some(type_idx) = promise_resume_trampoline_type_idx {
        functions.function(type_idx);
        exports.export(
            PROMISE_RESUME_TRAMPOLINE_EXPORT,
            ExportKind::Func,
            import_func_count + helper_func_idx_counter,
        );
        helper_func_idx_counter += 1;
        codes.function(&emit_promise_resume_trampoline(
            &coroutine_plan,
            coroutine_body_wrapper_type.ok_or_else(|| {
                Diagnostic::new("missing coroutine body wrapper type for promise resume trampoline")
            })?,
            coroutine_state_type.ok_or_else(|| {
                Diagnostic::new("missing coroutine state type for promise resume trampoline")
            })?,
            func_val_struct_type,
        )?);
    }
    if let Some(type_idx) = promise_reset_active_type_idx {
        functions.function(type_idx);
        exports.export(
            PROMISE_RESET_ACTIVE_EXPORT,
            ExportKind::Func,
            import_func_count + helper_func_idx_counter,
        );
        helper_func_idx_counter += 1;
        codes.function(&emit_reset_active_coroutine(
            &coroutine_plan,
            coroutine_state_type.ok_or_else(|| {
                Diagnostic::new("missing coroutine state type for promise reset helper")
            })?,
        )?);
    }

    // Emit record helper functions (constructors and getters)
    let mut record_helper_idx = 0;
    for record_ty in &record_types {
        let Type::Record(fields) = record_ty else {
            continue;
        };
        let info = &record_helpers[record_helper_idx];
        record_helper_idx += 1;

        // 1. Emit constructor
        functions.function(info.constructor_type_idx);
        exports.export(
            &format!("__waluau_new_record_{}", info.record_idx),
            ExportKind::Func,
            import_func_count + helper_func_idx_counter,
        );
        helper_func_idx_counter += 1;

        let mut constructor_fn = Function::new(Vec::new());
        for f in 0..fields.len() {
            constructor_fn.instruction(&Instruction::LocalGet(f as u32));
        }
        constructor_fn.instruction(&Instruction::StructNew(info.record_idx));
        constructor_fn.instruction(&Instruction::End);
        codes.function(&constructor_fn);

        // 2. Emit getters
        for (field_idx, _field_name) in fields.keys().enumerate() {
            functions.function(info.getter_type_indices[field_idx]);
            exports.export(
                &format!("__waluau_get_record_{}_{}", info.record_idx, field_idx),
                ExportKind::Func,
                import_func_count + helper_func_idx_counter,
            );
            helper_func_idx_counter += 1;

            let mut getter_fn = Function::new(Vec::new());
            getter_fn.instruction(&Instruction::LocalGet(0));
            getter_fn.instruction(&Instruction::StructGet {
                struct_type_index: info.record_idx,
                field_index: field_idx as u32,
            });
            getter_fn.instruction(&Instruction::End);
            codes.function(&getter_fn);
        }
    }

    let defined_func_count = module.functions.len() as u64;
    let wrapper_count = closure_targets.len() as u64;
    let table_size = import_func_count as u64 + defined_func_count + wrapper_count;
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        table64: false,
        minimum: table_size,
        maximum: Some(table_size),
        shared: false,
    });
    // Element segment: user functions at slots 0..N-1, wrappers at slots N..N+W-1.
    let mut table_inits: Vec<u32> = (0..module.functions.len() as u32)
        .map(|i| import_func_count + i)
        .collect();
    for i in 0..closure_targets.len() as u32 {
        let wrapper_module_idx =
            import_func_count + module.functions.len() as u32 + thunk_offset + i;
        table_inits.push(wrapper_module_idx);
    }
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
            function_index: import_func_count + module.functions.len() as u32,
        });
    }
    wasm.section(&elements);
    wasm.section(&codes);
    wasm.section(&CustomSection {
        name: host::BYTES_CUSTOM_SECTION_NAME.into(),
        data: Cow::Owned(host::encode_bytes_constants_section(&bytes_constants)),
    });

    let bytes = wasm.finish();
    let features = WasmFeatures::all();
    Validator::new_with_features(features)
        .validate_all(&bytes)
        .map_err(|err| Diagnostic::new(format!("emitted invalid wasm: {err}")))?;

    let record_type_indices = array_registry.record_indices.clone();

    Ok(EmitResult {
        wasm: bytes,
        record_type_indices,
    })
}

fn should_export_function(name: &str) -> bool {
    name != "__waluau_top_level_init" && !name.starts_with("__waluau_") && !name.contains("$lambda")
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
    bytes_constants: &'a [Vec<u8>],
    user_type_base: u32,
    coroutine_plan: &'a CoroutinePlan,
    /// Wrapper type index for zero-arg coroutine bodies stored as `func_val` continuations.
    coroutine_body_wrapper_type: Option<u32>,
    /// Map from closure-target function name to its wrapper table slot index.
    closure_wrapper_slots: &'a HashMap<String, u32>,
    /// Remapping from canonical host-import slots to actual Wasm function indices.
    import_map: &'a host::HostImportMap,
    declared_import_indices: &'a HashMap<SymbolId, u32>,
    import_func_count: u32,
}

impl EmissionContext<'_> {
    /// Wasm function index for a user-defined function (0-based user index).
    fn wasm_func_index(&self, user_index: u32) -> u32 {
        self.import_func_count + user_index
    }

    /// Wasm function index for a host import identified by its canonical slot.
    fn host_func_index(&self, canonical: u32) -> Result<u32, Diagnostic> {
        self.import_map.func_index(canonical)
    }

    fn declared_host_func_index(&self, symbol_id: SymbolId) -> Result<u32, Diagnostic> {
        self.declared_import_indices
            .get(&symbol_id)
            .copied()
            .ok_or_else(|| {
                Diagnostic::new(format!(
                    "internal error: declared host import symbol @{} used but not emitted",
                    symbol_id.0
                ))
            })
    }

    fn coroutine_state_type(&self) -> Result<u32, Diagnostic> {
        self.array_registry.coroutine_state_type()
    }

    fn coroutine_body_wrapper_type(&self) -> Result<u32, Diagnostic> {
        self.coroutine_body_wrapper_type
            .ok_or_else(|| Diagnostic::new("missing coroutine body wrapper type"))
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_function(
    function: &IrFunction,
    signatures: &HashMap<String, FunctionSignature>,
    signature_registry: &SignatureRegistry,
    array_registry: &ArrayTypeRegistry,
    string_constants: &[String],
    bytes_constants: &[Vec<u8>],
    user_type_base: u32,
    coroutine_plan: &CoroutinePlan,
    coroutine_body_wrapper_type: Option<u32>,
    closure_wrapper_slots: &HashMap<String, u32>,
    import_map: &host::HostImportMap,
    declared_import_indices: &HashMap<SymbolId, u32>,
    import_func_count: u32,
) -> Result<Function, Diagnostic> {
    let ctx = EmissionContext {
        signatures,
        signature_registry,
        array_registry,
        string_constants,
        bytes_constants,
        user_type_base,
        coroutine_plan,
        coroutine_body_wrapper_type,
        closure_wrapper_slots,
        import_map,
        declared_import_indices,
        import_func_count,
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

/// Emit a closure wrapper function for `target_fn`.
///
/// The wrapper has signature `(env: ref null $anyref_array, logical_params...) -> logical_returns`.
/// It extracts the capture-cell arrays from `env` (one per captured variable) and calls the
/// underlying function, which expects `(capture_cells..., logical_params...)`.
fn emit_closure_wrapper(
    target_fn: &IrFunction,
    target_sig: &FunctionSignature,
    logical_params: &[Type],
    array_registry: &ArrayTypeRegistry,
    import_count: u32,
) -> Result<Function, Diagnostic> {
    let capture_count = target_fn.capture_count;
    // Wrapper has no extra locals (all work is done via params and the stack).
    let mut out = Function::new(Vec::new());

    // For each capture slot: load its array-cell ref from the env array.
    // env is param 0; captures are elements 0..C-1 in the env array.
    for i in 0..capture_count {
        let capture_ty = &target_fn.params[i].1; // Array(T) for the capture cell
        out.instruction(&Instruction::LocalGet(0)); // env
        out.instruction(&Instruction::I32Const(i as i32));
        out.instruction(&Instruction::ArrayGet(array_registry.anyref_array_type));
        // Cast the anyref element back to the specific capture-cell array type.
        let heap_type = HeapType::Concrete(array_registry.index(capture_ty)?);
        out.instruction(&Instruction::RefCastNullable(heap_type));
    }

    // Push logical params: they are wrapper params 1..P (0-indexed, after env).
    for j in 0..logical_params.len() {
        out.instruction(&Instruction::LocalGet(1 + j as u32));
    }

    // Call the original function (which takes capture_cells... + logical_params...).
    out.instruction(&Instruction::Call(import_count + target_sig.index));
    out.instruction(&Instruction::End);
    Ok(out)
}

/// Exported browser-host ABI helper for MVP DOM events.
///
/// Signature: `(callback: (extern) -> unit, event: externref) -> unit`.
/// It uses the same `$func_val` wrapper table slot that normal Waluau `CallValue`
/// dispatch uses, so captured closures keep the existing representation.
fn emit_callback_event_unit_trampoline(
    signature_registry: &SignatureRegistry,
    array_registry: &ArrayTypeRegistry,
    user_type_base: u32,
) -> Result<Function, Diagnostic> {
    let wrapper_type_idx = signature_registry
        .get_wrapper_type_index(user_type_base, &[Type::Extern], &Type::Unit)
        .ok_or_else(|| Diagnostic::new("missing (extern) -> unit callback wrapper type"))?;

    let mut out = Function::new(Vec::new());
    // Push wrapper env, logical event argument, and wrapper table slot.
    out.instruction(&Instruction::LocalGet(0));
    out.instruction(&Instruction::StructGet {
        struct_type_index: array_registry.func_val_struct_type,
        field_index: 1,
    });
    out.instruction(&Instruction::LocalGet(1));
    out.instruction(&Instruction::LocalGet(0));
    out.instruction(&Instruction::StructGet {
        struct_type_index: array_registry.func_val_struct_type,
        field_index: 2,
    });
    out.instruction(&Instruction::CallIndirect {
        type_index: wrapper_type_idx,
        table_index: 0,
    });
    out.instruction(&Instruction::End);
    Ok(out)
}

/// Exported browser-host ABI helper for synchronous host callbacks.
///
/// Signature: `(callback: () -> extern) -> externref`.
fn emit_callback_unit_extern_trampoline(
    signature_registry: &SignatureRegistry,
    array_registry: &ArrayTypeRegistry,
    user_type_base: u32,
) -> Result<Function, Diagnostic> {
    let wrapper_type_idx = signature_registry
        .get_wrapper_type_index(user_type_base, &[], &Type::Extern)
        .ok_or_else(|| Diagnostic::new("missing () -> extern callback wrapper type"))?;

    let mut out = Function::new(Vec::new());
    out.instruction(&Instruction::LocalGet(0));
    out.instruction(&Instruction::StructGet {
        struct_type_index: array_registry.func_val_struct_type,
        field_index: 1,
    });
    out.instruction(&Instruction::LocalGet(0));
    out.instruction(&Instruction::StructGet {
        struct_type_index: array_registry.func_val_struct_type,
        field_index: 2,
    });
    out.instruction(&Instruction::CallIndirect {
        type_index: wrapper_type_idx,
        table_index: 0,
    });
    out.instruction(&Instruction::End);
    Ok(out)
}

/// Exported JS-settlement ABI helper for `coroutine.await_promise`.
///
/// Signature: `(thread_handle: thread, payload: extern, rejected: i32) -> unit`.
fn emit_call_coroutine_continuation(
    out: &mut Function,
    state_local: u32,
    state_ty: u32,
    func_val_ty: u32,
    wrapper_type_idx: u32,
) {
    out.instruction(&Instruction::LocalGet(state_local));
    out.instruction(&Instruction::StructGet {
        struct_type_index: state_ty,
        field_index: STATE_CONT_FIELD,
    });
    out.instruction(&Instruction::StructGet {
        struct_type_index: func_val_ty,
        field_index: 1,
    });
    out.instruction(&Instruction::LocalGet(state_local));
    out.instruction(&Instruction::StructGet {
        struct_type_index: state_ty,
        field_index: STATE_CONT_FIELD,
    });
    out.instruction(&Instruction::StructGet {
        struct_type_index: func_val_ty,
        field_index: 2,
    });
    out.instruction(&Instruction::CallIndirect {
        type_index: wrapper_type_idx,
        table_index: 0,
    });
}

fn emit_promise_resume_trampoline(
    coroutine_plan: &CoroutinePlan,
    body_wrapper_type: u32,
    state_ty: u32,
    func_val_ty: u32,
) -> Result<Function, Diagnostic> {
    let mut out = Function::new(vec![(2, coroutine_state_ref_type(state_ty))]);
    let active_save_local = 3u32;
    let state_local = 4u32;

    out.instruction(&Instruction::LocalGet(0));
    out.instruction(&Instruction::LocalTee(state_local));
    out.instruction(&Instruction::RefIsNull);
    out.instruction(&Instruction::If(BlockType::Empty));
    out.instruction(&Instruction::Return);
    out.instruction(&Instruction::End);

    out.instruction(&Instruction::LocalGet(state_local));
    out.instruction(&Instruction::StructGet {
        struct_type_index: state_ty,
        field_index: STATE_TAG_FIELD,
    });
    out.instruction(&Instruction::I32Const(TAG_AWAITING_PROMISE));
    out.instruction(&Instruction::I32Eq);
    out.instruction(&Instruction::If(BlockType::Empty));

    out.instruction(&Instruction::LocalGet(state_local));
    out.instruction(&Instruction::LocalGet(1));
    out.instruction(&Instruction::AnyConvertExtern);
    out.instruction(&Instruction::StructSet {
        struct_type_index: state_ty,
        field_index: STATE_YIELDED_FIELD,
    });

    out.instruction(&Instruction::LocalGet(state_local));
    out.instruction(&Instruction::LocalGet(2));
    out.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
    out.instruction(&Instruction::I32Const(AWAIT_STATUS_REJECTED));
    out.instruction(&Instruction::Else);
    out.instruction(&Instruction::I32Const(AWAIT_STATUS_FULFILLED));
    out.instruction(&Instruction::End);
    out.instruction(&Instruction::StructSet {
        struct_type_index: state_ty,
        field_index: STATE_AWAIT_STATUS_FIELD,
    });

    out.instruction(&Instruction::LocalGet(state_local));
    out.instruction(&Instruction::I32Const(TAG_FINISHED));
    out.instruction(&Instruction::StructSet {
        struct_type_index: state_ty,
        field_index: STATE_TAG_FIELD,
    });

    out.instruction(&Instruction::GlobalGet(coroutine_plan.active_global()?));
    out.instruction(&Instruction::LocalSet(active_save_local));
    out.instruction(&Instruction::LocalGet(state_local));
    out.instruction(&Instruction::GlobalSet(coroutine_plan.active_global()?));

    emit_call_coroutine_continuation(
        &mut out,
        state_local,
        state_ty,
        func_val_ty,
        body_wrapper_type,
    );
    out.instruction(&Instruction::Drop);

    out.instruction(&Instruction::LocalGet(active_save_local));
    out.instruction(&Instruction::GlobalSet(coroutine_plan.active_global()?));
    out.instruction(&Instruction::End);
    out.instruction(&Instruction::End);
    Ok(out)
}

fn emit_reset_active_coroutine(
    coroutine_plan: &CoroutinePlan,
    state_ty: u32,
) -> Result<Function, Diagnostic> {
    let mut out = Function::new(Vec::new());
    out.instruction(&Instruction::RefNull(HeapType::Concrete(state_ty)));
    out.instruction(&Instruction::GlobalSet(coroutine_plan.active_global()?));
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

/// Returns `true` if the block is trivially dead: it has no instructions and is unreachable.
/// The IR builder creates such blocks as placeholders (e.g., dead merge blocks after both
/// branches of an if/else both return), so they can appear in the block map but are never
/// actually reached.
fn is_trivially_dead(block: &BasicBlock) -> bool {
    block.instructions.is_empty() && matches!(block.terminator, Terminator::Unreachable { .. })
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

    // Single-block straight-line function: just emit it directly without any loop wrapper.
    if function.blocks.len() == 1 {
        let entry = function
            .blocks
            .get(&function.entry)
            .ok_or_else(|| Diagnostic::new("missing entry block"))?;
        if matches!(
            entry.terminator,
            Terminator::Return(_) | Terminator::Unreachable { .. }
        ) {
            emit_block(
                out,
                function,
                entry,
                ctx,
                value_types,
                local_plan,
                value_defs,
            )?;
            return Ok(true);
        }
    }

    let entry = function
        .blocks
        .get(&function.entry)
        .ok_or_else(|| Diagnostic::new("missing entry block"))?;

    // If/else where both branches return: emit a structured `if/else/end` with no loop wrapper.
    // The IR builder always creates a (dead) merge block even when both arms return, so we use
    // `is_trivially_dead` to allow any number of such placeholder blocks alongside the three
    // real blocks (entry, then, else).
    if let Terminator::Branch {
        condition,
        then_block,
        else_block,
    } = entry.terminator
    {
        let then_bb = function.blocks.get(&then_block);
        let else_bb = function.blocks.get(&else_block);

        // Both branches return → structured if/else.
        if then_bb.is_some_and(|b| matches!(b.terminator, Terminator::Return(_)))
            && else_bb.is_some_and(|b| matches!(b.terminator, Terminator::Return(_)))
            && function
                .blocks
                .values()
                .filter(|b| b.id != function.entry && b.id != then_block && b.id != else_block)
                .all(is_trivially_dead)
        {
            emit_block_instructions(
                out,
                function,
                entry,
                ctx,
                value_types,
                local_plan,
                value_defs,
            )?;
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
            // Both branches executed `return`, so the code here is unreachable.
            // Emit `unreachable` so the wasm validator accepts the empty stack even
            // for non-void functions.
            out.instruction(&Instruction::Unreachable);
            return Ok(true);
        }

        // One-sided if with early return: the then branch returns immediately while the else
        // branch falls through to a single merge block that itself returns.
        //   entry: Branch(cond, then=Return, else=Jump(merge))
        //   merge: Return
        // Any additional placeholder blocks must be trivially dead.
        if let (Some(then_bb), Some(else_bb)) = (then_bb, else_bb) {
            if matches!(then_bb.terminator, Terminator::Return(_)) {
                if let Terminator::Jump(merge_id) = else_bb.terminator {
                    if let Some(merge_bb) = function.blocks.get(&merge_id) {
                        if matches!(merge_bb.terminator, Terminator::Return(_))
                            && function
                                .blocks
                                .values()
                                .filter(|b| {
                                    b.id != function.entry
                                        && b.id != then_block
                                        && b.id != else_block
                                        && b.id != merge_id
                                })
                                .all(is_trivially_dead)
                        {
                            emit_block_instructions(
                                out,
                                function,
                                entry,
                                ctx,
                                value_types,
                                local_plan,
                                value_defs,
                            )?;
                            emit_value_operand(out, local_plan, condition)?;
                            out.instruction(&Instruction::If(BlockType::Empty));
                            emit_phi_copies(out, function, entry.id, then_block, local_plan)?;
                            emit_block(
                                out,
                                function,
                                then_bb,
                                ctx,
                                value_types,
                                local_plan,
                                value_defs,
                            )?;
                            out.instruction(&Instruction::End);
                            emit_phi_copies(out, function, entry.id, else_block, local_plan)?;
                            emit_block_instructions(
                                out,
                                function,
                                else_bb,
                                ctx,
                                value_types,
                                local_plan,
                                value_defs,
                            )?;
                            emit_phi_copies(out, function, else_block, merge_id, local_plan)?;
                            emit_block(
                                out,
                                function,
                                merge_bb,
                                ctx,
                                value_types,
                                local_plan,
                                value_defs,
                            )?;
                            return Ok(true);
                        }
                    }
                }
            }
        }
    }

    if function.blocks.len() == 4 {
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
                emit_block_instructions(
                    out,
                    function,
                    entry,
                    ctx,
                    value_types,
                    local_plan,
                    value_defs,
                )?;
                emit_phi_copies(out, function, entry.id, second.id, local_plan)?;
                out.instruction(&Instruction::Block(BlockType::Empty));
                out.instruction(&Instruction::Loop(BlockType::Empty));
                emit_block_instructions(
                    out,
                    function,
                    second,
                    ctx,
                    value_types,
                    local_plan,
                    value_defs,
                )?;
                emit_value_operand(out, local_plan, condition)?;
                out.instruction(&Instruction::I32Eqz);
                out.instruction(&Instruction::BrIf(1));
                emit_phi_copies(out, function, second.id, then_block, local_plan)?;
                emit_block_instructions(
                    out,
                    function,
                    then_bb.expect("checked above"),
                    ctx,
                    value_types,
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
                emit_block_instructions(
                    out,
                    function,
                    entry,
                    ctx,
                    value_types,
                    local_plan,
                    value_defs,
                )?;
                emit_phi_copies(out, function, entry.id, body.id, local_plan)?;
                out.instruction(&Instruction::Block(BlockType::Empty));
                out.instruction(&Instruction::Loop(BlockType::Empty));
                emit_block_instructions(
                    out,
                    function,
                    body,
                    ctx,
                    value_types,
                    local_plan,
                    value_defs,
                )?;
                emit_phi_copies(out, function, body.id, second.id, local_plan)?;
                emit_block_instructions(
                    out,
                    function,
                    second,
                    ctx,
                    value_types,
                    local_plan,
                    value_defs,
                )?;
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

fn emit_block(
    out: &mut Function,
    function: &IrFunction,
    block: &BasicBlock,
    ctx: &EmissionContext<'_>,
    value_types: &BTreeMap<ValueId, Type>,
    local_plan: &LocalPlan,
    value_defs: &HashMap<ValueId, IrInstruction>,
) -> Result<(), Diagnostic> {
    emit_block_instructions(
        out,
        function,
        block,
        ctx,
        value_types,
        local_plan,
        value_defs,
    )?;
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
            if *value_ty != Type::Unknown {
                return Err(Diagnostic::new(format!(
                    "coroutine.yield expected unknown payload during wasm emission, got {}",
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
            // Unwind the call stack back to `coroutine.resume`. The payload lives in the
            // coroutine state; returning a typed default only satisfies the surrounding
            // function's Wasm signature while delegated-yield checks propagate suspension.
            if !matches!(function.return_type, Type::Unit) {
                emit_default_value(out, &function.return_type, ctx.array_registry)?;
            }
            out.instruction(&Instruction::Return);
        }
        Terminator::CoroutineAwaitPromise {
            promise,
            resume_block,
        } => {
            let promise_ty = value_types.get(promise).ok_or_else(|| {
                Diagnostic::new(format!(
                    "missing type for coroutine await promise {:?}",
                    promise
                ))
            })?;
            if !is_promise_like_extern_type(promise_ty) {
                return Err(Diagnostic::new(format!(
                    "coroutine.await_promise expected extern payload during wasm emission, got {}",
                    promise_ty
                )));
            }
            let pc_field = ctx.coroutine_plan.pc_field(&function.name).ok_or_else(|| {
                Diagnostic::new(format!(
                    "missing coroutine pc field for suspending function '{}'",
                    function.name
                ))
            })?;
            let state_ty = ctx.coroutine_state_type()?;
            let promise_tmp = local_plan
                .coroutine_await_promise_tmp
                .ok_or_else(|| Diagnostic::new("missing coroutine await promise scratch local"))?;
            let attach_promise_idx = ctx
                .import_map
                .func_index(host::IMPORT_ATTACH_PROMISE_FUNC)?;

            emit_value_operand(out, local_plan, *promise)?;
            out.instruction(&Instruction::LocalSet(promise_tmp));

            emit_active_state_ref(out, ctx)?;
            out.instruction(&Instruction::RefIsNull);
            out.instruction(&Instruction::If(BlockType::Empty));
            out.instruction(&Instruction::Unreachable);
            out.instruction(&Instruction::End);

            emit_active_state_ref(out, ctx)?;
            out.instruction(&Instruction::I32Const(resume_block.0 as i32));
            out.instruction(&Instruction::StructSet {
                struct_type_index: state_ty,
                field_index: pc_field,
            });
            emit_active_state_field_set_const(
                out,
                ctx,
                STATE_AWAIT_STATUS_FIELD,
                AWAIT_STATUS_NONE,
            )?;

            emit_active_state_ref(out, ctx)?;
            out.instruction(&Instruction::LocalGet(promise_tmp));
            out.instruction(&Instruction::Call(attach_promise_idx));

            emit_active_state_field_set_const(out, ctx, STATE_TAG_FIELD, TAG_AWAITING_PROMISE)?;
            if !matches!(function.return_type, Type::Unit) {
                emit_default_value(out, &function.return_type, ctx.array_registry)?;
            }
            out.instruction(&Instruction::Return);
        }
        Terminator::Return(value) => {
            let return_ty = value_types.get(value).ok_or_else(|| {
                Diagnostic::new(format!("missing type for return value {:?}", value))
            })?;
            // A normal return needs no coroutine bookkeeping: `coroutine.resume` marks the
            // instance finished tentatively before the call, and only `coroutine.yield`
            // flips it back to suspended. So a return that is *not* a yield leaves the
            // finished tag in place, and `coroutine.resume` reads the body's result directly.
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
    value_types: &BTreeMap<ValueId, Type>,
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
            IrInstruction::Null { ty } => {
                emit_ref_null(out, ty, ctx.array_registry)?;
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::String(literal) => {
                let index = host::string_constant_index(ctx.string_constants, literal)?;
                out.instruction(&Instruction::GlobalGet(index));
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::Bytes(literal) => {
                let index = host::bytes_constant_index(ctx.bytes_constants, literal)?;
                out.instruction(&Instruction::I32Const(index as i32));
                out.instruction(&Instruction::Call(
                    ctx.host_func_index(host::IMPORT_BYTES_LITERAL_FUNC)?,
                ));
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::Cast {
                value: source,
                from,
                to,
            } => {
                emit_value_operand(out, local_plan, *source)?;
                emit_cast(out, from.clone(), to.clone(), ctx.array_registry)?;
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
                } else if matches!(op, BinaryOp::Pow) {
                    let left_local = local(local_plan, *left)?;
                    let right_local = local(local_plan, *right)?;
                    emit_pow(out, ctx, operand_ty.clone(), left_local, right_local)?;
                } else {
                    emit_value_operand(out, local_plan, *left)?;
                    emit_value_operand(out, local_plan, *right)?;
                    emit_binary(out, ctx, *op, operand_ty.clone(), result_ty.clone())?;
                }
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::IsNull {
                value: tested,
                ty: _,
            } => {
                emit_value_operand(out, local_plan, *tested)?;
                out.instruction(&Instruction::RefIsNull);
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::ExternCastTest {
                value: tested,
                target_name,
            } => {
                let index = host::string_constant_index(ctx.string_constants, target_name)?;
                emit_value_operand(out, local_plan, *tested)?;
                out.instruction(&Instruction::GlobalGet(index));
                out.instruction(&Instruction::Call(
                    ctx.host_func_index(host::IMPORT_EXTERN_IS_FUNC)?,
                ));
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
            IrInstruction::BitwiseIntrinsic {
                intrinsic, args, ..
            } => {
                for arg in args {
                    emit_value_operand(out, local_plan, *arg)?;
                }
                emit_bitwise_intrinsic(out, *intrinsic, args.len())?;
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::Print { value: printed } => {
                emit_value_operand(out, local_plan, *printed)?;
                out.instruction(&Instruction::Call(
                    ctx.host_func_index(host::IMPORT_PRINT_FUNC)?,
                ));
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::ToString {
                value: source,
                from,
            } => {
                emit_value_operand(out, local_plan, *source)?;
                match from {
                    Type::Numeric(NumericType::I32) => {
                        out.instruction(&Instruction::Call(
                            ctx.host_func_index(host::IMPORT_JS_TOSTRING_I32_FUNC)?,
                        ));
                    }
                    Type::Numeric(NumericType::U32) => {
                        out.instruction(&Instruction::Call(
                            ctx.host_func_index(host::IMPORT_JS_TOSTRING_U32_FUNC)?,
                        ));
                    }
                    Type::Numeric(NumericType::I64) => {
                        out.instruction(&Instruction::Call(
                            ctx.host_func_index(host::IMPORT_JS_TOSTRING_I64_FUNC)?,
                        ));
                    }
                    Type::Numeric(NumericType::U64) => {
                        out.instruction(&Instruction::Call(
                            ctx.host_func_index(host::IMPORT_JS_TOSTRING_U64_FUNC)?,
                        ));
                    }
                    Type::Numeric(NumericType::F32) => {
                        out.instruction(&Instruction::Call(
                            ctx.host_func_index(host::IMPORT_JS_TOSTRING_F32_FUNC)?,
                        ));
                    }
                    Type::Numeric(NumericType::F64) => {
                        out.instruction(&Instruction::Call(
                            ctx.host_func_index(host::IMPORT_JS_TOSTRING_F64_FUNC)?,
                        ));
                    }
                    Type::Bool => {
                        out.instruction(&Instruction::Call(
                            ctx.host_func_index(host::IMPORT_JS_TOSTRING_BOOL_FUNC)?,
                        ));
                    }
                    Type::Unknown => {
                        out.instruction(&Instruction::Call(
                            ctx.host_func_index(host::IMPORT_JS_TOSTRING_UNKNOWN_FUNC)?,
                        ));
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
            IrInstruction::Call { name, args, .. } => {
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
            IrInstruction::HostCall {
                symbol_id, args, ..
            } => {
                for arg in args {
                    emit_value_operand(out, local_plan, *arg)?;
                }
                out.instruction(&Instruction::Call(
                    ctx.declared_host_func_index(*symbol_id)?,
                ));
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
                // Indirect call via $func_val struct: push (env, args..., wrapper_idx).
                // Load env from field 1 of the $func_val struct.
                emit_value_operand(out, local_plan, *callee)?;
                out.instruction(&Instruction::StructGet {
                    struct_type_index: ctx.array_registry.func_val_struct_type,
                    field_index: 1,
                });
                // Push logical args.
                for arg in args {
                    emit_value_operand(out, local_plan, *arg)?;
                }
                // Load wrapper table slot from field 2 of the $func_val struct.
                emit_value_operand(out, local_plan, *callee)?;
                out.instruction(&Instruction::StructGet {
                    struct_type_index: ctx.array_registry.func_val_struct_type,
                    field_index: 2,
                });
                // call_indirect with wrapper type: (env, logical_params...) -> logical_returns.
                let type_index = ctx
                    .signature_registry
                    .get_wrapper_type_index(ctx.user_type_base, params, return_type)
                    .ok_or_else(|| {
                        Diagnostic::new(format!(
                            "missing wrapper type for indirect call ({}) -> {}",
                            params
                                .iter()
                                .map(|t| t.to_string())
                                .collect::<Vec<_>>()
                                .join(", "),
                            return_type
                        ))
                    })?;
                out.instruction(&Instruction::CallIndirect {
                    type_index,
                    table_index: 0,
                });
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::CoroutineCreate { callee } => {
                let state_ty = ctx.coroutine_state_type()?;
                // struct.new $coroutine_state {
                //   tag=suspended, yielded=null, continuation, await_status=none, pc*=0
                // }
                out.instruction(&Instruction::I32Const(TAG_SUSPENDED));
                out.instruction(&Instruction::RefNull(HeapType::Abstract {
                    shared: false,
                    ty: AbstractHeapType::Any,
                }));
                // Continuation: store the full func_val so resume can dispatch captured and
                // non-capturing coroutine bodies through the zero-arg wrapper uniformly.
                emit_value_operand(out, local_plan, *callee)?;
                out.instruction(&Instruction::I32Const(AWAIT_STATUS_NONE));
                for _ in 0..ctx.coroutine_plan.pc_field_count() {
                    out.instruction(&Instruction::I32Const(0));
                }
                out.instruction(&Instruction::StructNew(state_ty));
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::CoroutineResume { coroutine } => {
                let state_ty = ctx.coroutine_state_type()?;
                let body_wrapper_type = ctx.coroutine_body_wrapper_type()?;
                let active = ctx.coroutine_plan.active_global()?;
                let coroutine_local = local(local_plan, *coroutine)?;
                let save_local = local_plan
                    .coroutine_save_local
                    .ok_or_else(|| Diagnostic::new("missing coroutine save local for resume"))?;
                let value_tmp = local_plan.coroutine_resume_value_tmp.ok_or_else(|| {
                    Diagnostic::new("missing coroutine resume value local for resume")
                })?;
                let slots = local_plan.multi_slots.get(value).ok_or_else(|| {
                    Diagnostic::new("coroutine.resume result has no multi-value slots")
                })?;
                let ok_slot = slots[0];
                let value_slot = slots[1];

                // Suspended? Otherwise the coroutine is dead/errored → (false, null).
                emit_value_operand(out, local_plan, *coroutine)?;
                out.instruction(&Instruction::StructGet {
                    struct_type_index: state_ty,
                    field_index: STATE_TAG_FIELD,
                });
                out.instruction(&Instruction::I32Const(TAG_SUSPENDED));
                out.instruction(&Instruction::I32Eq);
                out.instruction(&Instruction::If(BlockType::Empty));

                // Tentatively mark finished; `coroutine.yield` flips it back to suspended.
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
                emit_call_coroutine_continuation(
                    out,
                    coroutine_local,
                    state_ty,
                    ctx.array_registry.func_val_struct_type,
                    body_wrapper_type,
                );
                out.instruction(&Instruction::LocalSet(value_tmp));
                // Restore the outer active instance.
                out.instruction(&Instruction::LocalGet(save_local));
                out.instruction(&Instruction::GlobalSet(active));
                // yielded path -> state payload, finished path -> box final i32 return.
                emit_value_operand(out, local_plan, *coroutine)?;
                out.instruction(&Instruction::StructGet {
                    struct_type_index: state_ty,
                    field_index: STATE_TAG_FIELD,
                });
                out.instruction(&Instruction::I32Const(TAG_SUSPENDED));
                out.instruction(&Instruction::I32Eq);
                out.instruction(&Instruction::If(BlockType::Result(anyref_val_type())));
                emit_value_operand(out, local_plan, *coroutine)?;
                out.instruction(&Instruction::StructGet {
                    struct_type_index: state_ty,
                    field_index: STATE_YIELDED_FIELD,
                });
                out.instruction(&Instruction::Else);
                emit_value_operand(out, local_plan, *coroutine)?;
                out.instruction(&Instruction::StructGet {
                    struct_type_index: state_ty,
                    field_index: STATE_TAG_FIELD,
                });
                out.instruction(&Instruction::I32Const(TAG_AWAITING_PROMISE));
                out.instruction(&Instruction::I32Eq);
                out.instruction(&Instruction::If(BlockType::Result(anyref_val_type())));
                out.instruction(&Instruction::RefNull(HeapType::Abstract {
                    shared: false,
                    ty: AbstractHeapType::Any,
                }));
                out.instruction(&Instruction::Else);
                out.instruction(&Instruction::LocalGet(value_tmp));
                emit_box(out, &Type::Numeric(NumericType::I32), ctx.array_registry)?;
                out.instruction(&Instruction::End);
                out.instruction(&Instruction::End);
                out.instruction(&Instruction::LocalSet(value_slot));
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
                out.instruction(&Instruction::RefNull(HeapType::Abstract {
                    shared: false,
                    ty: AbstractHeapType::Any,
                }));
                out.instruction(&Instruction::LocalSet(value_slot));
                out.instruction(&Instruction::End);
            }
            IrInstruction::CoroutineResumeTagged {
                coroutine,
                yielded_tag,
                finished_tag,
                error_tag,
            } => {
                let state_ty = ctx.coroutine_state_type()?;
                let body_wrapper_type = ctx.coroutine_body_wrapper_type()?;
                let active = ctx.coroutine_plan.active_global()?;
                let coroutine_local = local(local_plan, *coroutine)?;
                let save_local = local_plan.coroutine_save_local.ok_or_else(|| {
                    Diagnostic::new("missing coroutine save local for tagged resume")
                })?;
                let value_tmp = local_plan.tagged_resume_value_tmp.ok_or_else(|| {
                    Diagnostic::new("missing tagged_resume_value_tmp for tagged resume")
                })?;
                let state_tmp = local_plan.tagged_resume_state_tmp.ok_or_else(|| {
                    Diagnostic::new("missing tagged_resume_state_tmp for tagged resume")
                })?;

                let canonical = Type::canonical_tagged_union_record();
                let record_ty = ctx.array_registry.record_index(&canonical)?;
                let record_val_type = ValType::Ref(RefType {
                    nullable: true,
                    heap_type: HeapType::Concrete(record_ty),
                });

                // Outer if: suspended? else emit error record.
                emit_value_operand(out, local_plan, *coroutine)?;
                out.instruction(&Instruction::StructGet {
                    struct_type_index: state_ty,
                    field_index: STATE_TAG_FIELD,
                });
                out.instruction(&Instruction::I32Const(TAG_SUSPENDED));
                out.instruction(&Instruction::I32Eq);
                out.instruction(&Instruction::If(BlockType::Result(record_val_type)));

                // Tentatively mark as finished; yield flips it back to suspended.
                emit_value_operand(out, local_plan, *coroutine)?;
                out.instruction(&Instruction::I32Const(TAG_FINISHED));
                out.instruction(&Instruction::StructSet {
                    struct_type_index: state_ty,
                    field_index: STATE_TAG_FIELD,
                });
                // Save outer active instance, switch to this coroutine.
                out.instruction(&Instruction::GlobalGet(active));
                out.instruction(&Instruction::LocalSet(save_local));
                emit_value_operand(out, local_plan, *coroutine)?;
                out.instruction(&Instruction::GlobalSet(active));
                // Run the continuation; i32 result is the yielded or returned value.
                emit_call_coroutine_continuation(
                    out,
                    coroutine_local,
                    state_ty,
                    ctx.array_registry.func_val_struct_type,
                    body_wrapper_type,
                );
                out.instruction(&Instruction::LocalSet(value_tmp));
                // Restore outer active.
                out.instruction(&Instruction::LocalGet(save_local));
                out.instruction(&Instruction::GlobalSet(active));
                // Snapshot post-continuation state tag.
                emit_value_operand(out, local_plan, *coroutine)?;
                out.instruction(&Instruction::StructGet {
                    struct_type_index: state_ty,
                    field_index: STATE_TAG_FIELD,
                });
                out.instruction(&Instruction::LocalSet(state_tmp));

                // Compute tag discriminant: suspended→yielded, finished→finished, else→error.
                out.instruction(&Instruction::LocalGet(state_tmp));
                out.instruction(&Instruction::I32Const(TAG_SUSPENDED));
                out.instruction(&Instruction::I32Eq);
                out.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
                out.instruction(&Instruction::I32Const(*yielded_tag));
                out.instruction(&Instruction::Else);
                out.instruction(&Instruction::LocalGet(state_tmp));
                out.instruction(&Instruction::I32Const(TAG_AWAITING_PROMISE));
                out.instruction(&Instruction::I32Eq);
                out.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
                out.instruction(&Instruction::I32Const(*yielded_tag));
                out.instruction(&Instruction::Else);
                out.instruction(&Instruction::LocalGet(state_tmp));
                out.instruction(&Instruction::I32Const(TAG_FINISHED));
                out.instruction(&Instruction::I32Eq);
                out.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
                out.instruction(&Instruction::I32Const(*finished_tag));
                out.instruction(&Instruction::Else);
                out.instruction(&Instruction::I32Const(*error_tag));
                out.instruction(&Instruction::End);
                out.instruction(&Instruction::End);
                out.instruction(&Instruction::End);
                // stack: [i32: tag_discriminant]

                // Compute boxed value: error → null anyref; yielded → state payload;
                // finished → box the final i32 return.
                out.instruction(&Instruction::LocalGet(state_tmp));
                out.instruction(&Instruction::I32Const(TAG_ERROR));
                out.instruction(&Instruction::I32Eq);
                out.instruction(&Instruction::If(BlockType::Result(anyref_val_type())));
                out.instruction(&Instruction::RefNull(HeapType::Abstract {
                    shared: false,
                    ty: AbstractHeapType::Any,
                }));
                out.instruction(&Instruction::Else);
                out.instruction(&Instruction::LocalGet(state_tmp));
                out.instruction(&Instruction::I32Const(TAG_SUSPENDED));
                out.instruction(&Instruction::I32Eq);
                out.instruction(&Instruction::If(BlockType::Result(anyref_val_type())));
                emit_value_operand(out, local_plan, *coroutine)?;
                out.instruction(&Instruction::StructGet {
                    struct_type_index: state_ty,
                    field_index: STATE_YIELDED_FIELD,
                });
                out.instruction(&Instruction::Else);
                out.instruction(&Instruction::LocalGet(state_tmp));
                out.instruction(&Instruction::I32Const(TAG_AWAITING_PROMISE));
                out.instruction(&Instruction::I32Eq);
                out.instruction(&Instruction::If(BlockType::Result(anyref_val_type())));
                out.instruction(&Instruction::RefNull(HeapType::Abstract {
                    shared: false,
                    ty: AbstractHeapType::Any,
                }));
                out.instruction(&Instruction::Else);
                out.instruction(&Instruction::LocalGet(value_tmp));
                emit_box(out, &Type::Numeric(NumericType::I32), ctx.array_registry)?;
                out.instruction(&Instruction::End);
                out.instruction(&Instruction::End);
                out.instruction(&Instruction::End);
                // stack: [i32: tag_discriminant, anyref: value]

                // struct.new; field order: "tag" (field 0) then "value" (field 1) — alphabetical.
                out.instruction(&Instruction::StructNew(record_ty));

                // Else branch: coroutine was dead/errored → return error record directly.
                out.instruction(&Instruction::Else);
                out.instruction(&Instruction::I32Const(*error_tag));
                out.instruction(&Instruction::RefNull(HeapType::Abstract {
                    shared: false,
                    ty: AbstractHeapType::Any,
                }));
                out.instruction(&Instruction::StructNew(record_ty));

                out.instruction(&Instruction::End);
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::CoroutineAwaitResult => {
                let state_ty = ctx.coroutine_state_type()?;
                emit_active_state_ref(out, ctx)?;
                out.instruction(&Instruction::RefIsNull);
                out.instruction(&Instruction::If(BlockType::Empty));
                out.instruction(&Instruction::Unreachable);
                out.instruction(&Instruction::End);

                emit_active_state_field_get(out, ctx, STATE_AWAIT_STATUS_FIELD)?;
                out.instruction(&Instruction::I32Const(AWAIT_STATUS_REJECTED));
                out.instruction(&Instruction::I32Eq);
                out.instruction(&Instruction::If(BlockType::Empty));
                emit_active_state_field_set_const(out, ctx, STATE_TAG_FIELD, TAG_ERROR)?;
                out.instruction(&Instruction::Unreachable);
                out.instruction(&Instruction::End);

                emit_active_state_ref(out, ctx)?;
                out.instruction(&Instruction::StructGet {
                    struct_type_index: state_ty,
                    field_index: STATE_YIELDED_FIELD,
                });
                emit_active_state_field_set_const(
                    out,
                    ctx,
                    STATE_AWAIT_STATUS_FIELD,
                    AWAIT_STATUS_NONE,
                )?;
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::CoroutineClose { coroutine } => {
                let state_ty = ctx.coroutine_state_type()?;
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
                out.instruction(&Instruction::RefNull(HeapType::Concrete(
                    ctx.array_registry.func_val_struct_type,
                )));
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
                captures,
                params: _,
                return_type: _,
            } => {
                // Look up the original function and wrapper table slots.
                let orig_sig = ctx.signatures.get(name).ok_or_else(|| {
                    Diagnostic::new(format!("unknown function '{name}' during wasm emission"))
                })?;
                let wrapper_slot =
                    ctx.closure_wrapper_slots
                        .get(name)
                        .copied()
                        .ok_or_else(|| {
                            Diagnostic::new(format!("no wrapper slot for closure '{name}'"))
                        })?;
                // Build a $func_val struct: { orig_idx, env, wrapper_idx }.
                // struct.new expects fields in declaration order: field 0, field 1, field 2.
                out.instruction(&Instruction::I32Const(orig_sig.index as i32));
                // Build the env array: pack capture-cell refs as anyref elements.
                if captures.is_empty() {
                    out.instruction(&Instruction::RefNull(HeapType::Concrete(
                        ctx.array_registry.anyref_array_type,
                    )));
                } else {
                    for capture in captures {
                        // Each capture is already a GC array ref, which is an anyref subtype.
                        emit_value_operand(out, local_plan, *capture)?;
                    }
                    out.instruction(&Instruction::ArrayNewFixed {
                        array_type_index: ctx.array_registry.anyref_array_type,
                        array_size: captures.len() as u32,
                    });
                }
                out.instruction(&Instruction::I32Const(wrapper_slot as i32));
                out.instruction(&Instruction::StructNew(
                    ctx.array_registry.func_val_struct_type,
                ));
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
                if thread_array_storage_needs_cast(element_ty) {
                    out.instruction(&Instruction::RefCastNullable(HeapType::Concrete(
                        ctx.array_registry.coroutine_state_type()?,
                    )));
                }
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
            IrInstruction::ArraySlice {
                array,
                start,
                element_ty,
            } => {
                let array_local = local(local_plan, *array)?;
                let start_local = local(local_plan, *start)?;
                let array_ty = Type::Array(Box::new(element_ty.clone()));
                let array_type_index = ctx.array_registry.index(&array_ty)?;
                out.instruction(&Instruction::LocalGet(array_local));
                out.instruction(&Instruction::ArrayLen);
                out.instruction(&Instruction::LocalGet(start_local));
                out.instruction(&Instruction::I32Sub);
                out.instruction(&Instruction::ArrayNewDefault(array_type_index));
                out.instruction(&Instruction::LocalSet(local(local_plan, *value)?));

                out.instruction(&Instruction::LocalGet(local(local_plan, *value)?));
                out.instruction(&Instruction::I32Const(0));
                out.instruction(&Instruction::LocalGet(array_local));
                out.instruction(&Instruction::LocalGet(start_local));
                out.instruction(&Instruction::LocalGet(local(local_plan, *value)?));
                out.instruction(&Instruction::ArrayLen);
                out.instruction(&Instruction::ArrayCopy {
                    array_type_index_dst: array_type_index,
                    array_type_index_src: array_type_index,
                });
            }
            IrInstruction::BytesGet { bytes, index } => {
                emit_value_operand(out, local_plan, *bytes)?;
                emit_value_operand(out, local_plan, *index)?;
                out.instruction(&Instruction::Call(
                    ctx.host_func_index(host::IMPORT_BYTES_GET_FUNC)?,
                ));
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::BytesLen { bytes } => {
                emit_value_operand(out, local_plan, *bytes)?;
                out.instruction(&Instruction::Call(
                    ctx.host_func_index(host::IMPORT_BYTES_LEN_FUNC)?,
                ));
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::StructNew { struct_ty, fields } => {
                let struct_type_index = ctx.array_registry.record_index(struct_ty)?;
                for field in fields {
                    emit_value_operand(out, local_plan, *field)?;
                }
                out.instruction(&Instruction::StructNew(struct_type_index));
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::StructGet { base, field, .. } => {
                let base_ty = value_types.get(base).ok_or_else(|| {
                    Diagnostic::new(format!("missing type for struct.get base {:?}", base))
                })?;
                let Type::Record(_) = base_ty else {
                    return Err(Diagnostic::new(format!(
                        "struct.get base must be a record type, got {}",
                        base_ty
                    )));
                };
                let struct_type_index = ctx.array_registry.record_index(base_ty)?;
                let field_index = ctx.array_registry.record_field_index(base_ty, field)?;
                emit_value_operand(out, local_plan, *base)?;
                out.instruction(&Instruction::StructGet {
                    struct_type_index,
                    field_index,
                });
                emit_value_store(out, local_plan, *value)?;
            }
            IrInstruction::StructSet {
                base,
                field,
                value: stored,
            } => {
                let base_ty = value_types.get(base).ok_or_else(|| {
                    Diagnostic::new(format!("missing type for struct.set base {:?}", base))
                })?;
                let Type::Record(_) = base_ty else {
                    return Err(Diagnostic::new(format!(
                        "struct.set base must be a record type, got {}",
                        base_ty
                    )));
                };
                let struct_type_index = ctx.array_registry.record_index(base_ty)?;
                let field_index = ctx.array_registry.record_field_index(base_ty, field)?;
                emit_value_operand(out, local_plan, *base)?;
                emit_value_operand(out, local_plan, *stored)?;
                out.instruction(&Instruction::StructSet {
                    struct_type_index,
                    field_index,
                });
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

fn emit_ref_null(
    out: &mut Function,
    ty: &Type,
    array_registry: &ArrayTypeRegistry,
) -> Result<(), Diagnostic> {
    match ty {
        Type::Extern | Type::String | Type::Bytes | Type::Nil => {
            out.instruction(&Instruction::RefNull(HeapType::Abstract {
                shared: false,
                ty: AbstractHeapType::Extern,
            }));
            Ok(())
        }
        Type::Nullable(inner) => emit_ref_null(out, inner, array_registry),
        Type::Unknown => {
            out.instruction(&Instruction::RefNull(HeapType::Abstract {
                shared: false,
                ty: AbstractHeapType::Any,
            }));
            Ok(())
        }
        Type::Array(_) => {
            out.instruction(&Instruction::RefNull(HeapType::Concrete(
                array_registry.index(ty)?,
            )));
            Ok(())
        }
        Type::Record(_) | Type::TaggedVariant(_) | Type::TaggedUnion(_) => {
            let record_ty = if matches!(ty, Type::TaggedVariant(_) | Type::TaggedUnion(_)) {
                Cow::Owned(Type::canonical_tagged_union_record())
            } else {
                Cow::Borrowed(ty)
            };
            out.instruction(&Instruction::RefNull(HeapType::Concrete(
                array_registry.record_index(&record_ty)?,
            )));
            Ok(())
        }
        Type::Function { .. } => {
            out.instruction(&Instruction::RefNull(HeapType::Concrete(
                array_registry.func_val_struct_type,
            )));
            Ok(())
        }
        Type::Thread => {
            out.instruction(&Instruction::RefNull(HeapType::Concrete(
                array_registry.coroutine_state_type()?,
            )));
            Ok(())
        }
        other => Err(Diagnostic::new(format!(
            "cannot lower null literal for non-reference type {other}"
        ))),
    }
}

fn emit_default_value(
    out: &mut Function,
    ty: &Type,
    array_registry: &ArrayTypeRegistry,
) -> Result<(), Diagnostic> {
    match ty {
        Type::Bool | Type::Numeric(NumericType::U32 | NumericType::I32) => {
            out.instruction(&Instruction::I32Const(0));
            Ok(())
        }
        Type::Numeric(NumericType::U64 | NumericType::I64) => {
            out.instruction(&Instruction::I64Const(0));
            Ok(())
        }
        Type::Numeric(NumericType::F32) => {
            out.instruction(&Instruction::F32Const(0.0));
            Ok(())
        }
        Type::Numeric(NumericType::F64) => {
            out.instruction(&Instruction::F64Const(0.0));
            Ok(())
        }
        Type::Unit => Ok(()),
        _ => emit_ref_null(out, ty, array_registry),
    }
}

/// After calling a function that may suspend, unwind toward the current resume entrypoint
/// if the active instance yielded or started awaiting a Promise transitively.
fn emit_return_if_coroutine_yielded(
    out: &mut Function,
    function: &IrFunction,
    ctx: &EmissionContext<'_>,
) -> Result<(), Diagnostic> {
    // Skip entirely when no coroutine is running (the callee was invoked directly).
    emit_active_state_ref(out, ctx)?;
    out.instruction(&Instruction::RefIsNull);
    out.instruction(&Instruction::If(BlockType::Empty));
    out.instruction(&Instruction::Else);
    emit_active_state_field_get(out, ctx, STATE_TAG_FIELD)?;
    out.instruction(&Instruction::I32Const(TAG_SUSPENDED));
    out.instruction(&Instruction::I32Eq);
    emit_active_state_field_get(out, ctx, STATE_TAG_FIELD)?;
    out.instruction(&Instruction::I32Const(TAG_AWAITING_PROMISE));
    out.instruction(&Instruction::I32Eq);
    out.instruction(&Instruction::I32Or);
    out.instruction(&Instruction::If(BlockType::Empty));
    if !matches!(function.return_type, Type::Unit) {
        emit_default_value(out, &function.return_type, ctx.array_registry)?;
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

fn emit_numeric_const(
    out: &mut Function,
    ty: NumericType,
    literal: &NumberLiteral,
) -> Result<(), Diagnostic> {
    match ty {
        NumericType::U32 => {
            out.instruction(&Instruction::I32Const(parse_u32_literal(literal)? as i32));
        }
        NumericType::I32 => {
            out.instruction(&Instruction::I32Const(parse_i32_literal(literal)?));
        }
        NumericType::U64 => {
            out.instruction(&Instruction::I64Const(parse_u64_literal(literal)? as i64));
        }
        NumericType::I64 => {
            out.instruction(&Instruction::I64Const(parse_i64_literal(literal)?));
        }
        NumericType::F32 => {
            out.instruction(&Instruction::F32Const(parse_f64_literal(literal)? as f32));
        }
        NumericType::F64 => {
            out.instruction(&Instruction::F64Const(parse_f64_literal(literal)?));
        }
    }

    Ok(())
}

fn normalized_numeric_literal(literal: &NumberLiteral) -> String {
    literal.raw.replace('_', "")
}

fn hex_digits(raw: &str) -> Option<&str> {
    raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X"))
}

fn parse_u128_literal(literal: &NumberLiteral, ty_name: &str) -> Result<u128, Diagnostic> {
    let raw = normalized_numeric_literal(literal);
    if raw.contains('.') {
        return Err(Diagnostic::new(format!(
            "invalid {ty_name} numeric literal during wasm emission"
        )));
    }
    if let Some(hex) = hex_digits(&raw) {
        return u128::from_str_radix(hex, 16).map_err(|_| {
            Diagnostic::new(format!(
                "invalid {ty_name} numeric literal during wasm emission"
            ))
        });
    }
    raw.parse::<u128>().map_err(|_| {
        Diagnostic::new(format!(
            "invalid {ty_name} numeric literal during wasm emission"
        ))
    })
}

fn parse_i128_literal(literal: &NumberLiteral, ty_name: &str) -> Result<i128, Diagnostic> {
    let raw = normalized_numeric_literal(literal);
    if raw.contains('.') {
        return Err(Diagnostic::new(format!(
            "invalid {ty_name} numeric literal during wasm emission"
        )));
    }
    if let Some(hex) = hex_digits(&raw) {
        let value = u128::from_str_radix(hex, 16).map_err(|_| {
            Diagnostic::new(format!(
                "invalid {ty_name} numeric literal during wasm emission"
            ))
        })?;
        return i128::try_from(value).map_err(|_| {
            Diagnostic::new(format!(
                "invalid {ty_name} numeric literal during wasm emission"
            ))
        });
    }
    raw.parse::<i128>().map_err(|_| {
        Diagnostic::new(format!(
            "invalid {ty_name} numeric literal during wasm emission"
        ))
    })
}

fn parse_u32_literal(literal: &NumberLiteral) -> Result<u32, Diagnostic> {
    u32::try_from(parse_u128_literal(literal, "u32")?)
        .map_err(|_| Diagnostic::new("invalid u32 numeric literal during wasm emission"))
}

fn parse_i32_literal(literal: &NumberLiteral) -> Result<i32, Diagnostic> {
    i32::try_from(parse_i128_literal(literal, "i32")?)
        .map_err(|_| Diagnostic::new("invalid i32 numeric literal during wasm emission"))
}

fn parse_u64_literal(literal: &NumberLiteral) -> Result<u64, Diagnostic> {
    u64::try_from(parse_u128_literal(literal, "u64")?)
        .map_err(|_| Diagnostic::new("invalid u64 numeric literal during wasm emission"))
}

fn parse_i64_literal(literal: &NumberLiteral) -> Result<i64, Diagnostic> {
    i64::try_from(parse_i128_literal(literal, "i64")?)
        .map_err(|_| Diagnostic::new("invalid i64 numeric literal during wasm emission"))
}

fn parse_f64_literal(literal: &NumberLiteral) -> Result<f64, Diagnostic> {
    let raw = normalized_numeric_literal(literal);
    if let Some(hex) = hex_digits(&raw) {
        return u128::from_str_radix(hex, 16)
            .map(|value| value as f64)
            .map_err(|_| Diagnostic::new("invalid f64 numeric literal during wasm emission"));
    }
    raw.parse::<f64>()
        .map_err(|_| Diagnostic::new("invalid f64 numeric literal during wasm emission"))
}

fn emit_cast(
    out: &mut Function,
    from: Type,
    to: Type,
    array_registry: &ArrayTypeRegistry,
) -> Result<(), Diagnostic> {
    if from == to {
        return Ok(());
    }
    if from.nullable_inner().as_ref() == Some(&to) {
        return Ok(());
    }

    // Boxing a primitive into `unknown` (anyref), or unboxing it back out.
    if to == Type::Unknown {
        return emit_box(out, &from, array_registry);
    }
    if from == Type::Unknown {
        return emit_unbox(out, &to, array_registry);
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

/// `(ref i31)` heap type — the boxed representation of small integers and bools.
fn i31_heap_type() -> HeapType {
    HeapType::Abstract {
        shared: false,
        ty: AbstractHeapType::I31,
    }
}

/// Box a primitive value (already on the stack) into an `anyref` (`unknown`).
///
/// `i32` and `bool` use `i31ref` (the value is truncated to 31 bits, matching the
/// design's small-integer boxing); `f64` is wrapped in a `$boxed_f64` struct.
fn emit_box(
    out: &mut Function,
    from: &Type,
    array_registry: &ArrayTypeRegistry,
) -> Result<(), Diagnostic> {
    match from {
        Type::Numeric(NumericType::I32 | NumericType::U32) | Type::Bool => {
            out.instruction(&Instruction::RefI31);
            Ok(())
        }
        Type::Numeric(NumericType::F64) => {
            out.instruction(&Instruction::StructNew(
                array_registry.boxed_f64_struct_type,
            ));
            Ok(())
        }
        Type::Extern | Type::ExternSubtype(_) | Type::String | Type::Bytes | Type::Nil => {
            out.instruction(&Instruction::AnyConvertExtern);
            Ok(())
        }
        Type::Array(_) | Type::Function { .. } | Type::Record(_) | Type::Thread => Ok(()),
        Type::Unit => {
            out.instruction(&Instruction::RefNull(HeapType::Abstract {
                shared: false,
                ty: AbstractHeapType::Any,
            }));
            Ok(())
        }
        other => Err(Diagnostic::new(format!(
            "boxing {other} into unknown is not yet supported during wasm emission",
        ))),
    }
}

/// Unbox an `anyref` (`unknown`, on the stack) back into a concrete primitive.
/// Traps at runtime if the boxed value does not match the requested type.
fn emit_unbox(
    out: &mut Function,
    to: &Type,
    array_registry: &ArrayTypeRegistry,
) -> Result<(), Diagnostic> {
    match to {
        Type::Numeric(NumericType::I32 | NumericType::U32) | Type::Bool => {
            out.instruction(&Instruction::RefCastNonNull(i31_heap_type()));
            out.instruction(&Instruction::I31GetS);
            Ok(())
        }
        Type::Numeric(NumericType::F64) => {
            out.instruction(&Instruction::RefCastNonNull(HeapType::Concrete(
                array_registry.boxed_f64_struct_type,
            )));
            out.instruction(&Instruction::StructGet {
                struct_type_index: array_registry.boxed_f64_struct_type,
                field_index: 0,
            });
            Ok(())
        }
        Type::Extern | Type::ExternSubtype(_) | Type::String | Type::Bytes => {
            out.instruction(&Instruction::ExternConvertAny);
            Ok(())
        }
        Type::Array(_) => {
            out.instruction(&Instruction::RefCastNullable(HeapType::Concrete(
                array_registry.index(to)?,
            )));
            Ok(())
        }
        Type::Function { .. } => {
            out.instruction(&Instruction::RefCastNullable(HeapType::Concrete(
                array_registry.func_val_struct_type,
            )));
            Ok(())
        }
        Type::Record(_) | Type::TaggedVariant(_) | Type::TaggedUnion(_) => {
            let canonical = if matches!(to, Type::TaggedVariant(_) | Type::TaggedUnion(_)) {
                Type::canonical_tagged_union_record()
            } else {
                to.clone()
            };
            out.instruction(&Instruction::RefCastNullable(HeapType::Concrete(
                array_registry.record_index(&canonical)?,
            )));
            Ok(())
        }
        Type::Thread => {
            out.instruction(&Instruction::RefCastNullable(HeapType::Concrete(
                array_registry.coroutine_state_type()?,
            )));
            Ok(())
        }
        Type::Unit => {
            out.instruction(&Instruction::Drop);
            Ok(())
        }
        other => Err(Diagnostic::new(format!(
            "unboxing unknown into {other} is not yet supported during wasm emission",
        ))),
    }
}

fn thread_array_storage_needs_cast(ty: &Type) -> bool {
    match ty {
        Type::Thread => true,
        Type::Nullable(inner) => thread_array_storage_needs_cast(inner),
        _ => false,
    }
}

fn emit_binary(
    out: &mut Function,
    ctx: &EmissionContext<'_>,
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
            Type::Bytes => {
                return Err(Diagnostic::new(
                    "bytes add is not supported during wasm emission",
                ));
            }
            Type::Extern | Type::ExternSubtype(_) | Type::Named { .. } | Type::Opaque { .. } => {
                unreachable!()
            }
            Type::Array(_) => unreachable!(),
            Type::Multi(_) => {
                return Err(Diagnostic::new(
                    "multi-value add is not supported during wasm emission",
                ));
            }
            Type::TaggedVariant(_) | Type::TaggedUnion(_) => {
                return Err(Diagnostic::new(
                    "tagged unions are not yet supported during wasm emission",
                ));
            }
            Type::Function { .. }
            | Type::Record(_)
            | Type::TypeParam(_)
            | Type::Thread
            | Type::Unknown => {
                unreachable!()
            }
            Type::Nil | Type::Nullable(_) | Type::Unit => unreachable!(),
        },
        BinaryOp::Concat => match operand_ty {
            Type::String => {
                out.instruction(&Instruction::Call(
                    ctx.host_func_index(host::IMPORT_JS_STRING_CONCAT_FUNC)?,
                ));
            }
            Type::Bytes => {
                out.instruction(&Instruction::Call(
                    ctx.host_func_index(host::IMPORT_BYTES_CONCAT_FUNC)?,
                ));
            }
            _ => {
                return Err(Diagnostic::new(
                    "concat is only supported for strings and bytes during wasm emission",
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
            Type::Bytes => {
                return Err(Diagnostic::new(
                    "bytes sub is not supported during wasm emission",
                ));
            }
            Type::Extern | Type::ExternSubtype(_) | Type::Named { .. } | Type::Opaque { .. } => {
                unreachable!()
            }
            Type::Array(_) => unreachable!(),
            Type::Multi(_) => {
                return Err(Diagnostic::new(
                    "multi-value sub is not supported during wasm emission",
                ));
            }
            Type::TaggedVariant(_) | Type::TaggedUnion(_) => {
                return Err(Diagnostic::new(
                    "tagged unions are not yet supported during wasm emission",
                ));
            }
            Type::Function { .. }
            | Type::Record(_)
            | Type::TypeParam(_)
            | Type::Thread
            | Type::Unknown => {
                unreachable!()
            }
            Type::Nil | Type::Nullable(_) | Type::Unit => unreachable!(),
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
            Type::Bytes => {
                return Err(Diagnostic::new(
                    "bytes mul is not supported during wasm emission",
                ));
            }
            Type::Extern | Type::ExternSubtype(_) | Type::Named { .. } | Type::Opaque { .. } => {
                unreachable!()
            }
            Type::Array(_) => unreachable!(),
            Type::Multi(_) => {
                return Err(Diagnostic::new(
                    "multi-value mul is not supported during wasm emission",
                ));
            }
            Type::TaggedVariant(_) | Type::TaggedUnion(_) => {
                return Err(Diagnostic::new(
                    "tagged unions are not yet supported during wasm emission",
                ));
            }
            Type::Function { .. }
            | Type::Record(_)
            | Type::TypeParam(_)
            | Type::Thread
            | Type::Unknown => {
                unreachable!()
            }
            Type::Nil | Type::Nullable(_) | Type::Unit => unreachable!(),
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
            Type::Bytes => {
                return Err(Diagnostic::new(
                    "bytes div is not supported during wasm emission",
                ));
            }
            Type::Extern | Type::ExternSubtype(_) | Type::Named { .. } | Type::Opaque { .. } => {
                unreachable!()
            }
            Type::Array(_) => unreachable!(),
            Type::Multi(_) => {
                return Err(Diagnostic::new(
                    "multi-value div is not supported during wasm emission",
                ));
            }
            Type::TaggedVariant(_) | Type::TaggedUnion(_) => {
                return Err(Diagnostic::new(
                    "tagged unions are not yet supported during wasm emission",
                ));
            }
            Type::Function { .. }
            | Type::Record(_)
            | Type::TypeParam(_)
            | Type::Thread
            | Type::Unknown => {
                unreachable!()
            }
            Type::Nil | Type::Nullable(_) | Type::Unit => unreachable!(),
        },
        BinaryOp::FloorDiv | BinaryOp::Mod | BinaryOp::Pow => {
            unreachable!("handled before stack binary emission")
        }
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
                out.instruction(&Instruction::Call(
                    ctx.host_func_index(host::IMPORT_JS_STRING_EQ_FUNC)?,
                ));
            }
            Type::Bytes => {
                out.instruction(&Instruction::Call(
                    ctx.host_func_index(host::IMPORT_BYTES_EQ_FUNC)?,
                ));
            }
            Type::Extern | Type::ExternSubtype(_) | Type::Named { .. } | Type::Opaque { .. } => {
                unreachable!()
            }
            Type::Array(_) => unreachable!(),
            Type::Multi(_) => {
                return Err(Diagnostic::new(
                    "multi-value equality is not supported during wasm emission",
                ));
            }
            Type::TaggedVariant(_) | Type::TaggedUnion(_) => {
                return Err(Diagnostic::new(
                    "tagged unions are not yet supported during wasm emission",
                ));
            }
            Type::Function { .. }
            | Type::Record(_)
            | Type::TypeParam(_)
            | Type::Thread
            | Type::Unknown => {
                unreachable!()
            }
            Type::Nil | Type::Nullable(_) | Type::Unit => unreachable!(),
        },
        BinaryOp::NotEq => {
            emit_binary(out, ctx, BinaryOp::Eq, operand_ty, Type::Bool)?;
            out.instruction(&Instruction::I32Eqz);
        }
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
                out.instruction(&Instruction::Call(
                    ctx.host_func_index(host::IMPORT_JS_STRING_COMPARE_FUNC)?,
                ));
                out.instruction(&Instruction::I32Const(0));
                out.instruction(&Instruction::I32LtS);
            }
            Type::Bytes => {
                out.instruction(&Instruction::Call(
                    ctx.host_func_index(host::IMPORT_BYTES_COMPARE_FUNC)?,
                ));
                out.instruction(&Instruction::I32Const(0));
                out.instruction(&Instruction::I32LtS);
            }
            Type::Extern | Type::ExternSubtype(_) | Type::Named { .. } | Type::Opaque { .. } => {
                unreachable!()
            }
            Type::Array(_) => unreachable!(),
            Type::Multi(_) => {
                return Err(Diagnostic::new(
                    "multi-value comparison is not supported during wasm emission",
                ));
            }
            Type::TaggedVariant(_) | Type::TaggedUnion(_) => {
                return Err(Diagnostic::new(
                    "tagged unions are not yet supported during wasm emission",
                ));
            }
            Type::Function { .. }
            | Type::Record(_)
            | Type::TypeParam(_)
            | Type::Thread
            | Type::Unknown => {
                unreachable!()
            }
            Type::Nil | Type::Nullable(_) | Type::Unit => unreachable!(),
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
                out.instruction(&Instruction::Call(
                    ctx.host_func_index(host::IMPORT_JS_STRING_COMPARE_FUNC)?,
                ));
                out.instruction(&Instruction::I32Const(0));
                out.instruction(&Instruction::I32GtS);
            }
            Type::Bytes => {
                out.instruction(&Instruction::Call(
                    ctx.host_func_index(host::IMPORT_BYTES_COMPARE_FUNC)?,
                ));
                out.instruction(&Instruction::I32Const(0));
                out.instruction(&Instruction::I32GtS);
            }
            Type::Extern | Type::ExternSubtype(_) | Type::Named { .. } | Type::Opaque { .. } => {
                unreachable!()
            }
            Type::Array(_) => unreachable!(),
            Type::Multi(_) => {
                return Err(Diagnostic::new(
                    "multi-value comparison is not supported during wasm emission",
                ));
            }
            Type::TaggedVariant(_) | Type::TaggedUnion(_) => {
                return Err(Diagnostic::new(
                    "tagged unions are not yet supported during wasm emission",
                ));
            }
            Type::Function { .. }
            | Type::Record(_)
            | Type::TypeParam(_)
            | Type::Thread
            | Type::Unknown => {
                unreachable!()
            }
            Type::Nil | Type::Nullable(_) | Type::Unit => unreachable!(),
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

/// Emit `left ^ right` for any numeric `operand_ty`. Lua's `^` always computes
/// in floating point, so both operands are widened to `f64`, handed to the host
/// `math_pow` import, and the result is converted back to `operand_ty` (matching
/// how the language keeps the operand type for other arithmetic such as `/`).
fn emit_pow(
    out: &mut Function,
    ctx: &EmissionContext<'_>,
    operand_ty: Type,
    left_local: u32,
    right_local: u32,
) -> Result<(), Diagnostic> {
    let numeric = match operand_ty {
        Type::Numeric(numeric) => numeric,
        _ => {
            return Err(Diagnostic::new(
                "exponentiation requires numeric operands during wasm emission",
            ));
        }
    };
    emit_widen_to_f64(out, left_local, numeric);
    emit_widen_to_f64(out, right_local, numeric);
    out.instruction(&Instruction::Call(
        ctx.host_func_index(host::IMPORT_MATH_POW_FUNC)?,
    ));
    emit_narrow_from_f64(out, numeric);
    Ok(())
}

/// Load `local` (typed `ty`) and convert it to `f64`.
fn emit_widen_to_f64(out: &mut Function, local: u32, ty: NumericType) {
    out.instruction(&Instruction::LocalGet(local));
    match ty {
        NumericType::U32 => out.instruction(&Instruction::F64ConvertI32U),
        NumericType::I32 => out.instruction(&Instruction::F64ConvertI32S),
        NumericType::U64 => out.instruction(&Instruction::F64ConvertI64U),
        NumericType::I64 => out.instruction(&Instruction::F64ConvertI64S),
        NumericType::F32 => out.instruction(&Instruction::F64PromoteF32),
        NumericType::F64 => out,
    };
}

/// Convert the `f64` on top of the stack back to `ty`.
fn emit_narrow_from_f64(out: &mut Function, ty: NumericType) {
    match ty {
        NumericType::U32 => out.instruction(&Instruction::I32TruncF64U),
        NumericType::I32 => out.instruction(&Instruction::I32TruncF64S),
        NumericType::U64 => out.instruction(&Instruction::I64TruncF64U),
        NumericType::I64 => out.instruction(&Instruction::I64TruncF64S),
        NumericType::F32 => out.instruction(&Instruction::F32DemoteF64),
        NumericType::F64 => out,
    };
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

fn emit_bitwise_intrinsic(
    out: &mut Function,
    intrinsic: BitwiseIntrinsic,
    arity: usize,
) -> Result<(), Diagnostic> {
    match intrinsic {
        BitwiseIntrinsic::Not => {
            debug_assert_eq!(arity, 1);
            out.instruction(&Instruction::I32Const(-1));
            out.instruction(&Instruction::I32Xor);
        }
        BitwiseIntrinsic::And => {
            if arity == 0 {
                out.instruction(&Instruction::I32Const(-1));
            } else {
                for _ in 1..arity {
                    out.instruction(&Instruction::I32And);
                }
            }
        }
        BitwiseIntrinsic::Or => {
            if arity == 0 {
                out.instruction(&Instruction::I32Const(0));
            } else {
                for _ in 1..arity {
                    out.instruction(&Instruction::I32Or);
                }
            }
        }
        BitwiseIntrinsic::Xor => {
            if arity == 0 {
                out.instruction(&Instruction::I32Const(0));
            } else {
                for _ in 1..arity {
                    out.instruction(&Instruction::I32Xor);
                }
            }
        }
        BitwiseIntrinsic::Test => {
            if arity == 0 {
                out.instruction(&Instruction::I32Const(1));
            } else {
                for _ in 1..arity {
                    out.instruction(&Instruction::I32And);
                }
                out.instruction(&Instruction::I32Eqz);
                out.instruction(&Instruction::I32Eqz);
            }
        }
        BitwiseIntrinsic::LRotate => {
            debug_assert_eq!(arity, 2);
            out.instruction(&Instruction::I32Rotl);
        }
        BitwiseIntrinsic::RRotate => {
            debug_assert_eq!(arity, 2);
            out.instruction(&Instruction::I32Rotr);
        }
        BitwiseIntrinsic::CountLeadingZeros => {
            debug_assert_eq!(arity, 1);
            out.instruction(&Instruction::I32Clz);
        }
        BitwiseIntrinsic::CountTrailingZeros => {
            debug_assert_eq!(arity, 1);
            out.instruction(&Instruction::I32Ctz);
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

#[cfg(test)]
mod tests;
