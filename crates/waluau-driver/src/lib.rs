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
    let mut program = waluau_parser::parse(source)?;

    // Add builtin declarations to standalone programs
    add_builtins_to_program(&mut program)?;

    compile_program(program)
}

/// Compile `path`, resolving and linking any modules it imports with `require`.
pub fn compile_file(path: &Path) -> Result<Vec<u8>, Diagnostic> {
    let program = link::link_program(path)?;
    compile_program(program)
}

fn compile_program(program: waluau_ast::Program) -> Result<Vec<u8>, Diagnostic> {
    let mut typed_program = waluau_hir::type_check_and_infer(&program)?;
    waluau_ast::resolve_symbols(&mut typed_program)?;
    let ir = waluau_ir::build(&typed_program)?;
    Ok(waluau_codegen_wasm::emit(&ir)?.wasm)
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

fn add_builtins_to_program(program: &mut waluau_ast::Program) -> Result<(), Diagnostic> {
    // Load builtin declaration files and merge their declared_imports
    let builtin_files = ["core.walu"];

    for filename in &builtin_files {
        let builtin_source = match *filename {
            "core.walu" => include_str!("../../../builtins/core.walu"),
            _ => continue,
        };

        let builtin_program =
            waluau_parser::parse_with_path(builtin_source, &format!("builtin:{filename}"))?;
        program
            .declared_imports
            .extend(builtin_program.declared_imports);
    }

    Ok(())
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

    fn fixture_source(name: &str) -> &'static str {
        match name {
            "add" => include_str!("../../../fixtures/add.walu"),
            "mismatch" => include_str!("../../../fixtures/mismatch.walu"),
            "array_ops" => include_str!("../../../fixtures/array_ops.walu"),
            "string_ops" => include_str!("../../../fixtures/string-ops.walu"),
            other => panic!("unknown fixture: {other}"),
        }
    }

    fn os(value: impl AsRef<Path>) -> OsString {
        value.as_ref().as_os_str().to_owned()
    }

    fn fixture_path(relative: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(relative)
    }

    #[test]
    fn compiles_local_function_with_recursion() {
        super::compile_source(
            r#"
                function entry(n: i32): i32
                    local function fib(x: i32): i32
                        if x < 2 then
                            return x
                        end
                        return fib(x - 1) + fib(x - 2)
                    end
                    return fib(n)
                end
            "#,
        )
        .expect("local function with self-recursion should compile");
    }

    #[test]
    fn compiles_standalone_do_block_with_scoped_locals() {
        super::compile_source(
            r#"
                function entry(): i32
                    local x: i32 = 1
                    do
                        local y: i32 = 2
                        x = x + y
                    end
                    do
                        local y: i32 = 39
                        x = x + y
                    end
                    return x
                end
            "#,
        )
        .expect("standalone do blocks should compile");
    }

    #[test]
    fn compiles_fn_and_let_identifiers() {
        super::compile_source(
            r#"
                function entry(x: i32): i32
                    local fn = function(y: i32): i32 return y + 1 end
                    local let: i32 = fn(x)
                    return let
                end
            "#,
        )
        .expect("fn and let should be usable as identifiers");
    }

    #[test]
    fn compiles_backtick_string_interpolation() {
        super::compile_source(
            r#"
                function entry(n: i32): string
                    local label: string = "count"
                    return `{label} is {n + 1}!`
                end
            "#,
        )
        .expect("backtick interpolation should compile");
    }

    #[test]
    fn compiles_extended_string_escapes() {
        super::compile_source(
            r#"
                function entry(): string
                    return "\x41\u{1F600}" .. "a\z
                           b"
                end
            "#,
        )
        .expect("\\x, \\u{...}, and \\z escapes should compile");
    }

    #[test]
    fn compiles_generated_dom_externs_with_inheritance() {
        super::compile_source(include_str!("../../../externs/dom.walu"))
            .expect("generated DOM extern declarations should compile");
    }

    #[test]
    fn compile_file_resolves_dom_window_virtual_module() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("app.walu");
        fs::write(
            &input_path,
            r#"
                function main(): unit
                    local window = require("dom:window")
                    local document: Document = window.document
                end
            "#,
        )
        .expect("app should write");

        let wasm = super::compile_file(&input_path).expect("dom window require should compile");
        assert!(
            wasm.starts_with(b"\0asm"),
            "compiled wasm should start with the wasm magic bytes"
        );
    }

    #[test]
    fn compile_file_resolves_tfjs_model_loading_namespace() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("app.walu");
        fs::write(
            &input_path,
            r#"
                function load(url: string): Promise<GraphModel>
                    local tf = require("tfjs")
                    return tf.load_graph_model(url)
                end

                function predict(model: GraphModel, input: Tensor): Tensor
                    local tf = require("tfjs")
                    return tf.graph_model_predict(model, input)
                end
            "#,
        )
        .expect("app should write");

        let wasm = super::compile_file(&input_path).expect("tfjs model require should compile");
        assert!(
            wasm.starts_with(b"\0asm"),
            "compiled wasm should start with the wasm magic bytes"
        );
    }

    #[test]
    fn compile_file_resolves_tfjs_training_namespace() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("app.walu");
        fs::write(
            &input_path,
            r#"
                function train(model: LayersModel, input: Tensor, target: Tensor): Promise<TrainingHistory>
                    local tf = require("tfjs")
                    tf.layers_model_compile_sgd(model, "meanSquaredError", 0.01)
                    return tf.layers_model_fit_one(model, input, target, 2, 1)
                end

                function first_loss(history: TrainingHistory): f64
                    local tf = require("tfjs")
                    if tf.training_history_len(history) == 0 then
                        return 0.0
                    end
                    return tf.training_history_loss(history, 0)
                end
            "#,
        )
        .expect("app should write");

        let wasm = super::compile_file(&input_path).expect("tfjs training require should compile");
        assert!(
            wasm.starts_with(b"\0asm"),
            "compiled wasm should start with the wasm magic bytes"
        );
    }

    #[test]
    fn compile_file_resolves_tfjs_namespace_inside_coroutine_callback() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("app.walu");
        fs::write(
            &input_path,
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
            "#,
        )
        .expect("app should write");

        let wasm = super::compile_file(&input_path).expect("nested tfjs require should compile");
        assert!(
            wasm.starts_with(b"\0asm"),
            "compiled wasm should start with the wasm magic bytes"
        );
    }

    #[test]
    fn compile_file_resolves_top_level_dom_window_virtual_module() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("app.walu");
        fs::write(
            &input_path,
            r#"
                local window = require("dom:window")
                local document = window.document

                function main(): unit
                end
            "#,
        )
        .expect("app should write");

        let wasm = super::compile_file(&input_path).expect("dom window require should compile");
        assert!(
            wasm.starts_with(b"\0asm"),
            "compiled wasm should start with the wasm magic bytes"
        );
    }

    #[test]
    fn compile_file_accepts_callback_filtering_with_helper_and_string_find() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("app.walu");
        fs::write(
            &input_path,
            r#"
                function contains_text(haystack: string, needle: string): bool
                    return haystack:find(needle) ~= -1
                end

                function install(): unit
                    local window = require("dom:window")
                    local document: Document = window.document
                    local input_element: Element = document:create_element("input")
                    local seed_text: string = "typed card"

                    input_element:add_event_listener("input", function(event: Event): unit
                        if HTMLInputElement(target) = event.target then
                            local value: string = target.value
                            local matches_seed: bool = value == "" or seed_text:find(value) ~= -1
                            if contains_text(seed_text, value) then
                            end
                            if matches_seed then
                            end
                        end
                    end)
                end
            "#,
        )
        .expect("app should write");

        let wasm = super::compile_file(&input_path)
            .expect("callback filtering inside DOM event handlers should compile");
        assert!(
            wasm.starts_with(b"\0asm"),
            "compiled wasm should start with the wasm magic bytes"
        );
    }

    #[test]
    fn compile_file_rejects_unknown_dom_virtual_module() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("app.walu");
        fs::write(
            &input_path,
            r#"
                function main(): unit
                    local worker = require("dom:worker")
                end
            "#,
        )
        .expect("app should write");

        let error =
            super::compile_file(&input_path).expect_err("unknown DOM virtual module should fail");
        assert_eq!(
            error.to_string(),
            "unsupported DOM virtual module \"dom:worker\"; supported specifiers: \"dom:window\""
        );
    }

    #[test]
    fn rejects_math_abs_for_integer_types() {
        let source = r#"
            function bad(x: i32): i32
                return math.abs(x)
            end
        "#;
        let err = super::compile_source(source).expect_err("compile should fail");
        assert!(err.to_string().contains("math.abs does not support i32"));
    }

    #[test]
    fn compiles_immediately_invoked_function_expression() {
        // Anonymous functions that omit a return type annotation now have it
        // inferred (and backfilled onto the AST) so they lower to wasm. This is
        // the pattern that pervades the `basic.*` Luau conformance chunks.
        let source = r#"
            local answer = (function()
                local a = 1
                return a + 41
            end)()
            assert(answer == 42)
        "#;
        let wasm = super::compile_source(source)
            .expect("immediately-invoked function expression should compile");
        assert!(
            wasm.starts_with(b"\0asm"),
            "compiled wasm should start with the wasm magic bytes"
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

        assert!(
            output_path.exists(),
            "default output file should be created"
        );
        assert!(
            fs::metadata(&output_path).expect("output metadata").len() > 0,
            "output wasm should be non-empty"
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
    fn compiles_relative_imports() {
        super::compile_file(&fixture_path("modules/main.walu")).expect("compile should succeed");
    }

    #[test]
    fn compiles_namespace_table_exports() {
        super::compile_file(&fixture_path("modules/namespace_main.walu"))
            .expect("compile should succeed");
    }

    #[test]
    fn compiles_reexported_bindings() {
        super::compile_file(&fixture_path("modules/reexport_main.walu"))
            .expect("compile should succeed");
    }

    #[test]
    fn compile_file_supports_opaque_type_declarations_across_linking() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("types_main.walu");
        fs::write(
            &input_path,
            r#"
                type Meters = number

                function entry(): number
                    local len = 10::Meters
                    return len::number
                end
            "#,
        )
        .expect("fixture should write");

        let wasm = super::compile_file(&input_path).expect("compile should succeed");
        assert!(
            !wasm.is_empty(),
            "successful compilation should produce a wasm module"
        );
    }

    #[test]
    fn compiles_array_ops() {
        super::compile_file(&fixture_path("array_ops.walu")).expect("compile should succeed");
    }

    #[test]
    fn compile_file_supports_method_declarations_and_calls() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("methods.walu");
        fs::write(
            &input_path,
            r#"
                local point = { x = 41::i32 }

                function point:get_x(): i32
                    return self.x
                end

                assert(point:get_x() == 41)
            "#,
        )
        .expect("fixture should write");

        let wasm = super::compile_file(&input_path).expect("compile should succeed");
        assert!(
            !wasm.is_empty(),
            "successful compilation should produce a wasm module"
        );
    }

    #[test]
    fn compiles_bytes_ops() {
        let source = r#"
            function entry(data: bytes): i32
                local prefix: bytes = b"AB"
                local merged: bytes = prefix .. data
                if merged > b"A" then
                    return merged[0] + #merged
                end
                return 0
            end
        "#;
        super::compile_source(source).expect("compile should succeed");
    }

    #[test]
    fn compiles_string_ops() {
        let source = r#"
            function compare_strings(a: string, b: string): bool
                if a < b then
                    return true
                elseif a > b then
                    return false
                else
                    return a == b
                end
            end

            function test_strings(): string
                local greeting: string = "Hello, " .. "world"
                if greeting > "H" then
                    return greeting
                else
                    return "Empty"
                end
            end
        "#;
        super::compile_source(source).expect("compile should succeed");
    }

    #[test]
    fn compiles_string_ops_fixture() {
        super::compile_file(&fixture_path("string-ops.walu")).expect("compile should succeed");
    }
}
