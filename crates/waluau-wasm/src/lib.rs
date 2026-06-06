mod link;

use serde::Serialize;
use wasm_bindgen::prelude::*;

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
    Array {
        #[serde(rename = "elementType")]
        element_type: Box<TypeJson>,
    },
    Record {
        #[serde(rename = "typeIndex")]
        type_index: u32,
        fields: std::collections::BTreeMap<String, TypeJson>,
    },
    Unknown(String),
}

#[derive(Debug, Serialize)]
struct FunctionSignatureJson {
    params: Vec<TypeJson>,
    returns: Vec<TypeJson>,
}

#[derive(Debug, Serialize)]
struct CompileResult {
    ir: String,
    wat: String,
    wasm: Vec<u8>,
    #[serde(rename = "requiresWasmGc")]
    requires_wasm_gc: bool,
    signatures: std::collections::HashMap<String, FunctionSignatureJson>,
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
    let typed_program = waluau_hir::type_check_and_infer(&program).map_err(|e| e.to_string())?;
    let module = waluau_ir::build(&typed_program).map_err(|e| e.to_string())?;
    let requires_wasm_gc = module_requires_wasm_gc(&module);

    let mut ir_dump = String::new();
    for function in &module.functions {
        ir_dump.push_str(&function.dump());
        ir_dump.push('\n');
    }

    let emit_res = waluau_codegen_wasm::emit(&module).map_err(|e| e.to_string())?;
    let wat = wasmprinter::print_bytes(&emit_res.wasm).map_err(|e| e.to_string())?;

    let mut signatures = std::collections::HashMap::new();
    for function in &module.functions {
        if function.name != "__waluau_top_level_init" {
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
        requires_wasm_gc,
        signatures,
    })
}

fn compile_source(source: &str) -> Result<CompileResult, String> {
    let program = waluau_parser::parse(source).map_err(|e| e.to_string())?;
    let typed_program = waluau_hir::type_check_and_infer(&program).map_err(|e| e.to_string())?;
    let module = waluau_ir::build(&typed_program).map_err(|e| e.to_string())?;
    let requires_wasm_gc = module_requires_wasm_gc(&module);

    let mut ir_dump = String::new();
    for function in &module.functions {
        ir_dump.push_str(&function.dump());
        ir_dump.push('\n');
    }

    let emit_res = waluau_codegen_wasm::emit(&module).map_err(|e| e.to_string())?;
    let wat = wasmprinter::print_bytes(&emit_res.wasm).map_err(|e| e.to_string())?;

    let mut signatures = std::collections::HashMap::new();
    for function in &module.functions {
        if function.name != "__waluau_top_level_init" {
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
        requires_wasm_gc,
        signatures,
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

#[cfg(test)]
mod tests {
    use super::compile_source;

    #[test]
    fn scalar_program_does_not_require_wasm_gc() {
        let source = r#"
            function add_one(x: i32): i32
                return x + 1
            end
        "#;
        let result = compile_source(source).expect("compile should succeed");
        assert!(!result.requires_wasm_gc);
    }

    #[test]
    fn array_program_requires_wasm_gc() {
        let source = include_str!("../../../fixtures/array_ops.walu");
        let result = compile_source(source).expect("compile should succeed");
        assert!(result.requires_wasm_gc);
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
}
