use std::collections::{HashMap, HashSet};

use waluau_ast::{Expr, NumberLiteral, NumberLiteralUnion, NumberUnionMember, NumericType, Type};
use waluau_diagnostics::{Diagnostic, DiagnosticCategory};

use super::{Binding, FnSignature, inference_diagnostic};

fn record_fields(ty: &Type) -> Option<&std::collections::BTreeMap<String, Type>> {
    match ty {
        Type::Record(fields) => Some(fields),
        Type::Opaque { ty, .. } => record_fields(ty),
        _ => None,
    }
}

pub(super) fn common_element_type(left: Type, right: Type) -> Result<Type, Diagnostic> {
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

pub(super) fn infer_numeric_common_type(
    left: &Expr,
    right: &Expr,
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Result<Type, Diagnostic> {
    let expected_numeric = match expected.map(numeric_view) {
        Some(Type::Numeric(numeric)) => Some(numeric),
        _ => None,
    };

    // Binary operands are scalar contexts. Adjust a call's multi-value result
    // before using it to type the other operand (especially a number literal).
    match (
        matches!(left, Expr::Number(..)),
        matches!(right, Expr::Number(..)),
    ) {
        (true, true) => {
            let ty = expected_numeric.unwrap_or(NumericType::F64);
            let left_ty = super::expressions::infer_expr(
                left,
                vars,
                fn_signatures,
                active_type_params,
                Some(Type::Numeric(ty)),
            )
            .map(super::expressions::first_of_multi)?;
            let right_ty = super::expressions::infer_expr(
                right,
                vars,
                fn_signatures,
                active_type_params,
                Some(Type::Numeric(ty)),
            )
            .map(super::expressions::first_of_multi)?;
            require_same_numeric(left_ty.clone(), right_ty)?;
            Ok(left_ty)
        }
        (true, false) => {
            let right_ty = super::expressions::infer_expr(
                right,
                vars,
                fn_signatures,
                active_type_params,
                None,
            )
            .map(super::expressions::first_of_multi)?;
            let left_ty = super::expressions::infer_expr(
                left,
                vars,
                fn_signatures,
                active_type_params,
                Some(numeric_view(right_ty.clone())),
            )
            .map(super::expressions::first_of_multi)?;
            common_numeric_type(left_ty, right_ty)
        }
        (false, true) => {
            let left_ty =
                super::expressions::infer_expr(left, vars, fn_signatures, active_type_params, None)
                    .map(super::expressions::first_of_multi)?;
            let right_ty = super::expressions::infer_expr(
                right,
                vars,
                fn_signatures,
                active_type_params,
                Some(numeric_view(left_ty.clone())),
            )
            .map(super::expressions::first_of_multi)?;
            common_numeric_type(left_ty, right_ty)
        }
        _ => {
            let left_ty =
                super::expressions::infer_expr(left, vars, fn_signatures, active_type_params, None)
                    .map(super::expressions::first_of_multi)?;
            let right_ty = super::expressions::infer_expr(
                right,
                vars,
                fn_signatures,
                active_type_params,
                None,
            )
            .map(super::expressions::first_of_multi)?;
            common_numeric_type(left_ty, right_ty)
        }
    }
}

/// True for numeric-for bounds that are untyped number literals (optionally
/// behind unary minus, e.g. the `-1` step of a countdown loop). Such bounds
/// carry no numeric type of their own and adopt the type of the loop's typed
/// bounds, mirroring how untyped literals behave in binary expressions.
pub(super) fn is_untyped_literal_bound(expr: &Expr) -> bool {
    match expr {
        Expr::Number(..) => true,
        Expr::Unary {
            op: waluau_ast::UnaryOp::Neg,
            expr,
            ..
        } => is_untyped_literal_bound(expr),
        _ => false,
    }
}

/// Infers the loop-variable type of a numeric `for` from its bound
/// expressions. Typed bounds are inferred first and unified; untyped literal
/// bounds then adopt that type (defaulting to f64 when every bound is an
/// untyped literal), so `for i = 0, #a - 1` iterates with an i32 loop
/// variable instead of forcing the i32 bound into an f64 comparison.
pub(super) fn infer_numeric_for_loop_type(
    bounds: &[&Expr],
    mut infer: impl FnMut(&Expr, Option<Type>) -> Result<Type, Diagnostic>,
) -> Result<Type, Diagnostic> {
    let mut typed: Option<Type> = None;
    for bound in bounds.iter().filter(|b| !is_untyped_literal_bound(b)) {
        let ty = infer(bound, None)?;
        typed = Some(match typed {
            None => ty,
            Some(prev) => common_numeric_type(prev, ty)?,
        });
    }
    let mut loop_ty = typed.unwrap_or(Type::Numeric(NumericType::F64));
    if !matches!(loop_ty, Type::Numeric(_)) {
        return Err(Diagnostic::new("numeric for-loop bounds must be numeric"));
    }
    for bound in bounds.iter().filter(|b| is_untyped_literal_bound(b)) {
        let ty = infer(bound, Some(loop_ty.clone()))?;
        loop_ty = common_numeric_type(loop_ty, ty)?;
    }
    Ok(loop_ty)
}

/// A number literal union (bare or behind its nominal alias wrapper) reads
/// as its underlying numeric type in numeric contexts (arithmetic,
/// comparisons, and as the expectation typing an untyped literal operand).
/// Membership is enforced only where a value of the union type is produced,
/// not where one is read.
pub(super) fn numeric_view(ty: Type) -> Type {
    match ty.number_literal_union() {
        Some(union) => Type::Numeric(union.numeric),
        None => ty,
    }
}

pub(super) fn common_numeric_type(left: Type, right: Type) -> Result<Type, Diagnostic> {
    let (left, right) = (numeric_view(left), numeric_view(right));
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

pub(super) fn coerce_type(actual: Type, expected: Option<Type>) -> Result<Type, Diagnostic> {
    match expected {
        None => Ok(actual),
        Some(expected) if actual == expected => Ok(expected),
        // Recursive aliases are represented by a finite opaque anchor at the
        // cycle edge. Arrays are mutable, so do not make their element types
        // generally covariant; only identify two arrays when their opaque
        // alias identities are the same all the way down.
        Some(expected) if same_opaque_array_identity(&actual, &expected) => Ok(expected),
        // A variadic pack in a scalar context contributes its first value.
        Some(expected)
            if matches!(actual, Type::Variadic(_)) && !matches!(expected, Type::Variadic(_)) =>
        {
            let Type::Variadic(element) = actual else {
                unreachable!()
            };
            coerce_type(*element, Some(expected))
        }
        // A multi-value result in a single-value context collapses to its
        // first value, following Lua's adjustment rules.
        Some(expected)
            if matches!(actual, Type::Multi(_)) && !matches!(expected, Type::Multi(_)) =>
        {
            let Type::Multi(mut parts) = actual else {
                unreachable!()
            };
            if parts.is_empty() {
                return Err(Diagnostic::new(
                    "cannot use an empty multi-value result as a value",
                ));
            }
            coerce_type(parts.remove(0), Some(expected))
        }
        // Symmetrically, a single value fills a one-slot multi-value
        // expectation. Argument inference expands the trailing slots of a call
        // into Multi for builtins like string.byte, so a single-value result
        // must still satisfy the last remaining parameter.
        Some(Type::Multi(expected_parts))
            if !matches!(actual, Type::Multi(_)) && expected_parts.len() == 1 =>
        {
            let first = expected_parts
                .into_iter()
                .next()
                .expect("len() == 1 guarantees a first element");
            coerce_type(actual, Some(first))
        }
        // Multi's Display joins its parts bare ("i32, i32"), which read as a
        // nonsense conversion in this message; spell out the shape instead.
        Some(Type::Multi(expected_parts)) if !matches!(actual, Type::Multi(_)) => {
            Err(Diagnostic::new(format!(
                "cannot implicitly convert a single {actual} to {} values ({})",
                expected_parts.len(),
                Type::Multi(expected_parts)
            )))
        }
        // Nullable values are truthy exactly when non-nil, so they coerce to
        // bool in condition positions.
        Some(Type::Bool) if matches!(actual, Type::Nullable(_)) => Ok(Type::Bool),
        // Mutable structural values safely project to read-only aliases without
        // a copy. A read-only value never flows back to a mutable expectation:
        // that one-way relation is what prevents the view from escaping.
        Some(Type::Readonly(expected_inner)) => {
            let actual_inner = if let Some(inner) = actual.readonly_base() {
                inner.clone()
            } else if actual.is_mutable_structural() {
                actual
            } else {
                return Err(Diagnostic::new(format!(
                    "cannot implicitly convert {actual} to readonly<{expected_inner}>"
                )));
            };
            coerce_type(actual_inner, Some((*expected_inner).clone())).map_err(|_| {
                Diagnostic::new(format!(
                    "cannot implicitly convert to readonly<{expected_inner}>"
                ))
            })?;
            Ok(Type::Readonly(expected_inner))
        }
        // Any value implicitly boxes into `unknown` (anyref). Symmetrically, an
        // `unknown` value (e.g. an unannotated Lua parameter) implicitly
        // unboxes to any concrete type with a runtime-checked cast, mirroring
        // Lua's dynamic typing.
        Some(Type::Unknown) if actual.contains_readonly() => Err(Diagnostic::new(
            "cannot erase a read-only view into unknown",
        )),
        Some(Type::Unknown) => Ok(Type::Unknown),
        Some(expected) if actual == Type::Unknown && expected != Type::Unit => Ok(expected),
        Some(Type::Nullable(expected_inner)) => match actual {
            Type::Nil => Ok(Type::Nullable(expected_inner)),
            Type::Nullable(actual_inner) if actual_inner == expected_inner => {
                Ok(Type::Nullable(expected_inner))
            }
            // Same-name nominal aliases identify regardless of how deep the
            // resolver expanded each side — a recursion-edge anchor
            // (`Opaque { ty: Unknown }`) and the full alias share a runtime
            // representation, so the nullable wrapper follows the name.
            Type::Nullable(ref actual_inner)
                if matches!(
                    (actual_inner.as_ref(), expected_inner.as_ref()),
                    (
                        Type::Opaque {
                            name: actual_name,
                            generic_extern: actual_generic,
                            ..
                        },
                        Type::Opaque {
                            name: expected_name,
                            generic_extern: expected_generic,
                            ..
                        },
                    ) if opaque_identity_matches(
                        actual_name,
                        actual_generic.as_deref(),
                        expected_name,
                        expected_generic.as_deref(),
                    )
                ) =>
            {
                Ok(Type::Nullable(expected_inner))
            }
            other if is_extern_subtype_of(&other, &expected_inner) => {
                Ok(Type::Nullable(expected_inner))
            }
            other => coerce_type(other.clone(), Some((*expected_inner).clone()))
                .map(|_| Type::Nullable(expected_inner.clone()))
                .map_err(|_| {
                    Diagnostic::new(format!(
                        "cannot implicitly convert {other} to {}?",
                        expected_inner
                    ))
                }),
        },
        Some(Type::TaggedVariant(expected_variant)) => match actual {
            Type::TaggedVariant(actual_variant) if actual_variant.tag == expected_variant.tag => {
                let payload = coerce_type(
                    (*actual_variant.payload).clone(),
                    Some((*expected_variant.payload).clone()),
                )?;
                Ok(Type::TaggedVariant(waluau_ast::TaggedVariant {
                    tag: expected_variant.tag,
                    payload: Box::new(payload),
                }))
            }
            other => Err(Diagnostic::new(format!(
                "cannot implicitly convert {other} to {}({})",
                expected_variant.tag, expected_variant.payload
            ))),
        },
        Some(Type::TaggedUnion(expected_variants)) => match actual {
            Type::TaggedVariant(actual_variant) => {
                let Some(expected_variant) = expected_variants
                    .iter()
                    .find(|variant| variant.tag == actual_variant.tag)
                else {
                    return Err(Diagnostic::new(format!(
                        "cannot implicitly convert {}({}) to {}",
                        actual_variant.tag,
                        actual_variant.payload,
                        Type::TaggedUnion(expected_variants)
                    )));
                };
                let payload = coerce_type(
                    (*actual_variant.payload).clone(),
                    Some((*expected_variant.payload).clone()),
                )?;
                Ok(Type::TaggedVariant(waluau_ast::TaggedVariant {
                    tag: expected_variant.tag.clone(),
                    payload: Box::new(payload),
                }))
            }
            Type::TaggedUnion(actual_variants)
                if actual_variants.len() == expected_variants.len() =>
            {
                for expected_variant in &expected_variants {
                    let Some(actual_variant) = actual_variants
                        .iter()
                        .find(|variant| variant.tag == expected_variant.tag)
                    else {
                        return Err(Diagnostic::new(format!(
                            "cannot implicitly convert {} to {}",
                            Type::TaggedUnion(actual_variants),
                            Type::TaggedUnion(expected_variants)
                        )));
                    };
                    let _ = coerce_type(
                        Type::TaggedVariant(actual_variant.clone()),
                        Some(Type::TaggedVariant(expected_variant.clone())),
                    )?;
                }
                Ok(Type::TaggedUnion(expected_variants))
            }
            other => Err(Diagnostic::new(format!(
                "cannot implicitly convert {other} to {}",
                Type::TaggedUnion(expected_variants)
            ))),
        },
        // A string literal union is an enum in disguise: values come only
        // from member literals (checked at the literal site) or from another
        // union whose members are a subset, possibly behind a nominal alias.
        // Plain strings never convert in, and the union never converts out
        // to `string`.
        Some(Type::StringLiteralUnion(expected_members)) => match actual.string_literal_union() {
            Some(actual_members)
                if actual_members
                    .iter()
                    .all(|member| expected_members.contains(member)) =>
            {
                Ok(Type::StringLiteralUnion(expected_members))
            }
            _ => Err(Diagnostic::new(format!(
                "cannot implicitly convert {actual} to {}; only its member \
                 literals produce a value of this type",
                Type::StringLiteralUnion(expected_members)
            ))),
        },
        Some(Type::NumberLiteralUnion(expected_union)) => match actual.number_literal_union() {
            Some(actual_union)
                if actual_union.numeric == expected_union.numeric
                    && actual_union
                        .members
                        .iter()
                        .all(|member| expected_union.contains(*member)) =>
            {
                Ok(Type::NumberLiteralUnion(expected_union))
            }
            _ => Err(Diagnostic::new(format!(
                "cannot implicitly convert {actual} to {}; use a member \
                 literal or an explicit cast",
                Type::NumberLiteralUnion(expected_union)
            ))),
        },
        Some(Type::Opaque {
            name: expected_name,
            ty: expected_ty,
            generic_extern: expected_generic,
        }) => match actual {
            Type::Record(_) if expected_ty.as_ref() == &Type::Unknown => {
                Err(Diagnostic::new(format!(
                    "cannot construct opaque type '{}' outside its defining module",
                    super::module_type_display_name(&expected_name)
                )))
            }
            Type::Opaque {
                name: actual_name,
                ty: actual_ty,
                generic_extern: actual_generic,
            } if opaque_identity_matches(
                &actual_name,
                actual_generic.as_deref(),
                &expected_name,
                expected_generic.as_deref(),
            ) || extern_opaque_is_subtype(
                &actual_name,
                actual_generic.as_deref(),
                actual_ty.as_ref(),
                &expected_name,
                expected_generic.as_deref(),
            ) =>
            {
                Ok(Type::Opaque {
                    name: expected_name,
                    ty: expected_ty,
                    generic_extern: expected_generic,
                })
            }
            Type::Opaque {
                ty: ref actual_ty, ..
            } if matches!(
                (
                    record_fields(actual_ty.as_ref()),
                    record_fields(expected_ty.as_ref())
                ),
                (Some(_), Some(_))
            ) =>
            {
                let _ = coerce_type(*actual_ty.clone(), Some(*expected_ty.clone()))?;
                Ok(Type::Opaque {
                    name: expected_name,
                    ty: expected_ty,
                    generic_extern: expected_generic,
                })
            }
            // A tagged-union constructor produces a TaggedVariant; allow it to be
            // assigned to an Opaque alias whose inner type is a TaggedUnion or TaggedVariant.
            Type::TaggedVariant(ref actual_variant) => match expected_ty.as_ref() {
                Type::TaggedUnion(_) | Type::TaggedVariant(_) => {
                    let inner = coerce_type(actual, Some(*expected_ty))?;
                    Ok(Type::Opaque {
                        name: expected_name,
                        ty: Box::new(inner),
                        generic_extern: expected_generic,
                    })
                }
                _ => Err(Diagnostic::new(format!(
                    "cannot implicitly convert {} to {}",
                    actual_variant.tag, expected_name
                ))),
            },
            // coroutine.resume (and similar) produce a bare TaggedUnion; allow it to
            // be assigned to a named alias whose underlying type is that union.
            Type::TaggedUnion(ref actual_variants) => match expected_ty.as_ref() {
                Type::TaggedUnion(_) => {
                    let actual_union = Type::TaggedUnion(actual_variants.clone());
                    let inner = coerce_type(actual_union, Some(*expected_ty))?;
                    Ok(Type::Opaque {
                        name: expected_name,
                        ty: Box::new(inner),
                        generic_extern: expected_generic,
                    })
                }
                _ => Err(Diagnostic::new(format!(
                    "cannot implicitly convert {} to {}",
                    Type::TaggedUnion(actual_variants.clone()),
                    expected_name
                ))),
            },
            Type::Record(_) if matches!(expected_ty.as_ref(), Type::Record(_)) => {
                let _ = coerce_type(actual, Some(*expected_ty.clone()))?;
                Ok(Type::Opaque {
                    name: expected_name,
                    ty: expected_ty,
                    generic_extern: expected_generic,
                })
            }
            other if matches!(expected_ty.as_ref(), Type::Readonly(_)) => {
                let _ = coerce_type(other, Some((*expected_ty).clone()))?;
                Ok(Type::Opaque {
                    name: expected_name,
                    ty: expected_ty,
                    generic_extern: expected_generic,
                })
            }
            // An inline literal union value (from an un-aliased annotation
            // like `c: "red" | "black"`) flows into a nominal alias whose
            // underlying union accepts its members.
            Type::StringLiteralUnion(_) | Type::NumberLiteralUnion(_)
                if matches!(
                    expected_ty.as_ref(),
                    Type::StringLiteralUnion(_) | Type::NumberLiteralUnion(_)
                ) =>
            {
                let inner = coerce_type(actual, Some(*expected_ty))?;
                Ok(Type::Opaque {
                    name: expected_name,
                    ty: Box::new(inner),
                    generic_extern: expected_generic,
                })
            }
            _ => Err(Diagnostic::new(format!(
                "cannot implicitly convert {} to {}",
                super::module_type_display(&actual),
                super::module_type_display_name(&expected_name)
            ))),
        },
        Some(Type::Record(expected_fields)) => {
            let Some(actual_fields) = record_fields(&actual).cloned() else {
                let expected_record = Type::Record(expected_fields.clone());
                return Err(Diagnostic::new(format!(
                    "cannot implicitly convert {} to {}",
                    super::module_type_display(&actual),
                    super::module_type_display(&expected_record)
                )));
            };

            for (name, expected_ty) in &expected_fields {
                let Some(actual_ty) = actual_fields.get(name) else {
                    if expected_ty.accepts_nil() {
                        continue;
                    }
                    return Err(Diagnostic::new(format!("missing record field '{}'", name)));
                };
                // Each field coerces independently, so e.g. an `i32` value boxes
                // into an `unknown` field.
                coerce_type(actual_ty.clone(), Some(expected_ty.clone())).map_err(|_| {
                    Diagnostic::new(format!(
                        "record field '{}' expects {}, got {}",
                        name,
                        super::module_type_display(expected_ty),
                        super::module_type_display(actual_ty)
                    ))
                })?;
            }
            for name in actual_fields.keys() {
                if !expected_fields.contains_key(name) {
                    return Err(Diagnostic::new(format!(
                        "unexpected record field '{}'",
                        name
                    )));
                }
            }

            Ok(Type::Record(expected_fields))
        }
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
            Type::Unit => Err(Diagnostic::new(format!(
                "cannot implicitly convert unit to {expected_numeric}",
            ))),
            Type::String => Err(Diagnostic::new(format!(
                "cannot implicitly convert string to {expected_numeric}",
            ))),
            Type::Bytes => Err(Diagnostic::new(format!(
                "cannot implicitly convert bytes to {expected_numeric}",
            ))),
            Type::Extern | Type::ExternSubtype(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert extern to {expected_numeric}",
            ))),
            Type::Nil => Err(Diagnostic::new(format!(
                "cannot implicitly convert nil to {expected_numeric}",
            ))),
            Type::Nullable(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert nullable value to {expected_numeric}",
            ))),
            Type::Named { name, .. } => Err(Diagnostic::new(format!(
                "cannot implicitly convert {name} to {expected_numeric}",
            ))),
            // A constrained numeric alias reads as its underlying type.
            ref union_ty @ Type::Opaque { .. }
                if union_ty.number_literal_union().is_some_and(|union| {
                    union.numeric.can_implicitly_widen_to(expected_numeric)
                }) =>
            {
                Ok(Type::Numeric(expected_numeric))
            }
            Type::Opaque { name, .. } => Err(Diagnostic::new(format!(
                "cannot implicitly convert {name} to {expected_numeric}",
            ))),
            Type::Array(_) | Type::Variadic(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert array to {expected_numeric}",
            ))),
            Type::TypedArray(kind) => Err(Diagnostic::new(format!(
                "cannot implicitly convert {} to {expected_numeric}",
                kind.type_name(),
            ))),
            Type::Multi(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert multiple values to {expected_numeric}",
            ))),
            Type::Function { .. } => Err(Diagnostic::new(format!(
                "cannot implicitly convert function to {expected_numeric}",
            ))),
            Type::Record(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert namespace to {expected_numeric}",
            ))),
            Type::TypeParam(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert generic type parameter to {expected_numeric}",
            ))),
            Type::Thread => Err(Diagnostic::new(format!(
                "cannot implicitly convert thread to {expected_numeric}",
            ))),
            Type::Unknown => Err(Diagnostic::new(format!(
                "cannot implicitly convert unknown to {expected_numeric}; use an explicit cast",
            ))),
            // A constrained numeric alias reads as its underlying type.
            Type::NumberLiteralUnion(union)
                if union.numeric.can_implicitly_widen_to(expected_numeric) =>
            {
                Ok(Type::Numeric(expected_numeric))
            }
            Type::NumberLiteralUnion(union) => Err(Diagnostic::new(format!(
                "cannot implicitly convert {} to {expected_numeric}",
                union.numeric,
            ))),
            Type::StringLiteralUnion(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert {actual} to {expected_numeric}",
            ))),
            Type::TaggedVariant(_) | Type::TaggedUnion(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert {actual} to {expected_numeric}",
            ))),
            Type::Readonly(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert {actual} to {expected_numeric}",
            ))),
        },
        Some(Type::Bool) => Err(Diagnostic::new(format!(
            "cannot implicitly convert {actual} to bool",
        ))),
        Some(Type::Unit) => Err(Diagnostic::new(format!(
            "cannot implicitly convert {actual} to unit",
        ))),
        Some(expected) => Err(Diagnostic::new(format!(
            "cannot implicitly convert {} to {}",
            super::module_type_display(&actual),
            super::module_type_display(&expected)
        ))),
    }
}

