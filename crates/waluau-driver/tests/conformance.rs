//! Conformance test harness for Waluau programs.
//!
//! Every `*.walu` file under the top-level `conformance/` directory is a
//! self-contained test. Each program checks its own behaviour with top-level
//! `assert(...)` statements, which run during module instantiation (via the
//! WebAssembly `start` section). The harness therefore only has to compile and
//! instantiate each program:
//!
//! - a test **passes** when the program compiles and instantiates without
//!   trapping, and
//! - a test **fails** when the program fails to compile, or when a top-level
//!   assertion does not hold (which traps during instantiation).
//!
//! New `*.walu` files are discovered automatically, so adding a test is just a
//! matter of dropping a file into `conformance/`. See `conformance/README.md`.

use std::fs;
use std::path::{Path, PathBuf};

use std::sync::Arc;

use waluau_driver::{compile_source, runtime};
use wasmtime::{Config, Engine, Module};

/// Absolute path to the top-level `conformance/` directory.
fn conformance_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance")
        .canonicalize()
        .expect("conformance directory should exist")
}

/// All `*.walu` files under `dir`, recursively, in a stable order.
fn walu_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_walu_files(dir, &mut files);
    files.sort();
    files
}

fn collect_walu_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|error| panic!("read {}: {error}", dir.display()));
    for entry in entries {
        let path = entry.expect("directory entry should be readable").path();
        if path.is_dir() {
            collect_walu_files(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("walu") {
            out.push(path);
        }
    }
}

/// An engine configured with the WebAssembly features the codegen relies on
/// (e.g. wasm-gc, used by arrays).
fn engine() -> Engine {
    Engine::new(
        Config::new()
            .wasm_function_references(true)
            .wasm_gc(true)
            .wasm_reference_types(true),
    )
    .expect("engine should configure wasm features")
}

/// Compile and instantiate a single conformance program. Instantiation runs the
/// module's `start` section, executing every top-level statement (including
/// assertions), so a returned `Ok` means the program ran to completion without
/// trapping.
fn run_case(engine: &Engine, source: &str) -> Result<(), String> {
    let wasm = compile_source(source).map_err(|error| format!("compile error: {error}"))?;
    let strings = Arc::from(
        runtime::parse_string_constants(&wasm)
            .map_err(|error| format!("string constants error: {error}"))?,
    );
    let module = Module::new(engine, &wasm).map_err(|error| format!("invalid module: {error}"))?;
    runtime::instantiate(engine, &module, strings)
        .map_err(|error| format!("trap during instantiation: {error}"))?;
    Ok(())
}

#[test]
fn conformance_suite() {
    let dir = conformance_dir();
    let files = walu_files(&dir);
    assert!(
        !files.is_empty(),
        "no .walu conformance tests found under {}",
        dir.display()
    );

    let engine = engine();
    let mut failures = Vec::new();

    for file in &files {
        let name = file
            .strip_prefix(&dir)
            .unwrap_or(file)
            .display()
            .to_string();
        let source =
            fs::read_to_string(file).unwrap_or_else(|error| panic!("read {name}: {error}"));
        match run_case(&engine, &source) {
            Ok(()) => println!("ok    {name}"),
            Err(reason) => {
                println!("FAIL  {name}: {reason}");
                failures.push(format!("{name}: {reason}"));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} conformance test(s) failed:\n  {}",
        failures.len(),
        files.len(),
        failures.join("\n  ")
    );

    println!("all {} conformance test(s) passed", files.len());
}
