use std::collections::HashMap;

use waluau_ast::{
    AssignOp, BinaryOp, Expr, Function, FunctionExpr, NumberLiteral, NumericType, Program,
    Rebindability, Stmt, Type, UnaryOp,
};
use waluau_diagnostics::{Diagnostic, DiagnosticCategory};

const COROUTINE_CREATE: &str = "coroutine_create";
const COROUTINE_RESUME: &str = "coroutine_resume";
const COROUTINE_STATUS: &str = "coroutine_status";
const MATH_ABS: &str = "math_abs";
const MATH_MIN: &str = "math_min";
const MATH_MAX: &str = "math_max";
const MATH_SQRT: &str = "math_sqrt";
const MATH_FLOOR: &str = "math_floor";
const MATH_CEIL: &str = "math_ceil";
const MATH_TRUNC: &str = "math_trunc";
const MATH_NEAREST: &str = "math_nearest";
const MATH_COPYSIGN: &str = "math_copysign";
const ASSERT: &str = "assert";

fn inference_diagnostic(
    code: &'static str,
    category: DiagnosticCategory,
    message: impl Into<String>,
    action: impl Into<String>,
) -> Diagnostic {
    Diagnostic::new(message)
        .with_code(code)
        .with_category(category)
        .with_action(action)
}

#[derive(Clone)]
struct Binding {
    ty: Type,
    rebindability: Rebindability,
}

pub fn type_check(program: &Program) -> Result<(), Diagnostic> {
    let _ = type_check_and_infer(program)?;
    Ok(())
}

