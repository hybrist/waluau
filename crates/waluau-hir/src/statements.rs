use std::collections::{BTreeMap, HashMap, HashSet};

use waluau_ast::{AssignOp, Expr, Function, NumericType, Rebindability, Stmt, Type};
use waluau_diagnostics::{Diagnostic, DiagnosticCategory};

use super::builtins::{ASSERT, PCALL};
use super::expressions::{
    builtin_name, infer_expr, infer_expr_list, is_record_like, nominal_type_names,
};
use super::numeric::{infer_numeric_for_loop_type, is_extern_subtype_of};
use super::signatures::{
    FnSignature, active_type_param_set, generic_diagnostic, inference_diagnostic,
    validate_type_in_scope, validate_type_param_list,
};
use super::{Binding, PcallLink, binding_for};

fn method_signature_name(base: &str, method: &str) -> String {
    format!("{base}.{method}")
}

/// The per-binding expected types for a multi-binding declaration. Annotated
/// names use their annotation; unannotated names (Luau allows mixing the two
/// freely) fall back to the unconstrained type of their initializer slot, so
/// a mixed declaration checks like a fully annotated one.
fn expected_binding_types(
    bindings: &[waluau_ast::Binding],
    values: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
) -> Result<Vec<Type>, Diagnostic> {
    if bindings.iter().all(|binding| binding.ty.is_some()) {
        return Ok(bindings
            .iter()
            .map(|binding| binding.ty.clone().expect("checked above"))
            .collect());
    }
    let unconstrained = infer_expr_list(values, vars, fn_signatures, active_type_params, None)?;
    if unconstrained.len() < bindings.len() {
        return Err(Diagnostic::new(format!(
            "multi-binding declaration expects {} values, got {}",
            bindings.len(),
            unconstrained.len()
        )));
    }
    Ok(bindings
        .iter()
        .zip(unconstrained)
        .map(|(binding, inferred)| binding.ty.clone().unwrap_or(inferred))
        .collect())
}

fn property_setter_name(base: &str, property: &str) -> String {
    format!("{base}.set/{property}")
}

fn record_fields_from_type(ty: Type) -> Option<BTreeMap<String, Type>> {
    match ty {
        Type::Record(fields) => Some(fields),
        Type::Opaque { ty, .. } => record_fields_from_type(*ty),
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
    match (expected, actual) {
        (Type::Record(expected_fields), Type::Record(actual_fields)) => expected_fields
            .iter()
            .all(|(name, expected_ty)| actual_fields.get(name) == Some(expected_ty)),
        _ => false,
    }
}

pub(super) struct BranchScopes {
    pub(super) then_scope: HashMap<String, Binding>,
    pub(super) else_scope: HashMap<String, Binding>,
}

/// Resolves the dual-purpose `if Name(binding) = expr then ... end` syntax,
/// which is shared between two unrelated narrowing features that happen to
/// collide on surface syntax:
///
///   - Tagged-union pattern match with binding, e.g. `if Left(n) = either then`:
///     `Name` is a variant tag of the scrutinee's tagged-union type, and
///     `binding` is bound (in the `then` branch only) to the unboxed payload.
///   - Extern safe-cast, e.g. `if Element(node) = value then`: `Name` is an
///     extern type, and `binding` is bound (in the `then` branch only) to the
///     narrowed extern value.
///
/// Disambiguation can only happen here, once the scrutinee's inferred type is
/// known: the parser has no type/tag registries, so it always produces the
/// same `Stmt::IfCast` shape (with `target_ty` resolved to a real type when
/// possible, or left as an unresolved `Type::Named` when `target_name` turns
/// out to name a variant tag rather than a declared type).
pub(super) fn checked_if_cast_scopes(
    target_name: &str,
    target_ty: &Type,
    binding: &str,
    value: &Expr,
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
) -> Result<BranchScopes, Diagnostic> {
    let value_ty = infer_expr(value, vars, fn_signatures, active_type_params, None)?;
    let source_ty = value_ty
        .nullable_inner()
        .unwrap_or_else(|| value_ty.clone());

    // Tagged-union pattern match with binding: `if Tag(name) = expr then`.
    if let Some(variant) = source_ty.tagged_variant(target_name) {
        let mut then_scope = vars.clone();
        then_scope.insert(
            binding.to_string(),
            binding_for((*variant.payload).clone(), Rebindability::Const),
        );
        return Ok(BranchScopes {
            then_scope,
            else_scope: vars.clone(),
        });
    }

    // Extern safe-cast: `if Type(name) = expr then`.
    if !matches!(
        target_ty,
        Type::Opaque { ty, .. } if matches!(ty.as_ref(), Type::Extern | Type::ExternSubtype(_))
    ) {
        return Err(Diagnostic::new(format!(
            "'{target_name}' is neither a tagged-union variant of {source_ty} nor an extern type; \
             if-cast target must be an extern type or a tagged-union variant tag"
        )));
    }
    if !matches!(
        &source_ty,
        Type::Opaque { ty, .. } if matches!(ty.as_ref(), Type::Extern | Type::ExternSubtype(_))
    ) {
        return Err(Diagnostic::new(format!(
            "if-cast value must be an extern type, got {value_ty}"
        )));
    }
    if is_extern_subtype_of(&source_ty, target_ty) {
        return Err(Diagnostic::new(format!(
            "if-cast from {source_ty} to {target_ty} is an upcast; use direct assignment"
        )));
    }
    if !is_extern_subtype_of(target_ty, &source_ty) {
        return Err(Diagnostic::new(format!(
            "cannot if-cast unrelated extern types {source_ty} and {target_ty}"
        )));
    }
    let mut then_scope = vars.clone();
    then_scope.insert(
        binding.to_string(),
        binding_for(target_ty.clone(), Rebindability::Const),
    );
    Ok(BranchScopes {
        then_scope,
        else_scope: vars.clone(),
    })
}

fn type_property_setter_signature<'a>(
    receiver_ty: &Type,
    name: &str,
    fn_signatures: &'a HashMap<String, FnSignature>,
) -> Option<(&'a FnSignature, String)> {
    let setter_name = resolved_type_property_setter_name(receiver_ty, name, fn_signatures)?;
    let signature = fn_signatures.get(&setter_name)?;
    Some((signature, setter_name))
}

