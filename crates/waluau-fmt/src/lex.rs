//! Turn raw lossless lexer output into significant tokens carrying comment and
//! blank-line trivia, ready for the parser to consume.
//!
//! [`lex_with_trivia`](waluau_lexer::lex_with_trivia) yields comments and raw
//! interpolation inline with the code tokens. Here we split that stream into
//! *significant* tokens (everything the grammar reacts to) and attach each
//! comment to a neighbouring token — trailing the previous token when it shares
//! its line, otherwise leading the next token. Blank lines (two or more
//! newlines) are recorded so the formatter can preserve paragraph breaks.

use waluau_diagnostics::Diagnostic;
use waluau_lexer::{Token, TokenKind, lex_with_trivia};

use crate::cst::{Comment, SynToken};

/// The significant-token view of a source file.
pub struct TokenStream {
    /// Significant (code) tokens with attached leading/trailing comments.
    pub tokens: Vec<SynToken>,
    /// Comments after the last code token, with nothing following them.
    pub trailing_comments: Vec<Comment>,
}

/// Preprocess `source` into significant tokens with attached trivia.
pub fn significant_tokens(source: &str) -> Result<TokenStream, Diagnostic> {
    let chars: Vec<char> = source.chars().collect();
    let raw = lex_with_trivia(source)?;

    let mut out: Vec<SynToken> = Vec::new();
    // Comments seen since the last significant token that will lead the next
    // significant token.
    let mut pending_leading: Vec<Comment> = Vec::new();
    // Char offset just past the previously seen raw token (comment or code).
    let mut prev_end: Option<u32> = None;
    // Whether the most recent raw token was a significant (code) token, so a
    // following same-line comment can trail it.
    let mut last_was_significant = false;

    for Token { kind, span } in raw {
        let newlines = match prev_end {
            Some(end) => count_newlines(&chars, end, span.start),
            None => 0,
        };
        let blank_before = newlines >= 2;

        match comment_text(&kind) {
            Some((text, block)) => {
                if newlines == 0 && last_was_significant {
                    // Same line as the code before it: a trailing comment.
                    if let Some(last) = out.last_mut() {
                        last.trailing = Some(Comment {
                            text,
                            block,
                            blank_before: false,
                        });
                    }
                } else {
                    pending_leading.push(Comment {
                        text,
                        block,
                        blank_before,
                    });
                }
                last_was_significant = false;
            }
            None => {
                let text: String = chars[span.start as usize..span.end as usize]
                    .iter()
                    .collect();
                out.push(SynToken {
                    kind,
                    text,
                    leading: std::mem::take(&mut pending_leading),
                    trailing: None,
                    blank_before,
                });
                last_was_significant = true;
            }
        }
        prev_end = Some(span.end);
    }

    Ok(TokenStream {
        tokens: out,
        trailing_comments: pending_leading,
    })
}

/// The raw comment lexeme and whether it is a block comment, if `kind` is
/// comment trivia.
fn comment_text(kind: &TokenKind) -> Option<(String, bool)> {
    match kind {
        TokenKind::LineComment(s) => Some((s.clone(), false)),
        TokenKind::BlockComment(s) => Some((s.clone(), true)),
        _ => None,
    }
}

fn count_newlines(chars: &[char], start: u32, end: u32) -> u32 {
    chars[start as usize..end as usize]
        .iter()
        .filter(|&&c| c == '\n')
        .count() as u32
}
