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
    Repeat,
    Until,
    Do,
    Return,
    Not,
    NumberType,
    U32Type,
    U64Type,
    I32Type,
    I64Type,
    F32Type,
    F64Type,
    BoolType,
    True,
    False,
    Identifier(String),
    Number(String),
    Plus,
    PlusEqual,
    Minus,
    Star,
    Slash,
    DoubleSlash,
    Percent,
    Equal,
    EqualEqual,
    Less,
    Greater,
    And,
    Or,
    Arrow,
    ColonColon,
    Colon,
    Comma,
    Hash,
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
            '<' => (TokenKind::Less, 1),
            '>' => (TokenKind::Greater, 1),
            '=' => {
                if matches!(chars.get(i + 1), Some('=')) {
                    (TokenKind::EqualEqual, 2)
                } else {
                    (TokenKind::Equal, 1)
                }
            }
            '-' => {
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
                    return Err(Diagnostic::new("unexpected '|', expected '||'"));
                }
            }
            d if d.is_ascii_digit() => {
                let mut end = i + 1;
                while end < chars.len() && (chars[end].is_ascii_digit() || chars[end] == '.') {
                    end += 1;
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
                    "repeat" => TokenKind::Repeat,
                    "until" => TokenKind::Until,
                    "do" => TokenKind::Do,
                    "return" => TokenKind::Return,
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
                    "bool" => TokenKind::BoolType,
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
        assert_eq!(err("1..2").to_string(), "invalid number literal");
        assert_eq!(err("12.34.56").to_string(), "invalid number literal");
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
        assert_eq!(err("|").to_string(), "unexpected '|', expected '||'");
    }

    #[test]
    fn rejects_alternate_keyword_spellings() {
        assert_eq!(err("fn").to_string(), "unsupported 'fn', use 'function'");
        assert_eq!(err("let").to_string(), "unsupported 'let', use 'local'");
    }

    #[test]
    fn distinguishes_keywords_from_identifiers() {
        assert_eq!(
            kinds("function local ifelse trueish not_a_keyword _foo Function const"),
            vec![
                TokenKind::Function,
                TokenKind::Local,
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
            kinds("== :: : = += -> < >"),
            vec![
                TokenKind::EqualEqual,
                TokenKind::ColonColon,
                TokenKind::Colon,
                TokenKind::Equal,
                TokenKind::PlusEqual,
                TokenKind::Arrow,
                TokenKind::Less,
                TokenKind::Greater,
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
    fn rejects_unexpected_characters() {
        assert_eq!(err("@").to_string(), "unexpected character '@'");
        assert_eq!(err("local $").to_string(), "unexpected character '$'");
    }
}
