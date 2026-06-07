use std::collections::{HashMap, HashSet};

use waluau_ast::{Expr, NumericType, TaggedVariant, Type};
use waluau_diagnostics::Diagnostic;

use super::Binding;
use super::numeric::coerce_type;
use super::signatures::FnSignature;

pub(super) const COROUTINE_CREATE: &str = "coroutine.create";
pub(super) const COROUTINE_RESUME: &str = "coroutine.resume";
pub(super) const COROUTINE_CLOSE: &str = "coroutine.close";
pub(super) const COROUTINE_YIELD: &str = "coroutine.yield";
pub(super) const MATH_ABS: &str = "math.abs";
pub(super) const MATH_MIN: &str = "math.min";
pub(super) const MATH_MAX: &str = "math.max";
pub(super) const MATH_SQRT: &str = "math.sqrt";
pub(super) const MATH_FLOOR: &str = "math.floor";
pub(super) const MATH_CEIL: &str = "math.ceil";
pub(super) const MATH_TRUNC: &str = "math.trunc";
pub(super) const MATH_NEAREST: &str = "math.nearest";
pub(super) const MATH_COPYSIGN: &str = "math.copysign";
pub(super) const TABLE_CONCAT: &str = "table.concat";
pub(super) const TO_STRING: &str = "tostring";
pub(super) const ASSERT: &str = "assert";
pub(super) const STRING_FIND: &str = "string_find";
// pub(super) const PRINT: &str = "print"; // now handled via extern declaration

