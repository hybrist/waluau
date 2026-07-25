//! Module linking: resolves a graph of `require`-connected `.walu` files into a
//! single [`Program`] that the rest of the pipeline can compile unchanged.
//!
//! The strategy keeps the later compiler stages module-unaware:
//!
//! 1. Starting from an entry file, every `require("./path")` is resolved
//!    relative to the requiring file and loaded recursively, with cycle
//!    detection. Filesystem paths must start with `./` or `../`; bare
//!    specifiers are reserved for supported virtual modules.
//! 2. Each non-entry module's top-level functions are renamed with a unique,
//!    per-module prefix so names from different files cannot collide. The entry
//!    module keeps its original names so its Wasm exports stay stable.
//! 3. Every `require(...)` node is replaced with either the imported function
//!    (single export) or a table of mangled function references (namespace
//!    export). `m.field` member access on namespace locals is rewritten to the
//!    corresponding mangled function name. Resolution is static regardless of
//!    the require expression's lexical position, so requires inside functions
//!    do not perform runtime loading.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use waluau_ast::{
    DeclaredConstant, DeclaredImport, Expr, Function, FunctionExpr, FunctionName, Program, Stmt,
    TableField, Type, TypeDeclaration,
};
use waluau_diagnostics::Diagnostic;

const DOM_WINDOW_REQUIRE: &str = "dom:window";
const DOM_WINDOW_FUNCTION: &str = "dom_window";
const DOM_WINDOW_TYPE: &str = "Window";
const TFJS_REQUIRE: &str = "tfjs";
const ENGINE_REQUIRE: &str = "waluau:engine";
const VITEST_REQUIRE: &str = "waluau:vitest";

/// Resolve the module graph rooted at `entry` and merge it into one program.
/// Single-first-error contract retained for linker tests; the compile
/// pipeline goes through [`link_program_collect`].
#[cfg_attr(not(test), allow(dead_code))]
pub fn link_program(entry: &Path) -> Result<Program, Diagnostic> {
    let (program, mut diagnostics) = link_program_collect(entry)?;
    match diagnostics.len() {
        0 => Ok(program),
        1 => Err(diagnostics.remove(0)),
        _ => Err(Diagnostic::new(
            diagnostics
                .iter()
                .map(Diagnostic::render_for_playground)
                .collect::<Vec<_>>()
                .join("\n"),
        )),
    }
}

/// Source of parsed modules for the linker. The default filesystem provider
/// reads and parses on every call; a [`crate::session::CompilerSession`]
/// serves in-memory overlays and content-hash-cached parses instead.
pub trait ModuleProvider {
    /// Return the (recovered) parsed program for `path` plus its parse
    /// diagnostics. `Err` is reserved for hard failures such as unreadable
    /// files.
    fn parsed_module(&mut self, path: &Path) -> Result<(Program, Vec<Diagnostic>), Diagnostic>;
}

/// Stateless provider that reads from the filesystem and re-parses each call.
pub struct FsModules;

impl ModuleProvider for FsModules {
    fn parsed_module(&mut self, path: &Path) -> Result<(Program, Vec<Diagnostic>), Diagnostic> {
        let source = std::fs::read_to_string(path).map_err(|error| {
            Diagnostic::new(format!("read module `{}`: {error}", path.display()))
        })?;
        let outcome = waluau_parser::parse_with_recovery(&source, &path.to_string_lossy());
        Ok((outcome.program, outcome.diagnostics))
    }
}

/// Everything the linker learned about a module graph: the merged program,
/// collected parse diagnostics, and the set of source files involved (for
/// watch-mode invalidation; builtins/externs/engine modules are excluded).
pub struct LinkOutcome {
    pub program: Program,
    pub diagnostics: Vec<Diagnostic>,
    pub involved_files: Vec<PathBuf>,
}

/// Like [`link_program`], but recovers from module parse errors: every parse
/// diagnostic across the module graph is collected while traversal continues,
/// and the merged (possibly partial) program is returned alongside them.
/// Hard failures that prevent building a graph at all (unreadable entry,
/// import cycles) remain `Err`.
pub fn link_program_collect(entry: &Path) -> Result<(Program, Vec<Diagnostic>), Diagnostic> {
    let outcome = link_program_collect_with(entry, &mut FsModules)?;
    Ok((outcome.program, outcome.diagnostics))
}

pub fn link_program_collect_with(
    entry: &Path,
    provider: &mut dyn ModuleProvider,
) -> Result<LinkOutcome, Diagnostic> {
    let entry = entry.canonicalize().map_err(|error| {
        Diagnostic::new(format!(
            "cannot open input file `{}`: {error}",
            entry.display()
        ))
    })?;
    let mut loader = Loader::new(provider);

    // Load builtin declarations first
    let (builtin_imports, builtin_constants) = loader.load_builtins()?;

    let entry_id = loader.load(&entry)?;
    let dom_externs = if loader.requires_dom_externs {
        Some(loader.load_dom_externs()?)
    } else {
        None
    };
    let tfjs_externs = if loader.requires_tfjs_externs {
        Some(loader.load_tfjs_externs()?)
    } else {
        None
    };
    let vitest_externs = if loader.requires_vitest_externs {
        Some(loader.load_vitest_externs()?)
    } else {
        None
    };
    // Only real files belong in the involved set: engine modules register
    // under `/@waluau/...` pseudo-paths (their sources are embedded in the
    // compiler binary) and are useless to a file watcher.
    let mut involved_files: Vec<PathBuf> = loader
        .by_path
        .keys()
        .filter(|path| path.is_file())
        .cloned()
        .collect();
    involved_files.sort();
    match merge_with_builtins(
        &loader.modules,
        entry_id,
        builtin_imports,
        builtin_constants,
        dom_externs,
        tfjs_externs,
        vitest_externs,
    ) {
        Ok(program) => Ok(LinkOutcome {
            program,
            diagnostics: loader.diagnostics,
            involved_files,
        }),
        // A recovered (partial) AST can break merging in misleading ways —
        // e.g. a module whose `return` export failed to parse reports
        // "module has no export". The parse errors are the real story, so
        // surface them with the unmerged entry program instead.
        Err(_) if !loader.diagnostics.is_empty() => Ok(LinkOutcome {
            program: loader.modules[entry_id].program.clone(),
            diagnostics: loader.diagnostics,
            involved_files,
        }),
        Err(error) => Err(error),
    }
}

struct LoadedModule {
    program: Program,
    /// Raw `require` path strings to the module id they resolve to.
    requires: HashMap<String, usize>,
    /// Raw virtual extern module specifiers that do not resolve to source files.
    virtual_requires: HashSet<String>,
}

struct Loader<'a> {
    provider: &'a mut dyn ModuleProvider,
    modules: Vec<LoadedModule>,
    by_path: HashMap<PathBuf, usize>,
    stack: Vec<PathBuf>,
    requires_dom_externs: bool,
    requires_tfjs_externs: bool,
    requires_vitest_externs: bool,
    /// Parse diagnostics collected across the module graph; traversal
    /// continues past modules with syntax errors using their recovered ASTs.
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Loader<'a> {
    fn new(provider: &'a mut dyn ModuleProvider) -> Self {
        Self {
            provider,
            modules: Vec::new(),
            by_path: HashMap::new(),
            stack: Vec::new(),
            requires_dom_externs: false,
            requires_tfjs_externs: false,
            requires_vitest_externs: false,
            diagnostics: Vec::new(),
        }
    }

    fn load(&mut self, path: &Path) -> Result<usize, Diagnostic> {
        if let Some(&id) = self.by_path.get(path) {
            return Ok(id);
        }
        if self.stack.iter().any(|entry| entry == path) {
            let chain = self
                .stack
                .iter()
                .chain(std::iter::once(&path.to_path_buf()))
                .map(|entry| entry.display().to_string())
                .collect::<Vec<_>>()
                .join(" -> ");
            return Err(Diagnostic::new(format!("circular module import: {chain}")));
        }

        let (program, diagnostics) = self.provider.parsed_module(path)?;
        self.diagnostics.extend(diagnostics);
        let dir = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        let mut raw_paths = Vec::new();
        collect_require_paths(&program, &mut raw_paths);

        self.stack.push(path.to_path_buf());
        let mut requires = HashMap::new();
        let mut virtual_requires = HashSet::new();
        for raw in raw_paths {
            if engine_module_name(&raw).is_some() {
                let target = self.load_engine(&raw)?;
                requires.insert(raw, target);
                continue;
            }
            if raw == DOM_WINDOW_REQUIRE {
                self.requires_dom_externs = true;
                virtual_requires.insert(raw);
                continue;
            }
            if raw == TFJS_REQUIRE {
                self.requires_tfjs_externs = true;
                virtual_requires.insert(raw);
                continue;
            }
            if raw == VITEST_REQUIRE {
                self.requires_vitest_externs = true;
                virtual_requires.insert(raw);
                continue;
            }
            if raw.starts_with("dom:") {
                return Err(unsupported_dom_require(&raw));
            }
            if is_unsupported_virtual_require(&raw) {
                return Err(unsupported_virtual_require(&raw));
            }
            if raw.starts_with("waluau:") {
                return Err(unsupported_virtual_require(&raw));
            }
            if requires.contains_key(&raw) {
                continue;
            }
            let resolved = resolve_module_path(&dir, &raw)?;
            let target = self.load(&resolved)?;
            requires.insert(raw, target);
        }
        self.stack.pop();

        let id = self.modules.len();
        self.modules.push(LoadedModule {
            program,
            requires,
            virtual_requires,
        });
        self.by_path.insert(path.to_path_buf(), id);
        Ok(id)
    }

    fn load_engine(&mut self, specifier: &str) -> Result<usize, Diagnostic> {
        let module =
            engine_module_name(specifier).ok_or_else(|| unsupported_virtual_require(specifier))?;
        let key = PathBuf::from(format!("/@waluau/engine/v1/{module}.walu"));
        if let Some(&id) = self.by_path.get(&key) {
            return Ok(id);
        }
        if self.stack.iter().any(|entry| entry == &key) {
            return Err(Diagnostic::new(format!(
                "circular module import: {}",
                self.stack
                    .iter()
                    .chain(std::iter::once(&key))
                    .map(|entry| entry.display().to_string())
                    .collect::<Vec<_>>()
                    .join(" -> ")
            )));
        }

        let source = engine_module_source(module).expect("validated engine module name");
        let display_path = format!("package:waluau-engine/v1/{module}.walu");
        let program = waluau_parser::parse_with_path(source, &display_path)?;
        let mut raw_paths = Vec::new();
        collect_require_paths(&program, &mut raw_paths);

        self.stack.push(key.clone());
        let mut requires = HashMap::new();
        let mut virtual_requires = HashSet::new();
        for raw in raw_paths {
            if engine_module_name(&raw).is_some() {
                let target = self.load_engine(&raw)?;
                requires.insert(raw, target);
                continue;
            }
            if raw.starts_with("./") {
                let target_specifier = engine_relative_specifier(&raw)?;
                let target = self.load_engine(&target_specifier)?;
                requires.insert(raw, target);
                continue;
            }
            if raw == DOM_WINDOW_REQUIRE {
                self.requires_dom_externs = true;
                virtual_requires.insert(raw);
                continue;
            }
            if raw == TFJS_REQUIRE {
                self.requires_tfjs_externs = true;
                virtual_requires.insert(raw);
                continue;
            }
            return Err(Diagnostic::new(format!(
                "engine package module '{module}' has unsupported require \"{raw}\""
            )));
        }
        self.stack.pop();

        let id = self.modules.len();
        self.modules.push(LoadedModule {
            program,
            requires,
            virtual_requires,
        });
        self.by_path.insert(key, id);
        Ok(id)
    }

    fn load_builtins(
        &mut self,
    ) -> Result<(Vec<DeclaredImport>, Vec<DeclaredConstant>), Diagnostic> {
        // Load builtin declaration files and extract their declared imports
        // and constants.
        let builtin_files = ["core.walu", "math.walu"];
        let mut all_imports = Vec::new();
        let mut all_constants = Vec::new();

        for filename in &builtin_files {
            let builtin_source = match *filename {
                "core.walu" => include_str!("../../../builtins/core.walu"),
                "math.walu" => include_str!("../../../builtins/math.walu"),
                _ => continue,
            };

            let program =
                waluau_parser::parse_with_path(builtin_source, &format!("builtin:{filename}"))?;
            all_imports.extend(program.declared_imports);
            all_constants.extend(program.declared_constants);
        }

        Ok((all_imports, all_constants))
    }

    fn load_dom_externs(&mut self) -> Result<Program, Diagnostic> {
        waluau_parser::parse_with_path(
            include_str!("../../../externs/dom.walu"),
            "externs/dom.walu",
        )
    }

    fn load_tfjs_externs(&mut self) -> Result<Program, Diagnostic> {
        waluau_parser::parse_with_path(
            include_str!("../../../externs/tfjs.walu"),
            "externs/tfjs.walu",
        )
    }

    fn load_vitest_externs(&mut self) -> Result<Program, Diagnostic> {
        waluau_parser::parse_with_path(
            include_str!("../../../externs/vitest.walu"),
            "externs/vitest.walu",
        )
    }
}

