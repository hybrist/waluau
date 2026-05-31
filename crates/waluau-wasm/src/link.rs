use std::collections::{BTreeMap, HashMap, HashSet};
use waluau_ast::{Expr, Function, FunctionExpr, Program, Stmt, TableField};

pub struct LoadedModule {
    pub program: Program,
    pub requires: HashMap<String, usize>,
}

pub fn clean_path(path: &str) -> String {
    let mut parts = Vec::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            parts.pop();
        } else {
            parts.push(part);
        }
    }
    format!("/{}", parts.join("/"))
}

pub fn resolve_path(base_file: &str, relative_path: &str) -> Result<String, String> {
    if !(relative_path.starts_with("./") || relative_path.starts_with("../")) {
        return Err(format!(
            "require path must be relative and start with './' or '../', got \"{}\"",
            relative_path
        ));
    }
    let base_dir = if let Some(idx) = base_file.rfind('/') {
        &base_file[..idx]
    } else {
        ""
    };

    let combined = format!("{}/{}", base_dir, relative_path);
    let cleaned = clean_path(&combined);
    Ok(cleaned)
}

pub fn link_programs(files: &HashMap<String, String>, entry_path: &str) -> Result<Program, String> {
    let mut normalized_files = HashMap::new();
    for (path, source) in files {
        let mut norm = clean_path(path);
        if !norm.ends_with(".walu") && std::path::Path::new(&norm).extension().is_none() {
            norm.push_str(".walu");
        }
        normalized_files.insert(norm, source.clone());
    }

    let mut entry_norm = clean_path(entry_path);
    if !entry_norm.ends_with(".walu") && std::path::Path::new(&entry_norm).extension().is_none() {
        entry_norm.push_str(".walu");
    }

    let mut loader = Loader {
        files: &normalized_files,
        modules: Vec::new(),
        by_path: HashMap::new(),
        stack: Vec::new(),
    };

    let entry_id = loader.load(&entry_norm)?;
    merge(&loader.modules, entry_id)
}

struct Loader<'a> {
    files: &'a HashMap<String, String>,
    modules: Vec<LoadedModule>,
    by_path: HashMap<String, usize>,
    stack: Vec<String>,
}

impl<'a> Loader<'a> {
    fn load(&mut self, path: &str) -> Result<usize, String> {
        if let Some(&id) = self.by_path.get(path) {
            return Ok(id);
        }
        if self.stack.iter().any(|entry| entry == path) {
            let chain = self
                .stack
                .iter()
                .chain(std::iter::once(&path.to_string()))
                .cloned()
                .collect::<Vec<_>>()
                .join(" -> ");
            return Err(format!("circular module import: {chain}"));
        }

        let source = self
            .files
            .get(path)
            .ok_or_else(|| format!("cannot find module \"{}\"", path))?;
        let program =
            waluau_parser::parse(source).map_err(|e| format!("in module \"{}\": {}", path, e))?;

        let mut raw_paths = Vec::new();
        collect_require_paths(&program, &mut raw_paths);

        self.stack.push(path.to_string());
        let mut requires = HashMap::new();
        for raw in raw_paths {
            if requires.contains_key(&raw) {
                continue;
            }
            let resolved = resolve_path(path, &raw)?;
            let resolved = if self.files.contains_key(&resolved) {
                resolved
            } else {
                format!("{}.walu", resolved)
            };
            let target = self.load(&resolved)?;
            requires.insert(raw, target);
        }
        self.stack.pop();

        let id = self.modules.len();
        self.modules.push(LoadedModule { program, requires });
        self.by_path.insert(path.to_string(), id);
        Ok(id)
    }
}

