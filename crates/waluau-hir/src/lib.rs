use std::collections::{HashMap, HashSet};

use waluau_ast::{
    AssignOp, Expr, Function, FunctionExpr, FunctionName, NumberLiteral, Param, Program,
    Rebindability, Stmt, Type,
};
use waluau_diagnostics::{Diagnostic, DiagnosticCategory};

mod builtins;
mod expressions;
mod numeric;
mod signatures;
mod statements;

use signatures::{
    FnSignature, GenericScheme, active_type_param_set, infer_function_expr_return_type,
    infer_top_level_function_return_type, inference_diagnostic,
};
use statements::{check_function, check_stmt};

#[derive(Clone)]
struct Binding {
    ty: Type,
    rebindability: Rebindability,
    record_open: bool,
}

fn binding_for(ty: Type, rebindability: Rebindability) -> Binding {
    let record_open = matches!(ty, Type::Record(_));
    Binding {
        ty,
        rebindability,
        record_open,
    }
}

#[derive(Clone)]
struct AliasDecl {
    type_params: Vec<String>,
    ty: Type,
}

fn alias_diagnostic(
    code: &'static str,
    message: impl Into<String>,
    action: impl Into<String>,
) -> Diagnostic {
    Diagnostic::new(message)
        .with_code(code)
        .with_category(DiagnosticCategory::Unsupported)
        .with_action(action)
}

fn substitute_type_params(ty: &Type, subst: &HashMap<String, Type>) -> Type {
    match ty {
        Type::TypeParam(name) => subst
            .get(name)
            .cloned()
            .unwrap_or_else(|| Type::TypeParam(name.clone())),
        Type::Array(inner) => Type::Array(Box::new(substitute_type_params(inner, subst))),
        Type::Multi(types) => Type::Multi(
            types
                .iter()
                .map(|inner| substitute_type_params(inner, subst))
                .collect(),
        ),
        Type::Function {
            params,
            return_type,
        } => Type::Function {
            params: params
                .iter()
                .map(|param| substitute_type_params(param, subst))
                .collect(),
            return_type: Box::new(substitute_type_params(return_type, subst)),
        },
        Type::Record(fields) => Type::Record(
            fields
                .iter()
                .map(|(name, ty)| (name.clone(), substitute_type_params(ty, subst)))
                .collect(),
        ),
        Type::Named { name, type_args } => Type::Named {
            name: name.clone(),
            type_args: type_args
                .iter()
                .map(|arg| substitute_type_params(arg, subst))
                .collect(),
        },
        other => other.clone(),
    }
}

fn resolve_type_aliases(program: &Program) -> Result<Program, Diagnostic> {
    let mut aliases = HashMap::new();
    for alias in &program.type_aliases {
        if aliases
            .insert(
                alias.name.clone(),
                AliasDecl {
                    type_params: alias.type_params.clone(),
                    ty: alias.ty.clone(),
                },
            )
            .is_some()
        {
            return Err(alias_diagnostic(
                "alias/duplicate",
                format!("duplicate type alias '{}'", alias.name),
                "rename or remove the duplicate type alias",
            ));
        }
    }

    let mut resolved = program.clone();
    for alias in &mut resolved.type_aliases {
        let alias_params = active_type_param_set(&alias.type_params);
        alias.ty = resolve_type(
            &alias.ty,
            &alias_params,
            &aliases,
            &mut vec![alias.name.clone()],
        )?;
    }
    for function in &mut resolved.functions {
        resolve_function_types(function, &aliases, &HashSet::new())?;
    }
    for stmt in &mut resolved.top_level {
        resolve_stmt_types(stmt, &aliases, &HashSet::new())?;
    }
    if let Some(export) = &mut resolved.export {
        resolve_expr_types(export, &aliases, &HashSet::new())?;
    }
    Ok(resolved)
}