fn unsupported_dom_require(raw: &str) -> Diagnostic {
    Diagnostic::new(format!(
        "unsupported DOM virtual module \"{raw}\"; supported specifiers: \"{DOM_WINDOW_REQUIRE}\""
    ))
}

fn is_unsupported_virtual_require(raw: &str) -> bool {
    raw.starts_with("tf") || raw.starts_with("tensorflow")
}

fn unsupported_virtual_require(raw: &str) -> Diagnostic {
    Diagnostic::new(format!(
        "unsupported virtual module \"{raw}\"; supported specifiers: \"{DOM_WINDOW_REQUIRE}\", \"{TFJS_REQUIRE}\", \"{ENGINE_REQUIRE}\", \"{VITEST_REQUIRE}\""
    ))
}

fn engine_module_name(specifier: &str) -> Option<&str> {
    match specifier {
        "waluau:engine" | "waluau:engine/v1" => Some("init"),
        "waluau:engine/browser" | "waluau:engine/v1/browser" => Some("browser"),
        "waluau:engine/input" | "waluau:engine/v1/input" => Some("input"),
        "waluau:engine/graphics" | "waluau:engine/v1/graphics" => Some("graphics"),
        "waluau:engine/resources" | "waluau:engine/v1/resources" => Some("resources"),
        "waluau:engine/audio" | "waluau:engine/v1/audio" => Some("audio"),
        "waluau:engine/time" | "waluau:engine/v1/time" => Some("time"),
        "waluau:engine/font" | "waluau:engine/v1/font" => Some("font"),
        "waluau:engine/hot" | "waluau:engine/v1/hot" => Some("hot"),
        "waluau:engine/particles" | "waluau:engine/v1/particles" => Some("particles"),
        "waluau:engine/shader_sources" | "waluau:engine/v1/shader_sources" => {
            Some("shader_sources")
        }
        _ => None,
    }
}

fn engine_relative_specifier(raw: &str) -> Result<String, Diagnostic> {
    let module = raw
        .strip_prefix("./")
        .and_then(|path| path.strip_suffix(".walu").or(Some(path)))
        .filter(|path| !path.is_empty() && !path.contains('/'))
        .ok_or_else(|| Diagnostic::new(format!("invalid engine package require \"{raw}\"")))?;
    let specifier = format!("waluau:engine/v1/{module}");
    if engine_module_name(&specifier).is_some() {
        Ok(specifier)
    } else {
        Err(Diagnostic::new(format!(
            "unknown engine package module \"{raw}\""
        )))
    }
}

fn engine_module_source(module: &str) -> Option<&'static str> {
    match module {
        "init" => Some(include_str!("../../../engine/init.walu")),
        "browser" => Some(include_str!("../../../engine/browser.walu")),
        "input" => Some(include_str!("../../../engine/input.walu")),
        "graphics" => Some(include_str!("../../../engine/graphics.walu")),
        "resources" => Some(include_str!("../../../engine/resources.walu")),
        "audio" => Some(include_str!("../../../engine/audio.walu")),
        "time" => Some(include_str!("../../../engine/time.walu")),
        "font" => Some(include_str!("../../../engine/font.walu")),
        "hot" => Some(include_str!("../../../engine/hot.walu")),
        "particles" => Some(include_str!("../../../engine/particles.walu")),
        "shader_sources" => Some(include_str!("../../../engine/shader_sources.walu")),
        _ => None,
    }
}

fn resolve_module_path(dir: &Path, raw: &str) -> Result<PathBuf, Diagnostic> {
    if !(raw.starts_with("./") || raw.starts_with("../")) {
        return Err(Diagnostic::new(format!(
            "require path must be relative and start with './' or '../', got \"{raw}\""
        )));
    }
    let mut candidate = dir.join(raw);
    if candidate.extension().is_none() {
        candidate.set_extension("walu");
    }
    candidate
        .canonicalize()
        .map_err(|error| Diagnostic::new(format!("cannot resolve module \"{raw}\": {error}")))
}

fn module_prefix(id: usize, entry_id: usize) -> String {
    if id == entry_id {
        String::new()
    } else {
        format!("__waluau_m{id}_")
    }
}

fn merge_with_builtins(
    modules: &[LoadedModule],
    entry_id: usize,
    builtin_imports: Vec<DeclaredImport>,
    builtin_constants: Vec<DeclaredConstant>,
    dom_externs: Option<Program>,
    tfjs_externs: Option<Program>,
    vitest_externs: Option<Program>,
) -> Result<Program, Diagnostic> {
    let mut functions = Vec::new();
    let mut declared_imports = builtin_imports;
    let mut declared_constants = builtin_constants;
    let mut type_declarations = Vec::new();
    let mut extern_sources = BTreeMap::new();
    if let Some(dom_program) = dom_externs {
        declared_imports.extend(dom_program.declared_imports);
        extend_unique_type_declarations(&mut type_declarations, dom_program.type_declarations)?;
        extern_sources.extend(dom_program.sources);
    }
    if let Some(tfjs_program) = tfjs_externs {
        declared_imports.extend(tfjs_program.declared_imports);
        extend_unique_type_declarations(&mut type_declarations, tfjs_program.type_declarations)?;
        extern_sources.extend(tfjs_program.sources);
    }
    if let Some(vitest_program) = vitest_externs {
        declared_imports.extend(vitest_program.declared_imports);
        extend_unique_type_declarations(&mut type_declarations, vitest_program.type_declarations)?;
        extern_sources.extend(vitest_program.sources);
    }
    let mut top_level = Vec::new();
    let mut export_cache = HashMap::new();

    for (id, _) in modules.iter().enumerate() {
        if id != entry_id {
            compute_module_export(modules, id, entry_id, &mut export_cache)?;
        }
    }

    for (id, module) in modules.iter().enumerate() {
        let prefix = module_prefix(id, entry_id);
        let mut module_functions = module.program.functions.clone();
        if let Some(export) = &module.program.export {
            hoist_table_export_functions(&mut module_functions, export)?;
        }
        let func_names: HashSet<String> = module_functions
            .iter()
            .map(|function| function.name.to_string())
            .collect();
        let type_names: HashSet<String> = module
            .program
            .type_declarations
            .iter()
            .map(|decl| decl.name.clone())
            .collect();

        let mut imports = HashMap::new();
        for (raw, &target_id) in &module.requires {
            imports.insert(raw.clone(), export_cache[&target_id].clone());
        }
        for raw in &module.virtual_requires {
            imports.insert(raw.clone(), resolve_virtual_import(raw)?);
        }

        let (re_exports, namespaces, mut value_aliases) =
            process_reexport_bindings(&module.program.top_level, &imports);
        // The module's own constants inline wherever the name is visible but
        // the top-level local is not — most importantly function bodies.
        // Top-level statements after the `local` keep using the local itself
        // (the binding gates the alias), which is equivalent: the initializer
        // is a literal and const locals cannot be rebound.
        value_aliases.extend(module_constants(&module.program.top_level)?);
        let type_namespaces = module_type_namespaces(modules, module, entry_id);

        let mut rewriter = Rewriter {
            prefix: &prefix,
            func_names: &func_names,
            type_names: &type_names,
            type_namespaces: &type_namespaces,
            imports: &imports,
            re_exports,
            namespaces,
            value_aliases,
        };

        for decl in &module.program.type_declarations {
            let mut lowered = decl.clone();
            rewriter.rewrite_type(&mut lowered.ty);
            lowered.name = format!("{prefix}{}", lowered.name);
            type_declarations.push(lowered);
        }

        // Collect declared imports and constants from all modules (mainly
        // builtins)
        for import in &module.program.declared_imports {
            let mut lowered = import.clone();
            rewriter.rewrite_declared_import_types(&mut lowered);
            declared_imports.push(lowered);
        }
        for constant in &module.program.declared_constants {
            let mut lowered = constant.clone();
            rewriter.rewrite_type(&mut lowered.ty);
            declared_constants.push(lowered);
        }

        for function in &module_functions {
            let mut lowered = function.clone();
            rewriter.rewrite_function_types(&mut lowered);
            let mut bound: HashSet<String> = lowered
                .params
                .iter()
                .map(|param| param.name.clone())
                .collect();
            rewriter.rewrite_block(&mut lowered.body, &mut bound);
            strip_unused_namespace_lets(&mut lowered.body);
            lowered.name = match &function.name {
                FunctionName::Simple(name) => FunctionName::Simple(format!("{prefix}{name}")),
                FunctionName::Method { table, method } => FunctionName::Method {
                    table: format!("{prefix}{table}"),
                    method: method.clone(),
                },
            };
            functions.push(lowered);
        }

        let mut lowered = module.program.top_level.clone();
        for stmt in &mut lowered {
            rewriter.rewrite_stmt_types(stmt);
        }
        let mut bound = HashSet::new();
        rewriter.rewrite_block(&mut lowered, &mut bound);
        if id != entry_id {
            rename_imported_top_level_locals(&mut lowered, &prefix);
        }
        strip_unused_namespace_lets(&mut lowered);
        top_level.extend(lowered);
    }
    let entry_file_path = modules[entry_id].program.entry_file_path.clone();
    let mut sources = extern_sources;
    for module in modules {
        sources.extend(module.program.sources.clone());
    }

    Ok(Program {
        functions,
        declared_imports,
        declared_constants,
        type_declarations,
        top_level,
        // A trailing return is dependency-facing module metadata, not the
        // entry module's Wasm export declaration. Inline functions from every
        // module export were hoisted above; discard the entry expression so
        // later stages cannot mistake it for executable entry-point work.
        export: None,
        sources,
        entry_file_path,
    })
}

