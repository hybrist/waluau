use std::collections::{BTreeMap, HashMap, HashSet};

use waluau_ast::{BinaryOp, Expr, FunctionExpr, NumericType, Type, UnaryOp};
use waluau_diagnostics::{Diagnostic, DiagnosticCategory};

use super::Binding;
use super::builtins::{
    STRING_BYTE, STRING_FIND, STRING_FORMAT, STRING_GMATCH, STRING_GSUB, STRING_LEN, STRING_LOWER,
    STRING_MATCH, STRING_REP, STRING_REVERSE, STRING_SUB, STRING_UPPER, infer_bit32_builtin_call,
    infer_coroutine_builtin_call, infer_error_builtin_call, infer_pcall_builtin_call,
    infer_promise_await_method_call, infer_promise_builtin_call, infer_select_builtin_call,
    infer_string_builtin_call, infer_table_builtin_call, infer_tonumber_builtin_call,
    infer_tostring_builtin_call, infer_type_builtin_call,
};
use super::numeric::{
    coerce_type, common_element_type, infer_numeric_common_type, is_extern_subtype_of,
    require_bool_pair, require_numeric_cast, resolve_number_literal,
};
use super::signatures::{
    FnSignature, OverloadVariant, call_arity_matches, generic_diagnostic, infer_generic_call,
    infer_generic_function_expr_call, required_param_count,
};
use super::statements::check_stmt;

pub(super) fn builtin_name(callee: &Expr) -> Option<String> {
    match callee {
        Expr::Name(name, _, _) => Some(name.clone()),
        Expr::Field { base, name, .. } => match base.as_ref() {
            Expr::Name(namespace, _, _) => Some(format!("{namespace}.{name}")),
            _ => None,
        },
        _ => None,
    }
}

fn method_signature_name(base: &str, method: &str) -> String {
    format!("{base}.{method}")
}

fn property_getter_name(base: &str, property: &str) -> String {
    format!("{base}.get/{property}")
}

fn record_fields(ty: &Type) -> Option<&BTreeMap<String, Type>> {
    match ty {
        Type::Record(fields) => Some(fields),
        Type::Opaque { ty, .. } | Type::Nullable(ty) => record_fields(ty),
        _ => None,
    }
}

fn method_receiver_matches(expected: &Type, actual: &Type) -> bool {
    if expected == actual {
        return true;
    }
    if is_extern_subtype_of(actual, expected) {
        return true;
    }
    // Structural record match, unwrapping opaque aliases on either side so
    // methods dispatch across differently-named aliases of the same record —
    // mirroring how coerce_type unifies record aliases (e.g. one module's
    // State value passed to another module's structurally identical alias).
    match (record_fields(expected), record_fields(actual)) {
        (Some(expected_fields), Some(actual_fields)) => expected_fields
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
    let Expr::Name(base, _, _) = receiver else {
        return None;
    };
    let method_name = method_signature_name(base, name);
    fn_signatures
        .get(&method_name)
        .map(|signature| (signature, method_name))
}

fn type_method_signature<'a>(
    receiver_ty: &Type,
    name: &str,
    fn_signatures: &'a HashMap<String, FnSignature>,
) -> Option<(&'a FnSignature, String)> {
    let method_name = resolved_type_method_name(receiver_ty, name, fn_signatures)?;
    let signature = fn_signatures.get(&method_name)?;
    Some((signature, method_name))
}

fn binary_operator_method(op: &BinaryOp) -> Option<&'static str> {
    match op {
        BinaryOp::Add => Some("__add"),
        BinaryOp::Sub => Some("__sub"),
        BinaryOp::Mul => Some("__mul"),
        BinaryOp::Div => Some("__div"),
        BinaryOp::Pow => Some("__pow"),
        BinaryOp::FloorDiv
        | BinaryOp::Mod
        | BinaryOp::Concat
        | BinaryOp::Less
        | BinaryOp::LessEq
        | BinaryOp::Greater
        | BinaryOp::GreaterEq
        | BinaryOp::Eq
        | BinaryOp::NotEq
        | BinaryOp::And
        | BinaryOp::Or => None,
    }
}

fn binary_operator_symbol(op: &BinaryOp) -> Option<&'static str> {
    match op {
        BinaryOp::Add => Some("+"),
        BinaryOp::Sub => Some("-"),
        BinaryOp::Mul => Some("*"),
        BinaryOp::Div => Some("/"),
        BinaryOp::Pow => Some("^"),
        _ => None,
    }
}

fn unary_operator_method(op: &UnaryOp) -> Option<&'static str> {
    match op {
        UnaryOp::Neg => Some("__neg"),
        UnaryOp::Not | UnaryOp::Len => None,
    }
}

fn overload_eligible_type(ty: &Type) -> bool {
    match ty {
        Type::Opaque { .. } | Type::Extern | Type::ExternSubtype(_) => true,
        Type::Nullable(inner) => overload_eligible_type(inner),
        _ => false,
    }
}

