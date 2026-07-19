use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
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

    Ok(compile_program(program, "program.wasm", empty_asset_manifest())
        .map_err(|mut errors| errors.remove(0))?
        .wasm)
}

/// Like [`compile_source`], but reports every independently-attributable
/// diagnostic: all parse errors, or (when parsing is clean) every failing
/// function/statement from the type checker.
pub fn compile_source_collect(source: &str) -> Result<Vec<u8>, Vec<Diagnostic>> {
    let waluau_parser::ParseOutcome {
        mut program,
        diagnostics,
    } = waluau_parser::parse_with_recovery(source, "source");
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    // Add builtin declarations to standalone programs
    add_builtins_to_program(&mut program).map_err(|error| vec![error])?;

    Ok(compile_program(program, "program.wasm", empty_asset_manifest())?.wasm)
}

/// Compile `path`, resolving and linking any modules it imports with `require`.
pub fn compile_file(path: &Path) -> Result<Vec<u8>, Diagnostic> {
    Ok(compile_file_artifacts(path, "program.wasm")?.wasm)
}

#[derive(Debug)]
pub struct CompileArtifacts {
    pub wasm: Vec<u8>,
    pub js: String,
    pub required_imports: Vec<waluau_codegen_wasm::RequiredImport>,
    pub bytes_constants: Vec<Vec<u8>>,
}

/// Compile a linked file and return both in-memory artifacts. `wasm_file_name`
/// becomes the import-meta-relative sibling URL embedded in the JavaScript.
pub fn compile_file_artifacts(
    path: &Path,
    wasm_file_name: &str,
) -> Result<CompileArtifacts, Diagnostic> {
    compile_file_artifacts_with_assets(path, wasm_file_name, &BTreeMap::new())
}

fn compile_file_artifacts_with_assets(
    path: &Path,
    wasm_file_name: &str,
    assets: &BTreeMap<String, waluau_codegen_wasm::GeneratedAsset>,
) -> Result<CompileArtifacts, Diagnostic> {
    compile_file_artifacts_with_assets_collect(path, wasm_file_name, assets)
        .map_err(|mut errors| errors.remove(0))
}

/// Like [`compile_file_artifacts`], but reports every diagnostic the pipeline
/// can attribute independently: parse errors are collected across the whole
/// module graph, and when parsing is clean the type checker reports each
/// failing function/statement. Parse errors abort before type checking, since
/// type errors derived from a partial AST would be misleading.
pub fn compile_file_artifacts_collect(
    path: &Path,
    wasm_file_name: &str,
) -> Result<CompileArtifacts, Vec<Diagnostic>> {
    compile_file_artifacts_with_assets_collect(path, wasm_file_name, &BTreeMap::new())
}

fn compile_file_artifacts_with_assets_collect(
    path: &Path,
    wasm_file_name: &str,
    assets: &BTreeMap<String, waluau_codegen_wasm::GeneratedAsset>,
) -> Result<CompileArtifacts, Vec<Diagnostic>> {
    let (program, parse_diagnostics) =
        link::link_program_collect(path).map_err(|error| vec![error])?;
    if !parse_diagnostics.is_empty() {
        return Err(parse_diagnostics);
    }
    compile_program(program, wasm_file_name, assets)
}

fn compile_program(
    program: waluau_ast::Program,
    wasm_file_name: &str,
    assets: &BTreeMap<String, waluau_codegen_wasm::GeneratedAsset>,
) -> Result<CompileArtifacts, Vec<Diagnostic>> {
    let mut typed_program = waluau_hir::type_check_and_infer_collect(&program).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| resolve_diagnostic_source(error, &program))
            .collect::<Vec<_>>()
    })?;
    waluau_ast::resolve_symbols(&mut typed_program).map_err(|error| vec![error])?;
    let ir = waluau_ir::build(&typed_program).map_err(|error| vec![error])?;
    let emitted = waluau_codegen_wasm::emit(&ir).map_err(|error| vec![error])?;
    let js = waluau_codegen_wasm::generate_js_glue_with_assets(wasm_file_name, &emitted, assets);
    Ok(CompileArtifacts {
        wasm: emitted.wasm,
        js,
        required_imports: emitted.required_imports,
        bytes_constants: emitted.bytes_constants,
    })
}

fn resolve_diagnostic_source(error: Diagnostic, program: &waluau_ast::Program) -> Diagnostic {
    let file_path = error
        .file_path()
        .unwrap_or(&program.entry_file_path)
        .to_string();
    match program.sources.get(&file_path) {
        Some(source) => error.with_source(file_path, source),
        None => error.with_file_path_if_missing(file_path),
    }
}

pub fn run() -> Result<(), Vec<Diagnostic>> {
    run_with_args(std::env::args_os().skip(1))
}

