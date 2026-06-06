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
//! 3. Every `require(...)` node is replaced with either the imported function
//!    (single export) or a table of mangled function references (namespace
//!    export). `m.field` member access on namespace locals is rewritten to the
//!    corresponding mangled function name.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use waluau_ast::{Expr, Function, FunctionExpr, FunctionName, Program, Stmt, TableField, Type};
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
        let program = waluau_parser::parse_with_path(&source, &path.to_string_lossy())?;
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
    let mut type_declarations = Vec::new();
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

        let mut rewriter = Rewriter {
            prefix: &prefix,
            func_names: &func_names,
            type_names: &type_names,
            imports: &imports,
            re_exports: HashMap::new(),
            namespaces: HashMap::new(),
        };

        for decl in &module.program.type_declarations {
            let mut lowered = decl.clone();
            rewriter.rewrite_type(&mut lowered.ty);
            lowered.name = format!("{prefix}{}", lowered.name);
            type_declarations.push(lowered);
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
    let mut sources = BTreeMap::new();
    for module in modules {
        sources.extend(module.program.sources.clone());
    }

    Ok(Program {
        functions,
        type_declarations,
        top_level,
        export: None,
        sources,
        entry_file_path,
    })
}

#[derive(Clone)]
enum ResolvedImport {
    Function(String),
    Namespace(BTreeMap<String, String>),
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
        type_params: function.type_params.clone(),
        params: function.params.clone(),
        return_type: function.return_type.clone(),
        body: function.body.clone(),
        file_path: function.file_path.clone(),
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
    let (re_exports, namespaces) = process_reexport_bindings(&module.program.top_level, &imports);

