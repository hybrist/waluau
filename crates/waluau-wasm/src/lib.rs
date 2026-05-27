use serde::Serialize;
use wasm_bindgen::prelude::*;

#[derive(Serialize)]
struct CompileResult {
    ir: String,
    wat: String,
    wasm: Vec<u8>,
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
    })
}
