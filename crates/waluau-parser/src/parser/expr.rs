use waluau_ast::{BinaryOp, Expr, NumberLiteral, Span, TableField, UnaryOp};
use waluau_diagnostics::Diagnostic;
use waluau_lexer::{Token, TokenKind};

use super::Parser;

impl Parser {
    fn check_method_call_start(&self) -> bool {
        matches!(
            (
                self.peek().map(|token| &token.kind),
                self.peek_n(1).map(|token| &token.kind),
            ),
            (Some(TokenKind::Colon), Some(TokenKind::Identifier(_)),)
        ) && (matches!(
            self.peek_n(2).map(|token| &token.kind),
            Some(TokenKind::LParen)
                | Some(TokenKind::Less)
                | Some(TokenKind::Str(_))
                | Some(TokenKind::LBrace)
        ))
    }

    /// Lua's call-argument sugar: a call may take a single string literal or
    /// table constructor in place of a parenthesized argument list, e.g.
    /// `obj:method "text"` or `make_thing { x = 0, y = 1 }`.
    fn check_call_args_start(&self) -> bool {
        matches!(
            self.peek().map(|token| &token.kind),
            Some(TokenKind::LParen) | Some(TokenKind::Str(_)) | Some(TokenKind::LBrace)
        )
    }

    fn parse_call_args(&mut self) -> Result<(Vec<Expr>, u32, u32), Diagnostic> {
        if let Some(Token {
            kind: TokenKind::Str(_),
            span,
        }) = self.peek()
        {
            let span = *span;
            let value = match self.advance().expect("peeked string token").kind {
                TokenKind::Str(value) => value,
                _ => unreachable!("peeked string token"),
            };
            return Ok((vec![Expr::String(value, Some(span))], span.start, span.end));
        }
        if self.check_simple(&TokenKind::LBrace) {
            let start_pos = self.peek().map(|token| token.span.start).unwrap_or(0);
            self.advance();
            let arg = self.parse_brace_literal(start_pos)?;
            let end_pos = arg.span().map(|s| s.end).unwrap_or(start_pos);
            return Ok((vec![arg], start_pos, end_pos));
        }

        let call_start = self.peek().map(|token| token.span.start).unwrap_or(0);
        self.expect_simple(TokenKind::LParen, "expected '('")?;
        let mut args = Vec::new();
        if !self.check_simple(&TokenKind::RParen) {
            loop {
                args.push(self.parse_expr()?);
                if self.check_simple(&TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        let call_end = self
            .peek()
            .map(|token| token.span.end)
            .ok_or_else(|| Diagnostic::new("expected ')' after call arguments"))?;
        self.expect_simple(TokenKind::RParen, "expected ')' after call arguments")?;
        Ok((args, call_start, call_end))
    }

    pub(super) fn try_parse_type_arg_list(&mut self) -> Option<Vec<waluau_ast::Type>> {
        if !self.check_simple(&TokenKind::Less) {
            return None;
        }
        let checkpoint = self.index;
        self.advance();
        if self.check_simple(&TokenKind::Greater) {
            self.index = checkpoint;
            return None;
        }
        let mut type_args = Vec::new();
        loop {
            match self.parse_type() {
                Ok(ty) => type_args.push(ty),
                Err(_) => {
                    self.index = checkpoint;
                    return None;
                }
            }
            if self.check_simple(&TokenKind::Comma) {
                self.advance();
                continue;
            }
            if self.check_simple(&TokenKind::Greater) {
                self.advance();
                break;
            }
            self.index = checkpoint;
            return None;
        }
        if self.check_simple(&TokenKind::LParen) {
            Some(type_args)
        } else {
            self.index = checkpoint;
            None
        }
    }

    pub(super) fn parse_expr(&mut self) -> Result<Expr, Diagnostic> {
        if self.check_simple(&TokenKind::If) {
            return self.parse_if_expr();
        }
        self.parse_or()
    }

    fn parse_if_expr(&mut self) -> Result<Expr, Diagnostic> {
        let start_pos = self.peek().map(|t| t.span.start).unwrap_or(0);
        self.expect_simple(TokenKind::If, "expected 'if'")?;
        let condition = self.parse_expr()?;
        self.expect_simple(TokenKind::Then, "expected 'then' after if condition")?;
        let then_expr = self.parse_expr()?;
        self.expect_simple(TokenKind::Else, "expected 'else' in if expression")?;
        let else_expr = self.parse_expr()?;
        let end_pos = else_expr.span().map(|s| s.end).unwrap_or(start_pos);
        Ok(Expr::If {
            condition: Box::new(condition),
            then_expr: Box::new(then_expr),
            else_expr: Box::new(else_expr),
            span: Some(Span {
                start: start_pos,
                end: end_pos,
            }),
        })
    }

    fn parse_or(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_binary(Parser::parse_and, &[TokenKind::Or], &[BinaryOp::Or])
    }

    fn parse_and(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_binary(
            Parser::parse_comparison,
            &[TokenKind::And],
            &[BinaryOp::And],
        )
    }

    fn parse_comparison(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_concat()?;
        loop {
            if matches!(self.peek().map(|token| &token.kind), Some(TokenKind::Identifier(name)) if name == "is")
            {
                let start_pos = expr.span().map(|s| s.start).unwrap_or(0);
                self.advance();
                let tag = self.expect_identifier()?;
                let end_token = self.tokens.get(self.index.saturating_sub(1));
                let end_pos = end_token.map(|t| t.span.end).unwrap_or(start_pos);
                expr = Expr::IsVariant {
                    expr: Box::new(expr),
                    tag,
                    span: Some(Span {
                        start: start_pos,
                        end: end_pos,
                    }),
                };
                continue;
            }

            let Some(next) = self.peek() else {
                break;
            };
            let op = match next.kind {
                TokenKind::EqualEqual => BinaryOp::Eq,
                TokenKind::TildeEqual => BinaryOp::NotEq,
                TokenKind::Less => BinaryOp::Less,
                TokenKind::Greater => BinaryOp::Greater,
                _ => break,
            };
            let start_pos = expr.span().map(|s| s.start).unwrap_or(0);
            self.advance();
            let right = self.parse_concat()?;
            let end_pos = right.span().map(|s| s.end).unwrap_or(start_pos);
            expr = Expr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
                resolved_name: None,
                span: Some(Span {
                    start: start_pos,
                    end: end_pos,
                }),
            };
        }
        Ok(expr)
    }

    fn parse_concat(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_binary(
            Parser::parse_add,
            &[TokenKind::DoubleDot],
            &[BinaryOp::Concat],
        )
    }

    fn parse_add(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_binary(
            Parser::parse_mul,
            &[TokenKind::Plus, TokenKind::Minus],
            &[BinaryOp::Add, BinaryOp::Sub],
        )
    }

    fn parse_mul(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_binary(
            Parser::parse_cast,
            &[
                TokenKind::Star,
                TokenKind::Slash,
                TokenKind::DoubleSlash,
                TokenKind::Percent,
            ],
            &[
                BinaryOp::Mul,
                BinaryOp::Div,
                BinaryOp::FloorDiv,
                BinaryOp::Mod,
            ],
        )
    }

    fn parse_cast(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_unary()?;
        while self.check_simple(&TokenKind::ColonColon) {
            let start_pos = expr.span().map(|s| s.start).unwrap_or(0);
            self.advance();
            let ty = self.parse_type()?;
            let end_token = self.tokens.get(self.index.saturating_sub(1));
            let end_pos = end_token.map(|t| t.span.end).unwrap_or(start_pos);
            expr = Expr::Cast {
                expr: Box::new(expr),
                ty,
                span: Some(Span {
                    start: start_pos,
                    end: end_pos,
                }),
            };
        }
        Ok(expr)
    }

    fn parse_unary(&mut self) -> Result<Expr, Diagnostic> {
        if self.check_simple(&TokenKind::Minus) {
            let start_pos = self.peek().map(|t| t.span.start).unwrap_or(0);
            self.advance();
            let operand = self.parse_unary()?;
            let end_pos = operand.span().map(|s| s.end).unwrap_or(start_pos);
            return Ok(Expr::Unary {
                op: UnaryOp::Neg,
                expr: Box::new(operand),
                resolved_name: None,
                span: Some(Span {
                    start: start_pos,
                    end: end_pos,
                }),
            });
        }
        if self.check_simple(&TokenKind::Not) {
            let start_pos = self.peek().map(|t| t.span.start).unwrap_or(0);
            self.advance();
            let operand = self.parse_unary()?;
            let end_pos = operand.span().map(|s| s.end).unwrap_or(start_pos);
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(operand),
                resolved_name: None,
                span: Some(Span {
                    start: start_pos,
                    end: end_pos,
                }),
            });
        }
        if self.check_simple(&TokenKind::Hash) {
            let start_pos = self.peek().map(|t| t.span.start).unwrap_or(0);
            self.advance();
            let operand = self.parse_unary()?;
            let end_pos = operand.span().map(|s| s.end).unwrap_or(start_pos);
            return Ok(Expr::Unary {
                op: UnaryOp::Len,
                expr: Box::new(operand),
                resolved_name: None,
                span: Some(Span {
                    start: start_pos,
                    end: end_pos,
                }),
            });
        }
        self.parse_pow()
    }