pub(super) fn infer_coroutine_builtin_call(
    name: &str,
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Option<Result<Type, Diagnostic>> {
    let i32_ty = Type::Numeric(NumericType::I32);
    match name {
        COROUTINE_CREATE => {
            if args.len() != 1 {
                return Some(Err(Diagnostic::new(format!(
                    "{COROUTINE_CREATE} expects 1 argument, got {}",
                    args.len()
                ))));
            }
            let coroutine_ty = match super::expressions::infer_expr(
                &args[0],
                vars,
                fn_signatures,
                active_type_params,
                None,
            ) {
                Ok(ty) => ty,
                Err(error) => return Some(Err(error)),
            };
            match &coroutine_ty {
                Type::Function {
                    params,
                    return_type,
                } if params.is_empty() && **return_type == i32_ty => {
                    Some(coerce_type(Type::Thread, expected))
                }
                _ => Some(Err(Diagnostic::new(
                    "coroutine.create expects a zero-argument i32-returning function",
                ))),
            }
        }
        COROUTINE_RESUME => {
            if args.len() != 1 {
                return Some(Err(Diagnostic::new(format!(
                    "{COROUTINE_RESUME} expects 1 argument, got {}",
                    args.len()
                ))));
            }
            let coroutine_ty = match super::expressions::infer_expr(
                &args[0],
                vars,
                fn_signatures,
                active_type_params,
                None,
            ) {
                Ok(ty) => ty,
                Err(error) => return Some(Err(error)),
            };
            match coroutine_ty {
                Type::Thread => {
                    if matches!(
                        expected.as_ref(),
                        Some(Type::Multi(types))
                            if types.len() == 2
                                && types[0] == Type::Bool
                                && types[1] == i32_ty
                    ) {
                        return Some(Ok(Type::Multi(vec![Type::Bool, i32_ty])));
                    }
                    Some(coerce_type(
                        Type::TaggedUnion(vec![
                            TaggedVariant {
                                tag: "Error".to_string(),
                                payload: Box::new(Type::String),
                            },
                            TaggedVariant {
                                tag: "Finished".to_string(),
                                payload: Box::new(i32_ty.clone()),
                            },
                            TaggedVariant {
                                tag: "Yielded".to_string(),
                                payload: Box::new(Type::Unknown),
                            },
                        ]),
                        expected,
                    ))
                }
                _ => Some(Err(Diagnostic::new("coroutine.resume expects a thread"))),
            }
        }
        COROUTINE_CLOSE => {
            if args.len() != 1 {
                return Some(Err(Diagnostic::new(format!(
                    "{COROUTINE_CLOSE} expects 1 argument, got {}",
                    args.len()
                ))));
            }
            let coroutine_ty = match super::expressions::infer_expr(
                &args[0],
                vars,
                fn_signatures,
                active_type_params,
                None,
            ) {
                Ok(ty) => ty,
                Err(error) => return Some(Err(error)),
            };
            match coroutine_ty {
                Type::Thread => Some(coerce_type(Type::Bool, expected)),
                _ => Some(Err(Diagnostic::new("coroutine.close expects a thread"))),
            }
        }
        COROUTINE_YIELD => {
            if args.len() != 1 {
                return Some(Err(Diagnostic::new(format!(
                    "{COROUTINE_YIELD} expects 1 argument, got {}",
                    args.len()
                ))));
            }
            match super::expressions::infer_expr(
                &args[0],
                vars,
                fn_signatures,
                active_type_params,
                Some(i32_ty.clone()),
            ) {
                Ok(ty) if ty == i32_ty => {}
                Ok(_) => {
                    return Some(Err(Diagnostic::new("coroutine.yield expects an i32 value")));
                }
                Err(error) => return Some(Err(error)),
            }
            Some(coerce_type(Type::Unit, expected))
        }
        _ => None,
    }
}

pub(super) fn infer_math_builtin_call(
    name: &str,
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Option<Result<Type, Diagnostic>> {
    let arity = match name {
        MATH_ABS | MATH_SQRT | MATH_FLOOR | MATH_CEIL | MATH_TRUNC | MATH_NEAREST => 1,
        MATH_MIN | MATH_MAX | MATH_COPYSIGN => 2,
        _ => return None,
    };
    if args.len() != arity {
        return Some(Err(Diagnostic::new(format!(
            "{name} expects {arity} argument{}, got {}",
            if arity == 1 { "" } else { "s" },
            args.len()
        ))));
    }
    let first = match super::expressions::infer_expr(
        &args[0],
        vars,
        fn_signatures,
        active_type_params,
        None,
    ) {
        Ok(ty) => ty,
        Err(error) => return Some(Err(error)),
    };
    let Type::Numeric(first_numeric) = first else {
        return Some(Err(Diagnostic::new(format!(
            "{name} expects numeric arguments"
        ))));
    };
    if arity == 2 {
        let second = match super::expressions::infer_expr(
            &args[1],
            vars,
            fn_signatures,
            active_type_params,
            Some(Type::Numeric(first_numeric)),
        ) {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        if second != Type::Numeric(first_numeric) {
            return Some(Err(Diagnostic::new(format!(
                "{name} requires both arguments to have the same numeric type"
            ))));
        }
    }
    let supports = match name {
        MATH_MIN | MATH_MAX => matches!(first_numeric, NumericType::F32 | NumericType::F64),
        MATH_ABS | MATH_SQRT | MATH_FLOOR | MATH_CEIL | MATH_TRUNC | MATH_NEAREST
        | MATH_COPYSIGN => matches!(first_numeric, NumericType::F32 | NumericType::F64),
        _ => false,
    };
    if !supports {
        return Some(Err(Diagnostic::new(format!(
            "{name} does not support {}",
            Type::Numeric(first_numeric)
        ))));
    }
    Some(coerce_type(Type::Numeric(first_numeric), expected))
}

pub(super) fn infer_tostring_builtin_call(
    name: &str,
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Option<Result<Type, Diagnostic>> {
    if name != TO_STRING {
        return None;
    }
    if args.len() != 1 {
        return Some(Err(Diagnostic::new(format!(
            "{TO_STRING} expects 1 argument, got {}",
            args.len()
        ))));
    }
    let arg_ty = match super::expressions::infer_expr(
        &args[0],
        vars,
        fn_signatures,
        active_type_params,
        None,
    ) {
        Ok(ty) => ty,
        Err(error) => return Some(Err(error)),
    };
    if arg_ty.is_numeric() || arg_ty == Type::Bool || arg_ty == Type::String {
        Some(coerce_type(Type::String, expected))
    } else {
        Some(Err(Diagnostic::new(format!(
            "{TO_STRING} expects a primitive argument (numeric, bool, or string), got {arg_ty}",
        ))))
    }
}

pub(super) fn infer_table_builtin_call(
    name: &str,
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Option<Result<Type, Diagnostic>> {
    if name != TABLE_CONCAT {
        return None;
    }
    if args.is_empty() || args.len() > 2 {
        return Some(Err(Diagnostic::new(format!(
            "{TABLE_CONCAT} expects 1 or 2 arguments, got {}",
            args.len()
        ))));
    }
    let list_ty = match super::expressions::infer_expr(
        &args[0],
        vars,
        fn_signatures,
        active_type_params,
        None,
    ) {
        Ok(ty) => ty,
        Err(error) => return Some(Err(error)),
    };
    if list_ty != Type::Array(Box::new(Type::String)) {
        return Some(Err(Diagnostic::new(format!(
            "{TABLE_CONCAT} expects an array of strings, got {list_ty}"
        ))));
    }
    if let Some(separator) = args.get(1) {
        match super::expressions::infer_expr(
            separator,
            vars,
            fn_signatures,
            active_type_params,
            None,
        ) {
            Ok(Type::String) => {}
            Ok(ty) => {
                return Some(Err(Diagnostic::new(format!(
                    "{TABLE_CONCAT} expects a string separator, got {ty}"
                ))));
            }
            Err(error) => return Some(Err(error)),
        }
    }
    Some(coerce_type(Type::String, expected))
}

pub(super) fn infer_string_builtin_call(
    name: &str,
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Option<Result<Type, Diagnostic>> {
    if name != STRING_FIND {
        return None;
    }

    // string_find expects 2 arguments: (haystack: string, needle: string)
    if args.len() != 2 {
        return Some(Err(Diagnostic::new(format!(
            "{STRING_FIND} expects 2 arguments, got {}",
            args.len()
        ))));
    }

    // Check haystack argument (string)
    let haystack_ty = match super::expressions::infer_expr(
        &args[0],
        vars,
        fn_signatures,
        active_type_params,
        Some(Type::String),
    ) {
        Ok(ty) => ty,
        Err(error) => return Some(Err(error)),
    };
    if haystack_ty != Type::String {
        return Some(Err(Diagnostic::new(format!(
            "{STRING_FIND} expects haystack to be a string, got {haystack_ty}"
        ))));
    }

    // Check needle argument (string)
    let needle_ty = match super::expressions::infer_expr(
        &args[1],
        vars,
        fn_signatures,
        active_type_params,
        Some(Type::String),
    ) {
        Ok(ty) => ty,
        Err(error) => return Some(Err(error)),
    };
    if needle_ty != Type::String {
        return Some(Err(Diagnostic::new(format!(
            "{STRING_FIND} expects needle to be a string, got {needle_ty}"
        ))));
    }

    // Return type: NotFound(unit) | Found(u32)
    let u32_ty = Type::Numeric(NumericType::U32);
    let result_type = Type::TaggedUnion(vec![
        TaggedVariant {
            tag: "NotFound".to_string(),
            payload: Box::new(Type::Unit),
        },
        TaggedVariant {
            tag: "Found".to_string(),
            payload: Box::new(u32_ty),
        },
    ]);

    Some(coerce_type(result_type, expected))
}

// infer_print_builtin_call removed - now handled via extern function declaration
