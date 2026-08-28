use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use waluau_diagnostics::Diagnostic;

struct CompilerTimer {
    #[cfg(not(target_family = "wasm"))]
    started: std::time::Instant,
}

impl CompilerTimer {
    fn start() -> Self {
        Self {
            #[cfg(not(target_family = "wasm"))]
            started: std::time::Instant::now(),
        }
    }

    fn elapsed(&self) -> std::time::Duration {
        #[cfg(not(target_family = "wasm"))]
        return self.started.elapsed();
        #[cfg(target_family = "wasm")]
        return std::time::Duration::ZERO;
    }

    fn enabled() -> bool {
        #[cfg(not(target_family = "wasm"))]
        return std::env::var_os("WALUAU_TIMINGS").is_some();
        #[cfg(target_family = "wasm")]
        return false;
    }
}

mod fmt;
mod link;
pub mod session;

pub use link::{LinkOutcome, ModuleProvider};
pub use session::{Analysis, BuildOutcome, CompilerSession};

pub const CLI_HELP: &str = "usage: waluau <input.walu> [-o <output.wasm>] [--emit-js] [--development-dwarf] [--minimal-exports] [--manifest <waluau.assets.json>] [--report <report.json>]\n\n  --development-dwarf  Emit a referenced sibling debug Wasm for browser DevTools\n  --minimal-exports    Export only the runtime entry points so wasm-opt can remove unused code";

/// Explicit controls for compiler output. Defaults preserve the production
/// artifact format exactly.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CompileOptions {
    /// Emit external source mappings for browser DevTools development sessions.
    pub development_dwarf: bool,
    /// Export only the runtime entry points, leaving the playground's
    /// per-function and record-marshalling exports out so a post-link
    /// optimizer can remove unused code.
    pub minimal_exports: bool,
}

/// Compile a single source string with no module resolution.
///
/// Any `require(...)` in the source is rejected, since relative imports can only
/// be resolved against a file path. Use [`compile_file`] for programs that use
/// `require`.
pub fn compile_source(source: &str) -> Result<Vec<u8>, Diagnostic> {
    compile_source_with_options(source, CompileOptions::default())
}

pub fn compile_source_with_options(
    source: &str,
    options: CompileOptions,
) -> Result<Vec<u8>, Diagnostic> {
    if options.development_dwarf {
        return Err(Diagnostic::new(
            "development DWARF produces two artifacts; use compile_source_artifacts_with_options",
        ));
    }
    Ok(compile_source_artifacts_with_options(source, "program.wasm", options)?.wasm)
}

pub fn compile_source_artifacts_with_options(
    source: &str,
    wasm_file_name: &str,
    options: CompileOptions,
) -> Result<CompileArtifacts, Diagnostic> {
    let mut program = waluau_parser::parse(source)?;

    // Add builtin declarations to standalone programs
    add_builtins_to_program(&mut program)?;

    compile_program(program, wasm_file_name, empty_asset_manifest(), options)
        .map_err(|mut errors| errors.remove(0))
}

/// Like [`compile_source`], but reports every independently-attributable
/// diagnostic: all parse errors, or (when parsing is clean) every failing
/// function/statement from the type checker.
pub fn compile_source_collect(source: &str) -> Result<Vec<u8>, Vec<Diagnostic>> {
    let waluau_parser::ParseOutcome {
        mut program,
        diagnostics,
        ..
    } = waluau_parser::parse_with_recovery(source, "source");
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    // Add builtin declarations to standalone programs
    add_builtins_to_program(&mut program).map_err(|error| vec![error])?;

    Ok(compile_program(
        program,
        "program.wasm",
        empty_asset_manifest(),
        CompileOptions::default(),
    )?
    .wasm)
}

/// Compile `path`, resolving and linking any modules it imports with `require`.
pub fn compile_file(path: &Path) -> Result<Vec<u8>, Diagnostic> {
    Ok(compile_file_artifacts(path, "program.wasm")?.wasm)
}

pub fn compile_file_with_options(
    path: &Path,
    options: CompileOptions,
) -> Result<Vec<u8>, Diagnostic> {
    if options.development_dwarf {
        return Err(Diagnostic::new(
            "development DWARF produces two artifacts; use compile_file_artifacts_with_options",
        ));
    }
    Ok(compile_file_artifacts_with_options(path, "program.wasm", options)?.wasm)
}

#[derive(Debug)]
pub struct CompileArtifacts {
    pub wasm: Vec<u8>,
    pub development_dwarf: Option<Vec<u8>>,
    pub development_sources: Vec<waluau_codegen_wasm::DevelopmentSource>,
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
    compile_file_artifacts_with_options(path, wasm_file_name, CompileOptions::default())
}

pub fn compile_file_artifacts_with_options(
    path: &Path,
    wasm_file_name: &str,
    options: CompileOptions,
) -> Result<CompileArtifacts, Diagnostic> {
    compile_file_artifacts_with_assets(path, wasm_file_name, &BTreeMap::new(), options)
}

fn compile_file_artifacts_with_assets(
    path: &Path,
    wasm_file_name: &str,
    assets: &BTreeMap<String, waluau_codegen_wasm::GeneratedAsset>,
    options: CompileOptions,
) -> Result<CompileArtifacts, Diagnostic> {
    compile_file_artifacts_with_assets_collect(path, wasm_file_name, assets, options)
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
    compile_file_artifacts_with_assets_collect(
        path,
        wasm_file_name,
        &BTreeMap::new(),
        CompileOptions::default(),
    )
}

fn compile_file_artifacts_with_assets_collect(
    path: &Path,
    wasm_file_name: &str,
    assets: &BTreeMap<String, waluau_codegen_wasm::GeneratedAsset>,
    options: CompileOptions,
) -> Result<CompileArtifacts, Vec<Diagnostic>> {
    let asset_module_source = discover_asset_module(path).map_err(|error| vec![error])?;
    let outcome = link::link_program_collect_with_assets(
        path,
        &mut link::FsModules,
        asset_module_source.as_deref(),
    )
    .map_err(|error| vec![error])?;
    if !outcome.diagnostics.is_empty() {
        return Err(outcome.diagnostics);
    }
    compile_program(outcome.program, wasm_file_name, assets, options)
}

fn compile_program(
    program: waluau_ast::Program,
    wasm_file_name: &str,
    assets: &BTreeMap<String, waluau_codegen_wasm::GeneratedAsset>,
    options: CompileOptions,
) -> Result<CompileArtifacts, Vec<Diagnostic>> {
    compile_program_with_cache(program, wasm_file_name, assets, options, None, None, None)
}

fn compile_program_with_cache(
    program: waluau_ast::Program,
    wasm_file_name: &str,
    assets: &BTreeMap<String, waluau_codegen_wasm::GeneratedAsset>,
    options: CompileOptions,
    hir_cache: Option<&mut waluau_hir::TypeCheckCache>,
    ir_cache: Option<&mut waluau_ir::BuildCache>,
    wasm_cache: Option<&mut waluau_codegen_wasm::EmitCache>,
) -> Result<CompileArtifacts, Vec<Diagnostic>> {
    let started = CompilerTimer::start();
    let owned_typed;
    let changed_functions;
    let typed_program = match hir_cache {
        Some(cache) => {
            let (typed, changed) = waluau_hir::type_check_and_infer_collect_cached(&program, cache)
                .map_err(|errors| {
                    errors
                        .into_iter()
                        .map(|error| resolve_diagnostic_source(error, &program))
                        .collect::<Vec<_>>()
                })?;
            changed_functions = changed;
            typed
        }
        None => {
            owned_typed = waluau_hir::type_check_and_infer_collect(&program);
            changed_functions = &[];
            owned_typed
                .as_ref()
                .map_err(Clone::clone)
                .map_err(|errors| {
                    errors
                        .into_iter()
                        .map(|error| resolve_diagnostic_source(error, &program))
                        .collect::<Vec<_>>()
                })?
        }
    };
    let typed = started.elapsed();
    let resolved = started.elapsed();
    let owned_ir;
    let ir = match ir_cache {
        Some(cache) => {
            waluau_ir::build_cached_with_changes(typed_program, cache, changed_functions)
                .map_err(|error| vec![resolve_diagnostic_source(error, &program)])?
        }
        None => {
            owned_ir = waluau_ir::build(typed_program)
                .map_err(|error| vec![resolve_diagnostic_source(error, &program)])?;
            &owned_ir
        }
    };
    let lowered = started.elapsed();
    let debug_file_name = development_dwarf_file_name(wasm_file_name);
    let emitted = match wasm_cache {
        Some(cache) => waluau_codegen_wasm::emit_cached_with_options_and_debug_url(
            ir,
            cache,
            waluau_codegen_wasm::EmitOptions {
                development_dwarf: options.development_dwarf,
                minimal_exports: options.minimal_exports,
            },
            &debug_file_name,
        ),
        None => waluau_codegen_wasm::emit_with_options_and_debug_url(
            ir,
            waluau_codegen_wasm::EmitOptions {
                development_dwarf: options.development_dwarf,
                minimal_exports: options.minimal_exports,
            },
            &debug_file_name,
        ),
    }
    .map_err(|error| vec![error])?;
    let emitted_at = started.elapsed();
    let js = waluau_codegen_wasm::generate_js_glue_with_assets(wasm_file_name, &emitted, assets);
    if CompilerTimer::enabled() {
        eprintln!(
            "waluau timings: hir={:?} symbols={:?} ir={:?} wasm={:?} js={:?} total={:?}",
            typed,
            resolved - typed,
            lowered - resolved,
            emitted_at - lowered,
            started.elapsed() - emitted_at,
            started.elapsed(),
        );
    }
    Ok(CompileArtifacts {
        wasm: emitted.wasm,
        development_dwarf: emitted.development_dwarf,
        development_sources: emitted.development_sources,
        js,
        required_imports: emitted.required_imports,
        bytes_constants: emitted.bytes_constants,
    })
}

