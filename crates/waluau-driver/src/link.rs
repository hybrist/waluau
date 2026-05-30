//! Module linking: resolves a graph of `require`-connected `.walu` files into a
//! single [`Program`] that the rest of the pipeline can compile unchanged.
//!
//! The strategy keeps the later compiler stages module-unaware:
//!
//! 1. Starting from an entry file, every `require("./path")` is resolved
//!    relative to the requiring file and loaded recursively, with cycle
//!    detection.
//! 2. Each non-entry module's top-level functions are renamed with a unique,
//!    per-module prefix so names from different files cannot collide. The entry
//!    module keeps its original names so its Wasm exports stay stable.
//! 3. Every `require(...)` node is replaced with a reference to the imported
//!    module's exported function. References are rewritten with lexical-scope
//!    awareness so a local that shadows a function name is left untouched.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use waluau_ast::{Expr, Program, Stmt};
use waluau_diagnostics::Diagnostic;

/// Resolve the module graph rooted at `entry` and merge it into one program.
pub fn link_program(entry: &Path) -> Result<Program, Diagnostic> {
    let entry = entry.canonicalize().map_err(|error| {
        Diagnostic::new(format!(
            "cannot open input file `{}`: {error}",
            entry.display()
        ))
    })?;
    let mut loader = Loader::default();
    let entry_id = loader.load(&entry)?;
    merge(&loader.modules, entry_id)
}

struct LoadedModule {
    program: Program,
    /// Raw `require` path strings to the module id they resolve to.
    requires: HashMap<String, usize>,
}

#[derive(Default)]
struct Loader {
    modules: Vec<LoadedModule>,
    by_path: HashMap<PathBuf, usize>,
    stack: Vec<PathBuf>,
}

impl Loader {
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

        let source = std::fs::read_to_string(path).map_err(|error| {
            Diagnostic::new(format!("read module `{}`: {error}", path.display()))
        })?;
        let program = waluau_parser::parse(&source)?;
        let dir = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        let mut raw_paths = Vec::new();
        collect_require_paths(&program, &mut raw_paths);

        self.stack.push(path.to_path_buf());
        let mut requires = HashMap::new();
        for raw in raw_paths {
            if requires.contains_key(&raw) {
                continue;
            }
            let resolved = resolve_module_path(&dir, &raw)?;
            let target = self.load(&resolved)?;
            requires.insert(raw, target);
        }
        self.stack.pop();

        let id = self.modules.len();
        self.modules.push(LoadedModule { program, requires });
        self.by_path.insert(path.to_path_buf(), id);
        Ok(id)
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

fn merge(modules: &[LoadedModule], entry_id: usize) -> Result<Program, Diagnostic> {
    let mut functions = Vec::new();
    let mut top_level = Vec::new();

    for (id, module) in modules.iter().enumerate() {
        if id != entry_id && !module.program.top_level.is_empty() {
            return Err(Diagnostic::new(
                "imported modules may only contain functions and a trailing `return <function>`",
            ));
        }

        let prefix = module_prefix(id, entry_id);
        let func_names: HashSet<String> = module
            .program
            .functions
            .iter()
            .map(|function| function.name.clone())
            .collect();

        let mut imports = HashMap::new();
        for (raw, &target_id) in &module.requires {
            let export_name = exported_function(modules, target_id, raw)?;
            let resolved = format!("{}{}", module_prefix(target_id, entry_id), export_name);
            imports.insert(raw.clone(), resolved);
        }

        let rewriter = Rewriter {
            prefix: &prefix,
            func_names: &func_names,
            imports: &imports,
        };

        for function in &module.program.functions {
            let mut lowered = function.clone();
            let mut bound: HashSet<String> = lowered
                .params
                .iter()
                .map(|param| param.name.clone())
                .collect();
            rewriter.rewrite_block(&mut lowered.body, &mut bound);
            lowered.name = format!("{prefix}{}", function.name);
            functions.push(lowered);
        }

        if id == entry_id {
            let mut lowered = module.program.top_level.clone();
            let mut bound = HashSet::new();
            rewriter.rewrite_block(&mut lowered, &mut bound);
            top_level = lowered;
        }
    }

    Ok(Program {
        functions,
        top_level,
        export: None,
    })
}

/// Validate and return the name of the function a module exports.
fn exported_function(
    modules: &[LoadedModule],
    target_id: usize,
    raw: &str,
) -> Result<String, Diagnostic> {
    match &modules[target_id].program.export {
        Some(Expr::Name(name)) => {
            if modules[target_id]
                .program
                .functions
                .iter()
                .any(|function| &function.name == name)
            {
                Ok(name.clone())
            } else {
                Err(Diagnostic::new(format!(
                    "module imported via \"{raw}\" exports unknown function '{name}'"
                )))
            }
        }
        Some(_) => Err(Diagnostic::new(format!(
            "module imported via \"{raw}\" must export a function name, e.g. `return myFunction`"
        ))),
        None => Err(Diagnostic::new(format!(
            "module imported via \"{raw}\" has no export; add `return <function>`"
        ))),
    }
}

/// Rewrites a single module's bodies: mangles references to its own top-level
/// functions and replaces `require(...)` with the resolved import name.
struct Rewriter<'a> {
    prefix: &'a str,
    func_names: &'a HashSet<String>,
    imports: &'a HashMap<String, String>,
}

