use waluau_diagnostics::Diagnostic;

pub use waluau_span::Span;

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Fn,
    Let,
    If,
    Then,
    ElseIf,
    Else,
    End,
    While,
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
    Minus,
    Star,
    Slash,
    Equal,
    EqualEqual,
    Less,
    Greater,
    AndAnd,
    OrOr,
    ColonColon,
    Colon,
    Comma,
    LParen,
    RParen,
    Arrow,
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
            ':' => {
                if matches!(chars.get(i + 1), Some(':')) {
                    (TokenKind::ColonColon, 2)
                } else {
                    (TokenKind::Colon, 1)
                }
            }
            ',' => (TokenKind::Comma, 1),
            '+' => (TokenKind::Plus, 1),
            '*' => (TokenKind::Star, 1),
            '/' => (TokenKind::Slash, 1),
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
                    (TokenKind::AndAnd, 2)
                } else {
                    return Err(Diagnostic::new("unexpected '&', expected '&&'"));
                }
            }
            '|' => {
                if matches!(chars.get(i + 1), Some('|')) {
                    (TokenKind::OrOr, 2)
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
                    "fn" => TokenKind::Fn,
                    "let" => TokenKind::Let,
                    "if" => TokenKind::If,
                    "then" => TokenKind::Then,
                    "elseif" => TokenKind::ElseIf,
                    "else" => TokenKind::Else,
                    "end" => TokenKind::End,
                    "while" => TokenKind::While,
                    "do" => TokenKind::Do,
                    "return" => TokenKind::Return,
                    "not" => TokenKind::Not,
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
