use std::collections::{BTreeMap, HashMap, HashSet};

use waluau_ast::{AssignOp, Expr, Function, NumericType, Rebindability, Stmt, Type};
use waluau_diagnostics::{Diagnostic, DiagnosticCategory};

use super::builtins::ASSERT;
use super::expressions::{infer_expr, infer_expr_list};
use super::numeric::common_numeric_type;
use super::signatures::{
    FnSignature, active_type_param_set, generic_diagnostic, inference_diagnostic,
    validate_type_in_scope, validate_type_param_list,
};
use super::{Binding, binding_for};

pub(super) fn check_function(
    function: &Function,
    fn_signatures: &HashMap<String, FnSignature>,
    outer_type_params: &HashSet<String>,
) -> Result<(), Diagnostic> {
    validate_type_param_list(&function.type_params, outer_type_params)?;
    let active_type_params = active_type_param_set(&function.type_params);
    let mut allowed_type_params = outer_type_params.clone();
    allowed_type_params.extend(active_type_params.iter().cloned());
    for param in &function.params {
        validate_type_in_scope(&param.ty, &allowed_type_params)?;
    }
    if let Some(ret) = &function.return_type {
        validate_type_in_scope(ret, &allowed_type_params)?;
    }
    let expected_return = function.return_type.clone().ok_or_else(|| {
        if function.type_params.is_empty() {
            Diagnostic::new(format!(
                "cannot infer return type for recursive or cyclic function '{}'",
                function.name
            ))
        } else {
            generic_diagnostic(
                "generic/missing-return-type",
                format!(
                    "generic function '{}' requires an explicit return type",
                    function.name
                ),
                "add a return type annotation to the generic function",
            )
        }
    })?;
    let mut vars: HashMap<String, Binding> = HashMap::new();
    for param in &function.params {
        vars.insert(
            param.name.clone(),
            binding_for(param.ty.clone(), Rebindability::Rebindable),
        );
    }

    let mut saw_return = false;
    for stmt in &function.body {
        if check_stmt(
            stmt,
            &mut vars,
            fn_signatures,
            &active_type_params,
            &expected_return,
            false,
        )? {
            saw_return = true;
        }
    }
    if !saw_return && expected_return != Type::Unit {
        return Err(Diagnostic::new(format!(
            "function '{}' is missing a return",
            function.name
        )));
    }
    Ok(())
}