    let resolved = resolve_module_export(
        module.program.export.as_ref(),
        &prefix,
        &top_level_names,
        &re_exports,
        &namespaces,
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

fn process_reexport_bindings(
    top_level: &[Stmt],
    imports: &HashMap<String, ResolvedImport>,
) -> (
    HashMap<String, String>,
    HashMap<String, BTreeMap<String, String>>,
) {
    let empty = HashSet::new();
    let mut rewriter = Rewriter {
        prefix: "",
        func_names: &empty,
        type_names: &empty,
        imports,
        re_exports: HashMap::new(),
        namespaces: HashMap::new(),
    };
    let mut stmts = top_level.to_vec();
    let mut bound = HashSet::new();
    rewriter.rewrite_block(&mut stmts, &mut bound);
    (rewriter.re_exports, rewriter.namespaces)
}

fn resolve_module_export(
    export: Option<&Expr>,
    prefix: &str,
    top_level_names: &HashSet<String>,
    re_exports: &HashMap<String, String>,
    namespaces: &HashMap<String, BTreeMap<String, String>>,
) -> Result<ResolvedImport, Diagnostic> {
    match export {
        Some(Expr::Name(name, _)) => Ok(ResolvedImport::Function(export_function_name(
            name,
            prefix,
            top_level_names,
            re_exports,
            "module export",
        )?)),
        Some(Expr::TableLiteral { fields, .. }) => {
            let mut namespace = BTreeMap::new();
            for field in fields {
                let function_name = export_field_function_name(
                    field,
                    prefix,
                    top_level_names,
                    re_exports,
                    namespaces,
                )?;
                namespace.insert(field.name.clone(), function_name);
            }
            if namespace.is_empty() {
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

fn export_field_function_name(
    field: &TableField,
    prefix: &str,
    top_level_names: &HashSet<String>,
    re_exports: &HashMap<String, String>,
    namespaces: &HashMap<String, BTreeMap<String, String>>,
) -> Result<String, Diagnostic> {
    match &field.value {
        Expr::Name(name, _) => export_function_name(
            name,
            prefix,
            top_level_names,
            re_exports,
            &format!("module export field '{}'", field.name),
        ),
        Expr::Field {
            base, name: member, ..
        } if matches!(&**base, Expr::Name(..)) => {
            let Expr::Name(namespace, _) = &**base else {
                unreachable!()
            };
            let fields = namespaces.get(namespace).ok_or_else(|| {
                Diagnostic::new(format!(
                    "module export field '{}' references unknown namespace '{namespace}'",
                    field.name
                ))
            })?;
            fields.get(member).cloned().ok_or_else(|| {
                Diagnostic::new(format!(
                    "module export field '{}' references unknown member '{member}' on '{namespace}'",
                    field.name
                ))
            })
        }
        Expr::Function(_) => Ok(format!("{prefix}{}", field.name)),
        _ => Err(Diagnostic::new(format!(
            "module export field '{}' must be a function name, namespace member, or `function ... end`",
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
    imports: &'a HashMap<String, ResolvedImport>,
    re_exports: HashMap<String, String>,
    namespaces: HashMap<String, BTreeMap<String, String>>,
}

impl Rewriter<'_> {
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
            | Expr::String(..)
            | Expr::Bytes(..)
            | Expr::Name(..)
            | Expr::Require(..) => {}
        }
    }

    fn rewrite_type(&self, ty: &mut Type) {
        match ty {
            Type::Named { name, type_args } => {
                if self.type_names.contains(name) {
                    *name = format!("{}{name}", self.prefix);
                }
                for ty in type_args {
                    self.rewrite_type(ty);
                }
            }
            Type::Opaque { ty, .. } => self.rewrite_type(ty),
            Type::TaggedVariant(variant) => self.rewrite_type(variant.payload.as_mut()),
            Type::TaggedUnion(variants) => {
                for variant in variants {
                    self.rewrite_type(variant.payload.as_mut());
                }
            }
            Type::Array(inner) => self.rewrite_type(inner),
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
            | Type::TypeParam(_)
            | Type::Thread => {}
        }
    }

    fn rewrite_block(&mut self, stmts: &mut [Stmt], bound: &mut HashSet<String>) {
        for stmt in stmts {
            self.rewrite_stmt(stmt, bound);
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
                            ResolvedImport::Namespace(fields) => {
                                self.namespaces.insert(name.clone(), fields.clone());
                            }
                        }
                    }
                }
                self.rewrite_expr(value, bound);
                if let Expr::TableLiteral { fields, .. } = &*value {
                    let mut field_map = BTreeMap::new();
                    for field in fields {
                        if let Expr::Name(function_name, _) = &field.value {
                            field_map.insert(field.name.clone(), function_name.clone());
                        }
                    }
                    if !field_map.is_empty() && field_map.len() == fields.len() {
                        self.namespaces.insert(name.clone(), field_map);
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
            if let Expr::Name(local, _) = &**base {
                if let Some(fields) = self.namespaces.get(local) {
                    if let Some(resolved) = fields.get(field) {
                        *expr = Expr::Name(resolved.clone(), None);
                        return;
                    }
                }
            }
        }

        match expr {
            Expr::Require(path, span) => {
                if let Some(resolved) = self.imports.get(path) {
                    *expr = match resolved {
                        ResolvedImport::Function(name) => Expr::Name(name.clone(), *span),
                        ResolvedImport::Namespace(fields) => Expr::TableLiteral {
                            fields: fields
                                .iter()
                                .map(|(name, function)| TableField {
                                    name: name.clone(),
                                    value: Expr::Name(function.clone(), None),
                                })
                                .collect(),
                            span: *span,
                        },
                    };
                }
            }
            Expr::Name(name, _) => {
                if !bound.contains(name) && self.func_names.contains(name) {
                    *name = format!("{}{name}", self.prefix);
                }
            }
            Expr::Number(..) | Expr::Bool(..) | Expr::String(..) | Expr::Bytes(..) => {}
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
            Expr::Call {
                callee,
                type_args: _,
                args,
                ..
            } => {
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
        Expr::Name(name, _) => {
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
        | Expr::String(..)
        | Expr::Bytes(..)
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
        Expr::Name(local, _) => local == name,
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
        Expr::Call {
            callee,
            args,
            type_args: _,
            ..
        } => {
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
        | Expr::Number(..)
        | Expr::Bool(..)
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
        Expr::Name(..) | Expr::Number(..) | Expr::Bool(..) | Expr::String(..) | Expr::Bytes(..) => {
        }
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
        Expr::Call {
            callee,
            type_args: _,
            args,
            ..
        } => {
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
}
