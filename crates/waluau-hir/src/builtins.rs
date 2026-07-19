use std::collections::{HashMap, HashSet};

use waluau_ast::{Expr, NumericType, TaggedVariant, Type, UnaryOp};
use waluau_diagnostics::Diagnostic;

use super::Binding;
use super::numeric::coerce_type;
use super::signatures::{FnSignature, call_arity_matches};

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
// math.* builtins are extern host functions declared in builtins/math.walu;
// overload resolution replaces the old hardcoded polymorphic logic here.
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
pub(super) const TABLE_INSERT: &str = "table.insert";
pub(super) const TABLE_REMOVE: &str = "table.remove";
pub(super) const TABLE_SORT: &str = "table.sort";
pub(super) const TABLE_GETN: &str = "table.getn";
pub(super) const TABLE_PACK: &str = "table.pack";
pub(super) const TYPE: &str = "type";
pub(super) const TYPEOF: &str = "typeof";
pub(super) const TO_STRING: &str = "tostring";
pub(super) const TO_NUMBER: &str = "tonumber";
pub(super) const SELECT: &str = "select";
pub(super) const ASSERT: &str = "assert";
pub(super) const ERROR: &str = "error";
pub(super) const PCALL: &str = "pcall";
pub(super) const STRING_FIND: &str = "string.find";
pub(super) const STRING_MATCH: &str = "string.match";
pub(super) const STRING_GMATCH: &str = "string.gmatch";
pub(super) const STRING_GSUB: &str = "string.gsub";
pub(super) const STRING_LEN: &str = "string.len";
pub(super) const STRING_SUB: &str = "string.sub";
pub(super) const STRING_REP: &str = "string.rep";
pub(super) const STRING_BYTE: &str = "string.byte";
pub(super) const STRING_CHAR: &str = "string.char";
pub(super) const STRING_UPPER: &str = "string.upper";
pub(super) const STRING_LOWER: &str = "string.lower";
pub(super) const STRING_FORMAT: &str = "string.format";
pub(super) const STRING_REVERSE: &str = "string.reverse";
pub(super) const STRING_SPLIT: &str = "string.split";
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