pub fn run_with_args<I>(args: I) -> Result<(), Vec<Diagnostic>>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_args(args).map_err(|error| vec![error])?;
    if options.manifest.is_some() && !options.emit_js {
        return Err(vec![Diagnostic::new("--manifest requires --emit-js")]);
    }
    let asset_package = options
        .manifest
        .as_deref()
        .map(prepare_asset_package)
        .transpose()
        .map_err(|error| vec![error])?;
    let wasm_file_name = options
        .output
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| vec![Diagnostic::new("output Wasm path must have a UTF-8 file name")])?;
    let assets = asset_package
        .as_ref()
        .map(|package| &package.generated)
        .unwrap_or_else(|| empty_asset_manifest());
    let artifacts =
        compile_file_artifacts_with_assets_collect(&options.input, wasm_file_name, assets)?;
    fs::write(&options.output, artifacts.wasm)
        .map_err(|error| vec![io_error("write output file", &options.output, error)])?;
    if options.emit_js {
        let js_output = options.output.with_extension("js");
        fs::write(&js_output, artifacts.js)
            .map_err(|error| vec![io_error("write JavaScript glue", &js_output, error)])?;
    }
    if let Some(package) = asset_package {
        let output_dir = options.output.parent().unwrap_or_else(|| Path::new("."));
        package.write_to(output_dir).map_err(|error| vec![error])?;
    }
    Ok(())
}

fn empty_asset_manifest() -> &'static BTreeMap<String, waluau_codegen_wasm::GeneratedAsset> {
    static EMPTY: std::sync::OnceLock<BTreeMap<String, waluau_codegen_wasm::GeneratedAsset>> =
        std::sync::OnceLock::new();
    EMPTY.get_or_init(BTreeMap::new)
}

#[derive(Debug, Eq, PartialEq)]
struct CliOptions {
    input: PathBuf,
    output: PathBuf,
    emit_js: bool,
    manifest: Option<PathBuf>,
}

#[derive(Clone, Copy)]
enum PendingPath {
    Output,
    Manifest,
}

fn parse_args<I>(args: I) -> Result<CliOptions, Diagnostic>
where
    I: IntoIterator<Item = OsString>,
{
    let mut input = None;
    let mut output = None;
    let mut manifest = None;
    let mut pending_path = None;
    let mut emit_js = false;

    for arg in args {
        if let Some(pending) = pending_path.take() {
            match pending {
                PendingPath::Output => output = Some(PathBuf::from(arg)),
                PendingPath::Manifest => manifest = Some(PathBuf::from(arg)),
            }
            continue;
        }

        match arg.to_str() {
            Some("-o" | "--output") => pending_path = Some(PendingPath::Output),
            Some("--manifest") => pending_path = Some(PendingPath::Manifest),
            Some("--emit-js") => emit_js = true,
            Some(flag) if flag.starts_with('-') => {
                return Err(Diagnostic::new(format!(
                    "unsupported flag `{flag}`\nusage: waluau <input.walu> [-o <output.wasm>] [--emit-js] [--manifest <waluau.assets.json>]"
                )));
            }
            _ if input.is_none() => input = Some(PathBuf::from(arg)),
            _ => {
                return Err(Diagnostic::new(
                    "too many positional arguments\nusage: waluau <input.walu> [-o <output.wasm>] [--emit-js] [--manifest <waluau.assets.json>]",
                ));
            }
        }
    }

    if let Some(pending) = pending_path {
        let flag = match pending {
            PendingPath::Output => "-o/--output",
            PendingPath::Manifest => "--manifest",
        };
        return Err(Diagnostic::new(format!(
            "missing path after {flag}\nusage: waluau <input.walu> [-o <output.wasm>] [--emit-js] [--manifest <waluau.assets.json>]"
        )));
    }

    let input = input.ok_or_else(|| {
        Diagnostic::new(
            "missing input path\nusage: waluau <input.walu> [-o <output.wasm>] [--emit-js] [--manifest <waluau.assets.json>]",
        )
    })?;
    let output = output.unwrap_or_else(|| default_output_path(&input));

    Ok(CliOptions {
        input,
        output,
        emit_js,
        manifest,
    })
}

fn default_output_path(input: &Path) -> PathBuf {
    input.with_extension("wasm")
}

fn add_builtins_to_program(program: &mut waluau_ast::Program) -> Result<(), Diagnostic> {
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectAssetManifest {
    version: u32,
    assets: Vec<AssetDeclaration>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssetDeclaration {
    path: String,
    #[serde(rename = "type")]
    kind: AssetKind,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum AssetKind {
    Text,
    Bytes,
    Image,
    Font,
    Audio,
}

impl AssetKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Bytes => "bytes",
            Self::Image => "image",
            Self::Font => "font",
            Self::Audio => "audio",
        }
    }
}

#[derive(Debug)]
struct PreparedAsset {
    output_relative: PathBuf,
    bytes: Vec<u8>,
}

#[derive(Debug)]
struct PreparedAssetPackage {
    generated: BTreeMap<String, waluau_codegen_wasm::GeneratedAsset>,
    files: Vec<PreparedAsset>,
}

impl PreparedAssetPackage {
    fn write_to(self, output_dir: &Path) -> Result<(), Diagnostic> {
        for asset in self.files {
            let output = output_dir.join(&asset.output_relative);
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| io_error("create asset output directory", parent, error))?;
            }
            fs::write(&output, asset.bytes)
                .map_err(|error| io_error("write packaged asset", &output, error))?;
        }
        Ok(())
    }
}

