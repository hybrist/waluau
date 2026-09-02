//! Diagnostics for the logical operators `and`, `or`, and `not`.
//!
//! Waluau defines these over `bool` (see the strict-bool deviation in
//! `conformance/luau/DEVIATIONS.md`), with one exception: `or` also has a
//! nil-coalescing form, `a or b`, which supplies `b` when `a` is nil.
//!
//! Which form applies is decided by the **left** operand: only a left operand
//! whose type admits nil -- `T?`, or the top type `unknown` -- gets the
//! nil-coalescing form. Everything else is the boolean form, where both
//! operands must be `bool`.
//!
//! That asymmetry is why these diagnostics exist. Reporting a bare "cannot
//! implicitly convert string to bool" at whichever operand happened to be
//! checked second sends the reader to a right operand that could never have
//! changed the outcome; three successive comments on `waluau-esz6` recorded
//! the wrong blocker for `conformance/luau/pm.133`-`pm.143` for exactly that
//! reason. Name the operand that does not fit, its type, and -- when the left
//! operand is what ruled out the nil-coalescing form -- say so.

use waluau_ast::{Span, Type};
use waluau_diagnostics::Diagnostic;

/// Which operand of a logical operator a diagnostic is about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LogicalOperand {
    /// The left operand of `and`/`or`. This is the one that selects between
    /// the boolean and nil-coalescing forms of `or`.
    Left,
    /// The right operand of `and`/`or`.
    Right,
    /// The single operand of `not`.
    Only,
}

/// Reports a logical operator's operand that is not a `bool`.
///
/// `actual` is the operand's own type, `left_ty` the left operand's type when
/// it is known and is not the operand being reported. `span` anchors the
/// diagnostic on the operand itself, so the blame site and the fix site are
/// the same expression.
pub fn non_bool_operand(
    operator: &str,
    operand: LogicalOperand,
    actual: &Type,
    left_ty: Option<&Type>,
    span: Option<Span>,
) -> Diagnostic {
    let message = match operand {
        LogicalOperand::Only => {
            format!("'{operator}' requires a bool operand, got {actual}")
        }
        LogicalOperand::Left if operator == "or" => format!(
            "'or' requires a bool left operand, got {actual}; only a nullable or \
             unknown left operand supplies a default instead"
        ),
        LogicalOperand::Left => {
            format!("'{operator}' requires a bool left operand, got {actual}")
        }
        LogicalOperand::Right => match left_ty {
            // Naming the left operand is the point: it is what made this the
            // boolean form, and no edit to the right operand can undo that.
            Some(left_ty) => format!(
                "'{operator}' is boolean here because its left operand is {left_ty}, \
                 so its right operand must be bool too, got {actual}"
            ),
            None => format!("'{operator}' requires a bool right operand, got {actual}"),
        },
    };
    let diagnostic = Diagnostic::new(message);
    match span {
        Some(span) => diagnostic.with_span(span),
        None => diagnostic,
    }
}

/// Rewrites a failed bool coercion of a logical operand into [`non_bool_operand`].
///
/// `probe` is the operand's own type, inferred without a `bool` expectation.
/// `None` means the operand does not type check on its own terms either, so
/// its original diagnostic is already about the real problem and is kept.
pub fn explain_non_bool_operand(
    original: Diagnostic,
    operator: &str,
    operand: LogicalOperand,
    probe: Option<Type>,
    left_ty: Option<&Type>,
    span: Option<Span>,
) -> Diagnostic {
    match probe {
        Some(actual) if actual != Type::Bool => non_bool_operand(
            operator,
            operand,
            &actual,
            left_ty,
            span.or(original.span()),
        ),
        _ => original,
    }
}
