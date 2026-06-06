use std::collections::{BTreeMap, HashMap, HashSet};

use waluau_ast::{
    AssignOp, BinaryOp, Expr, Function as AstFunction, NumberLiteral, NumericType, Program, Stmt,
    SymbolId, Type, UnaryOp,
};
use waluau_diagnostics::{Diagnostic, DiagnosticCategory};

const COROUTINE_CREATE: &str = "coroutine.create";
const COROUTINE_RESUME: &str = "coroutine.resume";
const COROUTINE_CLOSE: &str = "coroutine.close";
const COROUTINE_YIELD: &str = "coroutine.yield";
const MATH_ABS: &str = "math.abs";
const MATH_MIN: &str = "math.min";
const MATH_MAX: &str = "math.max";
const MATH_SQRT: &str = "math.sqrt";
const MATH_FLOOR: &str = "math.floor";
const MATH_CEIL: &str = "math.ceil";
const MATH_TRUNC: &str = "math.trunc";
const MATH_NEAREST: &str = "math.nearest";
const MATH_COPYSIGN: &str = "math.copysign";
const TO_STRING: &str = "tostring";
const ASSERT: &str = "assert";
const PRINT: &str = "print";

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
    BasicBlock, BlockId, Function, Instruction, MathIntrinsic, Module, Terminator, ValueId,
};
pub use verify::verify;

use captures::{collect_assigned_names, collect_captures, collect_nested_function_capture_names};
use monomorphize::Monomorphizer;
use source_map::resolve_span_to_line_and_text;

#[cfg(test)]
mod tests;
