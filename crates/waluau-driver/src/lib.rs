use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use waluau_diagnostics::Diagnostic;

pub fn compile_source(source: &str) -> Result<Vec<u8>, Diagnostic> {
    let program = waluau_parser::parse(source)?;
    waluau_hir::type_check(&program)?;
    let ir = waluau_ir::build(&program)?;
    waluau_codegen_wasm::emit(&ir)
}

pub fn run() -> Result<(), Diagnostic> {
    run_with_args(std::env::args_os().skip(1))
}

pub fn run_with_args<I>(args: I) -> Result<(), Diagnostic>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_args(args)?;
    let source = fs::read_to_string(&options.input)
        .map_err(|error| io_error("read input file", &options.input, error))?;
    let wasm = compile_source(&source)?;
    fs::write(&options.output, wasm)
        .map_err(|error| io_error("write output file", &options.output, error))?;
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
struct CliOptions {
    input: PathBuf,
    output: PathBuf,
}

fn parse_args<I>(args: I) -> Result<CliOptions, Diagnostic>
where
    I: IntoIterator<Item = OsString>,
{
    let mut input = None;
    let mut output = None;
    let mut pending_output_flag = false;

    for arg in args {
        if pending_output_flag {
            output = Some(PathBuf::from(arg));
            pending_output_flag = false;
            continue;
        }

        match arg.to_str() {
            Some("-o" | "--output") => pending_output_flag = true,
            Some(flag) if flag.starts_with('-') => {
                return Err(Diagnostic::new(format!(
                    "unsupported flag `{flag}`\nusage: waluau <input.walu> [-o <output.wasm>]"
                )));
            }
            _ if input.is_none() => input = Some(PathBuf::from(arg)),
            _ => {
                return Err(Diagnostic::new(
                    "too many positional arguments\nusage: waluau <input.walu> [-o <output.wasm>]",
                ));
            }
        }
    }

    if pending_output_flag {
        return Err(Diagnostic::new(
            "missing path after -o/--output\nusage: waluau <input.walu> [-o <output.wasm>]",
        ));
    }

    let input = input.ok_or_else(|| {
        Diagnostic::new("missing input path\nusage: waluau <input.walu> [-o <output.wasm>]")
    })?;
    let output = output.unwrap_or_else(|| default_output_path(&input));

    Ok(CliOptions { input, output })
}

fn default_output_path(input: &Path) -> PathBuf {
    input.with_extension("wasm")
}