fn module_prefix(id: usize, entry_id: usize) -> String {
    if id == entry_id {
        String::new()
    } else {
        format!("__waluau_m{id}_")
    }
}
fn merge(modules: &[LoadedModule], entry_id: usize) -> Result<Program, String> {
    let mut functions = Vec::new();
    let mut top_level = Vec::new();

    for (id, module) in modules.iter().enumerate() {
        if id != entry_id && !module.program.top_level.is_empty() {
            return Err(
                "imported modules may only contain functions and a trailing `return` export"
                    .to_string(),
            );
        }

        let prefix = module_prefix(id, entry_id);
        let mut module_functions = module.program.functions.clone();
        if let Some(export) = &module.program.export {
            hoist_table_export_functions(&mut module_functions, export)?;
        }
        let func_names: HashSet<String> = module_functions
            .iter()
            .map(|function| function.name.clone())
            .collect();

        let mut imports = HashMap::new();
        for (raw, &target_id) in &module.requires {
            imports.insert(
                raw.clone(),
                resolve_import(modules, target_id, entry_id, raw)?,
            );
        }

        let mut rewriter = Rewriter {
            prefix: &prefix,
            func_names: &func_names,
            imports: &imports,
            namespaces: HashMap::new(),
        };

        for function in &module_functions {
            let mut lowered = function.clone();
            let mut bound: HashSet<String> = lowered
                .params
                .iter()
                .map(|param| param.name.clone())
                .collect();
            rewriter.rewrite_block(&mut lowered.body, &mut bound);
            strip_unused_namespace_lets(&mut lowered.body);
            lowered.name = format!("{prefix}{}", function.name);
            functions.push(lowered);
        }

        if id == entry_id {
            let mut lowered = module.program.top_level.clone();
            let mut bound = HashSet::new();
            rewriter.rewrite_block(&mut lowered, &mut bound);
            strip_unused_namespace_lets(&mut lowered);
            top_level = lowered;
        }
    }

    Ok(Program {
        functions,
        top_level,
        export: None,
    })
}

enum ResolvedImport {
    Function(String),
    Namespace(BTreeMap<String, String>),
}

fn hoist_table_export_functions(
    functions: &mut Vec<Function>,
    export: &Expr,
) -> Result<(), String> {
    let Expr::TableLiteral { fields } = export else {
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
        name: name.to_string(),
        type_params: function.type_params.clone(),
        params: function.params.clone(),
        return_type: function.return_type.clone(),
        body: function.body.clone(),
    }
}

fn resolve_import(
    modules: &[LoadedModule],
    target_id: usize,
    entry_id: usize,
    raw: &str,
) -> Result<ResolvedImport, String> {
    let prefix = module_prefix(target_id, entry_id);
    let module = &modules[target_id];
    let mut top_level_names: HashSet<String> = module
        .program
        .functions
        .iter()
        .map(|function| function.name.clone())
        .collect();
    if let Some(Expr::TableLiteral { fields }) = &module.program.export {
        for field in fields {
            if matches!(field.value, Expr::Function(_)) {
                top_level_names.insert(field.name.clone());
            }
        }
    }

    match &module.program.export {
        Some(Expr::Name(name)) => {
            if top_level_names.contains(name) {
                Ok(ResolvedImport::Function(format!("{prefix}{name}")))
            } else {
                Err(format!(
                    "module imported via \"{raw}\" exports unknown function '{name}'"
                ))
            }
        }
        Some(Expr::TableLiteral { fields }) => {
            let mut namespace = BTreeMap::new();
            for field in fields {
                let function_name = export_field_function_name(field, &top_level_names, raw)?;
                namespace.insert(field.name.clone(), format!("{prefix}{function_name}"));
            }
            if namespace.is_empty() {
                return Err(format!(
                    "module imported via \"{raw}\" exports an empty table"
                ));
            }
            Ok(ResolvedImport::Namespace(namespace))
        }
        Some(_) => Err(format!(
            "module imported via \"{raw}\" must export a function name or table of functions"
        )),
        None => Err(format!(
            "module imported via \"{raw}\" has no export; add `return <function>` or `return {{ ... }}`"
        )),
    }
}

fn export_field_function_name(
    field: &TableField,
    top_level_names: &HashSet<String>,
    raw: &str,
) -> Result<String, String> {
    match &field.value {
        Expr::Name(name) => {
            if top_level_names.contains(name) {
                Ok(name.clone())
            } else {
                Err(format!(
                    "module imported via \"{raw}\" exports unknown function '{name}' in field '{}'",
                    field.name
                ))
            }
        }
        Expr::Function(_) => Ok(field.name.clone()),
        _ => Err(format!(
            "module imported via \"{raw}\" field '{}' must be a function name or `function ... end`",
            field.name
        )),
    }
}