pub fn type_check_and_infer(program: &Program) -> Result<Program, Diagnostic> {
    let mut typed = program.clone();
    if !typed.top_level.is_empty() {
        typed.functions.push(Function {
            name: "__waluau_top_level_init".to_string(),
            params: Vec::new(),
            return_type: Some(Type::number()),
            body: {
                let mut body = typed.top_level.clone();
                body.push(Stmt::Return(Expr::Number(NumberLiteral {
                    raw: "0".into(),
                })));
                body
            },
        });
    }
    let mut signatures: HashMap<String, (Vec<Type>, Type)> = HashMap::new();
    for function in &typed.functions {
        if let Some(ret) = &function.return_type {
            signatures.insert(
                function.name.clone(),
                (
                    function
                        .params
                        .iter()
                        .map(|param| param.ty.clone())
                        .collect(),
                    ret.clone(),
                ),
            );
        }
    }

    let mut unresolved: Vec<usize> = typed
        .functions
        .iter()
        .enumerate()
        .filter_map(|(idx, function)| function.return_type.is_none().then_some(idx))
        .collect();

    while !unresolved.is_empty() {
        let mut progressed = false;
        let mut next_unresolved = Vec::new();
        let unresolved_names: Vec<String> = unresolved
            .iter()
            .map(|idx| typed.functions[*idx].name.clone())
            .collect();
        for idx in unresolved {
            let function = &typed.functions[idx];
            let function_name = function.name.clone();
            let function_params: Vec<Type> = function
                .params
                .iter()
                .map(|param| param.ty.clone())
                .collect();
            match infer_top_level_function_return_type(function, &signatures, &unresolved_names)? {
                Some(ret) => {
                    typed.functions[idx].return_type = Some(ret.clone());
                    signatures.insert(function_name, (function_params, ret));
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

    for function in &typed.functions {
        check_function(function, &signatures)?;
    }

    Ok(typed)
}

fn check_function(
    function: &Function,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
) -> Result<(), Diagnostic> {
    let expected_return = function.return_type.clone().ok_or_else(|| {
        Diagnostic::new(format!(
            "cannot infer return type for recursive or cyclic function '{}'",
            function.name
        ))
    })?;
    let mut vars: HashMap<String, Binding> = HashMap::new();
    for param in &function.params {
        vars.insert(
            param.name.clone(),
            Binding {
                ty: param.ty.clone(),
                rebindability: Rebindability::Rebindable,
            },
        );
    }

    let mut saw_return = false;
    for stmt in &function.body {
        if check_stmt(stmt, &mut vars, signatures, &expected_return, false)? {
            saw_return = true;
        }
    }
    if !saw_return {
        return Err(Diagnostic::new(format!(
            "function '{}' is missing a return",
            function.name
        )));
    }
    Ok(())
}

fn infer_top_level_function_return_type(
    function: &Function,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
    unresolved_names: &[String],
) -> Result<Option<Type>, Diagnostic> {
    let mut vars: HashMap<String, Binding> = HashMap::new();
    for param in &function.params {
        vars.insert(
            param.name.clone(),
            Binding {
                ty: param.ty.clone(),
                rebindability: Rebindability::Rebindable,
            },
        );
    }

    let mut returns = Vec::new();
    if unresolved_names.iter().any(|name| name == &function.name)
        && function_calls(function, &function.name)
    {
        return Ok(None);
    }
    if let Err(error) = collect_return_types(&function.body, &vars, signatures, &mut returns) {
        let message = error.to_string();
        if unresolved_names
            .iter()
            .any(|name| message.contains(&format!("unknown name '{name}'")))
        {
            return Ok(None);
        }
        return Err(error);
    }
    if returns.is_empty() {
        return Err(Diagnostic::new(format!(
            "function '{}' is missing a return",
            function.name
        )));
    }
    let mut merged = returns[0].clone();
    for ty in returns.into_iter().skip(1) {
        merged = common_return_type(merged, ty)?;
    }
    Ok(Some(merged))
}

fn function_calls(function: &Function, callee: &str) -> bool {
    function
        .body
        .iter()
        .any(|stmt| stmt_calls_name(stmt, callee))
}

fn stmt_calls_name(stmt: &Stmt, callee: &str) -> bool {
    match stmt {
        Stmt::Let { value, .. }
        | Stmt::Assign { value, .. }
        | Stmt::Expr(value)
        | Stmt::Return(value) => expr_calls_name(value, callee),
        Stmt::ReturnMulti(values) => values.iter().any(|value| expr_calls_name(value, callee)),
        Stmt::LetMulti { values, .. } | Stmt::AssignMulti { values, .. } => {
            values.iter().any(|value| expr_calls_name(value, callee))
        }
        Stmt::IndexAssign {
            base, index, value, ..
        } => {
            expr_calls_name(base, callee)
                || expr_calls_name(index, callee)
                || expr_calls_name(value, callee)
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            expr_calls_name(condition, callee)
                || then_body.iter().any(|stmt| stmt_calls_name(stmt, callee))
                || else_body.iter().any(|stmt| stmt_calls_name(stmt, callee))
        }
        Stmt::While { condition, body } => {
            expr_calls_name(condition, callee)
                || body.iter().any(|stmt| stmt_calls_name(stmt, callee))
        }
        Stmt::Repeat { body, condition } => {
            body.iter().any(|stmt| stmt_calls_name(stmt, callee))
                || expr_calls_name(condition, callee)
        }
        Stmt::Break | Stmt::Continue => false,
    }
}

fn expr_calls_name(expr: &Expr, callee: &str) -> bool {
    match expr {
        Expr::Name(_) | Expr::Number(_) | Expr::Bool(_) | Expr::Require(_) => false,
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => expr_calls_name(expr, callee),
        Expr::Binary { left, right, .. } => {
            expr_calls_name(left, callee) || expr_calls_name(right, callee)
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
        } => {
            expr_calls_name(condition, callee)
                || expr_calls_name(then_expr, callee)
                || expr_calls_name(else_expr, callee)
        }
        Expr::Call {
            callee: called,
            args,
        } => {
            matches!(called.as_ref(), Expr::Name(name) if name == callee)
                || expr_calls_name(called, callee)
                || args.iter().any(|arg| expr_calls_name(arg, callee))
        }
        Expr::Function(function) => function
            .body
            .iter()
            .any(|stmt| stmt_calls_name(stmt, callee)),
        Expr::ArrayLiteral { elements } => elements.iter().any(|el| expr_calls_name(el, callee)),
        Expr::Index { base, index } => {
            expr_calls_name(base, callee) || expr_calls_name(index, callee)
        }
    }
}

fn collect_return_types(
    body: &[Stmt],
    vars: &HashMap<String, Binding>,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
    returns: &mut Vec<Type>,
) -> Result<(), Diagnostic> {
    let mut scope = vars.clone();
    for stmt in body {
        match stmt {
            Stmt::Let {
                name,
                rebindability,
                ty,
                value,
            } => {
                let inferred_ty = if let Some(expected_ty) = ty {
                    infer_expr(value, &scope, signatures, Some(expected_ty.clone()))?
                } else {
                    infer_expr(value, &scope, signatures, None)?
                };
                scope.insert(
                    name.clone(),
                    Binding {
                        ty: inferred_ty,
                        rebindability: *rebindability,
                    },
                );
            }
            Stmt::Assign { name, value, .. } => {
                let existing = scope
                    .get(name)
                    .ok_or_else(|| Diagnostic::new(format!("unknown local '{name}'")))?;
                let _ = infer_expr(value, &scope, signatures, Some(existing.ty.clone()))?;
            }
            Stmt::IndexAssign {
                base, index, value, ..
            } => {
                let base_ty = infer_expr(base, &scope, signatures, None)?;
                let element_ty = base_ty.element_type().ok_or_else(|| {
                    Diagnostic::new("array element assignment requires an array operand")
                })?;
                let _ = infer_expr(
                    index,
                    &scope,
                    signatures,
                    Some(Type::Numeric(NumericType::I32)),
                )?;
                let _ = infer_expr(value, &scope, signatures, Some(element_ty))?;
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                let condition_ty = infer_expr(condition, &scope, signatures, None)?;
                if condition_ty != Type::Bool {
                    return Err(Diagnostic::new("if condition must be bool"));
                }
                collect_return_types(then_body, &scope, signatures, returns)?;
                collect_return_types(else_body, &scope, signatures, returns)?;
            }
            Stmt::While { condition, body } => {
                let condition_ty = infer_expr(condition, &scope, signatures, None)?;
                if condition_ty != Type::Bool {
                    return Err(Diagnostic::new("while condition must be bool"));
                }
                collect_return_types(body, &scope, signatures, returns)?;
            }
            Stmt::Repeat { body, condition } => {
                collect_return_types(body, &scope, signatures, returns)?;
                let condition_ty = infer_expr(condition, &scope, signatures, None)?;
                if condition_ty != Type::Bool {
                    return Err(Diagnostic::new("repeat-until condition must be bool"));
                }
            }
            Stmt::Break | Stmt::Continue => {}
            Stmt::Return(expr) => {
                returns.push(infer_expr(expr, &scope, signatures, None)?);
            }
            Stmt::ReturnMulti(values) => {
                returns.push(Type::Multi(infer_expr_list(
                    values, &scope, signatures, None,
                )?));
            }
            Stmt::LetMulti { bindings, values } => {
                let all_typed = bindings.iter().all(|binding| binding.ty.is_some());
                let any_typed = bindings.iter().any(|binding| binding.ty.is_some());
                if any_typed && !all_typed {
                    return Err(Diagnostic::new(
                        "multi-binding declaration must either annotate all bindings or none",
                    ));
                }
                let actual = if all_typed {
                    let expected: Vec<Type> = bindings
                        .iter()
                        .map(|binding| binding.ty.clone().expect("checked above"))
                        .collect();
                    let actual = infer_expr_list(values, &scope, signatures, Some(&expected))?;
                    if actual.len() != expected.len() {
                        return Err(Diagnostic::new(format!(
                            "multi-binding declaration expects {} values, got {}",
                            expected.len(),
                            actual.len()
                        )));
                    }
                    for (index, (binding, value_ty)) in
                        bindings.iter().zip(actual.iter()).enumerate()
                    {
                        let expected_ty = binding.ty.as_ref().expect("checked above");
                        if expected_ty != value_ty {
                            return Err(Diagnostic::new(format!(
                                "multi-binding declaration value {} expects {}, got {}",
                                index + 1,
                                expected_ty,
                                value_ty
                            )));
                        }
                    }
                    actual
                } else {
                    let actual = infer_expr_list(values, &scope, signatures, None)?;
                    if actual.len() != bindings.len() {
                        return Err(Diagnostic::new(format!(
                            "multi-binding declaration expects {} values, got {}",
                            bindings.len(),
                            actual.len()
                        )));
                    }
                    actual
                };
                for (binding, value_ty) in bindings.iter().zip(actual) {
                    let ty = binding.ty.clone().unwrap_or(value_ty);
                    scope.insert(
                        binding.name.clone(),
                        Binding {
                            ty,
                            rebindability: binding.rebindability,
                        },
                    );
                }
            }
            Stmt::AssignMulti { targets, values } => {
                let mut expected = Vec::new();
                for target in targets {
                    let binding = scope
                        .get(target)
                        .ok_or_else(|| Diagnostic::new(format!("unknown local '{target}'")))?;
                    expected.push(binding.ty.clone());
                }
                let actual = infer_expr_list(values, &scope, signatures, Some(&expected))?;
                if actual.len() != expected.len() {
                    return Err(Diagnostic::new(format!(
                        "multi-assignment expects {} values, got {}",
                        expected.len(),
                        actual.len()
                    )));
                }
            }
            Stmt::Expr(expr) => {
                let _ = infer_expr(expr, &scope, signatures, None)?;
            }
        }
    }
    Ok(())
}

fn common_return_type(left: Type, right: Type) -> Result<Type, Diagnostic> {
    if left == right {
        return Ok(left);
    }
    match (left, right) {
        (Type::Numeric(a), Type::Numeric(b)) => a
            .common(b)
            .map(Type::Numeric)
            .ok_or_else(|| {
                inference_diagnostic(
                    "inference/conflict",
                    DiagnosticCategory::Conflict,
                    "function return branches must resolve to the same type",
                    "ensure all return branches produce the same type or add an explicit return annotation",
                )
            }),
        _ => Err(inference_diagnostic(
            "inference/conflict",
            DiagnosticCategory::Conflict,
            "function return branches must resolve to the same type",
            "ensure all return branches produce the same type or add an explicit return annotation",
        )),
    }
}

fn check_stmt(
    stmt: &Stmt,
    vars: &mut HashMap<String, Binding>,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
    expected_return: &Type,
    in_loop: bool,
) -> Result<bool, Diagnostic> {
    match stmt {
        Stmt::Let {
            name,
            rebindability,
            ty,
            value,
        } => {
            let inferred_ty = if let Some(expected_ty) = ty {
                let value_ty = infer_expr(value, vars, signatures, Some(expected_ty.clone()))?;
                if &value_ty != expected_ty {
                    return Err(Diagnostic::new(format!(
                        "let '{}' expects {}, got {}",
                        name, expected_ty, value_ty
                    )));
                }
                expected_ty.clone()
            } else {
                infer_expr(value, vars, signatures, None)?
            };
            vars.insert(
                name.clone(),
                Binding {
                    ty: inferred_ty,
                    rebindability: *rebindability,
                },
            );
            Ok(false)
        }
        Stmt::Assign { op, name, value } => {
            let existing = vars
                .get(name)
                .ok_or_else(|| Diagnostic::new(format!("unknown local '{name}'")))?;
            if *op == AssignOp::Add && !existing.ty.is_numeric() {
                return Err(Diagnostic::new(format!(
                    "compound assignment to '{}' requires a numeric target",
                    name
                )));
            }
            if existing.rebindability == Rebindability::Const {
                return Err(Diagnostic::new(format!(
                    "cannot rebind const local '{}'",
                    name
                )));
            }
            let value_ty = infer_expr(value, vars, signatures, Some(existing.ty.clone()))?;
            if existing.ty != value_ty {
                return Err(Diagnostic::new(format!(
                    "assignment to '{}' expects {}, got {}",
                    name, existing.ty, value_ty
                )));
            }
            Ok(false)
        }
        Stmt::IndexAssign {
            op,
            base,
            index,
            value,
        } => {
            let base_ty = infer_expr(base, vars, signatures, None)?;
            let element_ty = base_ty.element_type().ok_or_else(|| {
                Diagnostic::new("array element assignment requires an array operand")
            })?;
            if *op == AssignOp::Add && !element_ty.is_numeric() {
                return Err(Diagnostic::new(
                    "compound array assignment requires numeric elements",
                ));
            }
            let index_ty = infer_expr(
                index,
                vars,
                signatures,
                Some(Type::Numeric(NumericType::I32)),
            )?;
            if index_ty != Type::Numeric(NumericType::I32) {
                return Err(Diagnostic::new("array index must be i32"));
            }
            let value_ty = infer_expr(value, vars, signatures, Some(element_ty.clone()))?;
            if value_ty != element_ty {
                return Err(Diagnostic::new(format!(
                    "array element assignment expects {}, got {}",
                    element_ty, value_ty
                )));
            }
            Ok(false)
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            let condition_ty = infer_expr(condition, vars, signatures, None)?;
            if condition_ty != Type::Bool {
                return Err(Diagnostic::new("if condition must be bool"));
            }
            let mut then_scope = vars.clone();
            let mut else_scope = vars.clone();
            let mut then_returns = false;
            let mut else_returns = false;
            for stmt in then_body {
                then_returns |=
                    check_stmt(stmt, &mut then_scope, signatures, expected_return, in_loop)?;
            }
            for stmt in else_body {
                else_returns |=
                    check_stmt(stmt, &mut else_scope, signatures, expected_return, in_loop)?;
            }
            Ok(then_returns && else_returns)
        }
        Stmt::While { condition, body } => {
            let condition_ty = infer_expr(condition, vars, signatures, None)?;
            if condition_ty != Type::Bool {
                return Err(Diagnostic::new("while condition must be bool"));
            }
            let mut loop_scope = vars.clone();
            for stmt in body {
                let _ = check_stmt(stmt, &mut loop_scope, signatures, expected_return, true)?;
            }
            Ok(false)
        }
        Stmt::Repeat { body, condition } => {
            let mut loop_scope = vars.clone();
            for stmt in body {
                let _ = check_stmt(stmt, &mut loop_scope, signatures, expected_return, true)?;
            }
            let condition_ty = infer_expr(condition, &loop_scope, signatures, None)?;
            if condition_ty != Type::Bool {
                return Err(Diagnostic::new("repeat-until condition must be bool"));
            }
            Ok(false)
        }
        Stmt::Break => {
            if !in_loop {
                return Err(Diagnostic::new("break is only allowed inside loops"));
            }
            Ok(false)
        }
        Stmt::Continue => {
            if !in_loop {
                return Err(Diagnostic::new("continue is only allowed inside loops"));
            }
            Ok(false)
        }
        Stmt::Return(expr) => {
            let ty = infer_expr(expr, vars, signatures, Some(expected_return.clone()))?;
            if &ty != expected_return {
                return Err(Diagnostic::new(format!(
                    "return expects {}, got {}",
                    expected_return, ty
                )));
            }
            Ok(true)
        }
        Stmt::ReturnMulti(values) => {
            let expected = match expected_return {
                Type::Multi(types) => types.clone(),
                _ => vec![expected_return.clone()],
            };
            let actual = infer_expr_list(values, vars, signatures, Some(&expected))?;
            if actual.len() != expected.len() {
                return Err(Diagnostic::new(format!(
                    "return expects {} values, got {}",
                    expected.len(),
                    actual.len()
                )));
            }
            for (index, (expected_ty, actual_ty)) in expected.iter().zip(actual.iter()).enumerate()
            {
                if expected_ty != actual_ty {
                    return Err(Diagnostic::new(format!(
                        "return value {} expects {}, got {}",
                        index + 1,
                        expected_ty,
                        actual_ty
                    )));
                }
            }
            Ok(true)
        }
        Stmt::LetMulti { bindings, values } => {
            let all_typed = bindings.iter().all(|binding| binding.ty.is_some());
            let any_typed = bindings.iter().any(|binding| binding.ty.is_some());
            if any_typed && !all_typed {
                return Err(Diagnostic::new(
                    "multi-binding declaration must either annotate all bindings or none",
                ));
            }
            let actual = if all_typed {
                let expected: Vec<Type> = bindings
                    .iter()
                    .map(|binding| binding.ty.clone().expect("checked above"))
                    .collect();
                let actual = infer_expr_list(values, vars, signatures, Some(&expected))?;
                if actual.len() != expected.len() {
                    return Err(Diagnostic::new(format!(
                        "multi-binding declaration expects {} values, got {}",
                        expected.len(),
                        actual.len()
                    )));
                }
                for (index, (binding, value_ty)) in bindings.iter().zip(actual.iter()).enumerate() {
                    let expected_ty = binding.ty.as_ref().expect("checked above");
                    if expected_ty != value_ty {
                        return Err(Diagnostic::new(format!(
                            "multi-binding declaration value {} expects {}, got {}",
                            index + 1,
                            expected_ty,
                            value_ty
                        )));
                    }
                }
                actual
            } else {
                let actual = infer_expr_list(values, vars, signatures, None)?;
                if actual.len() != bindings.len() {
                    return Err(Diagnostic::new(format!(
                        "multi-binding declaration expects {} values, got {}",
                        bindings.len(),
                        actual.len()
                    )));
                }
                actual
            };
            for (binding, value_ty) in bindings.iter().zip(actual) {
                let ty = binding.ty.clone().unwrap_or(value_ty);
                vars.insert(
                    binding.name.clone(),
                    Binding {
                        ty,
                        rebindability: binding.rebindability,
                    },
                );
            }
            Ok(false)
        }
        Stmt::AssignMulti { targets, values } => {
            let mut expected = Vec::new();
            for target in targets {
                let binding = vars
                    .get(target)
                    .ok_or_else(|| Diagnostic::new(format!("unknown local '{target}'")))?;
                if binding.rebindability == Rebindability::Const {
                    return Err(Diagnostic::new(format!(
                        "cannot rebind const local '{}'",
                        target
                    )));
                }
                expected.push(binding.ty.clone());
            }
            let actual = infer_expr_list(values, vars, signatures, Some(&expected))?;
            if actual.len() != expected.len() {
                return Err(Diagnostic::new(format!(
                    "multi-assignment expects {} values, got {}",
                    expected.len(),
                    actual.len()
                )));
            }
            for (index, (expected_ty, actual_ty)) in expected.iter().zip(actual.iter()).enumerate()
            {
                if expected_ty != actual_ty {
                    return Err(Diagnostic::new(format!(
                        "multi-assignment value {} expects {}, got {}",
                        index + 1,
                        expected_ty,
                        actual_ty
                    )));
                }
            }
            Ok(false)
        }
        Stmt::Expr(expr) => {
            if !matches!(expr, Expr::Call { .. }) {
                return Err(Diagnostic::new("expression statements must be calls"));
            }
            if let Expr::Call { callee, args } = expr {
                if let Expr::Name(name) = callee.as_ref() {
                    if name == ASSERT {
                        if args.len() != 1 {
                            return Err(Diagnostic::new(format!(
                                "{ASSERT} expects 1 argument, got {}",
                                args.len()
                            )));
                        }
                        let actual = infer_expr(&args[0], vars, signatures, Some(Type::Bool))?;
                        if actual != Type::Bool {
                            return Err(Diagnostic::new(format!(
                                "{ASSERT} expects bool, got {actual}"
                            )));
                        }
                        return Ok(false);
                    }
                }
            }
            let _ = infer_expr(expr, vars, signatures, None)?;
            Ok(false)
        }
    }
}

fn infer_expr(
    expr: &Expr,
    vars: &HashMap<String, Binding>,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
    expected: Option<Type>,
) -> Result<Type, Diagnostic> {
    match expr {
        Expr::Number(value) => resolve_number_literal(value, expected),
        Expr::Bool(_) => Ok(Type::Bool),
        Expr::Require(path) => Err(Diagnostic::new(format!(
            "require(\"{path}\") can only be resolved when compiling from a file; \
             relative imports are unavailable when compiling a single source string"
        ))),
        Expr::Name(name) => {
            let actual = if let Some(local) = vars.get(name) {
                local.ty.clone()
            } else if let Some((params, return_type)) = signatures.get(name) {
                Type::Function {
                    params: params.clone(),
                    return_type: Box::new(return_type.clone()),
                }
            } else {
                return Err(Diagnostic::new(format!("unknown name '{name}'")));
            };
            coerce_type(actual, expected)
        }
        Expr::Unary { op, expr } => match op {
            UnaryOp::Neg => {
                let actual = infer_expr(expr, vars, signatures, expected.clone())?;
                match actual {
                    Type::Numeric(_) => coerce_type(actual, expected),
                    Type::Bool => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                    Type::Array(_) => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                    Type::Multi(_) => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                    Type::Function { .. } => {
                        Err(Diagnostic::new("unary '-' requires a numeric operand"))
                    }
                }
            }
            UnaryOp::Not => {
                let actual = infer_expr(expr, vars, signatures, Some(Type::Bool))?;
                if actual != Type::Bool {
                    return Err(Diagnostic::new("unary 'not' requires a bool operand"));
                }
                coerce_type(Type::Bool, expected)
            }
            UnaryOp::Len => {
                let actual = infer_expr(expr, vars, signatures, None)?;
                if !actual.is_array() {
                    return Err(Diagnostic::new("# requires an array operand"));
                }
                coerce_type(Type::Numeric(NumericType::I32), expected)
            }
        },
        Expr::Cast { expr, ty } => {
            let actual = infer_expr(expr, vars, signatures, None)?;
            require_numeric_cast(actual, ty.clone())?;
            coerce_type(ty.clone(), expected)
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
        } => {
            let condition_ty = infer_expr(condition, vars, signatures, Some(Type::Bool))?;
            if condition_ty != Type::Bool {
                return Err(Diagnostic::new("if expression condition must be bool"));
            }
            let then_ty = infer_expr(then_expr, vars, signatures, expected.clone())?;
            let else_ty = infer_expr(else_expr, vars, signatures, expected.clone())?;
            if then_ty == else_ty {
                Ok(then_ty)
            } else {
                Err(Diagnostic::new(
                    "if expression branches must resolve to the same type",
                ))
            }
        }
        Expr::Call { callee, args } => {
            if let Expr::Name(name) = callee.as_ref() {
                if let Some(result) =
                    infer_math_builtin_call(name, args, vars, signatures, expected.clone())
                {
                    return result;
                }
            }
            if let Expr::Name(name) = callee.as_ref() {
                if let Some(result) =
                    infer_coroutine_builtin_call(name, args, vars, signatures, expected.clone())
                {
                    return result;
                }
            }
            let callee_ty = infer_expr(callee, vars, signatures, None)?;
            let (params, ret) = match callee_ty {
                Type::Function {
                    params,
                    return_type,
                } => (params, *return_type),
                other => {
                    return Err(Diagnostic::new(format!(
                        "attempt to call non-function value of type {other}",
                    )));
                }
            };
            let actual_args = infer_expr_list(args, vars, signatures, Some(&params))?;
            if params.len() != actual_args.len() {
                return Err(Diagnostic::new(format!(
                    "function expects {} arguments, got {}",
                    params.len(),
                    actual_args.len()
                )));
            }
            for (expected, actual) in params.iter().zip(actual_args.iter()) {
                if expected != actual {
                    return Err(Diagnostic::new(format!(
                        "call expected {}, got {}",
                        expected, actual
                    )));
                }
            }
            coerce_type(ret, expected)
        }
        Expr::Function(function) => infer_function_expr(function, vars, signatures, expected),
        Expr::ArrayLiteral { elements } => {
            infer_array_literal(elements, vars, signatures, expected)
        }
        Expr::Index { base, index } => {
            let base_ty = infer_expr(base, vars, signatures, None)?;
            let element_ty = base_ty
                .element_type()
                .ok_or_else(|| Diagnostic::new("indexing requires an array operand"))?;
            let index_ty = infer_expr(
                index,
                vars,
                signatures,
                Some(Type::Numeric(NumericType::I32)),
            )?;
            if index_ty != Type::Numeric(NumericType::I32) {
                return Err(Diagnostic::new("array index must be i32"));
            }
            coerce_type(element_ty, expected)
        }
        Expr::Binary { op, left, right } => match op {
            BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::FloorDiv
            | BinaryOp::Mod => {
                let operand_ty =
                    infer_numeric_common_type(left, right, vars, signatures, expected.clone())?;
                coerce_type(operand_ty, expected)
            }
            BinaryOp::Less | BinaryOp::Greater => {
                let _ = infer_numeric_common_type(left, right, vars, signatures, None)?;
                Ok(Type::Bool)
            }
            BinaryOp::And | BinaryOp::Or => {
                let left_ty = infer_expr(left, vars, signatures, Some(Type::Bool))?;
                let right_ty = infer_expr(right, vars, signatures, Some(Type::Bool))?;
                require_bool_pair(left_ty, right_ty)?;
                Ok(Type::Bool)
            }
            BinaryOp::Eq => {
                let left_ty = infer_expr(left, vars, signatures, None)?;
                if left_ty == Type::Bool {
                    let right_ty = infer_expr(right, vars, signatures, Some(Type::Bool))?;
                    if right_ty != Type::Bool {
                        return Err(Diagnostic::new("== requires both sides to have same type"));
                    }
                } else if left_ty.is_numeric() {
                    let _ = infer_numeric_common_type(left, right, vars, signatures, None)?;
                } else {
                    let right_ty = infer_expr(right, vars, signatures, Some(left_ty.clone()))?;
                    if left_ty != right_ty {
                        return Err(Diagnostic::new("== requires both sides to have same type"));
                    }
                }
                Ok(Type::Bool)
            }
        },
    }
}

fn infer_coroutine_builtin_call(
    name: &str,
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
    expected: Option<Type>,
) -> Option<Result<Type, Diagnostic>> {
    match name {
        COROUTINE_CREATE => {
            if args.len() != 1 {
                return Some(Err(Diagnostic::new(format!(
                    "{COROUTINE_CREATE} expects 1 argument, got {}",
                    args.len()
                ))));
            }
            let coroutine_ty = match infer_expr(&args[0], vars, signatures, None) {
                Ok(ty) => ty,
                Err(error) => return Some(Err(error)),
            };
            match &coroutine_ty {
                Type::Function { params, .. } if params.is_empty() => {
                    Some(coerce_type(coroutine_ty, expected))
                }
                _ => Some(Err(Diagnostic::new(
                    "coroutine_create expects a zero-argument function",
                ))),
            }
        }
        COROUTINE_RESUME => {
            if args.len() != 1 {
                return Some(Err(Diagnostic::new(format!(
                    "{COROUTINE_RESUME} expects 1 argument, got {}",
                    args.len()
                ))));
            }
            let coroutine_ty = match infer_expr(&args[0], vars, signatures, None) {
                Ok(ty) => ty,
                Err(error) => return Some(Err(error)),
            };
            match coroutine_ty {
                Type::Function {
                    params,
                    return_type,
                } if params.is_empty() => Some(coerce_type(*return_type, expected)),
                _ => Some(Err(Diagnostic::new(
                    "coroutine_resume expects a coroutine created from a zero-argument function",
                ))),
            }
        }
        COROUTINE_STATUS => {
            if args.len() != 1 {
                return Some(Err(Diagnostic::new(format!(
                    "{COROUTINE_STATUS} expects 1 argument, got {}",
                    args.len()
                ))));
            }
            let coroutine_ty = match infer_expr(&args[0], vars, signatures, None) {
                Ok(ty) => ty,
                Err(error) => return Some(Err(error)),
            };
            match coroutine_ty {
                Type::Function { params, .. } if params.is_empty() => {
                    Some(coerce_type(Type::Bool, expected))
                }
                _ => Some(Err(Diagnostic::new(
                    "coroutine_status expects a coroutine created from a zero-argument function",
                ))),
            }
        }
        _ => None,
    }
}

fn infer_math_builtin_call(
    name: &str,
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
    expected: Option<Type>,
) -> Option<Result<Type, Diagnostic>> {
    let arity = match name {
        MATH_ABS | MATH_SQRT | MATH_FLOOR | MATH_CEIL | MATH_TRUNC | MATH_NEAREST => 1,
        MATH_MIN | MATH_MAX | MATH_COPYSIGN => 2,
        _ => return None,
    };
    if args.len() != arity {
        return Some(Err(Diagnostic::new(format!(
            "{name} expects {arity} argument{}, got {}",
            if arity == 1 { "" } else { "s" },
            args.len()
        ))));
    }
    let first = match infer_expr(&args[0], vars, signatures, None) {
        Ok(ty) => ty,
        Err(error) => return Some(Err(error)),
    };
    let Type::Numeric(first_numeric) = first else {
        return Some(Err(Diagnostic::new(format!(
            "{name} expects numeric arguments"
        ))));
    };
    if arity == 2 {
        let second = match infer_expr(
            &args[1],
            vars,
            signatures,
            Some(Type::Numeric(first_numeric)),
        ) {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        if second != Type::Numeric(first_numeric) {
            return Some(Err(Diagnostic::new(format!(
                "{name} requires both arguments to have the same numeric type"
            ))));
        }
    }
    let supports = match name {
        MATH_MIN | MATH_MAX => matches!(first_numeric, NumericType::F32 | NumericType::F64),
        MATH_ABS | MATH_SQRT | MATH_FLOOR | MATH_CEIL | MATH_TRUNC | MATH_NEAREST
        | MATH_COPYSIGN => matches!(first_numeric, NumericType::F32 | NumericType::F64),
        _ => false,
    };
    if !supports {
        return Some(Err(Diagnostic::new(format!(
            "{name} does not support {}",
            Type::Numeric(first_numeric)
        ))));
    }
    Some(coerce_type(Type::Numeric(first_numeric), expected))
}

fn infer_expr_list(
    exprs: &[Expr],
    vars: &HashMap<String, Binding>,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
    expected: Option<&[Type]>,
) -> Result<Vec<Type>, Diagnostic> {
    let mut out = Vec::new();
    for expr in exprs {
        let next_expected = expected.and_then(|types| types.get(out.len()).cloned());
        let ty = if matches!(expr, Expr::Call { .. }) {
            infer_expr(expr, vars, signatures, None)?
        } else {
            infer_expr(expr, vars, signatures, next_expected)?
        };
        match ty {
            Type::Multi(types) => out.extend(types),
            other => out.push(other),
        }
    }
    if let Some(expected_types) = expected {
        for (index, ty) in out.clone().into_iter().enumerate() {
            if let Some(expected_ty) = expected_types.get(index) {
                out[index] = coerce_type(ty, Some(expected_ty.clone()))?;
            }
        }
    }
    Ok(out)
}

fn infer_array_literal(
    elements: &[Expr],
    vars: &HashMap<String, Binding>,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
    expected: Option<Type>,
) -> Result<Type, Diagnostic> {
    if elements.is_empty() {
        return Err(inference_diagnostic(
            "inference/missing-context",
            DiagnosticCategory::MissingContext,
            "empty array literal requires explicit element type",
            "add an explicit element type annotation, e.g. local xs: {i32} = {}",
        ));
    }

    let expected_element = expected.as_ref().and_then(Type::element_type);
    let mut iter = elements.iter();
    let first = iter.next().expect("non-empty array literal");
    let mut element_ty = infer_expr(first, vars, signatures, expected_element.clone())?;
    for element in iter {
        let actual = infer_expr(element, vars, signatures, Some(element_ty.clone()))?;
        element_ty = common_element_type(element_ty, actual)?;
    }

    let array_ty = Type::Array(Box::new(element_ty));
    coerce_type(array_ty, expected)
}

fn common_element_type(left: Type, right: Type) -> Result<Type, Diagnostic> {
    match (left, right) {
        (Type::Numeric(left), Type::Numeric(right)) => {
            left.common(right).map(Type::Numeric).ok_or_else(|| {
                inference_diagnostic(
                    "inference/conflict",
                    DiagnosticCategory::Conflict,
                    "array literal elements must share a common type",
                    "cast elements to a common numeric type or annotate the array type",
                )
            })
        }
        (left, right) if left == right => Ok(left),
        _ => Err(inference_diagnostic(
            "inference/conflict",
            DiagnosticCategory::Conflict,
            "array literal elements must share a common type",
            "cast elements to a common type or split values into separate arrays",
        )),
    }
}

fn infer_numeric_common_type(
    left: &Expr,
    right: &Expr,
    vars: &HashMap<String, Binding>,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
    expected: Option<Type>,
) -> Result<Type, Diagnostic> {
    let expected_numeric = match expected {
        Some(Type::Numeric(numeric)) => Some(numeric),
        _ => None,
    };

    match (
        matches!(left, Expr::Number(_)),
        matches!(right, Expr::Number(_)),
    ) {
        (true, true) => {
            let ty = expected_numeric.unwrap_or(NumericType::F64);
            let left_ty = infer_expr(left, vars, signatures, Some(Type::Numeric(ty)))?;
            let right_ty = infer_expr(right, vars, signatures, Some(Type::Numeric(ty)))?;
            require_same_numeric(left_ty.clone(), right_ty)?;
            Ok(left_ty)
        }
        (true, false) => {
            let right_ty = infer_expr(right, vars, signatures, None)?;
            let left_ty = infer_expr(left, vars, signatures, Some(right_ty.clone()))?;
            common_numeric_type(left_ty, right_ty)
        }
        (false, true) => {
            let left_ty = infer_expr(left, vars, signatures, None)?;
            let right_ty = infer_expr(right, vars, signatures, Some(left_ty.clone()))?;
            common_numeric_type(left_ty, right_ty)
        }
        _ => {
            let left_ty = infer_expr(left, vars, signatures, None)?;
            let right_ty = infer_expr(right, vars, signatures, None)?;
            common_numeric_type(left_ty, right_ty)
        }
    }
}

fn common_numeric_type(left: Type, right: Type) -> Result<Type, Diagnostic> {
    match (left, right) {
        (Type::Numeric(left), Type::Numeric(right)) => {
            left.common(right).map(Type::Numeric).ok_or_else(|| {
                inference_diagnostic(
                    "inference/ambiguous",
                    DiagnosticCategory::Ambiguous,
                    "operation requires compatible numeric operands",
                    "add an explicit cast to pick the intended numeric type",
                )
            })
        }
        _ => Err(inference_diagnostic(
            "inference/conflict",
            DiagnosticCategory::Conflict,
            "operation requires compatible numeric operands",
            "change one operand type or cast explicitly",
        )),
    }
}

fn require_same_numeric(left: Type, right: Type) -> Result<(), Diagnostic> {
    if left.is_numeric() && right.is_numeric() && left == right {
        Ok(())
    } else {
        Err(inference_diagnostic(
            "inference/conflict",
            DiagnosticCategory::Conflict,
            "operation requires matching numeric operands",
            "cast one operand so both sides use the same numeric type",
        ))
    }
}

fn coerce_type(actual: Type, expected: Option<Type>) -> Result<Type, Diagnostic> {
    match expected {
        None => Ok(actual),
        Some(expected) if actual == expected => Ok(expected),
        Some(Type::Numeric(expected_numeric)) => match actual {
            Type::Numeric(actual_numeric)
                if actual_numeric.can_implicitly_widen_to(expected_numeric) =>
            {
                Ok(Type::Numeric(expected_numeric))
            }
            Type::Numeric(actual_numeric) => Err(Diagnostic::new(format!(
                "cannot implicitly convert {actual_numeric} to {expected_numeric}",
            ))),
            Type::Bool => Err(Diagnostic::new(format!(
                "cannot implicitly convert bool to {expected_numeric}",
            ))),
            Type::Array(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert array to {expected_numeric}",
            ))),
            Type::Multi(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert multiple values to {expected_numeric}",
            ))),
            Type::Function { .. } => Err(Diagnostic::new(format!(
                "cannot implicitly convert function to {expected_numeric}",
            ))),
        },
        Some(Type::Bool) => Err(Diagnostic::new(format!(
            "cannot implicitly convert {actual} to bool",
        ))),
        Some(expected) => Err(Diagnostic::new(format!(
            "cannot implicitly convert {actual} to {expected}",
        ))),
    }
}

fn require_numeric_cast(actual: Type, target: Type) -> Result<(), Diagnostic> {
    match (actual, target) {
        (Type::Numeric(_), Type::Numeric(_)) => Ok(()),
        _ => Err(Diagnostic::new(
            "casts require numeric source and destination types",
        )),
    }
}

fn require_bool_pair(left: Type, right: Type) -> Result<(), Diagnostic> {
    if left == Type::Bool && right == Type::Bool {
        Ok(())
    } else {
        Err(Diagnostic::new("operation requires bool operands"))
    }
}

fn resolve_number_literal(
    value: &NumberLiteral,
    expected: Option<Type>,
) -> Result<Type, Diagnostic> {
    match expected {
        Some(Type::Numeric(numeric)) => {
            validate_numeric_literal(value, numeric)?;
            Ok(Type::Numeric(numeric))
        }
        Some(Type::Bool) => Err(Diagnostic::new("numeric literal is not assignable to bool")),
        Some(Type::Array(_)) => Err(Diagnostic::new(
            "numeric literal is not assignable to array",
        )),
        Some(Type::Function { .. }) => Err(Diagnostic::new(
            "numeric literal is not assignable to function",
        )),
        Some(Type::Multi(_)) => Err(Diagnostic::new(
            "numeric literal is not assignable to multiple values",
        )),
        None => Ok(Type::number()),
    }
}

fn infer_function_expr(
    function: &FunctionExpr,
    vars: &HashMap<String, Binding>,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
    expected: Option<Type>,
) -> Result<Type, Diagnostic> {
    let return_ty = function.return_type.clone().ok_or_else(|| {
        inference_diagnostic(
            "inference/unsupported",
            DiagnosticCategory::Unsupported,
            "function return inference is only supported for named functions in this MVP",
            "add an explicit return type annotation to the function expression",
        )
    })?;
    let function_ty = Type::Function {
        params: function
            .params
            .iter()
            .map(|param| param.ty.clone())
            .collect(),
        return_type: Box::new(return_ty.clone()),
    };
    let mut local_scope = vars.clone();
    for param in &function.params {
        local_scope.insert(
            param.name.clone(),
            Binding {
                ty: param.ty.clone(),
                rebindability: Rebindability::Rebindable,
            },
        );
    }
    if let Some(name) = &function.name {
        local_scope.insert(
            name.clone(),
            Binding {
                ty: function_ty.clone(),
                rebindability: Rebindability::Rebindable,
            },
        );
    }
    let mut saw_return = false;
    for stmt in &function.body {
        if check_stmt(stmt, &mut local_scope, signatures, &return_ty, false)? {
            saw_return = true;
        }
    }
    if !saw_return {
        return Err(Diagnostic::new("function expression is missing a return"));
    }
    coerce_type(function_ty, expected)
}

fn validate_numeric_literal(
    value: &NumberLiteral,
    expected: NumericType,
) -> Result<(), Diagnostic> {
    match expected {
        NumericType::F32 => {
            let value = parse_float_literal(value)?;
            if (value as f32).is_finite() || value == f64::INFINITY || value == f64::NEG_INFINITY {
                Ok(())
            } else {
                Err(Diagnostic::new("numeric literal is out of range for f32"))
            }
        }
        NumericType::F64 => parse_float_literal(value).map(|_| ()),
        NumericType::I32 => parse_integer_literal::<i32>(value, "i32").map(|_| ()),
        NumericType::I64 => parse_integer_literal::<i64>(value, "i64").map(|_| ()),
        NumericType::U32 => parse_integer_literal::<u32>(value, "u32").map(|_| ()),
        NumericType::U64 => parse_integer_literal::<u64>(value, "u64").map(|_| ()),
    }
}

fn parse_float_literal(value: &NumberLiteral) -> Result<f64, Diagnostic> {
    value
        .raw
        .parse::<f64>()
        .map_err(|_| Diagnostic::new("invalid number literal"))
}

fn parse_integer_literal<T>(value: &NumberLiteral, ty_name: &str) -> Result<T, Diagnostic>
where
    T: std::str::FromStr,
{
    if value.raw.contains('.') {
        return Err(Diagnostic::new(format!(
            "numeric literal must be an integer for {ty_name}",
        )));
    }

    value
        .raw
        .parse::<T>()
        .map_err(|_| Diagnostic::new(format!("numeric literal is out of range for {ty_name}",)))
}

#[cfg(test)]
mod tests {
    use waluau_ast::{NumericType, Type};
    use waluau_diagnostics::DiagnosticCategory;
    use waluau_parser::parse;

    #[test]
    fn type_checks_valid_program() {
        let source = r#"
            function add(x: i32, y: i32): i32
                return x + y
            end

            function entry(flag: bool, x: i32, y: i32): i32
                local z: i32 = add(x, y)
                if flag then
                    z = z + 1
                else
                    z = z + 2
                end
                return z
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn rejects_non_bool_condition() {
        let source = r#"
            function entry(x: i32): i32
                if x then
                    return x
                end
                return x
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(error.to_string(), "if condition must be bool");
    }

    #[test]
    fn accepts_numeric_alias_and_scalar_types() {
        let source = r#"
            function widen(x: number, y: f32, z: u64, w: i64): f64
                local sum: f64 = x + 1
                if z > 0 then
                    return sum
                end
                if w > 0 then
                    return x + 2
                end
                return x + 3
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn rejects_mixed_numeric_operands() {
        let source = r#"
            function entry(x: i64, y: f64): i64
                return x + y
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(
            error.to_string(),
            "operation requires compatible numeric operands"
        );
    }

    #[test]
    fn infers_local_types_from_literals() {
        let source = r#"
            function entry(flag: bool): f64
                local x = 41
                local y = x + 1
                if flag then
                    y = y + 1
                end
                return y
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn rejects_incompatible_reassignment_of_inferred_local() {
        let source = r#"
            function entry(): i32
                local x = 1
                x = true
                return x
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(error.to_string(), "assignment to 'x' expects f64, got bool");
    }

    #[test]
    fn accepts_full_range_i64_and_u64_literals() {
        let source = r#"
            function entry(x: i64, y: u64): i64
                local a: i64 = x + 1
                local b: u64 = 18446744073709551615
                if y > 0 then
                    return a
                end
                if b > 0 then
                    return 9223372036854775807
                end
                return x + 2
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn rejects_out_of_range_u64_literals() {
        let source = r#"
            function entry(): u64
                return 18446744073709551616
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(error.to_string(), "numeric literal is out of range for u64");
    }

    #[test]
    fn accepts_implicit_numeric_widening() {
        let source = r#"
            function widen(x: i32, y: f32, z: u32): f64
                local a: i64 = x
                local b: f64 = x + 1
                local c: f64 = y
                local d: i64 = z + 1
                if a < d then
                    return b
                end
                return c
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn requires_explicit_cast_for_narrowing() {
        let source = r#"
            function narrow(x: i64): i32
                return x
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(error.to_string(), "cannot implicitly convert i64 to i32");
    }

    #[test]
    fn accepts_explicit_numeric_casts() {
        let source = r#"
            function narrow(x: i64, y: f64): i32
                local a: i32 = x :: i32
                local b: i32 = y :: i32
                return a + b
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn accepts_unary_negation_not_and_elseif() {
        let source = r#"
            function entry(flag: bool, x: i32): i32
                if not flag then
                    return -x
                elseif x > 0 then
                    return x
                else
                    return 0
                end
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn rejects_non_call_expression_statements() {
        let source = r#"
            function entry(x: i32): i32
                x + 1
                return x
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(error.to_string(), "expression statements must be calls");
    }

    #[test]
    fn type_checks_array_literals_indexing_and_length() {
        let source = r#"
            function score_count(): i32
                local scores: {number} = {100, 250, 300}
                local first: number = scores[0]
                scores[1] = first + 1
                return #scores
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn rejects_heterogeneous_array_literals() {
        let source = r#"
            function entry(): i32
                local xs: {i32} = {1, true}
                return #xs
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(
            error.to_string(),
            "array literal elements must share a common type"
        );
    }

    #[test]
    fn rejects_empty_array_literals() {
        let source = r#"
            function entry(): i32
                local xs: {i32} = {}
                return #xs
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(
            error.to_string(),
            "empty array literal requires explicit element type"
        );
    }

    #[test]
    fn rejects_ambiguous_empty_array_local_inference() {
        let source = r#"
            function entry(): i32
                local xs = {}
                return #xs
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check_and_infer(&program).expect_err("inference should fail");
        assert_eq!(
            error.to_string(),
            "empty array literal requires explicit element type"
        );
    }

    #[test]
    fn rejects_length_on_non_array() {
        let source = r#"
            function entry(x: i32): i32
                return #x
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(error.to_string(), "# requires an array operand");
    }

    #[test]
    fn rejects_incompatible_array_assignment() {
        let source = r#"
            function entry(): i32
                local xs: {i32} = {1, 2, 3}
                local ys: {i64} = {1, 2, 3}
                xs = ys
                return #xs
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(
            error.to_string(),
            "cannot implicitly convert {i64} to {i32}"
        );
    }

    #[test]
    fn rejects_repeat_until_non_bool_condition() {
        let source = r#"
            function entry(x: i32): i32
                repeat
                    x = x + 1
                until x
                return x
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(error.to_string(), "repeat-until condition must be bool");
    }

    #[test]
    fn rejects_break_and_continue_outside_loops() {
        let source = r#"
            function entry(x: i32): i32
                break
                continue
                return x
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        let message = error.to_string();
        assert!(
            message.contains("break is only allowed inside loops")
                || message.contains("continue is only allowed inside loops")
        );
    }

    #[test]
    fn type_checks_break_and_continue_in_loops() {
        let source = r#"
            function entry(xs: {i32}, len: i32): i32
                local i: i32 = 0
                local acc: i32 = 0
                while i < len do
                    local x: i32 = xs[i]
                    if x < 0 then
                        i += 1
                        continue
                    end
                    acc += x
                    break
                end
                repeat
                    i += 1
                    if i > len then
                        break
                    end
                until false
                return acc
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn type_checks_repeat_until_loop() {
        let source = r#"
            function entry(limit: i32): i32
                local i: i32 = 0
                repeat
                    i = i + 1
                until i > limit
                return i
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn type_checks_closure_capture() {
        let source = r#"
            function entry(x: i32): i32
                local make: (i32) -> (i32) -> i32 = function(offset: i32): (i32) -> i32
                    return function(value: i32): i32
                        return x + offset + value
                    end
                end
                local add5: (i32) -> i32 = make(5)
                return add5(7)
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn type_checks_named_function_expression_recursion() {
        let source = r#"
            function entry(): i32
                local fact: (i32) -> i32 = function self(n: i32): i32
                    if n == 0 then
                        return 1
                    end
                    return n * self(n - 1)
                end
                return fact(5)
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn rejects_compound_assignment_on_non_numeric_targets() {
        let source = r#"
            function entry(flag: bool, xs: {bool}): i32
                flag += true
                xs[0] += false
                return 0
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(
            error.to_string(),
            "compound assignment to 'flag' requires a numeric target"
        );
    }

    #[test]
    fn rejects_rebinding_const_local() {
        let source = r#"
            function entry(x: i32): i32
                const y: i32 = x
                y = x + 1
                return y
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(error.to_string(), "cannot rebind const local 'y'");
    }

    #[test]
    fn allows_rebinding_plain_local_named_const() {
        let source = r#"
            function entry(): i32
                local const: i32 = 1
                const = const + 1
                return const
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn allows_shadowing_const_with_inner_local() {
        let source = r#"
            function entry(flag: bool): i32
                const x: i32 = 1
                if flag then
                    local x: i32 = 2
                    return x
                end
                return x
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn allows_mutation_through_const_array_binding() {
        let source = r#"
            function entry(): i32
                const xs: {i32} = {1, 2, 3}
                xs[0] = 9
                return xs[0]
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn type_checks_if_expression() {
        let source = r#"
            function entry(flag: bool, x: i32, y: i32): i32
                return if flag then x else y
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn rejects_if_expression_type_mismatch() {
        let source = r#"
            function entry(flag: bool, x: i32): i32
                return if flag then x else true
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(
            error.to_string(),
            "if expression branches must resolve to the same type"
        );
    }

    #[test]
    fn rejects_unannotated_function_expression_returns_for_now() {
        let source = r#"
            function entry(): i32
                local add1 = function(x: i32)
                    return x + 1
                end
                return add1(1)
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(
            error.to_string(),
            "function return inference is only supported for named functions in this MVP"
        );
    }

    #[test]
    fn rejects_typed_local_function_value_with_mismatched_annotation() {
        let source = r#"
            function entry(): i32
                local job: (i32) -> i32 = function(x: i32): bool
                    return x > 0
                end
                return job(1)
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(
            error.to_string(),
            "cannot implicitly convert (i32) -> bool to (i32) -> i32"
        );
    }
    #[test]
    fn infers_top_level_function_return_type_from_single_return() {
        let source = r#"
            function inc(x: i32)
                return x + 1
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let typed = super::type_check_and_infer(&program).expect("inference should succeed");
        assert_eq!(
            typed.functions[0].return_type,
            Some(Type::Numeric(NumericType::I32))
        );
    }

    #[test]
    fn infers_top_level_function_return_type_from_branches() {
        let source = r#"
            function choose(flag: bool)
                if flag then
                    return 1 :: i32
                else
                    return 2 :: i64
                end
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let typed = super::type_check_and_infer(&program).expect("inference should succeed");
        assert_eq!(
            typed.functions[0].return_type,
            Some(Type::Numeric(NumericType::I64))
        );
    }

    #[test]
    fn rejects_incompatible_inferred_return_branches() {
        let source = r#"
            function bad(flag: bool)
                if flag then
                    return 1
                end
                return true
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let error = super::type_check_and_infer(&program).expect_err("inference should fail");
        assert_eq!(
            error.to_string(),
            "function return branches must resolve to the same type"
        );
    }

    #[test]
    fn rejects_recursive_return_inference() {
        let source = r#"
            function fact(n: i32)
                if n == 0 then
                    return 1
                end
                return n * fact(n - 1)
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let error = super::type_check_and_infer(&program).expect_err("inference should fail");
        assert_eq!(
            error.to_string(),
            "cannot infer return type for recursive or cyclic function 'fact'"
        );
    }

    #[test]
    fn inference_is_deterministic_for_identical_ast_input() {
        let source = r#"
            function pick(flag: bool)
                local value = if flag then 1 else 2
                return value + 1
            end
        "#;
        let program = parse(source).expect("parse should succeed");

        let inferred_once =
            super::type_check_and_infer(&program).expect("first inference succeeds");
        let inferred_twice =
            super::type_check_and_infer(&program).expect("second inference succeeds");

        assert_eq!(inferred_once, inferred_twice);
    }

    #[test]
    fn type_checks_multi_return_and_multi_assignment() {
        let source = r#"
            function pair(x: i32, y: bool): i32, bool
                return x, y
            end

            function entry(x: i32, y: bool): i32
                local a: i32, b: bool = pair(x, y)
                a, b = pair(a + 1, b)
                return a
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn type_checks_multi_value_call_argument_expansion() {
        let source = r#"
            function pair(x: i32, y: i32): i32, i32
                return x, y
            end

            function sum2(a: i32, b: i32): i32
                return a + b
            end

            function entry(x: i32, y: i32): i32
                return sum2(pair(x, y))
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn rejects_multi_assignment_arity_mismatch() {
        let source = r#"
            function pair(x: i32): i32, i32
                return x, x + 1
            end

            function entry(x: i32): i32
                local a: i32, b: i32 = pair(x)
                a, b = x
                return a
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(
            error.to_string(),
            "multi-assignment expects 2 values, got 1"
        );
    }

    #[test]
    fn rejects_multi_return_type_mismatch() {
        let source = r#"
            function pair(x: i32): i32, bool
                return x, x + 1
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(error.to_string(), "cannot implicitly convert i32 to bool");
    }

    #[test]
    fn rejects_multi_binding_arity_mismatch() {
        let source = r#"
            function entry(x: i32): i32
                local a: i32, b: i32 = x
                return a
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(
            error.to_string(),
            "multi-binding declaration expects 2 values, got 1"
        );
    }

    #[test]
    fn rejects_multi_value_call_arity_mismatch() {
        let source = r#"
            function pair(x: i32): i32, i32
                return x, x + 1
            end

            function sum3(a: i32, b: i32, c: i32): i32
                return a + b + c
            end

            function entry(x: i32): i32
                return sum3(pair(x))
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(error.to_string(), "function expects 3 arguments, got 2");
    }

    #[test]
    fn rejects_multi_value_call_type_mismatch() {
        let source = r#"
            function pair(x: i32): i32, bool
                return x, x > 0
            end

            function sum2(a: i32, b: i32): i32
                return a + b
            end

            function entry(x: i32): i32
                return sum2(pair(x))
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(error.to_string(), "cannot implicitly convert bool to i32");
    }

    #[test]
    fn rejects_multi_value_in_scalar_context() {
        let source = r#"
            function pair(x: i32, y: i32): i32, i32
                return x, y
            end

            function entry(x: i32, y: i32): number
                local t: number = pair(x, y)
                return t
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(
            error.to_string(),
            "cannot implicitly convert multiple values to f64"
        );
    }

    #[test]
    fn type_checks_coroutine_create_and_resume_for_zero_arg_functions() {
        let source = r#"
            function run_job(): i32
                local job: () -> i32 = function(): i32
                    return 7
                end
                local co: () -> i32 = coroutine_create(job)
                return coroutine_resume(co)
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn rejects_coroutine_create_for_non_zero_arg_functions() {
        let source = r#"
            function run_job(): i32
                local job: (i32) -> i32 = function(x: i32): i32
                    return x
                end
                local co: (i32) -> i32 = coroutine_create(job)
                return 0
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(
            error.to_string(),
            "coroutine_create expects a zero-argument function"
        );
    }

    #[test]
    fn type_checks_coroutine_status_for_zero_arg_coroutines() {
        let source = r#"
            function run_job(): bool
                local job: () -> i32 = function(): i32
                    return 7
                end
                local co: () -> i32 = coroutine_create(job)
                return coroutine_status(co)
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn type_checks_assert_statement() {
        let source = r#"
            function check(x: i32): i32
                assert(x > 0)
                return x
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn rejects_assert_with_non_bool_argument() {
        let source = r#"
            function check(x: i32): i32
                assert(x)
                return x
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(error.to_string(), "cannot implicitly convert i32 to bool");
    }

    #[test]
    fn tags_empty_array_inference_failure_with_missing_context_diagnostic() {
        let source = r#"
            function entry(): i32
                local xs = {}
                return 0
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(error.code(), Some("inference/missing-context"));
        assert_eq!(error.category(), Some(DiagnosticCategory::MissingContext));
        assert_eq!(
            error.action(),
            Some("add an explicit element type annotation, e.g. local xs: {i32} = {}")
        );
    }

    #[test]
    fn tags_recursive_return_inference_failure_as_unsupported() {
        let source = r#"
            function fact(n: i32)
                if n == 0 then
                    return 1
                end
                return n * fact(n - 1)
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(error.code(), Some("inference/unsupported"));
        assert_eq!(error.category(), Some(DiagnosticCategory::Unsupported));
        assert_eq!(
            error.action(),
            Some("add an explicit return type annotation to break the cycle")
        );
    }
}