pub(super) fn collect_return_types(
    body: &[Stmt],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
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
                    infer_expr(
                        value,
                        &scope,
                        fn_signatures,
                        active_type_params,
                        Some(expected_ty.clone()),
                    )?
                } else if matches!(value, Expr::ArrayLiteral { elements, .. } if elements.is_empty())
                {
                    Type::Record(BTreeMap::new())
                } else {
                    infer_expr(value, &scope, fn_signatures, active_type_params, None)?
                };
                seal_record_locals_in_expr(value, &mut scope);
                scope.insert(name.clone(), binding_for(inferred_ty, *rebindability));
            }
            Stmt::Assign { name, value, .. } => {
                let existing = scope
                    .get(name)
                    .ok_or_else(|| Diagnostic::new(format!("unknown local '{name}'")))?;
                let _ = infer_expr(
                    value,
                    &scope,
                    fn_signatures,
                    active_type_params,
                    Some(existing.ty.clone()),
                )?;
                seal_record_locals_in_expr(value, &mut scope);
            }
            Stmt::IndexAssign {
                base, index, value, ..
            } => {
                let base_ty = infer_expr(base, &scope, fn_signatures, active_type_params, None)?;
                let element_ty = base_ty.element_type().ok_or_else(|| {
                    Diagnostic::new("array element assignment requires an array operand")
                })?;
                let _ = infer_expr(
                    index,
                    &scope,
                    fn_signatures,
                    active_type_params,
                    Some(Type::Numeric(NumericType::I32)),
                )?;
                let _ = infer_expr(
                    value,
                    &scope,
                    fn_signatures,
                    active_type_params,
                    Some(element_ty),
                )?;
            }
            Stmt::FieldAssign {
                base, name, value, ..
            } => {
                let base_name = match base.as_ref() {
                    Expr::Name(local, _) => local,
                    _ => {
                        return Err(Diagnostic::new(
                            "field assignment base must be a local name",
                        ));
                    }
                };
                let binding = scope
                    .get(base_name)
                    .cloned()
                    .ok_or_else(|| Diagnostic::new(format!("unknown local '{base_name}'")))?;
                let Type::Record(mut fields) = binding.ty else {
                    return Err(Diagnostic::new("field assignment requires a record base"));
                };
                let existing = fields.get(name).cloned();
                let value_ty = infer_expr(
                    value,
                    &scope,
                    fn_signatures,
                    active_type_params,
                    existing.clone(),
                )?;
                if let Some(ty) = fields.get(name) {
                    if *ty != value_ty {
                        return Err(Diagnostic::new(format!(
                            "field assignment to '{}.{}' expects {}, got {}",
                            base_name, name, ty, value_ty
                        )));
                    }
                } else if binding.record_open {
                    fields.insert(name.clone(), value_ty);
                } else {
                    return Err(Diagnostic::new(format!(
                        "cannot add new field '{}.{}' after record was sealed",
                        base_name, name
                    )));
                }
                seal_record_locals_in_expr(value, &mut scope);
                let mut updated = binding_for(Type::Record(fields), binding.rebindability);
                if !binding.record_open {
                    updated.record_open = false;
                }
                scope.insert(base_name.clone(), updated);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                let condition_ty =
                    infer_expr(condition, &scope, fn_signatures, active_type_params, None)?;
                seal_record_locals_in_expr(condition, &mut scope);
                if condition_ty != Type::Bool {
                    return Err(Diagnostic::new("if condition must be bool"));
                }
                collect_return_types(
                    then_body,
                    &scope,
                    fn_signatures,
                    active_type_params,
                    returns,
                )?;
                collect_return_types(
                    else_body,
                    &scope,
                    fn_signatures,
                    active_type_params,
                    returns,
                )?;
            }
            Stmt::While { condition, body } => {
                let condition_ty =
                    infer_expr(condition, &scope, fn_signatures, active_type_params, None)?;
                seal_record_locals_in_expr(condition, &mut scope);
                if condition_ty != Type::Bool {
                    return Err(Diagnostic::new("while condition must be bool"));
                }
                collect_return_types(body, &scope, fn_signatures, active_type_params, returns)?;
            }
            Stmt::Repeat { body, condition } => {
                collect_return_types(body, &scope, fn_signatures, active_type_params, returns)?;
                let condition_ty =
                    infer_expr(condition, &scope, fn_signatures, active_type_params, None)?;
                seal_record_locals_in_expr(condition, &mut scope);
                if condition_ty != Type::Bool {
                    return Err(Diagnostic::new("repeat-until condition must be bool"));
                }
            }
            Stmt::NumericFor {
                name,
                start,
                stop,
                step,
                body,
            } => {
                let start_ty = infer_expr(start, &scope, fn_signatures, active_type_params, None)?;
                let stop_ty = infer_expr(stop, &scope, fn_signatures, active_type_params, None)?;
                seal_record_locals_in_expr(start, &mut scope);
                seal_record_locals_in_expr(stop, &mut scope);
                let mut loop_ty = common_numeric_type(start_ty, stop_ty)?;
                if let Some(step_expr) = step {
                    let step_ty =
                        infer_expr(step_expr, &scope, fn_signatures, active_type_params, None)?;
                    seal_record_locals_in_expr(step_expr, &mut scope);
                    loop_ty = common_numeric_type(loop_ty, step_ty)?;
                }
                if !matches!(loop_ty, Type::Numeric(_)) {
                    return Err(Diagnostic::new("numeric for-loop bounds must be numeric"));
                }
                let mut loop_scope = scope.clone();
                loop_scope.insert(name.clone(), binding_for(loop_ty, Rebindability::Const));
                collect_return_types(
                    body,
                    &loop_scope,
                    fn_signatures,
                    active_type_params,
                    returns,
                )?;
            }
            Stmt::ForIn {
                names,
                iterator,
                body,
            } => {
                let iterator_ty =
                    infer_expr(iterator, &scope, fn_signatures, active_type_params, None)?;
                seal_record_locals_in_expr(iterator, &mut scope);
                let loop_value_types = match iterator_ty {
                    Type::Function {
                        params,
                        return_type,
                    } => {
                        if !params.is_empty() {
                            return Err(Diagnostic::new(
                                "for-in iterator function must not require parameters",
                            ));
                        }
                        let return_values = match *return_type {
                            Type::Multi(values) => values,
                            other => vec![other],
                        };
                        if return_values.len() != names.len() + 1 {
                            return Err(Diagnostic::new(format!(
                                "for-in iterator expects {} return values (bool + {} loop values), got {}",
                                names.len() + 1,
                                names.len(),
                                return_values.len()
                            )));
                        }
                        if return_values[0] != Type::Bool {
                            return Err(Diagnostic::new(
                                "for-in iterator first return value must be bool",
                            ));
                        }
                        return_values.into_iter().skip(1).collect::<Vec<_>>()
                    }
                    Type::Array(element_ty) => {
                        if names.len() == 1 {
                            vec![*element_ty]
                        } else if names.len() == 2 {
                            vec![Type::Numeric(NumericType::I32), *element_ty]
                        } else {
                            return Err(Diagnostic::new(format!(
                                "array for-in loop expects 1 or 2 loop variables, got {}",
                                names.len()
                            )));
                        }
                    }
                    _ => {
                        return Err(Diagnostic::new(
                            "for-in iterator must be a function or an array",
                        ));
                    }
                };
                let mut loop_scope = scope.clone();
                for (name, ty) in names.iter().zip(loop_value_types) {
                    loop_scope.insert(name.clone(), binding_for(ty, Rebindability::Const));
                }
                collect_return_types(
                    body,
                    &loop_scope,
                    fn_signatures,
                    active_type_params,
                    returns,
                )?;
            }
            Stmt::Break | Stmt::Continue => {}
            Stmt::Return(expr) => {
                seal_record_locals_in_expr(expr, &mut scope);
                returns.push(infer_expr(
                    expr,
                    &scope,
                    fn_signatures,
                    active_type_params,
                    None,
                )?);
            }
            Stmt::ReturnMulti(values) => {
                for value in values {
                    seal_record_locals_in_expr(value, &mut scope);
                }
                returns.push(Type::Multi(infer_expr_list(
                    values,
                    &scope,
                    fn_signatures,
                    active_type_params,
                    None,
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
                    let actual = infer_expr_list(
                        values,
                        &scope,
                        fn_signatures,
                        active_type_params,
                        Some(&expected),
                    )?;
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
                    let actual =
                        infer_expr_list(values, &scope, fn_signatures, active_type_params, None)?;
                    if actual.len() != bindings.len() {
                        return Err(Diagnostic::new(format!(
                            "multi-binding declaration expects {} values, got {}",
                            bindings.len(),
                            actual.len()
                        )));
                    }
                    actual
                };
                for value in values {
                    seal_record_locals_in_expr(value, &mut scope);
                }
                for (binding, value_ty) in bindings.iter().zip(actual) {
                    let ty = binding.ty.clone().unwrap_or(value_ty);
                    scope.insert(binding.name.clone(), binding_for(ty, binding.rebindability));
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
                let actual = infer_expr_list(
                    values,
                    &scope,
                    fn_signatures,
                    active_type_params,
                    Some(&expected),
                )?;
                for value in values {
                    seal_record_locals_in_expr(value, &mut scope);
                }
                if actual.len() != expected.len() {
                    return Err(Diagnostic::new(format!(
                        "multi-assignment expects {} values, got {}",
                        expected.len(),
                        actual.len()
                    )));
                }
            }
            Stmt::Expr(expr) => {
                let _ = infer_expr(expr, &scope, fn_signatures, active_type_params, None)?;
                seal_record_locals_in_expr(expr, &mut scope);
            }
        }
    }
    Ok(())
}

