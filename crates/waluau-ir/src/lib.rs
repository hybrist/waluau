use std::collections::{BTreeMap, HashMap, HashSet};

use waluau_ast::{
    AssignOp, BinaryOp, Expr, Function as AstFunction, NumberLiteral, NumericType, Program, Stmt,
    SymbolId, TaggedVariant, Type, TypeDeclaration, TypedArrayKind, UnaryOp,
};
use waluau_diagnostics::{Diagnostic, DiagnosticCategory};

const COROUTINE_CREATE: &str = "coroutine.create";
const COROUTINE_RESUME: &str = "coroutine.resume";
const COROUTINE_CLOSE: &str = "coroutine.close";
const COROUTINE_YIELD: &str = "coroutine.yield";
const COROUTINE_AWAIT_PROMISE: &str = "coroutine.await_promise";
const PROMISE_AWAIT: &str = "promise.await";
const BIT32_BNOT: &str = "bit32.bnot";
const BIT32_BAND: &str = "bit32.band";
const BIT32_BOR: &str = "bit32.bor";
const BIT32_BXOR: &str = "bit32.bxor";
const BIT32_BTEST: &str = "bit32.btest";
const BIT32_LROTATE: &str = "bit32.lrotate";
const BIT32_RROTATE: &str = "bit32.rrotate";
const BIT32_COUNTLZ: &str = "bit32.countlz";
const BIT32_COUNTRZ: &str = "bit32.countrz";
const TABLE_CONCAT: &str = "table.concat";
const TABLE_INSERT: &str = "table.insert";
const TABLE_REMOVE: &str = "table.remove";
const TABLE_SORT: &str = "table.sort";
const TABLE_GETN: &str = "table.getn";
const TABLE_PACK: &str = "table.pack";
const TABLE_CREATE: &str = "table.create";
const TABLE_UNPACK: &str = "table.unpack";
const TYPE: &str = "type";
const TYPEOF: &str = "typeof";
const TO_STRING: &str = "tostring";
const TO_NUMBER: &str = "tonumber";
const SELECT: &str = "select";
const ASSERT: &str = "assert";
const ERROR: &str = "error";
const PCALL: &str = "pcall";
const PRINT: &str = "print";
const STRING_FIND: &str = "string.find";
const STRING_MATCH: &str = "string.match";
const STRING_GMATCH: &str = "string.gmatch";
const STRING_GSUB: &str = "string.gsub";
const PM_FIND_HOST: &str = "pm_find";
const PM_MATCH_HOST: &str = "pm_match";
const PM_MATCH_START_HOST: &str = "pm_match_start";
const PM_MATCH_END_HOST: &str = "pm_match_end";
const PM_CAPTURE_STRING_HOST: &str = "pm_capture_string";
const PM_CAPTURE_POSITION_HOST: &str = "pm_capture_position";
const PM_GSUB_HOST: &str = "pm_gsub";
const PM_GSUB_COUNT_HOST: &str = "pm_gsub_count";
const PM_GSUB_BEGIN_HOST: &str = "pm_gsub_begin";
const PM_GSUB_NEXT_HOST: &str = "pm_gsub_next";
const PM_GSUB_REPLACE_HOST: &str = "pm_gsub_replace";
const PM_GSUB_KEEP_HOST: &str = "pm_gsub_keep";
const PM_GSUB_FINISH_HOST: &str = "pm_gsub_finish";
const PM_GMATCH_HOST: &str = "pm_gmatch";
const PM_GMATCH_NEXT_HOST: &str = "pm_gmatch_next";
const STRING_LEN: &str = "string.len";
const STRING_LEN_HOST: &str = "string_len";
const STRING_SUB: &str = "string.sub";
const STRING_SUB_HOST: &str = "string_sub";
const STRING_REP: &str = "string.rep";
const STRING_REP_HOST: &str = "string_rep";
const STRING_BYTE: &str = "string.byte";
const STRING_BYTE_HOST: &str = "string_byte";
const STRING_CHAR: &str = "string.char";
const STRING_CHAR_HOST_PREFIX: &str = "string_char";
const STRING_UPPER: &str = "string.upper";
const STRING_UPPER_HOST: &str = "string_upper";
const STRING_LOWER: &str = "string.lower";
const STRING_LOWER_HOST: &str = "string_lower";
const STRING_FORMAT: &str = "string.format";
const STRING_FORMAT_HOST_PREFIX: &str = "string_format";
const STRING_REVERSE: &str = "string.reverse";
const STRING_REVERSE_HOST: &str = "string_reverse";
const STRING_SPLIT: &str = "string.split";
const STRING_SPLIT_HOST: &str = "string_split";
const STRING_SPLIT_GET_HOST: &str = "string_split_get";
const JSON_PACK: &str = "json.pack";
const JSON_UNPACK: &str = "json.unpack";
const JSON_PACK_RESET_HOST: &str = "__json_pack_reset";
const JSON_PACK_NULL_HOST: &str = "__json_pack_null";
const JSON_PACK_BOOL_HOST: &str = "__json_pack_bool";
const JSON_PACK_I32_HOST: &str = "__json_pack_i32";
const JSON_PACK_U32_HOST: &str = "__json_pack_u32";
const JSON_PACK_I64_HOST: &str = "__json_pack_i64";
const JSON_PACK_U64_HOST: &str = "__json_pack_u64";
const JSON_PACK_F32_HOST: &str = "__json_pack_f32";
const JSON_PACK_F64_HOST: &str = "__json_pack_f64";
const JSON_PACK_STRING_HOST: &str = "__json_pack_string";
const JSON_PACK_BYTES_HOST: &str = "__json_pack_bytes";
const JSON_PACK_ARRAY_HOST: &str = "__json_pack_array";
const JSON_PACK_ARRAY_PUSH_HOST: &str = "__json_pack_array_push";
const JSON_PACK_OBJECT_HOST: &str = "__json_pack_object";
const JSON_PACK_OBJECT_SET_HOST: &str = "__json_pack_object_set";
const JSON_PACK_FINISH_HOST: &str = "__json_pack_finish";
const JSON_UNPACK_START_HOST: &str = "__json_unpack_start";
const JSON_UNPACK_IS_NULL_HOST: &str = "__json_unpack_is_null";
const JSON_UNPACK_BOOL_HOST: &str = "__json_unpack_bool";
const JSON_UNPACK_I32_HOST: &str = "__json_unpack_i32";
const JSON_UNPACK_U32_HOST: &str = "__json_unpack_u32";
const JSON_UNPACK_I64_HOST: &str = "__json_unpack_i64";
const JSON_UNPACK_U64_HOST: &str = "__json_unpack_u64";
const JSON_UNPACK_F32_HOST: &str = "__json_unpack_f32";
const JSON_UNPACK_F64_HOST: &str = "__json_unpack_f64";
const JSON_UNPACK_STRING_HOST: &str = "__json_unpack_string";
const JSON_UNPACK_BYTES_HOST: &str = "__json_unpack_bytes";
const JSON_UNPACK_ARRAY_LEN_HOST: &str = "__json_unpack_array_len";
const JSON_UNPACK_ARRAY_GET_HOST: &str = "__json_unpack_array_get";
const JSON_UNPACK_OBJECT_GET_HOST: &str = "__json_unpack_object_get";
const JSON_UNPACK_VARIANT_IS_HOST: &str = "__json_unpack_variant_is";