fn extend_unique_type_declarations(
    target: &mut Vec<TypeDeclaration>,
    declarations: Vec<TypeDeclaration>,
) -> Result<(), Diagnostic> {
    for declaration in declarations {
        if let Some(existing) = target
            .iter()
            .find(|existing| existing.name == declaration.name)
        {
            if existing == &declaration {
                continue;
            }
            return Err(Diagnostic::new(format!(
                "conflicting ambient type declaration '{}'",
                declaration.name
            )));
        }
        target.push(declaration);
    }
    Ok(())
}

#[derive(Clone)]
enum ResolvedImport {
    Function(String),
    Namespace(ModuleNamespace),
    DomWindow,
}

/// A module's table export: fields mapping to (mangled) function names, plus
/// fields mapping to constant literals (top-level `local NAME <const> =
/// <literal>` bindings), which member accesses inline.
#[derive(Clone, Debug, Default)]
struct ModuleNamespace {
    functions: BTreeMap<String, String>,
    constants: BTreeMap<String, Expr>,
}

impl ModuleNamespace {
    fn from_functions(functions: BTreeMap<String, String>) -> Self {
        Self {
            functions,
            constants: BTreeMap::new(),
        }
    }
}

type RequireAliases = (
    HashMap<String, String>,
    HashMap<String, ModuleNamespace>,
    HashMap<String, Expr>,
);

fn resolve_virtual_import(raw: &str) -> Result<ResolvedImport, Diagnostic> {
    match raw {
        DOM_WINDOW_REQUIRE => Ok(ResolvedImport::DomWindow),
        TFJS_REQUIRE => Ok(ResolvedImport::Namespace(ModuleNamespace::from_functions(
            tfjs_namespace(),
        ))),
        VITEST_REQUIRE => Ok(ResolvedImport::Namespace(ModuleNamespace::from_functions(
            vitest_namespace(),
        ))),
        _ => Err(unsupported_virtual_require(raw)),
    }
}

// The vitest test API (externs/vitest.walu) is usable both through this
// namespace (`local t = require("waluau:vitest")` then `t.describe(...)`)
// and as busted-style bare globals, since the module's declared imports
// merge program-wide once any file requires it.
fn vitest_namespace() -> BTreeMap<String, String> {
    [
        "describe",
        "it",
        "test",
        "xdescribe",
        "xit",
        "todo",
        "before_each",
        "after_each",
        "before_all",
        "after_all",
        "expect",
    ]
    .into_iter()
    .map(|name| (name.to_string(), name.to_string()))
    .collect()
}

fn dom_window_expr(span: Option<waluau_ast::Span>) -> Expr {
    Expr::Cast {
        expr: Box::new(Expr::Call {
            callee: Box::new(Expr::Name(DOM_WINDOW_FUNCTION.to_string(), None, span)),
            type_args: Vec::new(),
            args: Vec::new(),
            span,
            method_call_origin: None,
        }),
        ty: Type::Named {
            name: DOM_WINDOW_TYPE.to_string(),
            type_args: Vec::new(),
        },
        span,
    }
}

fn tfjs_namespace() -> BTreeMap<String, String> {
    [
        ("data_empty", "tfjs_data_empty"),
        ("data_set_f64", "tfjs_data_set_f64"),
        ("data_set_i32", "tfjs_data_set_i32"),
        ("data_len", "tfjs_data_len"),
        ("data_get_f64", "tfjs_data_get_f64"),
        ("data_get_i32", "tfjs_data_get_i32"),
        ("scalar", "tfjs_scalar"),
        ("scalar_i32", "tfjs_scalar_i32"),
        ("scalar_bool", "tfjs_scalar_bool"),
        ("tensor1d", "tfjs_tensor1d"),
        ("tensor1d_i32", "tfjs_tensor1d_i32"),
        ("tensor2d", "tfjs_tensor2d"),
        ("tensor2d_i32", "tfjs_tensor2d_i32"),
        ("zeros", "tfjs_zeros"),
        ("ones", "tfjs_ones"),
        ("eye", "tfjs_eye"),
        ("data", "tfjs_data"),
        ("data_sync", "tfjs_data_sync"),
        ("scalar_value", "tfjs_scalar_value"),
        ("scalar_value_i32", "tfjs_scalar_value_i32"),
        ("shape_rank", "tfjs_shape_rank"),
        ("shape_dim", "tfjs_shape_dim"),
        ("dtype", "tfjs_dtype"),
        ("dispose", "tfjs_dispose"),
        ("keep", "tfjs_keep"),
        ("tidy", "tfjs_tidy"),
        ("memory_num_tensors", "tfjs_memory_num_tensors"),
        ("add", "tfjs_add"),
        ("sub", "tfjs_sub"),
        ("mul", "tfjs_mul"),
        ("div", "tfjs_div"),
        ("neg", "tfjs_neg"),
        ("matmul", "tfjs_matmul"),
        ("reshape2d", "tfjs_reshape2d"),
        ("transpose", "tfjs_transpose"),
        ("load_graph_model", "tfjs_load_graph_model"),
        ("load_layers_model", "tfjs_load_layers_model"),
        ("dispose_graph_model", "tfjs_dispose_graph_model"),
        ("dispose_layers_model", "tfjs_dispose_layers_model"),
        ("graph_model_predict", "tfjs_graph_model_predict"),
        (
            "graph_model_predict_async",
            "tfjs_graph_model_predict_async",
        ),
        ("graph_model_execute", "tfjs_graph_model_execute"),
        ("layers_model_predict", "tfjs_layers_model_predict"),
        ("layers_model_compile_sgd", "tfjs_layers_model_compile_sgd"),
        ("layers_model_fit_one", "tfjs_layers_model_fit_one"),
        ("training_history_len", "tfjs_training_history_len"),
        ("training_history_loss", "tfjs_training_history_loss"),
        ("graph_model_input_count", "tfjs_graph_model_input_count"),
        ("graph_model_output_count", "tfjs_graph_model_output_count"),
        ("layers_model_input_count", "tfjs_layers_model_input_count"),
        (
            "layers_model_output_count",
            "tfjs_layers_model_output_count",
        ),
    ]
    .into_iter()
    .map(|(field, function)| (field.to_string(), function.to_string()))
    .collect()
}

fn hoist_table_export_functions(
    functions: &mut Vec<Function>,
    export: &Expr,
) -> Result<(), Diagnostic> {
    let Expr::TableLiteral { fields, .. } = export else {
        return Ok(());
    };
    for field in fields {
        if let Expr::Function(function) = &field.value {
            functions.push(function_expr_to_function(&field.name, function));
        }
    }
    Ok(())
}

fn function_expr_to_function(name: &str, function: &FunctionExpr) -> Function {
    Function {
        name: waluau_ast::FunctionName::Simple(name.to_string()),
        symbol_id: function.symbol_id,
        type_params: function.type_params.clone(),
        params: function.params.clone(),
        vararg: function.vararg,
        return_type: function.return_type.clone(),
        body: function.body.clone(),
        file_path: function.file_path.clone(),
    }
}

/// Collects a module's constants: top-level `local NAME <const> = <literal>`
/// bindings. Their values are inlined wherever the name is referenced from a
/// function body (top-level locals are otherwise invisible there) and
/// wherever a consumer reads them off the module's export table. A numeric
/// literal keeps its annotated type through an explicit cast.
fn module_constants(top_level: &[Stmt]) -> Result<HashMap<String, Expr>, Diagnostic> {
    let mut constants = HashMap::new();
    for stmt in top_level {
        let Stmt::Let {
            name,
            rebindability: waluau_ast::Rebindability::Const,
            ty,
            value,
            ..
        } = stmt
        else {
            continue;
        };
        let literal = constant_literal(ty.as_ref(), value).ok_or_else(|| {
            let diagnostic = Diagnostic::new(format!(
                "top-level const '{name}' initializer must be an inlinable literal"
            ));
            match value.span() {
                Some(span) => diagnostic.with_span(span),
                None => diagnostic,
            }
        })?;
        constants.insert(name.clone(), literal);
    }
    Ok(constants)
}

