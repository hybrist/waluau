//! Source formatting for Waluau.
//!
//! This crate is a full reflowing pretty-printer: it re-wraps constructs to a
//! target line width rather than preserving the author's line breaks. It is
//! built on a lossless concrete syntax tree ([`cst`]) parsed from a
//! comment-preserving token stream, plus the [`doc`] layout engine; it
//! deliberately does *not* reprint the compiler AST, which reorders top-level
//! items and desugars string interpolation.

pub mod cst;
pub mod doc;
mod lex;
mod parse;

pub use parse::parse;
