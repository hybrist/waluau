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
pub(super) const BIT32_BNOT: &str = "bit32.bnot";
pub(super) const BIT32_BAND: &str = "bit32.band";
pub(super) const BIT32_BOR: &str = "bit32.bor";
pub(super) const BIT32_BXOR: &str = "bit32.bxor";
pub(super) const BIT32_BTEST: &str = "bit32.btest";
pub(super) const BIT32_LROTATE: &str = "bit32.lrotate";
pub(super) const BIT32_RROTATE: &str = "bit32.rrotate";
pub(super) const BIT32_COUNTLZ: &str = "bit32.countlz";
pub(super) const BIT32_COUNTRZ: &str = "bit32.countrz";
pub(super) const TABLE_CONCAT: &str = "table.concat";
pub(super) const TO_STRING: &str = "tostring";
pub(super) const SELECT: &str = "select";
pub(super) const ASSERT: &str = "assert";
pub(super) const STRING_FIND: &str = "string.find";
pub(super) const STRING_LEN: &str = "string.len";
pub(super) const STRING_SUB: &str = "string.sub";
pub(super) const STRING_REP: &str = "string.rep";
pub(super) const STRING_BYTE: &str = "string.byte";
pub(super) const STRING_CHAR: &str = "string.char";
pub(super) const STRING_UPPER: &str = "string.upper";
pub(super) const STRING_LOWER: &str = "string.lower";
pub(super) const STRING_FORMAT: &str = "string.format";
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

