//! Signatures for compiler intrinsics referenced as first-class values.
//!
//! Declared browser-host imports become ordinary function values through a
//! synthesized adapter. Compiler intrinsics — the builtins the compiler lowers
//! itself instead of importing — get the same treatment whenever their
//! signature is representable as a fixed-arity Waluau function type. This
//! module is the single source of truth for which intrinsics qualify and with
//! which parameter and return types, so type checking and IR lowering cannot
//! disagree.
//!
//! Two shapes are needed:
//!
//! * an explicit function type is available (`local f: (string) -> string =
//!   string.upper`), so the signature is validated against it, and
//! * only the arguments of a protected call are available
//!   (`pcall(string.upper, text)`), so the signature is derived from them.
//!
//! Intrinsics whose result arity or element types depend on values rather than
//! types — `select`, `pcall`, `table.pack`, `table.unpack`, `string.format`
//! and the iterator protocol — are deliberately excluded and report why
//! instead of falling through to `unknown name`.

use std::sync::Arc;

use waluau_ast::{NumericType, Type};

fn i32_ty() -> Type {
    Type::Numeric(NumericType::I32)
}

fn u32_ty() -> Type {
    Type::Numeric(NumericType::U32)
}

fn array_of(element: Type) -> Type {
    Type::Array(Arc::new(element))
}

fn function_ty(params: Vec<Type>, return_type: Type) -> Type {
    Type::Function {
        params,
        return_type: Arc::new(return_type),
        has_self: false,
    }
}

/// Explains why a known intrinsic cannot become a function value. Callers use
/// this to replace the `unknown name 'select'` fallback with a diagnostic that
/// names the actual limitation.
pub fn non_representable_intrinsic_reason(name: &str) -> Option<&'static str> {
    Some(match name {
        "pcall" => "its result arity depends on the protected function",
        "select" => "its result arity depends on its first argument",
        "table.pack" | "table.unpack" | "unpack" => "its arity depends on the table's length",
        "string.format" => "its arity and parameter types depend on the format string",
        "string.gmatch" | "next" | "ipairs" | "pairs" => {
            "it is only supported as a for-in iterator"
        }
        _ => return None,
    })
}

/// A replacer function `string.gsub` accepts as a value. A value reference has
/// no literal pattern, so the host walks the match with a single string
/// capture and the replacer either returns the replacement or keeps the match.
fn is_gsub_replacement_function(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Function { params, return_type, has_self: false }
            if params.as_slice() == [Type::String]
                && matches!(return_type.as_ref(), Type::String | Type::Unit)
    )
}

