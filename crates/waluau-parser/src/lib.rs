use waluau_ast::{
    BinaryOp, Expr, Function, NumberLiteral, NumericType, Param, Program, Stmt, Type, UnaryOp,
};
use waluau_diagnostics::Diagnostic;
use waluau_lexer::{Token, TokenKind};

pub fn parse(source: &str) -> Result<Program, Diagnostic> {
    let tokens = waluau_lexer::lex(source)?;
    Parser::new(tokens).parse_program()
}

struct Parser {
    tokens: Vec<Token>,
    index: usize,
    diagnostics: Vec<String>,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            index: 0,
            diagnostics: Vec::new(),
        }
    }

    fn parse_program(&mut self) -> Result<Program, Diagnostic> {
        let mut functions = Vec::new();
        while self.peek().is_some() {
            match self.parse_function() {
                Ok(function) => functions.push(function),
                Err(error) => {
                    self.record_error(error);
                    self.sync_to_next_function();
                }
            }
        }

        if self.diagnostics.is_empty() {
            Ok(Program { functions })
        } else {
            Err(Diagnostic::new(self.diagnostics.join("\n")))
        }
    }

    fn parse_function(&mut self) -> Result<Function, Diagnostic> {
        self.expect_simple(TokenKind::Function, "expected 'function'")?;
        let name = self.expect_identifier()?;
        self.expect_simple(TokenKind::LParen, "expected '('")?;
        let mut params = Vec::new();
        if !self.check_simple(&TokenKind::RParen) {
            loop {
                let param_name = self.expect_identifier()?;
                self.expect_simple(TokenKind::Colon, "expected ':' after parameter name")?;
                let param_type = self.parse_type()?;
                params.push(Param {
                    name: param_name,
                    ty: param_type,
                });
                if self.check_simple(&TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.expect_simple(TokenKind::RParen, "expected ')'")?;
        self.expect_simple(TokenKind::Colon, "expected ':' before return type")?;
        let return_type = self.parse_type()?;
        let body = self.parse_block_until(&[TokenKind::End]);
        self.expect_simple(TokenKind::End, "expected 'end' after function body")?;
        Ok(Function {
            name,
            params,
            return_type,
            body,
        })
    }

    fn parse_block_until(&mut self, end_markers: &[TokenKind]) -> Vec<Stmt> {
        let mut statements = Vec::new();
        while let Some(token) = self.peek() {
            if end_markers
                .iter()
                .any(|marker| same_variant(&token.kind, marker))
            {
                break;
            }

            let start_index = self.index;
            match self.parse_stmt() {
                Ok(statement) => statements.push(statement),
                Err(error) => {
                    self.record_error(error);
                    self.synchronize_statement(end_markers, start_index);
                }
            }
        }
        statements
    }

    fn parse_stmt(&mut self) -> Result<Stmt, Diagnostic> {
        if self.check_simple(&TokenKind::Local) {
            self.advance();
            let name = self.expect_identifier()?;
            self.expect_simple(TokenKind::Colon, "expected ':' after local name")?;
            let ty = self.parse_type()?;
            self.expect_simple(TokenKind::Equal, "expected '=' in local declaration")?;
            let value = self.parse_expr()?;
            return Ok(Stmt::Let { name, ty, value });
        }
        if self.check_simple(&TokenKind::If) {
            return self.parse_if_stmt();
        }
        if self.check_simple(&TokenKind::While) {
            self.advance();
            let condition = self.parse_expr()?;
            self.expect_simple(TokenKind::Do, "expected 'do' after while condition")?;
            let body = self.parse_block_until(&[TokenKind::End]);
            self.expect_simple(TokenKind::End, "expected 'end' after while")?;
            return Ok(Stmt::While { condition, body });
        }
        if self.check_simple(&TokenKind::Return) {
            self.advance();
            return Ok(Stmt::Return(self.parse_expr()?));
        }

        if let Some(Token {
            kind: TokenKind::Identifier(name),
            ..
        }) = self.peek().cloned()
        {
            if matches!(self.peek_n(1).map(|t| &t.kind), Some(TokenKind::Equal)) {
                self.advance();
                self.advance();
                let value = self.parse_expr()?;
                return Ok(Stmt::Assign { name, value });
            }
        }

        Ok(Stmt::Expr(self.parse_expr()?))
    }

    fn parse_expr(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_or()
    }

    fn parse_if_stmt(&mut self) -> Result<Stmt, Diagnostic> {
        self.expect_simple(TokenKind::If, "expected 'if'")?;
        let stmt = self.parse_if_clause()?;
        self.expect_simple(TokenKind::End, "expected 'end' after if")?;
        Ok(stmt)
    }

    fn parse_if_clause(&mut self) -> Result<Stmt, Diagnostic> {
        let condition = self.parse_expr()?;
        self.expect_simple(TokenKind::Then, "expected 'then' after if condition")?;
        let then_body =
            self.parse_block_until(&[TokenKind::ElseIf, TokenKind::Else, TokenKind::End]);
        let else_body = if self.check_simple(&TokenKind::ElseIf) {
            self.advance();
            vec![self.parse_if_clause()?]
        } else if self.check_simple(&TokenKind::Else) {
            self.advance();
            self.parse_block_until(&[TokenKind::End])
        } else {
            Vec::new()
        };
        Ok(Stmt::If {
            condition,
            then_body,
            else_body,
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
        self.parse_binary(
            Parser::parse_add,
            &[TokenKind::EqualEqual, TokenKind::Less, TokenKind::Greater],
            &[BinaryOp::Eq, BinaryOp::Less, BinaryOp::Greater],
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
            &[TokenKind::Star, TokenKind::Slash],
            &[BinaryOp::Mul, BinaryOp::Div],
        )
    }

    fn parse_cast(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_unary()?;
        while self.check_simple(&TokenKind::ColonColon) {
            self.advance();
            let ty = self.parse_type()?;
            expr = Expr::Cast {
                expr: Box::new(expr),
                ty,
            };
        }
        Ok(expr)
    }

    fn parse_unary(&mut self) -> Result<Expr, Diagnostic> {
        if self.check_simple(&TokenKind::Minus) {
            self.advance();
            return Ok(Expr::Unary {
                op: UnaryOp::Neg,
                expr: Box::new(self.parse_unary()?),
            });
        }
        if self.check_simple(&TokenKind::Not) {
            self.advance();
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(self.parse_unary()?),
            });
        }
        self.parse_primary()
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
                if same_variant(&next.kind, op) {
                    matched = Some(mapped[i]);
                    break;
                }
            }
            if let Some(op) = matched {
                self.advance();
                let right = sub(self)?;
                expr = Expr::Binary {
                    op,
                    left: Box::new(expr),
                    right: Box::new(right),
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
        match token.kind {
            TokenKind::Number(value) => Ok(Expr::Number(NumberLiteral { raw: value })),
            TokenKind::True => Ok(Expr::Bool(true)),
            TokenKind::False => Ok(Expr::Bool(false)),
            TokenKind::Identifier(name) => {
                if self.check_simple(&TokenKind::LParen) {
                    self.advance();
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
                    self.expect_simple(TokenKind::RParen, "expected ')' after call arguments")?;
                    Ok(Expr::Call { name, args })
                } else {
                    Ok(Expr::Name(name))
                }
            }
            TokenKind::LParen => {
                let inner = self.parse_expr()?;
                self.expect_simple(TokenKind::RParen, "expected ')' after expression")?;
                Ok(inner)
            }
            _ => Err(self.diagnostic_at_current("expected expression")),
        }
    }

    fn parse_type(&mut self) -> Result<Type, Diagnostic> {
        match self.advance().map(|token| token.kind) {
            Some(TokenKind::NumberType | TokenKind::F64Type) => Ok(Type::number()),
            Some(TokenKind::U32Type) => Ok(Type::Numeric(NumericType::U32)),
            Some(TokenKind::U64Type) => Ok(Type::Numeric(NumericType::U64)),
            Some(TokenKind::I32Type) => Ok(Type::Numeric(NumericType::I32)),
            Some(TokenKind::I64Type) => Ok(Type::Numeric(NumericType::I64)),
            Some(TokenKind::F32Type) => Ok(Type::Numeric(NumericType::F32)),
            Some(TokenKind::BoolType) => Ok(Type::Bool),
            _ => Err(self.diagnostic_at_current(
                "expected type (number, u32, u64, i32, i64, f32, f64, or bool)",
            )),
        }
    }

    fn expect_identifier(&mut self) -> Result<String, Diagnostic> {
        match self.advance().map(|token| token.kind) {
            Some(TokenKind::Identifier(name)) => Ok(name),
            _ => Err(self.diagnostic_at_current("expected identifier")),
        }
    }

    fn expect_simple(&mut self, expected: TokenKind, message: &str) -> Result<(), Diagnostic> {
        let token = self
            .advance()
            .ok_or_else(|| Diagnostic::new("unexpected end of input"))?;
        if same_variant(&token.kind, &expected) {
            Ok(())
        } else {
            Err(Diagnostic::new(format!(
                "{message} at {}..{}",
                token.span.start, token.span.end
            )))
        }
    }

    fn check_simple(&self, expected: &TokenKind) -> bool {
        self.peek()
            .map(|token| same_variant(&token.kind, expected))
            .unwrap_or(false)
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.index)
    }

    fn peek_n(&self, n: usize) -> Option<&Token> {
        self.tokens.get(self.index + n)
    }

    fn advance(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.index).cloned();
        self.index += usize::from(token.is_some());
        token
    }

    fn record_error(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic.to_string());
    }

    fn diagnostic_at_current(&self, message: &str) -> Diagnostic {
        if self.index == 0 {
            return Diagnostic::new(message);
        }

        if let Some(token) = self.tokens.get(self.index.saturating_sub(1)) {
            Diagnostic::new(format!(
                "{message} at {}..{}",
                token.span.start, token.span.end
            ))
        } else {
            Diagnostic::new(message)
        }
    }

    fn sync_to_next_function(&mut self) {
        while let Some(token) = self.peek() {
            if matches!(token.kind, TokenKind::Function) {
                return;
            }
            self.advance();
        }
    }

    fn synchronize_statement(&mut self, end_markers: &[TokenKind], start_index: usize) {
        let mut depth = 0usize;
        while let Some(token) = self.peek() {
            if depth == 0
                && (is_statement_start(&token.kind)
                    || end_markers
                        .iter()
                        .any(|marker| same_variant(&token.kind, marker)))
            {
                return;
            }

            match token.kind {
                TokenKind::If | TokenKind::While | TokenKind::Function => depth += 1,
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
            | TokenKind::If
            | TokenKind::While
            | TokenKind::Return
            | TokenKind::Identifier(_)
    )
}

fn same_variant(a: &TokenKind, b: &TokenKind) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

#[cfg(test)]
mod tests {
    use super::parse;
    use waluau_ast::{NumberLiteral, NumericType, Type, UnaryOp};

    #[test]
    fn parses_v0_function() {
        let source = r#"
            function choose(flag: bool, x: i32, y: number): f64
                local result: f64 = y
                if flag then
                    result = x + 1
                else
                    result = x + y
                end
                return result
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        assert_eq!(program.functions.len(), 1);
    }

    #[test]
    fn parses_numeric_type_aliases() {
        let source = r#"
            function widen(x: number, y: f32, z: u64, w: i64): f64
                local result: f64 = x
                return result
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let function = &program.functions[0];
        assert_eq!(function.params[0].ty, Type::Numeric(NumericType::F64));
        assert_eq!(function.params[1].ty, Type::Numeric(NumericType::F32));
        assert_eq!(function.params[2].ty, Type::Numeric(NumericType::U64));
        assert_eq!(function.params[3].ty, Type::Numeric(NumericType::I64));
        assert_eq!(function.return_type, Type::Numeric(NumericType::F64));
    }

    #[test]
    fn parses_postfix_numeric_casts() {
        let source = r#"
            function cast(x: i64): i32
                return (x + 1) :: i32
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let function = &program.functions[0];
        assert!(matches!(
            &function.body[0],
            waluau_ast::Stmt::Return(waluau_ast::Expr::Cast {
                ty: Type::Numeric(NumericType::I32),
                ..
            })
        ));
    }

    #[test]
    fn preserves_large_integer_literal_text() {
        let source = r#"
            function entry(): u64
                return 18446744073709551615
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let function = &program.functions[0];
        assert!(matches!(
            &function.body[0],
            waluau_ast::Stmt::Return(waluau_ast::Expr::Number(NumberLiteral { raw }))
                if raw == "18446744073709551615"
        ));
    }

    #[test]
    fn parses_unary_and_elseif_forms() {
        let source = r#"
            function entry(flag: bool, x: i32): i32
                if not flag then
                    return -x
                elseif x > 0 then
                    return x
                else
                    return 0
                end
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let function = &program.functions[0];
        assert!(matches!(
            &function.body[0],
            waluau_ast::Stmt::If {
                condition: waluau_ast::Expr::Unary {
                    op: UnaryOp::Not,
                    ..
                },
                else_body,
                ..
            } if matches!(
                else_body.as_slice(),
                [waluau_ast::Stmt::If {
                    then_body,
                    ..
                }] if matches!(
                    then_body.as_slice(),
                    [waluau_ast::Stmt::Return(waluau_ast::Expr::Name(name))] if name == "x"
                )
            )
        ));
    }

    #[test]
    fn rejects_legacy_function_local_and_return_syntax() {
        let source = r#"
            fn entry(x: i32) -> i32
                let y: i32 = x
                return y
            end
        "#;

        let error = parse(source).expect_err("parse should fail");
        let message = error.to_string();
        assert!(
            message.contains("unsupported 'fn'")
                || message.contains("unsupported 'let'")
                || message.contains("unsupported '->'")
        );
    }

    #[test]
    fn rejects_symbolic_logical_operators() {
        let source = r#"
            function entry(a: bool, b: bool): bool
                return a && b || a
            end
        "#;

        let error = parse(source).expect_err("parse should fail");
        let message = error.to_string();
        assert!(message.contains("unsupported '&&'") || message.contains("unsupported '||'"));
    }
}
