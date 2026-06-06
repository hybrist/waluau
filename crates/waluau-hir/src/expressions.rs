use std::collections::{BTreeMap, HashMap, HashSet};

use waluau_ast::{BinaryOp, Expr, FunctionExpr, NumericType, Type, UnaryOp};
use waluau_diagnostics::{Diagnostic, DiagnosticCategory};

use super::Binding;
use super::builtins::{
    infer_coroutine_builtin_call, infer_math_builtin_call, infer_print_builtin_call,
    infer_tostring_builtin_call,
};
use super::numeric::{
    coerce_type, common_element_type, infer_numeric_common_type, require_bool_pair,
    require_numeric_cast, resolve_number_literal,
};
use super::signatures::{
    FnSignature, generic_diagnostic, infer_generic_call, infer_generic_function_expr_call,
};
use super::statements::check_stmt;

fn builtin_name(callee: &Expr) -> Option<String> {
    match callee {
        Expr::Name(name, _) => Some(name.clone()),
        Expr::Field { base, name, .. } => match base.as_ref() {
            Expr::Name(namespace, _) => Some(format!("{namespace}.{name}")),
            _ => None,
        },
        _ => None,
    }
}

fn method_signature_name(base: &str, method: &str) -> String {
    format!("{base}.{method}")
}

fn method_receiver_matches(expected: &Type, actual: &Type) -> bool {
    if expected == actual {
        return true;
    }
    match (expected, actual) {
        (Type::Record(expected_fields), Type::Record(actual_fields)) => expected_fields
            .iter()
            .all(|(name, expected_ty)| actual_fields.get(name) == Some(expected_ty)),
        _ => false,
    }
}

fn method_signature<'a>(
    receiver: &Expr,
    name: &str,
    fn_signatures: &'a HashMap<String, FnSignature>,
) -> Option<(&'a FnSignature, String)> {
    let Expr::Name(base, _) = receiver else {
        return None;
    };
    let method_name = method_signature_name(base, name);
    fn_signatures
        .get(&method_name)
        .map(|signature| (signature, method_name))
}