/// The inlinable expression for a constant initializer, or `None` when the
/// initializer is not a literal (only literals can be duplicated freely).
fn constant_literal(ty: Option<&Type>, value: &Expr) -> Option<Expr> {
    match value {
        Expr::Number(..) => numeric_constant_literal(ty, value),
        Expr::Unary {
            op: waluau_ast::UnaryOp::Neg,
            expr,
            ..
        } if matches!(&**expr, Expr::Number(..)) => numeric_constant_literal(ty, value),
        Expr::Bool(..) | Expr::String(..) => Some(value.clone()),
        _ => None,
    }
}

fn numeric_constant_literal(ty: Option<&Type>, value: &Expr) -> Option<Expr> {
    match ty {
        // Keep the annotated numeric type: a bare literal would default to f64
        // in unconstrained positions.
        Some(ty @ Type::Numeric(_)) => Some(Expr::Cast {
            expr: Box::new(value.clone()),
            ty: ty.clone(),
            span: None,
        }),
        None => Some(value.clone()),
        Some(_) => None,
    }
}

fn compute_module_export(
    modules: &[LoadedModule],
    id: usize,
    entry_id: usize,
    cache: &mut HashMap<usize, ResolvedImport>,
) -> Result<ResolvedImport, Diagnostic> {
    if let Some(resolved) = cache.get(&id) {
        return Ok(resolved.clone());
    }

    let module = &modules[id];
    let prefix = module_prefix(id, entry_id);

    let mut imports = HashMap::new();
    for (raw, &target_id) in &module.requires {
        let resolved = compute_module_export(modules, target_id, entry_id, cache)?;
        imports.insert(raw.clone(), resolved);
    }

    let mut module_functions = module.program.functions.clone();
    if let Some(export) = &module.program.export {
        hoist_table_export_functions(&mut module_functions, export)?;
    }
    let top_level_names = module_function_names(&module_functions, &module.program.export);
    let (re_exports, namespaces, _) =
        process_reexport_bindings(&module.program.top_level, &imports);
    let constants = module_constants(&module.program.top_level)?;

    let resolved = resolve_module_export(
        module.program.export.as_ref(),
        &prefix,
        &top_level_names,
        &re_exports,
        &namespaces,
        &constants,
    )?;

    cache.insert(id, resolved.clone());
    Ok(resolved)
}

fn module_function_names(functions: &[Function], export: &Option<Expr>) -> HashSet<String> {
    let mut names: HashSet<String> = functions
        .iter()
        .map(|function| function.name.to_string())
        .collect();
    if let Some(Expr::TableLiteral { fields, .. }) = export {
        for field in fields {
            if matches!(field.value, Expr::Function(_)) {
                names.insert(field.name.clone());
            }
        }
    }
    names
}

/// Maps each require binding's exported type names to their canonical linked
/// declarations. A direct re-export such as `type Input = input.Input` keeps
/// pointing at the subsystem's declaration instead of gaining a second,
/// nominal aggregate-module identity.
fn module_type_namespaces(
    modules: &[LoadedModule],
    module: &LoadedModule,
    entry_id: usize,
) -> HashMap<String, HashMap<String, String>> {
    let mut type_namespaces = HashMap::new();
    let mut cache = HashMap::new();
    for stmt in &module.program.top_level {
        let Stmt::Let {
            name,
            value: Expr::Require(path, _),
            ..
        } = stmt
        else {
            continue;
        };
        let Some(&target_id) = module.requires.get(path) else {
            continue;
        };
        let types = exported_type_names(modules, target_id, entry_id, &mut cache);
        type_namespaces.insert(name.clone(), types);
    }
    type_namespaces
}

fn exported_type_names(
    modules: &[LoadedModule],
    module_id: usize,
    entry_id: usize,
    cache: &mut HashMap<usize, HashMap<String, String>>,
) -> HashMap<String, String> {
    if let Some(types) = cache.get(&module_id) {
        return types.clone();
    }

    let module = &modules[module_id];
    let prefix = module_prefix(module_id, entry_id);
    let mut types = module
        .program
        .type_declarations
        .iter()
        .map(|decl| (decl.name.clone(), format!("{prefix}{}", decl.name)))
        .collect::<HashMap<_, _>>();
    cache.insert(module_id, types.clone());

    let require_bindings = module
        .program
        .top_level
        .iter()
        .filter_map(|stmt| {
            let Stmt::Let {
                name,
                value: Expr::Require(path, _),
                ..
            } = stmt
            else {
                return None;
            };
            module
                .requires
                .get(path)
                .map(|target| (name.as_str(), *target))
        })
        .collect::<HashMap<_, _>>();

    for declaration in &module.program.type_declarations {
        let Type::Named { name, type_args } = &declaration.ty else {
            continue;
        };
        if !type_args.is_empty() {
            continue;
        }
        let Some((namespace, member)) = name.split_once('.') else {
            continue;
        };
        let Some(&target_id) = require_bindings.get(namespace) else {
            continue;
        };
        let target_types = exported_type_names(modules, target_id, entry_id, cache);
        if let Some(canonical) = target_types.get(member) {
            types.insert(declaration.name.clone(), canonical.clone());
        }
    }

    cache.insert(module_id, types.clone());
    types
}

fn process_reexport_bindings(
    top_level: &[Stmt],
    imports: &HashMap<String, ResolvedImport>,
) -> RequireAliases {
    let empty = HashSet::new();
    let empty_type_namespaces = HashMap::new();
    let mut rewriter = Rewriter {
        prefix: "",
        func_names: &empty,
        type_names: &empty,
        type_namespaces: &empty_type_namespaces,
        imports,
        re_exports: HashMap::new(),
        namespaces: HashMap::new(),
        value_aliases: HashMap::new(),
    };
    let mut stmts = top_level.to_vec();
    let mut bound = HashSet::new();
    rewriter.rewrite_block(&mut stmts, &mut bound);
    (
        rewriter.re_exports,
        rewriter.namespaces,
        rewriter.value_aliases,
    )
}

fn resolve_module_export(
    export: Option<&Expr>,
    prefix: &str,
    top_level_names: &HashSet<String>,
    re_exports: &HashMap<String, String>,
    namespaces: &HashMap<String, ModuleNamespace>,
    constants: &HashMap<String, Expr>,
) -> Result<ResolvedImport, Diagnostic> {
    match export {
        Some(Expr::Name(name, _, _)) => Ok(ResolvedImport::Function(export_function_name(
            name,
            prefix,
            top_level_names,
            re_exports,
            "module export",
        )?)),
        Some(Expr::TableLiteral { fields, .. }) => {
            let mut namespace = ModuleNamespace::default();
            for field in fields {
                match export_field_value(
                    field,
                    prefix,
                    top_level_names,
                    re_exports,
                    namespaces,
                    constants,
                )? {
                    ExportedField::Function(function_name) => {
                        namespace
                            .functions
                            .insert(field.name.clone(), function_name);
                    }
                    ExportedField::Constant(value) => {
                        namespace.constants.insert(field.name.clone(), value);
                    }
                }
            }
            if namespace.functions.is_empty() && namespace.constants.is_empty() {
                return Err(Diagnostic::new("module exports an empty table"));
            }
            Ok(ResolvedImport::Namespace(namespace))
        }
        Some(_) => Err(Diagnostic::new(
            "module must export a function name or table of functions",
        )),
        None => Err(Diagnostic::new(
            "module has no export; add `return <function>` or `return { ... }`",
        )),
    }
}

fn export_function_name(
    name: &str,
    prefix: &str,
    top_level_names: &HashSet<String>,
    re_exports: &HashMap<String, String>,
    context: &str,
) -> Result<String, Diagnostic> {
    if let Some(mangled) = re_exports.get(name) {
        return Ok(mangled.clone());
    }
    if top_level_names.contains(name) {
        return Ok(format!("{prefix}{name}"));
    }
    Err(Diagnostic::new(format!(
        "{context} references unknown function '{name}'"
    )))
}

enum ExportedField {
    Function(String),
    Constant(Expr),
}

fn export_field_value(
    field: &TableField,
    prefix: &str,
    top_level_names: &HashSet<String>,
    re_exports: &HashMap<String, String>,
    namespaces: &HashMap<String, ModuleNamespace>,
    constants: &HashMap<String, Expr>,
) -> Result<ExportedField, Diagnostic> {
    match &field.value {
        Expr::Name(name, _, _) => {
            if let Some(value) = constants.get(name) {
                return Ok(ExportedField::Constant(value.clone()));
            }
            export_function_name(
                name,
                prefix,
                top_level_names,
                re_exports,
                &format!("module export field '{}'", field.name),
            )
            .map(ExportedField::Function)
        }
        Expr::Field {
            base, name: member, ..
        } if matches!(&**base, Expr::Name(..)) => {
            let Expr::Name(namespace, _, _) = &**base else {
                unreachable!()
            };
            // The module's own dot-named function (`new = State.new`).
            let dotted = format!("{namespace}.{member}");
            if top_level_names.contains(&dotted) {
                return Ok(ExportedField::Function(format!("{prefix}{dotted}")));
            }
            let fields = namespaces.get(namespace).ok_or_else(|| {
                Diagnostic::new(format!(
                    "module export field '{}' references unknown namespace '{namespace}'",
                    field.name
                ))
            })?;
            if let Some(function) = fields.functions.get(member) {
                return Ok(ExportedField::Function(function.clone()));
            }
            if let Some(value) = fields.constants.get(member) {
                return Ok(ExportedField::Constant(value.clone()));
            }
            Err(Diagnostic::new(format!(
                "module export field '{}' references unknown member '{member}' on '{namespace}'",
                field.name
            )))
        }
        Expr::Function(_) => Ok(ExportedField::Function(format!("{prefix}{}", field.name))),
        _ => Err(Diagnostic::new(format!(
            "module export field '{}' must be a function name, namespace member, `function ... end`, \
             or a top-level `local NAME <const> = <literal>` constant",
            field.name
        ))),
    }
}

