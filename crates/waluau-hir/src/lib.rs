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

fn resolve_program_types(program: &mut Program) -> Result<(), Diagnostic> {
    let raw_decls = program
        .type_declarations
        .iter()
        .map(|decl| (decl.name.clone(), decl.ty.clone()))
        .collect::<HashMap<_, _>>();
    if raw_decls.len() != program.type_declarations.len() {
        let mut seen = HashSet::new();
        for decl in &program.type_declarations {
            if !seen.insert(decl.name.clone()) {
                return Err(Diagnostic::new(format!(
                    "duplicate type declaration '{}'",
                    decl.name
                )));
            }
        }
    }

    let mut resolved = HashMap::new();
    let mut stack = Vec::new();
    for decl in &program.type_declarations {
        let resolved_ty = resolve_decl_type(&decl.name, &raw_decls, &mut resolved, &mut stack)?;
        resolved.insert(decl.name.clone(), resolved_ty);
    }

    for decl in &mut program.type_declarations {
        decl.ty = resolve_type_refs(&decl.ty, &resolved)?;
    }
    for function in &mut program.functions {
        resolve_function_type_refs(function, &resolved)?;
    }
    for stmt in &mut program.top_level {
        resolve_stmt_type_refs(stmt, &resolved)?;
    }
    if let Some(export) = &mut program.export {
        resolve_expr_type_refs(export, &resolved)?;
    }
    Ok(())
}

fn resolve_decl_type(
    name: &str,
    raw_decls: &HashMap<String, Type>,
    resolved: &mut HashMap<String, Type>,
    stack: &mut Vec<String>,
) -> Result<Type, Diagnostic> {
    if let Some(ty) = resolved.get(name) {
        return Ok(ty.clone());
    }
    if stack.iter().any(|entry| entry == name) {
        let cycle = stack
            .iter()
            .cloned()
            .chain(std::iter::once(name.to_string()))
            .collect::<Vec<_>>()
            .join(" -> ");
        return Err(Diagnostic::new(format!(
            "cyclic type declaration detected: {cycle}"
        )));
    }
    let raw_ty = raw_decls
        .get(name)
        .cloned()
        .ok_or_else(|| Diagnostic::new(format!("unknown type '{name}'")))?;
    stack.push(name.to_string());
    let resolved_underlying =
        resolve_type_refs_with_decl_cache(&raw_ty, raw_decls, resolved, stack)?;
    stack.pop();
    let opaque = Type::Opaque {
        name: name.to_string(),
        ty: Box::new(resolved_underlying),
    };
    resolved.insert(name.to_string(), opaque.clone());
    Ok(opaque)
}

fn resolve_type_refs_with_decl_cache(
    ty: &Type,
    raw_decls: &HashMap<String, Type>,
    resolved: &mut HashMap<String, Type>,
    stack: &mut Vec<String>,
) -> Result<Type, Diagnostic> {
    match ty {
        Type::Named(name) => resolve_decl_type(name, raw_decls, resolved, stack),
        Type::Opaque { name, ty } => Ok(Type::Opaque {
            name: name.clone(),
            ty: Box::new(resolve_type_refs_with_decl_cache(
                ty, raw_decls, resolved, stack,
            )?),
        }),
        Type::Array(inner) => Ok(Type::Array(Box::new(resolve_type_refs_with_decl_cache(
            inner, raw_decls, resolved, stack,
        )?))),
        Type::Multi(types) => Ok(Type::Multi(
            types
                .iter()
                .map(|ty| resolve_type_refs_with_decl_cache(ty, raw_decls, resolved, stack))
                .collect::<Result<_, _>>()?,
        )),
        Type::Function {
            params,
            return_type,
        } => Ok(Type::Function {
            params: params
                .iter()
                .map(|param| resolve_type_refs_with_decl_cache(param, raw_decls, resolved, stack))
                .collect::<Result<_, _>>()?,
            return_type: Box::new(resolve_type_refs_with_decl_cache(
                return_type,
                raw_decls,
                resolved,
                stack,
            )?),
        }),
        Type::Record(fields) => Ok(Type::Record(
            fields
                .iter()
                .map(|(name, ty)| {
                    Ok((
                        name.clone(),
                        resolve_type_refs_with_decl_cache(ty, raw_decls, resolved, stack)?,
                    ))
                })
                .collect::<Result<_, Diagnostic>>()?,
        )),
        other => Ok(other.clone()),
    }
}