pub(super) fn resolve_operator_overload(
    method: &str,
    operands: &[Type],
    fn_signatures: &HashMap<String, FnSignature>,
) -> Result<Option<(String, Type)>, Diagnostic> {
    let Some(receiver_ty) = operands.first() else {
        return Ok(None);
    };
    let exact_receiver_name = match receiver_ty {
        Type::Opaque { name, .. } => Some(method_signature_name(name, method)),
        _ => None,
    };
    let suffix = format!(".{method}");
    let mut matches = fn_signatures
        .iter()
        .filter_map(|(method_name, signature)| {
            if !method_name.ends_with(&suffix) {
                return None;
            }
            let FnSignature::Mono {
                params,
                return_type,
                ..
            } = signature
            else {
                return None;
            };
            if params.len() != operands.len() {
                return None;
            }
            if !params
                .iter()
                .zip(operands.iter())
                .all(|(expected, actual)| method_receiver_matches(expected, actual))
            {
                return None;
            }
            let score = if exact_receiver_name.as_deref() == Some(method_name.as_str()) {
                0
            } else {
                1
            };
            Some((score, method_name.clone(), return_type.clone()))
        })
        .collect::<Vec<_>>();

    if matches.is_empty() {
        return Ok(None);
    }
    matches.sort_by_key(|(score, name, _)| (*score, name.clone()));
    let best_score = matches[0].0;
    let mut best = matches
        .into_iter()
        .filter(|(score, _, _)| *score == best_score)
        .collect::<Vec<_>>();
    if best.len() > 1 {
        let mut names = best
            .iter()
            .map(|(_, name, _)| name.as_str())
            .collect::<Vec<_>>();
        names.sort_unstable();
        return Err(Diagnostic::new(format!(
            "ambiguous overload for operator method '{method}': {}",
            names.join(", ")
        )));
    }
    let (_, name, return_type) = best.remove(0);
    Ok(Some((name, return_type)))
}

pub(super) fn resolved_type_method_name(
    receiver_ty: &Type,
    name: &str,
    fn_signatures: &HashMap<String, FnSignature>,
) -> Option<String> {
    if let Type::Opaque {
        name: type_name, ..
    } = receiver_ty
    {
        let method_name = method_signature_name(type_name, name);
        if fn_signatures.contains_key(&method_name) {
            return Some(method_name);
        }
    }

    let suffix = format!(".{name}");
    let mut matches = fn_signatures
        .iter()
        .filter_map(|(method_name, signature)| {
            if !method_name.ends_with(&suffix) {
                return None;
            }
            let receiver_matches = match signature {
                FnSignature::Mono { params, .. } => params
                    .first()
                    .is_some_and(|param| method_receiver_matches(param, receiver_ty)),
                FnSignature::Overloaded(variants) => variants.iter().any(|variant| {
                    variant
                        .params
                        .first()
                        .is_some_and(|param| method_receiver_matches(param, receiver_ty))
                }),
                FnSignature::Generic(_) | FnSignature::Const { .. } => false,
            };
            receiver_matches.then(|| method_name.clone())
        })
        .collect::<Vec<_>>();
    (matches.len() == 1).then(|| matches.remove(0))
}

/// Resolve a method call on `receiver_ty` to a directly-callable signature
/// name, selecting the matching overload variant when the method is
/// overloaded. Returns the unique internal name IR lowering should call.
pub(super) fn resolved_type_method_call_name(
    receiver_ty: &Type,
    name: &str,
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
) -> Result<Option<String>, Diagnostic> {
    let Some(method_name) = resolved_type_method_name(receiver_ty, name, fn_signatures) else {
        return Ok(None);
    };
    match fn_signatures.get(&method_name) {
        Some(FnSignature::Overloaded(variants)) => {
            let chosen = select_overload(
                &method_name,
                variants,
                Some(receiver_ty),
                args,
                vars,
                fn_signatures,
                active_type_params,
            )?;
            Ok(Some(chosen.name.clone()))
        }
        _ => Ok(Some(method_name)),
    }
}

/// Pick the declared-import overload that best matches a call's arguments.
///
/// Resolution rules, in order:
/// 1. Keep variants whose arity matches the call (for method calls the
///    receiver occupies the first parameter and must be compatible).
/// 2. Reject variants where some argument neither equals nor coerces to the
///    parameter type. Each coercion (implicit widening, literal adoption)
///    costs 1; exact type matches cost 0.
/// 3. The variant with the lowest total cost wins. A tie between distinct
///    variants is an ambiguity error; an empty candidate set is a no-match
///    error. Both list the available overload signatures.
pub(super) fn select_overload<'a>(
    display_name: &str,
    variants: &'a [OverloadVariant],
    receiver_ty: Option<&Type>,
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
) -> Result<&'a OverloadVariant, Diagnostic> {
    let receiver_len = usize::from(receiver_ty.is_some());
    let total_args = args.len() + receiver_len;
    let overload_list = || {
        variants
            .iter()
            .map(OverloadVariant::params_display)
            .collect::<Vec<_>>()
            .join(", ")
    };

    let arity_matches = variants
        .iter()
        .filter(|variant| {
            call_arity_matches(&variant.params, total_args)
                && receiver_ty.is_none_or(|receiver| {
                    variant
                        .params
                        .first()
                        .is_some_and(|param| method_receiver_matches(param, receiver))
                })
        })
        .collect::<Vec<_>>();
    if arity_matches.is_empty() {
        return Err(Diagnostic::new(format!(
            "no overload of '{display_name}' accepts {total_args} argument{}; available overloads: {}",
            if total_args == 1 { "" } else { "s" },
            overload_list()
        )));
    }

    let mut viable: Vec<(&OverloadVariant, u32)> = Vec::new();
    for variant in arity_matches {
        // Prefer an exact-arity overload over one that requires omitted
        // nullable parameters to be filled with nil.
        let mut cost = (variant.params.len() - total_args) as u32;
        let mut candidate_viable = true;
        for (arg, param) in args.iter().zip(variant.params.iter().skip(receiver_len)) {
            let exact = infer_expr(arg, vars, fn_signatures, active_type_params, None)
                .is_ok_and(|actual| actual == *param);
            if exact {
                continue;
            }
            if infer_expr(
                arg,
                vars,
                fn_signatures,
                active_type_params,
                Some(param.clone()),
            )
            .is_ok()
            {
                cost += 1;
            } else {
                candidate_viable = false;
                break;
            }
        }
        if candidate_viable {
            viable.push((variant, cost));
        }
    }

    if viable.is_empty() {
        let arg_types = args
            .iter()
            .map(|arg| {
                infer_expr(arg, vars, fn_signatures, active_type_params, None)
                    .map_or_else(|_| "?".to_string(), |ty| ty.to_string())
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Diagnostic::new(format!(
            "no overload of '{display_name}' matches argument types ({arg_types}); available overloads: {}",
            overload_list()
        )));
    }
    let best_cost = viable
        .iter()
        .map(|(_, cost)| *cost)
        .min()
        .expect("viable is non-empty");
    let mut best = viable
        .into_iter()
        .filter(|(_, cost)| *cost == best_cost)
        .map(|(variant, _)| variant)
        .collect::<Vec<_>>();
    if best.len() > 1 {
        let candidates = best
            .iter()
            .map(|variant| variant.params_display())
            .collect::<Vec<_>>()
            .join(" and ");
        return Err(Diagnostic::new(format!(
            "ambiguous call to overloaded function '{display_name}': candidates {candidates} match equally well"
        )));
    }
    Ok(best.remove(0))
}

