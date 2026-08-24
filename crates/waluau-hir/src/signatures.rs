use std::collections::{HashMap, HashSet};

use waluau_ast::{Expr, Function, FunctionExpr, Rebindability, Type};
use waluau_diagnostics::{Diagnostic, DiagnosticCategory};

use super::expressions::{infer_expr, infer_expr_list};
use super::numeric::coerce_type;
use super::statements::{collect_return_types, common_return_type, function_calls};
use super::{Binding, binding_for};

#[derive(Clone, Debug)]
pub(super) struct GenericScheme {
    pub(super) type_params: Vec<String>,
    pub(super) params: Vec<Type>,
    pub(super) return_type: Type,
}

/// One overload of a declared host function. `name` is the unique internal
/// name the overload was renamed to (`base$overloadN`); the matching
/// `FnSignature::Mono` entry is registered under that name.
#[derive(Clone, Debug)]
pub(super) struct OverloadVariant {
    pub(super) name: String,
    pub(super) params: Vec<Type>,
    pub(super) return_type: Type,
}

impl OverloadVariant {
    /// Human-readable parameter list for diagnostics, e.g. `(f32, f32)`.
    pub(super) fn params_display(&self) -> String {
        let params = self
            .params
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        format!("({params})")
    }
}

#[derive(Clone, Debug)]
pub(super) enum FnSignature {
    Mono {
        params: Vec<Type>,
        vararg: bool,
        return_type: Type,
    },
    Generic(GenericScheme),
    /// A set of declared host function overloads sharing one source-level
    /// name, registered under the base name. Calls select a variant from the
    /// argument types; each variant is also registered as a `Mono` signature
    /// under its unique internal name.
    Overloaded(Vec<OverloadVariant>),
    /// A named compile-time constant on a builtin namespace, e.g. `math.pi`
    /// (declared as `declare const math.pi: f64 = ...`). Reads type as `ty`;
    /// calls are rejected. Lowering folds reads to the declared literal.
    Const {
        ty: Type,
    },
}

/// The number of arguments a call must provide. Only a trailing suffix of
/// nullable parameters may be omitted; nullable parameters before a required
/// parameter still occupy a positional slot.
pub(super) fn required_param_count(params: &[Type]) -> usize {
    params
        .iter()
        .rposition(|param| !param.accepts_nil())
        .map_or(0, |index| index + 1)
}