fn same_opaque_array_identity(actual: &Type, expected: &Type) -> bool {
    match (actual, expected) {
        (
            Type::Opaque {
                name: actual_name,
                generic_extern: actual_generic,
                ..
            },
            Type::Opaque {
                name: expected_name,
                generic_extern: expected_generic,
                ..
            },
        ) => opaque_identity_matches(
            actual_name,
            actual_generic.as_deref(),
            expected_name,
            expected_generic.as_deref(),
        ),
        (Type::Array(actual), Type::Array(expected)) => {
            same_opaque_array_identity(actual, expected)
        }
        _ => false,
    }
}

pub(super) fn is_extern_subtype_of(actual: &Type, expected: &Type) -> bool {
    if actual == expected {
        return true;
    }
    let (
        Type::Opaque {
            name: actual_name,
            ty: actual_ty,
            generic_extern: actual_generic,
        },
        Type::Opaque {
            name: expected_name,
            generic_extern: expected_generic,
            ..
        },
    ) = (actual, expected)
    else {
        return false;
    };
    extern_opaque_is_subtype(
        actual_name,
        actual_generic.as_deref(),
        actual_ty,
        expected_name,
        expected_generic.as_deref(),
    )
}

fn opaque_identity_matches(
    actual_name: &str,
    actual_generic: Option<&waluau_ast::GenericExternType>,
    expected_name: &str,
    expected_generic: Option<&waluau_ast::GenericExternType>,
) -> bool {
    match (actual_generic, expected_generic) {
        (Some(actual), Some(expected)) => {
            actual.constructor == expected.constructor && actual.type_args == expected.type_args
        }
        (None, None) => actual_name == expected_name,
        _ => false,
    }
}

