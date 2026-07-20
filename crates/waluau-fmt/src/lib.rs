//! Source formatting for Waluau.
//!
//! This crate is a full reflowing pretty-printer: it re-wraps constructs to a
//! target line width rather than preserving the author's line breaks. It is
//! built on a lossless concrete syntax tree (added separately) plus the
//! [`doc`] layout engine in this module; it deliberately does *not* reprint the
//! compiler AST, which reorders top-level items and desugars string
//! interpolation.
//!
//! Today this crate exposes the [`doc`] engine. The CST-to-[`doc`] lowering and
//! the `format_source` entry point land in follow-up changes.

pub mod doc;
