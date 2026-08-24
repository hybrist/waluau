use waluau_diagnostics::Diagnostic;

pub use waluau_span::Span;

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Function,
    Local,
    If,
    Then,
    ElseIf,
    Else,
    End,
    While,
    For,
    In,
    Repeat,
    Until,
    Do,
    Return,
    Break,
    Continue,
    Not,
    NumberType,
    U32Type,
    U64Type,
    I32Type,
    I64Type,
    F32Type,
    F64Type,
    UnitType,
    BoolType,
    UnknownType,
    StringType,
    BytesType,
    ExternType,
    ThreadType,
    Nil,
    True,
    False,
    Identifier(String),
    Number(String),
    Str(String),
    Bytes(Vec<u8>),
    Plus,
    PlusEqual,
    Minus,
    MinusEqual,
    Star,
    StarEqual,
    Slash,
    SlashEqual,
    DoubleSlash,
    DoubleSlashEqual,
    Percent,
    PercentEqual,
    Caret,
    CaretEqual,
    Equal,
    EqualEqual,
    TildeEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
    Pipe,
    Ampersand,
    Arrow,
    ColonColon,
    Colon,
    Dot,
    DoubleDot,
    DoubleDotEqual,
    TripleDot,
    Comma,
    Semicolon,
    Hash,
    Question,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    LParen,
    RParen,
    /// A single-line `-- ...` comment. Holds the raw lexeme including the
    /// leading `--` (e.g. `-- hello`). Only produced by [`lex_with_trivia`].
    LineComment(String),
    /// A `--[[ ... ]]` block comment (any bracket level). Holds the raw
    /// lexeme including the delimiters. Only produced by [`lex_with_trivia`].
    BlockComment(String),
    /// A backtick interpolated string, kept raw (undesugared) including the
    /// enclosing backticks and any `{expr}` holes, e.g. `` `hi {x}` ``. Only
    /// produced by [`lex_with_trivia`]; the compiler path desugars these into a
    /// parenthesized concatenation instead.
    InterpString(String),
}

impl TokenKind {
    /// Whether this token is comment trivia. Only [`lex_with_trivia`] emits
    /// these; the compiler token stream from [`lex`] never contains them.
    pub fn is_comment(&self) -> bool {
        matches!(self, TokenKind::LineComment(_) | TokenKind::BlockComment(_))
    }
}

pub fn lex(source: &str) -> Result<Vec<Token>, Diagnostic> {
    let chars: Vec<char> = source.chars().collect();
    let (tokens, _) = lex_range(&chars, 0, false, false)?;
    Ok(tokens)
}

/// Like [`lex`], but lossless: preserves comments as [`TokenKind::LineComment`]
/// / [`TokenKind::BlockComment`] trivia tokens, and keeps backtick strings raw
/// as [`TokenKind::InterpString`] rather than desugaring them, all with
/// accurate spans. Used by tooling (the formatter) that must reconstruct the
/// original source; the compiler path keeps using [`lex`], whose output is
/// unchanged.
pub fn lex_with_trivia(source: &str) -> Result<Vec<Token>, Diagnostic> {
    let chars: Vec<char> = source.chars().collect();
    let (tokens, _) = lex_range(&chars, 0, false, true)?;
    Ok(tokens)
}