fn extern_opaque_is_subtype(
    actual_name: &str,
    actual_generic: Option<&waluau_ast::GenericExternType>,
    actual_ty: &Type,
    expected_name: &str,
    expected_generic: Option<&waluau_ast::GenericExternType>,
) -> bool {
    if opaque_identity_matches(actual_name, actual_generic, expected_name, expected_generic) {
        return true;
    }
    let mut current = actual_ty;
    loop {
        match current {
            Type::ExternSubtype(parent) => match parent.as_ref() {
                Type::Opaque {
                    name,
                    ty,
                    generic_extern,
                } => {
                    if opaque_identity_matches(
                        name,
                        generic_extern.as_deref(),
                        expected_name,
                        expected_generic,
                    ) {
                        return true;
                    }
                    current = ty;
                }
                _ => return false,
            },
            _ => return false,
        }
    }
}

pub(super) fn require_numeric_cast(actual: Type, target: Type) -> Result<(), Diagnostic> {
    // Number literal unions cast like their numeric representation, giving
    // computed values an explicit route back into the constrained alias
    // (`(volume + 1) :: Volume`). String literal unions deliberately do not
    // appear here: no cast connects them to `string`.
    if actual.contains_readonly() && target == Type::Unknown {
        return Err(Diagnostic::new(
            "cannot erase a read-only view into unknown",
        ));
    }
    let (actual, target) = (numeric_view(actual), numeric_view(target));
    match (&actual, &target) {
        (Type::Opaque { ty, .. }, target) if ty.as_ref() == target => Ok(()),
        (actual, Type::Opaque { ty, .. }) if actual == ty.as_ref() => Ok(()),
        // Boxing into / unboxing out of `unknown` (anyref) is an explicit cast.
        (_, Type::Unknown) | (Type::Unknown, _) => Ok(()),
        _ => match (actual, target) {
            (Type::Numeric(_), Type::Numeric(_)) => Ok(()),
            _ => Err(Diagnostic::new(
                "casts require numeric source and destination types",
            )),
        },
    }
}