fn narrowed_variant_scopes(
    condition: &Expr,
    vars: &HashMap<String, Binding>,
) -> (HashMap<String, Binding>, HashMap<String, Binding>) {
    let mut then_scope = vars.clone();
    let mut else_scope = vars.clone();
    if let Expr::IsVariant { expr, tag, .. } = condition {
        let Expr::Name(name, _, _) = expr.as_ref() else {
            return (then_scope, else_scope);
        };
        let Some(binding) = vars.get(name) else {
            return (then_scope, else_scope);
        };
        if let Some(variant) = binding.ty.tagged_variant(tag) {
            then_scope.insert(
                name.clone(),
                binding_for(Type::TaggedVariant(variant), binding.rebindability),
            );
        }
        if let Some(remaining) = binding.ty.remove_tagged_variant(tag) {
            else_scope.insert(name.clone(), binding_for(remaining, binding.rebindability));
        }
    }
    (then_scope, else_scope)
}

/// Loop-variable types for `for ... in string.gmatch(s, p)`: the pattern's
/// captures, or the whole match when the pattern has none.
pub(super) fn gmatch_loop_value_types(
    iterator: &Expr,
    names_len: usize,
) -> Option<Result<Vec<Type>, Diagnostic>> {
    let pattern_arg = match iterator {
        Expr::Call { callee, args, .. } if args.len() == 2 => match callee.as_ref() {
            Expr::Field { base, name, .. }
                if name == "gmatch"
                    && matches!(base.as_ref(), Expr::Name(ns, _, _) if ns == "string") =>
            {
                &args[1]
            }
            _ => return None,
        },
        Expr::MethodCall { name, args, .. } if name == "gmatch" && args.len() == 1 => &args[0],
        _ => return None,
    };
    let captures = waluau_ast::expr_pattern_captures(pattern_arg);
    let slot_types: Vec<Type> = if captures.is_empty() {
        vec![Type::String]
    } else {
        captures.iter().map(|kind| kind.value_type()).collect()
    };
    if names_len > slot_types.len() {
        return Some(Err(Diagnostic::new(format!(
            "string.gmatch for-in loop binds {} variables, but the pattern only produces {} values",
            names_len,
            slot_types.len()
        ))));
    }
    Some(Ok(slot_types))
}

/// Whether a pcall payload type can be soundly materialized from the `unknown`
/// storage slot by a runtime unbox cast when narrowing applies.
pub(super) fn is_narrowable_pcall_payload(ty: &Type) -> bool {
    !matches!(ty, Type::Unit | Type::Unknown | Type::Nil | Type::Multi(_))
}

/// Recognizes `local ok, v = pcall(f, ...)` and returns the payload types for
/// the success and failure paths: `(f`'s return type, the error payload type)`.
/// Errors are always strings (`error(...)` and failed asserts both throw string
/// payloads), so the failure side is `Type::String`.
pub(super) fn pcall_discriminant_types(
    bindings: &[waluau_ast::Binding],
    values: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
) -> Option<(Type, Type)> {
    if bindings.len() != 2 {
        return None;
    }
    let [Expr::Call { callee, args, .. }] = values else {
        return None;
    };
    if builtin_name(callee.as_ref()).as_deref() != Some(PCALL) {
        return None;
    }
    let callee_ty =
        infer_expr(args.first()?, vars, fn_signatures, active_type_params, None).ok()?;
    let Type::Function { return_type, .. } = callee_ty else {
        return None;
    };
    Some((*return_type, Type::String))
}

/// Installs the bidirectional discriminant link for `local ok, v = pcall(...)`
/// on the freshly inserted `ok`/`v` bindings.
pub(super) fn link_pcall_bindings(
    bindings: &[waluau_ast::Binding],
    when_true: Type,
    when_false: Type,
    vars: &mut HashMap<String, Binding>,
) {
    let ok_name = bindings[0].name.clone();
    let payload_name = bindings[1].name.clone();
    if ok_name == payload_name {
        return;
    }
    if let Some(ok_binding) = vars.get_mut(&ok_name) {
        ok_binding.pcall_link = Some(PcallLink::Discriminant {
            payload: payload_name.clone(),
            when_true,
            when_false,
        });
    }
    if let Some(payload_binding) = vars.get_mut(&payload_name) {
        payload_binding.pcall_link = Some(PcallLink::Payload {
            discriminant: ok_name,
        });
    }
}

/// Reassigning either half of a pcall discriminant pair invalidates the
/// correlation, so drop the link from both sides.
pub(super) fn sever_pcall_link(name: &str, vars: &mut HashMap<String, Binding>) {
    let Some(binding) = vars.get_mut(name) else {
        return;
    };
    let Some(link) = binding.pcall_link.take() else {
        return;
    };
    let peer = match link {
        PcallLink::Discriminant { payload, .. } => payload,
        PcallLink::Payload { discriminant } => discriminant,
    };
    if let Some(peer_binding) = vars.get_mut(&peer) {
        peer_binding.pcall_link = None;
    }
}

/// A condition of the form `ok` or `not ok`, returning the name and whether
/// the `then` branch corresponds to `ok == true`.
fn discriminant_test_subject(condition: &Expr) -> Option<(&str, bool)> {
    match condition {
        Expr::Name(name, _, _) => Some((name, true)),
        Expr::Unary {
            op: waluau_ast::UnaryOp::Not,
            expr,
            ..
        } => match expr.as_ref() {
            Expr::Name(name, _, _) => Some((name, false)),
            _ => None,
        },
        _ => None,
    }
}

fn narrowed_discriminant_scopes(
    condition: &Expr,
    vars: &HashMap<String, Binding>,
    then_scope: &mut HashMap<String, Binding>,
    else_scope: &mut HashMap<String, Binding>,
) {
    let Some((name, positive)) = discriminant_test_subject(condition) else {
        return;
    };
    let Some(PcallLink::Discriminant {
        payload,
        when_true,
        when_false,
    }) = vars.get(name).and_then(|b| b.pcall_link.as_ref())
    else {
        return;
    };
    let Some(payload_binding) = vars.get(payload) else {
        return;
    };
    // The pair must still point at each other; shadowing either name severs it.
    if !matches!(
        &payload_binding.pcall_link,
        Some(PcallLink::Payload { discriminant }) if discriminant == name
    ) {
        return;
    }
    let (then_ty, else_ty) = if positive {
        (when_true, when_false)
    } else {
        (when_false, when_true)
    };
    if is_narrowable_pcall_payload(then_ty) {
        then_scope.insert(
            payload.clone(),
            binding_for(then_ty.clone(), payload_binding.rebindability),
        );
    }
    if is_narrowable_pcall_payload(else_ty) {
        else_scope.insert(
            payload.clone(),
            binding_for(else_ty.clone(), payload_binding.rebindability),
        );
    }
}