/// Lex starting at `start`. With `stop_at_unmatched_rbrace`, lexing stops at
/// the first `}` that closes no `{` opened within this range and returns its
/// index; backtick string interpolation uses this to lex the embedded
/// expression. Otherwise lexes to the end of input.
fn lex_range(
    chars: &[char],
    start: usize,
    stop_at_unmatched_rbrace: bool,
    lossless: bool,
) -> Result<(Vec<Token>, usize), Diagnostic> {
    let mut tokens = Vec::new();
    let mut i = start;
    let mut brace_depth = 0usize;

    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }

        let start = i as u32;
        let (kind, consumed) = match c {
            '(' => (TokenKind::LParen, 1),
            ')' => (TokenKind::RParen, 1),
            '[' => {
                // Lua long strings: `[[...]]` and the leveled `[=*[...]=*]`.
                if let Some(level) = long_bracket_level(chars, i) {
                    let (value, end) = parse_long_string(chars, i, level)?;
                    tokens.push(Token {
                        kind: TokenKind::Str(value),
                        span: Span {
                            start,
                            end: end as u32,
                        },
                    });
                    i = end;
                    continue;
                }
                (TokenKind::LBracket, 1)
            }
            ']' => (TokenKind::RBracket, 1),
            '{' => {
                brace_depth += 1;
                (TokenKind::LBrace, 1)
            }
            '}' => {
                if stop_at_unmatched_rbrace && brace_depth == 0 {
                    return Ok((tokens, i));
                }
                brace_depth = brace_depth.saturating_sub(1);
                (TokenKind::RBrace, 1)
            }
            '`' => {
                i = lex_interpolated_string(chars, i, &mut tokens, lossless)?;
                continue;
            }
            '#' => (TokenKind::Hash, 1),
            '?' => (TokenKind::Question, 1),
            ':' => {
                if matches!(chars.get(i + 1), Some(':')) {
                    (TokenKind::ColonColon, 2)
                } else {
                    (TokenKind::Colon, 1)
                }
            }
            ',' => (TokenKind::Comma, 1),
            ';' => (TokenKind::Semicolon, 1),
            '+' => {
                if matches!(chars.get(i + 1), Some('=')) {
                    (TokenKind::PlusEqual, 2)
                } else {
                    (TokenKind::Plus, 1)
                }
            }
            '*' => {
                if matches!(chars.get(i + 1), Some('=')) {
                    (TokenKind::StarEqual, 2)
                } else {
                    (TokenKind::Star, 1)
                }
            }
            '/' => {
                if matches!(chars.get(i + 1), Some('/')) {
                    if matches!(chars.get(i + 2), Some('=')) {
                        (TokenKind::DoubleSlashEqual, 3)
                    } else {
                        (TokenKind::DoubleSlash, 2)
                    }
                } else if matches!(chars.get(i + 1), Some('=')) {
                    (TokenKind::SlashEqual, 2)
                } else {
                    (TokenKind::Slash, 1)
                }
            }
            '%' => {
                if matches!(chars.get(i + 1), Some('=')) {
                    (TokenKind::PercentEqual, 2)
                } else {
                    (TokenKind::Percent, 1)
                }
            }
            '^' => {
                if matches!(chars.get(i + 1), Some('=')) {
                    (TokenKind::CaretEqual, 2)
                } else {
                    (TokenKind::Caret, 1)
                }
            }
            '.' => {
                if matches!(chars.get(i + 1), Some('.')) && matches!(chars.get(i + 2), Some('.')) {
                    (TokenKind::TripleDot, 3)
                } else if matches!(chars.get(i + 1), Some('.')) {
                    if matches!(chars.get(i + 2), Some('=')) {
                        (TokenKind::DoubleDotEqual, 3)
                    } else {
                        (TokenKind::DoubleDot, 2)
                    }
                } else {
                    (TokenKind::Dot, 1)
                }
            }
            '<' => {
                if matches!(chars.get(i + 1), Some('=')) {
                    (TokenKind::LessEqual, 2)
                } else {
                    (TokenKind::Less, 1)
                }
            }
            '>' => {
                if matches!(chars.get(i + 1), Some('=')) {
                    (TokenKind::GreaterEqual, 2)
                } else {
                    (TokenKind::Greater, 1)
                }
            }
            '=' => {
                if matches!(chars.get(i + 1), Some('=')) {
                    (TokenKind::EqualEqual, 2)
                } else {
                    (TokenKind::Equal, 1)
                }
            }
            '~' => {
                if matches!(chars.get(i + 1), Some('=')) {
                    (TokenKind::TildeEqual, 2)
                } else {
                    return Err(Diagnostic::new("unexpected '~', expected '~='"));
                }
            }
            '-' => {
                if matches!(chars.get(i + 1), Some('-')) {
                    // Long (block) comment: `--[[...]]` or the leveled `--[=*[...]=*]`.
                    if let Some(level) = long_bracket_level(chars, i + 2) {
                        let (_, end) = parse_long_bracket(
                            chars,
                            i + 2,
                            level,
                            "unterminated block comment '--[[...]]'",
                        )?;
                        if lossless {
                            let raw: String = chars[i..end].iter().collect();
                            tokens.push(Token {
                                kind: TokenKind::BlockComment(raw),
                                span: Span {
                                    start,
                                    end: end as u32,
                                },
                            });
                        }
                        i = end;
                        continue;
                    }
                    let mut end = i + 2;
                    while end < chars.len() && chars[end] != '\n' {
                        end += 1;
                    }
                    if lossless {
                        let raw: String = chars[i..end].iter().collect();
                        tokens.push(Token {
                            kind: TokenKind::LineComment(raw),
                            span: Span {
                                start,
                                end: end as u32,
                            },
                        });
                    }
                    i = end;
                    continue;
                }
                if matches!(chars.get(i + 1), Some('>')) {
                    (TokenKind::Arrow, 2)
                } else if matches!(chars.get(i + 1), Some('=')) {
                    (TokenKind::MinusEqual, 2)
                } else {
                    (TokenKind::Minus, 1)
                }
            }
            '&' => {
                if matches!(chars.get(i + 1), Some('&')) {
                    return Err(Diagnostic::new("unsupported '&&', use 'and'"));
                } else {
                    (TokenKind::Ampersand, 1)
                }
            }
            '|' => {
                if matches!(chars.get(i + 1), Some('|')) {
                    return Err(Diagnostic::new("unsupported '||', use 'or'"));
                } else {
                    (TokenKind::Pipe, 1)
                }
            }
            '"' | '\'' => {
                let (value, end) = parse_string_literal(chars, i)?;
                tokens.push(Token {
                    kind: TokenKind::Str(value),
                    span: Span {
                        start,
                        end: end as u32,
                    },
                });
                i = end;
                continue;
            }
            'b' if matches!(chars.get(i + 1), Some('"')) => {
                let (value, end) = parse_bytes_literal(chars, i + 1)?;
                tokens.push(Token {
                    kind: TokenKind::Bytes(value),
                    span: Span {
                        start,
                        end: end as u32,
                    },
                });
                i = end;
                continue;
            }
            d if d.is_ascii_digit() => {
                let mut end = i + 1;
                if d == '0' && matches!(chars.get(end), Some('x' | 'X')) {
                    end += 1;
                    while end < chars.len() && (chars[end].is_ascii_hexdigit() || chars[end] == '_')
                    {
                        end += 1;
                    }
                } else {
                    while end < chars.len() {
                        if chars[end].is_ascii_digit() || chars[end] == '_' {
                            end += 1;
                            continue;
                        }
                        if chars[end] == '.' {
                            if matches!(chars.get(end + 1), Some('.')) {
                                break;
                            }
                            end += 1;
                            continue;
                        }
                        break;
                    }
                }
                // `i`/`end` index into `chars`; slicing `source` with them
                // would use byte offsets and break after any multibyte char.
                let number: String = chars[i..end].iter().collect();
                if number.matches('.').count() > 1 {
                    return Err(Diagnostic::new("invalid number literal"));
                }
                if number.contains('.') {
                    let normalized = number.replace('_', "");
                    number
                        .parse::<f64>()
                        .or_else(|_| normalized.parse::<f64>())
                        .map_err(|_| Diagnostic::new("invalid number literal"))?;
                } else if matches!(number.as_str(), "0x" | "0X") {
                    return Err(Diagnostic::new("invalid number literal"));
                } else if number.starts_with("0x") || number.starts_with("0X") {
                    let digits = number[2..].replace('_', "");
                    u128::from_str_radix(&digits, 16)
                        .map_err(|_| Diagnostic::new("invalid number literal"))?;
                }
                tokens.push(Token {
                    kind: TokenKind::Number(number),
                    span: Span {
                        start,
                        end: end as u32,
                    },
                });
                i = end;
                continue;
            }
            a if a.is_ascii_alphabetic() || a == '_' => {
                let mut end = i + 1;
                while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '_')
                {
                    end += 1;
                }
                let text: String = chars[i..end].iter().collect();
                let kind = match text.as_str() {
                    "function" => TokenKind::Function,
                    "local" => TokenKind::Local,
                    "if" => TokenKind::If,
                    "then" => TokenKind::Then,
                    "elseif" => TokenKind::ElseIf,
                    "else" => TokenKind::Else,
                    "end" => TokenKind::End,
                    "while" => TokenKind::While,
                    "for" => TokenKind::For,
                    "in" => TokenKind::In,
                    "repeat" => TokenKind::Repeat,
                    "until" => TokenKind::Until,
                    "do" => TokenKind::Do,
                    "return" => TokenKind::Return,
                    "break" => TokenKind::Break,
                    "continue" => TokenKind::Continue,
                    "not" => TokenKind::Not,
                    "and" => TokenKind::And,
                    "or" => TokenKind::Or,
                    "number" => TokenKind::NumberType,
                    "u32" => TokenKind::U32Type,
                    "u64" => TokenKind::U64Type,
                    "i32" => TokenKind::I32Type,
                    "i64" => TokenKind::I64Type,
                    "f32" => TokenKind::F32Type,
                    "f64" => TokenKind::F64Type,
                    "unit" | "void" => TokenKind::UnitType,
                    "bool" => TokenKind::BoolType,
                    "unknown" => TokenKind::UnknownType,
                    "string" => TokenKind::StringType,
                    "bytes" => TokenKind::BytesType,
                    "extern" => TokenKind::ExternType,
                    "thread" => TokenKind::ThreadType,
                    "nil" => TokenKind::Nil,
                    "true" => TokenKind::True,
                    "false" => TokenKind::False,
                    _ => TokenKind::Identifier(text.to_string()),
                };
                tokens.push(Token {
                    kind,
                    span: Span {
                        start,
                        end: end as u32,
                    },
                });
                i = end;
                continue;
            }
            _ => return Err(Diagnostic::new(format!("unexpected character '{c}'"))),
        };

        tokens.push(Token {
            kind,
            span: Span {
                start,
                end: start + consumed as u32,
            },
        });
        i += consumed;
    }

    if stop_at_unmatched_rbrace {
        return Err(Diagnostic::new(
            "unterminated interpolation '{...}' in backtick string",
        ));
    }
    Ok((tokens, i))
}