/// Rewrites a single module's bodies: mangles references to its own top-level
/// functions and replaces `require(...)` with resolved imports.
struct Rewriter<'a> {
    prefix: &'a str,
    func_names: &'a HashSet<String>,
    type_names: &'a HashSet<String>,
    /// Require-binding name -> exported type name -> canonical linked name.
    type_namespaces: &'a HashMap<String, HashMap<String, String>>,
    imports: &'a HashMap<String, ResolvedImport>,
    re_exports: HashMap<String, String>,
    namespaces: HashMap<String, ModuleNamespace>,
    value_aliases: HashMap<String, Expr>,
}

impl Rewriter<'_> {
    fn rewrite_declared_import_types(&self, import: &mut DeclaredImport) {
        for param in &mut import.params {
            self.rewrite_type(&mut param.ty);
        }
        self.rewrite_type(&mut import.return_type);
    }

    fn rewrite_function_types(&self, function: &mut Function) {
        for param in &mut function.params {
            self.rewrite_type(&mut param.ty);
        }
        if let Some(return_type) = &mut function.return_type {
            self.rewrite_type(return_type);
        }
    }

    fn rewrite_stmt_types(&self, stmt: &mut Stmt) {
        match stmt {
            Stmt::Let { ty, value, .. } => {
                if let Some(ty) = ty {
                    self.rewrite_type(ty);
                }
                self.rewrite_expr_types(value);
            }
            Stmt::Assign { value, .. } | Stmt::Expr(value) | Stmt::Return(value) => {
                self.rewrite_expr_types(value);
            }
            Stmt::IndexAssign {
                base, index, value, ..
            } => {
                self.rewrite_expr_types(base);
                self.rewrite_expr_types(index);
                self.rewrite_expr_types(value);
            }
            Stmt::FieldAssign { base, value, .. } => {
                self.rewrite_expr_types(base);
                self.rewrite_expr_types(value);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                self.rewrite_expr_types(condition);
                for stmt in then_body {
                    self.rewrite_stmt_types(stmt);
                }
                for stmt in else_body {
                    self.rewrite_stmt_types(stmt);
                }
            }
            Stmt::IfCast {
                target_ty,
                value,
                then_body,
                else_body,
                ..
            } => {
                self.rewrite_type(target_ty);
                self.rewrite_expr_types(value);
                for stmt in then_body {
                    self.rewrite_stmt_types(stmt);
                }
                for stmt in else_body {
                    self.rewrite_stmt_types(stmt);
                }
            }
            Stmt::While { condition, body } => {
                self.rewrite_expr_types(condition);
                for stmt in body {
                    self.rewrite_stmt_types(stmt);
                }
            }
            Stmt::Repeat { body, condition } => {
                for stmt in body {
                    self.rewrite_stmt_types(stmt);
                }
                self.rewrite_expr_types(condition);
            }
            Stmt::NumericFor {
                start,
                stop,
                step,
                body,
                ..
            } => {
                self.rewrite_expr_types(start);
                self.rewrite_expr_types(stop);
                if let Some(step) = step {
                    self.rewrite_expr_types(step);
                }
                for stmt in body {
                    self.rewrite_stmt_types(stmt);
                }
            }
            Stmt::ForIn { iterator, body, .. } => {
                self.rewrite_expr_types(iterator);
                for stmt in body {
                    self.rewrite_stmt_types(stmt);
                }
            }
            Stmt::ReturnMulti(values) | Stmt::AssignMulti { values, .. } => {
                for value in values {
                    self.rewrite_expr_types(value);
                }
            }
            Stmt::LetMulti { bindings, values } => {
                for binding in bindings {
                    if let Some(ty) = &mut binding.ty {
                        self.rewrite_type(ty);
                    }
                }
                for value in values {
                    self.rewrite_expr_types(value);
                }
            }
            Stmt::Break | Stmt::Continue => {}
        }
    }

    fn rewrite_expr_types(&self, expr: &mut Expr) {
        match expr {
            Expr::Unary { expr, .. } => self.rewrite_expr_types(expr),
            Expr::IsVariant { expr, .. } => self.rewrite_expr_types(expr),
            Expr::Cast { expr, ty, .. } => {
                self.rewrite_expr_types(expr);
                self.rewrite_type(ty);
            }
            Expr::Binary { left, right, .. } => {
                self.rewrite_expr_types(left);
                self.rewrite_expr_types(right);
            }
            Expr::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => {
                self.rewrite_expr_types(condition);
                self.rewrite_expr_types(then_expr);
                self.rewrite_expr_types(else_expr);
            }
            Expr::Call {
                callee,
                type_args,
                args,
                ..
            } => {
                self.rewrite_expr_types(callee);
                for ty in type_args {
                    self.rewrite_type(ty);
                }
                for arg in args {
                    self.rewrite_expr_types(arg);
                }
            }
            Expr::MethodCall { receiver, args, .. } => {
                self.rewrite_expr_types(receiver);
                for arg in args {
                    self.rewrite_expr_types(arg);
                }
            }
            Expr::Function(function) => {
                for param in &mut function.params {
                    self.rewrite_type(&mut param.ty);
                }
                if let Some(return_type) = &mut function.return_type {
                    self.rewrite_type(return_type);
                }
                for stmt in &mut function.body {
                    self.rewrite_stmt_types(stmt);
                }
            }
            Expr::ArrayLiteral { elements, .. } => {
                for element in elements {
                    self.rewrite_expr_types(element);
                }
            }
            Expr::TableLiteral { fields, .. } => {
                for field in fields {
                    self.rewrite_expr_types(&mut field.value);
                }
            }
            Expr::Field { base, .. } => self.rewrite_expr_types(base),
            Expr::Index { base, index, .. } => {
                self.rewrite_expr_types(base);
                self.rewrite_expr_types(index);
            }
            Expr::Number(..)
            | Expr::Bool(..)
            | Expr::Nil(..)
            | Expr::String(..)
            | Expr::Bytes(..)
            | Expr::Vararg(..)
            | Expr::Name(..)
            | Expr::Require(..) => {}
        }
    }

    fn rewrite_type(&self, ty: &mut Type) {
        match ty {
            Type::Named { name, type_args } => {
                if let Some((namespace, member)) = name.split_once('.') {
                    // `game.State`: a type alias reached through a require
                    // binding; resolve it to the imported module's prefixed
                    // declaration. Unknown bindings or members are left
                    // as-is so type resolution reports the dotted name.
                    if let Some(canonical) = self
                        .type_namespaces
                        .get(namespace)
                        .and_then(|types| types.get(member))
                    {
                        *name = canonical.clone();
                    }
                } else if self.type_names.contains(name) {
                    *name = format!("{}{name}", self.prefix);
                }
                for ty in type_args {
                    self.rewrite_type(ty);
                }
            }
            Type::Opaque { ty, .. } => self.rewrite_type(ty),
            Type::ExternSubtype(parent) => self.rewrite_type(parent),
            Type::Nullable(inner) => self.rewrite_type(inner),
            Type::TaggedVariant(variant) => self.rewrite_type(variant.payload.as_mut()),
            Type::TaggedUnion(variants) => {
                for variant in variants {
                    self.rewrite_type(variant.payload.as_mut());
                }
            }
            Type::Array(inner) | Type::Variadic(inner) => self.rewrite_type(inner),
            Type::Multi(types) => {
                for ty in types {
                    self.rewrite_type(ty);
                }
            }
            Type::Function {
                params,
                return_type,
            } => {
                for ty in params {
                    self.rewrite_type(ty);
                }
                self.rewrite_type(return_type);
            }
            Type::Record(fields) => {
                for ty in fields.values_mut() {
                    self.rewrite_type(ty);
                }
            }
            Type::Numeric(_)
            | Type::Unit
            | Type::Bool
            | Type::Unknown
            | Type::String
            | Type::Bytes
            | Type::Extern
            | Type::Nil
            | Type::TypeParam(_)
            | Type::TypedArray(_)
            | Type::Thread => {}
        }
    }

    fn rewrite_block(&mut self, stmts: &mut Vec<Stmt>, bound: &mut HashSet<String>) {
        let mut index = 0;
        while index < stmts.len() {
            // A bare require of an extern-only virtual module (dom:window,
            // tfjs, waluau:vitest) is a dependency declaration. Loading the
            // module above already made its ambient types and host
            // declarations available, so do not synthesize a value unless the
            // require expression is actually used as one.
            let is_bare_extern_dependency = matches!(
                &stmts[index],
                Stmt::Expr(Expr::Require(path, _))
                    if matches!(self.imports.get(path), Some(ResolvedImport::DomWindow))
                        || path == TFJS_REQUIRE
                        || path == VITEST_REQUIRE
            );
            if is_bare_extern_dependency {
                stmts.remove(index);
                continue;
            }

            self.rewrite_stmt(&mut stmts[index], bound);
            index += 1;
        }
    }

    fn rewrite_stmt(&mut self, stmt: &mut Stmt, bound: &mut HashSet<String>) {
        self.rewrite_stmt_types(stmt);
        match stmt {
            Stmt::Let { name, value, .. } => {
                if let Expr::Require(path, _) = &*value {
                    if let Some(resolved) = self.imports.get(path) {
                        match resolved {
                            ResolvedImport::Function(function) => {
                                self.re_exports.insert(name.clone(), function.clone());
                            }
                            ResolvedImport::Namespace(namespace) => {
                                self.namespaces.insert(name.clone(), namespace.clone());
                            }
                            ResolvedImport::DomWindow => {
                                if let Expr::Require(_, span) = &*value {
                                    self.value_aliases
                                        .insert(name.clone(), dom_window_expr(*span));
                                }
                            }
                        }
                    }
                }
                // A table-of-functions local (`local ns = { add = add }`)
                // registers as a namespace so `ns.add` resolves to the
                // function directly. Eligibility is decided before rewriting:
                // every field must reference a top-level function (or a
                // re-exported import) — a record literal that merely stores
                // other bindings (e.g. a constructor's `{ value = start }`)
                // must not hijack later field accesses on a same-named local.
                let registers_namespace = match &*value {
                    Expr::TableLiteral { fields, .. } => {
                        !fields.is_empty()
                            && fields.iter().all(|field| {
                                matches!(
                                    &field.value,
                                    Expr::Name(field_name, _, _)
                                        if !bound.contains(field_name)
                                            && (self.func_names.contains(field_name)
                                                || self.re_exports.contains_key(field_name))
                                )
                            })
                    }
                    _ => false,
                };
                self.rewrite_expr(value, bound);
                if registers_namespace {
                    if let Expr::TableLiteral { fields, .. } = &*value {
                        let mut field_map = BTreeMap::new();
                        for field in fields {
                            if let Expr::Name(function_name, _, _) = &field.value {
                                field_map.insert(field.name.clone(), function_name.clone());
                            }
                        }
                        self.namespaces
                            .insert(name.clone(), ModuleNamespace::from_functions(field_map));
                    }
                }
                bound.insert(name.clone());
            }
            Stmt::Assign { value, .. } => self.rewrite_expr(value, bound),
            Stmt::IndexAssign {
                base, index, value, ..
            } => {
                self.rewrite_expr(base, bound);
                self.rewrite_expr(index, bound);
                self.rewrite_expr(value, bound);
            }
            Stmt::FieldAssign { base, value, .. } => {
                self.rewrite_expr(base, bound);
                self.rewrite_expr(value, bound);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                self.rewrite_expr(condition, bound);
                self.rewrite_block(then_body, &mut bound.clone());
                self.rewrite_block(else_body, &mut bound.clone());
            }
            Stmt::IfCast {
                binding,
                value,
                then_body,
                else_body,
                ..
            } => {
                self.rewrite_expr(value, bound);
                let mut then_bound = bound.clone();
                then_bound.insert(binding.clone());
                self.rewrite_block(then_body, &mut then_bound);
                self.rewrite_block(else_body, &mut bound.clone());
            }
            Stmt::While { condition, body } => {
                self.rewrite_expr(condition, bound);
                self.rewrite_block(body, &mut bound.clone());
            }
            Stmt::Repeat { body, condition } => {
                // `until` can observe locals declared inside the loop body.
                let mut inner = bound.clone();
                self.rewrite_block(body, &mut inner);
                self.rewrite_expr(condition, &inner);
            }
            Stmt::NumericFor {
                name,
                start,
                stop,
                step,
                body,
                ..
            } => {
                self.rewrite_expr(start, bound);
                self.rewrite_expr(stop, bound);
                if let Some(step_expr) = step {
                    self.rewrite_expr(step_expr, bound);
                }
                let mut inner = bound.clone();
                inner.insert(name.clone());
                self.rewrite_block(body, &mut inner);
            }
            Stmt::ForIn {
                names,
                iterator,
                body,
                ..
            } => {
                self.rewrite_expr(iterator, bound);
                let mut inner = bound.clone();
                for name in names {
                    inner.insert(name.clone());
                }
                self.rewrite_block(body, &mut inner);
            }
            Stmt::Return(expr) => self.rewrite_expr(expr, bound),
            Stmt::ReturnMulti(values) => {
                for value in values {
                    self.rewrite_expr(value, bound);
                }
            }
            Stmt::LetMulti { bindings, values } => {
                for value in values {
                    self.rewrite_expr(value, bound);
                }
                for binding in bindings {
                    bound.insert(binding.name.clone());
                }
            }
            Stmt::AssignMulti { values, .. } => {
                for value in values {
                    self.rewrite_expr(value, bound);
                }
            }
            Stmt::Expr(expr) => self.rewrite_expr(expr, bound),
            Stmt::Break | Stmt::Continue => {}
        }
    }

    fn rewrite_expr(&mut self, expr: &mut Expr, bound: &HashSet<String>) {
        if let Expr::Field {
            base, name: field, ..
        } = expr
        {
            if let Expr::Name(local, _, _) = &**base {
                if let Some(fields) = self.namespaces.get(local) {
                    if let Some(resolved) = fields.functions.get(field) {
                        *expr = Expr::Name(resolved.clone(), None, None);
                        return;
                    }
                    if let Some(value) = fields.constants.get(field) {
                        *expr = value.clone();
                        return;
                    }
                }
                // A reference to the module's own dot-named function
                // (`State.new`), mangled like any other top-level function.
                let dotted = format!("{local}.{field}");
                if !bound.contains(local) && self.func_names.contains(&dotted) {
                    *expr = Expr::Name(format!("{}{dotted}", self.prefix), None, None);
                    return;
                }
            }
        }

        match expr {
            Expr::Require(path, span) => {
                if let Some(resolved) = self.imports.get(path) {
                    *expr = match resolved {
                        ResolvedImport::Function(name) => Expr::Name(name.clone(), None, *span),
                        ResolvedImport::Namespace(namespace) => Expr::TableLiteral {
                            fields: namespace
                                .functions
                                .iter()
                                .map(|(name, function)| TableField {
                                    name: name.clone(),
                                    value: Expr::Name(function.clone(), None, None),
                                })
                                .chain(namespace.constants.iter().map(|(name, value)| TableField {
                                    name: name.clone(),
                                    value: value.clone(),
                                }))
                                .collect(),
                            span: *span,
                        },
                        ResolvedImport::DomWindow => dom_window_expr(*span),
                    };
                }
            }
            Expr::Name(name, _, _) => {
                if !bound.contains(name) {
                    if let Some(resolved) = self.re_exports.get(name) {
                        *expr = Expr::Name(resolved.clone(), None, None);
                    } else if let Some(alias) = self.value_aliases.get(name) {
                        *expr = alias.clone();
                    } else if self.func_names.contains(name) {
                        *name = format!("{}{name}", self.prefix);
                    }
                }
            }
            Expr::Number(..)
            | Expr::Bool(..)
            | Expr::Nil(..)
            | Expr::String(..)
            | Expr::Bytes(..)
            | Expr::Vararg(..) => {}
            Expr::Unary { expr, .. } | Expr::Cast { expr, .. } | Expr::IsVariant { expr, .. } => {
                self.rewrite_expr(expr, bound)
            }
            Expr::Binary { left, right, .. } => {
                self.rewrite_expr(left, bound);
                self.rewrite_expr(right, bound);
            }
            Expr::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => {
                self.rewrite_expr(condition, bound);
                self.rewrite_expr(then_expr, bound);
                self.rewrite_expr(else_expr, bound);
            }
            Expr::Call { callee, args, .. } => {
                self.rewrite_expr(callee, bound);
                for arg in args {
                    self.rewrite_expr(arg, bound);
                }
            }
            Expr::MethodCall { receiver, args, .. } => {
                self.rewrite_expr(receiver, bound);
                for arg in args {
                    self.rewrite_expr(arg, bound);
                }
            }
            Expr::Function(function) => {
                let mut inner = bound.clone();
                if let Some(name) = &function.name {
                    inner.insert(name.clone());
                }
                for param in &function.params {
                    inner.insert(param.name.clone());
                }
                self.rewrite_block(&mut function.body, &mut inner);
            }
            Expr::ArrayLiteral { elements, .. } => {
                for element in elements {
                    self.rewrite_expr(element, bound);
                }
            }
            Expr::TableLiteral { fields, .. } => {
                for field in fields {
                    self.rewrite_expr(&mut field.value, bound);
                }
            }
            Expr::Field { base, .. } => self.rewrite_expr(base, bound),
            Expr::Index { base, index, .. } => {
                self.rewrite_expr(base, bound);
                self.rewrite_expr(index, bound);
            }
        }
    }
}