pub(super) fn infer_bit32_builtin_call(
    name: &str,
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Option<Result<Type, Diagnostic>> {
    let u32_ty = Type::Numeric(NumericType::U32);
    let i32_ty = Type::Numeric(NumericType::I32);
    let result_ty = match name {
        BIT32_BNOT | BIT32_COUNTLZ | BIT32_COUNTRZ => {
            if args.len() != 1 {
                return Some(Err(Diagnostic::new(format!(
                    "{name} expects 1 argument, got {}",
                    args.len()
                ))));
            }
            u32_ty.clone()
        }
        BIT32_LROTATE | BIT32_RROTATE => {
            if args.len() != 2 {
                return Some(Err(Diagnostic::new(format!(
                    "{name} expects 2 arguments, got {}",
                    args.len()
                ))));
            }
            u32_ty.clone()
        }
        BIT32_BAND | BIT32_BOR | BIT32_BXOR => u32_ty.clone(),
        BIT32_BTEST => Type::Bool,
        _ => return None,
    };

    for (index, arg) in args.iter().enumerate() {
        let expected_arg = if matches!(name, BIT32_LROTATE | BIT32_RROTATE) && index == 1 {
            i32_ty.clone()
        } else {
            u32_ty.clone()
        };
        match super::expressions::infer_expr(
            arg,
            vars,
            fn_signatures,
            active_type_params,
            Some(expected_arg.clone()),
        ) {
            Ok(ty) if ty == expected_arg => {}
            Ok(ty) => {
                return Some(Err(Diagnostic::new(format!(
                    "{name} expects {} argument #{}, got {ty}",
                    expected_arg,
                    index + 1
                ))));
            }
            Err(error) => return Some(Err(error)),
        }
    }

    Some(coerce_type(result_ty, expected))
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
    let arg_ty = match super::expressions::infer_expr(
        &args[1],
        vars,
        fn_signatures,
        active_type_params,
        None,
    ) {
        Ok(ty) => ty,
        Err(error) => return Some(Err(error)),
    };

    if arg_ty.is_array() {
        Some(coerce_type(Type::Numeric(NumericType::I32), expected))
    } else {
        Some(Err(Diagnostic::new(format!(
            "{SELECT} expects an array, got {arg_ty}"
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
    // Infer with the expected element type so `{}` literals resolve, but fall
    // back to a plain inference when that fails so mismatches keep the
    // table.concat-specific diagnostic.
    let expected_list = Type::Array(Box::new(Type::String));
    let list_ty = super::expressions::infer_expr(
        &args[0],
        vars,
        fn_signatures,
        active_type_params,
        Some(expected_list.clone()),
    )
    .or_else(|_| {
        super::expressions::infer_expr(&args[0], vars, fn_signatures, active_type_params, None)
    });
    let list_ty = match list_ty {
        Ok(ty) => ty,
        Err(error) => return Some(Err(error)),
    };
    if list_ty != expected_list {
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
    let i32_ty = Type::Numeric(NumericType::I32);
    match name {
        STRING_FIND => {
            if args.len() < 2 || args.len() > 4 {
                return Some(Err(Diagnostic::new(format!(
                    "{STRING_FIND} expects 2 to 4 arguments, got {}",
                    args.len()
                ))));
            }
            if let Err(error) = require_arg_type(
                STRING_FIND,
                "haystack",
                &args[0],
                Type::String,
                vars,
                fn_signatures,
                active_type_params,
            ) {
                return Some(Err(error));
            }
            if let Err(error) = require_arg_type(
                STRING_FIND,
                "needle",
                &args[1],
                Type::String,
                vars,
                fn_signatures,
                active_type_params,
            ) {
                return Some(Err(error));
            }
            if let Some(init_arg) = args.get(2) {
                if let Err(error) = require_arg_type(
                    STRING_FIND,
                    "init",
                    init_arg,
                    i32_ty.clone(),
                    vars,
                    fn_signatures,
                    active_type_params,
                ) {
                    return Some(Err(error));
                }
            }
            if let Some(plain_arg) = args.get(3) {
                if let Err(error) = require_arg_type(
                    STRING_FIND,
                    "plain",
                    plain_arg,
                    Type::Bool,
                    vars,
                    fn_signatures,
                    active_type_params,
                ) {
                    return Some(Err(error));
                }
            }
            Some(coerce_type(i32_ty, expected))
        }
        STRING_LEN | STRING_UPPER | STRING_LOWER => {
            if args.len() != 1 {
                return Some(Err(Diagnostic::new(format!(
                    "{name} expects 1 argument, got {}",
                    args.len()
                ))));
            }
            if let Err(error) = require_arg_type(
                name,
                "value",
                &args[0],
                Type::String,
                vars,
                fn_signatures,
                active_type_params,
            ) {
                return Some(Err(error));
            }
            let result = if name == STRING_LEN {
                i32_ty
            } else {
                Type::String
            };
            Some(coerce_type(result, expected))
        }
        STRING_SUB => {
            if args.len() < 2 || args.len() > 3 {
                return Some(Err(Diagnostic::new(format!(
                    "{STRING_SUB} expects 2 or 3 arguments, got {}",
                    args.len()
                ))));
            }
            if let Err(error) = require_arg_type(
                STRING_SUB,
                "value",
                &args[0],
                Type::String,
                vars,
                fn_signatures,
                active_type_params,
            ) {
                return Some(Err(error));
            }
            for (index, label) in [(1, "first"), (2, "last")] {
                if let Some(arg) = args.get(index) {
                    if let Err(error) = require_arg_type(
                        STRING_SUB,
                        label,
                        arg,
                        i32_ty.clone(),
                        vars,
                        fn_signatures,
                        active_type_params,
                    ) {
                        return Some(Err(error));
                    }
                }
            }
            Some(coerce_type(Type::String, expected))
        }
        STRING_REP => {
            if args.len() < 2 || args.len() > 3 {
                return Some(Err(Diagnostic::new(format!(
                    "{STRING_REP} expects 2 or 3 arguments, got {}",
                    args.len()
                ))));
            }
            if let Err(error) = require_arg_type(
                STRING_REP,
                "value",
                &args[0],
                Type::String,
                vars,
                fn_signatures,
                active_type_params,
            ) {
                return Some(Err(error));
            }
            if let Err(error) = require_arg_type(
                STRING_REP,
                "count",
                &args[1],
                i32_ty.clone(),
                vars,
                fn_signatures,
                active_type_params,
            ) {
                return Some(Err(error));
            }
            if let Some(separator) = args.get(2) {
                if let Err(error) = require_arg_type(
                    STRING_REP,
                    "separator",
                    separator,
                    Type::String,
                    vars,
                    fn_signatures,
                    active_type_params,
                ) {
                    return Some(Err(error));
                }
            }
            Some(coerce_type(Type::String, expected))
        }
        STRING_BYTE => {
            if args.is_empty() || args.len() > 2 {
                return Some(Err(Diagnostic::new(format!(
                    "{STRING_BYTE} expects 1 or 2 arguments, got {}",
                    args.len()
                ))));
            }
            if let Err(error) = require_arg_type(
                STRING_BYTE,
                "value",
                &args[0],
                Type::String,
                vars,
                fn_signatures,
                active_type_params,
            ) {
                return Some(Err(error));
            }
            if let Some(index) = args.get(1) {
                if let Err(error) = require_arg_type(
                    STRING_BYTE,
                    "index",
                    index,
                    i32_ty.clone(),
                    vars,
                    fn_signatures,
                    active_type_params,
                ) {
                    return Some(Err(error));
                }
            }
            Some(coerce_type(i32_ty, expected))
        }
        STRING_CHAR => {
            if args.len() > 8 {
                return Some(Err(Diagnostic::new(format!(
                    "{STRING_CHAR} expects at most 8 arguments, got {}",
                    args.len()
                ))));
            }
            for arg in args {
                if let Err(error) = require_arg_type(
                    STRING_CHAR,
                    "code",
                    arg,
                    i32_ty.clone(),
                    vars,
                    fn_signatures,
                    active_type_params,
                ) {
                    return Some(Err(error));
                }
            }
            Some(coerce_type(Type::String, expected))
        }
        STRING_FORMAT => {
            if args.is_empty() || args.len() > 9 {
                return Some(Err(Diagnostic::new(format!(
                    "{STRING_FORMAT} expects 1 to 9 arguments, got {}",
                    args.len()
                ))));
            }
            if let Err(error) = require_arg_type(
                STRING_FORMAT,
                "format",
                &args[0],
                Type::String,
                vars,
                fn_signatures,
                active_type_params,
            ) {
                return Some(Err(error));
            }
            for arg in args.iter().skip(1) {
                let arg_ty = match super::expressions::infer_expr(
                    arg,
                    vars,
                    fn_signatures,
                    active_type_params,
                    None,
                ) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                if !(arg_ty.is_numeric()
                    || arg_ty == Type::Bool
                    || arg_ty == Type::String
                    || arg_ty == Type::Unknown)
                {
                    return Some(Err(Diagnostic::new(format!(
                        "{STRING_FORMAT} expects primitive format arguments, got {arg_ty}",
                    ))));
                }
            }
            Some(coerce_type(Type::String, expected))
        }
        _ => None,
    }
}

fn require_arg_type(
    builtin: &str,
    label: &str,
    arg: &Expr,
    expected: Type,
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
) -> Result<(), Diagnostic> {
    let actual = super::expressions::infer_expr(
        arg,
        vars,
        fn_signatures,
        active_type_params,
        Some(expected.clone()),
    )?;
    if actual == expected {
        Ok(())
    } else {
        Err(Diagnostic::new(format!(
            "{builtin} expects {label} to be {expected}, got {actual}"
        )))
    }
}

// infer_print_builtin_call removed - now handled via extern function declaration
