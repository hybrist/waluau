use std::collections::{HashMap, HashSet};

use waluau_ast::{Expr, NumberLiteral, NumericType, Type};
use waluau_diagnostics::{Diagnostic, DiagnosticCategory};

use super::{Binding, FnSignature, inference_diagnostic};

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
        // Any value implicitly boxes into `unknown` (anyref). Unboxing back to a
        // concrete type is never implicit — it requires an explicit cast.
        Some(Type::Unknown) => Ok(Type::Unknown),
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
                name: actual_name, ..
            } if actual_name == expected_name => Ok(Type::Opaque {
                name: expected_name,
                ty: expected_ty,
            }),
            _ => Err(Diagnostic::new(format!(
                "cannot implicitly convert {} to {}",
                actual, expected_name
            ))),
        },
        Some(Type::Record(expected_fields)) => {
            let Type::Record(actual_fields) = actual else {
                let expected_record = Type::Record(expected_fields.clone());
                return Err(Diagnostic::new(format!(
                    "cannot implicitly convert {} to {}",
                    actual, expected_record
                )));
            };

            for (name, expected_ty) in &expected_fields {
                let Some(actual_ty) = actual_fields.get(name) else {
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
            Type::Extern => Err(Diagnostic::new(format!(
                "cannot implicitly convert extern to {expected_numeric}",
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
        Some(Type::Extern) => Err(Diagnostic::new(
            "numeric literal is not assignable to extern",
        )),
        Some(Type::Named { name, .. }) => Err(Diagnostic::new(format!(
            "numeric literal is not assignable to {name}",
        ))),
        Some(Type::Opaque { name, .. }) => Err(Diagnostic::new(format!(
            "numeric literal is not assignable to {name}",
        ))),
        Some(Type::Array(_)) => Err(Diagnostic::new(
            "numeric literal is not assignable to array",
        )),
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
