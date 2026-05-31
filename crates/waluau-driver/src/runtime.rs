//! Wasmtime host imports for Waluau programs.

use std::sync::Arc;

use waluau_codegen_wasm::host;
use waluau_diagnostics::Diagnostic;
use wasmtime::{Caller, Engine, ExternRef, Instance, Linker, Module, Rooted, Store};

#[derive(Clone, Debug)]
pub struct HostState {
    strings: Arc<[String]>,
}

impl HostState {
    fn new(strings: Arc<[String]>) -> Self {
        Self { strings }
    }
}

pub fn parse_string_constants(wasm: &[u8]) -> Result<Vec<String>, Diagnostic> {
    host::parse_string_constants_from_wasm(wasm)
}

/// Create a store configured for Waluau modules (GC + reference types).
pub fn new_store(engine: &Engine, strings: Arc<[String]>) -> Store<HostState> {
    Store::new(engine, HostState::new(strings))
}

/// Instantiate a compiled Waluau module with the required `waluau` host imports.
pub fn instantiate(
    engine: &Engine,
    module: &Module,
    strings: Arc<[String]>,
) -> wasmtime::Result<(Store<HostState>, Instance)> {
    let mut store = new_store(engine, strings);
    let mut linker = Linker::new(engine);
    linker.func_wrap(
        host::IMPORT_MODULE,
        host::IMPORT_JS_STRING_CONST,
        |mut caller: Caller<'_, HostState>, index: i32| -> wasmtime::Result<Rooted<ExternRef>> {
            let value = caller
                .data()
                .strings
                .get(index as usize)
                .ok_or_else(|| wasmtime::Error::msg("js_string_const index out of bounds"))?
                .clone();
            ExternRef::new(&mut caller, value)
        },
    )?;
    linker.func_wrap(
        host::IMPORT_MODULE,
        host::IMPORT_JS_STRING_EQ,
        |mut caller: Caller<'_, HostState>, left: Rooted<ExternRef>, right: Rooted<ExternRef>| {
            let left = externref_string(
                &mut caller,
                left,
                "js_string_eq left operand is not a string",
            )?;
            let right = externref_string(
                &mut caller,
                right,
                "js_string_eq right operand is not a string",
            )?;
            Ok(i32::from(left == right))
        },
    )?;
    linker.func_wrap(
        host::IMPORT_MODULE,
        host::IMPORT_JS_STRING_CONCAT,
        |mut caller: Caller<'_, HostState>, left: Rooted<ExternRef>, right: Rooted<ExternRef>| {
            let left = externref_string(
                &mut caller,
                left,
                "js_string_concat left operand is not a string",
            )?;
            let right = externref_string(
                &mut caller,
                right,
                "js_string_concat right operand is not a string",
            )?;
            ExternRef::new(&mut caller, left + &right)
        },
    )?;
    linker.func_wrap(
        host::IMPORT_MODULE,
        host::IMPORT_PRINT,
        |mut caller: Caller<'_, HostState>, value: Rooted<ExternRef>| -> wasmtime::Result<()> {
            let value = externref_string(&mut caller, value, "print argument is not a string")?;
            println!("{value}");
            Ok(())
        },
    )?;
    let instance = linker.instantiate(&mut store, module)?;
    Ok((store, instance))
}

fn externref_string(
    caller: &mut Caller<'_, HostState>,
    value: Rooted<ExternRef>,
    message: &str,
) -> wasmtime::Result<String> {
    let payload = value
        .data(caller)?
        .ok_or_else(|| wasmtime::Error::msg(message.to_string()))?;
    payload
        .downcast_ref::<String>()
        .cloned()
        .ok_or_else(|| wasmtime::Error::msg(message.to_string()))
}
