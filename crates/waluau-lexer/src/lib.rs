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
    Star,
    Slash,
    DoubleSlash,
    Percent,
    Equal,
    EqualEqual,
    TildeEqual,
    Less,
    Greater,
    And,
    Or,
    Pipe,
    Arrow,
    ColonColon,
    Colon,
    Dot,
    DoubleDot,
    Comma,
    Hash,
    Question,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    LParen,
    RParen,
}

pub fn lex(source: &str) -> Result<Vec<Token>, Diagnostic> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = source.chars().collect();
    let mut i = 0usize;

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
            '[' => (TokenKind::LBracket, 1),
            ']' => (TokenKind::RBracket, 1),
            '{' => (TokenKind::LBrace, 1),
            '}' => (TokenKind::RBrace, 1),
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
            '+' => {
                if matches!(chars.get(i + 1), Some('=')) {
                    (TokenKind::PlusEqual, 2)
                } else {
                    (TokenKind::Plus, 1)
                }
            }
            '*' => (TokenKind::Star, 1),
            '/' => {
                if matches!(chars.get(i + 1), Some('/')) {
                    (TokenKind::DoubleSlash, 2)
                } else {
                    (TokenKind::Slash, 1)
                }
            }
            '%' => (TokenKind::Percent, 1),
            '.' => {
                if matches!(chars.get(i + 1), Some('.')) {
                    (TokenKind::DoubleDot, 2)
                } else {
                    (TokenKind::Dot, 1)
                }
            }
            '<' => (TokenKind::Less, 1),
            '>' => (TokenKind::Greater, 1),
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
                    if matches!(chars.get(i + 2), Some('['))
                        && matches!(chars.get(i + 3), Some('['))
                    {
                        let mut end = i + 4;
                        while end + 1 < chars.len() {
                            if chars[end] == ']' && chars[end + 1] == ']' {
                                break;
                            }
                            end += 1;
                        }
                        if end + 1 >= chars.len() {
                            return Err(Diagnostic::new("unterminated block comment '--[[...]]'"));
                        }
                        i = end + 2;
                        continue;
                    }
                    let mut end = i + 2;
                    while end < chars.len() && chars[end] != '\n' {
                        end += 1;
                    }
                    i = end;
                    continue;
                }
                if matches!(chars.get(i + 1), Some('>')) {
                    (TokenKind::Arrow, 2)
                } else {
                    (TokenKind::Minus, 1)
                }
            }
            '&' => {
                if matches!(chars.get(i + 1), Some('&')) {
                    return Err(Diagnostic::new("unsupported '&&', use 'and'"));
                } else {
                    return Err(Diagnostic::new("unexpected '&', expected '&&'"));
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
                let (value, end) = parse_string_literal(&chars, i)?;
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
                let (value, end) = parse_bytes_literal(&chars, i + 1)?;
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
                while end < chars.len() {
                    if chars[end].is_ascii_digit() {
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
                let number = source[i..end].to_string();
                if number.matches('.').count() > 1 {
                    return Err(Diagnostic::new("invalid number literal"));
                }
                if number.contains('.') {
                    number
                        .parse::<f64>()
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
                let text = &source[i..end];
                let kind = match text {
                    "fn" => {
                        return Err(Diagnostic::new("unsupported 'fn', use 'function'"));
                    }
                    "let" => {
                        return Err(Diagnostic::new("unsupported 'let', use 'local'"));
                    }
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

    Ok(tokens)
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
                end += 1;
                match chars.get(end) {
                    Some('"') => value.push('"'),
                    Some('\'') => value.push('\''),
                    Some('\\') => value.push('\\'),
                    Some('n') => value.push('\n'),
                    Some('t') => value.push('\t'),
                    Some(other) => {
                        return Err(Diagnostic::new(format!(
                            "unsupported string escape '\\{other}'"
                        )));
                    }
                    None => return Err(Diagnostic::new("unterminated string literal")),
                }
                end += 1;
            }
            Some(other) => {
                value.push(*other);
                end += 1;
            }
        }
    }
    Ok((value, end))
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
    use super::{Token, TokenKind, lex};
    use waluau_diagnostics::Diagnostic;
    use waluau_span::Span;

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
        assert_eq!(err("&").to_string(), "unexpected '&', expected '&&'");
        assert_eq!(kinds("|"), vec![TokenKind::Pipe]);
    }

    #[test]
    fn rejects_alternate_keyword_spellings() {
        assert_eq!(err("fn").to_string(), "unsupported 'fn', use 'function'");
        assert_eq!(err("let").to_string(), "unsupported 'let', use 'local'");
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
    fn reports_unterminated_block_comments() {
        assert_eq!(
            err("local x = 1 --[[ never ends").to_string(),
            "unterminated block comment '--[[...]]'"
        );
    }
}