fn resolve_type_refs(ty: &Type, resolved: &HashMap<String, Type>) -> Result<Type, Diagnostic> {
    match ty {
        Type::Named(name) => resolved
            .get(name)
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("unknown type '{name}'"))),
        Type::Opaque { name, ty } => Ok(Type::Opaque {
            name: name.clone(),
            ty: Box::new(resolve_type_refs(ty, resolved)?),
        }),
        Type::Array(inner) => Ok(Type::Array(Box::new(resolve_type_refs(inner, resolved)?))),
        Type::Multi(types) => Ok(Type::Multi(
            types
                .iter()
                .map(|ty| resolve_type_refs(ty, resolved))
                .collect::<Result<_, _>>()?,
        )),
        Type::Function {
            params,
            return_type,
        } => Ok(Type::Function {
            params: params
                .iter()
                .map(|param| resolve_type_refs(param, resolved))
                .collect::<Result<_, _>>()?,
            return_type: Box::new(resolve_type_refs(return_type, resolved)?),
        }),
        Type::Record(fields) => Ok(Type::Record(
            fields
                .iter()
                .map(|(name, ty)| Ok((name.clone(), resolve_type_refs(ty, resolved)?)))
                .collect::<Result<_, Diagnostic>>()?,
        )),
        other => Ok(other.clone()),
    }
}

fn resolve_function_type_refs(
    function: &mut Function,
    resolved: &HashMap<String, Type>,
) -> Result<(), Diagnostic> {
    for param in &mut function.params {
        param.ty = resolve_type_refs(&param.ty, resolved)?;
    }
    if let Some(return_type) = &mut function.return_type {
        *return_type = resolve_type_refs(return_type, resolved)?;
    }
    for stmt in &mut function.body {
        resolve_stmt_type_refs(stmt, resolved)?;
    }
    Ok(())
}

fn resolve_stmt_type_refs(
    stmt: &mut Stmt,
    resolved: &HashMap<String, Type>,
) -> Result<(), Diagnostic> {
    match stmt {
        Stmt::Let { ty, value, .. } => {
            if let Some(local_ty) = ty {
                *local_ty = resolve_type_refs(local_ty, resolved)?;
            }
            resolve_expr_type_refs(value, resolved)
        }
        Stmt::Assign { value, .. } | Stmt::Expr(value) | Stmt::Return(value) => {
            resolve_expr_type_refs(value, resolved)
        }
        Stmt::IndexAssign {
            base, index, value, ..
        } => {
            resolve_expr_type_refs(base, resolved)?;
            resolve_expr_type_refs(index, resolved)?;
            resolve_expr_type_refs(value, resolved)
        }
        Stmt::FieldAssign { base, value, .. } => {
            resolve_expr_type_refs(base, resolved)?;
            resolve_expr_type_refs(value, resolved)
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            resolve_expr_type_refs(condition, resolved)?;
            for stmt in then_body {
                resolve_stmt_type_refs(stmt, resolved)?;
            }
            for stmt in else_body {
                resolve_stmt_type_refs(stmt, resolved)?;
            }
            Ok(())
        }
        Stmt::While { condition, body } => {
            resolve_expr_type_refs(condition, resolved)?;
            for stmt in body {
                resolve_stmt_type_refs(stmt, resolved)?;
            }
            Ok(())
        }
        Stmt::Repeat { body, condition } => {
            for stmt in body {
                resolve_stmt_type_refs(stmt, resolved)?;
            }
            resolve_expr_type_refs(condition, resolved)
        }
        Stmt::NumericFor {
            start,
            stop,
            step,
            body,
            ..
        } => {
            resolve_expr_type_refs(start, resolved)?;
            resolve_expr_type_refs(stop, resolved)?;
            if let Some(step) = step {
                resolve_expr_type_refs(step, resolved)?;
            }
            for stmt in body {
                resolve_stmt_type_refs(stmt, resolved)?;
            }
            Ok(())
        }
        Stmt::ForIn { iterator, body, .. } => {
            resolve_expr_type_refs(iterator, resolved)?;
            for stmt in body {
                resolve_stmt_type_refs(stmt, resolved)?;
            }
            Ok(())
        }
        Stmt::ReturnMulti(values) | Stmt::AssignMulti { values, .. } => {
            for value in values {
                resolve_expr_type_refs(value, resolved)?;
            }
            Ok(())
        }
        Stmt::LetMulti { bindings, values } => {
            for binding in bindings {
                if let Some(ty) = &mut binding.ty {
                    *ty = resolve_type_refs(ty, resolved)?;
                }
            }
            for value in values {
                resolve_expr_type_refs(value, resolved)?;
            }
            Ok(())
        }
        Stmt::Break | Stmt::Continue => Ok(()),
    }
}

