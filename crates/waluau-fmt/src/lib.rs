//! Source formatting for Waluau.
//!
//! This crate is a full reflowing pretty-printer: it re-wraps constructs to a
//! target line width rather than preserving the author's line breaks. It is
//! built on a lossless concrete syntax tree ([`cst`]) parsed from a
//! comment-preserving token stream, plus the [`doc`] layout engine; it
//! deliberately does *not* reprint the compiler AST, which reorders top-level
//! items and desugars string interpolation.
//!
//! The entry point is [`format_source`].

pub mod cst;
pub mod doc;
mod lex;
mod lower;
mod parse;

use waluau_diagnostics::Diagnostic;

pub use parse::parse;

/// Formatter style options.
#[derive(Clone, Copy, Debug)]
pub struct FormatConfig {
    /// Target maximum line width before groups break onto multiple lines.
    pub line_width: usize,
    /// Number of spaces per indentation level.
    pub indent_width: usize,
}

impl Default for FormatConfig {
    fn default() -> Self {
        FormatConfig {
            line_width: 100,
            indent_width: 4,
        }
    }
}

/// Format Waluau `source` under `config`.
///
/// Returns an error only when `source` does not parse. The output ends with a
/// single trailing newline and has no trailing whitespace on any line.
pub fn format_source(source: &str, config: &FormatConfig) -> Result<String, Diagnostic> {
    let tree = parse::parse(source)?;
    let doc = lower::lower_root(&tree);
    let opts = doc::PrintOptions {
        width: config.line_width,
        indent: config.indent_width,
    };
    let mut out = doc::print(&doc, opts);
    // Normalise to exactly one trailing newline.
    let trimmed = out.trim_end();
    out.truncate(trimmed.len());
    out.push('\n');
    Ok(out)
}
