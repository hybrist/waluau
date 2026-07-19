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

pub use lower::build;
#[cfg(test)]
pub(crate) use lower::build_function;
pub use model::{
    BasicBlock, BitwiseIntrinsic, BlockId, DeclaredImport, Function, Instruction, MathIntrinsic,
    Module, Terminator, ValueId,
};
pub use verify::verify;

use captures::{collect_assigned_names, collect_captures, collect_nested_function_capture_names};
use monomorphize::Monomorphizer;
use source_map::resolve_span_to_line_and_text;

#[cfg(test)]
mod tests;