pub(super) fn require_bool_pair(left: Type, right: Type) -> Result<(), Diagnostic> {
    if left == Type::Bool && right == Type::Bool {
        Ok(())
    } else {
        Err(Diagnostic::new("operation requires bool operands"))
    }
}

pub(super) fn resolve_number_literal(
    value: &NumberLiteral,
    expected: Option<Type>,
) -> Result<Type, Diagnostic> {
    // A member literal is the only expression form (besides an explicit
    // cast) that produces a number literal union value; the union may sit
    // behind its nominal alias wrapper and a nullable.
    if let Some(expected_ty) = &expected {
        let union_inner = match expected_ty {
            Type::Nullable(inner) => inner.number_literal_union(),
            other => other.number_literal_union(),
        };
        if let Some(union) = union_inner {
            require_number_union_member(value, false, union)?;
            return Ok(expected_ty.clone());
        }
    }
    match expected {
        Some(Type::Numeric(numeric)) => {
            validate_numeric_literal(value, numeric)?;
            Ok(Type::Numeric(numeric))
        }
        Some(Type::Bool) => Err(Diagnostic::new("numeric literal is not assignable to bool")),
        Some(Type::Unit) => Err(Diagnostic::new("numeric literal is not assignable to unit")),
        Some(Type::String) => Err(Diagnostic::new(
            "numeric literal is not assignable to string",
        )),
        Some(Type::Bytes) => Err(Diagnostic::new(
            "numeric literal is not assignable to bytes",
        )),
        Some(Type::Extern) | Some(Type::ExternSubtype(_)) => Err(Diagnostic::new(
            "numeric literal is not assignable to extern",
        )),
        Some(Type::Nil) => Err(Diagnostic::new("numeric literal is not assignable to nil")),
        Some(Type::Nullable(inner)) => match *inner {
            Type::Numeric(numeric) => Ok(Type::Nullable(Box::new(Type::Numeric(numeric)))),
            Type::NumberLiteralUnion(union) => {
                require_number_union_member(value, false, &union)?;
                Ok(Type::Nullable(Box::new(Type::NumberLiteralUnion(union))))
            }
            other => Err(Diagnostic::new(format!(
                "numeric literal is not assignable to {other}?",
            ))),
        },
        Some(Type::Named { name, .. }) => Err(Diagnostic::new(format!(
            "numeric literal is not assignable to {name}",
        ))),
        Some(Type::Opaque { name, .. }) => Err(Diagnostic::new(format!(
            "numeric literal is not assignable to {name}",
        ))),
        Some(Type::Readonly(_)) => Err(Diagnostic::new(
            "numeric literal is not assignable to a read-only structural type",
        )),
        Some(Type::Array(_) | Type::Variadic(_)) => Err(Diagnostic::new(
            "numeric literal is not assignable to array",
        )),
        Some(Type::TypedArray(kind)) => Err(Diagnostic::new(format!(
            "numeric literal is not assignable to {}",
            kind.type_name(),
        ))),
        Some(Type::Function { .. }) => Err(Diagnostic::new(
            "numeric literal is not assignable to function",
        )),
        Some(Type::Record(_)) => Err(Diagnostic::new(
            "numeric literal is not assignable to namespace",
        )),
        Some(Type::Multi(_)) => Err(Diagnostic::new(
            "numeric literal is not assignable to multiple values",
        )),
        Some(Type::TypeParam(_)) => Err(Diagnostic::new(
            "numeric literal is not assignable to generic type parameter",
        )),
        Some(Type::Thread) => Err(Diagnostic::new(
            "numeric literal is not assignable to thread",
        )),
        // A bare literal boxed into `unknown` takes its default numeric type; the
        // surrounding coercion then boxes that value into anyref.
        Some(Type::Unknown) => Ok(Type::number()),
        Some(Type::TaggedVariant(_)) | Some(Type::TaggedUnion(_)) => Err(Diagnostic::new(
            "numeric literal is not assignable to tagged union type",
        )),
        Some(Type::NumberLiteralUnion(union)) => {
            require_number_union_member(value, false, &union)?;
            Ok(Type::NumberLiteralUnion(union))
        }
        Some(expected @ Type::StringLiteralUnion(_)) => Err(Diagnostic::new(format!(
            "numeric literal is not assignable to {expected}",
        ))),
        None => Ok(Type::number()),
    }
}