pub(super) fn common_return_type(left: Type, right: Type) -> Result<Type, Diagnostic> {
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

pub(super) fn check_stmt(
    stmt: &Stmt,
    vars: &mut HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
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
                let value_ty = infer_expr(
                    value,
                    vars,
                    fn_signatures,
                    active_type_params,
                    Some(expected_ty.clone()),
                )?;
                if &value_ty != expected_ty {
                    return Err(Diagnostic::new(format!(
                        "let '{}' expects {}, got {}",
                        name, expected_ty, value_ty
                    )));
                }
                expected_ty.clone()
            } else if matches!(value, Expr::ArrayLiteral { elements, .. } if elements.is_empty()) {
                Type::Record(BTreeMap::new())
            } else {
                infer_expr(value, vars, fn_signatures, active_type_params, None)?
            };
            seal_record_locals_in_expr(value, vars);
            vars.insert(name.clone(), binding_for(inferred_ty, *rebindability));
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
            let value_ty = infer_expr(
                value,
                vars,
                fn_signatures,
                active_type_params,
                Some(existing.ty.clone()),
            )?;
            if existing.ty != value_ty {
                return Err(Diagnostic::new(format!(
                    "assignment to '{}' expects {}, got {}",
                    name, existing.ty, value_ty
                )));
            }
            seal_record_locals_in_expr(value, vars);
            Ok(false)
        }
        Stmt::IndexAssign {
            op,
            base,
            index,
            value,
        } => {
            let base_ty = infer_expr(base, vars, fn_signatures, active_type_params, None)?;
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
                fn_signatures,
                active_type_params,
                Some(Type::Numeric(NumericType::I32)),
            )?;
            if index_ty != Type::Numeric(NumericType::I32) {
                return Err(Diagnostic::new("array index must be i32"));
            }
            let value_ty = infer_expr(
                value,
                vars,
                fn_signatures,
                active_type_params,
                Some(element_ty.clone()),
            )?;
            if value_ty != element_ty {
                return Err(Diagnostic::new(format!(
                    "array element assignment expects {}, got {}",
                    element_ty, value_ty
                )));
            }
            Ok(false)
        }
        Stmt::FieldAssign {
            op,
            base,
            name,
            value,
        } => {
            let base_name = match base.as_ref() {
                Expr::Name(local, _) => local,
                _ => {
                    return Err(Diagnostic::new(
                        "field assignment base must be a local name",
                    ));
                }
            };
            let binding = vars
                .get(base_name)
                .cloned()
                .ok_or_else(|| Diagnostic::new(format!("unknown local '{base_name}'")))?;
            let Type::Record(mut fields) = binding.ty else {
                return Err(Diagnostic::new("field assignment requires a record base"));
            };
            let existing_field = fields.get(name).cloned();
            let value_ty = infer_expr(
                value,
                vars,
                fn_signatures,
                active_type_params,
                existing_field.clone(),
            )?;
            if *op == AssignOp::Add {
                let field_ty = existing_field.clone().ok_or_else(|| {
                    Diagnostic::new("compound field assignment requires an existing numeric field")
                })?;
                if !field_ty.is_numeric() || value_ty != field_ty {
                    return Err(Diagnostic::new(
                        "compound field assignment requires a numeric field",
                    ));
                }
            }
            if let Some(ty) = fields.get(name) {
                if *ty != value_ty {
                    return Err(Diagnostic::new(format!(
                        "field assignment to '{}.{}' expects {}, got {}",
                        base_name, name, ty, value_ty
                    )));
                }
            } else if binding.record_open {
                fields.insert(name.clone(), value_ty);
            } else {
                return Err(Diagnostic::new(format!(
                    "cannot add new field '{}.{}' after record was sealed",
                    base_name, name
                )));
            }
            seal_record_locals_in_expr(value, vars);
            let mut updated = binding_for(Type::Record(fields), binding.rebindability);
            if !binding.record_open {
                updated.record_open = false;
            }
            vars.insert(base_name.clone(), updated);
            Ok(false)
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            let condition_ty =
                infer_expr(condition, vars, fn_signatures, active_type_params, None)?;
            seal_record_locals_in_expr(condition, vars);
            if condition_ty != Type::Bool {
                return Err(Diagnostic::new("if condition must be bool"));
            }
            let mut then_scope = vars.clone();
            let mut else_scope = vars.clone();
            let mut then_returns = false;
            let mut else_returns = false;
            for stmt in then_body {
                then_returns |= check_stmt(
                    stmt,
                    &mut then_scope,
                    fn_signatures,
                    active_type_params,
                    expected_return,
                    in_loop,
                )?;
            }
            for stmt in else_body {
                else_returns |= check_stmt(
                    stmt,
                    &mut else_scope,
                    fn_signatures,
                    active_type_params,
                    expected_return,
                    in_loop,
                )?;
            }
            Ok(then_returns && else_returns)
        }
        Stmt::While { condition, body } => {
            let condition_ty =
                infer_expr(condition, vars, fn_signatures, active_type_params, None)?;
            seal_record_locals_in_expr(condition, vars);
            if condition_ty != Type::Bool {
                return Err(Diagnostic::new("while condition must be bool"));
            }
            let mut loop_scope = vars.clone();
            for stmt in body {
                let _ = check_stmt(
                    stmt,
                    &mut loop_scope,
                    fn_signatures,
                    active_type_params,
                    expected_return,
                    true,
                )?;
            }
            Ok(false)
        }
        Stmt::Repeat { body, condition } => {
            let mut loop_scope = vars.clone();
            for stmt in body {
                let _ = check_stmt(
                    stmt,
                    &mut loop_scope,
                    fn_signatures,
                    active_type_params,
                    expected_return,
                    true,
                )?;
            }
            let condition_ty = infer_expr(
                condition,
                &loop_scope,
                fn_signatures,
                active_type_params,
                None,
            )?;
            seal_record_locals_in_expr(condition, &mut loop_scope);
            if condition_ty != Type::Bool {
                return Err(Diagnostic::new("repeat-until condition must be bool"));
            }
            Ok(false)
        }
        Stmt::NumericFor {
            name,
            start,
            stop,
            step,
            body,
        } => {
            let start_ty = infer_expr(start, vars, fn_signatures, active_type_params, None)?;
            let stop_ty = infer_expr(stop, vars, fn_signatures, active_type_params, None)?;
            seal_record_locals_in_expr(start, vars);
            seal_record_locals_in_expr(stop, vars);
            let mut loop_ty = common_numeric_type(start_ty, stop_ty)?;
            if let Some(step_expr) = step {
                let step_ty = infer_expr(step_expr, vars, fn_signatures, active_type_params, None)?;
                seal_record_locals_in_expr(step_expr, vars);
                loop_ty = common_numeric_type(loop_ty, step_ty)?;
            }
            if !matches!(loop_ty, Type::Numeric(_)) {
                return Err(Diagnostic::new("numeric for-loop bounds must be numeric"));
            }
            let mut loop_scope = vars.clone();
            loop_scope.insert(name.clone(), binding_for(loop_ty, Rebindability::Const));
            for stmt in body {
                let _ = check_stmt(
                    stmt,
                    &mut loop_scope,
                    fn_signatures,
                    active_type_params,
                    expected_return,
                    true,
                )?;
            }
            Ok(false)
        }
        Stmt::ForIn {
            names,
            iterator,
            body,
        } => {
            let iterator_ty = infer_expr(iterator, vars, fn_signatures, active_type_params, None)?;
            seal_record_locals_in_expr(iterator, vars);
            let loop_value_types = match iterator_ty {
                Type::Function {
                    params,
                    return_type,
                } => {
                    if !params.is_empty() {
                        return Err(Diagnostic::new(
                            "for-in iterator function must not require parameters",
                        ));
                    }
                    let return_values = match *return_type {
                        Type::Multi(values) => values,
                        other => vec![other],
                    };
                    if return_values.len() != names.len() + 1 {
                        return Err(Diagnostic::new(format!(
                            "for-in iterator expects {} return values (bool + {} loop values), got {}",
                            names.len() + 1,
                            names.len(),
                            return_values.len()
                        )));
                    }
                    if return_values[0] != Type::Bool {
                        return Err(Diagnostic::new(
                            "for-in iterator first return value must be bool",
                        ));
                    }
                    return_values.into_iter().skip(1).collect::<Vec<_>>()
                }
                Type::Array(element_ty) => {
                    if names.len() == 1 {
                        vec![*element_ty]
                    } else if names.len() == 2 {
                        vec![Type::Numeric(NumericType::I32), *element_ty]
                    } else {
                        return Err(Diagnostic::new(format!(
                            "array for-in loop expects 1 or 2 loop variables, got {}",
                            names.len()
                        )));
                    }
                }
                _ => {
                    return Err(Diagnostic::new(
                        "for-in iterator must be a function or an array",
                    ));
                }
            };
            let mut loop_scope = vars.clone();
            for (name, ty) in names.iter().zip(loop_value_types) {
                loop_scope.insert(name.clone(), binding_for(ty, Rebindability::Const));
            }
            for stmt in body {
                let _ = check_stmt(
                    stmt,
                    &mut loop_scope,
                    fn_signatures,
                    active_type_params,
                    expected_return,
                    true,
                )?;
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
            seal_record_locals_in_expr(expr, vars);
            let ty = infer_expr(
                expr,
                vars,
                fn_signatures,
                active_type_params,
                Some(expected_return.clone()),
            )?;
            if &ty != expected_return {
                return Err(Diagnostic::new(format!(
                    "return expects {}, got {}",
                    expected_return, ty
                )));
            }
            Ok(true)
        }
        Stmt::ReturnMulti(values) => {
            for value in values {
                seal_record_locals_in_expr(value, vars);
            }
            let expected = match expected_return {
                Type::Multi(types) => types.clone(),
                _ => vec![expected_return.clone()],
            };
            let actual = infer_expr_list(
                values,
                vars,
                fn_signatures,
                active_type_params,
                Some(&expected),
            )?;
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
                let actual = infer_expr_list(
                    values,
                    vars,
                    fn_signatures,
                    active_type_params,
                    Some(&expected),
                )?;
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
                let actual =
                    infer_expr_list(values, vars, fn_signatures, active_type_params, None)?;
                if actual.len() != bindings.len() {
                    return Err(Diagnostic::new(format!(
                        "multi-binding declaration expects {} values, got {}",
                        bindings.len(),
                        actual.len()
                    )));
                }
                actual
            };
            for value in values {
                seal_record_locals_in_expr(value, vars);
            }
            for (binding, value_ty) in bindings.iter().zip(actual) {
                let ty = binding.ty.clone().unwrap_or(value_ty);
                vars.insert(binding.name.clone(), binding_for(ty, binding.rebindability));
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
            let actual = infer_expr_list(
                values,
                vars,
                fn_signatures,
                active_type_params,
                Some(&expected),
            )?;
            for value in values {
                seal_record_locals_in_expr(value, vars);
            }
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
            if let Expr::Call {
                callee,
                type_args: _,
                args,
                ..
            } = expr
            {
                if let Expr::Name(name, _) = callee.as_ref() {
                    if name == ASSERT {
                        if args.len() != 1 {
                            return Err(Diagnostic::new(format!(
                                "{ASSERT} expects 1 argument, got {}",
                                args.len()
                            )));
                        }
                        let actual = infer_expr(
                            &args[0],
                            vars,
                            fn_signatures,
                            active_type_params,
                            Some(Type::Bool),
                        )?;
                        if actual != Type::Bool {
                            return Err(Diagnostic::new(format!(
                                "{ASSERT} expects bool, got {actual}"
                            )));
                        }
                        return Ok(false);
                    }
                }
            }
            let _ = infer_expr(expr, vars, fn_signatures, active_type_params, None)?;
            seal_record_locals_in_expr(expr, vars);
            Ok(false)
        }
    }
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
        Stmt::FieldAssign { base, value, .. } => {
            expr_calls_name(base, callee) || expr_calls_name(value, callee)
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
        Stmt::NumericFor {
            start,
            stop,
            step,
            body,
            ..
        } => {
            expr_calls_name(start, callee)
                || expr_calls_name(stop, callee)
                || step
                    .as_ref()
                    .is_some_and(|step_expr| expr_calls_name(step_expr, callee))
                || body.iter().any(|stmt| stmt_calls_name(stmt, callee))
        }
        Stmt::ForIn { iterator, body, .. } => {
            expr_calls_name(iterator, callee)
                || body.iter().any(|stmt| stmt_calls_name(stmt, callee))
        }
        Stmt::Break | Stmt::Continue => false,
    }
}