    /// Exponentiation binds tighter than the unary operators and is
    /// right-associative, so `-2 ^ 2` is `-(2 ^ 2)` and `2 ^ 2 ^ 3` is
    /// `2 ^ (2 ^ 3)`. The exponent is parsed as a unary expression so
    /// `2 ^ -3` is accepted.
    fn parse_pow(&mut self) -> Result<Expr, Diagnostic> {
        let base = self.parse_postfix_expr()?;
        if self.check_simple(&TokenKind::Caret) {
            let start_pos = base.span().map(|s| s.start).unwrap_or(0);
            self.advance();
            let exponent = self.parse_unary()?;
            let end_pos = exponent.span().map(|s| s.end).unwrap_or(start_pos);
            return Ok(Expr::Binary {
                op: BinaryOp::Pow,
                left: Box::new(base),
                right: Box::new(exponent),
                resolved_name: None,
                span: Some(Span {
                    start: start_pos,
                    end: end_pos,
                }),
            });
        }
        Ok(base)
    }

    pub(super) fn parse_postfix_expr(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_primary()?;
        loop {
            let start_pos = expr.span().map(|s| s.start).unwrap_or(0);
            if self.check_simple(&TokenKind::Dot) {
                self.advance();
                let name = self.expect_identifier()?;
                let end_token = self.tokens.get(self.index.saturating_sub(1));
                let end_pos = end_token.map(|t| t.span.end).unwrap_or(start_pos);
                expr = Expr::Field {
                    base: Box::new(expr),
                    name,
                    resolved_name: None,
                    span: Some(Span {
                        start: start_pos,
                        end: end_pos,
                    }),
                };
                continue;
            }
            if self.check_simple(&TokenKind::LBracket) {
                self.advance();
                let index = self.parse_expr()?;
                self.expect_simple(TokenKind::RBracket, "expected ']' after array index")?;
                let end_token = self.tokens.get(self.index.saturating_sub(1));
                let end_pos = end_token.map(|t| t.span.end).unwrap_or(start_pos);
                expr = Expr::Index {
                    base: Box::new(expr),
                    index: Box::new(index),
                    span: Some(Span {
                        start: start_pos,
                        end: end_pos,
                    }),
                };
                continue;
            }
            if self.check_method_call_start() {
                self.advance();
                let name = self.expect_identifier()?;
                let type_args = if self.check_simple(&TokenKind::Less) {
                    self.try_parse_type_arg_list().unwrap_or_default()
                } else {
                    Vec::new()
                };
                let (args, _, call_end) = self.parse_call_args()?;
                expr = Expr::MethodCall {
                    receiver: Box::new(expr),
                    name,
                    resolved_name: None,
                    type_args,
                    args,
                    span: Some(Span {
                        start: start_pos,
                        end: call_end,
                    }),
                };
                continue;
            }
            let type_args = if self.check_simple(&TokenKind::Less) {
                self.try_parse_type_arg_list().unwrap_or_default()
            } else {
                Vec::new()
            };
            if self.check_call_args_start() {
                let (args, call_start, call_end) = self.parse_call_args()?;
                expr = Expr::Call {
                    callee: Box::new(expr),
                    type_args,
                    args,
                    span: Some(Span {
                        start: call_start,
                        end: call_end,
                    }),
                    method_call_origin: None,
                };
                continue;
            }
            if !type_args.is_empty() {
                return Err(Diagnostic::new(
                    "type arguments are only allowed before a call argument list",
                ));
            }
            break;
        }
        Ok(expr)
    }