fn prepare_asset_package(path: &Path) -> Result<PreparedAssetPackage, Diagnostic> {
    let source =
        fs::read_to_string(path).map_err(|error| io_error("read asset manifest", path, error))?;
    let manifest: ProjectAssetManifest = serde_json::from_str(&source).map_err(|error| {
        Diagnostic::new(format!(
            "invalid asset manifest `{}`: {error}",
            path.display()
        ))
    })?;
    if manifest.version != 1 {
        return Err(Diagnostic::new(format!(
            "unsupported asset manifest version {}; expected 1",
            manifest.version
        )));
    }
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    let mut logical_paths = BTreeSet::new();
    let mut output_paths = BTreeSet::new();
    let mut generated = BTreeMap::new();
    let mut files = Vec::new();
    for declaration in manifest.assets {
        validate_logical_asset_path(&declaration.path)?;
        if !logical_paths.insert(declaration.path.clone()) {
            return Err(Diagnostic::new(format!(
                "duplicate asset manifest entry `{}`",
                declaration.path
            )));
        }
        let bytes = fs::read(root.join(&declaration.path)).map_err(|error| {
            io_error("read declared asset", &root.join(&declaration.path), error)
        })?;
        let output_relative = fingerprinted_asset_path(&declaration.path, &bytes)?;
        if !output_paths.insert(output_relative.clone()) {
            return Err(Diagnostic::new(format!(
                "asset output collision at `{}`",
                output_relative.display()
            )));
        }
        let url = format!("./{}", output_relative.to_string_lossy().replace('\\', "/"));
        generated.insert(
            declaration.path,
            waluau_codegen_wasm::GeneratedAsset {
                url,
                kind: declaration.kind.as_str().to_string(),
            },
        );
        files.push(PreparedAsset {
            output_relative,
            bytes,
        });
    }
    Ok(PreparedAssetPackage { generated, files })
}

fn validate_logical_asset_path(path: &str) -> Result<(), Diagnostic> {
    let lower = path.to_ascii_lowercase();
    let invalid = path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains('?')
        || path.contains('#')
        || path.contains(':')
        || lower.contains("%2e")
        || lower.contains("%2f")
        || lower.contains("%5c")
        || path
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..");
    if invalid {
        return Err(Diagnostic::new(format!(
            "invalid logical asset path `{path}`"
        )));
    }
    Ok(())
}