fn resolve_type(
    ty: &Type,
    active_type_params: &HashSet<String>,
    aliases: &HashMap<String, AliasDecl>,
    alias_stack: &mut Vec<String>,
) -> Result<Type, Diagnostic> {
    match ty {
        Type::Array(inner) => Ok(Type::Array(Box::new(resolve_type(
            inner,
            active_type_params,
            aliases,
            alias_stack,
        )?))),
        Type::Multi(types) => Ok(Type::Multi(
            types
                .iter()
                .map(|ty| resolve_type(ty, active_type_params, aliases, alias_stack))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Type::Function {
            params,
            return_type,
        } => Ok(Type::Function {
            params: params
                .iter()
                .map(|param| resolve_type(param, active_type_params, aliases, alias_stack))
                .collect::<Result<Vec<_>, _>>()?,
            return_type: Box::new(resolve_type(
                return_type,
                active_type_params,
                aliases,
                alias_stack,
            )?),
        }),
        Type::Record(fields) => Ok(Type::Record(
            fields
                .iter()
                .map(|(name, ty)| {
                    Ok((
                        name.clone(),
                        resolve_type(ty, active_type_params, aliases, alias_stack)?,
                    ))
                })
                .collect::<Result<_, Diagnostic>>()?,
        )),
        Type::Named { name, type_args } => {
            let resolved_args = type_args
                .iter()
                .map(|arg| resolve_type(arg, active_type_params, aliases, alias_stack))
                .collect::<Result<Vec<_>, _>>()?;
            if active_type_params.contains(name) {
                if resolved_args.is_empty() {
                    return Ok(Type::TypeParam(name.clone()));
                }
                return Err(alias_diagnostic(
                    "generic/unknown-type-param",
                    format!("type parameter '{name}' cannot be used with type arguments"),
                    "remove the type arguments or use a named type alias",
                ));
            }
            let Some(alias) = aliases.get(name) else {
                return Err(alias_diagnostic(
                    "alias/unknown",
                    format!("unknown type alias '{name}'"),
                    "declare the type alias before using it",
                ));
            };
            if resolved_args.len() != alias.type_params.len() {
                return Err(alias_diagnostic(
                    "alias/type-arg-count",
                    format!(
                        "type alias '{name}' expects {} type argument{}, got {}",
                        alias.type_params.len(),
                        if alias.type_params.len() == 1 {
                            ""
                        } else {
                            "s"
                        },
                        resolved_args.len()
                    ),
                    "match the number of type parameters declared on the type alias",
                ));
            }
            if alias_stack.contains(name) {
                return Err(alias_diagnostic(
                    "alias/cycle",
                    format!("cyclic type alias involving '{name}' is not supported"),
                    "rewrite the aliases to avoid recursion in this MVP",
                ));
            }
            let subst = alias
                .type_params
                .iter()
                .cloned()
                .zip(resolved_args)
                .collect::<HashMap<_, _>>();
            alias_stack.push(name.clone());
            let instantiated = substitute_type_params(&alias.ty, &subst);
            let resolved = resolve_type(&instantiated, active_type_params, aliases, alias_stack);
            alias_stack.pop();
            resolved
        }
        other => Ok(other.clone()),
    }
}

fn resolve_function_types(
    function: &mut Function,
    aliases: &HashMap<String, AliasDecl>,
    outer_type_params: &HashSet<String>,
) -> Result<(), Diagnostic> {
    let mut active_type_params = outer_type_params.clone();
    active_type_params.extend(active_type_param_set(&function.type_params));
    for param in &mut function.params {
        param.ty = resolve_type(&param.ty, &active_type_params, aliases, &mut Vec::new())?;
    }
    if let Some(return_type) = &mut function.return_type {
        *return_type = resolve_type(return_type, &active_type_params, aliases, &mut Vec::new())?;
    }
    for stmt in &mut function.body {
        resolve_stmt_types(stmt, aliases, &active_type_params)?;
    }
    Ok(())
}

fn resolve_stmt_types(
    stmt: &mut Stmt,
    aliases: &HashMap<String, AliasDecl>,
    active_type_params: &HashSet<String>,
) -> Result<(), Diagnostic> {
    match stmt {
        Stmt::Let { ty, value, .. } => {
            if let Some(ty) = ty {
                *ty = resolve_type(ty, active_type_params, aliases, &mut Vec::new())?;
            }
            resolve_expr_types(value, aliases, active_type_params)
        }
        Stmt::Assign { value, .. } | Stmt::Expr(value) | Stmt::Return(value) => {
            resolve_expr_types(value, aliases, active_type_params)
        }
        Stmt::FieldAssign { base, value, .. } => {
            resolve_expr_types(base, aliases, active_type_params)?;
            resolve_expr_types(value, aliases, active_type_params)
        }
        Stmt::IndexAssign {
            base, index, value, ..
        } => {
            resolve_expr_types(base, aliases, active_type_params)?;
            resolve_expr_types(index, aliases, active_type_params)?;
            resolve_expr_types(value, aliases, active_type_params)
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            resolve_expr_types(condition, aliases, active_type_params)?;
            for stmt in then_body {
                resolve_stmt_types(stmt, aliases, active_type_params)?;
            }
            for stmt in else_body {
                resolve_stmt_types(stmt, aliases, active_type_params)?;
            }
            Ok(())
        }
        Stmt::While { condition, body } => {
            resolve_expr_types(condition, aliases, active_type_params)?;
            for stmt in body {
                resolve_stmt_types(stmt, aliases, active_type_params)?;
            }
            Ok(())
        }
        Stmt::Repeat { body, condition } => {
            for stmt in body {
                resolve_stmt_types(stmt, aliases, active_type_params)?;
            }
            resolve_expr_types(condition, aliases, active_type_params)
        }
        Stmt::NumericFor {
            start,
            stop,
            step,
            body,
            ..
        } => {
            resolve_expr_types(start, aliases, active_type_params)?;
            resolve_expr_types(stop, aliases, active_type_params)?;
            if let Some(step) = step {
                resolve_expr_types(step, aliases, active_type_params)?;
            }
            for stmt in body {
                resolve_stmt_types(stmt, aliases, active_type_params)?;
            }
            Ok(())
        }
        Stmt::ForIn { iterator, body, .. } => {
            resolve_expr_types(iterator, aliases, active_type_params)?;
            for stmt in body {
                resolve_stmt_types(stmt, aliases, active_type_params)?;
            }
            Ok(())
        }
        Stmt::ReturnMulti(values) | Stmt::AssignMulti { values, .. } => {
            for value in values {
                resolve_expr_types(value, aliases, active_type_params)?;
            }
            Ok(())
        }
        Stmt::LetMulti { bindings, values } => {
            for binding in bindings {
                if let Some(ty) = &mut binding.ty {
                    *ty = resolve_type(ty, active_type_params, aliases, &mut Vec::new())?;
                }
            }
            for value in values {
                resolve_expr_types(value, aliases, active_type_params)?;
            }
            Ok(())
        }
        Stmt::Break | Stmt::Continue => Ok(()),
    }
}

fn resolve_expr_types(
    expr: &mut Expr,
    aliases: &HashMap<String, AliasDecl>,
    active_type_params: &HashSet<String>,
) -> Result<(), Diagnostic> {
    match expr {
        Expr::Unary { expr, .. } => resolve_expr_types(expr, aliases, active_type_params),
        Expr::Cast { expr, ty, .. } => {
            resolve_expr_types(expr, aliases, active_type_params)?;
            *ty = resolve_type(ty, active_type_params, aliases, &mut Vec::new())?;
            Ok(())
        }
        Expr::Binary { left, right, .. } => {
            resolve_expr_types(left, aliases, active_type_params)?;
            resolve_expr_types(right, aliases, active_type_params)
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            resolve_expr_types(condition, aliases, active_type_params)?;
            resolve_expr_types(then_expr, aliases, active_type_params)?;
            resolve_expr_types(else_expr, aliases, active_type_params)
        }
        Expr::Call {
            callee,
            type_args,
            args,
            ..
        } => {
            resolve_expr_types(callee, aliases, active_type_params)?;
            for type_arg in type_args {
                *type_arg = resolve_type(type_arg, active_type_params, aliases, &mut Vec::new())?;
            }
            for arg in args {
                resolve_expr_types(arg, aliases, active_type_params)?;
            }
            Ok(())
        }
        Expr::MethodCall { receiver, args, .. } => {
            resolve_expr_types(receiver, aliases, active_type_params)?;
            for arg in args {
                resolve_expr_types(arg, aliases, active_type_params)?;
            }
            Ok(())
        }
        Expr::Function(function) => {
            let mut active = active_type_params.clone();
            active.extend(active_type_param_set(&function.type_params));
            for param in &mut function.params {
                param.ty = resolve_type(&param.ty, &active, aliases, &mut Vec::new())?;
            }
            if let Some(return_type) = &mut function.return_type {
                *return_type = resolve_type(return_type, &active, aliases, &mut Vec::new())?;
            }
            for stmt in &mut function.body {
                resolve_stmt_types(stmt, aliases, &active)?;
            }
            Ok(())
        }
        Expr::ArrayLiteral { elements, .. } => {
            for element in elements {
                resolve_expr_types(element, aliases, active_type_params)?;
            }
            Ok(())
        }
        Expr::TableLiteral { fields, .. } => {
            for field in fields {
                resolve_expr_types(&mut field.value, aliases, active_type_params)?;
            }
            Ok(())
        }
        Expr::Field { base, .. } => resolve_expr_types(base, aliases, active_type_params),
        Expr::Index { base, index, .. } => {
            resolve_expr_types(base, aliases, active_type_params)?;
            resolve_expr_types(index, aliases, active_type_params)
        }
        Expr::Name(..)
        | Expr::Number(..)
        | Expr::Bool(..)
        | Expr::String(..)
        | Expr::Bytes(..)
        | Expr::Require(..) => Ok(()),
    }
}

fn desugar_method_declarations(program: &Program) -> Result<Program, Diagnostic> {
    let mut rewritten = program.clone();
    rewritten.functions.clear();
    rewritten.type_aliases = program.type_aliases.clone();
    rewritten.top_level.clear();
    let mut pending_methods: Vec<(String, Stmt)> = Vec::new();

    for function in &program.functions {
        match &function.name {
            FunctionName::Simple(_) => rewritten.functions.push(function.clone()),
            FunctionName::Method { table, method } => {
                let mut params = Vec::with_capacity(function.params.len() + 1);
                params.push(Param {
                    name: "self".to_string(),
                    ty: Type::Unit,
                });
                params.extend(function.params.clone());
                pending_methods.push((
                    table.clone(),
                    Stmt::FieldAssign {
                        op: AssignOp::Set,
                        base: Box::new(Expr::Name(table.clone(), None)),
                        name: method.clone(),
                        value: Expr::Function(FunctionExpr {
                            name: None,
                            implicit_self: Some(table.clone()),
                            type_params: function.type_params.clone(),
                            params,
                            return_type: function.return_type.clone(),
                            body: function.body.clone(),
                            file_path: function.file_path.clone(),
                            span: None,
                        }),
                    },
                ));
            }
        }
    }

    for stmt in &program.top_level {
        rewritten.top_level.push(stmt.clone());
        let Stmt::Let { name, .. } = stmt else {
            continue;
        };
        let mut remaining = Vec::with_capacity(pending_methods.len());
        for (table, method_stmt) in pending_methods.drain(..) {
            if table == *name {
                rewritten.top_level.push(method_stmt);
            } else {
                remaining.push((table, method_stmt));
            }
        }
        pending_methods = remaining;
    }
    rewritten
        .top_level
        .extend(pending_methods.into_iter().map(|(_, stmt)| stmt));

    Ok(rewritten)
}

fn method_signature_name(table: &str, method: &str) -> String {
    format!("{table}.{method}")
}

fn signature_from_function_expr(function: &FunctionExpr) -> Option<FnSignature> {
    let return_type = function.return_type.clone()?;
    let params = function
        .params
        .iter()
        .map(|param| param.ty.clone())
        .collect();
    Some(if function.type_params.is_empty() {
        FnSignature::Mono {
            params,
            return_type,
        }
    } else {
        FnSignature::Generic(GenericScheme {
            type_params: function.type_params.clone(),
            params,
            return_type,
        })
    })
}

fn resolve_implicit_self_functions(
    stmts: &mut [Stmt],
    fn_signatures: &mut HashMap<String, FnSignature>,
) -> Result<(), Diagnostic> {
    let active_type_params = HashSet::new();
    let mut vars: HashMap<String, Binding> = HashMap::new();

    for stmt in stmts.iter_mut() {
        resolve_stmt_implicit_self(stmt, &vars, fn_signatures, &active_type_params)?;
        if let Stmt::FieldAssign {
            base,
            name,
            value: Expr::Function(function),
            ..
        } = stmt
        {
            if let Expr::Name(table, _) = base.as_ref() {
                if let Some(signature) = signature_from_function_expr(function) {
                    fn_signatures.insert(method_signature_name(table, name), signature);
                }
            }
        }
        let _ = check_stmt(
            stmt,
            &mut vars,
            fn_signatures,
            &active_type_params,
            &Type::number(),
            false,
        )?;
    }

    Ok(())
}

fn resolve_stmt_implicit_self(
    stmt: &mut Stmt,
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
) -> Result<(), Diagnostic> {
    match stmt {
        Stmt::FieldAssign { value, .. }
        | Stmt::Let { value, .. }
        | Stmt::Assign { value, .. }
        | Stmt::Expr(value)
        | Stmt::Return(value) => {
            resolve_expr_implicit_self(value, vars, fn_signatures, active_type_params)
        }
        Stmt::IndexAssign {
            base, index, value, ..
        } => {
            resolve_expr_implicit_self(base, vars, fn_signatures, active_type_params)?;
            resolve_expr_implicit_self(index, vars, fn_signatures, active_type_params)?;
            resolve_expr_implicit_self(value, vars, fn_signatures, active_type_params)
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            resolve_expr_implicit_self(condition, vars, fn_signatures, active_type_params)?;
            for stmt in then_body {
                resolve_stmt_implicit_self(stmt, vars, fn_signatures, active_type_params)?;
            }
            for stmt in else_body {
                resolve_stmt_implicit_self(stmt, vars, fn_signatures, active_type_params)?;
            }
            Ok(())
        }
        Stmt::While { condition, body } => {
            resolve_expr_implicit_self(condition, vars, fn_signatures, active_type_params)?;
            for stmt in body {
                resolve_stmt_implicit_self(stmt, vars, fn_signatures, active_type_params)?;
            }
            Ok(())
        }
        Stmt::Repeat { body, condition } => {
            for stmt in body {
                resolve_stmt_implicit_self(stmt, vars, fn_signatures, active_type_params)?;
            }
            resolve_expr_implicit_self(condition, vars, fn_signatures, active_type_params)
        }
        Stmt::NumericFor {
            start,
            stop,
            step,
            body,
            ..
        } => {
            resolve_expr_implicit_self(start, vars, fn_signatures, active_type_params)?;
            resolve_expr_implicit_self(stop, vars, fn_signatures, active_type_params)?;
            if let Some(step) = step {
                resolve_expr_implicit_self(step, vars, fn_signatures, active_type_params)?;
            }
            for stmt in body {
                resolve_stmt_implicit_self(stmt, vars, fn_signatures, active_type_params)?;
            }
            Ok(())
        }
        Stmt::ForIn { iterator, body, .. } => {
            resolve_expr_implicit_self(iterator, vars, fn_signatures, active_type_params)?;
            for stmt in body {
                resolve_stmt_implicit_self(stmt, vars, fn_signatures, active_type_params)?;
            }
            Ok(())
        }
        Stmt::ReturnMulti(values)
        | Stmt::LetMulti { values, .. }
        | Stmt::AssignMulti { values, .. } => {
            for value in values {
                resolve_expr_implicit_self(value, vars, fn_signatures, active_type_params)?;
            }
            Ok(())
        }
        Stmt::Break | Stmt::Continue => Ok(()),
    }
}

fn resolve_expr_implicit_self(
    expr: &mut Expr,
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
) -> Result<(), Diagnostic> {
    match expr {
        Expr::Function(function) => {
            let mut function_type_params = active_type_params.clone();
            function_type_params.extend(active_type_param_set(&function.type_params));
            if let Some(table_name) = function.implicit_self.clone() {
                let table_ty = vars
                    .get(&table_name)
                    .map(|binding| binding.ty.clone())
                    .ok_or_else(|| Diagnostic::new(format!("unknown name '{table_name}'")))?;
                if function.params.first().map(|param| param.name.as_str()) != Some("self") {
                    return Err(Diagnostic::new("desugared method is missing implicit self"));
                }
                function.params[0].ty = table_ty;
                if function.return_type.is_none() {
                    function.return_type = Some(infer_function_expr_return_type(
                        function,
                        vars,
                        fn_signatures,
                        &function_type_params,
                    )?);
                }
                function.implicit_self = None;
            }
            for stmt in &mut function.body {
                resolve_stmt_implicit_self(stmt, vars, fn_signatures, &function_type_params)?;
            }
            Ok(())
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => {
            resolve_expr_implicit_self(expr, vars, fn_signatures, active_type_params)
        }
        Expr::Binary { left, right, .. } => {
            resolve_expr_implicit_self(left, vars, fn_signatures, active_type_params)?;
            resolve_expr_implicit_self(right, vars, fn_signatures, active_type_params)
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            resolve_expr_implicit_self(condition, vars, fn_signatures, active_type_params)?;
            resolve_expr_implicit_self(then_expr, vars, fn_signatures, active_type_params)?;
            resolve_expr_implicit_self(else_expr, vars, fn_signatures, active_type_params)
        }
        Expr::Call { callee, args, .. } => {
            resolve_expr_implicit_self(callee, vars, fn_signatures, active_type_params)?;
            for arg in args {
                resolve_expr_implicit_self(arg, vars, fn_signatures, active_type_params)?;
            }
            Ok(())
        }
        Expr::MethodCall { receiver, args, .. } => {
            resolve_expr_implicit_self(receiver, vars, fn_signatures, active_type_params)?;
            for arg in args {
                resolve_expr_implicit_self(arg, vars, fn_signatures, active_type_params)?;
            }
            Ok(())
        }
        Expr::ArrayLiteral { elements, .. } => {
            for element in elements {
                resolve_expr_implicit_self(element, vars, fn_signatures, active_type_params)?;
            }
            Ok(())
        }
        Expr::TableLiteral { fields, .. } => {
            for field in fields {
                resolve_expr_implicit_self(
                    &mut field.value,
                    vars,
                    fn_signatures,
                    active_type_params,
                )?;
            }
            Ok(())
        }
        Expr::Field { base, .. } => {
            resolve_expr_implicit_self(base, vars, fn_signatures, active_type_params)
        }
        Expr::Index { base, index, .. } => {
            resolve_expr_implicit_self(base, vars, fn_signatures, active_type_params)?;
            resolve_expr_implicit_self(index, vars, fn_signatures, active_type_params)
        }
        Expr::Name(..)
        | Expr::Number(..)
        | Expr::Bool(..)
        | Expr::String(..)
        | Expr::Bytes(..)
        | Expr::Require(..) => Ok(()),
    }
}

pub fn type_check(program: &Program) -> Result<(), Diagnostic> {
    let _ = type_check_and_infer(program)?;
    Ok(())
}

pub fn type_check_and_infer(program: &Program) -> Result<Program, Diagnostic> {
    let resolved = resolve_type_aliases(program)?;
    let mut typed = desugar_method_declarations(&resolved)?;
    if !typed.top_level.is_empty() {
        typed.functions.push(Function {
            name: FunctionName::Simple("__waluau_top_level_init".to_string()),
            type_params: Vec::new(),
            params: Vec::new(),
            return_type: Some(Type::number()),
            body: {
                let mut body = typed.top_level.clone();
                body.push(Stmt::Return(Expr::Number(
                    NumberLiteral { raw: "0".into() },
                    None,
                )));
                body
            },
            file_path: typed.entry_file_path.clone(),
        });
    }

    let mut fn_signatures: HashMap<String, FnSignature> = HashMap::new();
    for function in &typed.functions {
        if function.type_params.is_empty() {
            if let Some(ret) = &function.return_type {
                fn_signatures.insert(
                    function.name.to_string(),
                    FnSignature::Mono {
                        params: function
                            .params
                            .iter()
                            .map(|param| param.ty.clone())
                            .collect(),
                        return_type: ret.clone(),
                    },
                );
            }
        } else if let Some(ret) = &function.return_type {
            fn_signatures.insert(
                function.name.to_string(),
                FnSignature::Generic(GenericScheme {
                    type_params: function.type_params.clone(),
                    params: function
                        .params
                        .iter()
                        .map(|param| param.ty.clone())
                        .collect(),
                    return_type: ret.clone(),
                }),
            );
        }
    }

    let mut unresolved: Vec<usize> = typed
        .functions
        .iter()
        .enumerate()
        .filter_map(|(idx, function)| {
            (function.return_type.is_none() && function.type_params.is_empty()).then_some(idx)
        })
        .collect();

    while !unresolved.is_empty() {
        let mut progressed = false;
        let mut next_unresolved = Vec::new();
        let unresolved_names: Vec<String> = unresolved
            .iter()
            .map(|idx| typed.functions[*idx].name.to_string())
            .collect();
        for idx in unresolved {
            let function = &typed.functions[idx];
            let function_name = function.name.to_string();
            let function_params: Vec<Type> = function
                .params
                .iter()
                .map(|param| param.ty.clone())
                .collect();
            match infer_top_level_function_return_type(function, &fn_signatures, &unresolved_names)?
            {
                Some(ret) => {
                    typed.functions[idx].return_type = Some(ret.clone());
                    fn_signatures.insert(
                        function_name,
                        FnSignature::Mono {
                            params: function_params,
                            return_type: ret,
                        },
                    );
                    progressed = true;
                }
                None => next_unresolved.push(idx),
            }
        }
        if !progressed {
            let name = &typed.functions[next_unresolved[0]].name;
            return Err(inference_diagnostic(
                "inference/unsupported",
                DiagnosticCategory::Unsupported,
                format!("cannot infer return type for recursive or cyclic function '{name}'"),
                "add an explicit return type annotation to break the cycle",
            ));
        }
        unresolved = next_unresolved;
    }

    if let Some(top_level_init) = typed
        .functions
        .iter_mut()
        .find(|function| function.name.to_string() == "__waluau_top_level_init")
    {
        resolve_implicit_self_functions(&mut top_level_init.body, &mut fn_signatures)?;
    }

    for function in &typed.functions {
        check_function(function, &fn_signatures, &HashSet::new())?;
    }

    Ok(typed)
}

#[cfg(test)]
mod tests;