fn is_promise_like_extern(ty: &Type) -> bool {
    match ty {
        Type::Extern | Type::ExternSubtype(_) => true,
        Type::Opaque { ty, .. } => is_promise_like_extern(ty),
        _ => false,
    }
}

fn inference_diagnostic(
    code: &'static str,
    category: DiagnosticCategory,
    message: impl Into<String>,
    action: impl Into<String>,
) -> Diagnostic {
    Diagnostic::new(message)
        .with_code(code)
        .with_category(category)
        .with_action(action)
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

/// Static 0-based element indices produced by a `table.unpack` call, from
/// (in priority order) literal bounds, a statically sized array argument, or
/// the expected multi-value arity. Waluau arrays are 0-based, so the bounds
/// are too: the defaults are `first = 0` and `last = #a - 1`. Mirrors the
/// HIR-side helper of the same name.
fn table_unpack_static_indices(
    args: &[Expr],
    expected: Option<&Type>,
) -> Result<Vec<i32>, Diagnostic> {
    let start = match args.get(1) {
        None => 0,
        Some(arg) => expr_i32_literal(arg).ok_or_else(|| {
            Diagnostic::new(
                "table.unpack requires literal bounds (runtime-variable table.unpack is not supported yet)",
            )
        })?,
    };
    if start < 0 {
        return Err(Diagnostic::new(
            "table.unpack bounds are 0-based and must be non-negative",
        ));
    }
    if let Some(last) = args.get(2) {
        let end = expr_i32_literal(last).ok_or_else(|| {
            Diagnostic::new(
                "table.unpack requires literal bounds (runtime-variable table.unpack is not supported yet)",
            )
        })?;
        return Ok(if end < start {
            Vec::new()
        } else {
            (start..=end).collect()
        });
    }
    if let Some(len) = expr_static_array_len(&args[0]) {
        return Ok((start..len).collect());
    }
    if let Some(Type::Multi(types)) = expected {
        let count = i32::try_from(types.len())
            .map_err(|_| Diagnostic::new("table.unpack expected multi-value arity is too large"))?;
        return Ok((start..start + count).collect());
    }
    Err(Diagnostic::new(
        "table.unpack requires literal bounds, a statically sized array argument, or an \
         expected multi-value arity (runtime-variable table.unpack is not supported yet)",
    ))
}

/// Statically-known length of an array expression: an array literal without
/// multi-value elements, or a direct `table.create(count, ...)` call with a
/// literal count.
fn expr_static_array_len(expr: &Expr) -> Option<i32> {
    match expr {
        Expr::ArrayLiteral { elements, .. } => {
            // A trailing `...` or call could expand to multiple values, so
            // only literals made of single-value elements have a static length.
            if elements.iter().any(|element| {
                matches!(
                    element,
                    Expr::Vararg(_) | Expr::Call { .. } | Expr::MethodCall { .. }
                )
            }) {
                return None;
            }
            i32::try_from(elements.len()).ok()
        }
        Expr::Call { callee, args, .. } => match callee.as_ref() {
            Expr::Field { base, name, .. }
                if name == "create"
                    && matches!(base.as_ref(), Expr::Name(namespace, _, _) if namespace == "table") =>
            {
                expr_i32_literal(args.first()?)
            }
            _ => None,
        },
        _ => None,
    }
}

mod captures {

    include!("captures.rs");
}

mod model {
    use super::*;
    include!("model.rs");
}

mod monomorphize {

    include!("monomorphize.rs");
}

mod source_map {
    include!("source_map.rs");
}

mod verify {
    use super::*;
    include!("verify.rs");
}

mod lower {
    use super::*;
    include!("lower.rs");
}

#[cfg(test)]
pub(crate) use lower::build_function;
pub use lower::{BuildCache, build, build_cached, build_cached_with_changes};
pub use model::{
    BasicBlock, BitwiseIntrinsic, BlockId, DeclaredImport, Function, FunctionSourceMap, Global,
    Instruction, MathIntrinsic, Module, SourceFile, SourceFileId, SourceLocation, SourceOrigin,
    Terminator, ValueId,
};
pub use verify::verify;

use captures::{collect_assigned_names, collect_captures, collect_nested_function_capture_names};
use monomorphize::Monomorphizer;
use source_map::resolve_span_to_line_and_text;

#[cfg(test)]
mod tests;
