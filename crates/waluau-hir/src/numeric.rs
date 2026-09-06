use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use waluau_ast::{Expr, NumberLiteral, NumericType, Type};
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
    let expected_numeric = match expected {
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
                Some(right_ty.clone()),
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
                Some(left_ty.clone()),
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

pub(super) fn common_numeric_type(left: Type, right: Type) -> Result<Type, Diagnostic> {
    match (left, right) {
        // Untyped Lua parameters retain `unknown` until a numeric operator
        // gives them a runtime contract. Luau has one number type, so checked
        // dynamic operands specialize to f64. This remains local to numeric
        // operators; equality and other `unknown` uses stay dynamic.
        (Type::Unknown, Type::Unknown)
        | (Type::Unknown, Type::Numeric(_))
        | (Type::Numeric(_), Type::Unknown) => Ok(Type::number()),
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
        // Recursive aliases are represented by a finite opaque anchor at the
        // cycle edge. Arrays are mutable, so do not make their element types
        // generally covariant; only identify two arrays when their opaque
        // alias identities are the same all the way down.
        Some(expected) if same_opaque_array_identity(&actual, &expected) => Ok(expected),
        Some(expected) if actual == expected => Ok(expected),
        // Pack to pack: elements are boxed at runtime either way, so the
        // packs unify whenever their element types coerce (an annotated
        // `f64...` returns through a `unknown...` signature, and a forwarded
        // `unknown...` re-narrows at a typed vararg boundary).
        Some(Type::Variadic(expected_element)) if matches!(actual, Type::Variadic(_)) => {
            let Type::Variadic(actual_element) = actual else {
                unreachable!()
            };
            coerce_type(
                actual_element.as_ref().clone(),
                Some((*expected_element).clone()),
            )
            .map(|_| Type::Variadic(expected_element.clone()))
            .map_err(|_| {
                Diagnostic::new(format!(
                    "cannot implicitly convert {actual_element}... to {expected_element}..."
                ))
            })
        }
        // A variadic pack in a scalar context contributes its first value.
        Some(expected)
            if matches!(actual, Type::Variadic(_)) && !matches!(expected, Type::Variadic(_)) =>
        {
            let Type::Variadic(element) = actual else {
                unreachable!()
            };
            coerce_type(Arc::unwrap_or_clone(element), Some(expected))
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
        // The same adjustment at tuple width: a multi-value expression wider
        // than its context drops the trailing values. `local ok, message =
        // pcall(f)` asks for two of the values a protected call to a
        // multi-result `f` produces.
        Some(Type::Multi(expected_parts)) if multi_truncates_to(&actual, &expected_parts) => {
            let Type::Multi(actual_parts) = actual else {
                unreachable!()
            };
            actual_parts
                .into_iter()
                .zip(expected_parts)
                .map(|(actual_ty, expected_ty)| coerce_type(actual_ty, Some(expected_ty)))
                .collect::<Result<Vec<_>, _>>()
                .map(Type::Multi)
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
        // Any value implicitly boxes into `unknown` (anyref). Symmetrically, an
        // `unknown` value (e.g. an unannotated Lua parameter) implicitly
        // unboxes to any concrete type with a runtime-checked cast, mirroring
        // Lua's dynamic typing.
        Some(Type::Unknown) => Ok(Type::Unknown),
        // An interface method slot (`(self, ...) -> R` record field) holds a
        // *bound* method: a closure whose callable signature is the field's
        // signature with the receiver already applied. A plain function value
        // of that self-less shape therefore fills the slot (this is how the
        // conformance coercion builds interface records), and reading the
        // slot produces a value of the self-less shape. Any other value is
        // rejected. This arm sits before the `unknown` unboxing arm so a
        // dynamic value cannot sneak past the receiver contract.
        Some(Type::Function {
            params: expected_params,
            return_type: expected_return,
            has_self: expected_self,
        }) if matches!(
            &actual,
            Type::Function {
                params: actual_params,
                return_type: actual_return,
                ..
            } if same_function_signature_identity(
                actual_params,
                actual_return,
                &expected_params,
                &expected_return,
            )
        ) =>
        {
            Ok(Type::Function {
                params: expected_params,
                return_type: expected_return,
                has_self: expected_self,
            })
        }
        Some(expected @ Type::Function { has_self: true, .. }) => Err(Diagnostic::new(format!(
            "cannot provide {} for the method type {}: interface method slots \
             hold bound methods, created by coercing a value whose type \
             declares conformance (type T = Interface & {{ ... }})",
            super::module_type_display(&actual),
            super::module_type_display(&expected),
        ))),
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
            // A nullable enum erases into the reserved nullable `enum?` the
            // same way its non-nullable value does (see the expected-Opaque
            // arm below): both sides box the i32 ordinal identically.
            Type::Nullable(ref actual_inner)
                if matches!(
                    expected_inner.as_ref(),
                    Type::Opaque { name, .. } if name == super::ANY_ENUM_TYPE_NAME
                ) && matches!(
                    actual_inner.as_ref(),
                    Type::Opaque { ty, .. } if ty.is_numeric()
                ) =>
            {
                Ok(Type::Nullable(expected_inner))
            }
            // A nullable actual converts to a different nullable expectation
            // only through the explicit arms above (same inner, alias
            // identity, extern subtype, enum erasure). Falling through would
            // let the whole nullable re-enter scalar coercions — e.g. every
            // `X?` reads as truthy `bool`, which must not make `X?` an
            // implicit `bool?`.
            other @ Type::Nullable(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert {other} to {}?",
                expected_inner
            ))),
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
                    payload: Arc::new(payload),
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
                    payload: Arc::new(payload),
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
        Some(Type::Opaque {
            name: expected_name,
            ty: expected_ty,
            generic_extern: expected_generic,
        }) => match actual {
            // The reserved `enum` type accepts a value of any nominal enum
            // (an opaque numeric alias); the value keeps its i32 ordinal
            // representation. The conversion is one-way: an `enum` value
            // never flows back into a specific enum type.
            Type::Opaque {
                ty: ref actual_ty, ..
            } if expected_name == super::ANY_ENUM_TYPE_NAME && actual_ty.is_numeric() => {
                Ok(Type::Opaque {
                    name: expected_name,
                    ty: expected_ty,
                    generic_extern: expected_generic,
                })
            }
            Type::Record(_) if expected_ty.as_ref() == &Type::Unknown => {
                Err(Diagnostic::new(format!(
                    "cannot construct opaque type '{}' outside its defining module",
                    super::module_type_display_name(&expected_name)
                )))
            }
            Type::Function { .. } if matches!(expected_ty.as_ref(), Type::Function { .. }) => {
                let _ = coerce_type(actual, Some(expected_ty.as_ref().clone()))?;
                Ok(Type::Opaque {
                    name: expected_name,
                    ty: expected_ty,
                    generic_extern: expected_generic,
                })
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
                let _ = coerce_type(
                    actual_ty.as_ref().clone(),
                    Some(expected_ty.as_ref().clone()),
                )?;
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
                    let inner = coerce_type(actual, Some(expected_ty.as_ref().clone()))?;
                    Ok(Type::Opaque {
                        name: expected_name,
                        ty: waluau_ast::OpaquePayload::new(inner),
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
                    let inner = coerce_type(actual_union, Some(expected_ty.as_ref().clone()))?;
                    Ok(Type::Opaque {
                        name: expected_name,
                        ty: waluau_ast::OpaquePayload::new(inner),
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
                let _ = coerce_type(actual, Some(expected_ty.as_ref().clone()))?;
                Ok(Type::Opaque {
                    name: expected_name,
                    ty: expected_ty,
                    generic_extern: expected_generic,
                })
            }
            // An inline literal union value (from an un-aliased annotation
            // like `c: "red" | "black"`) flows into a nominal alias whose
            // underlying union accepts its members.
            Type::StringLiteralUnion(_)
                if matches!(expected_ty.as_ref(), Type::StringLiteralUnion(_)) =>
            {
                let inner = coerce_type(actual, Some(expected_ty.as_ref().clone()))?;
                Ok(Type::Opaque {
                    name: expected_name,
                    ty: waluau_ast::OpaquePayload::new(inner),
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

            for (name, expected_ty) in expected_fields.iter() {
                let Some(actual_ty) = actual_fields.get(name) else {
                    if expected_ty.accepts_nil() {
                        continue;
                    }
                    return Err(Diagnostic::new(format!("missing record field '{}'", name)));
                };
                // Each field coerces independently, so e.g. an `i32` value boxes
                // into an `unknown` field.
                coerce_type(actual_ty.clone(), Some(expected_ty.clone())).map_err(|inner| {
                    // The method-type arm explains the 'self' receiver and the
                    // missing conformance support; keep that message intact.
                    if matches!(expected_ty, Type::Function { has_self: true, .. }) {
                        return inner;
                    }
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
            Type::Buffer => Err(Diagnostic::new(format!(
                "cannot implicitly convert buffer to {expected_numeric}",
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
            Type::StringLiteralUnion(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert {actual} to {expected_numeric}",
            ))),
            Type::TaggedVariant(_) | Type::TaggedUnion(_) => Err(Diagnostic::new(format!(
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

/// Whether a multi-value result adjusts to a narrower multi-value context by
/// dropping its trailing values. Packs (`Variadic`) forward a runtime-sized
/// pack and keep their own expansion rules, so they never truncate here.
pub(super) fn multi_truncates_to(actual: &Type, expected_parts: &[Type]) -> bool {
    let Type::Multi(actual_parts) = actual else {
        return false;
    };
    actual_parts.len() > expected_parts.len()
        && !actual_parts
            .iter()
            .any(|ty| matches!(ty, Type::Variadic(_)))
        && !expected_parts
            .iter()
            .any(|ty| matches!(ty, Type::Variadic(_)))
}

fn same_opaque_array_identity(actual: &Type, expected: &Type) -> bool {
    matches!(
        (actual, expected),
        (Type::Array(_), Type::Array(_)) | (Type::Opaque { .. }, Type::Opaque { .. })
    ) && same_resolved_type_identity(actual, expected)
}

fn same_resolved_type_identity(actual: &Type, expected: &Type) -> bool {
    same_resolved_type_identity_cached(actual, expected, &mut HashSet::new())
}

fn same_function_signature_identity(
    actual_params: &[Type],
    actual_return: &Type,
    expected_params: &[Type],
    expected_return: &Type,
) -> bool {
    if actual_params.len() != expected_params.len() {
        return false;
    }
    let mut compared = HashSet::new();
    actual_params
        .iter()
        .zip(expected_params)
        .all(|(actual, expected)| {
            same_resolved_type_identity_cached(actual, expected, &mut compared)
        })
        && same_resolved_type_identity_cached(actual_return, expected_return, &mut compared)
}

fn same_resolved_type_identity_cached(
    actual: &Type,
    expected: &Type,
    compared: &mut HashSet<(*const Type, *const Type)>,
) -> bool {
    if std::ptr::eq(actual, expected) {
        return true;
    }
    if !compared.insert((std::ptr::from_ref(actual), std::ptr::from_ref(expected))) {
        return true;
    }
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
        (Type::ExternSubtype(actual), Type::ExternSubtype(expected))
        | (Type::Nullable(actual), Type::Nullable(expected))
        | (Type::Array(actual), Type::Array(expected))
        | (Type::Variadic(actual), Type::Variadic(expected)) => {
            same_resolved_type_identity_cached(actual, expected, compared)
        }
        (Type::Multi(actual), Type::Multi(expected)) => {
            actual.len() == expected.len()
                && actual.iter().zip(expected).all(|(actual, expected)| {
                    same_resolved_type_identity_cached(actual, expected, compared)
                })
        }
        (
            Type::Function {
                params: actual_params,
                return_type: actual_return,
                has_self: actual_self,
            },
            Type::Function {
                params: expected_params,
                return_type: expected_return,
                has_self: expected_self,
            },
        ) => {
            actual_self == expected_self
                && actual_params.len() == expected_params.len()
                && actual_params
                    .iter()
                    .zip(expected_params)
                    .all(|(actual, expected)| {
                        same_resolved_type_identity_cached(actual, expected, compared)
                    })
                && same_resolved_type_identity_cached(actual_return, expected_return, compared)
        }
        (Type::Record(actual), Type::Record(expected)) => {
            actual.len() == expected.len()
                && actual.iter().all(|(name, actual)| {
                    expected.get(name).is_some_and(|expected| {
                        same_resolved_type_identity_cached(actual, expected, compared)
                    })
                })
        }
        (Type::TaggedVariant(actual), Type::TaggedVariant(expected)) => {
            actual.tag == expected.tag
                && same_resolved_type_identity_cached(&actual.payload, &expected.payload, compared)
        }
        (Type::TaggedUnion(actual), Type::TaggedUnion(expected)) => {
            actual.len() == expected.len()
                && actual.iter().zip(expected).all(|(actual, expected)| {
                    actual.tag == expected.tag
                        && same_resolved_type_identity_cached(
                            &actual.payload,
                            &expected.payload,
                            compared,
                        )
                })
        }
        _ => actual == expected,
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
    // String literal unions deliberately do not appear here: no cast
    // connects them to `string`.
    match (&actual, &target) {
        (Type::Opaque { ty, .. }, target) if ty.as_ref() == target => Ok(()),
        // Numeric-backed nominal enums may be explicitly viewed as any
        // numeric representation (`kind::i32`, `kind::number`, and so on).
        // The cast remains explicit, so this does not make enum values
        // implicitly interchangeable with numbers.
        (Type::Opaque { ty, .. }, Type::Numeric(_)) if ty.is_numeric() => Ok(()),
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
        Some(Type::Buffer) => Err(Diagnostic::new(
            "numeric literal is not assignable to buffer",
        )),
        Some(Type::Extern) | Some(Type::ExternSubtype(_)) => Err(Diagnostic::new(
            "numeric literal is not assignable to extern",
        )),
        Some(Type::Nil) => Err(Diagnostic::new("numeric literal is not assignable to nil")),
        Some(Type::Nullable(inner)) => match Arc::unwrap_or_clone(inner) {
            Type::Numeric(numeric) => Ok(Type::Nullable(Arc::new(Type::Numeric(numeric)))),
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
        Some(expected @ Type::StringLiteralUnion(_)) => Err(Diagnostic::new(format!(
            "numeric literal is not assignable to {expected}",
        ))),
        None => Ok(Type::number()),
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
    if let Some(binary) = raw.strip_prefix("0b").or_else(|| raw.strip_prefix("0B")) {
        return u128::from_str_radix(binary, 2)
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
    if let Some(binary) = raw.strip_prefix("0b").or_else(|| raw.strip_prefix("0B")) {
        let value = u128::from_str_radix(binary, 2).map_err(|_| {
            Diagnostic::new(format!("numeric literal is out of range for {ty_name}"))
        })?;
        return T::try_from(value).map_err(|_| {
            Diagnostic::new(format!("numeric literal is out of range for {ty_name}"))
        });
    }

    raw.parse::<T>()
        .map_err(|_| Diagnostic::new(format!("numeric literal is out of range for {ty_name}")))
}