pub(super) fn infer_expr(
    expr: &Expr,
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Result<Type, Diagnostic> {
    match expr {
        Expr::Number(value, _) => resolve_number_literal(value, expected),
        Expr::Bool(..) => Ok(Type::Bool),
        Expr::String(..) => coerce_type(Type::String, expected),
        Expr::Bytes(..) => coerce_type(Type::Bytes, expected),
        Expr::Require(path, _) => Err(Diagnostic::new(format!(
            "require(\"{path}\") can only be resolved when compiling from a file; \
             relative imports are unavailable when compiling a single source string"
        ))),
        Expr::Name(name, _) => {
            if matches!(fn_signatures.get(name), Some(FnSignature::Generic(_))) {
                return Err(generic_diagnostic(
                    "generic/uninstantiated-value",
                    format!(
                        "generic function '{name}' cannot be used as a value without type arguments"
                    ),
                    "call the generic function with explicit type arguments, e.g. id<i32>(value)",
                ));
            }
            let actual = if let Some(local) = vars.get(name) {
                local.ty.clone()
            } else if let Some(FnSignature::Mono {
                params,
                return_type,
            }) = fn_signatures.get(name)
            {
                Type::Function {
                    params: params.clone(),
                    return_type: Box::new(return_type.clone()),
                }
            } else {
                return Err(Diagnostic::new(format!("unknown name '{name}'")));
            };
            coerce_type(actual, expected)
        }
        Expr::Unary { op, expr, .. } => match op {
            UnaryOp::Neg => {
                let actual = infer_expr(
                    expr,
                    vars,
                    fn_signatures,
                    active_type_params,
                    expected.clone(),
                )?;
                match actual {
                    Type::Numeric(_) => coerce_type(actual, expected),
                    Type::Bool => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                    Type::Unit => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                    Type::String => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                    Type::Bytes => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                    Type::Named { .. } => {
                        Err(Diagnostic::new("unary '-' requires a numeric operand"))
                    }
                    Type::Opaque { .. } => {
                        Err(Diagnostic::new("unary '-' requires a numeric operand"))
                    }
                    Type::Array(_) => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                    Type::Multi(_) => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                    Type::Function { .. }
                    | Type::Record(_)
                    | Type::TypeParam(_)
                    | Type::Thread
                    | Type::Unknown
                    | Type::TaggedVariant(_)
                    | Type::TaggedUnion(_) => {
                        Err(Diagnostic::new("unary '-' requires a numeric operand"))
                    }
                }
            }
            UnaryOp::Not => {
                let actual = infer_expr(
                    expr,
                    vars,
                    fn_signatures,
                    active_type_params,
                    Some(Type::Bool),
                )?;
                if actual != Type::Bool {
                    return Err(Diagnostic::new("unary 'not' requires a bool operand"));
                }
                coerce_type(Type::Bool, expected)
            }
            UnaryOp::Len => {
                let actual = infer_expr(expr, vars, fn_signatures, active_type_params, None)?;
                if actual == Type::Bytes {
                    return coerce_type(Type::Numeric(NumericType::I32), expected);
                }
                if !actual.is_array() {
                    return Err(Diagnostic::new("# requires an array or bytes operand"));
                }
                coerce_type(Type::Numeric(NumericType::I32), expected)
            }
        },
        Expr::Cast { expr, ty, .. } => {
            let actual = infer_expr(expr, vars, fn_signatures, active_type_params, None)?;
            require_numeric_cast(actual, ty.clone())?;
            coerce_type(ty.clone(), expected)
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            let condition_ty = infer_expr(
                condition,
                vars,
                fn_signatures,
                active_type_params,
                Some(Type::Bool),
            )?;
            if condition_ty != Type::Bool {
                return Err(Diagnostic::new("if expression condition must be bool"));
            }
            let then_ty = infer_expr(
                then_expr,
                vars,
                fn_signatures,
                active_type_params,
                expected.clone(),
            )?;
            let else_ty = infer_expr(
                else_expr,
                vars,
                fn_signatures,
                active_type_params,
                expected.clone(),
            )?;
            if then_ty == else_ty {
                Ok(then_ty)
            } else {
                Err(Diagnostic::new(
                    "if expression branches must resolve to the same type",
                ))
            }
        }
        Expr::Call {
            callee,
            type_args,
            args,
            ..
        } => {
            if let Some(name) = builtin_name(callee.as_ref()) {
                if let Some(result) = infer_math_builtin_call(
                    &name,
                    args,
                    vars,
                    fn_signatures,
                    active_type_params,
                    expected.clone(),
                ) {
                    return result;
                }
            }
            if let Some(name) = builtin_name(callee.as_ref()) {
                if let Some(result) = infer_coroutine_builtin_call(
                    &name,
                    args,
                    vars,
                    fn_signatures,
                    active_type_params,
                    expected.clone(),
                ) {
                    return result;
                }
            }
            if let Some(name) = builtin_name(callee.as_ref()) {
                if let Some(result) = infer_tostring_builtin_call(
                    &name,
                    args,
                    vars,
                    fn_signatures,
                    active_type_params,
                    expected.clone(),
                ) {
                    return result;
                }
            }
            if let Some(name) = builtin_name(callee.as_ref()) {
                if let Some(result) = infer_print_builtin_call(
                    &name,
                    args,
                    vars,
                    fn_signatures,
                    active_type_params,
                    expected.clone(),
                ) {
                    return result;
                }
            }
            if let Expr::Name(name, _) = callee.as_ref() {
                if let Some(FnSignature::Generic(scheme)) = fn_signatures.get(name) {
                    return infer_generic_call(
                        scheme,
                        type_args,
                        args,
                        vars,
                        fn_signatures,
                        active_type_params,
                        expected,
                    );
                }
            }
            if let Expr::Field { base, name, .. } = callee.as_ref() {
                if let Expr::Name(base_name, _) = base.as_ref() {
                    let method_name = method_signature_name(base_name, name);
                    if let Some(FnSignature::Generic(scheme)) = fn_signatures.get(&method_name) {
                        return infer_generic_call(
                            scheme,
                            type_args,
                            args,
                            vars,
                            fn_signatures,
                            active_type_params,
                            expected,
                        );
                    }
                }
            }
            if let Expr::Function(function) = callee.as_ref() {
                if !function.type_params.is_empty() {
                    return infer_generic_function_expr_call(
                        function,
                        type_args,
                        args,
                        vars,
                        fn_signatures,
                        active_type_params,
                        expected,
                    );
                }
            }
            if !type_args.is_empty() {
                return Err(generic_diagnostic(
                    "generic/extra-type-args",
                    "type arguments are only allowed when calling a generic function",
                    "remove the type argument list or call a generic function",
                ));
            }
            let callee_ty = infer_expr(callee, vars, fn_signatures, active_type_params, None)?;
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
            let actual_args =
                infer_expr_list(args, vars, fn_signatures, active_type_params, Some(&params))?;
            if params.len() != actual_args.len() {
                return Err(Diagnostic::new(format!(
                    "function expects {} arguments, got {}",
                    params.len(),
                    actual_args.len()
                )));
            }
            for (expected_param, actual) in params.iter().zip(actual_args.iter()) {
                if expected_param != actual {
                    return Err(Diagnostic::new(format!(
                        "call expected {}, got {}",
                        expected_param, actual
                    )));
                }
            }
            coerce_type(ret, expected)
        }
        Expr::MethodCall {
            receiver,
            name,
            type_args,
            args,
            ..
        } => {
            let receiver_ty = infer_expr(receiver, vars, fn_signatures, active_type_params, None)?;
            let (params, ret) = if let Some((signature, method_name)) =
                method_signature(receiver, name, fn_signatures)
            {
                match signature {
                    FnSignature::Mono {
                        params,
                        return_type,
                    } => {
                        if !type_args.is_empty() {
                            return Err(generic_diagnostic(
                                "generic/extra-type-args",
                                "type arguments are only allowed when calling a generic method",
                                "remove the type argument list or call a generic method",
                            ));
                        }
                        (params.clone(), return_type.clone())
                    }
                    FnSignature::Generic(scheme) => {
                        if type_args.is_empty() {
                            return Err(generic_diagnostic(
                                "generic/missing-type-args",
                                format!(
                                    "generic method '{method_name}' requires explicit type arguments"
                                ),
                                "provide type arguments between the method name and argument list, e.g. obj:method<T>(args)",
                            ));
                        }
                        // Create a modified args list with the receiver as the first argument
                        let mut method_args = vec![(**receiver).clone()];
                        method_args.extend_from_slice(args);

                        return infer_generic_call(
                            scheme,
                            type_args,
                            &method_args,
                            vars,
                            fn_signatures,
                            active_type_params,
                            expected,
                        );
                    }
                }
            } else {
                if !type_args.is_empty() {
                    return Err(generic_diagnostic(
                        "generic/extra-type-args",
                        "type arguments are only allowed when calling a generic method",
                        "remove the type argument list or call a generic method",
                    ));
                }
                let field_ty = receiver_ty
                    .record_field(name)
                    .ok_or_else(|| Diagnostic::new(format!("unknown record field '{name}'")))?;
                match field_ty {
                    Type::Function {
                        params,
                        return_type,
                    } => (params, *return_type),
                    other => {
                        return Err(Diagnostic::new(format!(
                            "attempt to call non-function value of type {other}",
                        )));
                    }
                }
            };
            if params.is_empty() {
                return Err(Diagnostic::new(format!(
                    "function expects 0 arguments, got {}",
                    args.len() + 1
                )));
            }
            if !method_receiver_matches(&params[0], &receiver_ty) {
                return Err(Diagnostic::new(format!(
                    "call expected {}, got {}",
                    params[0], receiver_ty
                )));
            }
            let actual_args = infer_expr_list(
                args,
                vars,
                fn_signatures,
                active_type_params,
                Some(&params[1..]),
            )?;
            if params.len() != actual_args.len() + 1 {
                return Err(Diagnostic::new(format!(
                    "function expects {} arguments, got {}",
                    params.len(),
                    actual_args.len() + 1
                )));
            }
            for (expected_param, actual) in params.iter().skip(1).zip(actual_args.iter()) {
                if expected_param != actual {
                    return Err(Diagnostic::new(format!(
                        "call expected {}, got {}",
                        expected_param, actual
                    )));
                }
            }
            coerce_type(ret, expected)
        }
        Expr::Function(function) => {
            infer_function_expr(function, vars, fn_signatures, active_type_params, expected)
        }
        Expr::ArrayLiteral { elements, .. } => {
            infer_array_literal(elements, vars, fn_signatures, active_type_params, expected)
        }
        Expr::TableLiteral { fields, .. } => {
            let expected_fields = if let Some(Type::Record(fields)) = expected.as_ref() {
                Some(fields.clone())
            } else {
                None
            };
            let mut record_fields = BTreeMap::new();
            for field in fields {
                let expected_field_ty = expected_fields
                    .as_ref()
                    .and_then(|ef| ef.get(&field.name))
                    .cloned();
                let field_ty = infer_expr(
                    &field.value,
                    vars,
                    fn_signatures,
                    active_type_params,
                    expected_field_ty,
                )?;
                record_fields.insert(field.name.clone(), field_ty);
            }
            coerce_type(Type::Record(record_fields), expected)
        }
        Expr::Field { base, name, .. } => {
            if let Expr::Name(base_name, _) = base.as_ref() {
                let method_name = method_signature_name(base_name, name);
                if let Some(signature) = fn_signatures.get(&method_name) {
                    return match signature {
                        FnSignature::Mono {
                            params,
                            return_type,
                        } => coerce_type(
                            Type::Function {
                                params: params.clone(),
                                return_type: Box::new(return_type.clone()),
                            },
                            expected,
                        ),
                        FnSignature::Generic(_) => Err(generic_diagnostic(
                            "generic/uninstantiated-value",
                            format!(
                                "generic function '{method_name}' cannot be used as a value without type arguments"
                            ),
                            "call the generic function with explicit type arguments",
                        )),
                    };
                }
            }
            let base_ty = infer_expr(base, vars, fn_signatures, active_type_params, None)?;
            let Type::Record(fields) = base_ty else {
                return Err(Diagnostic::new("field access requires a record base"));
            };
            let Some(field_ty) = fields.get(name) else {
                return Err(Diagnostic::new(format!("unknown record field '{name}'")));
            };
            coerce_type(field_ty.clone(), expected)
        }
        Expr::Index { base, index, .. } => {
            let base_ty = infer_expr(base, vars, fn_signatures, active_type_params, None)?;
            let element_ty = if base_ty == Type::Bytes {
                Type::Numeric(NumericType::I32)
            } else {
                base_ty
                    .element_type()
                    .ok_or_else(|| Diagnostic::new("indexing requires an array or bytes operand"))?
            };
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
            coerce_type(element_ty, expected)
        }
        Expr::Binary {
            op, left, right, ..
        } => match op {
            BinaryOp::Concat => {
                let left_ty = infer_expr(left, vars, fn_signatures, active_type_params, None)?;
                if left_ty == Type::String {
                    let right_ty = infer_expr(
                        right,
                        vars,
                        fn_signatures,
                        active_type_params,
                        Some(Type::String),
                    )?;
                    if right_ty != Type::String {
                        return Err(Diagnostic::new(
                            "string concatenation requires both operands to be strings",
                        ));
                    }
                    return coerce_type(Type::String, expected);
                }
                if left_ty == Type::Bytes {
                    let right_ty = infer_expr(
                        right,
                        vars,
                        fn_signatures,
                        active_type_params,
                        Some(Type::Bytes),
                    )?;
                    if right_ty != Type::Bytes {
                        return Err(Diagnostic::new(
                            "bytes concatenation requires both operands to be bytes",
                        ));
                    }
                    return coerce_type(Type::Bytes, expected);
                }
                Err(Diagnostic::new(
                    "concatenation requires both operands to be strings or both operands to be bytes",
                ))
            }
            BinaryOp::Add => {
                let operand_ty = infer_numeric_common_type(
                    left,
                    right,
                    vars,
                    fn_signatures,
                    active_type_params,
                    expected.clone(),
                )?;
                coerce_type(operand_ty, expected)
            }
            BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::FloorDiv | BinaryOp::Mod => {
                let operand_ty = infer_numeric_common_type(
                    left,
                    right,
                    vars,
                    fn_signatures,
                    active_type_params,
                    expected.clone(),
                )?;
                coerce_type(operand_ty, expected)
            }
            BinaryOp::Less | BinaryOp::Greater => {
                let left_ty = infer_expr(left, vars, fn_signatures, active_type_params, None)?;
                if left_ty == Type::Bytes {
                    let right_ty = infer_expr(
                        right,
                        vars,
                        fn_signatures,
                        active_type_params,
                        Some(Type::Bytes),
                    )?;
                    if right_ty != Type::Bytes {
                        return Err(Diagnostic::new(
                            "< and > require both sides to have same type",
                        ));
                    }
                } else if left_ty == Type::String {
                    let right_ty = infer_expr(
                        right,
                        vars,
                        fn_signatures,
                        active_type_params,
                        Some(Type::String),
                    )?;
                    if right_ty != Type::String {
                        return Err(Diagnostic::new(
                            "< and > require both sides to have same type",
                        ));
                    }
                } else {
                    let _ = infer_numeric_common_type(
                        left,
                        right,
                        vars,
                        fn_signatures,
                        active_type_params,
                        None,
                    )?;
                }
                Ok(Type::Bool)
            }
            BinaryOp::And | BinaryOp::Or => {
                let left_ty = infer_expr(
                    left,
                    vars,
                    fn_signatures,
                    active_type_params,
                    Some(Type::Bool),
                )?;
                let right_ty = infer_expr(
                    right,
                    vars,
                    fn_signatures,
                    active_type_params,
                    Some(Type::Bool),
                )?;
                require_bool_pair(left_ty, right_ty)?;
                Ok(Type::Bool)
            }
            BinaryOp::Eq => {
                let left_ty = infer_expr(left, vars, fn_signatures, active_type_params, None)?;
                if left_ty == Type::Bool {
                    let right_ty = infer_expr(
                        right,
                        vars,
                        fn_signatures,
                        active_type_params,
                        Some(Type::Bool),
                    )?;
                    if right_ty != Type::Bool {
                        return Err(Diagnostic::new("== requires both sides to have same type"));
                    }
                } else if left_ty.is_numeric() {
                    let _ = infer_numeric_common_type(
                        left,
                        right,
                        vars,
                        fn_signatures,
                        active_type_params,
                        None,
                    )?;
                } else if left_ty == Type::String {
                    let right_ty = infer_expr(
                        right,
                        vars,
                        fn_signatures,
                        active_type_params,
                        Some(Type::String),
                    )?;
                    if right_ty != Type::String {
                        return Err(Diagnostic::new("== requires both sides to have same type"));
                    }
                } else if left_ty == Type::Bytes {
                    let right_ty = infer_expr(
                        right,
                        vars,
                        fn_signatures,
                        active_type_params,
                        Some(Type::Bytes),
                    )?;
                    if right_ty != Type::Bytes {
                        return Err(Diagnostic::new("== requires both sides to have same type"));
                    }
                } else {
                    return Err(Diagnostic::new(
                        "== supports only numeric, bool, string, and bytes operands in MVP",
                    ));
                }
                Ok(Type::Bool)
            }
        },
    }
}

pub(super) fn infer_expr_list(
    exprs: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<&[Type]>,
) -> Result<Vec<Type>, Diagnostic> {
    let mut out = Vec::new();
    for expr in exprs {
        let remaining_expected = expected
            .and_then(|types| (!types[out.len()..].is_empty()).then(|| &types[out.len()..]));
        let next_expected = remaining_expected.and_then(|types| types.first().cloned());
        let ty = if let Expr::Call { callee, .. } = expr {
            let call_expected = builtin_name(callee.as_ref()).and_then(|name| {
                (name == "coroutine.resume")
                    .then(|| remaining_expected.map(|types| Type::Multi(types.to_vec())))
                    .flatten()
            });
            infer_expr(expr, vars, fn_signatures, active_type_params, call_expected)?
        } else if matches!(expr, Expr::MethodCall { .. }) {
            infer_expr(expr, vars, fn_signatures, active_type_params, None)?
        } else {
            infer_expr(expr, vars, fn_signatures, active_type_params, next_expected)?
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
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Result<Type, Diagnostic> {
    if elements.is_empty() {
        return Err(super::signatures::inference_diagnostic(
            "inference/missing-context",
            DiagnosticCategory::MissingContext,
            "empty array literal requires explicit element type",
            "add an explicit element type annotation, e.g. local xs: {i32} = {}",
        ));
    }

    let expected_element = expected.as_ref().and_then(Type::element_type);
    let mut iter = elements.iter();
    let first = iter.next().expect("non-empty array literal");
    let mut element_ty = infer_expr(
        first,
        vars,
        fn_signatures,
        active_type_params,
        expected_element.clone(),
    )?;
    for element in iter {
        let actual = infer_expr(
            element,
            vars,
            fn_signatures,
            active_type_params,
            Some(element_ty.clone()),
        )?;
        element_ty = common_element_type(element_ty, actual)?;
    }

    let array_ty = Type::Array(Box::new(element_ty));
    coerce_type(array_ty, expected)
}

fn infer_function_expr(
    function: &FunctionExpr,
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Result<Type, Diagnostic> {
    super::signatures::validate_type_param_list(&function.type_params, active_type_params)?;
    if !function.type_params.is_empty() {
        return Err(generic_diagnostic(
            "generic/uninstantiated-value",
            "generic function expressions cannot be used as values without instantiation",
            "call the generic function expression with explicit type arguments immediately",
        ));
    }
    let return_ty = function.return_type.clone().ok_or_else(|| {
        super::signatures::inference_diagnostic(
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
            super::binding_for(param.ty.clone(), waluau_ast::Rebindability::Rebindable),
        );
    }
    if let Some(name) = &function.name {
        local_scope.insert(
            name.clone(),
            super::binding_for(function_ty.clone(), waluau_ast::Rebindability::Rebindable),
        );
    }
    let mut saw_return = false;
    for stmt in &function.body {
        if check_stmt(
            stmt,
            &mut local_scope,
            fn_signatures,
            active_type_params,
            &return_ty,
            false,
        )? {
            saw_return = true;
        }
    }
    if !saw_return && return_ty != Type::Unit {
        return Err(Diagnostic::new("function expression is missing a return"));
    }
    coerce_type(function_ty, expected)
}
