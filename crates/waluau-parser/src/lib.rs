use waluau_ast::{BinaryOp, Expr, Function, Param, Program, Stmt, Type};
use waluau_diagnostics::Diagnostic;
use waluau_lexer::{Token, TokenKind};

pub fn parse(source: &str) -> Result<Program, Diagnostic> {
    let tokens = waluau_lexer::lex(source)?;
    Parser::new(tokens).parse_program()
}

struct Parser {
    tokens: Vec<Token>,
    index: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, index: 0 }
    }

    fn parse_program(&mut self) -> Result<Program, Diagnostic> {
        let mut functions = Vec::new();
        while self.peek().is_some() {
            functions.push(self.parse_function()?);
        }
        Ok(Program { functions })
    }

    fn parse_function(&mut self) -> Result<Function, Diagnostic> {
        self.expect_simple(TokenKind::Fn, "expected 'fn'")?;
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
        self.expect_simple(TokenKind::Arrow, "expected '->'")?;
        let return_type = self.parse_type()?;
        let body = self.parse_block_until(&[TokenKind::End])?;
        self.expect_simple(TokenKind::End, "expected 'end' after function body")?;
        Ok(Function {
            name,
            params,
            return_type,
            body,
        })
    }

    fn parse_block_until(&mut self, end_markers: &[TokenKind]) -> Result<Vec<Stmt>, Diagnostic> {
        let mut statements = Vec::new();
        while let Some(token) = self.peek() {
            if end_markers
                .iter()
                .any(|marker| same_variant(&token.kind, marker))
            {
                break;
            }
            statements.push(self.parse_stmt()?);
        }
        Ok(statements)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, Diagnostic> {
        if self.check_simple(&TokenKind::Let) {
            self.advance();
            let name = self.expect_identifier()?;
            self.expect_simple(TokenKind::Colon, "expected ':' after local name")?;
            let ty = self.parse_type()?;
            self.expect_simple(TokenKind::Equal, "expected '=' in local declaration")?;
            let value = self.parse_expr()?;
            return Ok(Stmt::Let { name, ty, value });
        }
        if self.check_simple(&TokenKind::If) {
            self.advance();
            let condition = self.parse_expr()?;
            self.expect_simple(TokenKind::Then, "expected 'then' after if condition")?;
            let then_body = self.parse_block_until(&[TokenKind::Else, TokenKind::End])?;
            let else_body = if self.check_simple(&TokenKind::Else) {
                self.advance();
                self.parse_block_until(&[TokenKind::End])?
            } else {
                Vec::new()
            };
            self.expect_simple(TokenKind::End, "expected 'end' after if")?;
            return Ok(Stmt::If {
                condition,
                then_body,
                else_body,
            });
        }
        if self.check_simple(&TokenKind::While) {
            self.advance();
            let condition = self.parse_expr()?;
            self.expect_simple(TokenKind::Do, "expected 'do' after while condition")?;
            let body = self.parse_block_until(&[TokenKind::End])?;
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

    fn parse_or(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_binary(Parser::parse_and, &[TokenKind::OrOr], &[BinaryOp::Or])
    }

    fn parse_and(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_binary(
            Parser::parse_comparison,
            &[TokenKind::AndAnd],
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
            Parser::parse_primary,
            &[TokenKind::Star, TokenKind::Slash],
            &[BinaryOp::Mul, BinaryOp::Div],
        )
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
            TokenKind::Number(value) => Ok(Expr::Number(value)),
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
            _ => Err(Diagnostic::new("expected expression")),
        }
    }

    fn parse_type(&mut self) -> Result<Type, Diagnostic> {
        match self.advance().map(|token| token.kind) {
            Some(TokenKind::NumberType) => Ok(Type::Number),
            Some(TokenKind::BoolType) => Ok(Type::Bool),
            _ => Err(Diagnostic::new("expected type (number or bool)")),
        }
    }

    fn expect_identifier(&mut self) -> Result<String, Diagnostic> {
        match self.advance().map(|token| token.kind) {
            Some(TokenKind::Identifier(name)) => Ok(name),
            _ => Err(Diagnostic::new("expected identifier")),
        }
    }

    fn expect_simple(&mut self, expected: TokenKind, message: &str) -> Result<(), Diagnostic> {
        let token = self
            .advance()
            .ok_or_else(|| Diagnostic::new("unexpected end of input"))?;
        if same_variant(&token.kind, &expected) {
            Ok(())
        } else {
            Err(Diagnostic::new(message))
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
}

fn same_variant(a: &TokenKind, b: &TokenKind) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_v0_function() {
        let source = r#"
            fn choose(flag: bool, x: number, y: number) -> number
                let result: number = x
                if flag then
                    result = y
                else
                    result = x + y
                end
                return result
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        assert_eq!(program.functions.len(), 1);
    }
}