    fn parse_binary(
        &mut self,
        sub: fn(&mut Parser) -> Result<Expr, Diagnostic>,
        ops: &[TokenKind],
        mapped: &[BinaryOp],
    ) -> Result<Expr, Diagnostic> {
        let mut expr = sub(self)?;
        while let Some(next) = self.peek() {
            let mut matched = None;
            for (i, op) in ops.iter().enumerate() {
                if super::tokens::same_variant(&next.kind, op) {
                    matched = Some(mapped[i]);
                    break;
                }
            }
            if let Some(op) = matched {
                let start_pos = expr.span().map(|s| s.start).unwrap_or(0);
                self.advance();
                let right = sub(self)?;
                let end_pos = right.span().map(|s| s.end).unwrap_or(start_pos);
                expr = Expr::Binary {
                    op,
                    left: Box::new(expr),
                    right: Box::new(right),
                    resolved_name: None,
                    span: Some(Span {
                        start: start_pos,
                        end: end_pos,
                    }),
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr, Diagnostic> {
        let token = self
            .advance()
            .ok_or_else(|| Diagnostic::new("unexpected end of input"))?;
        let span = Some(token.span);
        match token.kind {
            TokenKind::Number(value) => Ok(Expr::Number(NumberLiteral { raw: value }, span)),
            TokenKind::True => Ok(Expr::Bool(true, span)),
            TokenKind::False => Ok(Expr::Bool(false, span)),
            TokenKind::Nil => Ok(Expr::Nil(span)),
            TokenKind::Identifier(name) => {
                // `require("...")` (or the sugared `require "..."`) is parsed
                // as a dedicated node so the linker can resolve module ids
                // before IR lowering.
                if name == "require"
                    && matches!(
                        self.peek().map(|token| &token.kind),
                        Some(TokenKind::LParen) | Some(TokenKind::Str(_))
                    )
                {
                    return self.parse_require(token.span.start);
                }
                Ok(Expr::Name(name, None, span))
            }
            // `string` is lexed as a reserved type keyword, but it also
            // doubles as the namespace for builtins like `string.find`.
            // Treat it as a plain name in expression position so
            // `string.find(...)` can parse like `math.floor(...)`.
            TokenKind::StringType => Ok(Expr::Name("string".to_string(), None, span)),
            TokenKind::Str(value) => Ok(Expr::String(value, span)),
            TokenKind::Bytes(value) => Ok(Expr::Bytes(value, span)),
            TokenKind::TripleDot => Ok(Expr::Vararg(span)),
            TokenKind::Function => {
                let start_pos = token.span.start;
                let name = if let Some(Token {
                    kind: TokenKind::Identifier(_),
                    ..
                }) = self.peek()
                {
                    if self.peek_n(1).is_some_and(|token| {
                        matches!(token.kind, TokenKind::LParen | TokenKind::Less)
                    }) {
                        Some(self.expect_identifier()?)
                    } else {
                        None
                    }
                } else {
                    None
                };
                Ok(Expr::Function(
                    self.parse_function_expr_tail(name, false, start_pos)?,
                ))
            }
            TokenKind::LBrace => self.parse_brace_literal(token.span.start),
            TokenKind::LParen => {
                let inner = self.parse_expr()?;
                self.expect_simple(TokenKind::RParen, "expected ')' after expression")?;
                Ok(inner)
            }
            _ => Err(self.diagnostic_at_current("expected expression")),
        }
    }

    fn parse_require(&mut self, start_pos: u32) -> Result<Expr, Diagnostic> {
        // Lua's call-argument sugar allows `require "./module"` as shorthand
        // for `require("./module")`.
        if let Some(Token {
            kind: TokenKind::Str(_),
            span,
        }) = self.peek()
        {
            let span = *span;
            let path = match self.advance().expect("peeked string token").kind {
                TokenKind::Str(path) => path,
                _ => unreachable!("peeked string token"),
            };
            return Ok(Expr::Require(
                path,
                Some(Span {
                    start: start_pos,
                    end: span.end,
                }),
            ));
        }

        self.expect_simple(TokenKind::LParen, "expected '(' after require")?;
        let path = match self.advance() {
            Some(Token {
                kind: TokenKind::Str(path),
                ..
            }) => path,
            _ => {
                return Err(Diagnostic::new(
                    "require expects a string literal path, e.g. require(\"./module\")",
                ));
            }
        };
        let end_token = self.peek().cloned();
        let end_pos = end_token.map(|t| t.span.end).unwrap_or(start_pos);
        self.expect_simple(TokenKind::RParen, "expected ')' after require path")?;
        Ok(Expr::Require(
            path,
            Some(Span {
                start: start_pos,
                end: end_pos,
            }),
        ))
    }

    fn parse_brace_literal(&mut self, start_pos: u32) -> Result<Expr, Diagnostic> {
        if self.check_simple(&TokenKind::RBrace) {
            let end_token = self.peek().cloned();
            let end_pos = end_token.map(|t| t.span.end).unwrap_or(start_pos);
            self.advance();
            return Ok(Expr::ArrayLiteral {
                elements: Vec::new(),
                span: Some(Span {
                    start: start_pos,
                    end: end_pos,
                }),
            });
        }

        if let Some(Token {
            kind: TokenKind::Identifier(_),
            ..
        }) = self.peek()
        {
            if matches!(
                self.peek_n(1).map(|token| &token.kind),
                Some(TokenKind::Equal)
            ) {
                return self.parse_table_literal_body(start_pos);
            }
        }

        let mut elements = Vec::new();
        loop {
            elements.push(self.parse_expr()?);
            if self.check_simple(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        let end_token = self.peek().cloned();
        let end_pos = end_token.map(|t| t.span.end).unwrap_or(start_pos);
        self.expect_simple(TokenKind::RBrace, "expected '}' after array literal")?;
        Ok(Expr::ArrayLiteral {
            elements,
            span: Some(Span {
                start: start_pos,
                end: end_pos,
            }),
        })
    }

    fn parse_table_literal_body(&mut self, start_pos: u32) -> Result<Expr, Diagnostic> {
        let mut fields = Vec::new();
        loop {
            if self.check_simple(&TokenKind::RBrace) {
                break;
            }
            let name = self.expect_identifier()?;
            self.expect_simple(TokenKind::Equal, "expected '=' after table field name")?;
            let value = self.parse_expr()?;
            fields.push(TableField { name, value });
            if self.check_simple(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        let end_token = self.peek().cloned();
        let end_pos = end_token.map(|t| t.span.end).unwrap_or(start_pos);
        self.expect_simple(TokenKind::RBrace, "expected '}' after table literal")?;
        Ok(Expr::TableLiteral {
            fields,
            span: Some(Span {
                start: start_pos,
                end: end_pos,
            }),
        })
    }
}