/// Lex a backtick interpolated string starting at the opening backtick and
/// desugar it directly into the token stream as a parenthesized concatenation:
/// `` `a{x}b` `` becomes `("a" .. tostring(x) .. "b")`. Returns the index just
/// past the closing backtick.
/// Find the index just past the closing backtick of the interpolated string
/// starting at `backtick_index`, without desugaring. `{expr}` holes are skipped
/// using the same `}`-matching logic as the desugaring path, and escapes are
/// consumed so that `` \` `` and `\{` do not end the string or open a hole.
fn scan_interpolated_end(chars: &[char], backtick_index: usize) -> Result<usize, Diagnostic> {
    let mut i = backtick_index + 1;
    let mut scratch = String::new();
    loop {
        match chars.get(i) {
            None | Some('\n') => {
                return Err(Diagnostic::new("unterminated interpolated string literal"));
            }
            Some('`') => return Ok(i + 1),
            Some('{') => {
                // Skip the embedded expression up to its matching `}`.
                let (_, rbrace) = lex_range(chars, i + 1, true, true)?;
                i = rbrace + 1;
            }
            Some('\\') => {
                scratch.clear();
                i = push_string_escape(chars, i + 1, &mut scratch)?;
            }
            Some(_) => i += 1,
        }
    }
}