pub(super) fn call_arity_matches(params: &[Type], actual: usize) -> bool {
    (required_param_count(params)..=params.len()).contains(&actual)
}

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
        Type::Named { name, type_args } => Type::Named {
            name: name.clone(),
            type_args: type_args
                .iter()
                .map(|arg| substitute_type(arg, subst))
                .collect(),
        },
        Type::TaggedVariant(variant) => Type::TaggedVariant(waluau_ast::TaggedVariant {
            tag: variant.tag.clone(),
            payload: Box::new(substitute_type(variant.payload.as_ref(), subst)),
        }),
        Type::TaggedUnion(variants) => Type::TaggedUnion(
            variants
                .iter()
                .map(|variant| waluau_ast::TaggedVariant {
                    tag: variant.tag.clone(),
                    payload: Box::new(substitute_type(variant.payload.as_ref(), subst)),
                })
                .collect(),
        ),
        Type::TypeParam(name) => subst
            .get(name)
            .cloned()
            .unwrap_or_else(|| Type::TypeParam(name.clone())),
        Type::Opaque {
            name,
            ty,
            generic_extern,
        } => {
            let generic_extern = generic_extern
                .as_ref()
                .map(|generic| Box::new(generic.map_type_args(|ty| substitute_type(ty, subst))));
            Type::Opaque {
                name: generic_extern
                    .as_ref()
                    .map_or_else(|| name.clone(), |generic| generic.canonical_name()),
                ty: Box::new(substitute_type(ty, subst)),
                generic_extern,
            }
        }
        Type::Nullable(inner) => Type::Nullable(Box::new(substitute_type(inner, subst))),
        Type::Readonly(inner) => Type::Readonly(Box::new(substitute_type(inner, subst))),
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
            has_self,
        } => Type::Function {
            params: params
                .iter()
                .map(|param| substitute_type(param, subst))
                .collect(),
            return_type: Box::new(substitute_type(return_type, subst)),
            has_self: *has_self,
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
        Type::Named { type_args, .. } => {
            for ty in type_args {
                validate_type_in_scope(ty, allowed)?;
            }
            Ok(())
        }
        Type::TaggedVariant(variant) => validate_type_in_scope(variant.payload.as_ref(), allowed),
        Type::TaggedUnion(variants) => {
            for variant in variants {
                validate_type_in_scope(variant.payload.as_ref(), allowed)?;
            }
            Ok(())
        }
        Type::Opaque {
            ty, generic_extern, ..
        } => {
            validate_type_in_scope(ty, allowed)?;
            if let Some(generic) = generic_extern {
                for arg in &generic.type_args {
                    validate_type_in_scope(arg, allowed)?;
                }
            }
            Ok(())
        }
        Type::Nullable(inner) => validate_type_in_scope(inner, allowed),
        Type::Readonly(inner) => validate_type_in_scope(inner, allowed),
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
            ..
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
        Type::Named { type_args, .. } => type_args
            .iter()
            .all(|arg| is_valid_type_argument(arg, active_type_params)),
        Type::TaggedVariant(variant) => {
            is_valid_type_argument(variant.payload.as_ref(), active_type_params)
        }
        Type::TaggedUnion(variants) => variants
            .iter()
            .all(|variant| is_valid_type_argument(variant.payload.as_ref(), active_type_params)),
        Type::TypeParam(name) => active_type_params.contains(name),
        Type::Opaque {
            ty, generic_extern, ..
        } => {
            is_valid_type_argument(ty, active_type_params)
                && generic_extern.as_ref().is_none_or(|generic| {
                    generic
                        .type_args
                        .iter()
                        .all(|arg| is_valid_type_argument(arg, active_type_params))
                })
        }
        Type::Nullable(inner) => is_valid_type_argument(inner, active_type_params),
        // A generic body checked against bare `T` can erase `T` into unknown;
        // the instantiated readonly capability would be invisible there.
        // Reject readonly type arguments until generic constraints can carry
        // the capability through the body.
        Type::Readonly(_) => false,
        Type::Array(inner) => is_valid_type_argument(inner, active_type_params),
        Type::Record(fields) => fields
            .values()
            .all(|field| is_valid_type_argument(field, active_type_params)),
        Type::Function {
            params,
            return_type,
            ..
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

fn contains_type_param(ty: &Type) -> bool {
    match ty {
        Type::Named { type_args, .. } => type_args.iter().any(contains_type_param),
        Type::TaggedVariant(variant) => contains_type_param(variant.payload.as_ref()),
        Type::TaggedUnion(variants) => variants
            .iter()
            .any(|variant| contains_type_param(variant.payload.as_ref())),
        Type::TypeParam(_) => true,
        Type::Opaque {
            ty, generic_extern, ..
        } => {
            contains_type_param(ty.as_ref())
                || generic_extern
                    .as_ref()
                    .is_some_and(|generic| generic.type_args.iter().any(contains_type_param))
        }
        Type::Nullable(inner) => contains_type_param(inner.as_ref()),
        Type::Readonly(inner) => contains_type_param(inner.as_ref()),
        Type::Array(inner) => contains_type_param(inner.as_ref()),
        Type::Record(fields) => fields.values().any(contains_type_param),
        Type::Function {
            params,
            return_type,
            ..
        } => params.iter().any(contains_type_param) || contains_type_param(return_type.as_ref()),
        _ => false,
    }
}

fn unify(
    param_ty: &Type,
    actual_ty: &Type,
    subst: &mut HashMap<String, Type>,
) -> Result<(), Diagnostic> {
    match (param_ty, actual_ty) {
        (Type::TypeParam(name), _) => {
            if let Some(existing) = subst.get(name) {
                match (existing, actual_ty) {
                    (Type::Numeric(left), Type::Numeric(right)) => {
                        if let Some(common) = left.common(*right) {
                            subst.insert(name.clone(), Type::Numeric(common));
                            Ok(())
                        } else {
                            Err(Diagnostic::new(format!(
                                "conflicting types inferred for type parameter '{name}': {existing} vs {actual_ty}"
                            )))
                        }
                    }
                    (left, right) if left == right => Ok(()),
                    _ => Err(Diagnostic::new(format!(
                        "conflicting types inferred for type parameter '{name}': {existing} vs {actual_ty}"
                    ))),
                }
            } else {
                subst.insert(name.clone(), actual_ty.clone());
                Ok(())
            }
        }
        (
            Type::Named {
                name: p_name,
                type_args: p_args,
            },
            Type::Named {
                name: a_name,
                type_args: a_args,
            },
        ) => {
            if p_name != a_name || p_args.len() != a_args.len() {
                return Err(Diagnostic::new(format!(
                    "cannot match type {param_ty} with {actual_ty}"
                )));
            }
            for (p_arg, a_arg) in p_args.iter().zip(a_args.iter()) {
                unify(p_arg, a_arg, subst)?;
            }
            Ok(())
        }
        (Type::TaggedVariant(p_var), Type::TaggedVariant(a_var)) => {
            if p_var.tag != a_var.tag {
                return Err(Diagnostic::new(format!(
                    "cannot match type {param_ty} with {actual_ty}"
                )));
            }
            unify(p_var.payload.as_ref(), a_var.payload.as_ref(), subst)
        }
        (Type::TaggedUnion(p_variants), Type::TaggedVariant(a_variant)) => {
            let Some(p_var) = p_variants.iter().find(|v| v.tag == a_variant.tag) else {
                return Err(Diagnostic::new(format!(
                    "cannot match type {param_ty} with {actual_ty}"
                )));
            };
            unify(p_var.payload.as_ref(), a_variant.payload.as_ref(), subst)
        }
        (Type::TaggedVariant(p_variant), Type::TaggedUnion(a_variants)) => {
            let Some(a_var) = a_variants.iter().find(|v| v.tag == p_variant.tag) else {
                return Err(Diagnostic::new(format!(
                    "cannot match type {param_ty} with {actual_ty}"
                )));
            };
            unify(p_variant.payload.as_ref(), a_var.payload.as_ref(), subst)
        }
        (Type::TaggedUnion(p_variants), Type::TaggedUnion(a_variants)) => {
            for a_var in a_variants {
                let Some(p_var) = p_variants.iter().find(|v| v.tag == a_var.tag) else {
                    return Err(Diagnostic::new(format!(
                        "cannot match type {param_ty} with {actual_ty}"
                    )));
                };
                unify(p_var.payload.as_ref(), a_var.payload.as_ref(), subst)?;
            }
            Ok(())
        }
        (
            Type::Opaque {
                name: p_name,
                ty: p_ty,
                generic_extern: p_generic,
            },
            Type::Opaque {
                name: a_name,
                ty: a_ty,
                generic_extern: a_generic,
            },
        ) => {
            match (p_generic, a_generic) {
                (Some(param), Some(actual))
                    if param.constructor == actual.constructor
                        && param.type_args.len() == actual.type_args.len() =>
                {
                    for (param_arg, actual_arg) in
                        param.type_args.iter().zip(actual.type_args.iter())
                    {
                        unify(param_arg, actual_arg, subst)?;
                    }
                }
                (None, None) if p_name == a_name => {}
                _ => {
                    return Err(Diagnostic::new(format!(
                        "cannot match type {param_ty} with {actual_ty}"
                    )));
                }
            }
            if p_generic.is_none() && p_name != a_name {
                return Err(Diagnostic::new(format!(
                    "cannot match type {param_ty} with {actual_ty}"
                )));
            }
            unify(p_ty.as_ref(), a_ty.as_ref(), subst)
        }
        (Type::Array(p_inner), Type::Array(a_inner)) => {
            unify(p_inner.as_ref(), a_inner.as_ref(), subst)
        }
        (Type::Nullable(p_inner), Type::Nullable(a_inner)) => {
            unify(p_inner.as_ref(), a_inner.as_ref(), subst)
        }
        (Type::Readonly(p_inner), actual) if actual.readonly_base().is_some() => unify(
            p_inner.as_ref(),
            actual
                .readonly_base()
                .expect("guarded by readonly_base().is_some()"),
            subst,
        ),
        (Type::Readonly(p_inner), actual) if actual.is_mutable_structural() => {
            unify(p_inner.as_ref(), actual, subst)
        }
        (Type::Record(p_fields), Type::Record(a_fields)) => {
            for (name, p_ty) in p_fields {
                if let Some(a_ty) = a_fields.get(name) {
                    unify(p_ty, a_ty, subst)?;
                } else {
                    return Err(Diagnostic::new(format!(
                        "missing record field '{name}' in actual type {actual_ty}"
                    )));
                }
            }
            Ok(())
        }
        (
            Type::Function {
                params: p_params,
                return_type: p_ret,
                has_self: p_self,
            },
            Type::Function {
                params: a_params,
                return_type: a_ret,
                has_self: a_self,
            },
        ) => {
            if p_self != a_self {
                return Err(Diagnostic::new(format!(
                    "cannot match a 'self' method type with a plain function type: \
                     {param_ty} vs {actual_ty}"
                )));
            }
            if p_params.len() != a_params.len() {
                return Err(Diagnostic::new(format!(
                    "cannot match functions of different arity: {param_ty} vs {actual_ty}"
                )));
            }
            for (p_param, a_param) in p_params.iter().zip(a_params.iter()) {
                unify(p_param, a_param, subst)?;
            }
            unify(p_ret.as_ref(), a_ret.as_ref(), subst)
        }
        (p, a) => {
            if !contains_type_param(p) {
                if coerce_type(a.clone(), Some(p.clone())).is_ok() {
                    Ok(())
                } else {
                    Err(Diagnostic::new(format!("cannot match type {p} with {a}")))
                }
            } else {
                Err(Diagnostic::new(format!("cannot match type {p} with {a}")))
            }
        }
    }
}

fn infer_type_arguments(
    scheme: &GenericScheme,
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Result<Vec<Type>, Diagnostic> {
    let mut subst = HashMap::new();

    if let Some(expected_ty) = expected {
        if expected_ty != Type::Unknown {
            let _ = unify(&scheme.return_type, &expected_ty, &mut subst);
        }
    }

    for (param_ty, arg) in scheme.params.iter().zip(args.iter()) {
        let substituted_param_ty = substitute_type(param_ty, &subst);
        let actual_arg_ty = if contains_type_param(&substituted_param_ty) {
            infer_expr(arg, vars, fn_signatures, active_type_params, None)?
        } else {
            infer_expr(
                arg,
                vars,
                fn_signatures,
                active_type_params,
                Some(substituted_param_ty.clone()),
            )?
        };
        unify(&substituted_param_ty, &actual_arg_ty, &mut subst)?;
    }

    let mut inferred_type_args = Vec::new();
    for param in &scheme.type_params {
        if let Some(ty) = subst.get(param) {
            inferred_type_args.push(ty.clone());
        } else {
            return Err(generic_diagnostic(
                "generic/missing-type-args",
                format!("cannot infer type argument for type parameter '{param}'"),
                "specify type arguments explicitly",
            ));
        }
    }

    Ok(inferred_type_args)
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
    let inferred_type_args;
    let type_args = if type_args.is_empty() && !scheme.type_params.is_empty() {
        inferred_type_args = infer_type_arguments(
            scheme,
            args,
            vars,
            fn_signatures,
            active_type_params,
            expected.clone(),
        )?;
        &inferred_type_args
    } else {
        if type_args.is_empty() {
            return Err(generic_diagnostic(
                "generic/missing-type-args",
                "generic function call requires explicit type arguments",
                "supply type arguments between the callee and argument list, e.g. id<i32>(value)",
            ));
        }
        type_args
    };
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
        if ty.contains_readonly() {
            return Err(generic_diagnostic(
                "generic/readonly-type-arg",
                format!(
                    "read-only type argument '{ty}' cannot be used by an unconstrained generic body"
                ),
                "use a non-generic function whose parameter explicitly preserves readonly<T>",
            ));
        }
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

pub(super) fn infer_top_level_function_return_type(
    function: &Function,
    fn_signatures: &HashMap<String, FnSignature>,
    unresolved_names: &[String],
    module_bindings: &HashMap<String, Binding>,
) -> Result<Option<Type>, Diagnostic> {
    let mut vars = module_bindings.clone();
    for param in &function.params {
        vars.insert(
            param.name.clone(),
            binding_for(param.ty.clone(), Rebindability::Rebindable),
        );
    }

    let mut local_signatures = fn_signatures.clone();
    if let Some(function_name) = function.name.simple_name() {
        if unresolved_names.iter().any(|name| name == function_name)
            && function_calls(function, function_name)
            && !function.vararg
        {
            return Ok(None);
        }
        if unresolved_names.iter().any(|name| name == function_name) && function.vararg {
            local_signatures.insert(
                function_name.to_string(),
                FnSignature::Mono {
                    params: function
                        .params
                        .iter()
                        .map(|param| param.ty.clone())
                        .collect(),
                    vararg: function.vararg,
                    return_type: Type::String,
                },
            );
        }
    }
    let mut returns = Vec::new();
    if let Err(error) = collect_return_types(
        &function.body,
        &vars,
        &local_signatures,
        &HashSet::new(),
        &mut returns,
    ) {
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
        return Ok(Some(Type::Unit));
    }
    let mut merged = returns[0].clone();
    for ty in returns.into_iter().skip(1) {
        merged = common_return_type(merged, ty)?;
    }
    // A function that can fall off the end implicitly returns nil, making an
    // otherwise non-nil return type nullable (Lua semantics).
    if !matches!(
        merged,
        Type::Unit | Type::Nullable(_) | Type::Nil | Type::Multi(_)
    ) && !super::statements::stmts_always_return(&function.body)
    {
        merged = Type::Nullable(Box::new(merged));
    }
    Ok(Some(merged))
}

pub(super) fn infer_function_expr_return_type(
    function: &FunctionExpr,
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
) -> Result<Type, Diagnostic> {
    let mut local_vars = vars.clone();
    for param in &function.params {
        local_vars.insert(
            param.name.clone(),
            binding_for(param.ty.clone(), Rebindability::Rebindable),
        );
    }
    if let Some(function_name) = &function.name {
        let function_ty = Type::Function {
            params: function
                .params
                .iter()
                .map(|param| param.ty.clone())
                .collect(),
            return_type: Box::new(Type::Unit),
            has_self: false,
        };
        local_vars.insert(
            function_name.clone(),
            binding_for(function_ty, Rebindability::Rebindable),
        );
    }

    let mut returns = Vec::new();
    collect_return_types(
        &function.body,
        &local_vars,
        fn_signatures,
        active_type_params,
        &mut returns,
    )?;
    if returns.is_empty() {
        return Ok(Type::Unit);
    }
    let mut merged = returns[0].clone();
    for ty in returns.into_iter().skip(1) {
        merged = common_return_type(merged, ty)?;
    }
    if !matches!(
        merged,
        Type::Unit | Type::Nullable(_) | Type::Nil | Type::Multi(_)
    ) && !super::statements::stmts_always_return(&function.body)
    {
        merged = Type::Nullable(Box::new(merged));
    }
    Ok(merged)
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