fn resolve_expr_type_refs(
    expr: &mut Expr,
    resolved: &HashMap<String, Type>,
) -> Result<(), Diagnostic> {
    match expr {
        Expr::Unary { expr, .. } => resolve_expr_type_refs(expr, resolved),
        Expr::Cast { expr, ty, .. } => {
            resolve_expr_type_refs(expr, resolved)?;
            *ty = resolve_type_refs(ty, resolved)?;
            Ok(())
        }
        Expr::Binary { left, right, .. } => {
            resolve_expr_type_refs(left, resolved)?;
            resolve_expr_type_refs(right, resolved)
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            resolve_expr_type_refs(condition, resolved)?;
            resolve_expr_type_refs(then_expr, resolved)?;
            resolve_expr_type_refs(else_expr, resolved)
        }
        Expr::Call {
            callee,
            type_args,
            args,
            ..
        } => {
            resolve_expr_type_refs(callee, resolved)?;
            for ty in type_args {
                *ty = resolve_type_refs(ty, resolved)?;
            }
            for arg in args {
                resolve_expr_type_refs(arg, resolved)?;
            }
            Ok(())
        }
        Expr::MethodCall { receiver, args, .. } => {
            resolve_expr_type_refs(receiver, resolved)?;
            for arg in args {
                resolve_expr_type_refs(arg, resolved)?;
            }
            Ok(())
        }
        Expr::Function(function) => resolve_function_expr_type_refs(function, resolved),
        Expr::ArrayLiteral { elements, .. } => {
            for element in elements {
                resolve_expr_type_refs(element, resolved)?;
            }
            Ok(())
        }
        Expr::TableLiteral { fields, .. } => {
            for field in fields {
                resolve_expr_type_refs(&mut field.value, resolved)?;
            }
            Ok(())
        }
        Expr::Field { base, .. } => resolve_expr_type_refs(base, resolved),
        Expr::Index { base, index, .. } => {
            resolve_expr_type_refs(base, resolved)?;
            resolve_expr_type_refs(index, resolved)
        }
        Expr::Number(..)
        | Expr::Bool(..)
        | Expr::String(..)
        | Expr::Bytes(..)
        | Expr::Name(..)
        | Expr::Require(..) => Ok(()),
    }
}

fn resolve_function_expr_type_refs(
    function: &mut FunctionExpr,
    resolved: &HashMap<String, Type>,
) -> Result<(), Diagnostic> {
    for param in &mut function.params {
        param.ty = resolve_type_refs(&param.ty, resolved)?;
    }
    if let Some(return_type) = &mut function.return_type {
        *return_type = resolve_type_refs(return_type, resolved)?;
    }
    for stmt in &mut function.body {
        resolve_stmt_type_refs(stmt, resolved)?;
    }
    Ok(())
}

fn desugar_method_declarations(program: &Program) -> Result<Program, Diagnostic> {
    let mut rewritten = program.clone();
    rewritten.functions.clear();
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
    let mut typed = desugar_method_declarations(program)?;
    resolve_program_types(&mut typed)?;
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
