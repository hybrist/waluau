use waluau_diagnostics::Diagnostic;
use waluau_lexer::{Token, TokenKind};

use super::Parser;

impl Parser {
    pub(super) fn expect_identifier(&mut self) -> Result<String, Diagnostic> {
        self.expect_identifier_spanned().map(|(name, _)| name)
    }

    pub(super) fn expect_identifier_spanned(
        &mut self,
    ) -> Result<(String, waluau_ast::Span), Diagnostic> {
        match self.advance() {
            Some(Token {
                kind: TokenKind::Identifier(name),
                span,
            }) => Ok((name, span)),
            Some(_) => {
                // Leave the unexpected token unconsumed so statement-level
                // recovery can resynchronize on it (it may close a block).
                let diagnostic = self.diagnostic_at_current("expected identifier");
                self.index -= 1;
                Err(diagnostic)
            }
            None => Err(self.diagnostic_at_current("expected identifier")),
        }
    }

    pub(super) fn expect_simple(
        &mut self,
        expected: TokenKind,
        message: &str,
    ) -> Result<(), Diagnostic> {
        let token = self
            .advance()
            .ok_or_else(|| self.end_of_input_diagnostic())?;
        if same_variant(&token.kind, &expected) {
            Ok(())
        } else {
            // Leave the unexpected token unconsumed so statement-level
            // recovery can resynchronize on it (it may close a block).
            self.index -= 1;
            Err(Diagnostic::new(message).with_span(token.span))
        }
    }

    pub(super) fn end_of_input_diagnostic(&self) -> Diagnostic {
        let diagnostic = Diagnostic::new("unexpected end of input");
        match self.tokens.last() {
            Some(token) => diagnostic.with_span(token.span),
            None => diagnostic,
        }
    }

    pub(super) fn check_simple(&self, expected: &TokenKind) -> bool {
        self.peek()
            .map(|token| same_variant(&token.kind, expected))
            .unwrap_or(false)
    }

    /// True when the next token can close a `<...>` type list: either `>`
    /// itself, or a `>=` that greedy lexing produced from `Foo<T>=value`.
    pub(super) fn check_greater(&self) -> bool {
        matches!(
            self.peek().map(|token| &token.kind),
            Some(TokenKind::Greater | TokenKind::GreaterEqual)
        )
    }

    /// Consume a `>` closing a `<...>` type list. A `>=` token is split in
    /// place: the `>` half is consumed and an `=` token is left behind, so
    /// `local x: Foo<T>=value` parses the same as `local x: Foo<T> = value`.
    pub(super) fn expect_greater(&mut self, message: &str) -> Result<(), Diagnostic> {
        match self.peek().map(|token| &token.kind) {
            Some(TokenKind::Greater) => {
                self.advance();
                Ok(())
            }
            Some(TokenKind::GreaterEqual) => {
                let token = &mut self.tokens[self.index];
                token.kind = TokenKind::Equal;
                token.span.start += 1;
                Ok(())
            }
            _ => self.expect_simple(TokenKind::Greater, message),
        }
    }

    pub(super) fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.index)
    }

    pub(super) fn peek_n(&self, n: usize) -> Option<&Token> {
        self.tokens.get(self.index + n)
    }

    pub(super) fn advance(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.index).cloned();
        self.index += usize::from(token.is_some());
        token
    }

    pub(super) fn record_error(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    pub(super) fn diagnostic_at_current(&self, message: &str) -> Diagnostic {
        if self.index == 0 {
            return Diagnostic::new(message);
        }

        if let Some(token) = self.tokens.get(self.index.saturating_sub(1)) {
            Diagnostic::new(message).with_span(token.span)
        } else {
            Diagnostic::new(message)
        }
    }

    pub(super) fn sync_to_next_function(&mut self) {
        while let Some(token) = self.peek() {
            if matches!(token.kind, TokenKind::Function) {
                return;
            }
            self.advance();
        }
    }

    pub(super) fn synchronize_statement(&mut self, end_markers: &[TokenKind], start_index: usize) {
        let mut depth = 0usize;
        while let Some(token) = self.peek() {
            if depth == 0
                && (is_statement_start(&token.kind)
                    || end_markers
                        .iter()
                        .any(|marker| same_variant(&token.kind, marker)))
            {
                if self.index == start_index {
                    self.advance();
                    continue;
                }
                return;
            }

            match token.kind {
                TokenKind::If
                | TokenKind::While
                | TokenKind::For
                | TokenKind::Repeat
                | TokenKind::Function => depth += 1,
                TokenKind::End if depth > 0 => depth -= 1,
                _ => {}
            }
            self.advance();
        }

        if self.index == start_index {
            self.advance();
        }
    }
}

fn is_statement_start(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Local
            | TokenKind::Function
            | TokenKind::If
            | TokenKind::While
            | TokenKind::For
            | TokenKind::Repeat
            | TokenKind::Return
            | TokenKind::Break
            | TokenKind::Continue
            | TokenKind::Identifier(_)
    )
}

pub(super) fn same_variant(a: &TokenKind, b: &TokenKind) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}