impl Rewriter<'_> {
    fn rewrite_block(&self, stmts: &mut [Stmt], bound: &mut HashSet<String>) {
        for stmt in stmts {
            self.rewrite_stmt(stmt, bound);
        }
    }

    fn rewrite_stmt(&self, stmt: &mut Stmt, bound: &mut HashSet<String>) {
        match stmt {
            Stmt::Let { name, value, .. } => {
                self.rewrite_expr(value, bound);
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
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                self.rewrite_expr(condition, bound);
                self.rewrite_block(then_body, &mut bound.clone());
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
        }
    }

    fn rewrite_expr(&self, expr: &mut Expr, bound: &HashSet<String>) {
        match expr {
            Expr::Require(path) => {
                if let Some(resolved) = self.imports.get(path) {
                    *expr = Expr::Name(resolved.clone());
                }
            }
            Expr::Name(name) => {
                if !bound.contains(name) && self.func_names.contains(name) {
                    *name = format!("{}{name}", self.prefix);
                }
            }
            Expr::Number(_) | Expr::Bool(_) => {}
            Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => self.rewrite_expr(expr, bound),
            Expr::Binary { left, right, .. } => {
                self.rewrite_expr(left, bound);
                self.rewrite_expr(right, bound);
            }
            Expr::If {
                condition,
                then_expr,
                else_expr,
            } => {
                self.rewrite_expr(condition, bound);
                self.rewrite_expr(then_expr, bound);
                self.rewrite_expr(else_expr, bound);
            }
            Expr::Call { callee, args } => {
                self.rewrite_expr(callee, bound);
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
            Expr::ArrayLiteral { elements } => {
                for element in elements {
                    self.rewrite_expr(element, bound);
                }
            }
            Expr::Index { base, index } => {
                self.rewrite_expr(base, bound);
                self.rewrite_expr(index, bound);
            }
        }
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
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_expr(condition, out);
                collect_block(then_body, out);
                collect_block(else_body, out);
            }
            Stmt::While { condition, body } | Stmt::Repeat { body, condition } => {
                collect_expr(condition, out);
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
        }
    }
}

fn collect_expr(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::Require(path) => out.push(path.clone()),
        Expr::Name(_) | Expr::Number(_) | Expr::Bool(_) => {}
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => collect_expr(expr, out),
        Expr::Binary { left, right, .. } => {
            collect_expr(left, out);
            collect_expr(right, out);
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
        } => {
            collect_expr(condition, out);
            collect_expr(then_expr, out);
            collect_expr(else_expr, out);
        }
        Expr::Call { callee, args } => {
            collect_expr(callee, out);
            for arg in args {
                collect_expr(arg, out);
            }
        }
        Expr::Function(function) => collect_block(&function.body, out),
        Expr::ArrayLiteral { elements } => {
            for element in elements {
                collect_expr(element, out);
            }
        }
        Expr::Index { base, index } => {
            collect_expr(base, out);
            collect_expr(index, out);
        }
    }
}