fn type_property_getter_signature<'a>(
    receiver_ty: &Type,
    name: &str,
    fn_signatures: &'a HashMap<String, FnSignature>,
) -> Option<(&'a FnSignature, String)> {
    let getter_name = resolved_type_property_getter_name(receiver_ty, name, fn_signatures)?;
    let signature = fn_signatures.get(&getter_name)?;
    Some((signature, getter_name))
}

pub(super) fn resolved_type_property_getter_name(
    receiver_ty: &Type,
    name: &str,
    fn_signatures: &HashMap<String, FnSignature>,
) -> Option<String> {
    if let Type::Opaque {
        name: type_name, ..
    } = receiver_ty
    {
        let getter_name = property_getter_name(type_name, name);
        if fn_signatures.contains_key(&getter_name) {
            return Some(getter_name);
        }
    }

    let suffix = format!(".get/{name}");
    let mut matches = fn_signatures
        .iter()
        .filter_map(|(getter_name, signature)| {
            if !getter_name.ends_with(&suffix) {
                return None;
            }
            let FnSignature::Mono { params, .. } = signature else {
                return None;
            };
            let receiver_param = params.first()?;
            (params.len() == 1 && method_receiver_matches(receiver_param, receiver_ty))
                .then_some(getter_name.clone())
        })
        .collect::<Vec<_>>();
    (matches.len() == 1).then(|| matches.remove(0))
}

fn attach_expr_span(error: Diagnostic, expr: &Expr) -> Diagnostic {
    match (error.span(), expr.span()) {
        (None, Some(span)) => error.with_span(span),
        _ => error,
    }
}

