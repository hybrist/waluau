mod link;

use serde::Serialize;
use wasm_bindgen::prelude::*;

#[derive(Debug, Serialize)]
struct TaggedVariantJson {
    tag: String,
    payload: Box<TypeJson>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "value")]
enum TypeJson {
    I32,
    I64,
    F32,
    F64,
    Bool,
    String,
    Bytes,
    Unit,
    Thread,
    Nullable {
        #[serde(rename = "innerType")]
        inner_type: Box<TypeJson>,
    },
    Array {
        #[serde(rename = "elementType")]
        element_type: Box<TypeJson>,
    },
    Record {
        #[serde(rename = "typeIndex")]
        type_index: u32,
        fields: std::collections::BTreeMap<String, TypeJson>,
    },
    TaggedUnion {
        #[serde(rename = "typeIndex")]
        type_index: u32,
        variants: Vec<TaggedVariantJson>,
    },
    Unknown(String),
}

#[derive(Debug, Serialize)]
struct FunctionSignatureJson {
    params: Vec<TypeJson>,
    returns: Vec<TypeJson>,
}

#[derive(Debug, Serialize)]
struct RequiredImportJson {
    module: String,
    name: String,
    kind: String,
}

#[derive(Debug, Serialize)]
struct CompileResult {
    ir: String,
    wat: String,
    wasm: Vec<u8>,
    #[serde(rename = "jsGlue")]
    js_glue: String,
    #[serde(rename = "requiredImports")]
    required_imports: Vec<RequiredImportJson>,
    #[serde(rename = "bytesConstants")]
    bytes_constants: Vec<Vec<u8>>,
    #[serde(rename = "requiresWasmGc")]
    requires_wasm_gc: bool,
    signatures: std::collections::HashMap<String, FunctionSignatureJson>,
    #[serde(rename = "tagIds")]
    tag_ids: std::collections::BTreeMap<String, i32>,
}

fn required_imports_json(
    imports: &[waluau_codegen_wasm::RequiredImport],
) -> Vec<RequiredImportJson> {
    imports
        .iter()
        .map(|import| RequiredImportJson {
            module: import.module.clone(),
            name: import.name.clone(),
            kind: import.kind.as_str().to_string(),
        })
        .collect()
}

fn to_type_json(
    ty: &waluau_ast::Type,
    record_type_indices: &std::collections::HashMap<String, u32>,
) -> TypeJson {
    match ty {
        waluau_ast::Type::Numeric(numeric_ty) => match numeric_ty {
            waluau_ast::NumericType::I32 | waluau_ast::NumericType::U32 => TypeJson::I32,
            waluau_ast::NumericType::I64 | waluau_ast::NumericType::U64 => TypeJson::I64,
            waluau_ast::NumericType::F32 => TypeJson::F32,
            waluau_ast::NumericType::F64 => TypeJson::F64,
        },
        waluau_ast::Type::Bool => TypeJson::Bool,
        waluau_ast::Type::String => TypeJson::String,
        waluau_ast::Type::Bytes => TypeJson::Bytes,
        waluau_ast::Type::Unit => TypeJson::Unit,
        waluau_ast::Type::Thread => TypeJson::Thread,
        waluau_ast::Type::Nullable(inner_ty) => TypeJson::Nullable {
            inner_type: Box::new(to_type_json(inner_ty, record_type_indices)),
        },
        waluau_ast::Type::Opaque { ty, .. } => to_type_json(ty, record_type_indices),
        waluau_ast::Type::Array(elem_ty) => TypeJson::Array {
            element_type: Box::new(to_type_json(elem_ty, record_type_indices)),
        },
        waluau_ast::Type::Record(fields) => {
            let type_index = record_type_indices
                .get(&ty.to_string())
                .copied()
                .unwrap_or(0);
            let fields_json = fields
                .iter()
                .map(|(name, field_ty)| (name.clone(), to_type_json(field_ty, record_type_indices)))
                .collect();
            TypeJson::Record {
                type_index,
                fields: fields_json,
            }
        }
        waluau_ast::Type::TaggedVariant(variant) => {
            let canonical = waluau_ast::Type::canonical_tagged_union_record();
            let type_index = record_type_indices
                .get(&canonical.to_string())
                .copied()
                .unwrap_or(0);
            TypeJson::TaggedUnion {
                type_index,
                variants: vec![TaggedVariantJson {
                    tag: variant.tag.clone(),
                    payload: Box::new(to_type_json(&variant.payload, record_type_indices)),
                }],
            }
        }
        waluau_ast::Type::TaggedUnion(variants) => {
            let canonical = waluau_ast::Type::canonical_tagged_union_record();
            let type_index = record_type_indices
                .get(&canonical.to_string())
                .copied()
                .unwrap_or(0);
            let variants_json = variants
                .iter()
                .map(|variant| TaggedVariantJson {
                    tag: variant.tag.clone(),
                    payload: Box::new(to_type_json(&variant.payload, record_type_indices)),
                })
                .collect();
            TypeJson::TaggedUnion {
                type_index,
                variants: variants_json,
            }
        }
        other => TypeJson::Unknown(format!("{:?}", other)),
    }
}

