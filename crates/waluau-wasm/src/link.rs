use std::collections::{HashMap, HashSet};
use waluau_ast::{Expr, Program, Stmt};

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
                "imported modules may only contain functions and a trailing `return <function>`"
                    .to_string(),
            );
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

fn exported_function(
    modules: &[LoadedModule],
    target_id: usize,
    raw: &str,
) -> Result<String, String> {
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
                Err(format!(
                    "module imported via \"{raw}\" exports unknown function '{name}'"
                ))
            }
        }
        Some(_) => Err(format!(
            "module imported via \"{raw}\" must export a function name, e.g. `return myFunction`"
        )),
        None => Err(format!(
            "module imported via \"{raw}\" has no export; add `return <function>`"
        )),
    }
}

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
            Stmt::Break | Stmt::Continue => {}
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