/// The scope in effect after `assert(cond)` succeeded: the full then-branch
/// narrowing (pcall discriminant, variant tests, nil tests). A consequence is
/// that testing a variant the assert already ruled out is a type error — by
/// design, since such a check can never be meaningful.
pub(super) fn assert_narrowed_scope(
    condition: &Expr,
    vars: &HashMap<String, Binding>,
) -> HashMap<String, Binding> {
    narrowed_scopes(condition, vars).0
}

fn nil_test_subject(condition: &Expr) -> Option<(&str, Option<&str>, bool)> {
    // A bare `if x then` on a nullable value is a truthiness test: the
    // then-branch sees the non-nil type.
    if let Expr::Name(name, _, _) = condition {
        return Some((name, None, true));
    }
    if let Expr::Unary {
        op: waluau_ast::UnaryOp::Not,
        expr,
        ..
    } = condition
    {
        if let Expr::Name(name, _, _) = expr.as_ref() {
            return Some((name, None, false));
        }
    }
    let Expr::Binary {
        op, left, right, ..
    } = condition
    else {
        return None;
    };
    let non_null_when_true = match op {
        waluau_ast::BinaryOp::Eq => false,
        waluau_ast::BinaryOp::NotEq => true,
        _ => return None,
    };
    match (left.as_ref(), right.as_ref()) {
        (Expr::Name(name, _, _), Expr::Nil(..)) | (Expr::Nil(..), Expr::Name(name, _, _)) => {
            Some((name, None, non_null_when_true))
        }
        (Expr::Field { base, name, .. }, Expr::Nil(..))
        | (Expr::Nil(..), Expr::Field { base, name, .. }) => {
            let Expr::Name(base, _, _) = base.as_ref() else {
                return None;
            };
            Some((base, Some(name), non_null_when_true))
        }
        _ => None,
    }
}

fn narrow_nullable_record_field(ty: &Type, field: &str) -> Option<Type> {
    match ty {
        Type::Record(fields) => {
            let inner = fields.get(field)?.nullable_inner()?;
            let mut narrowed = fields.clone();
            narrowed.insert(field.to_string(), inner);
            Some(Type::Record(narrowed))
        }
        Type::Opaque { name, ty } => Some(Type::Opaque {
            name: name.clone(),
            ty: Box::new(narrow_nullable_record_field(ty, field)?),
        }),
        _ => None,
    }
}

pub(super) fn narrowed_scopes(
    condition: &Expr,
    vars: &HashMap<String, Binding>,
) -> (HashMap<String, Binding>, HashMap<String, Binding>) {
    let (mut then_scope, mut else_scope) = narrowed_variant_scopes(condition, vars);
    narrowed_discriminant_scopes(condition, vars, &mut then_scope, &mut else_scope);
    let Some((name, field, non_null_when_true)) = nil_test_subject(condition) else {
        return (then_scope, else_scope);
    };
    let Some(binding) = vars.get(name) else {
        return (then_scope, else_scope);
    };
    let narrowed_ty = if let Some(field) = field {
        let Some(narrowed) = narrow_nullable_record_field(&binding.ty, field) else {
            return (then_scope, else_scope);
        };
        narrowed
    } else {
        let Some(inner) = binding.ty.nullable_inner() else {
            return (then_scope, else_scope);
        };
        inner
    };
    let target = if non_null_when_true {
        &mut then_scope
    } else {
        &mut else_scope
    };
    let mut narrowed = binding.clone();
    narrowed.ty = narrowed_ty;
    target.insert(name.to_string(), narrowed);
    (then_scope, else_scope)
}

pub(super) fn resolved_type_property_setter_name(
    receiver_ty: &Type,
    name: &str,
    fn_signatures: &HashMap<String, FnSignature>,
) -> Option<String> {
    for type_name in nominal_type_names(receiver_ty) {
        let setter_name = property_setter_name(type_name, name);
        if fn_signatures.contains_key(&setter_name) {
            return Some(setter_name);
        }
    }

    if is_record_like(receiver_ty) {
        return None;
    }

    let suffix = format!(".set/{name}");
    let mut matches = fn_signatures
        .iter()
        .filter_map(|(setter_name, signature)| {
            if !setter_name.ends_with(&suffix) {
                return None;
            }
            let FnSignature::Mono { params, .. } = signature else {
                return None;
            };
            let receiver_param = params.first()?;
            (params.len() == 2 && method_receiver_matches(receiver_param, receiver_ty))
                .then_some(setter_name.clone())
        })
        .collect::<Vec<_>>();
    (matches.len() == 1).then(|| matches.remove(0))
}

pub(super) fn check_function(
    function: &Function,
    fn_signatures: &HashMap<String, FnSignature>,
    outer_type_params: &HashSet<String>,
) -> Result<(), Diagnostic> {
    let mut diagnostics =
        check_function_collect(function, fn_signatures, outer_type_params, &HashMap::new());
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics.remove(0))
    }
}