fn io_error(action: &str, path: &Path, error: std::io::Error) -> Diagnostic {
    Diagnostic::new(format!("{action} `{}`: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;
    use wasmtime::{Config, Engine, Instance, Module, Store};

    fn fixture_source(name: &str) -> &'static str {
        match name {
            "add" => include_str!("../../../fixtures/add.walu"),
            "branch_calls_i32" => include_str!("../../../fixtures/branch_calls_i32.walu"),
            "literals_i64_u64" => include_str!("../../../fixtures/literals_i64_u64.walu"),
            "loop_sum_to_i32" => include_str!("../../../fixtures/loop_sum_to_i32.walu"),
            "repeat_until_sum" => include_str!("../../../fixtures/repeat_until_sum.walu"),
            "mismatch" => include_str!("../../../fixtures/mismatch.walu"),
            other => panic!("unknown fixture: {other}"),
        }
    }

    fn instantiate(wasm: &[u8]) -> (Store<()>, Instance) {
        let engine = Engine::default();
        let module = Module::new(&engine, wasm).expect("module should compile");
        let mut store = Store::new(&engine, ());
        let instance = Instance::new(&mut store, &module, &[]).expect("instance should create");
        (store, instance)
    }

    fn instantiate_with_gc(wasm: &[u8]) -> (Store<()>, Instance) {
        let engine = Engine::new(Config::new().wasm_function_references(true).wasm_gc(true))
            .expect("engine should configure wasm-gc");
        let module = Module::new(&engine, wasm).expect("module should compile");
        let mut store = Store::new(&engine, ());
        let instance = Instance::new(&mut store, &module, &[]).expect("instance should create");
        (store, instance)
    }

    fn os(value: impl AsRef<Path>) -> OsString {
        value.as_ref().as_os_str().to_owned()
    }

    #[test]
    fn compiles_and_executes_valid_fixture_file() {
        let source = fixture_source("add");
        let wasm = super::compile_source(source).expect("compile should succeed");
        let (mut store, instance) = instantiate(&wasm);
        let add = instance
            .get_typed_func::<(f64, f64), f64>(&mut store, "add")
            .expect("add export should exist");
        let result = add
            .call(&mut store, (1.5, 2.25))
            .expect("call should succeed");
        assert_eq!(result, 3.75);
    }

    #[test]
    fn executes_branching_and_direct_calls() {
        let source = fixture_source("branch_calls_i32");
        let wasm = super::compile_source(source).expect("compile should succeed");
        let (mut store, instance) = instantiate(&wasm);
        let max_plus_one = instance
            .get_typed_func::<(i32, i32), i32>(&mut store, "max_plus_one")
            .expect("max_plus_one export should exist");
        assert_eq!(
            max_plus_one
                .call(&mut store, (7, 3))
                .expect("call should succeed"),
            8
        );
        assert_eq!(
            max_plus_one
                .call(&mut store, (2, 5))
                .expect("call should succeed"),
            6
        );
    }

    #[test]
    fn executes_loops() {
        let source = fixture_source("loop_sum_to_i32");
        let wasm = super::compile_source(source).expect("compile should succeed");
        let (mut store, instance) = instantiate(&wasm);
        let sum_to = instance
            .get_typed_func::<i32, i32>(&mut store, "sum_to")
            .expect("sum_to export should exist");
        assert_eq!(sum_to.call(&mut store, 5).expect("call should succeed"), 15);
    }

    #[test]
    fn executes_repeat_until_loops() {
        let source = fixture_source("repeat_until_sum");
        let wasm = super::compile_source(source).expect("compile should succeed");
        let (mut store, instance) = instantiate(&wasm);
        let sum_to = instance
            .get_typed_func::<i32, i32>(&mut store, "sum_to")
            .expect("sum_to export should exist");
        assert_eq!(sum_to.call(&mut store, 5).expect("call should succeed"), 15);

        let runs_once = instance
            .get_typed_func::<(), i32>(&mut store, "runs_once")
            .expect("runs_once export should exist");
        assert_eq!(
            runs_once.call(&mut store, ()).expect("call should succeed"),
            1
        );
    }

    #[test]
    fn executes_i64_and_u64_locals_initialized_from_literals() {
        let source = fixture_source("literals_i64_u64");
        let wasm = super::compile_source(source).expect("compile should succeed");
        let (mut store, instance) = instantiate(&wasm);

        let return_u64_small = instance
            .get_typed_func::<(), u64>(&mut store, "return_u64_small")
            .expect("return_u64_small export should exist");
        assert_eq!(
            return_u64_small
                .call(&mut store, ())
                .expect("call should succeed"),
            42
        );

        let return_i64_small = instance
            .get_typed_func::<(), i64>(&mut store, "return_i64_small")
            .expect("return_i64_small export should exist");
        assert_eq!(
            return_i64_small
                .call(&mut store, ())
                .expect("call should succeed"),
            42
        );

        let return_u64_max = instance
            .get_typed_func::<(), u64>(&mut store, "return_u64_max")
            .expect("return_u64_max export should exist");
        assert_eq!(
            return_u64_max
                .call(&mut store, ())
                .expect("call should succeed"),
            u64::MAX
        );

        let return_i64_max = instance
            .get_typed_func::<(), i64>(&mut store, "return_i64_max")
            .expect("return_i64_max export should exist");
        assert_eq!(
            return_i64_max
                .call(&mut store, ())
                .expect("call should succeed"),
            i64::MAX
        );
    }

    #[test]
    fn executes_array_length_after_mutation() {
        let source = r#"
            function score_count(): i32
                local scores: {number} = {100, 250, 300}
                local first: number = scores[0]
                scores[1] = first + 1
                return #scores
            end
        "#;
        let wasm = super::compile_source(source).expect("compile should succeed");
        let (mut store, instance) = instantiate_with_gc(&wasm);
        let score_count = instance
            .get_typed_func::<(), i32>(&mut store, "score_count")
            .expect("score_count export should exist");
        assert_eq!(
            score_count
                .call(&mut store, ())
                .expect("call should succeed"),
            3
        );
    }

    #[test]
    fn executes_floor_division_and_modulo_for_negative_operands() {
        let source = r#"
            function floor_div(): number
                return -7 // 3
            end

            function modulo(): number
                return -7 % 3
            end

            function precedence(): number
                return 1 + 8 // 3 * 2 % 5
            end
        "#;
        let wasm = super::compile_source(source).expect("compile should succeed");
        let (mut store, instance) = instantiate(&wasm);

        let floor_div = instance
            .get_typed_func::<(), f64>(&mut store, "floor_div")
            .expect("floor_div export should exist");
        assert_eq!(
            floor_div.call(&mut store, ()).expect("call should succeed"),
            -3.0
        );

        let modulo = instance
            .get_typed_func::<(), f64>(&mut store, "modulo")
            .expect("modulo export should exist");
        assert_eq!(
            modulo.call(&mut store, ()).expect("call should succeed"),
            2.0
        );

        let precedence = instance
            .get_typed_func::<(), f64>(&mut store, "precedence")
            .expect("precedence export should exist");
        assert_eq!(
            precedence
                .call(&mut store, ())
                .expect("call should succeed"),
            5.0
        );
    }

    #[test]
    fn rejects_invalid_fixture_file() {
        let source = fixture_source("mismatch");
        let err = super::compile_source(source).expect_err("compile should fail");
        assert_eq!(err.to_string(), "return expects f64, got bool");
    }

    #[test]
    fn cli_writes_default_output_file() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("add.walu");
        let output_path = tempdir.path().join("add.wasm");
        fs::write(&input_path, fixture_source("add")).expect("fixture should write");

        super::run_with_args([os(&input_path)]).expect("cli run should succeed");

        let wasm = fs::read(&output_path).expect("default output should exist");
        Module::new(&Engine::default(), wasm).expect("output should be valid wasm");
    }

    #[test]
    fn cli_reports_compile_diagnostics() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("mismatch.walu");
        let output_path = tempdir.path().join("custom-output.wasm");
        fs::write(&input_path, fixture_source("mismatch")).expect("fixture should write");

        let error = super::run_with_args([
            os(&input_path),
            OsString::from("--output"),
            os(&output_path),
        ])
        .expect_err("cli run should fail");

        assert_eq!(error.to_string(), "return expects f64, got bool");
        assert!(
            !output_path.exists(),
            "failed compilation must not write output"
        );
    }
}
