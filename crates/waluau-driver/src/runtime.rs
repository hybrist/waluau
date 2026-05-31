//! Wasmtime host imports for Waluau programs.

use std::sync::Arc;

use waluau_codegen_wasm::host;
use waluau_diagnostics::Diagnostic;
use wasmtime::{
    Caller, Engine, Extern, ExternRef, ExternType, Global, Instance, Linker, Module, Rooted, Store,
    Val,
};

#[derive(Clone, Debug)]
pub struct HostState;

impl HostState {
    fn new() -> Self {
        Self
    }
}

pub fn parse_string_constants(wasm: &[u8]) -> Result<Vec<String>, Diagnostic> {
    host::parse_string_constants_from_wasm(wasm)
}

/// Create a store configured for Waluau modules (GC + reference types).
pub fn new_store(engine: &Engine) -> Store<HostState> {
    Store::new(engine, HostState::new())
}

/// Instantiate a compiled Waluau module with the required host imports.
pub fn instantiate(
    engine: &Engine,
    module: &Module,
    strings: Arc<[String]>,
) -> wasmtime::Result<(Store<HostState>, Instance)> {
    let mut store = new_store(engine);
    let mut linker = Linker::new(engine);
    linker.func_wrap(
        host::JS_STRING_BUILTINS_MODULE,
        host::IMPORT_JS_STRING_EQ,
        |mut caller: Caller<'_, HostState>,
         left: Option<Rooted<ExternRef>>,
         right: Option<Rooted<ExternRef>>| {
            let left = externref_nullable_string(
                &mut caller,
                left,
                "js_string_eq left operand is not a string",
            )?;
            let right = externref_nullable_string(
                &mut caller,
                right,
                "js_string_eq right operand is not a string",
            )?;
            Ok(i32::from(left == right))
        },
    )?;
    linker.func_wrap(
        host::JS_STRING_BUILTINS_MODULE,
        host::IMPORT_JS_STRING_CONCAT,
        |mut caller: Caller<'_, HostState>,
         left: Option<Rooted<ExternRef>>,
         right: Option<Rooted<ExternRef>>| {
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
            Ok(Some(ExternRef::new(&mut caller, left + &right)?))
        },
    )?;
    for import in module.imports() {
        if import.module() != host::IMPORTED_STRING_CONSTANTS_MODULE {
            continue;
        }
        let ExternType::Global(global_ty) = import.ty() else {
            continue;
        };
        let literal = import.name();
        if !strings.iter().any(|value| value == literal) {
            return Err(wasmtime::Error::msg(format!(
                "missing imported string constant `{literal}`"
            )));
        }
        let literal_ref = ExternRef::new(&mut store, literal.to_string())?;
        let global = Global::new(&mut store, global_ty, Val::ExternRef(Some(literal_ref)))?;
        linker.define(
            &mut store,
            host::IMPORTED_STRING_CONSTANTS_MODULE,
            literal,
            Extern::Global(global),
        )?;
    }
    linker.func_wrap(
        host::IMPORT_MODULE,
        host::IMPORT_JS_TOSTRING_I32,
        |mut caller: Caller<'_, HostState>, value: i32| {
            Ok(Some(ExternRef::new(&mut caller, value.to_string())?))
        },
    )?;
    linker.func_wrap(
        host::IMPORT_MODULE,
        host::IMPORT_JS_TOSTRING_U32,
        |mut caller: Caller<'_, HostState>, value: i32| {
            Ok(Some(ExternRef::new(
                &mut caller,
                (value as u32).to_string(),
            )?))
        },
    )?;
    linker.func_wrap(
        host::IMPORT_MODULE,
        host::IMPORT_JS_TOSTRING_I64,
        |mut caller: Caller<'_, HostState>, value: i64| {
            Ok(Some(ExternRef::new(&mut caller, value.to_string())?))
        },
    )?;
    linker.func_wrap(
        host::IMPORT_MODULE,
        host::IMPORT_JS_TOSTRING_U64,
        |mut caller: Caller<'_, HostState>, value: i64| {
            Ok(Some(ExternRef::new(
                &mut caller,
                (value as u64).to_string(),
            )?))
        },
    )?;
    linker.func_wrap(
        host::IMPORT_MODULE,
        host::IMPORT_JS_TOSTRING_F32,
        |mut caller: Caller<'_, HostState>, value: f32| {
            Ok(Some(ExternRef::new(&mut caller, value.to_string())?))
        },
    )?;
    linker.func_wrap(
        host::IMPORT_MODULE,
        host::IMPORT_JS_TOSTRING_F64,
        |mut caller: Caller<'_, HostState>, value: f64| {
            Ok(Some(ExternRef::new(&mut caller, value.to_string())?))
        },
    )?;
    linker.func_wrap(
        host::IMPORT_MODULE,
        host::IMPORT_JS_TOSTRING_BOOL,
        |mut caller: Caller<'_, HostState>, value: i32| {
            Ok(Some(ExternRef::new(&mut caller, (value != 0).to_string())?))
        },
    )?;
    linker.func_wrap(
        host::IMPORT_MODULE,
        host::IMPORT_PRINT,
        |mut caller: Caller<'_, HostState>,
         value: Option<Rooted<ExternRef>>|
         -> wasmtime::Result<()> {
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
    value: Option<Rooted<ExternRef>>,
    message: &str,
) -> wasmtime::Result<String> {
    let Some(value) = value else {
        return Err(wasmtime::Error::msg(message.to_string()));
    };
    let payload = value
        .data(caller)?
        .ok_or_else(|| wasmtime::Error::msg(message.to_string()))?;
    payload
        .downcast_ref::<String>()
        .cloned()
        .ok_or_else(|| wasmtime::Error::msg(message.to_string()))
}

fn externref_nullable_string(
    caller: &mut Caller<'_, HostState>,
    value: Option<Rooted<ExternRef>>,
    message: &str,
) -> wasmtime::Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let payload = value
        .data(caller)?
        .ok_or_else(|| wasmtime::Error::msg(message.to_string()))?;
    let string = payload
        .downcast_ref::<String>()
        .cloned()
        .ok_or_else(|| wasmtime::Error::msg(message.to_string()))?;
    Ok(Some(string))
}