fn get_returns_json(
    ty: &waluau_ast::Type,
    record_type_indices: &std::collections::HashMap<String, u32>,
) -> Vec<TypeJson> {
    match ty {
        waluau_ast::Type::Multi(tys) => tys
            .iter()
            .map(|t| to_type_json(t, record_type_indices))
            .collect(),
        waluau_ast::Type::Unit => Vec::new(),
        other => vec![to_type_json(other, record_type_indices)],
    }
}

/// Compile Waluau source to IR, WAT, and Wasm bytes.
///
/// On success, returns a JavaScript object `{ ir, wat, wasm }` where `wasm` is a
/// `Uint8Array`. On failure, throws a string diagnostic message.
#[wasm_bindgen]
pub fn compile(source: &str) -> Result<JsValue, JsValue> {
    match compile_source(source) {
        Ok(result) => {
            serde_wasm_bindgen::to_value(&result).map_err(|err| JsValue::from_str(&err.to_string()))
        }
        Err(err) => Err(JsValue::from_str(&err)),
    }
}

#[wasm_bindgen]
pub fn compile_multi(files: JsValue, entry_path: &str) -> Result<JsValue, JsValue> {
    let files: std::collections::HashMap<String, String> = serde_wasm_bindgen::from_value(files)
        .map_err(|err| JsValue::from_str(&format!("failed to parse files map: {}", err)))?;

    match compile_sources(&files, entry_path) {
        Ok(result) => {
            serde_wasm_bindgen::to_value(&result).map_err(|err| JsValue::from_str(&err.to_string()))
        }
        Err(err) => Err(JsValue::from_str(&err)),
    }
}

fn compile_sources(
    files: &std::collections::HashMap<String, String>,
    entry_path: &str,
) -> Result<CompileResult, String> {
    let program = link::link_programs(files, entry_path)?;
    let typed_program =
        waluau_hir::type_check_and_infer(&program).map_err(|e| e.render_for_playground())?;
    let module = waluau_ir::build(&typed_program).map_err(|e| e.render_for_playground())?;
    let requires_wasm_gc = module_requires_wasm_gc(&module);

    let mut ir_dump = String::new();
    for function in &module.functions {
        ir_dump.push_str(&function.dump());
        ir_dump.push('\n');
    }

    let emit_res = waluau_codegen_wasm::emit(&module).map_err(|e| e.to_string())?;
    let wat = wasmprinter::print_bytes(&emit_res.wasm).map_err(|e| e.to_string())?;
    let js_glue = waluau_codegen_wasm::generate_js_glue("program.wasm", &emit_res);
    let required_imports = required_imports_json(&emit_res.required_imports);

    let mut signatures = std::collections::HashMap::new();
    for function in &module.functions {
        if !function.name.starts_with("__waluau_") {
            let params = function
                .params
                .iter()
                .map(|(_, ty)| to_type_json(ty, &emit_res.record_type_indices))
                .collect();
            let returns = get_returns_json(&function.return_type, &emit_res.record_type_indices);
            signatures.insert(
                function.name.clone(),
                FunctionSignatureJson { params, returns },
            );
        }
    }

    Ok(CompileResult {
        ir: ir_dump,
        wat,
        wasm: emit_res.wasm,
        js_glue,
        required_imports,
        bytes_constants: emit_res.bytes_constants,
        requires_wasm_gc,
        signatures,
        tag_ids: module.tag_ids.clone(),
    })
}