fn expr_calls_name(expr: &Expr, callee: &str) -> bool {
    match expr {
        Expr::Name(..)
        | Expr::Number(..)
        | Expr::Bool(..)
        | Expr::String(..)
        | Expr::Require(..) => false,
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => expr_calls_name(expr, callee),
        Expr::Binary { left, right, .. } => {
            expr_calls_name(left, callee) || expr_calls_name(right, callee)
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            expr_calls_name(condition, callee)
                || expr_calls_name(then_expr, callee)
                || expr_calls_name(else_expr, callee)
        }
        Expr::Call {
            callee: called,
            type_args: _,
            args,
            ..
        } => {
            matches!(called.as_ref(), Expr::Name(name, _) if name == callee)
                || expr_calls_name(called, callee)
                || args.iter().any(|arg| expr_calls_name(arg, callee))
        }
        Expr::MethodCall { receiver, args, .. } => {
            expr_calls_name(receiver, callee) || args.iter().any(|arg| expr_calls_name(arg, callee))
        }
        Expr::Function(function) => function
            .body
            .iter()
            .any(|stmt| stmt_calls_name(stmt, callee)),
        Expr::ArrayLiteral { elements, .. } => {
            elements.iter().any(|el| expr_calls_name(el, callee))
        }
        Expr::TableLiteral { fields, .. } => fields
            .iter()
            .any(|field| expr_calls_name(&field.value, callee)),
        Expr::Field { base, .. } => expr_calls_name(base, callee),
        Expr::Index { base, index, .. } => {
            expr_calls_name(base, callee) || expr_calls_name(index, callee)
        }
    }
}