pub(super) fn infer_expr(
    expr: &Expr,
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Result<Type, Diagnostic> {
    infer_expr_inner(expr, vars, fn_signatures, active_type_params, expected)
        .map_err(|error| attach_expr_span(error, expr))
}

fn infer_expr_inner(
    expr: &Expr,
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Result<Type, Diagnostic> {
    match expr {
        Expr::Number(value, _) => resolve_number_literal(value, expected),
        Expr::Bool(..) => Ok(Type::Bool),
        Expr::Nil(..) => coerce_type(Type::Nil, expected),
        Expr::IsVariant { expr, tag, .. } => {
            let actual = infer_expr(expr, vars, fn_signatures, active_type_params, None)?;
            if actual.tagged_variant(tag).is_none() {
                return Err(Diagnostic::new(format!(
                    "type {actual} has no tagged variant '{tag}'"
                )));
            }
            Ok(Type::Bool)
        }
        Expr::String(..) => coerce_type(Type::String, expected),
        Expr::Bytes(..) => coerce_type(Type::Bytes, expected),
        Expr::Vararg(..) => coerce_type(Type::Array(Box::new(Type::Unknown)), expected),
        Expr::Require(path, _) => Err(Diagnostic::new(format!(
            "require(\"{path}\") can only be resolved when compiling from a file; \
             relative imports are unavailable when compiling a single source string"
        ))),
        Expr::Name(name, _, _) => {
            if matches!(fn_signatures.get(name), Some(FnSignature::Generic(_))) {
                return Err(generic_diagnostic(
                    "generic/uninstantiated-value",
                    format!(
                        "generic function '{name}' cannot be used as a value without type arguments"
                    ),
                    "call the generic function with explicit type arguments, e.g. id<i32>(value)",
                ));
            }
            if vars.get(name).is_none()
                && matches!(fn_signatures.get(name), Some(FnSignature::Overloaded(_)))
            {
                return Err(Diagnostic::new(format!(
                    "overloaded host function '{name}' cannot be used as a value; call it directly"
                )));
            }
            let actual = if let Some(local) = vars.get(name) {
                local.ty.clone()
            } else if let Some(FnSignature::Mono {
                params,
                vararg,
                return_type,
            }) = fn_signatures.get(name)
            {
                Type::Function {
                    params: if *vararg {
                        params
                            .iter()
                            .cloned()
                            .chain(std::iter::once(Type::Array(Box::new(Type::Unknown))))
                            .collect()
                    } else {
                        params.clone()
                    },
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
                if let Some(method) = unary_operator_method(op) {
                    if let Some((_, return_type)) = resolve_operator_overload(
                        method,
                        std::slice::from_ref(&actual),
                        fn_signatures,
                    )? {
                        return coerce_type(return_type, expected);
                    }
                }
                if overload_eligible_type(&actual) {
                    return Err(Diagnostic::new(format!(
                        "unary '-' is not defined for {actual}"
                    )));
                }
                match actual {
                    Type::Numeric(_) => coerce_type(actual, expected),
                    Type::Bool => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                    Type::Unit => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                    Type::String => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                    Type::Bytes => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                    Type::Extern | Type::ExternSubtype(_) => {
                        Err(Diagnostic::new("unary '-' requires a numeric operand"))
                    }
                    Type::Nil | Type::Nullable(_) => {
                        Err(Diagnostic::new("unary '-' requires a numeric operand"))
                    }
                    Type::Named { .. } => {
                        Err(Diagnostic::new("unary '-' requires a numeric operand"))
                    }
                    Type::Opaque { .. } => {
                        Err(Diagnostic::new("unary '-' requires a numeric operand"))
                    }
                    Type::Array(_) | Type::TypedArray(_) => {
                        Err(Diagnostic::new("unary '-' requires a numeric operand"))
                    }
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
                // `#` on unknown dispatches over the module's array types at
                // runtime (traps for non-arrays).
                if actual == Type::String || actual == Type::Bytes || actual == Type::Unknown {
                    return coerce_type(Type::Numeric(NumericType::I32), expected);
                }
                if !actual.is_array() && !matches!(actual, Type::TypedArray(_)) {
                    return Err(Diagnostic::new(
                        "# requires a string, array, or bytes operand",
                    ));
                }
                coerce_type(Type::Numeric(NumericType::I32), expected)
            }
        },
        Expr::Cast { expr, ty, .. } => {
            // The unconstrained probe may itself fail (e.g. `{}` needs
            // context); fall through to inference against the cast target.
            let numeric_cast = infer_expr(expr, vars, fn_signatures, active_type_params, None)
                .is_ok_and(|actual| require_numeric_cast(actual, ty.clone()).is_ok());
            if !numeric_cast {
                infer_expr(
                    expr,
                    vars,
                    fn_signatures,
                    active_type_params,
                    Some(ty.clone()),
                )?;
            }
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
            // Tagged-union constructor: Tag(expr) when Tag is a variant in the expected type.
            // The inferred type is the full expected type (union or variant) so that
            // Stmt::Let's exact-equality check passes and the variable carries the union type.
            if let (Expr::Name(tag, _, _), [arg]) = (callee.as_ref(), args.as_slice()) {
                if !fn_signatures.contains_key(tag.as_str()) && !vars.contains_key(tag.as_str()) {
                    if let Some(variant) = expected.as_ref().and_then(|e| e.tagged_variant(tag)) {
                        let payload_ty = *variant.payload.clone();
                        let arg_ty = infer_expr(
                            arg,
                            vars,
                            fn_signatures,
                            active_type_params,
                            Some(payload_ty.clone()),
                        )?;
                        coerce_type(arg_ty, Some(payload_ty))?;
                        return Ok(expected.clone().expect("variant lookup requires Some"));
                    }
                }
            }
            if let Some(name) = builtin_name(callee.as_ref()) {
                if let Some(result) = infer_promise_builtin_call(
                    &name,
                    args,
                    vars,
                    fn_signatures,
                    active_type_params,
                    expected.clone(),
                ) {
                    return result;
                }
                if let Some(result) = infer_pcall_builtin_call(
                    &name,
                    args,
                    vars,
                    fn_signatures,
                    active_type_params,
                    expected.clone(),
                ) {
                    return result;
                }
                if let Some(result) = infer_error_builtin_call(
                    &name,
                    args,
                    vars,
                    fn_signatures,
                    active_type_params,
                    expected.clone(),
                ) {
                    return result;
                }
                if let Some(result) = infer_bit32_builtin_call(
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
                if let Some(result) = infer_select_builtin_call(
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
                if let Some(result) = infer_type_builtin_call(
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
                if let Some(result) = infer_tonumber_builtin_call(
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
                if let Some(result) = infer_table_builtin_call(
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
                if let Some(result) = super::builtins::infer_typed_array_builtin_call(
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
                if let Some(result) = infer_string_builtin_call(
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
            // Note: print builtin is now handled via extern function declaration
            if let Expr::Name(name, _, _) = callee.as_ref() {
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
                if let Expr::Name(base_name, _, _) = base.as_ref() {
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
            if let Expr::Name(name, _, _) = callee.as_ref() {
                if let Some(FnSignature::Mono {
                    params,
                    vararg: true,
                    return_type,
                }) = fn_signatures.get(name)
                {
                    if args.len() > 1 {
                        for arg in &args[..args.len() - 1] {
                            if matches!(arg, Expr::Vararg(_)) {
                                return Err(Diagnostic::new(
                                    "'...' is only supported as the last argument of a call",
                                ));
                            }
                        }
                    }
                    // A trailing `...` forwards the caller's varargs, which can
                    // cover any number of missing fixed parameters at runtime.
                    let trailing_vararg = matches!(args.last(), Some(Expr::Vararg(_)));
                    let explicit = if trailing_vararg {
                        &args[..args.len() - 1]
                    } else {
                        &args[..]
                    };
                    if !trailing_vararg && explicit.len() < required_param_count(params) {
                        return Err(Diagnostic::new(format!(
                            "function expects at least {} arguments, got {}",
                            required_param_count(params),
                            explicit.len()
                        )));
                    }
                    for (arg, expected_param) in explicit.iter().zip(params.iter()) {
                        let actual = infer_expr(
                            arg,
                            vars,
                            fn_signatures,
                            active_type_params,
                            Some(expected_param.clone()),
                        )?;
                        if coerce_type(actual.clone(), Some(expected_param.clone())).is_err() {
                            return Err(Diagnostic::new(format!(
                                "call expected {}, got {}",
                                expected_param, actual
                            )));
                        }
                    }
                    for arg in explicit.iter().skip(params.len()) {
                        let _ = infer_expr(
                            arg,
                            vars,
                            fn_signatures,
                            active_type_params,
                            Some(Type::Unknown),
                        )?;
                    }
                    return coerce_type(return_type.clone(), expected);
                }
            }
            if let Expr::Name(name, _, _) = callee.as_ref() {
                if vars.get(name).is_none() {
                    if let Some(FnSignature::Overloaded(variants)) = fn_signatures.get(name) {
                        let chosen = select_overload(
                            name,
                            variants,
                            None,
                            args,
                            vars,
                            fn_signatures,
                            active_type_params,
                        )?;
                        for (arg, expected_param) in args.iter().zip(chosen.params.iter()) {
                            let actual = infer_expr(
                                arg,
                                vars,
                                fn_signatures,
                                active_type_params,
                                Some(expected_param.clone()),
                            )?;
                            if coerce_type(actual.clone(), Some(expected_param.clone())).is_err() {
                                return Err(Diagnostic::new(format!(
                                    "call expected {expected_param}, got {actual}"
                                )));
                            }
                        }
                        return coerce_type(chosen.return_type.clone(), expected);
                    }
                }
            }
            // Namespaced declared imports called through a field expression
            // (`math.abs(x)`): resolve the dotted name against the declared
            // function signatures when the namespace is not a local binding.
            if let Expr::Field { base, name, .. } = callee.as_ref() {
                if let Expr::Name(base_name, _, _) = base.as_ref() {
                    if !vars.contains_key(base_name) {
                        let dotted = method_signature_name(base_name, name);
                        match fn_signatures.get(&dotted) {
                            Some(FnSignature::Overloaded(variants)) => {
                                let chosen = select_overload(
                                    &dotted,
                                    variants,
                                    None,
                                    args,
                                    vars,
                                    fn_signatures,
                                    active_type_params,
                                )?;
                                for (arg, expected_param) in args.iter().zip(chosen.params.iter()) {
                                    let actual = infer_expr(
                                        arg,
                                        vars,
                                        fn_signatures,
                                        active_type_params,
                                        Some(expected_param.clone()),
                                    )?;
                                    if coerce_type(actual.clone(), Some(expected_param.clone()))
                                        .is_err()
                                    {
                                        return Err(Diagnostic::new(format!(
                                            "call expected {expected_param}, got {actual}"
                                        )));
                                    }
                                }
                                return coerce_type(chosen.return_type.clone(), expected);
                            }
                            Some(FnSignature::Mono {
                                params,
                                vararg: false,
                                return_type,
                            }) => {
                                if !call_arity_matches(params, args.len()) {
                                    return Err(Diagnostic::new(format!(
                                        "{dotted} expects {} argument{}, got {}",
                                        params.len(),
                                        if params.len() == 1 { "" } else { "s" },
                                        args.len()
                                    )));
                                }
                                for (arg, expected_param) in args.iter().zip(params.iter()) {
                                    let actual = infer_expr(
                                        arg,
                                        vars,
                                        fn_signatures,
                                        active_type_params,
                                        Some(expected_param.clone()),
                                    )?;
                                    if coerce_type(actual.clone(), Some(expected_param.clone()))
                                        .is_err()
                                    {
                                        return Err(Diagnostic::new(format!(
                                            "call expected {expected_param}, got {actual}"
                                        )));
                                    }
                                }
                                return coerce_type(return_type.clone(), expected);
                            }
                            _ => {}
                        }
                    }
                }
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
            if !call_arity_matches(&params, actual_args.len()) {
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
            if receiver_ty == Type::String {
                let mut call_args = Vec::with_capacity(args.len() + 1);
                call_args.push((**receiver).clone());
                call_args.extend_from_slice(args);
                let builtin = match name.as_str() {
                    "find" => Some(STRING_FIND),
                    "match" => Some(STRING_MATCH),
                    "gmatch" => Some(STRING_GMATCH),
                    "gsub" => Some(STRING_GSUB),
                    "len" => Some(STRING_LEN),
                    "sub" => Some(STRING_SUB),
                    "rep" => Some(STRING_REP),
                    "byte" => Some(STRING_BYTE),
                    "upper" => Some(STRING_UPPER),
                    "lower" => Some(STRING_LOWER),
                    "format" => Some(STRING_FORMAT),
                    "reverse" => Some(STRING_REVERSE),
                    _ => None,
                };
                if let Some(builtin) = builtin {
                    if let Some(result) = infer_string_builtin_call(
                        builtin,
                        &call_args,
                        vars,
                        fn_signatures,
                        active_type_params,
                        expected.clone(),
                    ) {
                        return result;
                    }
                }
            }
            if let Some(result) = infer_promise_await_method_call(
                receiver,
                name,
                args,
                vars,
                fn_signatures,
                active_type_params,
                expected.clone(),
            ) {
                return result;
            }
            let (params, ret) = if let Some((signature, signature_name)) =
                method_signature(receiver, name, fn_signatures)
                    .or_else(|| type_method_signature(&receiver_ty, name, fn_signatures))
            {
                match signature {
                    FnSignature::Mono {
                        params,
                        return_type,
                        ..
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
                    FnSignature::Overloaded(variants) => {
                        if !type_args.is_empty() {
                            return Err(generic_diagnostic(
                                "generic/extra-type-args",
                                "type arguments are only allowed when calling a generic method",
                                "remove the type argument list or call a generic method",
                            ));
                        }
                        let chosen = select_overload(
                            &signature_name,
                            variants,
                            Some(&receiver_ty),
                            args,
                            vars,
                            fn_signatures,
                            active_type_params,
                        )?;
                        (chosen.params.clone(), chosen.return_type.clone())
                    }
                    FnSignature::Const { .. } => {
                        return Err(Diagnostic::new(format!(
                            "'{signature_name}' is a constant, not a method"
                        )));
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
            if !call_arity_matches(&params, actual_args.len() + 1) {
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
            let expected_fields = expected.as_ref().and_then(record_fields).cloned();
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
            if let Expr::Name(base_name, _, _) = base.as_ref() {
                let method_name = method_signature_name(base_name, name);
                if let Some(signature) = fn_signatures.get(&method_name) {
                    return match signature {
                        FnSignature::Mono {
                            params,
                            return_type,
                            ..
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
                        FnSignature::Overloaded(_) => Err(Diagnostic::new(format!(
                            "overloaded host function '{method_name}' cannot be used as a value; call it directly"
                        ))),
                        FnSignature::Const { ty } => coerce_type(ty.clone(), expected),
                    };
                }
            }
            let base_ty = infer_expr(base, vars, fn_signatures, active_type_params, None)?;
            if let Some((signature, _)) =
                type_property_getter_signature(&base_ty, name, fn_signatures)
            {
                let FnSignature::Mono {
                    params,
                    return_type,
                    ..
                } = signature
                else {
                    return Err(generic_diagnostic(
                        "generic-property/getter",
                        format!("generic property getter for '{name}' is not supported"),
                        "declare properties with concrete types",
                    ));
                };
                if params.len() == 1 && method_receiver_matches(&params[0], &base_ty) {
                    return coerce_type(return_type.clone(), expected);
                }
            }
            if matches!(base_ty, Type::TaggedUnion(_)) {
                return Err(Diagnostic::new(format!(
                    "field access on tagged union requires narrowing before reading '{name}'"
                )));
            }
            // `.n` on an array reads its length, so `table.pack(...).n`
            // behaves like Lua's packed-table count field.
            if matches!(base_ty, Type::Array(_)) && name == "n" {
                return coerce_type(Type::Numeric(NumericType::I32), expected);
            }
            let Some(field_ty) = base_ty.record_field(name) else {
                if matches!(
                    base_ty,
                    Type::Record(_) | Type::Opaque { .. } | Type::TaggedVariant(_)
                ) {
                    return Err(Diagnostic::new(format!("unknown record field '{name}'")));
                }
                return Err(Diagnostic::new("field access requires a record base"));
            };
            coerce_type(field_ty, expected)
        }
        Expr::Index { base, index, .. } => {
            let base_ty = infer_expr(base, vars, fn_signatures, active_type_params, None)?;
            let element_ty = if base_ty == Type::Bytes {
                Type::Numeric(NumericType::I32)
            } else if base_ty == Type::Unknown {
                // Indexing an unknown value dispatches over the module's
                // array types at runtime; the element comes back boxed.
                Type::Unknown
            } else {
                base_ty
                    .element_type()
                    .ok_or_else(|| Diagnostic::new("indexing requires an array or bytes operand"))?
            };
            // Any numeric index is accepted and converted to i32 at lowering
            // (f64 loop variables index arrays, as in Lua).
            let index_ty = infer_expr(index, vars, fn_signatures, active_type_params, None)
                .map(first_of_multi)
                .or_else(|_| {
                    infer_expr(
                        index,
                        vars,
                        fn_signatures,
                        active_type_params,
                        Some(Type::Numeric(NumericType::I32)),
                    )
                })?;
            if !index_ty.is_numeric() && index_ty != Type::Unknown {
                return Err(Diagnostic::new("array index must be numeric"));
            }
            coerce_type(element_ty, expected)
        }
        Expr::Binary {
            op, left, right, ..
        } => match op {
            BinaryOp::Concat => {
                let mut left_ty = first_of_multi(infer_expr(
                    left,
                    vars,
                    fn_signatures,
                    active_type_params,
                    None,
                )?);
                if left_ty == Type::Unknown
                    || matches!(left_ty, Type::Nullable(ref inner) if **inner == Type::String)
                {
                    left_ty = Type::String;
                }
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
                let left_ty = infer_expr(left, vars, fn_signatures, active_type_params, None)?;
                let right_ty = infer_expr(right, vars, fn_signatures, active_type_params, None)?;
                if !left_ty.is_numeric() || !right_ty.is_numeric() {
                    if let Some(method) = binary_operator_method(op) {
                        if let Some((_, return_type)) = resolve_operator_overload(
                            method,
                            &[left_ty.clone(), right_ty.clone()],
                            fn_signatures,
                        )? {
                            return coerce_type(return_type, expected);
                        }
                    }
                    if overload_eligible_type(&left_ty) || overload_eligible_type(&right_ty) {
                        let symbol = binary_operator_symbol(op).unwrap_or("?");
                        return Err(Diagnostic::new(format!(
                            "operator '{symbol}' is not defined for {left_ty} and {right_ty}"
                        )));
                    }
                }
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
            BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::FloorDiv
            | BinaryOp::Mod
            | BinaryOp::Pow => {
                if let Some(method) = binary_operator_method(op) {
                    let left_ty = infer_expr(left, vars, fn_signatures, active_type_params, None)?;
                    let right_ty =
                        infer_expr(right, vars, fn_signatures, active_type_params, None)?;
                    if !left_ty.is_numeric() || !right_ty.is_numeric() {
                        if let Some((_, return_type)) = resolve_operator_overload(
                            method,
                            &[left_ty.clone(), right_ty.clone()],
                            fn_signatures,
                        )? {
                            return coerce_type(return_type, expected);
                        }
                        if overload_eligible_type(&left_ty) || overload_eligible_type(&right_ty) {
                            let symbol = binary_operator_symbol(op).unwrap_or("?");
                            return Err(Diagnostic::new(format!(
                                "operator '{symbol}' is not defined for {left_ty} and {right_ty}"
                            )));
                        }
                    }
                }
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
            BinaryOp::Less | BinaryOp::LessEq | BinaryOp::Greater | BinaryOp::GreaterEq => {
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
            BinaryOp::Eq | BinaryOp::NotEq => {
                if is_nil_comparison(left, right) {
                    let value = if matches!(left.as_ref(), Expr::Nil(..)) {
                        right
                    } else {
                        left
                    };
                    let value_ty = first_of_multi(infer_expr(
                        value,
                        vars,
                        fn_signatures,
                        active_type_params,
                        None,
                    )?);
                    if matches!(value_ty, Type::Nullable(_)) {
                        return Ok(Type::Bool);
                    }
                    return Err(Diagnostic::new(
                        "nil comparison requires a nullable operand",
                    ));
                }
                let left_ty = first_of_multi(infer_expr(
                    left,
                    vars,
                    fn_signatures,
                    active_type_params,
                    None,
                )?);
                if matches!(left_ty, Type::Nullable(_)) {
                    let _ = infer_expr(
                        right,
                        vars,
                        fn_signatures,
                        active_type_params,
                        Some(left_ty),
                    )?;
                    return Ok(Type::Bool);
                }
                if let Ok(right_probe) =
                    infer_expr(right, vars, fn_signatures, active_type_params, None)
                        .map(first_of_multi)
                {
                    if matches!(right_probe, Type::Nullable(_)) {
                        let _ = infer_expr(
                            left,
                            vars,
                            fn_signatures,
                            active_type_params,
                            Some(right_probe),
                        )?;
                        return Ok(Type::Bool);
                    }
                }
                // An unknown operand compares with Lua dynamic equality; the
                // other side is boxed into unknown as needed.
                if left_ty == Type::Unknown {
                    let _ = infer_expr(
                        right,
                        vars,
                        fn_signatures,
                        active_type_params,
                        Some(Type::Unknown),
                    )?;
                    return Ok(Type::Bool);
                }
                if let Ok(right_probe) =
                    infer_expr(right, vars, fn_signatures, active_type_params, None)
                        .map(first_of_multi)
                {
                    if right_probe == Type::Unknown {
                        let _ = infer_expr(
                            left,
                            vars,
                            fn_signatures,
                            active_type_params,
                            Some(Type::Unknown),
                        )?;
                        return Ok(Type::Bool);
                    }
                }
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
                } else if matches!(left_ty, Type::TypedArray(_)) {
                    // Typed arrays compare by identity (same allocation).
                    let right_ty = infer_expr(
                        right,
                        vars,
                        fn_signatures,
                        active_type_params,
                        Some(left_ty.clone()),
                    )?;
                    if right_ty != left_ty {
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

/// Lua multi-value adjustment for a type in a single-value context.
pub(super) fn first_of_multi(ty: Type) -> Type {
    match ty {
        Type::Multi(mut parts) if !parts.is_empty() => parts.remove(0),
        other => other,
    }
}

fn is_nil_comparison(left: &Expr, right: &Expr) -> bool {
    matches!(left, Expr::Nil(..)) || matches!(right, Expr::Nil(..))
}

pub(super) fn infer_expr_list(
    exprs: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<&[Type]>,
) -> Result<Vec<Type>, Diagnostic> {
    let mut out = Vec::new();
    let mut out_exprs = Vec::new();
    for expr in exprs {
        let remaining_expected = expected
            .and_then(|types| (!types[out.len()..].is_empty()).then(|| &types[out.len()..]));
        let next_expected = remaining_expected.and_then(|types| types.first().cloned());
        let ty = if let Expr::Call { callee, .. } = expr {
            let expands_multi = builtin_name(callee.as_ref()).is_some_and(|name| {
                name == "coroutine.resume"
                    || name == "pcall"
                    || name == STRING_FIND
                    || name == STRING_MATCH
                    || name == STRING_GSUB
                    || name == STRING_BYTE
            });
            // A Name not found in fn_signatures or vars may be a tagged-union constructor
            // (e.g. `Num(42)`). Pass next_expected so the constructor intercept in
            // infer_expr can see the expected union type and fire correctly.
            let is_potential_constructor = matches!(callee.as_ref(), Expr::Name(tag, _, _)
                    if !fn_signatures.contains_key(tag.as_str())
                        && !vars.contains_key(tag.as_str()));
            let call_expected = if expands_multi {
                // coroutine.resume/pcall return Multi; pass the full expected slice so the
                // return-type inference captures all slots rather than just the first.
                remaining_expected.map(|types| Type::Multi(types.to_vec()))
            } else if is_potential_constructor {
                next_expected
            } else {
                None
            };
            infer_expr(expr, vars, fn_signatures, active_type_params, call_expected)?
        } else if matches!(expr, Expr::MethodCall { .. }) {
            infer_expr(expr, vars, fn_signatures, active_type_params, None)?
        } else {
            infer_expr(expr, vars, fn_signatures, active_type_params, next_expected)?
        };
        match ty {
            Type::Multi(types) => {
                out_exprs.extend(std::iter::repeat_n(expr, types.len()));
                out.extend(types);
            }
            other => {
                out_exprs.push(expr);
                out.push(other);
            }
        }
    }
    if let Some(expected_types) = expected {
        for (index, ty) in out.clone().into_iter().enumerate() {
            if let Some(expected_ty) = expected_types.get(index) {
                out[index] = coerce_type(ty, Some(expected_ty.clone()))
                    .map_err(|error| attach_expr_span(error, out_exprs[index]))?;
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
    // A typed-array literal (`{1, 2, 3}::Float32Array` or an annotated let)
    // allocates in linear memory. Elements accept any numeric type: the store
    // converts to the element type like an explicit cast, mirroring JS typed
    // arrays where every write coerces the number.
    if let Some(Type::TypedArray(kind)) = expected.as_ref() {
        let element_ty = Type::Numeric(kind.element_numeric_type());
        for element in elements {
            if matches!(element, Expr::Vararg(_)) {
                return Err(Diagnostic::new(
                    "'...' is not supported in typed-array literals",
                ));
            }
            let actual = infer_expr(
                element,
                vars,
                fn_signatures,
                active_type_params,
                Some(element_ty.clone()),
            )
            .or_else(|_| infer_expr(element, vars, fn_signatures, active_type_params, None))?;
            if !actual.is_numeric() {
                return Err(Diagnostic::new(format!(
                    "{} element must be numeric, got {actual}",
                    kind.type_name()
                )));
            }
        }
        return Ok(Type::TypedArray(*kind));
    }
    if elements.is_empty() {
        if let Some(element_ty) = expected.as_ref().and_then(Type::element_type) {
            return Ok(Type::Array(Box::new(element_ty)));
        }
        return Err(super::signatures::inference_diagnostic(
            "inference/missing-context",
            DiagnosticCategory::MissingContext,
            "empty array literal requires explicit element type",
            "add an explicit element type annotation, e.g. local xs: {i32} = {}",
        ));
    }

    if elements.len() > 1 {
        for element in &elements[..elements.len() - 1] {
            if matches!(element, Expr::Vararg(_)) {
                return Err(Diagnostic::new(
                    "'...' is only supported as the last element of a table constructor",
                ));
            }
        }
    }
    let expected_element = expected.as_ref().and_then(Type::element_type);
    // A trailing `...` splats the caller's varargs (unknown values) into the
    // array, and an expected `{unknown}` boxes every element, so both force
    // the element type to unknown.
    if matches!(elements.last(), Some(Expr::Vararg(_))) || expected_element == Some(Type::Unknown) {
        for element in elements {
            if matches!(element, Expr::Vararg(_)) {
                continue;
            }
            let _ = infer_expr(
                element,
                vars,
                fn_signatures,
                active_type_params,
                Some(Type::Unknown),
            )?;
        }
        return coerce_type(Type::Array(Box::new(Type::Unknown)), expected);
    }
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
        // Heterogeneous literals without a concrete expected element type fall
        // back to `{unknown}` with boxed elements, matching Lua's untyped
        // sequences (e.g. `{ true, 1, 2, 42 }`).
        if element_ty != Type::Unknown {
            match infer_expr(
                element,
                vars,
                fn_signatures,
                active_type_params,
                Some(element_ty.clone()),
            )
            .and_then(|actual| common_element_type(element_ty.clone(), actual))
            {
                Ok(unified) => {
                    element_ty = unified;
                    continue;
                }
                Err(error) => {
                    if expected_element.is_some() {
                        return Err(error);
                    }
                    element_ty = Type::Unknown;
                }
            }
        }
        let _ = infer_expr(
            element,
            vars,
            fn_signatures,
            active_type_params,
            Some(Type::Unknown),
        )?;
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
    let return_ty = match function.return_type.clone() {
        Some(return_ty) => return_ty,
        // Non-generic function expressions (generic ones are rejected above) may
        // omit their return type; infer it from the body just like named
        // functions. This is what lets immediately-invoked function expressions
        // such as `(function() return 1 end)()` type-check.
        None => super::signatures::infer_function_expr_return_type(
            function,
            vars,
            fn_signatures,
            active_type_params,
        )?,
    };
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
    if !saw_return && return_ty != Type::Unit && !matches!(return_ty, Type::Nullable(_)) {
        return Err(Diagnostic::new("function expression is missing a return"));
    }
    coerce_type(function_ty, expected)
}