fn compile_source(source: &str) -> Result<CompileResult, String> {
    let mut program = waluau_parser::parse(source).map_err(|e| e.to_string())?;

    // Add builtin declarations to standalone programs
    add_builtins_to_program(&mut program).map_err(|e| e.to_string())?;

    let typed_program =
        waluau_hir::type_check_and_infer(&program).map_err(|e| e.render_for_playground())?;
    let module = waluau_ir::build(&typed_program).map_err(|e| e.render_for_playground())?;
    let requires_wasm_gc = module_requires_wasm_gc(&module);

    let mut ir_dump = String::new();
    for function in &module.functions {
        ir_dump.push_str(&function.dump());
        ir_dump.push('\n');
    }

    let emit_res = waluau_codegen_wasm::emit(&module).map_err(|e| e.to_string())?;
    let wat = wasmprinter::print_bytes(&emit_res.wasm).map_err(|e| e.to_string())?;
    let js_glue = waluau_codegen_wasm::generate_js_glue("program.wasm", &emit_res);
    let required_imports = required_imports_json(&emit_res.required_imports);

    let mut signatures = std::collections::HashMap::new();
    for function in &module.functions {
        if !function.name.starts_with("__waluau_") {
            let params = function
                .params
                .iter()
                .map(|(_, ty)| to_type_json(ty, &emit_res.record_type_indices))
                .collect();
            let returns = get_returns_json(&function.return_type, &emit_res.record_type_indices);
            signatures.insert(
                function.name.clone(),
                FunctionSignatureJson { params, returns },
            );
        }
    }

    Ok(CompileResult {
        ir: ir_dump,
        wat,
        wasm: emit_res.wasm,
        js_glue,
        required_imports,
        bytes_constants: emit_res.bytes_constants,
        requires_wasm_gc,
        signatures,
        tag_ids: module.tag_ids.clone(),
    })
}

fn module_requires_wasm_gc(module: &waluau_ir::Module) -> bool {
    module.functions.iter().any(function_requires_wasm_gc)
}

fn function_requires_wasm_gc(function: &waluau_ir::Function) -> bool {
    type_requires_wasm_gc(&function.return_type)
        || function
            .params
            .iter()
            .any(|(_, ty)| type_requires_wasm_gc(ty))
        || function.blocks.values().any(|block| {
            block.instructions.iter().any(|(_, instruction)| {
                matches!(
                    instruction,
                    waluau_ir::Instruction::ArrayNew { .. }
                        | waluau_ir::Instruction::ArrayGet { .. }
                        | waluau_ir::Instruction::ArraySet { .. }
                        | waluau_ir::Instruction::ArrayLen { .. }
                        | waluau_ir::Instruction::ArrayPop { .. }
                        | waluau_ir::Instruction::ArraySlice { .. }
                        | waluau_ir::Instruction::DynLen { .. }
                        | waluau_ir::Instruction::DynIndex { .. }
                        | waluau_ir::Instruction::Closure { .. }
                )
            })
        })
}

fn type_requires_wasm_gc(ty: &waluau_ast::Type) -> bool {
    matches!(
        ty,
        waluau_ast::Type::Array(_)
            | waluau_ast::Type::Function { .. }
            | waluau_ast::Type::Record(_)
    )
}

