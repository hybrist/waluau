use std::collections::{HashMap, HashSet};

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
            )?;
            let right_ty = super::expressions::infer_expr(
                right,
                vars,
                fn_signatures,
                active_type_params,
                Some(Type::Numeric(ty)),
            )?;
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
            )?;
            let left_ty = super::expressions::infer_expr(
                left,
                vars,
                fn_signatures,
                active_type_params,
                Some(right_ty.clone()),
            )?;
            common_numeric_type(left_ty, right_ty)
        }
        (false, true) => {
            let left_ty = super::expressions::infer_expr(
                left,
                vars,
                fn_signatures,
                active_type_params,
                None,
            )?;
            let right_ty = super::expressions::infer_expr(
                right,
                vars,
                fn_signatures,
                active_type_params,
                Some(left_ty.clone()),
            )?;
            common_numeric_type(left_ty, right_ty)
        }
        _ => {
            let left_ty = super::expressions::infer_expr(
                left,
                vars,
                fn_signatures,
                active_type_params,
                None,
            )?;
            let right_ty = super::expressions::infer_expr(
                right,
                vars,
                fn_signatures,
                active_type_params,
                None,
            )?;
            common_numeric_type(left_ty, right_ty)
        }
    }
}

pub(super) fn common_numeric_type(left: Type, right: Type) -> Result<Type, Diagnostic> {
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
        // Any value implicitly boxes into `unknown` (anyref). Symmetrically, an
        // `unknown` value (e.g. an unannotated Lua parameter) implicitly
        // unboxes to any concrete type with a runtime-checked cast, mirroring
        // Lua's dynamic typing.
        Some(Type::Unknown) => Ok(Type::Unknown),
        Some(expected) if actual == Type::Unknown && expected != Type::Unit => Ok(expected),
        Some(Type::Nullable(expected_inner)) => match actual {
            Type::Nil => Ok(Type::Nullable(expected_inner)),
            Type::Nullable(actual_inner) if actual_inner == expected_inner => {
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
        Some(Type::Opaque {
            name: expected_name,
            ty: expected_ty,
        }) => match actual {
            Type::Opaque {
                name: actual_name,
                ty: actual_ty,
            } if actual_name == expected_name
                || extern_opaque_is_subtype(&actual_name, actual_ty.as_ref(), &expected_name) =>
            {
                Ok(Type::Opaque {
                    name: expected_name,
                    ty: expected_ty,
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
                })
            }
            _ => Err(Diagnostic::new(format!(
                "cannot implicitly convert {} to {}",
                actual, expected_name
            ))),
        },
        Some(Type::Record(expected_fields)) => {
            let Some(actual_fields) = record_fields(&actual).cloned() else {
                let expected_record = Type::Record(expected_fields.clone());
                return Err(Diagnostic::new(format!(
                    "cannot implicitly convert {} to {}",
                    actual, expected_record
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
                        name, expected_ty, actual_ty
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
            Type::Opaque { name, .. } => Err(Diagnostic::new(format!(
                "cannot implicitly convert {name} to {expected_numeric}",
            ))),
            Type::Array(_) => Err(Diagnostic::new(format!(
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
            "cannot implicitly convert {actual} to {expected}",
        ))),
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
        },
        Type::Opaque {
            name: expected_name,
            ..
        },
    ) = (actual, expected)
    else {
        return false;
    };
    extern_opaque_is_subtype(actual_name, actual_ty, expected_name)
}

fn extern_opaque_is_subtype(actual_name: &str, actual_ty: &Type, expected_name: &str) -> bool {
    if actual_name == expected_name {
        return true;
    }
    let mut current = actual_ty;
    loop {
        match current {
            Type::ExternSubtype(parent) => match parent.as_ref() {
                Type::Opaque { name, ty } => {
                    if name == expected_name {
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
    match (&actual, &target) {
        (Type::Opaque { ty, .. }, target) if ty.as_ref() == target => Ok(()),
        (actual, Type::Opaque { name: _, ty }) if actual == ty.as_ref() => Ok(()),
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
        Some(Type::Extern) | Some(Type::ExternSubtype(_)) => Err(Diagnostic::new(
            "numeric literal is not assignable to extern",
        )),
        Some(Type::Nil) => Err(Diagnostic::new("numeric literal is not assignable to nil")),
        Some(Type::Nullable(inner)) => match *inner {
            Type::Numeric(numeric) => Ok(Type::Nullable(Box::new(Type::Numeric(numeric)))),
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
        Some(Type::Array(_)) => Err(Diagnostic::new(
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