/// Check a function, collecting a diagnostic per failing top-level statement
/// instead of stopping at the first. Bindings introduced by a failed
/// statement fall back to `unknown` so later statements don't report
/// cascading unknown-variable errors.
pub(super) fn check_function_collect(
    function: &Function,
    fn_signatures: &HashMap<String, FnSignature>,
    outer_type_params: &HashSet<String>,
    module_bindings: &HashMap<String, Binding>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if let Err(error) = validate_type_param_list(&function.type_params, outer_type_params) {
        diagnostics.push(error);
        return diagnostics;
    }
    let active_type_params = active_type_param_set(&function.type_params);
    let mut allowed_type_params = outer_type_params.clone();
    allowed_type_params.extend(active_type_params.iter().cloned());
    for param in &function.params {
        if let Err(error) = validate_type_in_scope(&param.ty, &allowed_type_params) {
            diagnostics.push(error);
        }
    }
    if let Some(ret) = &function.return_type {
        if let Err(error) = validate_type_in_scope(ret, &allowed_type_params) {
            diagnostics.push(error);
        }
    }
    let Some(expected_return) = function.return_type.clone() else {
        diagnostics.push(if function.type_params.is_empty() {
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
        });
        return diagnostics;
    };
    let mut vars = module_bindings.clone();
    for param in &function.params {
        vars.insert(
            param.name.clone(),
            binding_for(param.ty.clone(), Rebindability::Rebindable),
        );
    }

    let mut saw_return = false;
    let mut any_stmt_error = false;
    for stmt in &function.body {
        match check_stmt(
            stmt,
            &mut vars,
            fn_signatures,
            &active_type_params,
            &expected_return,
            false,
        ) {
            Ok(returned) => saw_return |= returned,
            Err(error) => {
                any_stmt_error = true;
                diagnostics.push(error);
                bind_failed_stmt_names_as_unknown(stmt, &mut vars);
            }
        }
    }
    // A failed statement may have been the return, so only report a missing
    // return when every statement checked cleanly.
    if !any_stmt_error
        && !saw_return
        && expected_return != Type::Unit
        && !matches!(expected_return, Type::Nullable(_))
    {
        let mut diagnostic =
            Diagnostic::new(format!("function '{}' is missing a return", function.name));
        // Point at the last statement — where the return should follow.
        if let Some(span) = function.body.last().and_then(primary_stmt_span) {
            diagnostic = diagnostic.with_span(span);
        }
        diagnostics.push(diagnostic);
    }
    diagnostics
}

/// After a statement fails to check, bind any names it declares so later
/// statements referencing them don't cascade: an explicit annotation is
/// trusted (it's the user's stated intent), otherwise fall back to `unknown`.
fn bind_failed_stmt_names_as_unknown(stmt: &Stmt, vars: &mut HashMap<String, Binding>) {
    match stmt {
        Stmt::Let {
            name,
            rebindability,
            ty,
            ..
        } => {
            let fallback = ty.clone().unwrap_or(Type::Unknown);
            vars.insert(name.clone(), binding_for(fallback, *rebindability));
        }
        Stmt::LetMulti { bindings, .. } => {
            for binding in bindings {
                let fallback = binding.ty.clone().unwrap_or(Type::Unknown);
                vars.insert(
                    binding.name.clone(),
                    binding_for(fallback, binding.rebindability),
                );
            }
        }
        _ => {}
    }
}

pub(super) fn collect_return_types(
    body: &[Stmt],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    returns: &mut Vec<Type>,
) -> Result<(), Diagnostic> {
    collect_return_types_with_scope(body, vars, fn_signatures, active_type_params, returns)
        .map(|_| ())
}