fn lex_interpolated_string(
    chars: &[char],
    backtick_index: usize,
    tokens: &mut Vec<Token>,
    lossless: bool,
) -> Result<usize, Diagnostic> {
    // Lossless mode keeps the whole backtick literal as one raw token so the
    // formatter can reproduce it verbatim, rather than desugaring to concat.
    if lossless {
        let end = scan_interpolated_end(chars, backtick_index)?;
        let raw: String = chars[backtick_index..end].iter().collect();
        tokens.push(Token {
            kind: TokenKind::InterpString(raw),
            span: Span {
                start: backtick_index as u32,
                end: end as u32,
            },
        });
        return Ok(end);
    }

    enum Part {
        Lit(String, Span),
        Expr(Vec<Token>),
    }

    let mut parts: Vec<Part> = Vec::new();
    let mut lit = String::new();
    let mut lit_start = backtick_index + 1;
    let mut i = backtick_index + 1;
    let flush_lit = |lit: &mut String, from: usize, to: usize, parts: &mut Vec<Part>| {
        if !lit.is_empty() {
            parts.push(Part::Lit(
                std::mem::take(lit),
                Span {
                    start: from as u32,
                    end: to as u32,
                },
            ));
        }
    };
    loop {
        match chars.get(i) {
            None | Some('\n') => {
                return Err(Diagnostic::new("unterminated interpolated string literal"));
            }
            Some('`') => {
                flush_lit(&mut lit, lit_start, i, &mut parts);
                i += 1;
                break;
            }
            Some('{') => {
                flush_lit(&mut lit, lit_start, i, &mut parts);
                let (expr_tokens, rbrace) = lex_range(chars, i + 1, true, lossless)?;
                if expr_tokens.is_empty() {
                    return Err(Diagnostic::new(
                        "empty interpolation '{}' in backtick string",
                    ));
                }
                parts.push(Part::Expr(expr_tokens));
                i = rbrace + 1;
                lit_start = i;
            }
            Some('\\') => {
                i = push_string_escape(chars, i + 1, &mut lit)?;
            }
            Some(other) => {
                lit.push(*other);
                i += 1;
            }
        }
    }

    let whole_span = Span {
        start: backtick_index as u32,
        end: i as u32,
    };
    tokens.push(Token {
        kind: TokenKind::LParen,
        span: whole_span,
    });
    if parts.is_empty() {
        tokens.push(Token {
            kind: TokenKind::Str(String::new()),
            span: whole_span,
        });
    }
    for (index, part) in parts.into_iter().enumerate() {
        if index > 0 {
            tokens.push(Token {
                kind: TokenKind::DoubleDot,
                span: whole_span,
            });
        }
        match part {
            Part::Lit(value, span) => tokens.push(Token {
                kind: TokenKind::Str(value),
                span,
            }),
            Part::Expr(expr_tokens) => {
                let expr_span = Span {
                    start: expr_tokens.first().map(|t| t.span.start).unwrap_or(0),
                    end: expr_tokens.last().map(|t| t.span.end).unwrap_or(0),
                };
                tokens.push(Token {
                    kind: TokenKind::Identifier("tostring".to_string()),
                    span: expr_span,
                });
                tokens.push(Token {
                    kind: TokenKind::LParen,
                    span: expr_span,
                });
                tokens.extend(expr_tokens);
                tokens.push(Token {
                    kind: TokenKind::RParen,
                    span: expr_span,
                });
            }
        }
    }
    tokens.push(Token {
        kind: TokenKind::RParen,
        span: whole_span,
    });
    Ok(i)
}

/// Detect a Lua long-bracket opener at `idx`: `[`, then zero or more `=`
/// (the *level*), then `[`. Returns the level when one is present.
fn long_bracket_level(chars: &[char], idx: usize) -> Option<usize> {
    if chars.get(idx) != Some(&'[') {
        return None;
    }
    let mut j = idx + 1;
    let mut level = 0;
    while chars.get(j) == Some(&'=') {
        level += 1;
        j += 1;
    }
    if chars.get(j) == Some(&'[') {
        Some(level)
    } else {
        None
    }
}

/// Read a long-bracket region (`[[...]]` / `[=*[...]=*]`) starting at the
/// opening bracket `idx` with the given `level`. Returns the contained text and
/// the index just past the closing bracket. Used for both long strings and long
/// comments; `unterminated_msg` tailors the diagnostic to the caller.
fn parse_long_bracket(
    chars: &[char],
    idx: usize,
    level: usize,
    unterminated_msg: &str,
) -> Result<(String, usize), Diagnostic> {
    // The opener is `[` + `level` `=` + `[`, i.e. `2 + level` characters.
    let mut pos = idx + 2 + level;
    // Lua discards a newline immediately following the opening bracket.
    match chars.get(pos) {
        Some('\n') => pos += 1,
        Some('\r') => {
            pos += 1;
            if chars.get(pos) == Some(&'\n') {
                pos += 1;
            }
        }
        _ => {}
    }
    let mut value = String::new();
    loop {
        match chars.get(pos) {
            None => return Err(Diagnostic::new(unterminated_msg.to_string())),
            Some(']') => {
                let mut k = pos + 1;
                let mut eq = 0;
                while chars.get(k) == Some(&'=') {
                    eq += 1;
                    k += 1;
                }
                if eq == level && chars.get(k) == Some(&']') {
                    return Ok((value, k + 1));
                }
                value.push(']');
                pos += 1;
            }
            Some(other) => {
                value.push(*other);
                pos += 1;
            }
        }
    }
}

fn parse_long_string(
    chars: &[char],
    idx: usize,
    level: usize,
) -> Result<(String, usize), Diagnostic> {
    parse_long_bracket(chars, idx, level, "unterminated long string '[[...]]'")
}

fn parse_string_literal(chars: &[char], quote_index: usize) -> Result<(String, usize), Diagnostic> {
    let quote = chars[quote_index];
    let mut end = quote_index + 1;
    let mut value = String::new();
    loop {
        match chars.get(end) {
            None | Some('\n') => return Err(Diagnostic::new("unterminated string literal")),
            Some(c) if *c == quote => {
                end += 1;
                break;
            }
            Some('\\') => {
                end = push_string_escape(chars, end + 1, &mut value)?;
            }
            Some(other) => {
                value.push(*other);
                end += 1;
            }
        }
    }
    Ok((value, end))
}