fn strip_unused_namespace_lets(stmts: &mut Vec<Stmt>) {
    for stmt in stmts.iter_mut() {
        strip_unused_namespace_lets_in_stmt(stmt);
    }
    let unused: HashSet<String> = stmts
        .iter()
        .filter_map(|stmt| {
            if let Stmt::Let { name, value, .. } = stmt {
                if matches!(value, Expr::TableLiteral { .. }) && !stmt_mentions_name(name, stmts) {
                    return Some(name.clone());
                }
            }
            None
        })
        .collect();
    if unused.is_empty() {
        return;
    }
    stmts.retain(|stmt| !matches!(stmt, Stmt::Let { name, .. } if unused.contains(name)));
}

fn strip_unused_namespace_lets_in_stmt(stmt: &mut Stmt) {
    match stmt {
        Stmt::Let { value, .. }
        | Stmt::Assign { value, .. }
        | Stmt::Return(value)
        | Stmt::Expr(value) => {
            strip_unused_namespace_lets_in_expr(value);
        }
        Stmt::IndexAssign {
            base, index, value, ..
        } => {
            strip_unused_namespace_lets_in_expr(base);
            strip_unused_namespace_lets_in_expr(index);
            strip_unused_namespace_lets_in_expr(value);
        }
        Stmt::FieldAssign { base, value, .. } => {
            strip_unused_namespace_lets_in_expr(base);
            strip_unused_namespace_lets_in_expr(value);
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            strip_unused_namespace_lets_in_expr(condition);
            strip_unused_namespace_lets(then_body);
            strip_unused_namespace_lets(else_body);
        }
        Stmt::IfCast {
            value,
            then_body,
            else_body,
            ..
        } => {
            strip_unused_namespace_lets_in_expr(value);
            strip_unused_namespace_lets(then_body);
            strip_unused_namespace_lets(else_body);
        }
        Stmt::While { condition, body } => {
            strip_unused_namespace_lets_in_expr(condition);
            strip_unused_namespace_lets(body);
        }
        Stmt::Repeat { body, condition } => {
            strip_unused_namespace_lets(body);
            strip_unused_namespace_lets_in_expr(condition);
        }
        Stmt::NumericFor {
            start,
            stop,
            step,
            body,
            ..
        } => {
            strip_unused_namespace_lets_in_expr(start);
            strip_unused_namespace_lets_in_expr(stop);
            if let Some(step) = step {
                strip_unused_namespace_lets_in_expr(step);
            }
            strip_unused_namespace_lets(body);
        }
        Stmt::ForIn { iterator, body, .. } => {
            strip_unused_namespace_lets_in_expr(iterator);
            strip_unused_namespace_lets(body);
        }
        Stmt::ReturnMulti(values)
        | Stmt::LetMulti { values, .. }
        | Stmt::AssignMulti { values, .. } => {
            for value in values {
                strip_unused_namespace_lets_in_expr(value);
            }
        }
        Stmt::Break | Stmt::Continue => {}
    }
}