fn add_builtins_to_program(
    program: &mut waluau_ast::Program,
) -> Result<(), waluau_diagnostics::Diagnostic> {
    // Load builtin declaration files and merge their declared imports and
    // constants.
    let builtin_files = ["core.walu", "math.walu"];

    for filename in &builtin_files {
        let builtin_source = match *filename {
            "core.walu" => include_str!("../../../builtins/core.walu"),
            "math.walu" => include_str!("../../../builtins/math.walu"),
            _ => continue,
        };

        let builtin_program =
            waluau_parser::parse_with_path(builtin_source, &format!("builtin:{filename}"))?;
        program
            .declared_imports
            .extend(builtin_program.declared_imports);
        program
            .declared_constants
            .extend(builtin_program.declared_constants);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{compile_source, compile_sources};

    #[test]
    fn scalar_program_does_not_require_wasm_gc() {
        let source = r#"
            function add_one(x: i32): i32
                return x + 1
            end
        "#;
        let result = compile_source(source).expect("compile should succeed");
        assert!(!result.requires_wasm_gc);
        assert!(result.required_imports.is_empty());
        assert!(result.js_glue.contains("export async function instantiate"));
        assert!(!result.js_glue.contains("WebAssembly.Module.imports"));
    }

    #[test]
    fn array_program_requires_wasm_gc() {
        let source = include_str!("../../../fixtures/array_ops.walu");
        let result = compile_source(source).expect("compile should succeed");
        assert!(result.requires_wasm_gc);
    }

    #[test]
    fn compile_multi_embeds_stable_engine_package() {
        let files = HashMap::from([(
            "/main.walu".to_string(),
            include_str!("../../../examples/game-project/main.walu").to_string(),
        )]);

        let result = compile_sources(&files, "/main.walu")
            .expect("browser compiler should resolve the embedded engine package");
        assert!(result.wat.contains("dom_window"));
        assert!(result.wat.contains("requestAnimationFrame"));
        assert!(result.required_imports.iter().any(|import| {
            import.module == "waluau" && import.name == "Window.requestAnimationFrame"
        }));
        assert!(result.js_glue.contains("assetManifest"));
        assert!(result.js_glue.contains("createImports(context)"));
    }

    #[test]
    fn compile_success_ir_contains_function_name() {
        let source = "function greet(x: i32): i32\n    return x\nend";
        let result = compile_source(source).expect("compile should succeed");
        assert!(result.ir.contains("greet"));
    }

    #[test]
    fn compile_success_wat_is_wasm_module() {
        let source = "function greet(x: i32): i32\n    return x\nend";
        let result = compile_source(source).expect("compile should succeed");
        assert!(result.wat.contains("(module"));
    }

    #[test]
    fn compile_success_wasm_has_magic_bytes() {
        let source = "function greet(x: i32): i32\n    return x\nend";
        let result = compile_source(source).expect("compile should succeed");
        assert!(
            result.wasm.starts_with(b"\0asm"),
            "wasm output should begin with the WebAssembly magic bytes"
        );
    }

    #[test]
    fn compile_parse_error_propagated() {
        let err = compile_source("function").expect_err("truncated input should fail to parse");
        assert!(!err.is_empty());
    }

    #[test]
    fn compile_type_check_error_propagated() {
        let source = include_str!("../../../fixtures/mismatch.walu");
        let err = compile_source(source).expect_err("type mismatch should fail type check");
        assert!(!err.is_empty());
    }

    #[test]
    fn compile_closure_capture_succeeds() {
        let source = include_str!("../../../fixtures/closure_capture.walu");
        let result = compile_source(source).expect("closure variable capture should compile");
        assert!(result.wat.contains("(module"));
        assert!(
            result.wasm.starts_with(b"\0asm"),
            "wasm output should begin with the WebAssembly magic bytes"
        );
    }

    #[test]
    fn compile_incremental_record_init_from_empty_table() {
        let source = "function f(): i32\n    local t = {}\n    t.x = 1::i32\n    return t.x\nend\n";
        let result = compile_source(source).expect("incremental record init should compile");
        assert!(result.wat.contains("(module"));
    }

    #[test]
    fn compile_generic_type_declarations_conformance() {
        let source = include_str!("../../../conformance/generic_type_declarations.walu");
        let result =
            compile_source(source).expect("generic type declarations conformance should compile");
        assert!(result.wat.contains("(module"));
    }

    #[test]
    fn compile_generic_extern_type_constructors_conformance() {
        let source = include_str!("../../../conformance/generic_extern_type_constructors.walu");
        let result = compile_source(source)
            .expect("generic extern type constructors conformance should compile");
        assert!(result.wat.contains("(module"));
    }

    #[test]
    fn compile_promise_extern_api_signatures_conformance() {
        let source = include_str!("../../../conformance/promise_extern_api_signatures.walu");
        let result =
            compile_source(source).expect("Promise<T> extern API conformance should compile");
        assert!(result.wat.contains("(module"));
    }

    #[test]
    fn compile_promise_await_conformance() {
        let source = include_str!("../../../conformance/promise_await.walu");
        let result = compile_source(source).expect("Promise await conformance should compile");
        assert!(result.wat.contains("(module"));
    }

    #[test]
    fn compile_loop_bound_literals_conformance() {
        // Regression for `for i = 0, #a - 1`: the untyped literal bound must
        // adopt the i32 type of the array-length bound instead of forcing an
        // f64 loop over i32 values (which used to emit invalid wasm).
        let source = include_str!("../../../conformance/loop_bound_literals.walu");
        let result =
            compile_source(source).expect("loop bound literals conformance should compile");
        assert!(result.wat.contains("(module"));
    }

    #[test]
    fn compile_multi_coroutine_await_promise_fixture() {
        let source = r#"
            declare function make_string_promise(): extern
            declare function make_object_promise(): extern
            declare function make_rejected_promise(): extern
            declare function record_string(value: string): unit
            declare function record_object(value: extern): unit
            declare function record_nested(value: string): unit
            declare function record_status(value: string): unit

            function run_string(): unit
                local co: thread = coroutine.create(function(): i32
                    local value: string = coroutine.await_promise(make_string_promise())::string
                    record_string(value)
                    return 0
                end)
                coroutine.resume(co)
            end

            function run_object(): unit
                local co: thread = coroutine.create(function(): i32
                    local value: extern = coroutine.await_promise(make_object_promise())::extern
                    record_object(value)
                    return 0
                end)
                coroutine.resume(co)
            end

            function run_nested(): unit
                local inner: thread = coroutine.create(function(): i32
                    coroutine.yield("inner-yield")
                    return 17
                end)
                local outer: thread = coroutine.create(function(): i32
                    local value: string = coroutine.await_promise(make_string_promise())::string
                    local ok1: bool, payload1: unknown = coroutine.resume(inner)
                    assert(ok1)
                    local ok2: bool, payload2: unknown = coroutine.resume(inner)
                    assert(ok2)
                    record_nested(value .. ":" .. payload1::string .. ":" .. tostring(payload2::i32))
                    return 0
                end)
                coroutine.resume(outer)
            end

            function run_rejected(): unit
                local co: thread = coroutine.create(function(): i32
                    coroutine.await_promise(make_rejected_promise())
                    record_status("unexpected")
                    return 0
                end)
                coroutine.resume(co)
            end
        "#;
        let files = HashMap::from([("/main.walu".to_string(), source.to_string())]);
        let result =
            compile_sources(&files, "/main.walu").expect("compile_multi fixture should compile");
        assert!(result.wat.contains("(module"));
    }

    #[test]
    fn compile_math_constants_fold_to_literals() {
        let result = compile_source("assert(math.pi > 3.14 and math.pi < 3.15)")
            .expect("math.pi read should compile");
        // The constant folds to an f64 literal; no math.pi host import exists.
        assert!(result.wat.contains("3.141592653589793"));
        assert!(!result.wat.contains("(import \"waluau\" \"math.pi\""));
    }

    #[test]
    fn compile_records_conformance_with_alias_mutation() {
        let source = include_str!("../../../conformance/records_sealed_tables.walu");
        let result =
            compile_source(source).expect("records conformance with alias mutation should compile");
        assert!(result.wat.contains("(module"));
    }

    #[test]
    fn compile_method_calls_conformance_with_mutation() {
        let source = include_str!("../../../conformance/method_calls.walu");
        let result =
            compile_source(source).expect("method call conformance with mutation should compile");
        assert!(result.wat.contains("(module"));
    }

    #[test]
    fn compile_multi_resolves_imports() {
        let mut files = std::collections::HashMap::new();
        files.insert(
            "main.walu".to_string(),
            "function compute(n: i32): i32\n    local double: (i32) -> i32 = require(\"./double\")\n    return double(n)\nend\n".to_string(),
        );
        files.insert(
            "double.walu".to_string(),
            "function double(x: i32): i32\n    return x * 2\nend\nreturn double\n".to_string(),
        );
        let result = super::compile_sources(&files, "main.walu").expect("compile should succeed");
        assert!(result.wat.contains("(module"));
        assert!(result.ir.contains("compute"));
    }

    #[test]
    fn compile_multi_rewrites_module_aliases_in_declared_import_params_and_returns() {
        let files = HashMap::from([
            (
                "/main.walu".to_string(),
                r#"
                    local service = require("./service")

                    function exchange(value: service.Promise<i32>): service.Promise<string>
                        return service.exchange(value)
                    end
                "#
                .to_string(),
            ),
            (
                "/service.walu".to_string(),
                r#"
                    type Promise<T> = extern

                    declare function host_exchange(value: Promise<i32>): Promise<string>

                    function exchange(value: Promise<i32>): Promise<string>
                        return host_exchange(value)
                    end

                    return { exchange = exchange }
                "#
                .to_string(),
            ),
        ]);

        let result = compile_sources(&files, "/main.walu")
            .expect("module-local aliases in host import signatures should compile");
        assert!(
            result
                .required_imports
                .iter()
                .any(|import| import.name == "host_exchange")
        );
    }

    #[test]
    fn compile_multi_supports_negated_module_constants_in_functions() {
        let files = std::collections::HashMap::from([(
            "main.walu".to_string(),
            r#"
                local NEGATIVE_INDEX <const>: i32 = -1
                local NEGATIVE_SCALE <const>: f64 = -1.5

                function negative_index(): i32
                    return NEGATIVE_INDEX
                end

                function negative_scale(): f64
                    return NEGATIVE_SCALE
                end

                assert(negative_index() == -1)
                assert(negative_scale() == -1.5)
            "#
            .to_string(),
        )]);

        let result = super::compile_sources(&files, "main.walu")
            .expect("negated module constants should compile");
        assert!(result.wat.contains("(module"));
    }

    #[test]
    fn compile_multi_rejects_non_literal_module_constant_at_declaration() {
        let files = std::collections::HashMap::from([(
            "main.walu".to_string(),
            r#"
                function initial(): i32
                    return 1
                end

                local SIZE <const>: i32 = initial()

                function read_size(): i32
                    return SIZE
                end
            "#
            .to_string(),
        )]);

        let error = super::compile_sources(&files, "main.walu")
            .expect_err("non-literal module constant should fail");
        assert_eq!(
            error,
            "top-level const 'SIZE' initializer must be an inlinable literal"
        );
    }

    #[test]
    fn compile_reexported_bindings() {
        let mut files = std::collections::HashMap::new();
        files.insert(
            "main.walu".to_string(),
            "function main(): f64\n    local bundle = require(\"./reexport\")\n    return bundle.add(bundle.double(2) :: f64, 1.0)\nend\n".to_string(),
        );
        files.insert(
            "reexport.walu".to_string(),
            "local double: (i32) -> i32 = require(\"./double\")\nlocal ns = require(\"./ops\")\n\nreturn { double = double, add = ns.add }\n".to_string(),
        );
        files.insert(
            "double.walu".to_string(),
            "function double(x: i32): i32\n    return x * 2\nend\nreturn double\n".to_string(),
        );
        files.insert(
            "ops.walu".to_string(),
            "return {\n    add = function (a: f64, b: f64): f64\n        return a + b\n    end,\n}\n".to_string(),
        );
        let result =
            super::compile_sources(&files, "main.walu").expect("re-export compile should succeed");
        assert!(result.wat.contains("(module"));
        assert!(result.ir.contains("main"));
    }

    #[test]
    fn compile_multi_only_exports_entry_surface() {
        let mut files = std::collections::HashMap::new();
        files.insert(
            "main.walu".to_string(),
            "function main(): i32\n    local double: (i32) -> i32 = require(\"./double\")\n    return double(2)\nend\n".to_string(),
        );
        files.insert(
            "double.walu".to_string(),
            "function double(x: i32): i32\n    return x * 2\nend\nreturn double\n".to_string(),
        );

        let result = super::compile_sources(&files, "main.walu").expect("compile should succeed");
        assert!(result.wat.contains("(export \"main\""));
        assert!(!result.wat.contains("__waluau_m"));
    }

    #[test]
    fn compile_multi_preserves_filenames_in_assertions() {
        let mut files = std::collections::HashMap::new();
        files.insert(
            "main.walu".to_string(),
            "function test(): i32\n    assert(false)\n    return 0\nend\n".to_string(),
        );
        let result = super::compile_sources(&files, "main.walu").expect("compile should succeed");
        assert!(result.ir.contains("at /main.walu:2"));
    }

    #[test]
    fn compile_tagged_unions_exports_tag_ids() {
        let source = r#"
            type Option = Some(i32) | None(unit)

            function process_option(opt: Option): i32
                if opt is Some then
                    return 1
                else
                    return 0
                end
            end
        "#;
        let result = compile_source(source).expect("compile should succeed");
        println!("tag_ids: {:?}", result.tag_ids);
        assert!(result.tag_ids.contains_key("Some"));
        assert!(result.tag_ids.contains_key("None"));
    }

    #[test]
    fn compile_multi_resolves_dom_window_virtual_module() {
        let mut files = std::collections::HashMap::new();
        files.insert(
            "main.walu".to_string(),
            r#"
                function main(): unit
                    local window = require("dom:window")
                    local document: Document = window.document
                    local root: Element = document.document_element
                    local child: Element = document:create_element("div")
                end
            "#
            .to_string(),
        );

        let result =
            super::compile_sources(&files, "main.walu").expect("dom window require should compile");
        assert!(result.wat.contains("(module"));
        assert!(result.wat.contains("dom_window"));
        assert!(result.wat.contains("Window.get/document"));
        assert!(result.wat.contains("Document.get/documentElement"));
        assert!(result.wat.contains("Document.createElement"));
    }

    #[test]
    fn compile_multi_resolves_top_level_dom_window_virtual_module() {
        let mut files = std::collections::HashMap::new();
        files.insert(
            "main.walu".to_string(),
            r#"
                local window = require("dom:window")
                local document = window.document

                function main(): unit
                end
            "#
            .to_string(),
        );

        let result =
            super::compile_sources(&files, "main.walu").expect("dom window require should compile");
        assert!(result.wat.contains("(module"));
        assert!(result.wat.contains("dom_window"));
        assert!(result.wat.contains("Window.get/document"));
    }

    #[test]
    fn compile_multi_resolves_tfjs_training_namespace() {
        let mut files = std::collections::HashMap::new();
        files.insert(
            "main.walu".to_string(),
            r#"
                function train(model: LayersModel, input: Tensor, target: Tensor): Promise<TrainingHistory>
                    local tf = require("tfjs")
                    tf.layers_model_compile_sgd(model, "meanSquaredError", 0.01)
                    return tf.layers_model_fit_one(model, input, target, 2, 1)
                end

                function first_loss(history: TrainingHistory): f64
                    local tf = require("tfjs")
                    return tf.training_history_loss(history, 0)
                end
            "#
            .to_string(),
        );

        let result = super::compile_sources(&files, "main.walu")
            .expect("tfjs training require should compile");
        assert!(result.wat.contains("(module"));
        assert!(result.wat.contains("tfjs_layers_model_compile_sgd"));
        assert!(result.wat.contains("tfjs_layers_model_fit_one"));
        assert!(result.wat.contains("tfjs_training_history_loss"));
    }

    #[test]
    fn compile_multi_resolves_tfjs_namespace_inside_coroutine_callback() {
        let mut files = std::collections::HashMap::new();
        files.insert(
            "main.walu".to_string(),
            r#"
                function run(): unit
                    local co: thread = coroutine.create(function(): i32
                        local tf = require("tfjs")
                        local values: TensorData = tf.data_empty(4)
                        tf.data_set_f64(values, 0, 1.0)
                        local tensor: Tensor = tf.tensor2d(values, 2, 2)
                        tf.dispose(tensor)
                        return 0
                    end)
                    coroutine.resume(co)
                end
            "#
            .to_string(),
        );

        let result = super::compile_sources(&files, "main.walu")
            .expect("nested tfjs require should compile");
        assert!(result.wat.contains("(module"));
        assert!(result.wat.contains("tfjs_data_empty"));
        assert!(result.wat.contains("tfjs_tensor2d"));
        assert!(result.wat.contains("tfjs_dispose"));
    }

    #[test]
    fn compile_multi_rejects_unknown_dom_virtual_module() {
        let mut files = std::collections::HashMap::new();
        files.insert(
            "main.walu".to_string(),
            r#"
                function main(): unit
                    local worker = require("dom:worker")
                end
            "#
            .to_string(),
        );

        let error = super::compile_sources(&files, "main.walu")
            .expect_err("unknown DOM virtual module should fail");
        assert_eq!(
            error,
            "unsupported DOM virtual module \"dom:worker\"; supported specifiers: \"dom:window\""
        );
    }

    #[test]
    fn compile_error_renders_inference_span_for_playground_markers() {
        let source = r#"
            declare function accept(value: i32): unit

            function bad(): unit
                accept("wrong")
            end
        "#;
        let start = source
            .find("\"wrong\"")
            .expect("argument should be present");

        let error = super::compile_source(source).expect_err("compile should fail");

        assert_eq!(
            error,
            format!(
                "cannot implicitly convert string to i32 at {start}..{}",
                start + "\"wrong\"".len()
            )
        );
    }

    #[test]
    fn compile_multi_error_identifies_module_for_playground_markers() {
        let mut files = std::collections::HashMap::new();
        let dependency = r#"
            declare function accept(value: i32): unit

            function bad(): unit
                accept("wrong")
            end

            return bad
        "#;
        files.insert(
            "main.walu".to_string(),
            "local bad = require(\"./dependency\")\n".to_string(),
        );
        files.insert("dependency.walu".to_string(), dependency.to_string());
        let start = dependency
            .find("\"wrong\"")
            .expect("argument should be present");

        let error = super::compile_sources(&files, "main.walu").expect_err("compile should fail");

        assert_eq!(
            error,
            format!(
                "in module \"/dependency.walu\": cannot implicitly convert string to i32 at \
                 {start}..{}",
                start + "\"wrong\"".len()
            )
        );
    }

    #[test]
    fn nullable_types_have_structured_signature_metadata() {
        let json = super::to_type_json(
            &waluau_ast::Type::Nullable(Box::new(waluau_ast::Type::String)),
            &std::collections::HashMap::new(),
        );
        let super::TypeJson::Nullable { inner_type } = json else {
            panic!("nullable type should not fall back to unknown metadata");
        };
        assert!(matches!(*inner_type, super::TypeJson::String));
    }
}