fn fingerprinted_asset_path(logical: &str, bytes: &[u8]) -> Result<PathBuf, Diagnostic> {
    let path = Path::new(logical);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Diagnostic::new(format!("asset path `{logical}` has no UTF-8 file name")))?;
    let (stem, extension) = match file_name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && !extension.is_empty() => {
            (stem, Some(extension))
        }
        _ => (file_name, None),
    };
    let hash = bytes.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    let fingerprinted = match extension {
        Some(extension) => format!("{stem}.{hash:016x}.{extension}"),
        None => format!("{stem}.{hash:016x}"),
    };
    Ok(path.with_file_name(fingerprinted))
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

    fn app_path(relative: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps")
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
    fn compiles_implicit_top_level_declaration_captured_by_callback() {
        super::compile_source(
            r#"
                t = { value = 0::i32 }
                local callback: () -> unit = function(): unit
                    t.value = 41::i32
                end
                callback()
                assert(t.value == 41::i32)
            "#,
        )
        .expect("implicit top-level declaration should compile and be captured");
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
        let wat = wasmprinter::print_bytes(&wasm).expect("wasm should print");
        assert!(
            wat.contains(r#"(import "waluau" "dom_window""#),
            "value-returning require should call the dom_window host import:\n{wat}"
        );
    }

    #[test]
    fn compile_file_accepts_bare_dom_window_as_extern_dependency() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("app.walu");
        fs::write(
            &input_path,
            r#"
                require("dom:window")

                function create_div(document: Document): Element
                    return document:create_element("div")
                end
            "#,
        )
        .expect("app should write");

        let wasm =
            super::compile_file(&input_path).expect("bare DOM dependency require should compile");
        let wat = wasmprinter::print_bytes(&wasm).expect("wasm should print");
        assert!(
            !wat.contains(r#"(import "waluau" "dom_window""#),
            "extern-only dependency should not load the window value:\n{wat}"
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
    fn compile_file_resolves_vitest_virtual_module_as_bare_globals() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("math.test.walu");
        fs::write(
            &input_path,
            r#"
                require("waluau:vitest")

                function add(a: i32, b: i32): i32
                    return a + b
                end

                describe("add", function(): unit
                    it("adds", function(): unit
                        expect(add(2, 2)):toBe(4)
                        expect(add(1, 1) == 2):toBeTruthy()
                        expect("walu"):toContain("alu")
                    end)
                end)
            "#,
        )
        .expect("test file should write");

        let wasm = super::compile_file(&input_path).expect("vitest require should compile");
        let wat = wasmprinter::print_bytes(&wasm).expect("wasm should print");
        assert!(
            wat.contains(r#"(import "waluau" "describe""#),
            "describe should import as a host function:\n{wat}"
        );
        assert!(
            wat.contains(r#"(import "waluau" "NumberExpectation.toBe""#),
            "matcher methods should import under their extern type names:\n{wat}"
        );
        assert!(
            wat.contains(&format!("(export \"{}\"", "__waluau_call_callback_unit")),
            "test bodies need the () -> unit callback trampoline:\n{wat}"
        );
    }

    #[test]
    fn compile_file_resolves_vitest_namespace_binding() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("suite.test.walu");
        fs::write(
            &input_path,
            r#"
                local t = require("waluau:vitest")

                t.it("works through the namespace", function(): unit
                    t.expect(21 * 2):toBe(42)
                end)
            "#,
        )
        .expect("test file should write");

        let wasm =
            super::compile_file(&input_path).expect("vitest namespace require should compile");
        let wat = wasmprinter::print_bytes(&wasm).expect("wasm should print");
        assert!(
            wat.contains(r#"(import "waluau" "it""#),
            "namespace members should resolve to the declared host imports:\n{wat}"
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
                    return haystack:find(needle) ~= nil
                end

                function install(): unit
                    local window = require("dom:window")
                    local document: Document = window.document
                    local input_element: Element = document:create_element("input")
                    local seed_text: string = "typed card"

                    input_element:add_event_listener("input", function(event: Event): unit
                        if HTMLInputElement(target) = event.target then
                            local value: string = target.value
                            local matches_seed: bool = value == "" or seed_text:find(value) ~= nil
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
    fn math_abs_on_integers_widens_to_f64() {
        // math.abs is declared for f32/f64 only (builtins/math.walu); an i32
        // argument follows the language's implicit i32 -> f64 widening, so
        // the f64 overload is selected and the result is f64, not i32.
        let err = super::compile_source(
            r#"
            function bad(x: i32): i32
                return math.abs(x)
            end
        "#,
        )
        .expect_err("compile should fail");
        assert!(
            err.to_string()
                .contains("cannot implicitly convert f64 to i32"),
            "expected f64 result diagnostic, got: {err}"
        );

        super::compile_source(
            r#"
            function ok(x: i32): f64
                return math.abs(x)
            end
        "#,
        )
        .expect("i32 argument should widen to the f64 overload");
    }

    #[test]
    fn compiles_math_builtins_as_host_import_overloads() {
        // math.* builtins are extern declarations (builtins/math.walu):
        // overload selection picks the f32/f64 variant from the argument
        // types and each selected variant becomes its own host import under
        // the shared dotted host name.
        let source = r#"
            function wide(x: f64): f64
                return math.sqrt(math.abs(x)) + math.min(x, 2.0)
            end
            function narrow(x: f32): f32
                return math.floor(math.copysign(x, x))
            end
        "#;
        let wasm = super::compile_source(source).expect("compile should succeed");
        let wat = wasmprinter::print_bytes(&wasm).expect("wat should print");
        for import in [
            "math.sqrt",
            "math.abs",
            "math.min",
            "math.floor",
            "math.copysign",
        ] {
            assert!(
                wat.contains(&format!("(import \"waluau\" \"{import}\"")),
                "expected a {import} host import:\n{wat}"
            );
        }
        for unused_import in ["math.acos", "math.atan2", "math.log10", "print"] {
            assert!(
                !wat.contains(&format!("(import \"waluau\" \"{unused_import}\"")),
                "unused builtin {unused_import} should not be imported:\n{wat}"
            );
        }
        assert!(
            !wat.contains("f64.sqrt") && !wat.contains("f32.sqrt"),
            "math.sqrt must lower to a host call, not a wasm intrinsic:\n{wat}"
        );
    }

    #[test]
    fn compiles_math_random_and_randomseed() {
        let source = r#"
            function roll(): f64
                math.randomseed(42.0)
                local unit_sample: f64 = math.random()
                local die: i32 = math.random(6)
                local ranged: i32 = math.random(3, 9)
                return unit_sample + die::f64 + ranged::f64
            end
        "#;
        let wasm = super::compile_source(source).expect("compile should succeed");
        let wat = wasmprinter::print_bytes(&wasm).expect("wat should print");
        assert!(
            wat.contains("(import \"waluau\" \"math.randomseed\""),
            "expected a math.randomseed host import:\n{wat}"
        );
        // All three math.random overloads share the host name but are
        // imported separately (arities 0, 1, and 2).
        assert_eq!(
            wat.matches("(import \"waluau\" \"math.random\" ").count(),
            3,
            "expected three math.random overload imports:\n{wat}"
        );
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
    fn compiles_string_split_function_and_method_forms() {
        // string.split lowers to the string_split/string_split_get host
        // imports plus a compiler-emitted loop that fills a growable
        // {string} array; the separator defaults to ",".
        let source = r#"
            local parts: {string} = string.split("a,b,c", ",")
            assert(#parts == 3)
            assert(parts[0] == "a")

            local defaulted = string.split("a,b")
            assert(#defaulted == 2)

            local chars = ("abc"):split("")
            assert(#chars == 3)
        "#;
        let wasm = super::compile_source(source).expect("compile should succeed");
        let wat = wasmprinter::print_bytes(&wasm).expect("wat should print");
        assert!(
            wat.contains("(import \"waluau\" \"string_split\" "),
            "expected a string_split host import:\n{wat}"
        );
        assert!(
            wat.contains("(import \"waluau\" \"string_split_get\" "),
            "expected a string_split_get host import:\n{wat}"
        );
    }

    #[test]
    fn rejects_invalid_fixture_file() {
        let source = fixture_source("mismatch");
        let err = super::compile_source(source).expect_err("compile should fail");
        assert_eq!(err.to_string(), "return expects f64, got bool");
    }

    #[test]
    fn cli_reports_type_errors_from_multiple_functions() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("multi.walu");
        let source = concat!(
            "function first(x: i32): bool\n",
            "    return x\n",
            "end\n",
            "function second(x: i32): i32\n",
            "    if x then\n",
            "        return x\n",
            "    end\n",
            "    return x\n",
            "end\n",
        );
        fs::write(&input_path, source).expect("fixture should write");

        let errors = super::run_with_args([os(&input_path)]).expect_err("cli run should fail");
        assert_eq!(errors.len(), 2, "one error per function: {errors:?}");
    }

    #[test]
    fn cli_collects_parse_errors_across_required_modules() {
        let tempdir = tempdir().expect("tempdir should exist");
        let lib_path = tempdir.path().join("lib.walu");
        let entry_path = tempdir.path().join("entry.walu");
        // Both modules have a syntax error; both must be reported.
        fs::write(
            &lib_path,
            "function broken_lib(): i32\n    return 1 +\nend\nreturn broken_lib\n",
        )
        .expect("fixture should write");
        fs::write(
            &entry_path,
            "local lib = require(\"./lib\")\nlocal x: i32 =\n",
        )
        .expect("fixture should write");

        let errors = super::run_with_args([os(&entry_path)]).expect_err("cli run should fail");
        assert_eq!(errors.len(), 2, "one parse error per module: {errors:?}");
        let rendered: Vec<String> = errors.iter().map(|error| error.render()).collect();
        assert!(
            rendered.iter().any(|line| line.contains("entry.walu")),
            "entry module error missing: {rendered:?}"
        );
        assert!(
            rendered.iter().any(|line| line.contains("lib.walu")),
            "lib module error missing: {rendered:?}"
        );
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
    fn cli_optionally_writes_sibling_javascript_glue() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("add.walu");
        let wasm_path = tempdir.path().join("game.wasm");
        let js_path = tempdir.path().join("game.js");
        fs::write(&input_path, fixture_source("add")).expect("fixture should write");

        super::run_with_args([
            os(&input_path),
            OsString::from("--output"),
            os(&wasm_path),
            OsString::from("--emit-js"),
        ])
        .expect("CLI run should succeed");

        assert!(wasm_path.exists(), "Wasm sibling should exist");
        let js = fs::read_to_string(&js_path).expect("JavaScript sibling should exist");
        assert!(js.contains("new URL(\"./game.wasm\", import.meta.url)"));
        assert!(js.contains("export async function instantiate"));
        assert!(!js.contains("WebAssembly.Module.imports"));
    }

    #[test]
    fn cli_packages_typed_asset_manifest_with_fingerprinted_outputs() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("main.walu");
        let manifest_path = tempdir.path().join("waluau.assets.json");
        let dist = tempdir.path().join("dist");
        fs::create_dir_all(tempdir.path().join("assets")).expect("asset dir should exist");
        fs::create_dir_all(&dist).expect("dist should exist");
        fs::write(&input_path, fixture_source("add")).expect("fixture should write");
        let declarations = [
            ("story.txt", "text", b"story".as_slice()),
            ("data.bin", "bytes", b"bytes".as_slice()),
            ("sprite.png", "image", b"image".as_slice()),
            ("typeface.woff2", "font", b"font".as_slice()),
            ("theme.ogg", "audio", b"audio".as_slice()),
        ];
        for (name, _, bytes) in declarations {
            fs::write(tempdir.path().join("assets").join(name), bytes).expect("asset should write");
        }
        fs::write(
            &manifest_path,
            r#"{
                "version": 1,
                "assets": [
                    {"path":"assets/story.txt","type":"text"},
                    {"path":"assets/data.bin","type":"bytes"},
                    {"path":"assets/sprite.png","type":"image"},
                    {"path":"assets/typeface.woff2","type":"font"},
                    {"path":"assets/theme.ogg","type":"audio"}
                ]
            }"#,
        )
        .expect("manifest should write");

        super::run_with_args([
            os(&input_path),
            OsString::from("--output"),
            os(dist.join("game.wasm")),
            OsString::from("--emit-js"),
            OsString::from("--manifest"),
            os(&manifest_path),
        ])
        .expect("packaged CLI run should succeed");

        let js = fs::read_to_string(dist.join("game.js")).expect("glue should exist");
        for (name, kind, bytes) in declarations {
            let logical = format!("assets/{name}");
            let emitted = super::fingerprinted_asset_path(&logical, bytes)
                .expect("fingerprinted path should build");
            assert!(
                dist.join(&emitted).exists(),
                "{} should be copied",
                emitted.display()
            );
            assert!(js.contains(&format!("\"{logical}\"")));
            assert!(js.contains(&format!("type: \"{kind}\"")));
            assert!(js.contains(&emitted.to_string_lossy().replace('\\', "/")));
        }
        assert!(js.contains("export const assetBaseUrl"));
        assert!(js.contains("export const assetManifest"));
    }

    #[test]
    fn asset_manifest_diagnoses_missing_duplicate_and_unknown_types() {
        let tempdir = tempdir().expect("tempdir should exist");
        let missing = tempdir.path().join("missing.json");
        fs::write(
            &missing,
            r#"{"version":1,"assets":[{"path":"assets/nope.txt","type":"text"}]}"#,
        )
        .expect("manifest should write");
        let error = super::prepare_asset_package(&missing).expect_err("missing asset should fail");
        assert!(error.to_string().contains("assets/nope.txt"));

        fs::write(tempdir.path().join("same.txt"), "same").expect("asset should write");
        let duplicate = tempdir.path().join("duplicate.json");
        fs::write(
            &duplicate,
            r#"{"version":1,"assets":[{"path":"same.txt","type":"text"},{"path":"same.txt","type":"bytes"}]}"#,
        )
        .expect("manifest should write");
        let error = super::prepare_asset_package(&duplicate).expect_err("duplicate should fail");
        assert!(error.to_string().contains("duplicate asset manifest entry"));

        let unknown = tempdir.path().join("unknown.json");
        fs::write(
            &unknown,
            r#"{"version":1,"assets":[{"path":"same.txt","type":"shader"}]}"#,
        )
        .expect("manifest should write");
        let error = super::prepare_asset_package(&unknown).expect_err("unknown type should fail");
        assert!(error.to_string().contains("unknown variant `shader`"));

        for invalid_path in ["/absolute.txt", "../escape.txt", "assets/%2e%2e/escape.txt"] {
            let invalid = tempdir.path().join("invalid.json");
            fs::write(
                &invalid,
                format!(r#"{{"version":1,"assets":[{{"path":"{invalid_path}","type":"text"}}]}}"#),
            )
            .expect("manifest should write");
            let error =
                super::prepare_asset_package(&invalid).expect_err("invalid path should fail");
            assert!(error.to_string().contains("invalid logical asset path"));
        }
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

        assert_eq!(error.len(), 1);
        assert_eq!(error[0].to_string(), "return expects f64, got bool");
        assert!(
            !output_path.exists(),
            "failed compilation must not write output"
        );
    }

    #[test]
    fn cli_renders_inference_diagnostic_file_line_and_column() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("span-mismatch.walu");
        let output_path = tempdir.path().join("span-mismatch.wasm");
        let source = concat!(
            "declare function accept(value: i32): unit\n",
            "\n",
            "function bad(): unit\n",
            "    accept(\"wrong\")\n",
            "end\n",
        );
        fs::write(&input_path, source).expect("fixture should write");

        let error = super::run_with_args([
            os(&input_path),
            OsString::from("--output"),
            os(&output_path),
        ])
        .expect_err("cli run should fail");

        assert_eq!(error.len(), 1);
        assert_eq!(
            error[0].render(),
            format!(
                "{}:4:12: cannot implicitly convert string to i32",
                input_path
                    .canonicalize()
                    .expect("fixture path should canonicalize")
                    .display()
            )
        );
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
    fn reports_non_string_require_argument_at_its_source_location() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("main.walu");
        let source = "local lib = require(module_name)\n";
        fs::write(&input_path, source).expect("main should write");
        let canonical_path = input_path
            .canonicalize()
            .expect("main path should canonicalize");
        let column = source.find("module_name").expect("argument should exist") + 1;

        let error = super::compile_file(&input_path).expect_err("non-string require should fail");
        assert_eq!(error.code(), Some("module/require-literal-path"));
        assert_eq!(
            error.render(),
            format!(
                "{}:1:{column}: require expects a string literal path, e.g. \
                 require(\"./module\")",
                canonical_path.display()
            )
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
    fn compile_file_rewrites_module_aliases_in_declared_import_params_and_returns() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            tempdir.path().join("service.walu"),
            r#"
                type Promise<T> = extern

                declare function host_exchange(value: Promise<i32>): Promise<string>

                function exchange(value: Promise<i32>): Promise<string>
                    return host_exchange(value)
                end

                return { exchange = exchange }
            "#,
        )
        .expect("service module should write");
        fs::write(
            &input_path,
            r#"
                local service = require("./service")

                function exchange(value: service.Promise<i32>): service.Promise<string>
                    return service.exchange(value)
                end
            "#,
        )
        .expect("entry module should write");

        let artifacts = super::compile_file_artifacts(&input_path, "game.wasm")
            .expect("module-local aliases in host import signatures should compile");
        assert!(artifacts.js.contains("host_exchange"));
    }

    #[test]
    fn compiles_2d_game_engine_browser_fixture() {
        super::compile_file(&fixture_path("game-engine/main.walu"))
            .expect("browser game engine fixture should compile");
    }

    #[test]
    fn compiles_game_engine_text_alignment_fixture() {
        super::compile_file(&fixture_path("game-engine/text-alignment.walu"))
            .expect("text alignment fixture should compile");
    }

    #[test]
    fn compiles_game_engine_graphics_paths_fixture() {
        super::compile_file(&fixture_path("game-engine/graphics-paths.walu"))
            .expect("graphics paths fixture should compile");
    }

    #[test]
    fn compiles_game_engine_gpu_shaders_fixture() {
        super::compile_file(&fixture_path("game-engine/gpu-shaders.walu"))
            .expect("GPU shaders fixture should compile");
    }

    #[test]
    fn compiles_game_engine_gpu_resources_fixture() {
        super::compile_file(&fixture_path("game-engine/gpu-resources.walu"))
            .expect("GPU resources fixture should compile");
    }

    #[test]
    fn compiles_game_engine_gpu_font_resources_fixture() {
        super::compile_file(&fixture_path("game-engine/gpu-font-resources.walu"))
            .expect("GPU font resources fixture should compile");
    }

    #[test]
    fn compiles_2d_game_engine_headless_simulation() {
        super::compile_file(&fixture_path("game-engine/sim.walu"))
            .expect("headless game engine simulation should compile");
    }

    #[test]
    fn compiles_2d_game_engine_resource_services_contract() {
        super::compile_file(&fixture_path("game-engine/resources.walu"))
            .expect("resource, audio and save-data contract should compile");
    }

    #[test]
    fn compiles_transitive_await_state_fixture() {
        super::compile_file(&fixture_path("coroutine-await-state/main.walu"))
            .expect("transitive await state fixture should compile");
    }

    #[test]
    fn compiles_snake_game_engine_fixture() {
        super::compile_file(&fixture_path("snake/main.walu"))
            .expect("Snake game engine fixture should compile");
    }

    #[test]
    fn compiles_arcane_heist_game_engine_fixture() {
        super::compile_file(&app_path("ante/src/main.walu"))
            .expect("Arcane Heist game engine fixture should compile");
    }

    #[test]
    fn compiles_stable_engine_package_from_outside_repository() {
        let project = tempdir().expect("temp project should exist");
        let entry = project.path().join("main.walu");
        fs::write(
            &entry,
            include_str!("../../../examples/game-project/main.walu"),
        )
        .expect("external project entry should write");

        super::compile_file(&entry)
            .expect("embedded engine package and its re-exported callback types should compile");
    }

    #[test]
    fn compiles_pinned_engine_subsystem_package() {
        let project = tempdir().expect("temp project should exist");
        let entry = project.path().join("main.walu");
        fs::write(
            &entry,
            r#"
                local input_module = require("waluau:engine/v1/input")

                function consume(input: input_module.Input): bool
                    return input:is_down("Space")
                end

                local input: input_module.Input = input_module.new()
                assert(not consume(input))
            "#,
        )
        .expect("external subsystem project should write");

        super::compile_file(&entry).expect("versioned subsystem import should compile");
    }

    #[test]
    fn compiles_pinned_engine_audio_subsystem_package() {
        let project = tempdir().expect("temp project should exist");
        let entry = project.path().join("main.walu");
        fs::write(
            &entry,
            r#"
                local audio = require("waluau:engine/v1/audio")

                function unlock_audio(): bool
                    return audio.unlock()
                end
            "#,
        )
        .expect("external audio subsystem project should write");

        super::compile_file(&entry).expect("versioned audio subsystem import should compile");
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
    fn compile_file_imports_type_aliases_across_modules() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("state.walu"),
            r#"
                type State = { score: i32 }

                function new_state(): State
                    return { score = 41::i32 }
                end

                return {
                    new_state = new_state,
                }
            "#,
        )
        .expect("state module should write");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                local state_mod = require("./state")

                type State = state_mod.State

                function bump(state: State): i32
                    state.score += 1
                    return state.score
                end

                function direct(state: state_mod.State): i32
                    return state.score
                end

                local state: state_mod.State = state_mod.new_state()
                assert(bump(state) == 42)
                assert(direct(state) == 42)
            "#,
        )
        .expect("main module should write");

        let wasm = super::compile_file(&input_path).expect("compile should succeed");
        assert!(!wasm.is_empty());
    }

    #[test]
    fn compile_file_dispatches_methods_and_statics_across_modules() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("counter.walu"),
            r#"
                type Counter = { value: i32 }

                function Counter.new(start: i32): Counter
                    local counter: Counter = { value = start }
                    counter:clamp()
                    return counter
                end

                function Counter:bump(amount: i32): unit
                    self.value += amount
                    self:clamp()
                end

                function Counter:clamp(): unit
                    if self.value > 100 then
                        self.value = 100
                    end
                end

                return { new = Counter.new }
            "#,
        )
        .expect("counter module should write");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                local counter = require("./counter")

                local c = counter.new(5)
                c:bump(10)
                assert(c.value == 15)
                c:bump(1000)
                assert(c.value == 100)

                -- A consumer-side structurally identical alias still
                -- dispatches the defining module's methods.
                type Counter = { value: i32 }
                local aliased: Counter = counter.new(7)
                aliased:bump(1)
                assert(aliased.value == 8)
            "#,
        )
        .expect("main module should write");

        let wasm = super::compile_file(&input_path).expect("compile should succeed");
        assert!(!wasm.is_empty());
    }

    #[test]
    fn compile_file_reports_unknown_imported_type_alias() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("state.walu"),
            r#"
                type State = { score: i32 }

                function new_state(): State
                    return { score = 41::i32 }
                end

                return {
                    new_state = new_state,
                }
            "#,
        )
        .expect("state module should write");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                local state_mod = require("./state")

                local state: state_mod.Missing = state_mod.new_state()
            "#,
        )
        .expect("main module should write");

        let error = super::compile_file(&input_path).expect_err("compile should fail");
        assert!(error.to_string().contains("state_mod.Missing"));
    }

    #[test]
    fn compile_file_unifies_record_type_aliases_across_modules() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("state.walu"),
            r#"
                type State = { score: i32 }

                function new_state(): State
                    return { score = 41::i32 }
                end

                return {
                    new_state = new_state,
                }
            "#,
        )
        .expect("state module should write");
        fs::write(
            tempdir.path().join("consumer.walu"),
            r#"
                type State = { score: i32 }

                function score(state: State): i32
                    return state.score
                end

                return {
                    score = score,
                }
            "#,
        )
        .expect("consumer module should write");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                local state_mod = require("./state")
                local consumer = require("./consumer")

                function entry(): i32
                    local state = state_mod.new_state()
                    return consumer.score(state)
                end
            "#,
        )
        .expect("main module should write");

        let wasm = super::compile_file(&input_path).expect("compile should succeed");
        assert!(
            !wasm.is_empty(),
            "successful compilation should produce a wasm module"
        );
    }

    #[test]
    fn compile_file_exports_module_constants() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("config.walu"),
            r#"
                local CELL_SIZE <const>: f64 = 16.0
                local TITLE <const> = "snake"
                local NEGATIVE_INDEX <const>: i32 = -1
                local NEGATIVE_SCALE <const>: f64 = -1.5

                function cell_px(v: i32): f64
                    return v::f64 * CELL_SIZE
                end

                function negative_index(): i32
                    return NEGATIVE_INDEX
                end

                function negative_scale(): f64
                    return NEGATIVE_SCALE
                end

                return {
                    CELL_SIZE = CELL_SIZE,
                    TITLE = TITLE,
                    cell_px = cell_px,
                    negative_index = negative_index,
                    negative_scale = negative_scale,
                }
            "#,
        )
        .expect("config module should write");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                local config = require("./config")

                assert(config.CELL_SIZE == 16.0)
                assert(config.TITLE == "snake")
                assert(config.cell_px(2) == 32.0)
                assert(config.negative_index() == -1)
                assert(config.negative_scale() == -1.5)

                function in_function(): f64
                    return config.CELL_SIZE + 1.0
                end
                assert(in_function() == 17.0)
            "#,
        )
        .expect("main module should write");

        let wasm = super::compile_file(&input_path).expect("compile should succeed");
        assert!(!wasm.is_empty());
    }

    #[test]
    fn compile_file_rejects_non_literal_module_constant_at_declaration() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("config.walu"),
            r#"
                function first(): i32
                    return 16
                end

                local SIZE <const>: i32 = first()

                function read_size(): i32
                    return SIZE
                end

                function unrelated(): f64
                    return 16.0
                end

                return {
                    read_size = read_size,
                    unrelated = unrelated,
                }
            "#,
        )
        .expect("config module should write");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                local config = require("./config")
                assert(config.unrelated() == 16.0)
            "#,
        )
        .expect("main module should write");

        // Reject the declaration itself rather than allowing later references
        // to fail as unknown names.
        let error = super::compile_file(&input_path).expect_err("compile should fail");
        assert_eq!(
            error.to_string(),
            "top-level const 'SIZE' initializer must be an inlinable literal"
        );
        assert!(
            error.span().is_some(),
            "diagnostic should point at the initializer"
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

    #[test]
    fn compiles_loop_nested_in_if_branch_inside_loop() {
        let source = r#"
            function count(width: i32): i32
                local column: i32 = 0
                while column < width do
                    if column % 2 == 0 then
                        local run: i32 = 1
                        while column + run < width do
                            run += 1
                        end
                        column += run
                    else
                        column += 1
                    end
                end
                return column
            end

            assert(count(5) == 5)
        "#;

        super::compile_source(source).expect("nested branch and loop phis should compile");
    }

    #[test]
    fn compiles_conditional_aggregate_reassignment_before_card_loop() {
        let source = r#"
            type Card = { rank: i32 }

            function render(revealed: bool, cards: {Card}): i32
                local shown: {Card} = {}
                local total: i32 = 0
                if revealed then
                    shown = cards
                    for index = 0::i32, #shown - 1 do
                        if shown[index].rank == 0 then
                            continue
                        end
                        total += shown[index].rank
                    end
                end
                return total + #shown
            end
        "#;

        super::compile_source(source)
            .expect("conditional aggregate and card-loop phis should compile");
    }

    #[test]
    fn compiles_branch_initialized_bool_updated_in_numeric_loop() {
        let source = r#"
            function resolves_tie(pair_rank: i32, winning_rank: i32, kickers: {i32}): bool
                local wins: bool = pair_rank > winning_rank
                if pair_rank == winning_rank then
                    for index = 0::i32, #kickers - 1 do
                        if kickers[index] ~= winning_rank then
                            wins = kickers[index] > winning_rank
                            break
                        end
                    end
                else
                    wins = pair_rank > winning_rank
                end
                return wins
            end
        "#;

        super::compile_source(source).expect("branch-initialized loop-carried bool should compile");
    }
}