/// Rewrites a single module's bodies: mangles references to its own top-level
/// functions and replaces `require(...)` with resolved imports.
struct Rewriter<'a> {
    prefix: &'a str,
    func_names: &'a HashSet<String>,
    imports: &'a HashMap<String, ResolvedImport>,
    namespaces: HashMap<String, BTreeMap<String, String>>,
}

impl Rewriter<'_> {
    fn rewrite_block(&mut self, stmts: &mut [Stmt], bound: &mut HashSet<String>) {
        for stmt in stmts {
            self.rewrite_stmt(stmt, bound);
        }
    }

    fn rewrite_stmt(&mut self, stmt: &mut Stmt, bound: &mut HashSet<String>) {
        match stmt {
            Stmt::Let { name, value, .. } => {
                self.rewrite_expr(value, bound);
                if let Expr::TableLiteral { fields } = &*value {
                    let mut field_map = BTreeMap::new();
                    for field in fields {
                        if let Expr::Name(function_name) = &field.value {
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
            Stmt::Break | Stmt::Continue => {}
        }
    }

    fn rewrite_expr(&mut self, expr: &mut Expr, bound: &HashSet<String>) {
        if let Expr::Field { base, name: field } = expr {
            if let Expr::Name(local) = &**base {
                if let Some(fields) = self.namespaces.get(local) {
                    if let Some(resolved) = fields.get(field) {
                        *expr = Expr::Name(resolved.clone());
                        return;
                    }
                }
            }
        }

        match expr {
            Expr::Require(path) => {
                if let Some(resolved) = self.imports.get(path) {
                    *expr = match resolved {
                        ResolvedImport::Function(name) => Expr::Name(name.clone()),
                        ResolvedImport::Namespace(fields) => Expr::TableLiteral {
                            fields: fields
                                .iter()
                                .map(|(name, function)| TableField {
                                    name: name.clone(),
                                    value: Expr::Name(function.clone()),
                                })
                                .collect(),
                        },
                    };
                }
            }
            Expr::Name(name) => {
                if !bound.contains(name) && self.func_names.contains(name) {
                    *name = format!("{}{name}", self.prefix);
                }
            }
            Expr::Number(_) | Expr::Bool(_) | Expr::String(_) => {}
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
            Expr::Call {
                callee,
                type_args: _,
                args,
            } => {
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
            Expr::TableLiteral { fields } => {
                for field in fields {
                    self.rewrite_expr(&mut field.value, bound);
                }
            }
            Expr::Field { base, .. } => self.rewrite_expr(base, bound),
            Expr::Index { base, index } => {
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
        Expr::Name(local) => local == name,
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => expr_mentions_name(name, expr),
        Expr::Binary { left, right, .. } => {
            expr_mentions_name(name, left) || expr_mentions_name(name, right)
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
        } => {
            expr_mentions_name(name, condition)
                || expr_mentions_name(name, then_expr)
                || expr_mentions_name(name, else_expr)
        }
        Expr::Call {
            callee,
            args,
            type_args: _,
        } => {
            expr_mentions_name(name, callee) || args.iter().any(|arg| expr_mentions_name(name, arg))
        }
        Expr::Function(function) => stmt_mentions_name(name, &function.body),
        Expr::ArrayLiteral { elements } => elements.iter().any(|el| expr_mentions_name(name, el)),
        Expr::TableLiteral { fields } => fields
            .iter()
            .any(|field| expr_mentions_name(name, &field.value)),
        Expr::Field { base, .. } => expr_mentions_name(name, base),
        Expr::Index { base, index } => {
            expr_mentions_name(name, base) || expr_mentions_name(name, index)
        }
        Expr::Require(_) | Expr::Number(_) | Expr::Bool(_) | Expr::String(_) => false,
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
            Stmt::Break | Stmt::Continue => {}
        }
    }
}

fn collect_expr(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::Require(path) => out.push(path.clone()),
        Expr::Name(_) | Expr::Number(_) | Expr::Bool(_) | Expr::String(_) => {}
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
        Expr::Call {
            callee,
            type_args: _,
            args,
        } => {
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
        Expr::TableLiteral { fields } => {
            for field in fields {
                collect_expr(&field.value, out);
            }
        }
        Expr::Field { base, .. } => collect_expr(base, out),
        Expr::Index { base, index } => {
            collect_expr(base, out);
            collect_expr(index, out);
        }
    }
}
