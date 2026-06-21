use std::collections::{HashMap, HashSet};

use waluau_ast::{Expr, NumericType, TaggedVariant, Type};
use waluau_diagnostics::Diagnostic;

use super::Binding;
use super::numeric::coerce_type;
use super::signatures::FnSignature;

fn is_promise_like_extern(ty: &Type) -> bool {
    match ty {
        Type::Extern | Type::ExternSubtype(_) => true,
        Type::Opaque { ty, .. } => is_promise_like_extern(ty),
        _ => false,
    }
}

pub(super) const COROUTINE_CREATE: &str = "coroutine.create";
pub(super) const COROUTINE_RESUME: &str = "coroutine.resume";
pub(super) const COROUTINE_CLOSE: &str = "coroutine.close";
pub(super) const COROUTINE_YIELD: &str = "coroutine.yield";
pub(super) const COROUTINE_AWAIT_PROMISE: &str = "coroutine.await_promise";
pub(super) const PROMISE_AWAIT: &str = "promise.await";
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
pub(super) const SELECT: &str = "select";
pub(super) const ASSERT: &str = "assert";
pub(super) const STRING_FIND: &str = "string.find";
// pub(super) const PRINT: &str = "print"; // now handled via extern declaration

fn promise_resolved_type(ty: &Type) -> Option<Type> {
    let Type::Opaque { name, ty } = ty else {
        return None;
    };
    if !is_promise_like_extern(ty) {
        return None;
    }
    let inner = name.strip_prefix("Promise<")?.strip_suffix('>')?;
    Some(match inner {
        "unit" => Type::Unit,
        "bool" => Type::Bool,
        "string" => Type::String,
        "bytes" => Type::Bytes,
        "extern" => Type::Extern,
        "unknown" => Type::Unknown,
        "thread" => Type::Thread,
        "i32" => Type::Numeric(NumericType::I32),
        "i64" => Type::Numeric(NumericType::I64),
        "u32" => Type::Numeric(NumericType::U32),
        "u64" => Type::Numeric(NumericType::U64),
        "f32" => Type::Numeric(NumericType::F32),
        "f64" => Type::Numeric(NumericType::F64),
        name => Type::Opaque {
            name: name.to_string(),
            ty: Box::new(Type::Extern),
        },
    })
}

fn infer_promise_await_arg(
    arg: &Expr,
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Result<Type, Diagnostic> {
    let promise_ty =
        super::expressions::infer_expr(arg, vars, fn_signatures, active_type_params, None)?;
    let Some(resolved_ty) = promise_resolved_type(&promise_ty) else {
        return Err(Diagnostic::new(
            "promise.await expects a Promise<T> extern value",
        ));
    };
    coerce_type(resolved_ty, expected)
}

pub(super) fn infer_promise_builtin_call(
    name: &str,
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Option<Result<Type, Diagnostic>> {
    match name {
        PROMISE_AWAIT => {
            if args.len() != 1 {
                return Some(Err(Diagnostic::new(format!(
                    "{PROMISE_AWAIT} expects 1 argument, got {}",
                    args.len()
                ))));
            }
            Some(infer_promise_await_arg(
                &args[0],
                vars,
                fn_signatures,
                active_type_params,
                expected,
            ))
        }
        _ => None,
    }
}

pub(super) fn infer_promise_await_method_call(
    receiver: &Expr,
    name: &str,
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Option<Result<Type, Diagnostic>> {
    if name != "await" {
        return None;
    }
    if !args.is_empty() {
        return Some(Err(Diagnostic::new(format!(
            "Promise<T>:await expects 0 arguments, got {}",
            args.len()
        ))));
    }
    Some(infer_promise_await_arg(
        receiver,
        vars,
        fn_signatures,
        active_type_params,
        expected,
    ))
}

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
                    ) {
                        return Some(Ok(Type::Multi(vec![Type::Bool, Type::Unknown])));
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
                Some(Type::Unknown),
            ) {
                Ok(_) => {}
                Err(error) => return Some(Err(error)),
            }
            Some(coerce_type(Type::Unit, expected))
        }
        COROUTINE_AWAIT_PROMISE => {
            if args.len() != 1 {
                return Some(Err(Diagnostic::new(format!(
                    "{COROUTINE_AWAIT_PROMISE} expects 1 argument, got {}",
                    args.len()
                ))));
            }
            let promise_ty = match super::expressions::infer_expr(
                &args[0],
                vars,
                fn_signatures,
                active_type_params,
                None,
            ) {
                Ok(ty) => ty,
                Err(error) => return Some(Err(error)),
            };
            if is_promise_like_extern(&promise_ty) {
                Some(coerce_type(Type::Unknown, expected))
            } else {
                Some(Err(Diagnostic::new(
                    "coroutine.await_promise expects an extern Promise-like value",
                )))
            }
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
    if arg_ty.is_numeric()
        || arg_ty == Type::Bool
        || arg_ty == Type::String
        || arg_ty == Type::Unknown
    {
        Some(coerce_type(Type::String, expected))
    } else {
        Some(Err(Diagnostic::new(format!(
            "{TO_STRING} expects a primitive argument (numeric, bool, or string), got {arg_ty}",
        ))))
    }
}

pub(super) fn infer_select_builtin_call(
    name: &str,
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Option<Result<Type, Diagnostic>> {
    if name != SELECT {
        return None;
    }
    if args.len() != 2 {
        return Some(Err(Diagnostic::new(format!(
            "{SELECT} expects 2 arguments, got {}",
            args.len()
        ))));
    }
    match &args[0] {
        Expr::String(marker, _) if marker == "#" => {}
        _ => {
            return Some(Err(Diagnostic::new(
                "select currently supports only select('#', ...)",
            )));
        }
    }
    match super::expressions::infer_expr(
        &args[1],
        vars,
        fn_signatures,
        active_type_params,
        Some(Type::Array(Box::new(Type::Unknown))),
    ) {
        Ok(_) => Some(coerce_type(Type::Numeric(NumericType::I32), expected)),
        Err(error) => Some(Err(error)),
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

    // string.find(haystack, needle, init?, plain?): the trailing `init` (search
    // start offset) and `plain` (plain substring search) arguments are optional.
    if args.len() < 2 || args.len() > 4 {
        return Some(Err(Diagnostic::new(format!(
            "{STRING_FIND} expects 2 to 4 arguments, got {}",
            args.len()
        ))));
    }

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

    let i32_ty = Type::Numeric(NumericType::I32);
    if let Some(init_arg) = args.get(2) {
        let init_ty = match super::expressions::infer_expr(
            init_arg,
            vars,
            fn_signatures,
            active_type_params,
            Some(i32_ty.clone()),
        ) {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        if init_ty != i32_ty {
            return Some(Err(Diagnostic::new(format!(
                "{STRING_FIND} expects init to be an i32, got {init_ty}"
            ))));
        }
    }

    if let Some(plain_arg) = args.get(3) {
        let plain_ty = match super::expressions::infer_expr(
            plain_arg,
            vars,
            fn_signatures,
            active_type_params,
            Some(Type::Bool),
        ) {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        if plain_ty != Type::Bool {
            return Some(Err(Diagnostic::new(format!(
                "{STRING_FIND} expects plain to be a bool, got {plain_ty}"
            ))));
        }
    }

    // Return type: i32 (0-based position, or -1 if not found)
    Some(coerce_type(i32_ty, expected))
}

// infer_print_builtin_call removed - now handled via extern function declaration
