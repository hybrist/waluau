use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use waluau_diagnostics::Diagnostic;

mod link;

/// Compile a single source string with no module resolution.
///
/// Any `require(...)` in the source is rejected, since relative imports can only
/// be resolved against a file path. Use [`compile_file`] for programs that use
/// `require`.
pub fn compile_source(source: &str) -> Result<Vec<u8>, Diagnostic> {
    let program = waluau_parser::parse(source)?;
    compile_program(program)
}

/// Compile `path`, resolving and linking any modules it imports with `require`.
pub fn compile_file(path: &Path) -> Result<Vec<u8>, Diagnostic> {
    let program = link::link_program(path)?;
    compile_program(program)
}

fn compile_program(program: waluau_ast::Program) -> Result<Vec<u8>, Diagnostic> {
    let typed_program = waluau_hir::type_check_and_infer(&program)?;
    let ir = waluau_ir::build(&typed_program)?;
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
    let wasm = compile_file(&options.input)?;
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
            "closure_named_recursion" => {
                include_str!("../../../fixtures/closure_named_recursion.walu")
            }
            "nested_closure_noncapturing" => {
                include_str!("../../../fixtures/nested_closure_noncapturing.walu")
            }
            "closure_capture_unsupported" => {
                include_str!("../../../fixtures/closure_capture_unsupported.walu")
            }
            "assert_pass" => include_str!("../../../fixtures/assert_pass.walu"),
            "top_level_statements" => include_str!("../../../fixtures/top_level_statements.walu"),
            "mismatch" => include_str!("../../../fixtures/mismatch.walu"),
            "multi_value" => include_str!("../../../fixtures/multi_value.walu"),
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
    fn executes_named_function_expression_recursion() {
        let source = fixture_source("closure_named_recursion");
        let wasm = super::compile_source(source).expect("compile should succeed");
        let (mut store, instance) = instantiate(&wasm);
        let fact_entry = instance
            .get_typed_func::<i32, i32>(&mut store, "fact_entry")
            .expect("fact_entry export should exist");
        assert_eq!(
            fact_entry.call(&mut store, 5).expect("call should succeed"),
            120
        );
    }

    #[test]
    fn executes_nested_non_capturing_closures() {
        let source = fixture_source("nested_closure_noncapturing");
        let wasm = super::compile_source(source).expect("compile should succeed");
        let (mut store, instance) = instantiate(&wasm);
        let nested_result = instance
            .get_typed_func::<(), i32>(&mut store, "nested_result")
            .expect("nested_result export should exist");
        assert_eq!(
            nested_result
                .call(&mut store, ())
                .expect("call should succeed"),
            7
        );
    }

    #[test]
    fn executes_coroutine_create_and_resume() {
        let source = r#"
            function run_job(): i32
                local job: () -> i32 = function(): i32
                    return 7
                end
                local co: () -> i32 = coroutine_create(job)
                return coroutine_resume(co)
            end
        "#;
        let wasm = super::compile_source(source).expect("compile should succeed");
        let (mut store, instance) = instantiate(&wasm);
        let run_job = instance
            .get_typed_func::<(), i32>(&mut store, "run_job")
            .expect("run_job export should exist");
        assert_eq!(
            run_job.call(&mut store, ()).expect("call should succeed"),
            7
        );
    }

    #[test]
    fn executes_coroutine_status_for_created_coroutine() {
        let source = r#"
            function status_flag(): i32
                local job: () -> i32 = function(): i32
                    return 1
                end
                local co: () -> i32 = coroutine_create(job)
                if coroutine_status(co) then
                    return 1
                end
                return 0
            end
        "#;
        let wasm = super::compile_source(source).expect("compile should succeed");
        let (mut store, instance) = instantiate(&wasm);
        let status_flag = instance
            .get_typed_func::<(), i32>(&mut store, "status_flag")
            .expect("status_flag export should exist");
        assert_eq!(
            status_flag
                .call(&mut store, ())
                .expect("call should succeed"),
            1
        );
    }

    #[test]
    fn executes_math_intrinsics_mvp() {
        let source = r#"
            function math_ops(a: f64, b: f64): f64
                local m1: f64 = math_min(a, b)
                local m2: f64 = math_max(a, b)
                local abs_a: f64 = math_abs(a)
                local root: f64 = math_sqrt(9.0)
                local floored: f64 = math_floor(2.8)
                local ceiled: f64 = math_ceil(2.2)
                local truncated: f64 = math_trunc(-2.8)
                local rounded: f64 = math_nearest(2.5)
                local sign: f64 = math_copysign(3.0, -1.0)
                return m1 + m2 + abs_a + root + floored + ceiled + truncated + rounded + sign
            end
        "#;
        let wasm = super::compile_source(source).expect("compile should succeed");
        let (mut store, instance) = instantiate(&wasm);
        let math_ops = instance
            .get_typed_func::<(f64, f64), f64>(&mut store, "math_ops")
            .expect("math_ops export should exist");
        assert_eq!(
            math_ops
                .call(&mut store, (-4.0, 2.0))
                .expect("call should succeed"),
            7.0
        );
    }

    #[test]
    fn rejects_math_abs_for_integer_types() {
        let source = r#"
            function bad(x: i32): i32
                return math_abs(x)
            end
        "#;
        let err = super::compile_source(source).expect_err("compile should fail");
        assert!(err.to_string().contains("math_abs does not support i32"));
    }

    #[test]
    fn executes_closure_capture_fixture() {
        let source = fixture_source("closure_capture_unsupported");
        let wasm = super::compile_source(source).expect("compile should succeed");
        let (mut store, instance) = instantiate_with_gc(&wasm);
        let capture_entry = instance
            .get_typed_func::<i32, i32>(&mut store, "capture_entry")
            .expect("capture_entry export should exist");
        assert_eq!(
            capture_entry
                .call(&mut store, 5)
                .expect("call should succeed"),
            12
        );
    }

    #[test]
    fn compiles_fixture_with_top_level_statements() {
        let source = fixture_source("top_level_statements");
        let wasm = super::compile_source(source).expect("compile should succeed");
        let (mut store, instance) = instantiate(&wasm);
        let answer = instance
            .get_typed_func::<(), i32>(&mut store, "answer")
            .expect("answer export should exist");
        assert_eq!(
            answer.call(&mut store, ()).expect("call should succeed"),
            42
        );
    }

    #[test]
    fn executes_top_level_code_during_instantiation() {
        let source = r#"
            local boom: i32 = (1 :: i32) / (0 :: i32)

            function answer(): i32
                return 42
            end
        "#;
        let wasm = super::compile_source(source).expect("compile should succeed");
        let engine = Engine::default();
        let module = Module::new(&engine, wasm).expect("module should compile");
        let mut store = Store::new(&engine, ());
        Instance::new(&mut store, &module, &[]).expect_err("instantiation should trap");
    }

    #[test]
    fn executes_multi_value_fixture() {
        let source = fixture_source("multi_value");
        let wasm = super::compile_source(source).expect("compile should succeed");
        let (mut store, instance) = instantiate(&wasm);
        let swap = instance
            .get_typed_func::<(i32, i32), (i32, i32)>(&mut store, "swap")
            .expect("swap export should exist");
        assert_eq!(
            swap.call(&mut store, (3, 7)).expect("call should succeed"),
            (7, 3)
        );
        let sum_of_swap = instance
            .get_typed_func::<(i32, i32), i32>(&mut store, "sum_of_swap")
            .expect("sum_of_swap export should exist");
        assert_eq!(
            sum_of_swap
                .call(&mut store, (10, 20))
                .expect("call should succeed"),
            30
        );
    }

    #[test]
    fn executes_assertion_fixture() {
        let source = fixture_source("assert_pass");
        let wasm = super::compile_source(source).expect("compile should succeed");
        let (mut store, instance) = instantiate(&wasm);
        let check = instance
            .get_typed_func::<i32, i32>(&mut store, "check")
            .expect("check export should exist");
        assert_eq!(check.call(&mut store, 41).expect("call should succeed"), 42);
    }

    #[test]
    fn traps_on_failed_assertion() {
        let source = r#"
            function check(): i32
                assert(false)
                return 1
            end
        "#;
        let wasm = super::compile_source(source).expect("compile should succeed");
        let (mut store, instance) = instantiate(&wasm);
        let check = instance
            .get_typed_func::<(), i32>(&mut store, "check")
            .expect("check export should exist");
        check
            .call(&mut store, ())
            .expect_err("assert(false) should trap");
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

    fn fixture_path(relative: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(relative)
    }

    #[test]
    fn compiles_and_executes_relative_imports() {
        let wasm = super::compile_file(&fixture_path("modules/main.walu"))
            .expect("compile should succeed");
        let (mut store, instance) = instantiate(&wasm);
        let compute = instance
            .get_typed_func::<i32, i32>(&mut store, "compute")
            .expect("compute export should exist");
        // double(5) = add(helper(5), 5) = add(5, 5) = 10
        // compute(5) = add(double(5), helper()) = add(10, 100) = 110
        assert_eq!(
            compute.call(&mut store, 5).expect("call should succeed"),
            110
        );
    }

    #[test]
    fn mangling_keeps_same_named_functions_from_different_modules() {
        // `helper` is defined in both the entry module and the imported
        // `double` module; linking must keep both as distinct exports.
        let wasm = super::compile_file(&fixture_path("modules/main.walu"))
            .expect("compile should succeed");
        let (mut store, instance) = instantiate(&wasm);
        let entry_helper = instance
            .get_typed_func::<(), i32>(&mut store, "helper")
            .expect("entry helper export should exist");
        assert_eq!(
            entry_helper
                .call(&mut store, ())
                .expect("call should succeed"),
            100
        );
    }

    #[test]
    fn detects_circular_imports() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("a.walu"),
            "function a(): i32\n    local b: () -> i32 = require(\"./b\")\n    return b()\nend\nreturn a\n",
        )
        .expect("a should write");
        fs::write(
            tempdir.path().join("b.walu"),
            "function b(): i32\n    local a: () -> i32 = require(\"./a\")\n    return a()\nend\nreturn b\n",
        )
        .expect("b should write");

        let error = super::compile_file(&tempdir.path().join("a.walu"))
            .expect_err("circular import should fail");
        assert!(
            error.to_string().contains("circular module import"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn reports_missing_module_export() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("lib.walu"),
            "function noop(): i32\n    return 0\nend\n",
        )
        .expect("lib should write");
        fs::write(
            tempdir.path().join("app.walu"),
            "function main(): i32\n    local f: () -> i32 = require(\"./lib\")\n    return f()\nend\n",
        )
        .expect("app should write");

        let error = super::compile_file(&tempdir.path().join("app.walu"))
            .expect_err("missing export should fail");
        assert!(
            error.to_string().contains("has no export"),
            "unexpected error: {error}"
        );
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
