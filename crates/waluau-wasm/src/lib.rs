use serde::Serialize;
use wasm_bindgen::prelude::*;

#[derive(Serialize)]
struct CompileResult {
    ir: String,
    wat: String,
    wasm: Vec<u8>,
    #[serde(rename = "requiresWasmGc")]
    requires_wasm_gc: bool,
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

fn compile_source(source: &str) -> Result<CompileResult, String> {
    let program = waluau_parser::parse(source).map_err(|e| e.to_string())?;
    waluau_hir::type_check(&program).map_err(|e| e.to_string())?;
    let module = waluau_ir::build(&program).map_err(|e| e.to_string())?;
    let requires_wasm_gc = module_requires_wasm_gc(&module);

    let mut ir_dump = String::new();
    for function in &module.functions {
        ir_dump.push_str(&function.dump());
        ir_dump.push('\n');
    }

    let wasm_bytes = waluau_codegen_wasm::emit(&module).map_err(|e| e.to_string())?;
    let wat = wasmprinter::print_bytes(&wasm_bytes).map_err(|e| e.to_string())?;

    Ok(CompileResult {
        ir: ir_dump,
        wat,
        wasm: wasm_bytes,
        requires_wasm_gc,
    })
}

fn module_requires_wasm_gc(module: &waluau_ir::Module) -> bool {
    module
        .functions
        .iter()
        .any(function_requires_wasm_gc)
}

fn function_requires_wasm_gc(function: &waluau_ir::Function) -> bool {
    type_requires_wasm_gc(&function.return_type)
        || function.params.iter().any(|(_, ty)| type_requires_wasm_gc(ty))
        || function.blocks.values().any(|block| {
            block.instructions.iter().any(|(_, instruction)| {
                matches!(
                    instruction,
                    waluau_ir::Instruction::ArrayNew { .. }
                        | waluau_ir::Instruction::ArrayGet { .. }
                        | waluau_ir::Instruction::ArraySet { .. }
                        | waluau_ir::Instruction::ArrayLen { .. }
                )
            })
        })
}

fn type_requires_wasm_gc(ty: &waluau_ast::Type) -> bool {
    matches!(ty, waluau_ast::Type::Array(_))
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
}