/// Checks that a number literal (negated when it sits under unary minus)
/// names a member of `union`. Values compare exactly: integers as integers,
/// floats by their f64 value.
pub(super) fn require_number_union_member(
    value: &NumberLiteral,
    negated: bool,
    union: &NumberLiteralUnion,
) -> Result<(), Diagnostic> {
    let member = match union.numeric {
        NumericType::F32 | NumericType::F64 => value
            .float_value()
            .map(|value| NumberUnionMember::float(if negated { -value } else { value })),
        _ => value.int_value().and_then(|value| {
            let value = if negated { -value } else { value };
            i64::try_from(value).ok().map(NumberUnionMember::Int)
        }),
    };
    if member.is_some_and(|member| union.contains(member)) {
        Ok(())
    } else {
        Err(Diagnostic::new(format!(
            "{}{} is not a member of {}",
            if negated { "-" } else { "" },
            value.raw,
            Type::NumberLiteralUnion(union.clone())
        )))
    }
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
    let raw = value.raw.replace('_', "");
    if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        return u128::from_str_radix(hex, 16)
            .map(|value| value as f64)
            .map_err(|_| Diagnostic::new("invalid number literal"));
    }
    raw.parse::<f64>()
        .map_err(|_| Diagnostic::new("invalid number literal"))
}

fn parse_integer_literal<T>(value: &NumberLiteral, ty_name: &str) -> Result<T, Diagnostic>
where
    T: std::str::FromStr + TryFrom<u128>,
{
    let raw = value.raw.replace('_', "");
    if raw.contains('.') {
        return Err(Diagnostic::new(format!(
            "numeric literal must be an integer for {ty_name}",
        )));
    }

    if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        let value = u128::from_str_radix(hex, 16).map_err(|_| {
            Diagnostic::new(format!("numeric literal is out of range for {ty_name}"))
        })?;
        return T::try_from(value).map_err(|_| {
            Diagnostic::new(format!("numeric literal is out of range for {ty_name}"))
        });
    }

    raw.parse::<T>()
        .map_err(|_| Diagnostic::new(format!("numeric literal is out of range for {ty_name}")))
}