/// The value signature of `name` at `arity` arguments. `generic` supplies the
/// one type a signature can vary in — the element type of the element-generic
/// `table.*` members, and the replacer type of `string.gsub` — and is ignored
/// by every other intrinsic.
fn canonical_signature(
    name: &str,
    arity: usize,
    generic: Option<&Type>,
) -> Option<(Vec<Type>, Type)> {
    let element = generic;
    let signature = match name {
        "type" | "typeof" | "tostring" if arity == 1 => (vec![Type::Unknown], Type::String),
        "print" if arity == 1 => (vec![Type::String], Type::Unit),
        "tonumber" if (1..=2).contains(&arity) => (
            vec![Type::Unknown, i32_ty()][..arity].to_vec(),
            Type::Nullable(Arc::new(Type::Numeric(NumericType::F64))),
        ),
        // `assert` has no expression form: its direct lowering is a statement
        // that traps, so as a value it returns nothing.
        "assert" if (1..=2).contains(&arity) => {
            (vec![Type::Bool, Type::String][..arity].to_vec(), Type::Unit)
        }
        "error" if (1..=2).contains(&arity) => {
            (vec![Type::String, i32_ty()][..arity].to_vec(), Type::Unit)
        }
        "string.len" if arity == 1 => (vec![Type::String], i32_ty()),
        "string.upper" | "string.lower" | "string.reverse" if arity == 1 => {
            (vec![Type::String], Type::String)
        }
        "string.byte" if (1..=2).contains(&arity) => (
            vec![Type::String, i32_ty()][..arity].to_vec(),
            Type::Nullable(Arc::new(i32_ty())),
        ),
        "string.char" if arity >= 1 => (vec![i32_ty(); arity], Type::String),
        "string.sub" if (2..=3).contains(&arity) => (
            vec![Type::String, i32_ty(), i32_ty()][..arity].to_vec(),
            Type::String,
        ),
        "string.rep" if (2..=3).contains(&arity) => (
            vec![Type::String, i32_ty(), Type::String][..arity].to_vec(),
            Type::String,
        ),
        "string.split" if (1..=2).contains(&arity) => (
            vec![Type::String, Type::String][..arity].to_vec(),
            array_of(Type::String),
        ),
        // The pattern-matching builtins return a statically shaped multi-value
        // whose arity depends on the pattern text. A value reference has no
        // literal pattern, so the result stays `unknown`.
        "string.find" if (2..=4).contains(&arity) => (
            vec![Type::String, Type::String, i32_ty(), Type::Bool][..arity].to_vec(),
            Type::Unknown,
        ),
        "string.match" if (2..=3).contains(&arity) => (
            vec![Type::String, Type::String, i32_ty()][..arity].to_vec(),
            Type::Unknown,
        ),
        "string.gsub" if (3..=4).contains(&arity) => {
            let replacement = generic
                .filter(|ty| is_gsub_replacement_function(ty))
                .cloned()
                .unwrap_or(Type::String);
            (
                vec![Type::String, Type::String, replacement, i32_ty()][..arity].to_vec(),
                Type::Unknown,
            )
        }
        "table.getn" if arity == 1 => (vec![array_of(element?.clone())], i32_ty()),
        "table.insert" if (2..=3).contains(&arity) => {
            let element = element?.clone();
            let mut params = vec![array_of(element.clone())];
            if arity == 3 {
                params.push(i32_ty());
            }
            params.push(element);
            (params, Type::Unit)
        }
        "table.remove" if (1..=2).contains(&arity) => {
            let element = element?.clone();
            let mut params = vec![array_of(element.clone())];
            if arity == 2 {
                params.push(i32_ty());
            }
            (params, element)
        }
        "table.concat" if (1..=4).contains(&arity) => (
            vec![array_of(Type::String), Type::String, i32_ty(), i32_ty()][..arity].to_vec(),
            Type::String,
        ),
        "table.sort" if (1..=2).contains(&arity) => {
            let element = element?.clone();
            let mut params = vec![array_of(element.clone())];
            if arity == 2 {
                params.push(function_ty(vec![element.clone(), element], Type::Bool));
            }
            (params, Type::Unit)
        }
        "table.create" if (1..=2).contains(&arity) => {
            let element = element?.clone();
            let mut params = vec![i32_ty()];
            if arity == 2 {
                params.push(element.clone());
            }
            (params, array_of(element))
        }
        "bit32.bnot" | "bit32.byteswap" | "bit32.countlz" | "bit32.countrz" if arity == 1 => {
            (vec![u32_ty()], u32_ty())
        }
        "bit32.lrotate" | "bit32.rrotate" | "bit32.lshift" | "bit32.rshift" | "bit32.arshift"
            if arity == 2 =>
        {
            (vec![u32_ty(), i32_ty()], u32_ty())
        }
        "bit32.extract" if (2..=3).contains(&arity) => (
            vec![u32_ty(), i32_ty(), i32_ty()][..arity].to_vec(),
            u32_ty(),
        ),
        "bit32.replace" if (3..=4).contains(&arity) => (
            vec![u32_ty(), u32_ty(), i32_ty(), i32_ty()][..arity].to_vec(),
            u32_ty(),
        ),
        "bit32.band" | "bit32.bor" | "bit32.bxor" if arity >= 1 => {
            (vec![u32_ty(); arity], u32_ty())
        }
        "bit32.btest" if arity >= 1 => (vec![u32_ty(); arity], Type::Bool),
        _ => return None,
    };
    Some(signature)
}

/// Whether `name` names a compiler intrinsic that has at least one value
/// signature. `table.insert` qualifies even though a specific reference may
/// still be rejected for its element type or arity.
pub fn is_value_representable_intrinsic(name: &str) -> bool {
    (0..=5).any(|arity| canonical_signature(name, arity, Some(&Type::Unknown)).is_some())
}

/// The single value signature of `name`, when it has exactly one. These
/// intrinsics need no annotation to be used as values; every other one keeps
/// the existing rule that an explicit function type, or a protected call's
/// argument list, has to pick the arity.
pub fn unique_intrinsic_value_signature(name: &str) -> Option<(Vec<Type>, Type)> {
    let mut found = None;
    for arity in 0..=5 {
        // No element type is supplied, so the element-generic members produce
        // no candidate at all and correctly stay ambiguous.
        let Some(signature) = canonical_signature(name, arity, None) else {
            continue;
        };
        if found.is_some() {
            return None;
        }
        found = Some(signature);
    }
    found
}

/// The parameter and return types an intrinsic takes when it is referenced as
/// a value and only the call arguments are known. `arg_types` holds each
/// argument's independently inferred type, or `None` where inference needed an
/// expectation the caller could not supply.
pub fn intrinsic_value_signature_for_arguments(
    name: &str,
    arg_types: &[Option<Type>],
) -> Option<(Vec<Type>, Type)> {
    // Most signatures vary in the element type of their array argument.
    // `table.create` reads it from the fill value instead, and `string.gsub`
    // varies in its replacement argument rather than an element type.
    let generic = match name {
        "table.create" => arg_types.get(1).cloned().flatten(),
        "string.gsub" => arg_types.get(2).cloned().flatten(),
        _ => arg_types
            .first()
            .and_then(Option::as_ref)
            .and_then(Type::element_type),
    };
    canonical_signature(name, arg_types.len(), generic.as_ref())
}

