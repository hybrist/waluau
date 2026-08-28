//! Compile-time scanner for Lua pattern literals.
//!
//! The compiler needs the capture arity and kinds of a literal pattern to give
//! `string.find`/`string.match`/`string.gsub` statically-typed multi-value
//! results. This mirrors the pattern grammar of the runtime matcher in
//! `apps/playground/src/utils/lua-pattern.js` (a port of Luau's lstrlib.c) but
//! only walks the pattern; it never matches a subject.
//!
//! Malformed patterns yield `Err`; callers fall back to the no-capture typing
//! and let the runtime raise the corresponding Lua error when the pattern is
//! actually used.

use crate::{NumericType, Type};
use std::sync::Arc;

/// The kind of one Lua pattern capture, in pattern order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LuaCaptureKind {
    /// A regular `(...)` capture: captures the matched substring.
    Str,
    /// A `()` position capture: captures the 1-based match position.
    Position,
}

impl LuaCaptureKind {
    /// The value type a capture of this kind produces.
    pub fn value_type(self) -> Type {
        match self {
            LuaCaptureKind::Str => Type::String,
            LuaCaptureKind::Position => Type::Numeric(NumericType::I32),
        }
    }
}

/// Result types of a conforming `string.find(s, p, init?, plain?)` call:
/// `(start, end, captures...)`. `start` is nil when there is no match, so it
/// is nullable; the remaining values are only meaningful when `start` is
/// non-nil and keep their plain types.
pub fn string_find_result_types(captures: &[LuaCaptureKind]) -> Vec<Type> {
    let i32_ty = Type::Numeric(NumericType::I32);
    let mut types = vec![Type::Nullable(Arc::new(i32_ty.clone())), i32_ty];
    types.extend(captures.iter().map(|kind| kind.value_type()));
    types
}

/// Result types of a conforming `string.match(s, p, init?)` call: each
/// capture (or the whole match when the pattern has no captures), all nil
/// when there is no match.
pub fn string_match_result_types(captures: &[LuaCaptureKind]) -> Vec<Type> {
    if captures.is_empty() {
        return vec![Type::Nullable(Arc::new(Type::String))];
    }
    captures
        .iter()
        .map(|kind| Type::Nullable(Arc::new(kind.value_type())))
        .collect()
}

const MAXCAPTURES: usize = 32;

/// Scan a pattern literal and return its capture kinds in order.
pub fn lua_pattern_captures(pattern: &str) -> Result<Vec<LuaCaptureKind>, String> {
    let p: Vec<char> = pattern.chars().collect();
    let mut kinds = Vec::new();
    let mut open = 0usize;
    let mut i = if p.first() == Some(&'^') { 1 } else { 0 };
    while i < p.len() {
        match p[i] {
            '(' => {
                if kinds.len() >= MAXCAPTURES {
                    return Err("too many captures".to_string());
                }
                if p.get(i + 1) == Some(&')') {
                    kinds.push(LuaCaptureKind::Position);
                    i += 2;
                } else {
                    kinds.push(LuaCaptureKind::Str);
                    open += 1;
                    i += 1;
                }
            }
            ')' => {
                if open == 0 {
                    return Err("invalid pattern capture".to_string());
                }
                open -= 1;
                i += 1;
            }
            '%' => match p.get(i + 1) {
                None => return Err("malformed pattern (ends with '%')".to_string()),
                Some('b') => {
                    if i + 3 >= p.len() {
                        return Err("malformed pattern (missing arguments to '%b')".to_string());
                    }
                    i += 4;
                }
                Some('f') => {
                    i += 2;
                    if p.get(i) != Some(&'[') {
                        return Err("missing '[' after '%f' in pattern".to_string());
                    }
                    i = skip_set(&p, i)?;
                }
                Some(digit) if digit.is_ascii_digit() => {
                    let index = (*digit as usize).wrapping_sub('1' as usize);
                    if index >= kinds.len() {
                        return Err("invalid capture index".to_string());
                    }
                    i += 2;
                }
                Some(_) => i += 2,
            },
            '[' => {
                i = skip_set(&p, i)?;
            }
            _ => i += 1,
        }
    }
    if open > 0 {
        return Err("unfinished capture".to_string());
    }
    Ok(kinds)
}

/// Skip a `[set]` starting at the `[` and return the index just past the `]`.
/// A `]` directly after `[` (or `[^`) is a literal set member, as in lstrlib.
fn skip_set(p: &[char], start: usize) -> Result<usize, String> {
    let mut i = start + 1;
    if p.get(i) == Some(&'^') {
        i += 1;
    }
    loop {
        if i >= p.len() {
            return Err("malformed pattern (missing ']')".to_string());
        }
        let c = p[i];
        i += 1;
        if c == '%' && i < p.len() {
            i += 1; // skip the escaped character, e.g. '%]'
        }
        if p.get(i) == Some(&']') {
            return Ok(i + 1);
        }
    }
}

/// True when the pattern contains no special characters, i.e. a plain
/// substring search and a pattern search behave identically.
pub fn lua_pattern_is_plain(pattern: &str) -> bool {
    !pattern.contains(['^', '$', '*', '+', '?', '.', '(', ')', '[', ']', '%', '-'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_string_and_position_captures() {
        assert_eq!(lua_pattern_captures("%w+"), Ok(vec![]));
        assert_eq!(lua_pattern_captures("(%w+)"), Ok(vec![LuaCaptureKind::Str]));
        assert_eq!(
            lua_pattern_captures("()(%d+)()"),
            Ok(vec![
                LuaCaptureKind::Position,
                LuaCaptureKind::Str,
                LuaCaptureKind::Position
            ])
        );
        assert_eq!(
            lua_pattern_captures("^(((.).).* (%w*))$"),
            Ok(vec![
                LuaCaptureKind::Str,
                LuaCaptureKind::Str,
                LuaCaptureKind::Str,
                LuaCaptureKind::Str
            ])
        );
    }

    #[test]
    fn handles_sets_and_escapes() {
        assert_eq!(lua_pattern_captures("[()]"), Ok(vec![]));
        assert_eq!(lua_pattern_captures("[^)(]*"), Ok(vec![]));
        assert_eq!(lua_pattern_captures("%(%)"), Ok(vec![]));
        assert_eq!(lua_pattern_captures("[]]"), Ok(vec![]));
        assert_eq!(lua_pattern_captures("[^]]"), Ok(vec![]));
        assert_eq!(
            lua_pattern_captures("%f[%w](%a+)"),
            Ok(vec![LuaCaptureKind::Str])
        );
        assert_eq!(lua_pattern_captures("%b()"), Ok(vec![]));
        assert_eq!(
            lua_pattern_captures("(%w+)%s+%1"),
            Ok(vec![LuaCaptureKind::Str])
        );
    }

    #[test]
    fn rejects_malformed_patterns() {
        assert!(lua_pattern_captures("a%").is_err());
        assert!(lua_pattern_captures("[a").is_err());
        assert!(lua_pattern_captures("(a").is_err());
        assert!(lua_pattern_captures("a)").is_err());
        assert!(lua_pattern_captures("%b").is_err());
        assert!(lua_pattern_captures("%f%a").is_err());
        assert!(lua_pattern_captures("%1").is_err());
    }

    #[test]
    fn detects_plain_patterns() {
        assert!(lua_pattern_is_plain("alo"));
        assert!(lua_pattern_is_plain("a b"));
        assert!(!lua_pattern_is_plain("a+"));
        assert!(!lua_pattern_is_plain("a-b"));
    }
}