/// Handle the string escape whose introducing `\` sits just before `idx`.
/// Appends the escaped text to `value` and returns the index just past the
/// escape. Shared by quoted strings and backtick interpolated strings.
fn push_string_escape(chars: &[char], idx: usize, value: &mut String) -> Result<usize, Diagnostic> {
    let mut end = idx;
    match chars.get(end) {
        Some('"') => value.push('"'),
        Some('\'') => value.push('\''),
        Some('`') => value.push('`'),
        Some('{') => value.push('{'),
        Some('}') => value.push('}'),
        Some('\\') => value.push('\\'),
        Some('n') => value.push('\n'),
        Some('t') => value.push('\t'),
        Some('r') => value.push('\r'),
        Some('a') => value.push('\u{7}'),
        Some('b') => value.push('\u{8}'),
        Some('f') => value.push('\u{c}'),
        Some('v') => value.push('\u{b}'),
        // An escaped literal newline continues the string on the next line.
        Some('\n') => value.push('\n'),
        // `\z` skips all following whitespace, including newlines.
        Some('z') => {
            end += 1;
            while matches!(chars.get(end), Some(c) if c.is_whitespace()) {
                end += 1;
            }
            return Ok(end);
        }
        Some('x') => {
            let hi = chars.get(end + 1).and_then(|c| c.to_digit(16));
            let lo = chars.get(end + 2).and_then(|c| c.to_digit(16));
            let (Some(hi), Some(lo)) = (hi, lo) else {
                return Err(Diagnostic::new("string \\x escape requires two hex digits"));
            };
            value.push(char::from_u32((hi << 4) | lo).expect("byte value is a valid char"));
            end += 2;
        }
        Some('u') => {
            if chars.get(end + 1) != Some(&'{') {
                return Err(Diagnostic::new(
                    "string \\u escape requires braces, e.g. '\\u{1F600}'",
                ));
            }
            end += 2;
            let mut code: u32 = 0;
            let mut digits = 0;
            while let Some(digit) = chars.get(end).and_then(|c| c.to_digit(16)) {
                code = code
                    .checked_mul(16)
                    .and_then(|code| code.checked_add(digit))
                    .ok_or_else(|| Diagnostic::new("string \\u escape exceeds U+10FFFF"))?;
                digits += 1;
                end += 1;
            }
            if digits == 0 || chars.get(end) != Some(&'}') {
                return Err(Diagnostic::new(
                    "string \\u escape requires hex digits followed by '}'",
                ));
            }
            let unicode = char::from_u32(code).ok_or_else(|| {
                Diagnostic::new(format!(
                    "string \\u escape 'u{{{code:x}}}' is not a valid codepoint"
                ))
            })?;
            value.push(unicode);
        }
        Some(digit) if digit.is_ascii_digit() => {
            // Luau decimal escape: up to three digits, byte value <= 255.
            let mut code: u32 = 0;
            let mut digits = 0;
            while digits < 3 {
                match chars.get(end) {
                    Some(c) if c.is_ascii_digit() => {
                        code = code * 10 + c.to_digit(10).unwrap();
                        digits += 1;
                        end += 1;
                    }
                    _ => break,
                }
            }
            if code > 255 {
                return Err(Diagnostic::new(format!(
                    "decimal string escape '\\{code}' exceeds 255"
                )));
            }
            value.push(char::from_u32(code).unwrap());
            return Ok(end);
        }
        Some(other) => {
            return Err(Diagnostic::new(format!(
                "unsupported string escape '\\{other}'"
            )));
        }
        None => return Err(Diagnostic::new("unterminated string literal")),
    }
    Ok(end + 1)
}

fn parse_bytes_literal(chars: &[char], quote_index: usize) -> Result<(Vec<u8>, usize), Diagnostic> {
    let mut end = quote_index + 1;
    let mut value = Vec::new();
    loop {
        match chars.get(end) {
            None | Some('\n') => return Err(Diagnostic::new("unterminated bytes literal")),
            Some('"') => {
                end += 1;
                break;
            }
            Some('\\') => {
                end += 1;
                match chars.get(end) {
                    Some('"') => value.push(b'"'),
                    Some('\\') => value.push(b'\\'),
                    Some('n') => value.push(b'\n'),
                    Some('t') => value.push(b'\t'),
                    Some('x') => {
                        let hi =
                            chars
                                .get(end + 1)
                                .and_then(|c| c.to_digit(16))
                                .ok_or_else(|| {
                                    Diagnostic::new(
                                        "bytes literal \\x escape requires two hex digits",
                                    )
                                })?;
                        let lo =
                            chars
                                .get(end + 2)
                                .and_then(|c| c.to_digit(16))
                                .ok_or_else(|| {
                                    Diagnostic::new(
                                        "bytes literal \\x escape requires two hex digits",
                                    )
                                })?;
                        value.push(((hi << 4) | lo) as u8);
                        end += 2;
                    }
                    Some(other) if other.is_ascii() => {
                        return Err(Diagnostic::new(format!(
                            "unsupported bytes escape '\\{other}'"
                        )));
                    }
                    Some(other) => {
                        return Err(Diagnostic::new(format!(
                            "bytes literal only supports ASCII source characters, got '{other}'"
                        )));
                    }
                    None => return Err(Diagnostic::new("unterminated bytes literal")),
                }
                end += 1;
            }
            Some(other) if other.is_ascii() => {
                value.push(*other as u8);
                end += 1;
            }
            Some(other) => {
                return Err(Diagnostic::new(format!(
                    "bytes literal only supports ASCII source characters, got '{other}'"
                )));
            }
        }
    }
    Ok((value, end))
}

#[cfg(test)]
mod tests {
    use super::{Token, TokenKind, lex, lex_with_trivia};
    use waluau_diagnostics::Diagnostic;
    use waluau_span::Span;

    fn trivia_kinds(source: &str) -> Vec<TokenKind> {
        lex_with_trivia(source)
            .expect("lex_with_trivia should succeed")
            .into_iter()
            .map(|token| token.kind)
            .collect()
    }

    #[test]
    fn lex_with_trivia_preserves_line_comments() {
        assert_eq!(
            trivia_kinds("local x = 1 -- hi\n-- whole line\nx = 2"),
            vec![
                TokenKind::Local,
                TokenKind::Identifier("x".into()),
                TokenKind::Equal,
                TokenKind::Number("1".into()),
                TokenKind::LineComment("-- hi".into()),
                TokenKind::LineComment("-- whole line".into()),
                TokenKind::Identifier("x".into()),
                TokenKind::Equal,
                TokenKind::Number("2".into()),
            ]
        );
    }

