use std::collections::{HashMap, HashSet};

use waluau_ast::{Expr, FunctionExpr, Type};
use waluau_diagnostics::{Diagnostic, DiagnosticCategory};

use super::{Binding, FnSignature, GenericScheme, coerce_type, infer_expr_list};

pub(super) fn inference_diagnostic(
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

pub(super) fn generic_diagnostic(
    code: &'static str,
    message: impl Into<String>,
    action: impl Into<String>,
) -> Diagnostic {
    Diagnostic::new(message)
        .with_code(code)
        .with_category(DiagnosticCategory::Unsupported)
        .with_action(action)
}

fn substitute_type(ty: &Type, subst: &HashMap<String, Type>) -> Type {
    match ty {
        Type::TypeParam(name) => subst
            .get(name)
            .cloned()
            .unwrap_or_else(|| Type::TypeParam(name.clone())),
        Type::Array(inner) => Type::Array(Box::new(substitute_type(inner, subst))),
        Type::Record(fields) => Type::Record(
            fields
                .iter()
                .map(|(name, ty)| (name.clone(), substitute_type(ty, subst)))
                .collect(),
        ),
        Type::Function {
            params,
            return_type,
        } => Type::Function {
            params: params
                .iter()
                .map(|param| substitute_type(param, subst))
                .collect(),
            return_type: Box::new(substitute_type(return_type, subst)),
        },
        other => other.clone(),
    }
}

pub(super) fn validate_type_param_list(
    type_params: &[String],
    outer_type_params: &HashSet<String>,
) -> Result<(), Diagnostic> {
    let mut seen = HashSet::new();
    for param in type_params {
        if !seen.insert(param.clone()) {
            return Err(generic_diagnostic(
                "generic/duplicate-type-param",
                format!("duplicate type parameter '{param}'"),
                "rename or remove the duplicate type parameter",
            ));
        }
        if outer_type_params.contains(param) {
            return Err(generic_diagnostic(
                "generic/shadowed-type-param",
                format!("type parameter '{param}' shadows an outer generic type parameter"),
                "choose a different type parameter name",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_type_in_scope(
    ty: &Type,
    allowed: &HashSet<String>,
) -> Result<(), Diagnostic> {
    match ty {
        Type::TypeParam(name) if !allowed.contains(name) => Err(generic_diagnostic(
            "generic/unknown-type-param",
            format!("unknown type parameter '{name}'"),
            "declare the type parameter on the enclosing generic function",
        )),
        Type::Array(inner) => validate_type_in_scope(inner, allowed),
        Type::Record(fields) => {
            for ty in fields.values() {
                validate_type_in_scope(ty, allowed)?;
            }
            Ok(())
        }
        Type::Function {
            params,
            return_type,
        } => {
            for param in params {
                validate_type_in_scope(param, allowed)?;
            }
            validate_type_in_scope(return_type, allowed)
        }
        _ => Ok(()),
    }
}

fn is_valid_type_argument(ty: &Type, active_type_params: &HashSet<String>) -> bool {
    match ty {
        Type::TypeParam(name) => active_type_params.contains(name),
        Type::Array(inner) => is_valid_type_argument(inner, active_type_params),
        Type::Record(fields) => fields
            .values()
            .all(|field| is_valid_type_argument(field, active_type_params)),
        Type::Function {
            params,
            return_type,
        } => {
            params
                .iter()
                .all(|param| is_valid_type_argument(param, active_type_params))
                && is_valid_type_argument(return_type, active_type_params)
        }
        _ => true,
    }
}

pub(super) fn active_type_param_set(type_params: &[String]) -> HashSet<String> {
    type_params.iter().cloned().collect()
}

pub(super) fn infer_generic_call(
    scheme: &GenericScheme,
    type_args: &[Type],
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Result<Type, Diagnostic> {
    if type_args.is_empty() {
        return Err(generic_diagnostic(
            "generic/missing-type-args",
            "generic function call requires explicit type arguments",
            "supply type arguments between the callee and argument list, e.g. id<i32>(value)",
        ));
    }
    if type_args.len() != scheme.type_params.len() {
        return Err(generic_diagnostic(
            "generic/type-arg-count",
            format!(
                "generic function expects {} type argument{}, got {}",
                scheme.type_params.len(),
                if scheme.type_params.len() == 1 {
                    ""
                } else {
                    "s"
                },
                type_args.len()
            ),
            "match the number of type parameters declared on the generic function",
        ));
    }
    for ty in type_args {
        if !is_valid_type_argument(ty, active_type_params) {
            return Err(generic_diagnostic(
                "generic/non-concrete-type-arg",
                format!("type argument '{ty}' is not a concrete type in this scope"),
                "use a concrete type or forward an in-scope type parameter",
            ));
        }
        validate_type_in_scope(ty, active_type_params)?;
    }
    let subst = scheme
        .type_params
        .iter()
        .cloned()
        .zip(type_args.iter().cloned())
        .collect::<HashMap<_, _>>();
    let params = scheme
        .params
        .iter()
        .map(|param| substitute_type(param, &subst))
        .collect::<Vec<_>>();
    let ret = substitute_type(&scheme.return_type, &subst);
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

pub(super) fn infer_generic_function_expr_call(
    function: &FunctionExpr,
    type_args: &[Type],
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Result<Type, Diagnostic> {
    let return_ty = function.return_type.clone().ok_or_else(|| {
        generic_diagnostic(
            "generic/missing-return-type",
            "generic function expression requires an explicit return type",
            "add a return type annotation to the generic function expression",
        )
    })?;
    let scheme = GenericScheme {
        type_params: function.type_params.clone(),
        params: function
            .params
            .iter()
            .map(|param| param.ty.clone())
            .collect(),
        return_type: return_ty,
    };
    infer_generic_call(
        &scheme,
        type_args,
        args,
        vars,
        fn_signatures,
        active_type_params,
        expected,
    )
}