fn strip_unused_namespace_lets_in_expr(expr: &mut Expr) {
    match expr {
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } | Expr::IsVariant { expr, .. } => {
            strip_unused_namespace_lets_in_expr(expr);
        }
        Expr::Binary { left, right, .. } => {
            strip_unused_namespace_lets_in_expr(left);
            strip_unused_namespace_lets_in_expr(right);
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            strip_unused_namespace_lets_in_expr(condition);
            strip_unused_namespace_lets_in_expr(then_expr);
            strip_unused_namespace_lets_in_expr(else_expr);
        }
        Expr::Call { callee, args, .. } => {
            strip_unused_namespace_lets_in_expr(callee);
            for arg in args {
                strip_unused_namespace_lets_in_expr(arg);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            strip_unused_namespace_lets_in_expr(receiver);
            for arg in args {
                strip_unused_namespace_lets_in_expr(arg);
            }
        }
        Expr::Function(function) => strip_unused_namespace_lets(&mut function.body),
        Expr::ArrayLiteral { elements, .. } => {
            for element in elements {
                strip_unused_namespace_lets_in_expr(element);
            }
        }
        Expr::TableLiteral { fields, .. } => {
            for field in fields {
                strip_unused_namespace_lets_in_expr(&mut field.value);
            }
        }
        Expr::Field { base, .. } => strip_unused_namespace_lets_in_expr(base),
        Expr::Index { base, index, .. } => {
            strip_unused_namespace_lets_in_expr(base);
            strip_unused_namespace_lets_in_expr(index);
        }
        Expr::Name(..)
        | Expr::Vararg(..)
        | Expr::Number(..)
        | Expr::Bool(..)
        | Expr::Nil(..)
        | Expr::String(..)
        | Expr::Bytes(..)
        | Expr::Require(..) => {}
    }
}

fn rename_imported_top_level_locals(stmts: &mut [Stmt], prefix: &str) {
    let renames = collect_top_level_local_renames(stmts, prefix);
    if renames.is_empty() {
        return;
    }

    let mut available = HashSet::new();
    let mut shadowed = HashSet::new();
    rename_stmt_block(stmts, &renames, &mut available, &mut shadowed);
}

fn collect_top_level_local_renames(stmts: &[Stmt], prefix: &str) -> HashMap<String, String> {
    let mut renames = HashMap::new();
    for stmt in stmts {
        match stmt {
            Stmt::Let { name, .. } => {
                renames.insert(name.clone(), format!("{prefix}{name}"));
            }
            Stmt::LetMulti { bindings, .. } => {
                for binding in bindings {
                    renames.insert(binding.name.clone(), format!("{prefix}{}", binding.name));
                }
            }
            _ => {}
        }
    }
    renames
}

fn rename_stmt_block(
    stmts: &mut [Stmt],
    renames: &HashMap<String, String>,
    available: &mut HashSet<String>,
    shadowed: &mut HashSet<String>,
) {
    for stmt in stmts {
        rename_stmt(stmt, renames, available, shadowed);
    }
}

fn rename_stmt(
    stmt: &mut Stmt,
    renames: &HashMap<String, String>,
    available: &mut HashSet<String>,
    shadowed: &mut HashSet<String>,
) {
    match stmt {
        Stmt::Let { name, value, .. } => {
            let original_name = name.clone();
            rename_expr(value, renames, available, shadowed);
            if let Some(renamed) = renames.get(&original_name) {
                *name = renamed.clone();
            }
            available.insert(original_name);
        }
        Stmt::Assign { value, .. } | Stmt::Expr(value) | Stmt::Return(value) => {
            rename_expr(value, renames, available, shadowed);
        }
        Stmt::IndexAssign {
            base, index, value, ..
        } => {
            rename_expr(base, renames, available, shadowed);
            rename_expr(index, renames, available, shadowed);
            rename_expr(value, renames, available, shadowed);
        }
        Stmt::FieldAssign { base, value, .. } => {
            rename_expr(base, renames, available, shadowed);
            rename_expr(value, renames, available, shadowed);
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            rename_expr(condition, renames, available, shadowed);
            rename_stmt_block(
                then_body,
                renames,
                &mut available.clone(),
                &mut shadowed.clone(),
            );
            rename_stmt_block(
                else_body,
                renames,
                &mut available.clone(),
                &mut shadowed.clone(),
            );
        }
        Stmt::IfCast {
            binding,
            value,
            then_body,
            else_body,
            ..
        } => {
            rename_expr(value, renames, available, shadowed);
            let mut then_available = available.clone();
            let mut then_shadowed = shadowed.clone();
            if renames.contains_key(binding) {
                then_shadowed.insert(binding.clone());
            }
            rename_stmt_block(then_body, renames, &mut then_available, &mut then_shadowed);
            rename_stmt_block(
                else_body,
                renames,
                &mut available.clone(),
                &mut shadowed.clone(),
            );
        }
        Stmt::While { condition, body } => {
            rename_expr(condition, renames, available, shadowed);
            rename_stmt_block(body, renames, &mut available.clone(), &mut shadowed.clone());
        }
        Stmt::Repeat { body, condition } => {
            let mut body_available = available.clone();
            let mut body_shadowed = shadowed.clone();
            rename_stmt_block(body, renames, &mut body_available, &mut body_shadowed);
            rename_expr(condition, renames, &body_available, &body_shadowed);
        }
        Stmt::NumericFor {
            name,
            start,
            stop,
            step,
            body,
            ..
        } => {
            rename_expr(start, renames, available, shadowed);
            rename_expr(stop, renames, available, shadowed);
            if let Some(step) = step {
                rename_expr(step, renames, available, shadowed);
            }
            let mut inner_available = available.clone();
            let mut inner_shadowed = shadowed.clone();
            if renames.contains_key(name) {
                inner_shadowed.insert(name.clone());
            }
            rename_stmt_block(body, renames, &mut inner_available, &mut inner_shadowed);
        }
        Stmt::ForIn {
            names,
            iterator,
            body,
            ..
        } => {
            rename_expr(iterator, renames, available, shadowed);
            let mut inner_available = available.clone();
            let mut inner_shadowed = shadowed.clone();
            for name in names {
                if renames.contains_key(name) {
                    inner_shadowed.insert(name.clone());
                }
            }
            rename_stmt_block(body, renames, &mut inner_available, &mut inner_shadowed);
        }
        Stmt::ReturnMulti(values) | Stmt::AssignMulti { values, .. } => {
            for value in values {
                rename_expr(value, renames, available, shadowed);
            }
        }
        Stmt::LetMulti { bindings, values } => {
            for value in values {
                rename_expr(value, renames, available, shadowed);
            }
            for binding in bindings {
                let original_name = binding.name.clone();
                if let Some(renamed) = renames.get(&original_name) {
                    binding.name = renamed.clone();
                }
                available.insert(original_name);
            }
        }
        Stmt::Break | Stmt::Continue => {}
    }
}