    #[test]
    fn lex_with_trivia_preserves_block_comments_with_level() {
        assert_eq!(
            trivia_kinds("local x --[==[ a ]] still ]==] = 1"),
            vec![
                TokenKind::Local,
                TokenKind::Identifier("x".into()),
                TokenKind::BlockComment("--[==[ a ]] still ]==]".into()),
                TokenKind::Equal,
                TokenKind::Number("1".into()),
            ]
        );
    }

    #[test]
    fn lex_with_trivia_keeps_interpolation_raw() {
        assert_eq!(
            trivia_kinds(r#"local s = `hi {name}!`"#),
            vec![
                TokenKind::Local,
                TokenKind::Identifier("s".into()),
                TokenKind::Equal,
                TokenKind::InterpString("`hi {name}!`".into()),
            ]
        );
    }

    #[test]
    fn lex_with_trivia_interpolation_handles_nested_braces_and_strings() {
        // A `}` inside a nested table/string within the hole must not end the
        // interpolation early, and an escaped backtick must not end the string.
        let src = r#"`a {f({x = "}"})} \` b`"#;
        assert_eq!(trivia_kinds(src), vec![TokenKind::InterpString(src.into())]);
    }

    #[test]
    fn lex_with_trivia_records_comment_spans() {
        let tokens = lex_with_trivia("x -- c").expect("lex should succeed");
        let comment = tokens
            .iter()
            .find(|t| t.kind.is_comment())
            .expect("comment token present");
        assert_eq!(comment.span, Span { start: 2, end: 6 });
    }

    #[test]
    fn plain_lex_still_discards_comments() {
        assert_eq!(
            kinds("local x = 1 -- hi\n--[[ b ]] x = 2"),
            vec![
                TokenKind::Local,
                TokenKind::Identifier("x".into()),
                TokenKind::Equal,
                TokenKind::Number("1".into()),
                TokenKind::Identifier("x".into()),
                TokenKind::Equal,
                TokenKind::Number("2".into()),
            ]
        );
    }

    fn kinds(source: &str) -> Vec<TokenKind> {
        lex(source)
            .expect("lex should succeed")
            .into_iter()
            .map(|token| token.kind)
            .collect()
    }

    fn err(source: &str) -> Diagnostic {
        lex(source).expect_err("lex should fail")
    }

    #[test]
    fn rejects_malformed_number_literals() {
        assert_eq!(err("12.34.56").to_string(), "invalid number literal");
    }

    #[test]
    fn tokenizes_number_concat_number() {
        assert_eq!(
            kinds("1..2"),
            vec![
                TokenKind::Number("1".into()),
                TokenKind::DoubleDot,
                TokenKind::Number("2".into()),
            ]
        );
    }

    #[test]
    fn tokenizes_concat_operator() {
        assert_eq!(
            kinds(r#""a" .. "b""#),
            vec![
                TokenKind::Str("a".into()),
                TokenKind::DoubleDot,
                TokenKind::Str("b".into()),
            ]
        );
    }

    #[test]
    fn accepts_well_formed_number_literals() {
        assert_eq!(
            kinds("0 42 3.14 1."),
            vec![
                TokenKind::Number("0".into()),
                TokenKind::Number("42".into()),
                TokenKind::Number("3.14".into()),
                TokenKind::Number("1.".into()),
            ]
        );
    }

    #[test]
    fn rejects_unsupported_operators() {
        assert_eq!(err("&&").to_string(), "unsupported '&&', use 'and'");
        assert_eq!(err("||").to_string(), "unsupported '||', use 'or'");
        assert_eq!(kinds("&"), vec![TokenKind::Ampersand]);
        assert_eq!(kinds("|"), vec![TokenKind::Pipe]);
    }

    #[test]
    fn treats_fn_and_let_as_plain_identifiers() {
        assert_eq!(
            kinds("local fn = 1 let = fn"),
            vec![
                TokenKind::Local,
                TokenKind::Identifier("fn".into()),
                TokenKind::Equal,
                TokenKind::Number("1".into()),
                TokenKind::Identifier("let".into()),
                TokenKind::Equal,
                TokenKind::Identifier("fn".into()),
            ]
        );
    }

    #[test]
    fn distinguishes_keywords_from_identifiers() {
        assert_eq!(
            kinds("function local for ifelse trueish not_a_keyword _foo Function const"),
            vec![
                TokenKind::Function,
                TokenKind::Local,
                TokenKind::For,
                TokenKind::Identifier("ifelse".into()),
                TokenKind::Identifier("trueish".into()),
                TokenKind::Identifier("not_a_keyword".into()),
                TokenKind::Identifier("_foo".into()),
                TokenKind::Identifier("Function".into()),
                TokenKind::Identifier("const".into()),
            ]
        );
    }

    #[test]
    fn tokenizes_compound_punctuation() {
        assert_eq!(
            kinds("== :: : = += -> < > .."),
            vec![
                TokenKind::EqualEqual,
                TokenKind::ColonColon,
                TokenKind::Colon,
                TokenKind::Equal,
                TokenKind::PlusEqual,
                TokenKind::Arrow,
                TokenKind::Less,
                TokenKind::Greater,
                TokenKind::DoubleDot,
            ]
        );
    }

    #[test]
    fn tokenizes_compound_assignment_operators() {
        assert_eq!(
            kinds("+= -= *= /= //= %= ^= ..="),
            vec![
                TokenKind::PlusEqual,
                TokenKind::MinusEqual,
                TokenKind::StarEqual,
                TokenKind::SlashEqual,
                TokenKind::DoubleSlashEqual,
                TokenKind::PercentEqual,
                TokenKind::CaretEqual,
                TokenKind::DoubleDotEqual,
            ]
        );
    }

    #[test]
    fn distinguishes_compound_assignment_from_base_operators() {
        // The non-`=` forms must still tokenize as their plain operators.
        assert_eq!(
            kinds("- * / // % ^ .. ..."),
            vec![
                TokenKind::Minus,
                TokenKind::Star,
                TokenKind::Slash,
                TokenKind::DoubleSlash,
                TokenKind::Percent,
                TokenKind::Caret,
                TokenKind::DoubleDot,
                TokenKind::TripleDot,
            ]
        );
    }

    #[test]
    fn records_spans_for_tokens() {
        let source = "local x = 42";
        let tokens = lex(source).expect("lex should succeed");
        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Local,
                    span: Span { start: 0, end: 5 },
                },
                Token {
                    kind: TokenKind::Identifier("x".into()),
                    span: Span { start: 6, end: 7 },
                },
                Token {
                    kind: TokenKind::Equal,
                    span: Span { start: 8, end: 9 },
                },
                Token {
                    kind: TokenKind::Number("42".into()),
                    span: Span { start: 10, end: 12 },
                },
            ]
        );
    }

    #[test]
    fn records_spans_after_leading_whitespace() {
        let source = "  return 1";
        let tokens = lex(source).expect("lex should succeed");
        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Return,
                    span: Span { start: 2, end: 8 },
                },
                Token {
                    kind: TokenKind::Number("1".into()),
                    span: Span { start: 9, end: 10 },
                },
            ]
        );
    }

    #[test]
    fn skips_comments_containing_multibyte_characters() {
        assert_eq!(
            kinds("-- em dash \u{2014} here\nlocal a = 1"),
            vec![
                TokenKind::Local,
                TokenKind::Identifier("a".into()),
                TokenKind::Equal,
                TokenKind::Number("1".into()),
            ]
        );
    }

    #[test]
    fn tokenizes_comparison_and_semicolon_punctuation() {
        assert_eq!(
            kinds("< <= > >= ;"),
            vec![
                TokenKind::Less,
                TokenKind::LessEqual,
                TokenKind::Greater,
                TokenKind::GreaterEqual,
                TokenKind::Semicolon,
            ]
        );
    }

    #[test]
    fn tokenizes_decimal_string_escapes() {
        assert_eq!(
            kinds(r#"'\0' '\65b' '\200' '\0500'"#),
            vec![
                TokenKind::Str("\u{0}".into()),
                TokenKind::Str("Ab".into()),
                TokenKind::Str("\u{c8}".into()),
                TokenKind::Str("20".into()),
            ]
        );
        assert_eq!(
            err(r#"'\256'"#).to_string(),
            "decimal string escape '\\256' exceeds 255"
        );
    }

    #[test]
    fn tokenizes_hex_unicode_and_skip_escapes() {
        assert_eq!(
            kinds(
                r#"'\x41\x62' '\u{48}\u{1F600}' "a\z
                     b" '\a\b\f\v'"#
            ),
            vec![
                TokenKind::Str("Ab".into()),
                TokenKind::Str("H\u{1F600}".into()),
                TokenKind::Str("ab".into()),
                TokenKind::Str("\u{7}\u{8}\u{c}\u{b}".into()),
            ]
        );
        assert_eq!(
            err(r#"'\xg1'"#).to_string(),
            "string \\x escape requires two hex digits"
        );
        assert_eq!(
            err(r#"'\u{}'"#).to_string(),
            "string \\u escape requires hex digits followed by '}'"
        );
        assert_eq!(
            err(r#"'\u{110000}'"#).to_string(),
            "string \\u escape 'u{110000}' is not a valid codepoint"
        );
    }

    #[test]
    fn desugars_backtick_interpolation_to_concat() {
        // `a{x}b` => ("a" .. tostring(x) .. "b")
        assert_eq!(
            kinds("`a{x}b`"),
            vec![
                TokenKind::LParen,
                TokenKind::Str("a".into()),
                TokenKind::DoubleDot,
                TokenKind::Identifier("tostring".into()),
                TokenKind::LParen,
                TokenKind::Identifier("x".into()),
                TokenKind::RParen,
                TokenKind::DoubleDot,
                TokenKind::Str("b".into()),
                TokenKind::RParen,
            ]
        );
        // Expression-only and empty strings still produce string-typed results.
        assert_eq!(
            kinds("`{1 + 2}`"),
            vec![
                TokenKind::LParen,
                TokenKind::Identifier("tostring".into()),
                TokenKind::LParen,
                TokenKind::Number("1".into()),
                TokenKind::Plus,
                TokenKind::Number("2".into()),
                TokenKind::RParen,
                TokenKind::RParen,
            ]
        );
        assert_eq!(
            kinds("``"),
            vec![
                TokenKind::LParen,
                TokenKind::Str(String::new()),
                TokenKind::RParen,
            ]
        );
    }

    #[test]
    fn lexes_nested_braces_and_strings_inside_interpolation() {
        // A table literal inside the interpolation keeps its own braces, and a
        // string containing '}' does not end the interpolation early.
        assert_eq!(
            kinds(r#"`v={#{1}} s={"}"}`"#),
            vec![
                TokenKind::LParen,
                TokenKind::Str("v=".into()),
                TokenKind::DoubleDot,
                TokenKind::Identifier("tostring".into()),
                TokenKind::LParen,
                TokenKind::Hash,
                TokenKind::LBrace,
                TokenKind::Number("1".into()),
                TokenKind::RBrace,
                TokenKind::RParen,
                TokenKind::DoubleDot,
                TokenKind::Str(" s=".into()),
                TokenKind::DoubleDot,
                TokenKind::Identifier("tostring".into()),
                TokenKind::LParen,
                TokenKind::Str("}".into()),
                TokenKind::RParen,
                TokenKind::RParen,
            ]
        );
    }

    #[test]
    fn escapes_braces_and_backticks_in_interpolated_strings() {
        assert_eq!(
            kinds(r"`\{not expr\} \` done`"),
            vec![
                TokenKind::LParen,
                TokenKind::Str("{not expr} ` done".into()),
                TokenKind::RParen,
            ]
        );
    }

    #[test]
    fn rejects_malformed_interpolated_strings() {
        assert_eq!(
            err("`open").to_string(),
            "unterminated interpolated string literal"
        );
        assert_eq!(
            err("`a{1`").to_string(),
            "unterminated interpolated string literal"
        );
        assert_eq!(
            err("`a{}b`").to_string(),
            "empty interpolation '{}' in backtick string"
        );
    }

    #[test]
    fn tokenizes_string_literals_with_escapes() {
        assert_eq!(
            kinds(r#""./math" "a\tb\n" "quote\"end""#),
            vec![
                TokenKind::Str("./math".into()),
                TokenKind::Str("a\tb\n".into()),
                TokenKind::Str("quote\"end".into()),
            ]
        );
    }

    #[test]
    fn tokenizes_single_quoted_string_literals_with_escapes() {
        assert_eq!(
            kinds(r#"'abc' 'it\'s' 'a\\b\n'"#),
            vec![
                TokenKind::Str("abc".into()),
                TokenKind::Str("it's".into()),
                TokenKind::Str("a\\b\n".into()),
            ]
        );
    }

    #[test]
    fn tokenizes_bytes_literals_with_hex_and_ascii_escapes() {
        assert_eq!(
            kinds(r#"b"ABC\x00\t\"""#),
            vec![TokenKind::Bytes(vec![65, 66, 67, 0, 9, 34])]
        );
    }

    #[test]
    fn rejects_unterminated_and_invalid_strings() {
        assert_eq!(err("\"open").to_string(), "unterminated string literal");
        assert_eq!(
            err("\"line\nbreak\"").to_string(),
            "unterminated string literal"
        );
        assert_eq!(
            err("\"\\q\"").to_string(),
            "unsupported string escape '\\q'"
        );
    }

    #[test]
    fn rejects_invalid_bytes_literals() {
        assert_eq!(err("b\"open").to_string(), "unterminated bytes literal");
        assert_eq!(
            err("b\"\\x0\"").to_string(),
            "bytes literal \\x escape requires two hex digits"
        );
        assert_eq!(
            err("b\"é\"").to_string(),
            "bytes literal only supports ASCII source characters, got 'é'"
        );
    }

    #[test]
    fn rejects_unexpected_characters() {
        assert_eq!(err("@").to_string(), "unexpected character '@'");
        assert_eq!(err("local $").to_string(), "unexpected character '$'");
    }

    #[test]
    fn skips_lua_comment_syntax() {
        assert_eq!(
            kinds("local x = 1 -- comment\n-- whole line\nx = x + 1"),
            vec![
                TokenKind::Local,
                TokenKind::Identifier("x".into()),
                TokenKind::Equal,
                TokenKind::Number("1".into()),
                TokenKind::Identifier("x".into()),
                TokenKind::Equal,
                TokenKind::Identifier("x".into()),
                TokenKind::Plus,
                TokenKind::Number("1".into()),
            ]
        );
    }

    #[test]
    fn tokenizes_break_and_continue_keywords() {
        assert_eq!(
            kinds("break continue"),
            vec![TokenKind::Break, TokenKind::Continue]
        );
    }

    #[test]
    fn skips_block_comments() {
        assert_eq!(
            kinds("local x = 1 --[[ block\ncomment ]] x = x + 1"),
            vec![
                TokenKind::Local,
                TokenKind::Identifier("x".into()),
                TokenKind::Equal,
                TokenKind::Number("1".into()),
                TokenKind::Identifier("x".into()),
                TokenKind::Equal,
                TokenKind::Identifier("x".into()),
                TokenKind::Plus,
                TokenKind::Number("1".into()),
            ]
        );
    }

    #[test]
    fn tokenizes_caret_operator() {
        assert_eq!(
            kinds("2 ^ 3"),
            vec![
                TokenKind::Number("2".into()),
                TokenKind::Caret,
                TokenKind::Number("3".into()),
            ]
        );
    }

    #[test]
    fn tokenizes_long_strings() {
        assert_eq!(kinds("[[hello]]"), vec![TokenKind::Str("hello".into())]);
        // A newline immediately after the opening bracket is dropped.
        assert_eq!(kinds("[[\nline]]"), vec![TokenKind::Str("line".into())]);
        // Leveled brackets let the content contain `]]`.
        assert_eq!(kinds("[==[a]]b]==]"), vec![TokenKind::Str("a]]b".into())]);
        // No escape processing inside long strings.
        assert_eq!(kinds(r"[[a\tb]]"), vec![TokenKind::Str(r"a\tb".into())]);
    }

    #[test]
    fn distinguishes_index_brackets_from_long_strings() {
        assert_eq!(
            kinds("a[b]"),
            vec![
                TokenKind::Identifier("a".into()),
                TokenKind::LBracket,
                TokenKind::Identifier("b".into()),
                TokenKind::RBracket,
            ]
        );
    }

    #[test]
    fn reports_unterminated_long_strings() {
        assert_eq!(
            err("[[ never ends").to_string(),
            "unterminated long string '[[...]]'"
        );
        assert_eq!(
            err("[==[ unmatched ]=]").to_string(),
            "unterminated long string '[[...]]'"
        );
    }

    #[test]
    fn skips_leveled_block_comments() {
        assert_eq!(
            kinds("local x --[==[ a ]] still comment ]==]\n= 1"),
            vec![
                TokenKind::Local,
                TokenKind::Identifier("x".into()),
                TokenKind::Equal,
                TokenKind::Number("1".into()),
            ]
        );
    }

    #[test]
    fn reports_unterminated_block_comments() {
        assert_eq!(
            err("local x = 1 --[[ never ends").to_string(),
            "unterminated block comment '--[[...]]'"
        );
    }
}