fn development_dwarf_file_name(wasm_file_name: &str) -> String {
    Path::new(wasm_file_name)
        .with_extension("debug.wasm")
        .to_string_lossy()
        .replace('\\', "/")
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
    let mut args = args.into_iter().peekable();
    // The `fmt` subcommand is dispatched before the default build path.
    if args.peek().is_some_and(|arg| arg == "fmt") {
        args.next();
        return fmt::run_fmt(args);
    }
    let mut session = session::CompilerSession::new();
    run_with_session_args(&mut session, args)
}

fn run_with_session_args<I>(
    session: &mut session::CompilerSession,
    args: I,
) -> Result<(), Vec<Diagnostic>>
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
        .ok_or_else(|| {
            vec![Diagnostic::new(
                "output Wasm path must have a UTF-8 file name",
            )]
        })?;
    let assets = asset_package
        .as_ref()
        .map(|package| &package.generated)
        .unwrap_or_else(|| empty_asset_manifest());
    let asset_module_source = asset_package
        .as_ref()
        .map(|package| package.module_source.as_str());
    let outcome = session.build_root_with_assets(
        &options.input,
        wasm_file_name,
        assets,
        asset_module_source,
        CompileOptions {
            development_dwarf: options.development_dwarf,
            minimal_exports: options.minimal_exports,
        },
    );
    if let Some(report_path) = &options.report {
        write_build_report(report_path, &outcome).map_err(|error| vec![error])?;
    }
    if !outcome.diagnostics.is_empty() {
        return Err(outcome.diagnostics);
    }
    let artifacts = outcome
        .artifacts
        .expect("artifacts are present when no diagnostics were reported");
    let debug_output = options.output.with_extension("debug.wasm");
    if let Some(debug_wasm) = artifacts.development_dwarf {
        // Publish the companion first so the runtime module never references a
        // missing artifact after a successful build.
        fs::write(&debug_output, debug_wasm)
            .map_err(|error| vec![io_error("write DWARF companion", &debug_output, error)])?;
    } else if let Err(error) = fs::remove_file(&debug_output)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(vec![io_error(
            "remove stale DWARF companion",
            &debug_output,
            error,
        )]);
    }
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompilerServerRequest {
    id: u64,
    args: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CompilerServerResponse {
    id: u64,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    diagnostics: Vec<serde_json::Value>,
    parses_performed: usize,
    cached_parse_count: usize,
}

/// Serve repeated CLI-compatible builds over newline-delimited JSON.
///
/// Each request is `{ "id": number, "args": string[] }`; each response uses
/// the same id and reports success plus session cache counters. The process
/// retains one [`CompilerSession`] until stdin closes, allowing Vite and other
/// build hosts to reuse parsed modules without embedding the compiler.
pub fn run_server(
    mut input: impl std::io::BufRead,
    mut output: impl std::io::Write,
) -> Result<(), Diagnostic> {
    let mut session = session::CompilerSession::new();
    let mut line = String::new();
    loop {
        line.clear();
        let bytes_read = input
            .read_line(&mut line)
            .map_err(|error| Diagnostic::new(format!("read compiler server request: {error}")))?;
        if bytes_read == 0 {
            return Ok(());
        }
        if line.trim().is_empty() {
            continue;
        }
        let request: CompilerServerRequest = serde_json::from_str(&line).map_err(|error| {
            Diagnostic::new(format!("invalid compiler server request: {error}"))
        })?;
        let result =
            run_with_session_args(&mut session, request.args.into_iter().map(OsString::from));
        let diagnostics = result.err().unwrap_or_default();
        let response = CompilerServerResponse {
            id: request.id,
            ok: diagnostics.is_empty(),
            error: (!diagnostics.is_empty()).then(|| {
                diagnostics
                    .iter()
                    .map(Diagnostic::render)
                    .collect::<Vec<_>>()
                    .join("\n")
            }),
            diagnostics: diagnostics.iter().map(serialize_diagnostic).collect(),
            parses_performed: session.parses_performed(),
            cached_parse_count: session.cached_parse_count(),
        };
        serde_json::to_writer(&mut output, &response)
            .map_err(|error| Diagnostic::new(format!("write compiler server response: {error}")))?;
        output
            .write_all(b"\n")
            .and_then(|()| output.flush())
            .map_err(|error| Diagnostic::new(format!("flush compiler server response: {error}")))?;
    }
}

/// Write the machine-readable build report consumed by build integrations
/// (e.g. the vite plugin wires `involvedFiles` into its watcher).
fn write_build_report(path: &Path, outcome: &session::BuildOutcome) -> Result<(), Diagnostic> {
    let diagnostics: Vec<serde_json::Value> = outcome
        .diagnostics
        .iter()
        .map(serialize_diagnostic)
        .collect();
    let report = serde_json::json!({
        "success": outcome.artifacts.is_some(),
        "involvedFiles": outcome
            .involved_files
            .iter()
            .map(|file| file.display().to_string())
            .collect::<Vec<_>>(),
        "developmentSources": outcome
            .artifacts
            .as_ref()
            .map(|artifacts| artifacts
                .development_sources
                .iter()
                .map(|source| serde_json::json!({
                    "path": source.path,
                    "source": source.source,
                }))
                .collect::<Vec<_>>())
            .unwrap_or_default(),
        "diagnostics": diagnostics,
        "workload": {
            "astNodes": outcome.ast_nodes,
            "linkedSourceBytes": outcome.linked_source_bytes,
            "sourceUnits": outcome.involved_files.len(),
        },
    });
    let serialized = serde_json::to_string_pretty(&report)
        .map_err(|error| Diagnostic::new(format!("serialize build report: {error}")))?;
    fs::write(path, serialized).map_err(|error| io_error("write build report", path, error))
}

fn serialize_diagnostic(diagnostic: &Diagnostic) -> serde_json::Value {
    serde_json::json!({
        "message": diagnostic.to_string(),
        "rendered": diagnostic.render(),
        "file": diagnostic.file_path(),
        "line": diagnostic.source_location().map(|(line, _)| line),
        "column": diagnostic.source_location().map(|(_, column)| column),
        "span": diagnostic.span().map(|span| serde_json::json!({
            "start": span.start,
            "end": span.end,
        })),
        "severity": match diagnostic.severity() {
            waluau_diagnostics::Severity::Error => "error",
            waluau_diagnostics::Severity::Warning => "warning",
        },
    })
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
    report: Option<PathBuf>,
    development_dwarf: bool,
    minimal_exports: bool,
}

#[derive(Clone, Copy)]
enum PendingPath {
    Output,
    Manifest,
    Report,
}

fn parse_args<I>(args: I) -> Result<CliOptions, Diagnostic>
where
    I: IntoIterator<Item = OsString>,
{
    let mut input = None;
    let mut output = None;
    let mut manifest = None;
    let mut report = None;
    let mut pending_path = None;
    let mut emit_js = false;
    let mut development_dwarf = false;
    let mut minimal_exports = false;

    for arg in args {
        if let Some(pending) = pending_path.take() {
            match pending {
                PendingPath::Output => output = Some(PathBuf::from(arg)),
                PendingPath::Manifest => manifest = Some(PathBuf::from(arg)),
                PendingPath::Report => report = Some(PathBuf::from(arg)),
            }
            continue;
        }

        match arg.to_str() {
            Some("-o" | "--output") => pending_path = Some(PendingPath::Output),
            Some("--manifest") => pending_path = Some(PendingPath::Manifest),
            Some("--report") => pending_path = Some(PendingPath::Report),
            Some("--emit-js") => emit_js = true,
            Some("--development-dwarf") => development_dwarf = true,
            Some("--minimal-exports") => minimal_exports = true,
            Some(flag) if flag.starts_with('-') => {
                return Err(Diagnostic::new(format!(
                    "unsupported flag `{flag}`\nusage: waluau <input.walu> [-o <output.wasm>] [--emit-js] [--development-dwarf] [--minimal-exports] [--manifest <waluau.assets.json>] [--report <report.json>]"
                )));
            }
            _ if input.is_none() => input = Some(PathBuf::from(arg)),
            _ => {
                return Err(Diagnostic::new(
                    "too many positional arguments\nusage: waluau <input.walu> [-o <output.wasm>] [--emit-js] [--development-dwarf] [--minimal-exports] [--manifest <waluau.assets.json>] [--report <report.json>]",
                ));
            }
        }
    }

    if let Some(pending) = pending_path {
        let flag = match pending {
            PendingPath::Output => "-o/--output",
            PendingPath::Manifest => "--manifest",
            PendingPath::Report => "--report",
        };
        return Err(Diagnostic::new(format!(
            "missing path after {flag}\nusage: waluau <input.walu> [-o <output.wasm>] [--emit-js] [--development-dwarf] [--minimal-exports] [--manifest <waluau.assets.json>] [--report <report.json>]"
        )));
    }

    let input = input.ok_or_else(|| {
        Diagnostic::new(
            "missing input path\nusage: waluau <input.walu> [-o <output.wasm>] [--emit-js] [--development-dwarf] [--minimal-exports] [--manifest <waluau.assets.json>] [--report <report.json>]",
        )
    })?;
    let output = output.unwrap_or_else(|| default_output_path(&input));

    Ok(CliOptions {
        input,
        output,
        emit_js,
        manifest,
        report,
        development_dwarf,
        minimal_exports,
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

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssetDeclaration {
    #[serde(default)]
    name: Option<String>,
    path: String,
    #[serde(rename = "type")]
    kind: AssetKind,
    #[serde(default)]
    family: Option<String>,
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
    module_source: String,
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
    let manifest = read_asset_manifest(path)?;
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    let module_source = generate_asset_module(&manifest.assets)?;
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
    Ok(PreparedAssetPackage {
        generated,
        files,
        module_source,
    })
}

fn read_asset_manifest(path: &Path) -> Result<ProjectAssetManifest, Diagnostic> {
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
    Ok(manifest)
}

pub(crate) fn discover_asset_module(root: &Path) -> Result<Option<String>, Diagnostic> {
    let start = root.parent().unwrap_or_else(|| Path::new("."));
    for directory in start.ancestors() {
        let manifest_path = directory.join("waluau.assets.json");
        if manifest_path.is_file() {
            let manifest = read_asset_manifest(&manifest_path)?;
            return generate_asset_module(&manifest.assets).map(Some);
        }
    }
    Ok(None)
}

fn valid_asset_name(name: &str) -> bool {
    const RESERVED: &[&str] = &[
        "and", "bool", "break", "bytes", "continue", "do", "else", "elseif", "end", "extern",
        "f32", "f64", "false", "for", "function", "i32", "i64", "if", "in", "local", "nil", "not",
        "number", "or", "repeat", "return", "string", "then", "thread", "true", "u32", "u64",
        "unit", "unknown", "until", "void", "while",
    ];
    let mut chars = name.chars();
    !RESERVED.contains(&name)
        && chars
            .next()
            .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn waluau_string(value: &str) -> String {
    serde_json::to_string(value).expect("strings always serialize")
}

fn generate_asset_module(declarations: &[AssetDeclaration]) -> Result<String, Diagnostic> {
    let named = declarations
        .iter()
        .filter(|declaration| declaration.name.is_some())
        .collect::<Vec<_>>();
    let mut names = BTreeSet::new();
    for declaration in &named {
        let name = declaration
            .name
            .as_deref()
            .expect("filtered named declaration");
        if !valid_asset_name(name) {
            return Err(Diagnostic::new(format!(
                "invalid asset name `{name}`; expected a Waluau identifier"
            )));
        }
        if !names.insert(name) {
            return Err(Diagnostic::new(format!("duplicate asset name `{name}`")));
        }
        match declaration.kind {
            AssetKind::Image | AssetKind::Audio => {}
            AssetKind::Font
                if declaration
                    .family
                    .as_deref()
                    .is_some_and(|family| !family.is_empty()) => {}
            AssetKind::Font => {
                return Err(Diagnostic::new(format!(
                    "named font asset `{name}` requires a non-empty `family`"
                )));
            }
            AssetKind::Text | AssetKind::Bytes => {
                return Err(Diagnostic::new(format!(
                    "typed bundle asset `{name}` has unsupported type `{}`",
                    declaration.kind.as_str()
                )));
            }
        }
    }

    let mut source = String::from(
        "-- Generated from waluau.assets.json. Do not edit.\n\
local resources = require(\"waluau:engine/resources\")\n\
local audio = require(\"waluau:engine/audio\")\n\n\
export type Bundle = {\n    owner: resources.Owner",
    );
    for declaration in &named {
        let name = declaration
            .name
            .as_deref()
            .expect("filtered named declaration");
        let ty = match declaration.kind {
            AssetKind::Image => "resources.ImageResource",
            AssetKind::Font => "resources.FontResource",
            AssetKind::Audio => "resources.SoundResource",
            AssetKind::Text | AssetKind::Bytes => unreachable!(),
        };
        source.push_str(&format!(",\n    {name}: {ty}?"));
    }
    source.push_str(
        "\n}\n\
export type LoadResult = { bundle: Bundle, errors: {resources.ResourceError} }\n\n\
function load(): LoadResult\n\
    local owner: resources.Owner = resources.new_owner()\n\
    local errors: {resources.ResourceError} = {}\n\
    local bundle: Bundle = {\n        owner = owner,\n",
    );
    for declaration in &named {
        source.push_str(&format!(
            "        {} = nil,\n",
            declaration
                .name
                .as_deref()
                .expect("filtered named declaration")
        ));
    }
    source.push_str("    }\n");
    for declaration in &named {
        let name = declaration
            .name
            .as_deref()
            .expect("filtered named declaration");
        let path = waluau_string(&declaration.path);
        let (result_ty, resource_ty, await_call, own_call) = match declaration.kind {
            AssetKind::Image => (
                "resources.ImageLoadResult",
                "resources.ImageResource",
                format!("resources.await_typed_image({path})"),
                "resources.own_image",
            ),
            AssetKind::Font => (
                "resources.FontLoadResult",
                "resources.FontResource",
                format!(
                    "resources.await_typed_font({path}, {})",
                    waluau_string(
                        declaration
                            .family
                            .as_deref()
                            .expect("validated font family")
                    )
                ),
                "resources.own_font",
            ),
            AssetKind::Audio => (
                "resources.SoundLoadResult",
                "resources.SoundResource",
                format!("audio.await_typed_sound({path})"),
                "resources.own_sound",
            ),
            AssetKind::Text | AssetKind::Bytes => unreachable!(),
        };
        source.push_str(&format!(
            "    local {name}_result: {result_ty} = {await_call}\n\
    local maybe_{name}: {resource_ty}? = {name}_result.resource\n\
    if maybe_{name} ~= nil then\n\
        local value: {resource_ty} = maybe_{name}::{resource_ty}\n\
        bundle.{name} = value\n\
        {own_call}(owner, value)\n\
    else\n\
        local maybe_error: resources.ResourceError? = {name}_result.error\n\
        if maybe_error ~= nil then\n\
            table.insert(errors, maybe_error::resources.ResourceError)\n\
        end\n\
    end\n"
        ));
    }
    source.push_str(
        "    return { bundle = bundle, errors = errors }\nend\n\nreturn { load = load }\n",
    );
    Ok(source)
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

    fn read_u32_leb(bytes: &[u8], cursor: &mut usize) -> u32 {
        let mut value = 0u32;
        let mut shift = 0;
        loop {
            let byte = bytes[*cursor];
            *cursor += 1;
            value |= u32::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return value;
            }
            shift += 7;
        }
    }

    fn custom_section(wasm: &[u8], target: &str) -> Option<Vec<u8>> {
        let mut cursor = 8;
        while cursor < wasm.len() {
            let id = wasm[cursor];
            cursor += 1;
            let payload_len = read_u32_leb(wasm, &mut cursor) as usize;
            let end = cursor + payload_len;
            if id == 0 {
                let name_len = read_u32_leb(wasm, &mut cursor) as usize;
                let name_end = cursor + name_len;
                if &wasm[cursor..name_end] == target.as_bytes() {
                    return Some(wasm[name_end..end].to_vec());
                }
            }
            cursor = end;
        }
        None
    }

    fn contains_cstring(bytes: &[u8], value: &str) -> bool {
        let mut encoded = value.as_bytes().to_vec();
        encoded.push(0);
        bytes.windows(encoded.len()).any(|window| window == encoded)
    }

    fn dwarf_line_row_files(section: &[u8]) -> std::collections::BTreeSet<u32> {
        let header_length = u32::from_le_bytes(section[6..10].try_into().unwrap()) as usize;
        let mut cursor = 10 + header_length;
        let mut file = 1u32;
        let mut files = std::collections::BTreeSet::new();
        while cursor < section.len() {
            match section[cursor] {
                0 => {
                    cursor += 1;
                    let length = read_u32_leb(section, &mut cursor) as usize;
                    cursor += length;
                }
                1 => {
                    cursor += 1;
                    files.insert(file);
                }
                2 | 3 | 5 => {
                    cursor += 1;
                    let _ = read_u32_leb(section, &mut cursor);
                }
                4 => {
                    cursor += 1;
                    file = read_u32_leb(section, &mut cursor);
                }
                other => panic!("unexpected line opcode {other}"),
            }
        }
        files
    }

    #[test]
    fn interface_brand_narrowing_links_across_modules() {
        // Brand checks work when the interface, a conforming type, and the
        // consuming module live in different files: the dotted narrowing
        // target (`ops.Add`), the dotted hard cast, and a second conforming
        // sibling declared in the entry module all compile through the
        // linker's canonicalized conformance names.
        let tempdir = tempdir().expect("tempdir should exist");
        let ops_path = tempdir.path().join("ops.walu");
        let entry_path = tempdir.path().join("entry.walu");
        fs::write(
            &ops_path,
            "export type Op = { exec: (self, a: i32, b: i32) -> i32 }\n\
             export type Add = Op & { bias: i32 }\n\
             function Add:exec(a: i32, b: i32): i32\n    return a + b + self.bias\nend\n\
             function make_add(): Add\n    return { bias = 1 }\nend\n\
             return { make_add = make_add }\n",
        )
        .expect("ops fixture should write");
        fs::write(
            &entry_path,
            "local ops = require(\"./ops\")\n\
             type Mul = ops.Op & { bias: i32 }\n\
             function Mul:exec(a: i32, b: i32): i32\n    return a * b + self.bias\nend\n\
             function pick(op: ops.Op): i32\n\
                 if ops.Add(a) = op then\n        return a.bias\n    end\n\
                 if Mul(m) = op then\n        return m.bias + 100\n    end\n\
                 return -1\nend\n\
             function force(op: ops.Op): i32\n    return (op :: ops.Add).bias\nend\n\
             function entry(): i32\n\
                 local mul: Mul = { bias = 2 }\n\
                 return pick(ops.make_add()) + pick(mul) + force(ops.make_add())\nend\n",
        )
        .expect("entry fixture should write");
        super::compile_file(&entry_path).expect("cross-module brand narrowing should compile");
    }

    #[test]
    fn cli_development_dwarf_maps_linked_sources_and_is_gated() {
        assert!(super::CLI_HELP.contains("--development-dwarf"));
        let tempdir = tempdir().expect("tempdir should exist");
        let lib_path = tempdir.path().join("lib.walu");
        let entry_path = tempdir.path().join("entry.walu");
        let default_output = tempdir.path().join("default.wasm");
        let debug_output = tempdir.path().join("debug.wasm");
        let debug_companion = tempdir.path().join("debug.debug.wasm");
        let debug_report = tempdir.path().join("debug-report.json");
        fs::write(
            &lib_path,
            "local function increment(value: i32): i32\n    return value + 1\nend\nreturn increment\n",
        )
        .expect("library fixture should write");
        fs::write(
            &entry_path,
            "local increment = require(\"./lib\")\nfunction entry(): i32\n    local function twice(value: i32): i32\n        return value * 2\n    end\n    return twice(increment(20))\nend\n",
        )
        .expect("entry fixture should write");

        super::run_with_args([os(&entry_path), OsString::from("-o"), os(&default_output)])
            .expect("default CLI build should succeed");
        let default_wasm = fs::read(&default_output).expect("default output");
        assert!(custom_section(&default_wasm, ".debug_info").is_none());

        super::run_with_args([
            os(&entry_path),
            OsString::from("-o"),
            os(&debug_output),
            OsString::from("--emit-js"),
            OsString::from("--development-dwarf"),
            OsString::from("--report"),
            os(&debug_report),
        ])
        .expect("development CLI build should succeed");
        let runtime_wasm = fs::read(&debug_output).expect("runtime output");
        let debug_wasm = fs::read(&debug_companion).expect("DWARF companion");
        let debug_js = fs::read_to_string(debug_output.with_extension("js"))
            .expect("development JavaScript glue");
        assert!(
            debug_js.contains("WebAssembly.compileStreaming(response.clone(), compilerOptions())"),
            "URL-based loading must retain the HTTP module URL for relative debug companions"
        );
        assert!(custom_section(&runtime_wasm, ".debug_info").is_none());
        assert!(custom_section(&runtime_wasm, "name").is_some());
        let reference =
            custom_section(&runtime_wasm, "external_debug_info").expect("external debug reference");
        let mut reference_cursor = 0;
        let reference_length = read_u32_leb(&reference, &mut reference_cursor) as usize;
        assert_eq!(
            &reference[reference_cursor..reference_cursor + reference_length],
            b"debug.debug.wasm"
        );
        let info = custom_section(&debug_wasm, ".debug_info").expect(".debug_info");
        let line = custom_section(&debug_wasm, ".debug_line").expect(".debug_line");
        assert!(custom_section(&debug_wasm, ".debug_abbrev").is_some());
        assert!(contains_cstring(&info, "entry"));
        assert!(contains_cstring(
            &line,
            "__waluau/sources/files/s-entry.walu"
        ));
        assert!(contains_cstring(&line, "__waluau/sources/files/s-lib.walu"));
        assert_eq!(
            dwarf_line_row_files(&line),
            std::collections::BTreeSet::from([1, 2]),
            "linked entry and lifted library function both need line mappings"
        );
        assert!(
            !String::from_utf8_lossy(&line).contains(tempdir.path().to_string_lossy().as_ref()),
            "debug paths must not contain the absolute build directory"
        );

        let report: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&debug_report).expect("development build report"),
        )
        .expect("development build report should be valid JSON");
        let development_sources = report["developmentSources"]
            .as_array()
            .expect("developmentSources array");
        assert_eq!(development_sources.len(), 2, "{development_sources:?}");
        assert!(development_sources.iter().any(|source| {
            source["path"] == "__waluau/sources/files/s-entry.walu"
                && source["source"].as_str()
                    == Some(
                        "local increment = require(\"./lib\")\nfunction entry(): i32\n    local function twice(value: i32): i32\n        return value * 2\n    end\n    return twice(increment(20))\nend\n",
                    )
        }));
        assert!(
            development_sources
                .iter()
                .any(|source| source["path"] == "__waluau/sources/files/s-lib.walu")
        );

        super::run_with_args([
            os(&entry_path),
            OsString::from("-o"),
            os(&debug_output),
            OsString::from("--report"),
            os(&debug_report),
        ])
        .expect("production rebuild should succeed");
        assert!(
            !debug_companion.exists(),
            "production rebuild must remove a stale DWARF companion"
        );
        let report: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&debug_report).expect("production build report"),
        )
        .expect("production build report should be valid JSON");
        assert_eq!(report["developmentSources"], serde_json::json!([]));
    }

    #[test]
    fn cli_development_report_carries_browser_paths_for_embedded_engine_sources() {
        let tempdir = tempdir().expect("tempdir should exist");
        let entry_path = tempdir.path().join("entry.walu");
        let output = tempdir.path().join("game.wasm");
        let report_path = tempdir.path().join("report.json");
        fs::write(
            &entry_path,
            "local graphics = require(\"waluau:engine/graphics\")\nfunction entry() end\n",
        )
        .expect("entry fixture should write");

        super::run_with_args([
            os(&entry_path),
            OsString::from("-o"),
            os(&output),
            OsString::from("--development-dwarf"),
            OsString::from("--report"),
            os(&report_path),
        ])
        .expect("engine development build should succeed");

        let report: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&report_path).expect("development build report"),
        )
        .expect("development build report should be valid JSON");
        let development_sources = report["developmentSources"]
            .as_array()
            .expect("developmentSources array");
        let graphics = development_sources
            .iter()
            .find(|source| {
                source["path"] == "__waluau/sources/packages/s-waluau-engine/s-v1/s-graphics.walu"
            })
            .expect("embedded graphics source snapshot");
        assert_eq!(
            graphics["source"],
            include_str!("../../../engine/graphics.walu")
        );
        assert!(development_sources.iter().all(|source| {
            !source["path"]
                .as_str()
                .expect("source path")
                .contains("package:")
        }));

        let debug_wasm = fs::read(output.with_extension("debug.wasm")).expect("DWARF companion");
        let line = custom_section(&debug_wasm, ".debug_line").expect(".debug_line");
        assert!(contains_cstring(
            &line,
            "__waluau/sources/packages/s-waluau-engine/s-v1/s-graphics.walu"
        ));
        assert!(!contains_cstring(&line, "package:waluau-engine"));
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
    fn compile_file_supports_enum_expectations_and_not_modifier() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("enums.test.walu");
        fs::write(
            &input_path,
            r#"
                require("waluau:vitest")

                enum Direction { north, east, south, west }

                describe("enums", function(): unit
                    it("compares enum values", function(): unit
                        local actual = Direction.east
                        expect(actual):toBe(Direction.east)
                        expect(actual):notToBe(Direction.west)
                        expect(actual):not:toBe(Direction.west)
                        expect(add(1, 1)):not:toBe(3)
                        expect("walu"):not:toContain("moon")
                        expect(1 == 2):not:toBeTruthy()
                    end)
                end)

                function add(a: i32, b: i32): i32
                    return a + b
                end
            "#,
        )
        .expect("test file should write");

        let wasm = super::compile_file(&input_path).expect("enum expectations should compile");
        let wat = wasmprinter::print_bytes(&wasm).expect("wasm should print");
        assert!(
            wat.contains(r#"(import "waluau" "EnumExpectation.toBe""#),
            "enum matcher should import under its extern type name:\n{wat}"
        );
        assert!(
            wat.contains(r#"(import "waluau" "EnumExpectation.get/not""#),
            "the :not modifier should import as the extern property getter:\n{wat}"
        );
        assert!(
            wat.contains(r#"(import "waluau" "NumberExpectation.get/not""#),
            "the :not modifier should resolve per expectation type:\n{wat}"
        );
    }

    #[test]
    fn compile_file_supports_nullable_expectations() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("nullable.test.walu");
        fs::write(
            &input_path,
            r#"
                require("waluau:vitest")

                enum Direction { north, east }

                it("nullable expectations", function(): unit
                    local maybe: Direction? = Direction.north
                    expect(maybe):not:toBeNil()
                    expect(maybe):toBe(Direction.north)
                    expect(maybe):not:toBe(Direction.east)

                    local maybe_number: f64? = 4.5
                    expect(maybe_number):toBe(4.5)
                    local maybe_text: string? = nil
                    expect(maybe_text):toBeNil()
                    local maybe_flag: bool? = true
                    expect(maybe_flag):toBe(true)
                end)
            "#,
        )
        .expect("test file should write");

        let wasm = super::compile_file(&input_path).expect("nullable expectations should compile");
        let wat = wasmprinter::print_bytes(&wasm).expect("wasm should print");
        assert!(
            wat.contains(r#"(import "waluau" "expect#enum?""#),
            "nullable-primitive expect overloads should import under \
             signature-derived host names:\n{wat}"
        );
        assert!(
            wat.contains(r#"(import "waluau" "expect#f64?""#),
            "each nullable-primitive signature should get its own import name:\n{wat}"
        );
        assert!(
            wat.contains(r#"(import "waluau" "NullableEnumExpectation.toBeNil""#),
            "nullable matchers should import under their extern type names:\n{wat}"
        );
        assert!(
            wat.contains(r#"(import "waluau" "NullableEnumExpectation.get/not""#),
            "the :not modifier should resolve on nullable expectations:\n{wat}"
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
    fn compiles_typed_json_builtins_to_specialized_host_visits() {
        let source = r#"
            type Payload = { name: string, count: i32 }
            local payload: Payload = { name = "Ada", count = 42 }
            local packed = json.pack(payload)
            local decoded, err = json.unpack<Payload>(packed)
            assert(err == "")
            if decoded ~= nil then
                assert(decoded.count == 42)
            end

            local samples = {1.5, -2.25}::Float32Array
            local samples_copy, samples_err = json.unpack<Float32Array>(json.pack(samples))
            assert(samples_err == "")
            if samples_copy ~= nil then
                assert(samples_copy[1] == -2.25)
            end

            type Result = Count(i32) | Ratio(f64)
            local result: Result = Count(9)
            local result_copy, result_err = json.unpack<Result>(json.pack(result))
            assert(result_err == "")
            if result_copy ~= nil then
                assert(result_copy is Count)
            end
        "#;
        let wasm = super::compile_source(source).expect("typed JSON should compile");
        let wat = wasmprinter::print_bytes(&wasm).expect("wasm should print");
        assert!(wat.contains("__json_pack_object"), "{wat}");
        assert!(wat.contains("__json_unpack_start"), "{wat}");
        assert!(wat.contains("__json_unpack_object_get"), "{wat}");
        assert!(wat.contains("__json_unpack_variant_is"), "{wat}");
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
    fn cli_report_lists_involved_files_and_diagnostics() {
        let tempdir = tempdir().expect("tempdir should exist");
        let lib_path = tempdir.path().join("lib.walu");
        let entry_path = tempdir.path().join("entry.walu");
        let report_path = tempdir.path().join("report.json");
        fs::write(
            &lib_path,
            "function double(x: i32): i32\n    return x * 2\nend\nreturn double\n",
        )
        .expect("fixture should write");
        fs::write(
            &entry_path,
            "local double = require(\"./lib\")\nfunction entry(): bool\n    return double(3)\nend\n",
        )
        .expect("fixture should write");

        let _ = super::run_with_args([
            os(&entry_path),
            OsString::from("--report"),
            os(&report_path),
        ])
        .expect_err("type error should fail the build");

        let report: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&report_path).expect("report should exist"))
                .expect("report should be valid JSON");
        assert_eq!(report["success"], false);
        let involved: Vec<String> = report["involvedFiles"]
            .as_array()
            .expect("involvedFiles array")
            .iter()
            .map(|value| value.as_str().expect("path string").to_string())
            .collect();
        assert_eq!(involved.len(), 2, "{involved:?}");
        assert!(involved.iter().any(|path| path.ends_with("entry.walu")));
        assert!(involved.iter().any(|path| path.ends_with("lib.walu")));
        let diagnostics = report["diagnostics"].as_array().expect("diagnostics array");
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0]["severity"], "error");
        assert!(diagnostics[0]["file"].as_str().is_some());
    }

    #[test]
    fn compiler_server_reuses_one_session_across_build_requests() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input = tempdir.path().join("main.walu");
        let output = tempdir.path().join("game.wasm");
        let report = tempdir.path().join("report.json");
        fs::write(&input, "local answer: i32 = 42\n").expect("source should write");
        let args = vec![
            input.display().to_string(),
            "-o".to_string(),
            output.display().to_string(),
            "--emit-js".to_string(),
            "--report".to_string(),
            report.display().to_string(),
        ];
        let requests = format!(
            "{}\n{}\n",
            serde_json::json!({ "id": 1, "args": args }),
            serde_json::json!({ "id": 2, "args": args }),
        );
        let mut responses = Vec::new();

        super::run_server(std::io::Cursor::new(requests), &mut responses)
            .expect("compiler server should complete");

        let responses = String::from_utf8(responses).expect("responses should be UTF-8");
        let parsed: Vec<serde_json::Value> = responses
            .lines()
            .map(|line| serde_json::from_str(line).expect("response should be JSON"))
            .collect();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0]["ok"], true);
        assert_eq!(parsed[1]["ok"], true);
        assert_eq!(parsed[0]["parsesPerformed"], 1);
        assert_eq!(
            parsed[1]["parsesPerformed"], 1,
            "the unchanged second build should reuse the cached parse"
        );
        assert_eq!(parsed[1]["cachedParseCount"], 1);
        assert!(output.exists());
        assert!(output.with_extension("js").exists());
    }

    #[test]
    fn compiler_server_returns_structured_diagnostics() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input = tempdir.path().join("main.walu");
        fs::write(&input, "local answer: bool = 42\n").expect("source should write");
        let requests = format!(
            "{}\n",
            serde_json::json!({ "id": 1, "args": [input.display().to_string()] }),
        );
        let mut responses = Vec::new();

        super::run_server(std::io::Cursor::new(requests), &mut responses)
            .expect("compiler server should complete");

        let response: serde_json::Value =
            serde_json::from_slice(&responses).expect("response should be JSON");
        assert_eq!(response["ok"], false);
        let diagnostic = &response["diagnostics"][0];
        assert!(
            diagnostic["file"]
                .as_str()
                .is_some_and(|file| file.ends_with("main.walu"))
        );
        assert_eq!(diagnostic["line"], 1);
        assert!(diagnostic["column"].as_u64().is_some());
        assert_eq!(diagnostic["severity"], "error");
        assert!(diagnostic["message"].as_str().is_some());
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
    fn cli_compiles_manifest_generated_typed_asset_bundle() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("main.walu");
        let manifest_path = tempdir.path().join("waluau.assets.json");
        fs::create_dir_all(tempdir.path().join("assets")).expect("asset dir should exist");
        fs::write(tempdir.path().join("assets/card.png"), b"image").expect("image should write");
        fs::write(tempdir.path().join("assets/font.woff2"), b"font").expect("font should write");
        fs::write(tempdir.path().join("assets/cue.wav"), b"audio").expect("audio should write");
        fs::write(
            &input_path,
            r#"local assets = require("waluau:assets")
function load_assets(): i32
    local result: assets.LoadResult = assets.load()
    local bundle: assets.Bundle = result.bundle
    if bundle.card ~= nil and bundle.font ~= nil and bundle.cue ~= nil then return 3 end
    return #result.errors
end
"#,
        )
        .expect("fixture should write");
        fs::write(
            &manifest_path,
            r#"{
                "version": 1,
                "assets": [
                    {"name":"card","path":"assets/card.png","type":"image"},
                    {"name":"font","path":"assets/font.woff2","type":"font","family":"Fixture"},
                    {"name":"cue","path":"assets/cue.wav","type":"audio"}
                ]
            }"#,
        )
        .expect("manifest should write");

        super::run_with_args([
            os(&input_path),
            OsString::from("--output"),
            os(tempdir.path().join("game.wasm")),
            OsString::from("--emit-js"),
            OsString::from("--manifest"),
            os(&manifest_path),
        ])
        .expect("generated typed bundle should compile");

        fs::write(
            &input_path,
            r#"local assets = require("waluau:assets")
local resources = require("waluau:engine/resources")
local graphics = require("waluau:engine/graphics")
function wrong_kind(g: graphics.Graphics, bundle: assets.Bundle): graphics.TextureResult
    local sound: resources.SoundResource = bundle.cue::resources.SoundResource
    return g:texture_from_image(sound)
end
"#,
        )
        .expect("invalid fixture should write");
        let errors = super::run_with_args([
            os(&input_path),
            OsString::from("--output"),
            os(tempdir.path().join("bad.wasm")),
            OsString::from("--emit-js"),
            OsString::from("--manifest"),
            os(&manifest_path),
        ])
        .expect_err("sound resources must not type-check as images");
        assert!(
            errors
                .iter()
                .any(|error| error.to_string().contains("SoundResource")
                    && error.to_string().contains("ImageResource")),
            "{errors:?}"
        );
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

        let bad_name = tempdir.path().join("bad-name.json");
        fs::write(
            &bad_name,
            r#"{"version":1,"assets":[{"name":"card-back","path":"same.txt","type":"image"}]}"#,
        )
        .expect("manifest should write");
        let error = super::prepare_asset_package(&bad_name).expect_err("bad name should fail");
        assert!(error.to_string().contains("invalid asset name"));

        let missing_family = tempdir.path().join("missing-family.json");
        fs::write(
            &missing_family,
            r#"{"version":1,"assets":[{"name":"font","path":"same.txt","type":"font"}]}"#,
        )
        .expect("manifest should write");
        let error = super::prepare_asset_package(&missing_family)
            .expect_err("named font without family should fail");
        assert!(error.to_string().contains("requires a non-empty `family`"));
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
                export type Promise<T> = extern

                declare function host_exchange(value: Promise<i32>): Promise<string>
                declare function host_text(): Promise<string>

                function exchange(value: Promise<i32>): Promise<string>
                    return host_exchange(value)
                end

                function make_text(): Promise<string>
                    return host_text()
                end

                return { exchange = exchange, make_text = make_text }
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

                function read(): string
                    return promise.await(service.make_text())
                end
            "#,
        )
        .expect("entry module should write");

        let artifacts = super::compile_file_artifacts(&input_path, "game.wasm")
            .expect("module-local aliases in host import signatures should compile");
        assert!(artifacts.js.contains("host_exchange"));
        assert!(artifacts.js.contains("host_text"));
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
    fn compiles_particle_gallery_fixture() {
        super::compile_file(&fixture_path("particles/main.walu"))
            .expect("particle gallery fixture should compile");
    }

    #[test]
    fn compiles_particle_headless_simulation() {
        super::compile_file(&fixture_path("particles/sim.walu"))
            .expect("headless particle simulation should compile");
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
    fn compiles_ante_magic_game_engine_fixture() {
        let output = tempdir().expect("output directory should exist");
        super::run_with_args([
            os(app_path("ante/src/main.walu")),
            OsString::from("--output"),
            os(output.path().join("ante.wasm")),
            OsString::from("--emit-js"),
            OsString::from("--manifest"),
            os(app_path("ante/waluau.assets.json")),
        ])
        .expect("Ante Magic game engine fixture should compile");
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

        let wasm = super::compile_file(&entry)
            .expect("embedded engine package and its re-exported callback types should compile");
        let wat = wasmprinter::print_bytes(&wasm).expect("game package Wasm should print");
        assert!(
            wat.contains(r#"(import "waluau" "__waluau_hmr_register""#),
            "hot registration should use the development host bridge:\n{wat}"
        );
        assert!(
            wat.contains(r#"(export "__waluau_call_callback_unit""#),
            "hot registration closures should emit the unit callback trampoline:\n{wat}"
        );
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
    fn compiles_pinned_engine_particle_subsystem_package() {
        let project = tempdir().expect("temp project should exist");
        let entry = project.path().join("main.walu");
        fs::write(
            &entry,
            r#"
                local engine = require("waluau:engine")
                local particles = require("waluau:engine/v1/particles")

                local system: particles.ParticleSystem = particles.new(8)
                system:set_particle_lifetime(1.0, 1.0)
                system:emit(3)
                assert(system:count() == 3)

                -- The aggregate facade re-exports the same linked type.
                local shared: engine.ParticleSystem = system
                assert(shared:buffer_size() == 8)
            "#,
        )
        .expect("external particle subsystem project should write");

        super::compile_file(&entry).expect("versioned particle subsystem import should compile");
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
    fn compiles_revisioned_shader_source_package() {
        let project = tempdir().expect("temp project should exist");
        let entry = project.path().join("main.walu");
        fs::write(
            &entry,
            r#"
                local shader_sources = require("waluau:engine/v1/shader_sources")
                local pixel = shader_sources.open("effects.pixel")
                local update = pixel:poll()
                assert(update.changed)
            "#,
        )
        .expect("external shader source project should write");

        let wasm = super::compile_file(&entry)
            .expect("versioned shader source subsystem import should compile");
        let wat = wasmprinter::print_bytes(&wasm).expect("shader source Wasm should print");
        assert!(
            wat.contains(r#"(import "waluau" "__waluau_shader_source_revision""#),
            "shader source polling should import the host revision:\n{wat}"
        );
        assert!(
            wat.contains(r#"(import "waluau" "__waluau_shader_source_text""#),
            "shader source polling should import the host text:\n{wat}"
        );
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
                export type State = { score: i32 }

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
    fn compile_file_preserves_imported_method_result_fields_in_for_in() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("model.walu"),
            r#"
                export type Item = { value: i32 }
                export type View = { items: {Item} }
                export opaque type State = { items: {Item} }

                function State.new(): State
                    return { items = { { value = 20 }, { value = 22 } } }
                end

                function State:view(): View
                    return { items = self.items }
                end
            "#,
        )
        .expect("model module should write");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                local model = require("./model")

                function total(): i32
                    local state: model.State = model.State.new()
                    local sum: i32 = 0
                    for item in state:view().items do
                        sum += item.value
                    end
                    return sum
                end

                assert(total() == 42)
            "#,
        )
        .expect("main module should write");

        let wasm = super::compile_file(&input_path)
            .expect("for-in should preserve an imported method's declared result field type");
        assert!(!wasm.is_empty());
    }

    #[test]
    fn compile_file_treats_imported_module_type_aliases_as_transparent() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("gfx.walu"),
            r#"
                export type Graphics = { width: f64 }

                function make(): Graphics
                    return { width = 2.0 }
                end

                return { make = make }
            "#,
        )
        .expect("graphics module should write");
        fs::write(
            tempdir.path().join("engine.walu"),
            r#"
                local gfx = require("./gfx")

                type Hooks = { draw: (gfx.Graphics, f64) -> unit }

                function run_hooks(hooks: Hooks): unit
                    hooks.draw(gfx.make(), 1.0)
                end

                return { run_hooks = run_hooks }
            "#,
        )
        .expect("engine module should write");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                local gfx = require("./gfx")
                local engine = require("./engine")

                type Graphics = gfx.Graphics

                local function draw(graphics: Graphics, dt: f64): unit
                    assert(graphics.width + dt == 3.0)
                end

                engine.run_hooks({ draw = draw })
            "#,
        )
        .expect("main module should write");

        let wasm = super::compile_file(&input_path).expect("compile should succeed");
        assert!(!wasm.is_empty());
    }

    #[test]
    fn compile_file_preserves_record_identity_through_exported_import_aliases() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("geometry.walu"),
            r#"
                export type Surface = { width: f64 }
                export type Rect = { x: f64 }

                function surface(): Surface
                    return { width = 2.0 }
                end

                function rect(): Rect
                    return { x = 1.0 }
                end

                return { surface = surface, rect = rect }
            "#,
        )
        .expect("geometry module should write");
        fs::write(
            tempdir.path().join("ui.walu"),
            r#"
                local geometry = require("./geometry")

                export type Surface = geometry.Surface
                export type Rect = geometry.Rect
                export type Node = { paint: (Surface, Rect) -> unit }

                function paint(node: Node): unit
                    node.paint(geometry.surface(), geometry.rect())
                end

                function nodes(value: {Node}): {Node}
                    return value
                end

                return { paint = paint, nodes = nodes }
            "#,
        )
        .expect("ui module should write");
        fs::write(
            tempdir.path().join("bridge.walu"),
            r#"
                local ui = require("./ui")

                export type Surface = ui.Surface
                export type Rect = ui.Rect
                export type Node = ui.Node

                function nodes(value: {Node}): {Node}
                    return value
                end

                return { nodes = nodes }
            "#,
        )
        .expect("bridge module should write");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                local geometry = require("./geometry")
                local ui = require("./ui")
                local bridge = require("./bridge")

                local function draw(surface: geometry.Surface, rect: geometry.Rect): unit
                    assert(surface.width + rect.x == 3.0)
                end

                local node: ui.Node = { paint = draw }
                local input: {bridge.Node} = { node }
                local output: {ui.Node} = bridge.nodes(input)
                ui.paint(output[0])
            "#,
        )
        .expect("main module should write");

        let wasm =
            super::compile_file(&input_path).expect("aliases should preserve record identity");
        assert!(!wasm.is_empty());
    }

    #[test]
    fn compile_file_distinguishes_unrelated_record_aliases_in_callback_diagnostics() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("left.walu"),
            r#"
                export type Surface = { width: f64 }
                export type Hook = { paint: (Surface) -> unit }

                function run(hook: Hook, surface: Surface): unit
                    hook.paint(surface)
                end

                return { run = run }
            "#,
        )
        .expect("left module should write");
        fs::write(
            tempdir.path().join("right.walu"),
            r#"
                export type Surface = { width: f64 }

                function surface(): Surface
                    return { width = 2.0 }
                end

                return { surface = surface }
            "#,
        )
        .expect("right module should write");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                local left = require("./left")
                local right = require("./right")

                local function paint(surface: right.Surface): unit
                    assert(surface.width == 2.0)
                end

                left.run({ paint = paint }, right.surface())
            "#,
        )
        .expect("main module should write");

        let error = super::compile_file(&input_path).expect_err("unrelated aliases should differ");
        let message = error.to_string();
        assert_eq!(
            message,
            "cannot implicitly convert (right.Surface) -> unit to (left.Surface) -> unit"
        );
        assert!(
            !message.contains("__waluau_m"),
            "diagnostic leaked an internal module name: {message}"
        );
    }

    #[test]
    fn compile_file_hides_module_mangling_in_callback_conversion_diagnostics() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("gfx.walu"),
            r#"
                type Graphics = { width: f64 }

                function make(): Graphics
                    return { width = 2.0 }
                end

                return { make = make }
            "#,
        )
        .expect("graphics module should write");
        fs::write(
            tempdir.path().join("engine.walu"),
            r#"
                local gfx = require("./gfx")

                type Hooks = { draw: (gfx.Graphics, f64) -> unit }

                function run_hooks(hooks: Hooks): unit
                    hooks.draw(gfx.make(), 1.0)
                end

                return { run_hooks = run_hooks }
            "#,
        )
        .expect("engine module should write");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                local engine = require("./engine")

                type WrongGraphics = { label: string }

                local function draw(graphics: WrongGraphics, dt: f64): unit
                    assert(graphics.label == tostring(dt))
                end

                engine.run_hooks({ draw = draw })
            "#,
        )
        .expect("main module should write");

        let error = super::compile_file(&input_path).expect_err("compile should fail");
        let message = error.to_string();
        assert!(
            message.contains("Graphics"),
            "unexpected diagnostic: {message}"
        );
        assert!(
            !message.contains("__waluau_m"),
            "diagnostic leaked an internal module name: {message}"
        );
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
    fn compile_file_iterates_imported_enum_with_pairs() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("spells.walu"),
            r#"
                export enum SpellKind { Firebolt, FreezeRay, RaiseCard, Clone }

                function noop(): i32
                    return 0
                end

                return { noop = noop }
            "#,
        )
        .expect("spells module should write");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                local spells = require("./spells")

                local names = ""
                local ordinals: i32 = 0
                for name, kind in pairs(spells.SpellKind) do
                    names = names .. name .. ";"
                    ordinals = ordinals * 10 + (kind :: i32)
                end
                assert(names == "Firebolt;FreezeRay;RaiseCard;Clone;")
                assert(ordinals == 123)

                local count: i32 = 0
                for name in pairs(spells.SpellKind) do
                    count = count + 1
                end
                assert(count == 4)
            "#,
        )
        .expect("main module should write");

        let wasm = super::compile_file(&input_path).expect("compile should succeed");
        assert!(!wasm.is_empty());
    }

    #[test]
    fn compile_file_resolves_type_statics_through_require_bindings() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("t.walu"),
            r#"
                export type S = {
                    v: number,
                }

                function S.new(v: number): S
                    return { v = v }
                end

                function S:double(): number
                    return self.v * 2.0
                end

                export enum SpellKind { Firebolt, FreezeRay }

                function SpellKind.from(value: i32): SpellKind?
                    if value == 1 then return SpellKind.Firebolt end
                    if value == 2 then return SpellKind.FreezeRay end
                    return nil
                end
            "#,
        )
        .expect("t module should write");
        let input_path = tempdir.path().join("call.walu");
        fs::write(
            &input_path,
            r#"
                local t = require("./t")

                local s = t.S.new(10)
                assert(s.v == 10)
                assert(s:double() == 20)

                -- A colon method's static form is reachable the same way.
                assert(t.S.double(s) == 20)

                local frost = t.SpellKind.from(2)
                assert(frost == t.SpellKind.FreezeRay)
                assert(t.SpellKind.from(9) == nil)

                -- A static reference is a plain function value.
                local make = t.S.new
                assert(make(3).v == 3)
            "#,
        )
        .expect("call module should write");

        let wasm = super::compile_file(&input_path).expect("compile should succeed");
        assert!(!wasm.is_empty());
    }

    #[test]
    fn compile_file_resolves_static_members_through_local_type_aliases() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                type X = { v: i32 }

                function X.new(): X
                    return { v = 42 }
                end

                type X2 = X
                type X3 = X2

                enum E { A }
                type E2 = E
                type E3 = E2

                local x = X3.new()
                local e = E3.A
                assert(x.v == 42)
                assert(e == E.A)
            "#,
        )
        .expect("main should write");

        let wasm = super::compile_file(&input_path).expect("compile should succeed");
        assert!(!wasm.is_empty());
    }

    #[test]
    fn compile_source_resolves_static_members_through_local_type_aliases() {
        let source = r#"
            type X = { v: i32 }

            function X.new(): X
                return { v = 42 }
            end

            type X2 = X
            local x = X2.new()

            enum E { A }
            type E2 = E
            local e = E2.A

            assert(x.v == 42)
            assert(e == E.A)
        "#;

        let wasm = super::compile_source(source).expect("compile should succeed");
        assert!(!wasm.is_empty());
    }

    #[test]
    fn compile_file_resolves_exported_and_imported_type_alias_members() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("types.walu"),
            r#"
                export type X = { v: i32 }

                function X.new(): X
                    return { v = 42 }
                end

                export type X2 = X

                export enum E { A }
                export type E2 = E
            "#,
        )
        .expect("types module should write");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                local types = require("./types")

                local exported = types.X2.new()
                local exported_enum = types.E2.A

                type X3 = types.X2
                type E3 = types.E2
                local imported = X3.new()
                local imported_enum: E3 = E3.A

                assert(exported.v == 42)
                assert(imported.v == 42)
                assert(exported_enum == imported_enum)
            "#,
        )
        .expect("main should write");

        let wasm = super::compile_file(&input_path).expect("compile should succeed");
        assert!(!wasm.is_empty());
    }

    #[test]
    fn compile_file_rejects_statics_on_private_types() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("t.walu"),
            r#"
                export type Public = { v: number }

                type Hidden = { v: number }

                function Hidden.new(v: number): Hidden
                    return { v = v }
                end
            "#,
        )
        .expect("t module should write");
        let input_path = tempdir.path().join("call.walu");
        fs::write(
            &input_path,
            r#"
                local t = require("./t")

                local h = t.Hidden.new(1)
            "#,
        )
        .expect("call module should write");

        let error = super::compile_file(&input_path).expect_err("compile should fail");
        assert!(
            error.to_string().contains("Hidden"),
            "a private type's statics must stay unreachable: {error}"
        );
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
    fn compile_file_reads_nullable_bool_from_exported_module_record() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("config.walu"),
            r#"
                export type Config = { enabled: bool? }

                function enabled(config: Config): bool
                    local value: bool? = config.enabled
                    if value ~= nil then
                        return value::bool
                    end
                    return false
                end

                return {
                    enabled = enabled,
                }
            "#,
        )
        .expect("config module should write");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                local config = require("./config")

                type Config = config.Config

                function entry(value: bool?): bool
                    return config.enabled({ enabled = value })
                end
            "#,
        )
        .expect("main module should write");

        let wasm = super::compile_file(&input_path).expect("compile should succeed");
        assert!(!wasm.is_empty());
    }

    #[test]
    fn compile_file_supports_stateful_module_local_bindings() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("counter.walu"),
            r#"
                type State = { value: i32 }

                local state: State = { value = 0::i32 }

                local function set(value: i32): unit
                    state.value = value
                end

                local function get(): i32
                    return state.value
                end

                return {
                    set = set,
                    get = get,
                }
            "#,
        )
        .expect("counter module should write");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                local counter = require("./counter")

                counter.set(41)
                assert(counter.get() == 41)
            "#,
        )
        .expect("main module should write");

        let wasm = super::compile_file(&input_path).expect("compile should succeed");
        assert!(!wasm.is_empty());
    }

    #[test]
    fn compile_file_keeps_record_module_local_used_by_public_functions() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("targets.walu"),
            r#"
                type Box = { x: f64 }
                type Targets = { active: Box }

                function nowhere(): Box
                    return { x = -1.0 }
                end

                local targets: Targets = { active = nowhere() }

                function set_target(x: f64): unit
                    targets = { active = { x = x } }
                end

                function target_x(): f64
                    return targets.active.x
                end

                return { set_target = set_target, target_x = target_x }
            "#,
        )
        .expect("targets module should write");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                local targets = require("./targets")
                assert(targets.target_x() == -1.0)
                targets.set_target(41.0)
                assert(targets.target_x() == 41.0)
            "#,
        )
        .expect("main module should write");

        let wasm = super::compile_file(&input_path).expect("compile should succeed");
        assert!(!wasm.is_empty());
    }

    #[test]
    fn compile_file_clones_typed_aggregate_constants_at_each_use() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("defaults.walu"),
            r#"
                type Inner = { value: i32 }
                type Defaults = { inner: Inner, values: {i32} }

                const BASE: i32 = 7
                const DEFAULTS: Defaults = {
                    inner = { value = BASE },
                    values = { BASE, 8::i32 },
                }

                local changed = DEFAULTS
                changed.inner.value = 55
                changed.values[0] = 55
                local unchanged = DEFAULTS
                assert(unchanged.inner.value == BASE)
                assert(unchanged.values[0] == BASE)

                function defaults_are_independent(): bool
                    local first: Defaults = DEFAULTS
                    first.inner.value = 99
                    first.values[0] = 99
                    local second: Defaults = DEFAULTS
                    return second.inner.value == BASE and second.values[0] == BASE
                end

                return {
                    DEFAULTS = DEFAULTS,
                    defaults_are_independent = defaults_are_independent,
                }
            "#,
        )
        .expect("defaults module should write");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                local defaults = require("./defaults")
                assert(defaults.defaults_are_independent())
                local first = defaults.DEFAULTS
                first.inner.value = 101
                first.values[0] = 101
                local second = defaults.DEFAULTS
                assert(second.inner.value == 7)
                assert(second.values[0] == 7)
            "#,
        )
        .expect("main module should write");

        let wasm = super::compile_file(&input_path).expect("compile should succeed");
        assert!(!wasm.is_empty());
    }

    #[test]
    fn compile_file_exports_module_constants() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("config.walu"),
            r#"
                -- Keep declarations beyond the importing file's length. This
                -- catches copied constants retaining cross-file source spans.
                -- xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
                -- xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
                -- xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
                -- xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
                -- xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
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

        let artifacts = super::compile_file_artifacts_with_options(
            &input_path,
            "program.wasm",
            super::CompileOptions {
                development_dwarf: true,
                ..super::CompileOptions::default()
            },
        )
        .expect("compile with external DWARF should succeed");
        assert!(!artifacts.wasm.is_empty());
        assert!(artifacts.development_dwarf.is_some());
    }

    #[test]
    fn compile_file_exports_nominal_enum_constants() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("faces.walu"),
            r#"
                export enum Face { down, up }
                const FACES_UP: Face = Face.up

                function is_up(face: Face): bool
                    return face == FACES_UP
                end

                return {
                    FACES_UP = FACES_UP,
                    is_up = is_up,
                }
            "#,
        )
        .expect("faces module should write");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                local faces = require("./faces")
                local face: faces.Face = faces.FACES_UP
                assert(faces.is_up(face))
            "#,
        )
        .expect("main module should write");

        let wasm = super::compile_file(&input_path).expect("compile should succeed");
        assert!(!wasm.is_empty());
    }

    #[test]
    fn compile_file_imports_exported_declarations_and_enum_namespace() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("directions.walu"),
            r#"
                export enum Direction { north, east, south }
                export type Step = { direction: Direction, distance: i32 }
                type Scratch = { value: i32 }
            "#,
        )
        .expect("declaration-only module should write");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                local directions = require("./directions")

                function score(direction: directions.Direction): i32
                    match direction do
                    case directions.Direction.north then
                        return 1
                    case directions.Direction.east then
                        return 2
                    case directions.Direction.south then
                        return 3
                    end
                end

                local direction: directions.Direction = directions.Direction.east
                local step: directions.Step = { direction = direction, distance = 4 }
                assert(score(step.direction) == 2)
            "#,
        )
        .expect("main module should write");

        let wasm = super::compile_file(&input_path).expect("compile should succeed");
        assert!(!wasm.is_empty());
    }

    #[test]
    fn compile_file_keeps_plain_type_declarations_private() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("types.walu"),
            r#"
                export type Public = { value: i32 }
                type Hidden = { value: i32 }
            "#,
        )
        .expect("types module should write");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                local types = require("./types")
                local value: types.Hidden = { value = 1 }
            "#,
        )
        .expect("main module should write");

        let error = super::compile_file(&input_path)
            .expect_err("private imported type should be rejected")
            .to_string();
        assert!(error.contains("unknown type 'types.Hidden'"), "{error}");
    }

    #[test]
    fn compile_file_rejects_non_exhaustive_imported_enum_match() {
        let tempdir = tempdir().expect("tempdir should exist");
        fs::write(
            tempdir.path().join("directions.walu"),
            "export enum Direction { north, south }",
        )
        .expect("enum module should write");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                local directions = require("./directions")
                function score(direction: directions.Direction): i32
                    match direction do
                    case directions.Direction.north then
                        return 1
                    end
                end
            "#,
        )
        .expect("main module should write");

        let error = super::compile_file(&input_path)
            .expect_err("imported enum match should remain exhaustive")
            .to_string();
        assert!(
            error.contains("non-exhaustive match for enum 'directions.Direction'")
                && error.contains("directions.Direction.south"),
            "{error}"
        );
    }

    #[test]
    fn compile_file_rejects_effectful_module_constant_at_declaration() {
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
            "top-level const 'SIZE' initializer must be a side-effect-free expression over literals and earlier constants"
        );
        assert!(
            error.span().is_some(),
            "diagnostic should point at the initializer"
        );
    }

    #[test]
    fn compile_file_supports_computed_module_constants() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                const BASE: i32 = 3
                const SQUARED: i32 = BASE ^ 2
                const RESULT: i32 = -(SQUARED * 5 + BASE) // 2

                function result(): i32
                    return RESULT
                end

                assert(result() == -24)
            "#,
        )
        .expect("fixture should write");

        let wasm = super::compile_file(&input_path).expect("computed constants should compile");
        assert!(!wasm.is_empty());
    }

    #[test]
    fn compile_file_rejects_module_constant_cycles() {
        let tempdir = tempdir().expect("tempdir should exist");
        let input_path = tempdir.path().join("main.walu");
        fs::write(
            &input_path,
            r#"
                const FIRST: i32 = SECOND + 1
                const SECOND: i32 = FIRST + 1
            "#,
        )
        .expect("fixture should write");

        let error = super::compile_file(&input_path).expect_err("constant cycle should fail");
        assert_eq!(
            error.to_string(),
            "top-level const cycle detected: 'FIRST' -> 'SECOND' -> 'FIRST'"
        );
        assert!(
            error.span().is_some(),
            "diagnostic should point at the cycle"
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