/// Whether an explicit function type is one of the value signatures of `name`.
/// Returns `None` when `name` is not a value-representable intrinsic at all,
/// so callers keep their existing fallbacks.
pub fn intrinsic_value_signature_matches(
    name: &str,
    params: &[Type],
    return_type: &Type,
) -> Option<bool> {
    if !is_value_representable_intrinsic(name) {
        return None;
    }
    // The varying type is read back out of the requested signature: from the
    // array parameter for the in-place members, from the result for
    // `table.create`, which builds a fresh array, and from the replacement
    // parameter for `string.gsub`.
    let generic = match name {
        "table.create" => return_type.element_type(),
        "string.gsub" => params.get(2).cloned(),
        _ => params.first().and_then(Type::element_type),
    };
    let Some((expected_params, expected_return)) =
        canonical_signature(name, params.len(), generic.as_ref())
    else {
        return Some(false);
    };
    Some(params == expected_params && matches_return(name, return_type, &expected_return))
}

/// `string.byte` narrows its nullable result when the caller demands a plain
/// number, so both spellings are accepted as value signatures.
fn matches_return(name: &str, requested: &Type, canonical: &Type) -> bool {
    requested == canonical || (name == "string.byte" && requested == &i32_ty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_explicit_signature_for_fixed_arity_intrinsics() {
        assert_eq!(
            intrinsic_value_signature_matches("string.upper", &[Type::String], &Type::String),
            Some(true)
        );
        assert_eq!(
            intrinsic_value_signature_matches("string.upper", &[Type::String], &i32_ty()),
            Some(false)
        );
        assert_eq!(
            intrinsic_value_signature_matches("string.len", &[Type::String], &i32_ty()),
            Some(true)
        );
        assert_eq!(
            intrinsic_value_signature_matches(
                "string.rep",
                &[Type::String, i32_ty()],
                &Type::String
            ),
            Some(true)
        );
    }

    #[test]
    fn matches_element_generic_table_signatures() {
        let strings = array_of(Type::String);
        assert_eq!(
            intrinsic_value_signature_matches(
                "table.insert",
                &[strings.clone(), Type::String],
                &Type::Unit
            ),
            Some(true)
        );
        assert_eq!(
            intrinsic_value_signature_matches("table.insert", &[strings, Type::Bool], &Type::Unit),
            Some(false)
        );
        assert_eq!(
            intrinsic_value_signature_matches("table.create", &[i32_ty()], &array_of(Type::String)),
            Some(true)
        );
    }

    #[test]
    fn derives_signatures_from_protected_call_arguments() {
        assert_eq!(
            intrinsic_value_signature_for_arguments(
                "table.insert",
                &[Some(array_of(Type::String)), Some(Type::String)]
            ),
            Some((vec![array_of(Type::String), Type::String], Type::Unit))
        );
        assert_eq!(
            intrinsic_value_signature_for_arguments(
                "string.rep",
                &[Some(Type::String), Some(i32_ty())]
            ),
            Some((vec![Type::String, i32_ty()], Type::String))
        );
    }

    #[test]
    fn infers_unambiguous_intrinsic_values_without_an_annotation() {
        assert_eq!(
            unique_intrinsic_value_signature("string.upper"),
            Some((vec![Type::String], Type::String))
        );
        assert_eq!(
            unique_intrinsic_value_signature("print"),
            Some((vec![Type::String], Type::Unit))
        );
        // Several arities, an element-generic member, and a variadic one all
        // stay ambiguous.
        assert_eq!(unique_intrinsic_value_signature("string.sub"), None);
        assert_eq!(unique_intrinsic_value_signature("table.insert"), None);
        assert_eq!(unique_intrinsic_value_signature("string.char"), None);
        assert_eq!(unique_intrinsic_value_signature("assert"), None);
    }

    #[test]
    fn reports_non_representable_intrinsics() {
        assert!(non_representable_intrinsic_reason("select").is_some());
        assert!(non_representable_intrinsic_reason("table.unpack").is_some());
        assert!(non_representable_intrinsic_reason("string.upper").is_none());
        assert!(!is_value_representable_intrinsic("select"));
    }

    #[test]
    fn leaves_unrelated_names_alone() {
        assert_eq!(
            intrinsic_value_signature_matches("math.abs", &[Type::Unknown], &Type::Unknown),
            None
        );
    }
}