pub(super) fn function_calls(function: &Function, callee: &str) -> bool {
    function
        .body
        .iter()
        .any(|stmt| stmt_calls_name(stmt, callee))
}

fn seal_record_locals_in_expr(expr: &Expr, vars: &mut HashMap<String, Binding>) {
    match expr {
        Expr::Name(name, _) => {
            if let Some(binding) = vars.get_mut(name) {
                if matches!(binding.ty, Type::Record(_)) {
                    binding.record_open = false;
                }
            }
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => {
            seal_record_locals_in_expr(expr, vars)
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            seal_record_locals_in_expr(condition, vars);
            seal_record_locals_in_expr(then_expr, vars);
            seal_record_locals_in_expr(else_expr, vars);
        }
        Expr::Call { callee, args, .. } => {
            seal_record_locals_in_expr(callee, vars);
            for arg in args {
                seal_record_locals_in_expr(arg, vars);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            seal_record_locals_in_expr(receiver, vars);
            for arg in args {
                seal_record_locals_in_expr(arg, vars);
            }
        }
        Expr::Function(_)
        | Expr::Number(..)
        | Expr::Bool(..)
        | Expr::String(..)
        | Expr::Require(..) => {}
        Expr::ArrayLiteral { elements, .. } => {
            for element in elements {
                seal_record_locals_in_expr(element, vars);
            }
        }
        Expr::TableLiteral { fields, .. } => {
            for field in fields {
                seal_record_locals_in_expr(&field.value, vars);
            }
        }
        Expr::Field { base, .. } => seal_record_locals_in_expr(base, vars),
        Expr::Index { base, index, .. } => {
            seal_record_locals_in_expr(base, vars);
            seal_record_locals_in_expr(index, vars);
        }
        Expr::Binary { left, right, .. } => {
            seal_record_locals_in_expr(left, vars);
            seal_record_locals_in_expr(right, vars);
        }
    }
}