fn rename_expr(
    expr: &mut Expr,
    renames: &HashMap<String, String>,
    available: &HashSet<String>,
    shadowed: &HashSet<String>,
) {
    match expr {
        Expr::Name(name, _, _) => {
            if available.contains(name) && !shadowed.contains(name) {
                if let Some(renamed) = renames.get(name) {
                    *name = renamed.clone();
                }
            }
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => {
            rename_expr(expr, renames, available, shadowed)
        }
        Expr::IsVariant { expr, .. } => rename_expr(expr, renames, available, shadowed),
        Expr::Binary { left, right, .. } => {
            rename_expr(left, renames, available, shadowed);
            rename_expr(right, renames, available, shadowed);
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            rename_expr(condition, renames, available, shadowed);
            rename_expr(then_expr, renames, available, shadowed);
            rename_expr(else_expr, renames, available, shadowed);
        }
        Expr::Call { callee, args, .. } => {
            rename_expr(callee, renames, available, shadowed);
            for arg in args {
                rename_expr(arg, renames, available, shadowed);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            rename_expr(receiver, renames, available, shadowed);
            for arg in args {
                rename_expr(arg, renames, available, shadowed);
            }
        }
        Expr::Function(function) => {
            let mut inner_available = available.clone();
            let mut inner_shadowed = shadowed.clone();
            if let Some(name) = &function.name {
                if renames.contains_key(name) {
                    inner_shadowed.insert(name.clone());
                }
            }
            for param in &function.params {
                if renames.contains_key(&param.name) {
                    inner_shadowed.insert(param.name.clone());
                }
            }
            rename_stmt_block(
                &mut function.body,
                renames,
                &mut inner_available,
                &mut inner_shadowed,
            );
        }
        Expr::ArrayLiteral { elements, .. } => {
            for element in elements {
                rename_expr(element, renames, available, shadowed);
            }
        }
        Expr::TableLiteral { fields, .. } => {
            for field in fields {
                rename_expr(&mut field.value, renames, available, shadowed);
            }
        }
        Expr::Field { base, .. } => rename_expr(base, renames, available, shadowed),
        Expr::Index { base, index, .. } => {
            rename_expr(base, renames, available, shadowed);
            rename_expr(index, renames, available, shadowed);
        }
        Expr::Number(..)
        | Expr::Bool(..)
        | Expr::Nil(..)
        | Expr::String(..)
        | Expr::Bytes(..)
        | Expr::Vararg(..)
        | Expr::Require(..) => {}
    }
}

fn stmt_mentions_name(name: &str, stmts: &[Stmt]) -> bool {
    stmts
        .iter()
        .any(|stmt| stmt_mentions_name_in_stmt(name, stmt))
}

fn stmt_mentions_name_in_stmt(name: &str, stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Let {
            name: local, value, ..
        } if local == name => false,
        Stmt::Let { value, .. }
        | Stmt::Assign { value, .. }
        | Stmt::Return(value)
        | Stmt::Expr(value) => expr_mentions_name(name, value),
        Stmt::IndexAssign {
            base, index, value, ..
        } => {
            expr_mentions_name(name, base)
                || expr_mentions_name(name, index)
                || expr_mentions_name(name, value)
        }
        Stmt::FieldAssign { base, value, .. } => {
            expr_mentions_name(name, base) || expr_mentions_name(name, value)
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            expr_mentions_name(name, condition)
                || stmt_mentions_name(name, then_body)
                || stmt_mentions_name(name, else_body)
        }
        Stmt::IfCast {
            binding,
            value,
            then_body,
            else_body,
            ..
        } => {
            expr_mentions_name(name, value)
                || (binding != name && stmt_mentions_name(name, then_body))
                || stmt_mentions_name(name, else_body)
        }
        Stmt::While { condition, body } => {
            expr_mentions_name(name, condition) || stmt_mentions_name(name, body)
        }
        Stmt::Repeat { body, condition } => {
            stmt_mentions_name(name, body) || expr_mentions_name(name, condition)
        }
        Stmt::NumericFor {
            start,
            stop,
            step,
            body,
            ..
        } => {
            expr_mentions_name(name, start)
                || expr_mentions_name(name, stop)
                || step
                    .as_ref()
                    .is_some_and(|step_expr| expr_mentions_name(name, step_expr))
                || stmt_mentions_name(name, body)
        }
        Stmt::ForIn { iterator, body, .. } => {
            expr_mentions_name(name, iterator) || stmt_mentions_name(name, body)
        }
        Stmt::ReturnMulti(values)
        | Stmt::LetMulti { values, .. }
        | Stmt::AssignMulti { values, .. } => {
            values.iter().any(|value| expr_mentions_name(name, value))
        }
        Stmt::Break | Stmt::Continue => false,
    }
}

fn expr_mentions_name(name: &str, expr: &Expr) -> bool {
    match expr {
        Expr::Name(local, _, _) => local == name,
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => expr_mentions_name(name, expr),
        Expr::IsVariant { expr, .. } => expr_mentions_name(name, expr),
        Expr::Binary { left, right, .. } => {
            expr_mentions_name(name, left) || expr_mentions_name(name, right)
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            expr_mentions_name(name, condition)
                || expr_mentions_name(name, then_expr)
                || expr_mentions_name(name, else_expr)
        }
        Expr::Call { callee, args, .. } => {
            expr_mentions_name(name, callee) || args.iter().any(|arg| expr_mentions_name(name, arg))
        }
        Expr::MethodCall { receiver, args, .. } => {
            expr_mentions_name(name, receiver)
                || args.iter().any(|arg| expr_mentions_name(name, arg))
        }
        Expr::Function(function) => stmt_mentions_name(name, &function.body),
        Expr::ArrayLiteral { elements, .. } => {
            elements.iter().any(|el| expr_mentions_name(name, el))
        }
        Expr::TableLiteral { fields, .. } => fields
            .iter()
            .any(|field| expr_mentions_name(name, &field.value)),
        Expr::Field { base, .. } => expr_mentions_name(name, base),
        Expr::Index { base, index, .. } => {
            expr_mentions_name(name, base) || expr_mentions_name(name, index)
        }
        Expr::Require(..)
        | Expr::Vararg(..)
        | Expr::Number(..)
        | Expr::Bool(..)
        | Expr::Nil(..)
        | Expr::String(..)
        | Expr::Bytes(..) => false,
    }
}

fn collect_require_paths(program: &Program, out: &mut Vec<String>) {
    for function in &program.functions {
        collect_block(&function.body, out);
    }
    collect_block(&program.top_level, out);
}

fn collect_block(stmts: &[Stmt], out: &mut Vec<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. } | Stmt::Assign { value, .. } => collect_expr(value, out),
            Stmt::IndexAssign {
                base, index, value, ..
            } => {
                collect_expr(base, out);
                collect_expr(index, out);
                collect_expr(value, out);
            }
            Stmt::FieldAssign { base, value, .. } => {
                collect_expr(base, out);
                collect_expr(value, out);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_expr(condition, out);
                collect_block(then_body, out);
                collect_block(else_body, out);
            }
            Stmt::IfCast {
                value,
                then_body,
                else_body,
                ..
            } => {
                collect_expr(value, out);
                collect_block(then_body, out);
                collect_block(else_body, out);
            }
            Stmt::While { condition, body } | Stmt::Repeat { body, condition } => {
                collect_expr(condition, out);
                collect_block(body, out);
            }
            Stmt::NumericFor {
                start,
                stop,
                step,
                body,
                ..
            } => {
                collect_expr(start, out);
                collect_expr(stop, out);
                if let Some(step_expr) = step {
                    collect_expr(step_expr, out);
                }
                collect_block(body, out);
            }
            Stmt::ForIn { iterator, body, .. } => {
                collect_expr(iterator, out);
                collect_block(body, out);
            }
            Stmt::Return(expr) | Stmt::Expr(expr) => collect_expr(expr, out),
            Stmt::ReturnMulti(values)
            | Stmt::LetMulti { values, .. }
            | Stmt::AssignMulti { values, .. } => {
                for value in values {
                    collect_expr(value, out);
                }
            }
            Stmt::Break | Stmt::Continue => {}
        }
    }
}

fn collect_expr(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::Require(path, _) => out.push(path.clone()),
        Expr::Name(..)
        | Expr::Vararg(..)
        | Expr::Number(..)
        | Expr::Bool(..)
        | Expr::Nil(..)
        | Expr::String(..)
        | Expr::Bytes(..) => {}
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } | Expr::IsVariant { expr, .. } => {
            collect_expr(expr, out)
        }
        Expr::Binary { left, right, .. } => {
            collect_expr(left, out);
            collect_expr(right, out);
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            collect_expr(condition, out);
            collect_expr(then_expr, out);
            collect_expr(else_expr, out);
        }
        Expr::Call { callee, args, .. } => {
            collect_expr(callee, out);
            for arg in args {
                collect_expr(arg, out);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            collect_expr(receiver, out);
            for arg in args {
                collect_expr(arg, out);
            }
        }
        Expr::Function(function) => collect_block(&function.body, out),
        Expr::ArrayLiteral { elements, .. } => {
            for element in elements {
                collect_expr(element, out);
            }
        }
        Expr::TableLiteral { fields, .. } => {
            for field in fields {
                collect_expr(&field.value, out);
            }
        }
        Expr::Field { base, .. } => collect_expr(base, out),
        Expr::Index { base, index, .. } => {
            collect_expr(base, out);
            collect_expr(index, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::link_program;
    use std::fs;
    use tempfile::tempdir;
    use waluau_ast::{Expr, Stmt};

    #[test]
    fn imported_top_level_statements_are_merged_and_mangled() {
        let dir = tempdir().expect("tempdir should exist");
        fs::write(
            dir.path().join("lib.walu"),
            r#"
                local value: i32 = 41
                assert(value == 41)

                return {
                    add_one = function(x: i32): i32
                        return x + 1
                    end,
                }
            "#,
        )
        .expect("lib should write");
        fs::write(
            dir.path().join("main.walu"),
            r#"
                function main(): i32
                    local lib = require("./lib")
                    return lib.add_one(1)
                end
            "#,
        )
        .expect("main should write");

        let program = link_program(&dir.path().join("main.walu")).expect("link should succeed");
        assert!(
            matches!(
                &program.top_level[0],
                Stmt::Let { name, .. } if name.starts_with("__waluau_m0_")
            ),
            "expected imported locals to be mangled: {:?}",
            program.top_level
        );
        assert!(
            matches!(&program.top_level[1], Stmt::Expr(Expr::Call { .. })),
            "expected imported assert to remain in merged top-level init: {:?}",
            program.top_level
        );
    }

    #[test]
    fn top_level_require_namespace_is_visible_in_function_body() {
        let dir = tempdir().expect("tempdir should exist");
        fs::write(
            dir.path().join("lib.walu"),
            r#"
                function add_one(x: i32): i32
                    return x + 1
                end

                return {
                    add_one = add_one,
                }
            "#,
        )
        .expect("lib should write");
        fs::write(
            dir.path().join("main.walu"),
            r#"
                local lib = require("./lib")

                function main(): i32
                    return lib.add_one(1)
                end
            "#,
        )
        .expect("main should write");

        let program = link_program(&dir.path().join("main.walu")).expect("link should succeed");
        let main = program
            .functions
            .iter()
            .find(|function| function.name.to_string() == "main")
            .expect("main should be present");
        assert!(
            matches!(
                &main.body[0],
                Stmt::Return(Expr::Call { callee, .. })
                    if matches!(&**callee, Expr::Name(name, _, _) if name == "__waluau_m0_add_one")
            ),
            "expected top-level require namespace access to rewrite to imported function: {:?}",
            main.body
        );
    }

    #[test]
    fn require_inside_function_is_statically_linked() {
        let dir = tempdir().expect("tempdir should exist");
        fs::write(
            dir.path().join("lib.walu"),
            r#"
                function add_one(x: i32): i32
                    return x + 1
                end

                return add_one
            "#,
        )
        .expect("lib should write");
        fs::write(
            dir.path().join("main.walu"),
            r#"
                function main(): i32
                    local add_one = require("./lib")
                    return add_one(1)
                end
            "#,
        )
        .expect("main should write");

        let program = link_program(&dir.path().join("main.walu")).expect("link should succeed");
        let main = program
            .functions
            .iter()
            .find(|function| function.name.to_string() == "main")
            .expect("main should be present");
        assert!(
            matches!(
                &main.body[0],
                Stmt::Let {
                    value: Expr::Name(name, _, _),
                    ..
                } if name == "__waluau_m0_add_one"
            ),
            "expected nested require to resolve statically: {:?}",
            main.body
        );
    }

    #[test]
    fn rejects_bare_filesystem_require_path() {
        let dir = tempdir().expect("tempdir should exist");
        fs::write(
            dir.path().join("main.walu"),
            "local lib = require(\"lib\")\n",
        )
        .expect("main should write");

        let error = link_program(&dir.path().join("main.walu")).expect_err("bare path should fail");
        assert_eq!(
            error.to_string(),
            "require path must be relative and start with './' or '../', got \"lib\""
        );
    }

    #[test]
    fn linked_entry_export_is_consumed_as_module_metadata() {
        let dir = tempdir().expect("tempdir should exist");
        fs::write(
            dir.path().join("main.walu"),
            r#"
                function main(): i32
                    return 1
                end

                return main
            "#,
        )
        .expect("main should write");

        let program = link_program(&dir.path().join("main.walu")).expect("link should succeed");
        assert!(
            program.export.is_none(),
            "linked entry return must not survive as executable metadata"
        );
        assert!(
            program
                .functions
                .iter()
                .any(|function| function.name.to_string() == "main"),
            "entry function should remain the Wasm-facing surface"
        );
    }
}