/// Like `collect_return_types`, but also returns the scope at the end of the
/// statement list. `repeat ... until <cond>` needs it: the condition is
/// evaluated in the body's scope (Lua keeps body locals alive for the
/// condition), so it must be inferred against the accumulated bindings.
fn collect_return_types_with_scope(
    body: &[Stmt],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    returns: &mut Vec<Type>,
) -> Result<HashMap<String, Binding>, Diagnostic> {
    let mut scope = vars.clone();
    for stmt in body {
        match stmt {
            Stmt::Match {
                value,
                enum_ty,
                arms,
            } => {
                let actual = infer_expr(
                    value,
                    &scope,
                    fn_signatures,
                    active_type_params,
                    Some(enum_ty.clone()),
                )?;
                if &actual != enum_ty {
                    return Err(Diagnostic::new(format!(
                        "match expects {enum_ty}, got {actual}"
                    )));
                }
                for arm in arms {
                    let _ = collect_return_types_with_scope(
                        &arm.body,
                        &scope,
                        fn_signatures,
                        active_type_params,
                        returns,
                    )?;
                }
            }
            Stmt::Let {
                name,
                rebindability,
                ty,
                value,
                ..
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
                    super::expressions::first_of_multi(infer_expr(
                        value,
                        &scope,
                        fn_signatures,
                        active_type_params,
                        None,
                    )?)
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
                sever_pcall_link(name, &mut scope);
            }
            Stmt::IndexAssign {
                base, index, value, ..
            } => {
                let base_ty = infer_expr(base, &scope, fn_signatures, active_type_params, None)?;
                let element_ty = base_ty.element_type().ok_or_else(|| {
                    Diagnostic::new("array element assignment requires an array operand")
                })?;
                if let Type::TypedArray(kind) = &base_ty {
                    // Typed arrays accept any numeric index and value; the
                    // lowering converts both (index to i32, value to the
                    // element type) like explicit casts.
                    let index_ty = infer_expr(
                        index,
                        &scope,
                        fn_signatures,
                        active_type_params,
                        Some(Type::Numeric(NumericType::I32)),
                    )
                    .or_else(|_| {
                        infer_expr(index, &scope, fn_signatures, active_type_params, None)
                    })?;
                    if !index_ty.is_numeric() {
                        return Err(Diagnostic::new(format!(
                            "{} index must be numeric, got {index_ty}",
                            kind.type_name()
                        )));
                    }
                    let value_ty = infer_expr(
                        value,
                        &scope,
                        fn_signatures,
                        active_type_params,
                        Some(element_ty),
                    )
                    .or_else(|_| {
                        infer_expr(value, &scope, fn_signatures, active_type_params, None)
                    })?;
                    if !value_ty.is_numeric() {
                        return Err(Diagnostic::new(format!(
                            "{} element must be numeric, got {value_ty}",
                            kind.type_name()
                        )));
                    }
                } else {
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
            }
            Stmt::FieldAssign {
                base, name, value, ..
            } => {
                if let Expr::Name(base_name, _, _) = base.as_ref() {
                    let binding = scope
                        .get(base_name)
                        .cloned()
                        .ok_or_else(|| Diagnostic::new(format!("unknown local '{base_name}'")))?;
                    let Some(mut fields) = record_fields_from_type(binding.ty.clone()) else {
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
                } else {
                    let base_ty =
                        infer_expr(base, &scope, fn_signatures, active_type_params, None)?;
                    let Some(fields) = record_fields_from_type(base_ty) else {
                        return Err(Diagnostic::new("field assignment requires a record base"));
                    };
                    let field_ty = fields
                        .get(name)
                        .ok_or_else(|| Diagnostic::new(format!("unknown record field '{name}'")))?;
                    let value_ty = infer_expr(
                        value,
                        &scope,
                        fn_signatures,
                        active_type_params,
                        Some(field_ty.clone()),
                    )?;
                    if value_ty != *field_ty {
                        return Err(Diagnostic::new(format!(
                            "field assignment to '{name}' expects {field_ty}, got {value_ty}"
                        )));
                    }
                    seal_record_locals_in_expr(value, &mut scope);
                }
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                let condition_ty =
                    infer_expr(condition, &scope, fn_signatures, active_type_params, None)?;
                seal_record_locals_in_expr(condition, &mut scope);
                if condition_ty != Type::Bool && !matches!(condition_ty, Type::Nullable(_)) {
                    return Err(Diagnostic::new("if condition must be bool"));
                }
                let (then_scope, else_scope) = narrowed_scopes(condition, &scope);
                collect_return_types(
                    then_body,
                    &then_scope,
                    fn_signatures,
                    active_type_params,
                    returns,
                )?;
                collect_return_types(
                    else_body,
                    &else_scope,
                    fn_signatures,
                    active_type_params,
                    returns,
                )?;
            }
            Stmt::IfCast {
                target_name,
                target_ty,
                binding,
                value,
                then_body,
                else_body,
                ..
            } => {
                let branch_scopes = checked_if_cast_scopes(
                    target_name,
                    target_ty,
                    binding,
                    value,
                    &scope,
                    fn_signatures,
                    active_type_params,
                )?;
                seal_record_locals_in_expr(value, &mut scope);
                collect_return_types(
                    then_body,
                    &branch_scopes.then_scope,
                    fn_signatures,
                    active_type_params,
                    returns,
                )?;
                collect_return_types(
                    else_body,
                    &branch_scopes.else_scope,
                    fn_signatures,
                    active_type_params,
                    returns,
                )?;
            }
            Stmt::While { condition, body } => {
                let condition_ty =
                    infer_expr(condition, &scope, fn_signatures, active_type_params, None)?;
                seal_record_locals_in_expr(condition, &mut scope);
                if condition_ty != Type::Bool && !matches!(condition_ty, Type::Nullable(_)) {
                    return Err(Diagnostic::new("while condition must be bool"));
                }
                collect_return_types(body, &scope, fn_signatures, active_type_params, returns)?;
            }
            Stmt::Repeat { body, condition } => {
                let mut loop_scope = collect_return_types_with_scope(
                    body,
                    &scope,
                    fn_signatures,
                    active_type_params,
                    returns,
                )?;
                let condition_ty = infer_expr(
                    condition,
                    &loop_scope,
                    fn_signatures,
                    active_type_params,
                    None,
                )?;
                seal_record_locals_in_expr(condition, &mut loop_scope);
                if condition_ty != Type::Bool && !matches!(condition_ty, Type::Nullable(_)) {
                    return Err(Diagnostic::new("repeat-until condition must be bool"));
                }
            }
            Stmt::NumericFor {
                name,
                start,
                stop,
                step,
                body,
                ..
            } => {
                let mut bounds = vec![start, stop];
                if let Some(step_expr) = step {
                    bounds.push(step_expr);
                }
                let loop_ty = infer_numeric_for_loop_type(&bounds, |expr, expected| {
                    infer_expr(expr, &scope, fn_signatures, active_type_params, expected)
                })?;
                seal_record_locals_in_expr(start, &mut scope);
                seal_record_locals_in_expr(stop, &mut scope);
                if let Some(step_expr) = step {
                    seal_record_locals_in_expr(step_expr, &mut scope);
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
                ..
            } => {
                if let Some(result) = gmatch_loop_value_types(iterator, names.len()) {
                    let loop_value_types = result?;
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
                    continue;
                }
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
                let any_typed = bindings.iter().any(|binding| binding.ty.is_some());
                let actual = if any_typed {
                    let expected = expected_binding_types(
                        bindings,
                        values,
                        &scope,
                        fn_signatures,
                        active_type_params,
                    )?;
                    let actual = infer_expr_list(
                        values,
                        &scope,
                        fn_signatures,
                        active_type_params,
                        Some(&expected),
                    )?;
                    if actual.len() < expected.len() {
                        return Err(Diagnostic::new(format!(
                            "multi-binding declaration expects {} values, got {}",
                            expected.len(),
                            actual.len()
                        )));
                    }
                    for (index, (binding, value_ty)) in
                        bindings.iter().zip(actual.iter()).enumerate()
                    {
                        let Some(expected_ty) = binding.ty.as_ref() else {
                            continue;
                        };
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
                    if actual.len() < bindings.len() {
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
                let discriminant_link = pcall_discriminant_types(
                    bindings,
                    values,
                    &scope,
                    fn_signatures,
                    active_type_params,
                );
                for (binding, value_ty) in bindings.iter().zip(actual) {
                    let ty = binding.ty.clone().unwrap_or(value_ty);
                    scope.insert(binding.name.clone(), binding_for(ty, binding.rebindability));
                }
                if let Some((when_true, when_false)) = discriminant_link {
                    link_pcall_bindings(bindings, when_true, when_false, &mut scope);
                }
            }
            Stmt::AssignMulti {
                targets, values, ..
            } => {
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
                if actual.len() < expected.len() {
                    return Err(Diagnostic::new(format!(
                        "multi-assignment expects {} values, got {}",
                        expected.len(),
                        actual.len()
                    )));
                }
                for target in targets {
                    sever_pcall_link(target, &mut scope);
                }
            }
            Stmt::Expr(expr) => {
                if let Expr::Call { callee, args, .. } = expr {
                    if let Expr::Name(name, _, _) = callee.as_ref() {
                        if name == ASSERT {
                            if !(1..=2).contains(&args.len()) {
                                return Err(Diagnostic::new(format!(
                                    "{ASSERT} expects 1 or 2 arguments, got {}",
                                    args.len()
                                )));
                            }
                            let actual = infer_expr(
                                &args[0],
                                &scope,
                                fn_signatures,
                                active_type_params,
                                Some(Type::Bool),
                            )?;
                            if actual != Type::Bool {
                                return Err(Diagnostic::new(format!(
                                    "{ASSERT} expects bool, got {actual}"
                                )));
                            }
                            if args.len() == 2 {
                                infer_expr(
                                    &args[1],
                                    &scope,
                                    fn_signatures,
                                    active_type_params,
                                    Some(Type::String),
                                )?;
                            }
                            // Statements after `assert(cond)` only run when the
                            // condition held, so the then-branch narrowing applies.
                            scope = assert_narrowed_scope(&args[0], &scope);
                            continue;
                        }
                    }
                }
                let _ = infer_expr(expr, &scope, fn_signatures, active_type_params, None)?;
                seal_record_locals_in_expr(expr, &mut scope);
            }
        }
    }
    Ok(scope)
}

/// Conservatively decides whether every control path through `stmts` ends in
/// an explicit return (or an unconditional `error(...)` call). Used to detect
/// functions that can fall off the end and therefore implicitly return nil.
pub(super) fn stmts_always_return(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Stmt::Return(_) | Stmt::ReturnMulti(_) => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        }
        | Stmt::IfCast {
            then_body,
            else_body,
            ..
        } => {
            !else_body.is_empty()
                && stmts_always_return(then_body)
                && stmts_always_return(else_body)
        }
        Stmt::Match { arms, .. } => {
            !arms.is_empty() && arms.iter().all(|arm| stmts_always_return(&arm.body))
        }
        Stmt::Expr(Expr::Call { callee, .. }) => {
            matches!(callee.as_ref(), Expr::Name(name, _, _) if name == "error")
        }
        _ => false,
    })
}

pub(super) fn common_return_type(left: Type, right: Type) -> Result<Type, Diagnostic> {
    if left == right {
        return Ok(left);
    }
    match (left, right) {
        // `return x` mixed with `return nil` (or an implicit nil fall-through)
        // makes the return type nullable.
        (Type::Nil, Type::Nullable(inner)) | (Type::Nullable(inner), Type::Nil) => {
            Ok(Type::Nullable(inner))
        }
        (Type::Nil, other) | (other, Type::Nil) if other != Type::Unit => {
            Ok(Type::Nullable(Box::new(other)))
        }
        (Type::Nullable(inner), other) | (other, Type::Nullable(inner)) if *inner == other => {
            Ok(Type::Nullable(inner))
        }
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

/// The span most representative of a statement, for diagnostics minted at
/// statement level (as opposed to inside expression inference, which
/// attaches the offending expression's own span): the condition for
/// conditionals and loops, the value/iterator otherwise.
pub(super) fn primary_stmt_span(stmt: &Stmt) -> Option<waluau_ast::Span> {
    match stmt {
        Stmt::Let { value, .. }
        | Stmt::Assign { value, .. }
        | Stmt::IndexAssign { value, .. }
        | Stmt::FieldAssign { value, .. } => value.span(),
        Stmt::If { condition, .. }
        | Stmt::While { condition, .. }
        | Stmt::Repeat { condition, .. } => condition.span(),
        Stmt::IfCast { value, .. } => value.span(),
        Stmt::Match { value, .. } => value.span(),
        Stmt::NumericFor { start, .. } => start.span(),
        Stmt::ForIn { iterator, .. } => iterator.span(),
        Stmt::Return(value) => value.span(),
        Stmt::ReturnMulti(values) => values.first().and_then(|value| value.span()),
        Stmt::LetMulti { values, .. } | Stmt::AssignMulti { values, .. } => {
            values.first().and_then(|value| value.span())
        }
        Stmt::Expr(expr) => expr.span(),
        Stmt::Break | Stmt::Continue => None,
    }
}

fn attach_stmt_span(error: Diagnostic, stmt: &Stmt) -> Diagnostic {
    match (error.span(), primary_stmt_span(stmt)) {
        (None, Some(span)) => error.with_span(span),
        _ => error,
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
    check_stmt_inner(
        stmt,
        vars,
        fn_signatures,
        active_type_params,
        expected_return,
        in_loop,
    )
    .map_err(|error| attach_stmt_span(error, stmt))
}

fn check_stmt_inner(
    stmt: &Stmt,
    vars: &mut HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected_return: &Type,
    in_loop: bool,
) -> Result<bool, Diagnostic> {
    match stmt {
        Stmt::Match {
            value,
            enum_ty,
            arms,
        } => {
            let actual = infer_expr(
                value,
                vars,
                fn_signatures,
                active_type_params,
                Some(enum_ty.clone()),
            )?;
            if &actual != enum_ty {
                return Err(Diagnostic::new(format!(
                    "match expects {enum_ty}, got {actual}"
                )));
            }
            seal_record_locals_in_expr(value, vars);
            let mut all_return = !arms.is_empty();
            for arm in arms {
                let mut arm_scope = vars.clone();
                let mut arm_returns = false;
                for stmt in &arm.body {
                    arm_returns |= check_stmt(
                        stmt,
                        &mut arm_scope,
                        fn_signatures,
                        active_type_params,
                        expected_return,
                        in_loop,
                    )?;
                }
                all_return &= arm_returns;
            }
            Ok(all_return)
        }
        Stmt::Let {
            name,
            rebindability,
            ty,
            value,
            ..
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
                super::expressions::first_of_multi(infer_expr(
                    value,
                    vars,
                    fn_signatures,
                    active_type_params,
                    None,
                )?)
            };
            seal_record_locals_in_expr(value, vars);
            vars.insert(name.clone(), binding_for(inferred_ty, *rebindability));
            Ok(false)
        }
        Stmt::Assign {
            op, name, value, ..
        } => {
            let existing = vars
                .get(name)
                .ok_or_else(|| Diagnostic::new(format!("unknown local '{name}'")))?;
            if let AssignOp::Compound(bin_op) = *op {
                if !bin_op.compound_target_ok(&existing.ty) {
                    return Err(Diagnostic::new(format!(
                        "compound assignment to '{}' requires a {} target",
                        name,
                        bin_op.compound_target_kind()
                    )));
                }
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
            sever_pcall_link(name, vars);
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
            if let AssignOp::Compound(bin_op) = *op {
                if !bin_op.compound_target_ok(&element_ty) {
                    return Err(Diagnostic::new(format!(
                        "compound array assignment requires {} elements",
                        bin_op.compound_target_kind()
                    )));
                }
            }
            if let Type::TypedArray(kind) = &base_ty {
                // Typed arrays accept any numeric index and value; lowering
                // converts both (index to i32, value to the element type)
                // like explicit casts.
                let index_ty = infer_expr(
                    index,
                    vars,
                    fn_signatures,
                    active_type_params,
                    Some(Type::Numeric(NumericType::I32)),
                )
                .or_else(|_| infer_expr(index, vars, fn_signatures, active_type_params, None))?;
                if !index_ty.is_numeric() {
                    return Err(Diagnostic::new(format!(
                        "{} index must be numeric, got {index_ty}",
                        kind.type_name()
                    )));
                }
                let value_ty = infer_expr(
                    value,
                    vars,
                    fn_signatures,
                    active_type_params,
                    Some(element_ty),
                )
                .or_else(|_| infer_expr(value, vars, fn_signatures, active_type_params, None))?;
                if !value_ty.is_numeric() {
                    return Err(Diagnostic::new(format!(
                        "{} element must be numeric, got {value_ty}",
                        kind.type_name()
                    )));
                }
                return Ok(false);
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
            resolved_name: _,
            value,
        } => {
            let base_ty = infer_expr(base, vars, fn_signatures, active_type_params, None)?;
            if let Some((signature, _)) =
                type_property_setter_signature(&base_ty, name, fn_signatures)
            {
                if *op != AssignOp::Set {
                    return Err(Diagnostic::new(
                        "compound property assignment is not supported",
                    ));
                }
                let FnSignature::Mono {
                    params,
                    return_type,
                    ..
                } = signature
                else {
                    return Err(generic_diagnostic(
                        "generic-property/setter",
                        format!("generic property setter for '{name}' is not supported"),
                        "declare properties with concrete types",
                    ));
                };
                if params.len() != 2 || !method_receiver_matches(&params[0], &base_ty) {
                    return Err(Diagnostic::new(format!(
                        "property setter for '{name}' does not accept receiver {base_ty}"
                    )));
                }
                if return_type != &Type::Unit {
                    return Err(Diagnostic::new(format!(
                        "property setter for '{name}' must return unit"
                    )));
                }
                infer_expr(
                    value,
                    vars,
                    fn_signatures,
                    active_type_params,
                    Some(params[1].clone()),
                )?;
                seal_record_locals_in_expr(value, vars);
                return Ok(false);
            }
            if let Expr::Name(base_name, _, _) = base.as_ref() {
                let binding = vars
                    .get(base_name)
                    .cloned()
                    .ok_or_else(|| Diagnostic::new(format!("unknown local '{base_name}'")))?;
                let Some(mut fields) = record_fields_from_type(binding.ty.clone()) else {
                    return Err(Diagnostic::new("field assignment requires a record base"));
                };
                let method_name = method_signature_name(base_name, name);
                if let Expr::Function(function) = value {
                    if !function.type_params.is_empty() && fn_signatures.contains_key(&method_name)
                    {
                        if *op != AssignOp::Set {
                            return Err(Diagnostic::new(
                                "compound field assignment requires a numeric field",
                            ));
                        }
                        if !fields.contains_key(name) && !binding.record_open {
                            return Err(Diagnostic::new(format!(
                                "cannot add new field '{}.{}' after record was sealed",
                                base_name, name
                            )));
                        }
                        let synthetic = Function {
                            name: waluau_ast::FunctionName::Simple(method_name),
                            symbol_id: None,
                            type_params: function.type_params.clone(),
                            params: function.params.clone(),
                            vararg: function.vararg,
                            return_type: function.return_type.clone(),
                            body: function.body.clone(),
                            file_path: function.file_path.clone(),
                        };
                        check_function(&synthetic, fn_signatures, active_type_params)?;
                        seal_record_locals_in_expr(value, vars);
                        let mut updated = binding_for(Type::Record(fields), binding.rebindability);
                        if !binding.record_open {
                            updated.record_open = false;
                        }
                        vars.insert(base_name.clone(), updated);
                        return Ok(false);
                    }
                }
                let existing_field = fields.get(name).cloned();
                let value_ty = infer_expr(
                    value,
                    vars,
                    fn_signatures,
                    active_type_params,
                    existing_field.clone(),
                )?;
                if let AssignOp::Compound(bin_op) = *op {
                    let field_ty = existing_field.clone().ok_or_else(|| {
                        Diagnostic::new(format!(
                            "compound field assignment requires an existing {} field",
                            bin_op.compound_target_kind()
                        ))
                    })?;
                    if !bin_op.compound_target_ok(&field_ty) || value_ty != field_ty {
                        return Err(Diagnostic::new(format!(
                            "compound field assignment requires a {} field",
                            bin_op.compound_target_kind()
                        )));
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
            } else {
                let base_ty = infer_expr(base, vars, fn_signatures, active_type_params, None)?;
                let Some(fields) = record_fields_from_type(base_ty) else {
                    return Err(Diagnostic::new("field assignment requires a record base"));
                };
                let field_ty = fields
                    .get(name)
                    .ok_or_else(|| Diagnostic::new(format!("unknown record field '{name}'")))?;
                if let AssignOp::Compound(bin_op) = *op {
                    if !bin_op.compound_target_ok(field_ty) {
                        return Err(Diagnostic::new(format!(
                            "compound field assignment requires a {} field",
                            bin_op.compound_target_kind()
                        )));
                    }
                }
                let value_ty = infer_expr(
                    value,
                    vars,
                    fn_signatures,
                    active_type_params,
                    Some(field_ty.clone()),
                )?;
                if value_ty != *field_ty {
                    return Err(Diagnostic::new(format!(
                        "field assignment to '{name}' expects {field_ty}, got {value_ty}",
                    )));
                }
                seal_record_locals_in_expr(value, vars);
                Ok(false)
            }
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            let condition_ty =
                infer_expr(condition, vars, fn_signatures, active_type_params, None)?;
            seal_record_locals_in_expr(condition, vars);
            if condition_ty != Type::Bool && !matches!(condition_ty, Type::Nullable(_)) {
                return Err(Diagnostic::new("if condition must be bool"));
            }
            let (mut then_scope, mut else_scope) = narrowed_scopes(condition, vars);
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
            if then_returns && !else_returns {
                *vars = else_scope;
            } else if else_returns && !then_returns {
                *vars = then_scope;
            }
            Ok(then_returns && else_returns)
        }
        Stmt::IfCast {
            target_name,
            target_ty,
            binding,
            value,
            then_body,
            else_body,
            ..
        } => {
            let branch_scopes = checked_if_cast_scopes(
                target_name,
                target_ty,
                binding,
                value,
                vars,
                fn_signatures,
                active_type_params,
            )?;
            let mut then_scope = branch_scopes.then_scope;
            let mut else_scope = branch_scopes.else_scope;
            seal_record_locals_in_expr(value, vars);
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
            if then_returns && !else_returns {
                *vars = else_scope;
            }
            Ok(then_returns && else_returns)
        }
        Stmt::While { condition, body } => {
            let condition_ty =
                infer_expr(condition, vars, fn_signatures, active_type_params, None)?;
            seal_record_locals_in_expr(condition, vars);
            if condition_ty != Type::Bool && !matches!(condition_ty, Type::Nullable(_)) {
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
            if condition_ty != Type::Bool && !matches!(condition_ty, Type::Nullable(_)) {
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
            ..
        } => {
            let mut bounds = vec![start, stop];
            if let Some(step_expr) = step {
                bounds.push(step_expr);
            }
            let loop_ty = infer_numeric_for_loop_type(&bounds, |expr, expected| {
                infer_expr(expr, vars, fn_signatures, active_type_params, expected)
            })?;
            seal_record_locals_in_expr(start, vars);
            seal_record_locals_in_expr(stop, vars);
            if let Some(step_expr) = step {
                seal_record_locals_in_expr(step_expr, vars);
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
            ..
        } => {
            let loop_value_types = if let Some(result) =
                gmatch_loop_value_types(iterator, names.len())
            {
                seal_record_locals_in_expr(iterator, vars);
                result?
            } else {
                let iterator_ty =
                    infer_expr(iterator, vars, fn_signatures, active_type_params, None)?;
                seal_record_locals_in_expr(iterator, vars);
                match iterator_ty {
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
            if matches!(expr, Expr::Nil(_)) && expected_return == &Type::Unit {
                return Ok(true);
            }
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
            let any_typed = bindings.iter().any(|binding| binding.ty.is_some());
            let actual = if any_typed {
                let expected = expected_binding_types(
                    bindings,
                    values,
                    vars,
                    fn_signatures,
                    active_type_params,
                )?;
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
                    let Some(expected_ty) = binding.ty.as_ref() else {
                        continue;
                    };
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
            let discriminant_link =
                pcall_discriminant_types(bindings, values, vars, fn_signatures, active_type_params);
            for (binding, value_ty) in bindings.iter().zip(actual) {
                let ty = binding.ty.clone().unwrap_or(value_ty);
                vars.insert(binding.name.clone(), binding_for(ty, binding.rebindability));
            }
            if let Some((when_true, when_false)) = discriminant_link {
                link_pcall_bindings(bindings, when_true, when_false, vars);
            }
            Ok(false)
        }
        Stmt::AssignMulti {
            targets, values, ..
        } => {
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
            for target in targets {
                sever_pcall_link(target, vars);
            }
            Ok(false)
        }
        Stmt::Expr(expr) => {
            if !matches!(expr, Expr::Call { .. } | Expr::MethodCall { .. }) {
                return Err(Diagnostic::new("expression statements must be calls"));
            }
            if let Expr::Call { callee, args, .. } = expr {
                if let Expr::Name(name, _, _) = callee.as_ref() {
                    if name == ASSERT {
                        if !(1..=2).contains(&args.len()) {
                            return Err(Diagnostic::new(format!(
                                "{ASSERT} expects 1 or 2 arguments, got {}",
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
                        if args.len() == 2 {
                            infer_expr(
                                &args[1],
                                vars,
                                fn_signatures,
                                active_type_params,
                                Some(Type::String),
                            )?;
                        }
                        // Statements after `assert(cond)` only run when the
                        // condition held, so the then-branch narrowing applies.
                        *vars = assert_narrowed_scope(&args[0], vars);
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
        Stmt::ReturnMulti(values)
        | Stmt::LetMulti { values, .. }
        | Stmt::AssignMulti { values, .. } => {
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
        Stmt::Match { value, arms, .. } => {
            expr_calls_name(value, callee)
                || arms
                    .iter()
                    .flat_map(|arm| &arm.body)
                    .any(|stmt| stmt_calls_name(stmt, callee))
        }
        Stmt::IfCast {
            value,
            then_body,
            else_body,
            ..
        } => {
            expr_calls_name(value, callee)
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
        | Expr::Nil(..)
        | Expr::String(..)
        | Expr::Bytes(..)
        | Expr::Vararg(..)
        | Expr::Require(..) => false,
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } | Expr::IsVariant { expr, .. } => {
            expr_calls_name(expr, callee)
        }
        Expr::Binary { left, right, .. }
        | Expr::Index {
            base: left,
            index: right,
            ..
        } => expr_calls_name(left, callee) || expr_calls_name(right, callee),
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
            args,
            ..
        } => {
            matches!(called.as_ref(), Expr::Name(name, _, _) if name == callee)
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
        Expr::ArrayLiteral { elements, .. } => elements
            .iter()
            .any(|element| expr_calls_name(element, callee)),
        Expr::TableLiteral { fields, .. } => fields
            .iter()
            .any(|field| expr_calls_name(&field.value, callee)),
        Expr::Field { base, .. } => expr_calls_name(base, callee),
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
        Expr::Name(name, _, _) => {
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
        | Expr::Nil(..)
        | Expr::String(..)
        | Expr::Bytes(..)
        | Expr::Vararg(..)
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
        Expr::IsVariant { expr, .. } => {
            seal_record_locals_in_expr(expr, vars);
        }
    }
}