pub(super) fn infer_pcall_builtin_call(
    name: &str,
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Option<Result<Type, Diagnostic>> {
    if name != PCALL {
        return None;
    }
    if args.is_empty() {
        return Some(Err(Diagnostic::new("{PCALL} expects at least 1 argument")));
    }
    let callee_ty = match super::expressions::infer_expr(
        &args[0],
        vars,
        fn_signatures,
        active_type_params,
        None,
    ) {
        Ok(ty) => ty,
        Err(error) => return Some(Err(error)),
    };
    let Type::Function {
        params,
        return_type: _,
    } = callee_ty
    else {
        return Some(Err(Diagnostic::new(format!(
            "{PCALL} expects a function, got {callee_ty}"
        ))));
    };
    if !call_arity_matches(&params, args.len() - 1) {
        return Some(Err(Diagnostic::new(format!(
            "{PCALL} protected function expects {} arguments, got {}",
            params.len(),
            args.len() - 1
        ))));
    }
    for (arg, param_ty) in args.iter().skip(1).zip(params.iter()) {
        if let Err(error) = super::expressions::infer_expr(
            arg,
            vars,
            fn_signatures,
            active_type_params,
            Some(param_ty.clone()),
        )
        .and_then(|actual| coerce_type(actual, Some(param_ty.clone())))
        {
            return Some(Err(error));
        }
    }
    Some(coerce_type(
        Type::Multi(vec![Type::Bool, Type::Unknown]),
        expected,
    ))
}

pub(super) fn infer_error_builtin_call(
    name: &str,
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Option<Result<Type, Diagnostic>> {
    if name != ERROR {
        return None;
    }
    if !(1..=2).contains(&args.len()) {
        return Some(Err(Diagnostic::new(format!(
            "{ERROR} expects 1 or 2 arguments, got {}",
            args.len()
        ))));
    }
    if let Err(error) = super::expressions::infer_expr(
        &args[0],
        vars,
        fn_signatures,
        active_type_params,
        Some(Type::String),
    ) {
        return Some(Err(error));
    }
    if args.len() == 2 {
        if let Err(error) = super::expressions::infer_expr(
            &args[1],
            vars,
            fn_signatures,
            active_type_params,
            Some(Type::Numeric(NumericType::I32)),
        ) {
            return Some(Err(error));
        }
    }
    Some(Ok(expected.unwrap_or(Type::Unit)))
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

pub(super) fn infer_type_builtin_call(
    name: &str,
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Option<Result<Type, Diagnostic>> {
    if !matches!(name, TYPE | TYPEOF) {
        return None;
    }
    if args.len() != 1 {
        return Some(Err(Diagnostic::new(format!(
            "{name} expects 1 argument, got {}",
            args.len()
        ))));
    }
    match super::expressions::infer_expr(&args[0], vars, fn_signatures, active_type_params, None) {
        Ok(_) => Some(coerce_type(Type::String, expected)),
        Err(error) => Some(Err(error)),
    }
}

pub(super) fn infer_tonumber_builtin_call(
    name: &str,
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Option<Result<Type, Diagnostic>> {
    if name != TO_NUMBER {
        return None;
    }
    if args.is_empty() || args.len() > 2 {
        return Some(Err(Diagnostic::new(format!(
            "{TO_NUMBER} expects 1 or 2 arguments, got {}",
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
    if !(arg_ty.is_numeric() || arg_ty == Type::String || arg_ty == Type::Unknown) {
        return Some(Err(Diagnostic::new(format!(
            "{TO_NUMBER} expects a numeric, string, or unknown argument, got {arg_ty}",
        ))));
    }
    if let Some(base) = args.get(1) {
        if let Err(error) = require_arg_type(
            TO_NUMBER,
            "base",
            base,
            Type::Numeric(NumericType::I32),
            vars,
            fn_signatures,
            active_type_params,
        ) {
            return Some(Err(error));
        }
        if !(arg_ty == Type::String || arg_ty == Type::Unknown) {
            return Some(Err(Diagnostic::new(format!(
                "{TO_NUMBER} with a base expects a string or unknown value, got {arg_ty}",
            ))));
        }
    }
    Some(coerce_type(Type::Numeric(NumericType::F64), expected))
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
    let count_marker = matches!(&args[0], Expr::String(marker, _) if marker == "#");
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
    let Some(element_ty) = arg_ty.element_type() else {
        return Some(Err(Diagnostic::new(format!(
            "{SELECT} expects an array, got {arg_ty}"
        ))));
    };
    if count_marker {
        return Some(coerce_type(Type::Numeric(NumericType::I32), expected));
    }
    // `select(n, ...)` returns the single value at 1-based position n
    // (negative n counts from the end); positions past the end trap where
    // Lua raises "index out of range".
    let index_ty = match super::expressions::infer_expr(
        &args[0],
        vars,
        fn_signatures,
        active_type_params,
        Some(Type::Numeric(NumericType::I32)),
    ) {
        Ok(ty) => ty,
        Err(error) => return Some(Err(error)),
    };
    if index_ty != Type::Numeric(NumericType::I32) {
        return Some(Err(Diagnostic::new(format!(
            "{SELECT} expects '#' or an i32 index as its first argument, got {index_ty}"
        ))));
    }
    Some(coerce_type(element_ty, expected))
}

/// Infer the element type of the array passed as a table builtin's first argument.
fn infer_table_array_element(
    name: &str,
    arg: &Expr,
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
) -> Result<Type, Diagnostic> {
    let list_ty =
        super::expressions::infer_expr(arg, vars, fn_signatures, active_type_params, None)?;
    match list_ty {
        Type::Array(element) => Ok(*element),
        other => Err(Diagnostic::new(format!(
            "{name} expects an array as its first argument, got {other}"
        ))),
    }
}

/// `Float32Array.create(len)` (and the other typed-array kinds): allocate a
/// zero-filled typed array of `len` elements in linear memory.
pub(super) fn typed_array_create_kind(name: &str) -> Option<waluau_ast::TypedArrayKind> {
    let (type_name, member) = name.split_once('.')?;
    if member != "create" {
        return None;
    }
    waluau_ast::TypedArrayKind::from_type_name(type_name)
}

pub(super) fn infer_typed_array_builtin_call(
    name: &str,
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Option<Result<Type, Diagnostic>> {
    let kind = typed_array_create_kind(name)?;
    if args.len() != 1 {
        return Some(Err(Diagnostic::new(format!(
            "{name} expects 1 argument, got {}",
            args.len()
        ))));
    }
    // The length accepts any numeric type; lowering converts it to i32 like
    // an array index.
    let length_ty = match super::expressions::infer_expr(
        &args[0],
        vars,
        fn_signatures,
        active_type_params,
        Some(Type::Numeric(NumericType::I32)),
    )
    .or_else(|_| {
        super::expressions::infer_expr(&args[0], vars, fn_signatures, active_type_params, None)
    }) {
        Ok(ty) => ty,
        Err(error) => return Some(Err(error)),
    };
    if !length_ty.is_numeric() {
        return Some(Err(Diagnostic::new(format!(
            "{name} expects a numeric length, got {length_ty}"
        ))));
    }
    Some(coerce_type(Type::TypedArray(kind), expected))
}

pub(super) fn infer_table_builtin_call(
    name: &str,
    args: &[Expr],
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
    expected: Option<Type>,
) -> Option<Result<Type, Diagnostic>> {
    let i32_ty = Type::Numeric(NumericType::I32);
    match name {
        TABLE_PACK => {
            if args.len() > 1 {
                for arg in &args[..args.len() - 1] {
                    if matches!(arg, Expr::Vararg(_)) {
                        return Some(Err(Diagnostic::new(
                            "'...' is only supported as the last argument of a call",
                        )));
                    }
                }
            }
            for arg in args {
                if matches!(arg, Expr::Vararg(_)) {
                    continue;
                }
                if let Err(error) = super::expressions::infer_expr(
                    arg,
                    vars,
                    fn_signatures,
                    active_type_params,
                    Some(Type::Unknown),
                ) {
                    return Some(Err(error));
                }
            }
            return Some(coerce_type(Type::Array(Box::new(Type::Unknown)), expected));
        }
        TABLE_GETN => {
            if args.len() != 1 {
                return Some(Err(Diagnostic::new(format!(
                    "{TABLE_GETN} expects 1 argument, got {}",
                    args.len()
                ))));
            }
            if let Err(error) =
                infer_table_array_element(name, &args[0], vars, fn_signatures, active_type_params)
            {
                return Some(Err(error));
            }
            return Some(coerce_type(i32_ty, expected));
        }
        TABLE_INSERT => {
            if args.len() < 2 || args.len() > 3 {
                return Some(Err(Diagnostic::new(format!(
                    "{TABLE_INSERT} expects 2 or 3 arguments, got {}",
                    args.len()
                ))));
            }
            let element_ty = match infer_table_array_element(
                name,
                &args[0],
                vars,
                fn_signatures,
                active_type_params,
            ) {
                Ok(ty) => ty,
                Err(error) => return Some(Err(error)),
            };
            if args.len() == 3 {
                match super::expressions::infer_expr(
                    &args[1],
                    vars,
                    fn_signatures,
                    active_type_params,
                    Some(i32_ty.clone()),
                ) {
                    Ok(ty) if ty == i32_ty => {}
                    Ok(ty) => {
                        return Some(Err(Diagnostic::new(format!(
                            "{TABLE_INSERT} expects an i32 position, got {ty}"
                        ))));
                    }
                    Err(error) => return Some(Err(error)),
                }
            }
            let value_arg = args.last().expect("checked arity above");
            match super::expressions::infer_expr(
                value_arg,
                vars,
                fn_signatures,
                active_type_params,
                Some(element_ty.clone()),
            ) {
                Ok(ty) if ty == element_ty => {}
                Ok(ty) => {
                    return Some(Err(Diagnostic::new(format!(
                        "{TABLE_INSERT} expects a {element_ty} value, got {ty}"
                    ))));
                }
                Err(error) => return Some(Err(error)),
            }
            return Some(coerce_type(Type::Unit, expected));
        }
        TABLE_REMOVE => {
            if args.is_empty() || args.len() > 2 {
                return Some(Err(Diagnostic::new(format!(
                    "{TABLE_REMOVE} expects 1 or 2 arguments, got {}",
                    args.len()
                ))));
            }
            let element_ty = match infer_table_array_element(
                name,
                &args[0],
                vars,
                fn_signatures,
                active_type_params,
            ) {
                Ok(ty) => ty,
                Err(error) => return Some(Err(error)),
            };
            if let Some(pos_arg) = args.get(1) {
                match super::expressions::infer_expr(
                    pos_arg,
                    vars,
                    fn_signatures,
                    active_type_params,
                    Some(i32_ty.clone()),
                ) {
                    Ok(ty) if ty == i32_ty => {}
                    Ok(ty) => {
                        return Some(Err(Diagnostic::new(format!(
                            "{TABLE_REMOVE} expects an i32 position, got {ty}"
                        ))));
                    }
                    Err(error) => return Some(Err(error)),
                }
            }
            return Some(coerce_type(element_ty, expected));
        }
        TABLE_SORT => {
            if args.is_empty() || args.len() > 2 {
                return Some(Err(Diagnostic::new(format!(
                    "{TABLE_SORT} expects 1 or 2 arguments, got {}",
                    args.len()
                ))));
            }
            let element_ty = match infer_table_array_element(
                name,
                &args[0],
                vars,
                fn_signatures,
                active_type_params,
            ) {
                Ok(ty) => ty,
                Err(error) => return Some(Err(error)),
            };
            if let Some(comparator) = args.get(1) {
                let comparator_ty = Type::Function {
                    params: vec![element_ty.clone(), element_ty.clone()],
                    return_type: Box::new(Type::Bool),
                };
                match super::expressions::infer_expr(
                    comparator,
                    vars,
                    fn_signatures,
                    active_type_params,
                    Some(comparator_ty.clone()),
                ) {
                    Ok(ty) if ty == comparator_ty => {}
                    Ok(ty) => {
                        return Some(Err(Diagnostic::new(format!(
                            "{TABLE_SORT} expects a ({element_ty}, {element_ty}) -> bool comparator, got {ty}"
                        ))));
                    }
                    Err(error) => return Some(Err(error)),
                }
            } else if !element_ty.is_numeric() && element_ty != Type::String {
                return Some(Err(Diagnostic::new(format!(
                    "{TABLE_SORT} without a comparator requires numeric or string elements, got {element_ty}"
                ))));
            }
            return Some(coerce_type(Type::Unit, expected));
        }
        TABLE_CONCAT => {}
        _ => return None,
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
                "pattern",
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
            let captures = if matches!(args.get(3), Some(Expr::Bool(true, _))) {
                Vec::new()
            } else {
                waluau_ast::expr_pattern_captures(&args[1])
            };
            Some(coerce_type(
                Type::Multi(waluau_ast::string_find_result_types(&captures)),
                expected,
            ))
        }
        STRING_MATCH => {
            if args.len() < 2 || args.len() > 3 {
                return Some(Err(Diagnostic::new(format!(
                    "{STRING_MATCH} expects 2 or 3 arguments, got {}",
                    args.len()
                ))));
            }
            if let Err(error) = require_arg_type(
                STRING_MATCH,
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
                STRING_MATCH,
                "pattern",
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
                    STRING_MATCH,
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
            Some(coerce_type(
                Type::Multi(waluau_ast::string_match_result_types(
                    &waluau_ast::expr_pattern_captures(&args[1]),
                )),
                expected,
            ))
        }
        STRING_GSUB => {
            if args.len() < 3 || args.len() > 4 {
                return Some(Err(Diagnostic::new(format!(
                    "{STRING_GSUB} expects 3 or 4 arguments, got {}",
                    args.len()
                ))));
            }
            if let Err(error) = require_arg_type(
                STRING_GSUB,
                "source",
                &args[0],
                Type::String,
                vars,
                fn_signatures,
                active_type_params,
            ) {
                return Some(Err(error));
            }
            if let Err(error) = require_arg_type(
                STRING_GSUB,
                "pattern",
                &args[1],
                Type::String,
                vars,
                fn_signatures,
                active_type_params,
            ) {
                return Some(Err(error));
            }
            // The replacement is either a template string or a function of
            // the pattern's captures; its exact shape is validated during IR
            // lowering.
            let repl_ty = match super::expressions::infer_expr(
                &args[2],
                vars,
                fn_signatures,
                active_type_params,
                None,
            ) {
                Ok(ty) => ty,
                Err(error) => return Some(Err(error)),
            };
            if !matches!(repl_ty, Type::String | Type::Function { .. }) {
                return Some(Err(Diagnostic::new(format!(
                    "{STRING_GSUB} expects a string or function replacement, got {repl_ty}"
                ))));
            }
            if let Some(max_arg) = args.get(3) {
                if let Err(error) = require_arg_type(
                    STRING_GSUB,
                    "max_count",
                    max_arg,
                    i32_ty.clone(),
                    vars,
                    fn_signatures,
                    active_type_params,
                ) {
                    return Some(Err(error));
                }
            }
            Some(coerce_type(
                Type::Multi(vec![Type::String, i32_ty]),
                expected,
            ))
        }
        STRING_GMATCH => Some(Err(Diagnostic::new(format!(
            "{STRING_GMATCH} is only supported as a for-in iterator"
        )))),
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
            if args.is_empty() || args.len() > 3 {
                return Some(Err(Diagnostic::new(format!(
                    "{STRING_BYTE} expects 1 to 3 arguments, got {}",
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
            if let Some(last) = args.get(2) {
                if let Err(error) = require_arg_type(
                    STRING_BYTE,
                    "last",
                    last,
                    i32_ty.clone(),
                    vars,
                    fn_signatures,
                    active_type_params,
                ) {
                    return Some(Err(error));
                }
                let count = match string_byte_static_count(args, expected.as_ref()) {
                    Ok(count) => count,
                    Err(error) => return Some(Err(error)),
                };
                return Some(coerce_type(Type::Multi(vec![i32_ty; count]), expected));
            }
            Some(coerce_type(i32_ty, expected))
        }
        STRING_CHAR => {
            let mut arg_types = Vec::new();
            for arg in args {
                let arg_ty = match super::expressions::infer_expr(
                    arg,
                    vars,
                    fn_signatures,
                    active_type_params,
                    Some(i32_ty.clone()),
                ) {
                    Ok(ty) => ty,
                    Err(_) => match super::expressions::infer_expr(
                        arg,
                        vars,
                        fn_signatures,
                        active_type_params,
                        None,
                    ) {
                        Ok(Type::Multi(types)) => Type::Multi(types),
                        Ok(ty) => ty,
                        Err(error) => return Some(Err(error)),
                    },
                };
                match arg_ty {
                    Type::Multi(types) => arg_types.extend(types),
                    ty => arg_types.push(ty),
                }
            }
            if arg_types.len() > 16 {
                return Some(Err(Diagnostic::new(format!(
                    "{STRING_CHAR} expects at most 16 statically expanded arguments, got {}",
                    arg_types.len()
                ))));
            }
            for arg_ty in arg_types {
                if arg_ty != i32_ty {
                    return Some(Err(Diagnostic::new(format!(
                        "{STRING_CHAR} expects i32 character codes, got {arg_ty}"
                    ))));
                }
            }
            Some(coerce_type(Type::String, expected))
        }
        STRING_REVERSE => {
            if args.len() != 1 {
                return Some(Err(Diagnostic::new(format!(
                    "{STRING_REVERSE} expects 1 argument, got {}",
                    args.len()
                ))));
            }
            if let Err(error) = require_arg_type(
                STRING_REVERSE,
                "value",
                &args[0],
                Type::String,
                vars,
                fn_signatures,
                active_type_params,
            ) {
                return Some(Err(error));
            }
            Some(coerce_type(Type::String, expected))
        }
        STRING_SPLIT => {
            if args.is_empty() || args.len() > 2 {
                return Some(Err(Diagnostic::new(format!(
                    "{STRING_SPLIT} expects 1 or 2 arguments, got {}",
                    args.len()
                ))));
            }
            if let Err(error) = require_arg_type(
                STRING_SPLIT,
                "source",
                &args[0],
                Type::String,
                vars,
                fn_signatures,
                active_type_params,
            ) {
                return Some(Err(error));
            }
            if let Some(separator) = args.get(1) {
                if let Err(error) = require_arg_type(
                    STRING_SPLIT,
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
            Some(coerce_type(Type::Array(Box::new(Type::String)), expected))
        }
        STRING_FORMAT => {
            if args.is_empty() || args.len() > 101 {
                return Some(Err(Diagnostic::new(format!(
                    "{STRING_FORMAT} expects 1 to 101 arguments, got {}",
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
            let format_arg_types = match super::expressions::infer_expr_list(
                &args[1..],
                vars,
                fn_signatures,
                active_type_params,
                None,
            ) {
                Ok(types) => types,
                Err(error) => return Some(Err(error)),
            };
            if format_arg_types.len() > 100 {
                return Some(Err(Diagnostic::new(format!(
                    "{STRING_FORMAT} expects at most 100 statically expanded format arguments, got {}",
                    format_arg_types.len()
                ))));
            }
            for arg_ty in format_arg_types {
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

pub(super) fn string_byte_static_count(
    args: &[Expr],
    expected: Option<&Type>,
) -> Result<usize, Diagnostic> {
    if let Some(Type::Multi(types)) = expected {
        return Ok(types.len());
    }
    let start = args.get(1).and_then(expr_i32_literal).unwrap_or(1);
    let Some(end) = args.get(2).and_then(expr_i32_literal) else {
        return Err(Diagnostic::new(
            "string.byte range form requires literal indices or an expected multi-value arity",
        ));
    };
    let (start, end) = if let Some(len) = args.first().and_then(expr_string_len) {
        (
            normalize_lua_index(start, len),
            normalize_lua_index(end, len),
        )
    } else if start >= 1 && end >= 0 {
        (start, end)
    } else {
        return Err(Diagnostic::new(
            "string.byte range with negative indices requires a literal string",
        ));
    };
    Ok(if end < start {
        0
    } else {
        (end - start + 1) as usize
    })
}

fn expr_i32_literal(expr: &Expr) -> Option<i32> {
    match expr {
        Expr::Number(number, _) => number.raw.replace('_', "").parse::<i32>().ok(),
        Expr::Unary {
            op: UnaryOp::Neg,
            expr,
            ..
        } => expr_i32_literal(expr).and_then(i32::checked_neg),
        _ => None,
    }
}

fn expr_string_len(expr: &Expr) -> Option<i32> {
    let Expr::String(value, _) = expr else {
        return None;
    };
    i32::try_from(value.chars().count()).ok()
}

fn normalize_lua_index(index: i32, len: i32) -> i32 {
    let normalized = if index < 0 { len + index + 1 } else { index };
    normalized.clamp(1, len)
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
    if actual == expected
        || coerce_type(actual.clone(), Some(expected.clone()))
            .map(|ty| ty == expected)
            .unwrap_or(false)
    {
        Ok(())
    } else {
        Err(Diagnostic::new(format!(
            "{builtin} expects {label} to be {expected}, got {actual}"
        )))
    }
}

// infer_print_builtin_call removed - now handled via extern function declaration
