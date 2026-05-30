use waluau_ast::{
    AssignOp, BinaryOp, Binding, Expr, Function, FunctionExpr, NumberLiteral, NumericType, Param,
    Program, Rebindability, Stmt, Type, UnaryOp,
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
        let mut top_level = Vec::new();
        let mut export = None;
        while self.peek().is_some() {
            if self.check_simple(&TokenKind::Function) {
                match self.parse_function() {
                    Ok(function) => functions.push(function),
                    Err(error) => {
                        self.record_error(error);
                        self.sync_to_next_function();
                    }
                }
            } else {
                match self.parse_stmt() {
                    // A trailing top-level `return <expr>` declares the value a
                    // module exports through `require`. It must be the final
                    // item in the file.
                    Ok(Stmt::Return(value)) => {
                        if export.is_some() {
                            self.record_error(Diagnostic::new(
                                "a module may only have one top-level return",
                            ));
                        } else {
                            export = Some(value);
                        }
                        if self.peek().is_some() {
                            self.record_error(Diagnostic::new(
                                "top-level return must be the final statement in a module",
                            ));
                        }
                    }
                    Ok(Stmt::ReturnMulti(_)) => self.record_error(Diagnostic::new(
                        "a module return must export a single value",
                    )),
                    Ok(stmt) => top_level.push(stmt),
                    Err(error) => {
                        self.record_error(error);
                        self.synchronize_statement(&[], self.index);
                    }
                }
            }
        }

        if self.diagnostics.is_empty() {
            Ok(Program {
                functions,
                top_level,
                export,
            })
        } else {
            Err(Diagnostic::new(self.diagnostics.join("\n")))
        }
    }

    fn parse_function(&mut self) -> Result<Function, Diagnostic> {
        self.expect_simple(TokenKind::Function, "expected 'function'")?;
        let name = self.expect_identifier()?;
        let function_expr = self.parse_function_expr_tail(Some(name), false)?;
        Ok(Function {
            name: function_expr
                .name
                .expect("top-level functions always have a name"),
            params: function_expr.params,
            return_type: function_expr.return_type,
            body: function_expr.body,
        })
    }

    fn parse_function_expr_tail(
        &mut self,
        name: Option<String>,
        require_return_type: bool,
    ) -> Result<FunctionExpr, Diagnostic> {
        self.expect_simple(TokenKind::LParen, "expected '('")?;
        let mut params = Vec::new();
        if !self.check_simple(&TokenKind::RParen) {
            loop {
                let param_name = self.expect_identifier()?;
                self.expect_simple(TokenKind::Colon, "expected ':' after parameter name")?;
                let param_type = match self.parse_type() {
                    Ok(ty) => ty,
                    Err(error) => {
                        self.record_error(error);
                        Type::number()
                    }
                };
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
        let return_type = if self.check_simple(&TokenKind::Colon) {
            self.advance();
            Some(match self.parse_return_type_list() {
                Ok(ty) => ty,
                Err(error) => {
                    self.record_error(error);
                    Type::number()
                }
            })
        } else if require_return_type {
            return Err(Diagnostic::new("expected ':' before return type"));
        } else {
            None
        };
        let body = self.parse_block_until(&[TokenKind::End]);
        self.expect_simple(TokenKind::End, "expected 'end' after function body")?;
        Ok(FunctionExpr {
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
            return self.parse_local_decl();
        }
        if self.is_const_decl_start() {
            return self.parse_const_decl();
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
        if self.check_simple(&TokenKind::Repeat) {
            self.advance();
            let body = self.parse_block_until(&[TokenKind::Until]);
            self.expect_simple(TokenKind::Until, "expected 'until' after repeat body")?;
            let condition = self.parse_expr()?;
            return Ok(Stmt::Repeat { body, condition });
        }
        if self.check_simple(&TokenKind::Break) {
            self.advance();
            return Ok(Stmt::Break);
        }
        if self.check_simple(&TokenKind::Continue) {
            self.advance();
            return Ok(Stmt::Continue);
        }
        if self.check_simple(&TokenKind::Return) {
            self.advance();
            let values = self.parse_expr_list()?;
            return Ok(if values.len() == 1 {
                Stmt::Return(values.into_iter().next().expect("len checked"))
            } else {
                Stmt::ReturnMulti(values)
            });
        }

        if let Some(assignment) = self.try_parse_assignment()? {
            return Ok(assignment);
        }

        Ok(Stmt::Expr(self.parse_expr()?))
    }

    fn parse_local_decl(&mut self) -> Result<Stmt, Diagnostic> {
        self.expect_simple(TokenKind::Local, "expected 'local'")?;
        let name = self.expect_identifier()?;
        let rebindability = if self.check_simple(&TokenKind::Less) {
            self.advance();
            let attr_name = self.expect_identifier()?;
            self.expect_simple(TokenKind::Greater, "expected '>' after local attribute")?;
            if attr_name != "const" {
                return Err(Diagnostic::new(format!(
                    "unsupported local attribute '<{}>'",
                    attr_name
                )));
            }
            Rebindability::Const
        } else {
            Rebindability::Rebindable
        };
        let has_explicit_type = self.check_simple(&TokenKind::Colon);
        let ty = if has_explicit_type {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };
        if self.check_simple(&TokenKind::Comma) {
            let mut bindings = vec![Binding {
                name,
                rebindability,
                ty,
            }];
            self.advance();
            bindings.extend(self.parse_binding_list()?);
            self.expect_simple(TokenKind::Equal, "expected '=' in local declaration")?;
            let values = self.parse_expr_list()?;
            return Ok(Stmt::LetMulti { bindings, values });
        }
        self.expect_simple(TokenKind::Equal, "expected '=' in local declaration")?;
        let value = self.parse_expr()?;
        Ok(Stmt::Let {
            name,
            rebindability,
            ty,
            value,
        })
    }

    fn is_const_decl_start(&self) -> bool {
        matches!(
            (
                self.peek().map(|token| &token.kind),
                self.peek_n(1).map(|token| &token.kind),
                self.peek_n(2).map(|token| &token.kind),
            ),
            (
                Some(TokenKind::Identifier(keyword)),
                Some(TokenKind::Identifier(_)),
                Some(TokenKind::Colon)
            ) if keyword == "const"
        )
    }

    fn parse_const_decl(&mut self) -> Result<Stmt, Diagnostic> {
        let keyword = self.expect_identifier()?;
        if keyword != "const" {
            return Err(Diagnostic::new("expected 'const'"));
        }
        let name = self.expect_identifier()?;
        self.expect_simple(TokenKind::Colon, "expected ':' after const name")?;
        let ty = self.parse_type()?;
        self.expect_simple(TokenKind::Equal, "expected '=' in const declaration")?;
        let values = self.parse_expr_list()?;
        Ok(if values.len() == 1 {
            Stmt::Let {
                name,
                rebindability: Rebindability::Const,
                ty: Some(ty),
                value: values.into_iter().next().expect("len checked"),
            }
        } else {
            Stmt::LetMulti {
                bindings: vec![Binding {
                    name,
                    rebindability: Rebindability::Const,
                    ty: Some(ty),
                }],
                values,
            }
        })
    }

    fn try_parse_assignment(&mut self) -> Result<Option<Stmt>, Diagnostic> {
        let checkpoint = self.index;
        let target = match self.parse_postfix_expr() {
            Ok(expr) => expr,
            Err(error) => {
                self.index = checkpoint;
                return Err(error);
            }
        };
        let mut targets = vec![target];
        while self.check_simple(&TokenKind::Comma) {
            self.advance();
            targets.push(self.parse_postfix_expr()?);
        }
        let op = if self.check_simple(&TokenKind::Equal) {
            AssignOp::Set
        } else if self.check_simple(&TokenKind::PlusEqual) {
            AssignOp::Add
        } else {
            self.index = checkpoint;
            return Ok(None);
        };
        self.advance();
        let values = self.parse_expr_list()?;
        if targets.len() > 1 && op != AssignOp::Set {
            return Err(Diagnostic::new(
                "compound assignment does not support multiple targets",
            ));
        }
        Ok(Some(if targets.len() == 1 && values.len() == 1 {
            match targets.into_iter().next().expect("len checked") {
                Expr::Name(name) => Stmt::Assign {
                    op,
                    name,
                    value: values.into_iter().next().expect("len checked"),
                },
                Expr::Index { base, index } => Stmt::IndexAssign {
                    op,
                    base,
                    index,
                    value: values.into_iter().next().expect("len checked"),
                },
                _ => {
                    return Err(Diagnostic::new("invalid assignment target"));
                }
            }
        } else {
            let mut names = Vec::new();
            for target in targets {
                match target {
                    Expr::Name(name) => names.push(name),
                    _ => return Err(Diagnostic::new("multi-assignment targets must be names")),
                }
            }
            Stmt::AssignMulti {
                targets: names,
                values,
            }
        }))
    }

    fn parse_binding_list(&mut self) -> Result<Vec<Binding>, Diagnostic> {
        let mut bindings = Vec::new();
        loop {
            let name = self.expect_identifier()?;
            let rebindability = if self.check_simple(&TokenKind::Less) {
                self.advance();
                let attr_name = self.expect_identifier()?;
                self.expect_simple(TokenKind::Greater, "expected '>' after local attribute")?;
                if attr_name != "const" {
                    return Err(Diagnostic::new(format!(
                        "unsupported local attribute '<{}>'",
                        attr_name
                    )));
                }
                Rebindability::Const
            } else {
                Rebindability::Rebindable
            };
            let ty = if self.check_simple(&TokenKind::Colon) {
                self.advance();
                Some(self.parse_type()?)
            } else {
                None
            };
            bindings.push(Binding {
                name,
                rebindability,
                ty,
            });
            if !self.check_simple(&TokenKind::Comma) {
                break;
            }
            self.advance();
        }
        Ok(bindings)
    }

    fn parse_expr_list(&mut self) -> Result<Vec<Expr>, Diagnostic> {
        let mut values = vec![self.parse_expr()?];
        while self.check_simple(&TokenKind::Comma) {
            self.advance();
            values.push(self.parse_expr()?);
        }
        Ok(values)
    }

    fn parse_return_type_list(&mut self) -> Result<Type, Diagnostic> {
        let first = self.parse_type()?;
        if !self.check_simple(&TokenKind::Comma) {
            return Ok(first);
        }
        let mut types = vec![first];
        while self.check_simple(&TokenKind::Comma) {
            self.advance();
            types.push(self.parse_type()?);
        }
        Ok(Type::Multi(types))
    }

    fn parse_expr(&mut self) -> Result<Expr, Diagnostic> {
        if self.check_simple(&TokenKind::If) {
            return self.parse_if_expr();
        }
        self.parse_or()
    }

    fn parse_if_expr(&mut self) -> Result<Expr, Diagnostic> {
        self.expect_simple(TokenKind::If, "expected 'if'")?;
        let condition = self.parse_expr()?;
        self.expect_simple(TokenKind::Then, "expected 'then' after if condition")?;
        let then_expr = self.parse_expr()?;
        self.expect_simple(TokenKind::Else, "expected 'else' in if expression")?;
        let else_expr = self.parse_expr()?;
        Ok(Expr::If {
            condition: Box::new(condition),
            then_expr: Box::new(then_expr),
            else_expr: Box::new(else_expr),
        })
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
        if self.check_simple(&TokenKind::Hash) {
            self.advance();
            return Ok(Expr::Unary {
                op: UnaryOp::Len,
                expr: Box::new(self.parse_unary()?),
            });
        }
        self.parse_postfix_expr()
    }

    fn parse_postfix_expr(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.check_simple(&TokenKind::LBracket) {
                self.advance();
                let index = self.parse_expr()?;
                self.expect_simple(TokenKind::RBracket, "expected ']' after array index")?;
                expr = Expr::Index {
                    base: Box::new(expr),
                    index: Box::new(index),
                };
                continue;
            }
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
                expr = Expr::Call {
                    callee: Box::new(expr),
                    args,
                };
                continue;
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
                // `require("...")` is parsed as a dedicated node so the
                // linker can resolve module ids before IR lowering.
                if name == "require"
                    && self
                        .peek()
                        .is_some_and(|token| same_variant(&token.kind, &TokenKind::LParen))
                {
                    return self.parse_require();
                }
                Ok(Expr::Name(name))
            }
            TokenKind::Str(value) => Ok(Expr::String(value)),
            TokenKind::Function => {
                let name = if let Some(Token {
                    kind: TokenKind::Identifier(_),
                    ..
                }) = self.peek()
                {
                    if self
                        .peek_n(1)
                        .map(|token| same_variant(&token.kind, &TokenKind::LParen))
                        .unwrap_or(false)
                    {
                        Some(self.expect_identifier()?)
                    } else {
                        None
                    }
                } else {
                    None
                };
                Ok(Expr::Function(self.parse_function_expr_tail(name, false)?))
            }
            TokenKind::LBrace => self.parse_array_literal(),
            TokenKind::LParen => {
                let inner = self.parse_expr()?;
                self.expect_simple(TokenKind::RParen, "expected ')' after expression")?;
                Ok(inner)
            }
            _ => Err(self.diagnostic_at_current("expected expression")),
        }
    }

    fn parse_require(&mut self) -> Result<Expr, Diagnostic> {
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
        self.expect_simple(TokenKind::RParen, "expected ')' after require path")?;
        Ok(Expr::Require(path))
    }

    fn parse_array_literal(&mut self) -> Result<Expr, Diagnostic> {
        let mut elements = Vec::new();
        if !self.check_simple(&TokenKind::RBrace) {
            loop {
                if let Some(Token {
                    kind: TokenKind::Identifier(_),
                    ..
                }) = self.peek()
                {
                    if matches!(
                        self.peek_n(1).map(|token| &token.kind),
                        Some(TokenKind::Equal)
                    ) {
                        return Err(Diagnostic::new(
                            "table literals with named fields are not supported",
                        ));
                    }
                }
                elements.push(self.parse_expr()?);
                if self.check_simple(&TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.expect_simple(TokenKind::RBrace, "expected '}' after array literal")?;
        Ok(Expr::ArrayLiteral { elements })
    }

    fn parse_type(&mut self) -> Result<Type, Diagnostic> {
        if self.check_simple(&TokenKind::LBrace) {
            self.advance();
            let element = self.parse_type()?;
            self.expect_simple(TokenKind::RBrace, "expected '}' after array element type")?;
            return Ok(Type::Array(Box::new(element)));
        }
        if self.check_simple(&TokenKind::LParen) {
            self.advance();
            let mut params = Vec::new();
            if !self.check_simple(&TokenKind::RParen) {
                loop {
                    params.push(self.parse_type()?);
                    if self.check_simple(&TokenKind::Comma) {
                        self.advance();
                    } else {
                        break;
                    }
                }
            }
            self.expect_simple(TokenKind::RParen, "expected ')' after function type params")?;
            self.expect_simple(TokenKind::Arrow, "expected '->' in function type")?;
            let return_type = self.parse_type()?;
            return Ok(Type::Function {
                params,
                return_type: Box::new(return_type),
            });
        }

        match self.advance().map(|token| token.kind) {
            Some(TokenKind::NumberType | TokenKind::F64Type) => Ok(Type::number()),
            Some(TokenKind::U32Type) => Ok(Type::Numeric(NumericType::U32)),
            Some(TokenKind::U64Type) => Ok(Type::Numeric(NumericType::U64)),
            Some(TokenKind::I32Type) => Ok(Type::Numeric(NumericType::I32)),
            Some(TokenKind::I64Type) => Ok(Type::Numeric(NumericType::I64)),
            Some(TokenKind::F32Type) => Ok(Type::Numeric(NumericType::F32)),
            Some(TokenKind::BoolType) => Ok(Type::Bool),
            Some(TokenKind::StringType) => Ok(Type::String),
            _ => Err(self.diagnostic_at_current(
                "expected type (number, u32, u64, i32, i64, f32, f64, bool, string, {T}, or (T1, T2) -> R)",
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
                if self.index == start_index {
                    self.advance();
                    continue;
                }
                return;
            }

            match token.kind {
                TokenKind::If | TokenKind::While | TokenKind::Repeat | TokenKind::Function => {
                    depth += 1
                }
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
            | TokenKind::Repeat
            | TokenKind::Return
            | TokenKind::Break
            | TokenKind::Continue
            | TokenKind::Identifier(_)
    )
}

fn same_variant(a: &TokenKind, b: &TokenKind) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

#[cfg(test)]
mod tests {
    use super::parse;
    use waluau_ast::{
        AssignOp, BinaryOp, NumberLiteral, NumericType, Rebindability, Stmt, Type, UnaryOp,
    };

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
        assert_eq!(function.return_type, Some(Type::Numeric(NumericType::F64)));
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
    fn parses_floor_division_with_multiplicative_precedence() {
        let source = r#"
            function entry(x: number, y: number, z: number): number
                return -x // y * z % 2 / 3
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let function = &program.functions[0];
        let waluau_ast::Stmt::Return(waluau_ast::Expr::Binary {
            op: BinaryOp::Div,
            left: div_left,
            ..
        }) = &function.body[0]
        else {
            panic!("return should end with division");
        };
        let waluau_ast::Expr::Binary {
            op: BinaryOp::Mod,
            left: mod_left,
            ..
        } = div_left.as_ref()
        else {
            panic!("division left side should be modulo");
        };
        let waluau_ast::Expr::Binary {
            op: BinaryOp::Mul,
            left: mul_left,
            ..
        } = mod_left.as_ref()
        else {
            panic!("modulo left side should be multiplication");
        };
        let waluau_ast::Expr::Binary {
            op: BinaryOp::FloorDiv,
            left: floor_div_left,
            ..
        } = mul_left.as_ref()
        else {
            panic!("multiplication left side should be floor division");
        };
        assert!(matches!(
            floor_div_left.as_ref(),
            waluau_ast::Expr::Unary {
                op: UnaryOp::Neg,
                ..
            }
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
        assert!(message.contains("unsupported 'fn'") || message.contains("unsupported 'let'"));
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

    #[test]
    fn reports_multiple_invalid_type_annotations_in_one_function() {
        let source = r#"
            function add(x: f3, y: f4): f1
                local z: f2 = x + y
                return z
            end
        "#;

        let error = parse(source).expect_err("parse should fail");
        let message = error.to_string();
        assert_eq!(message.matches("expected type").count(), 4);
    }

    #[test]
    fn parses_array_types_literals_indexing_and_length() {
        let source = r#"
            function score_count(): i32
                local scores: {number} = {100, 250, 300}
                scores[1] = 250
                return #scores
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let function = &program.functions[0];
        assert_eq!(function.return_type, Some(Type::Numeric(NumericType::I32)));
        assert!(matches!(
            &function.body[0],
            waluau_ast::Stmt::Let {
                ty: Some(Type::Array(element)),
                value: waluau_ast::Expr::ArrayLiteral { elements },
                ..
            } if elements.len() == 3
                && **element == Type::number()
        ));
        assert!(matches!(
            &function.body[1],
            waluau_ast::Stmt::IndexAssign {
                op: AssignOp::Set,
                index,
                ..
            } if matches!(index.as_ref(), waluau_ast::Expr::Number(NumberLiteral { raw }) if raw == "1")
        ));
        assert!(matches!(
            &function.body[2],
            waluau_ast::Stmt::Return(waluau_ast::Expr::Unary {
                op: UnaryOp::Len,
                ..
            })
        ));
    }

    #[test]
    fn rejects_named_table_literals() {
        let source = r#"
            function entry(): i32
                local t: {i32} = {key = 1}
                return 0
            end
        "#;

        let error = parse(source).expect_err("parse should fail");
        assert!(
            error
                .to_string()
                .contains("table literals with named fields are not supported")
        );
    }

    #[test]
    fn parses_repeat_until_loop() {
        let source = r#"
            function entry(limit: i32): i32
                local i: i32 = 0
                repeat
                    i = i + 1
                until i > limit
                return i
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let function = &program.functions[0];
        assert!(matches!(
            &function.body[1],
            waluau_ast::Stmt::Repeat { body, condition } if body.len() == 1
                && matches!(condition, waluau_ast::Expr::Binary { .. })
        ));
    }

    #[test]
    fn parses_const_declarations_in_both_forms() {
        let source = r#"
            function entry(v: i32): i32
                local a <const>: i32 = v
                const b: i32 = a
                return b
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let function = &program.functions[0];
        assert!(matches!(
            &function.body[0],
            waluau_ast::Stmt::Let {
                name,
                rebindability: Rebindability::Const,
                ..
            } if name == "a"
        ));
        assert!(matches!(
            &function.body[1],
            waluau_ast::Stmt::Let {
                name,
                rebindability: Rebindability::Const,
                ..
            } if name == "b"
        ));
    }

    #[test]
    fn parses_function_type_and_literal_assignment() {
        let source = r#"
            function entry(): i32
                local add1: (i32) -> i32 = function(x: i32): i32
                    return x + 1
                end
                return add1(41)
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let function = &program.functions[0];
        assert!(matches!(
            &function.body[0],
            waluau_ast::Stmt::Let {
                ty: Some(Type::Function { params, return_type }),
                value: waluau_ast::Expr::Function(_),
                ..
            } if params == &vec![Type::Numeric(NumericType::I32)]
                && **return_type == Type::Numeric(NumericType::I32)
        ));
        assert!(matches!(
            &function.body[1],
            waluau_ast::Stmt::Return(waluau_ast::Expr::Call { .. })
        ));
    }

    #[test]
    fn parses_function_literal_without_return_annotation() {
        let source = r#"
            function entry(): i32
                local add1 = function(x: i32)
                    return x + 1
                end
                return add1(41)
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let function = &program.functions[0];
        assert!(matches!(
            &function.body[0],
            waluau_ast::Stmt::Let {
                ty: None,
                value: waluau_ast::Expr::Function(waluau_ast::FunctionExpr {
                    return_type: None,
                    ..
                }),
                ..
            }
        ));
    }

    #[test]
    fn const_is_contextual_not_reserved() {
        let source = r#"
            function entry(): i32
                local const: i32 = 20
                return const
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let function = &program.functions[0];
        assert!(matches!(
            &function.body[0],
            waluau_ast::Stmt::Let {
                name,
                rebindability: Rebindability::Rebindable,
                ..
            } if name == "const"
        ));
    }

    #[test]
    fn parses_local_without_annotation() {
        let source = r#"
            function entry(): i32
                local x = 20
                return x
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let function = &program.functions[0];
        assert!(matches!(
            &function.body[0],
            waluau_ast::Stmt::Let { name, ty: None, .. } if name == "x"
        ));
    }

    #[test]
    fn parses_top_level_function_without_return_annotation() {
        let source = r#"
            function entry(x: i32)
                return x + 1
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let function = &program.functions[0];
        assert_eq!(function.return_type, None);
    }

    #[test]
    fn parses_compound_assignments() {
        let source = r#"
            function entry(xs: {i32}, i: i32, x: i32): i32
                x += 1
                xs[i] += x
                return x
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let function = &program.functions[0];
        assert!(matches!(
            &function.body[0],
            waluau_ast::Stmt::Assign {
                op: AssignOp::Add,
                ..
            }
        ));
        assert!(matches!(
            &function.body[1],
            waluau_ast::Stmt::IndexAssign {
                op: AssignOp::Add,
                ..
            }
        ));
    }

    #[test]
    fn parses_if_expression_in_return() {
        let source = r#"
            function entry(flag: bool, x: i32, y: i32): i32
                return if flag then x else y
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let function = &program.functions[0];
        assert!(matches!(
            &function.body[0],
            waluau_ast::Stmt::Return(waluau_ast::Expr::If { .. })
        ));
    }

    #[test]
    fn parses_multi_return_signature_and_statement() {
        let source = r#"
            function pair(x: i32, y: bool): i32, bool
                return x, y
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let function = &program.functions[0];
        assert!(matches!(
            function.return_type,
            Some(Type::Multi(ref tys)) if tys == &vec![Type::Numeric(NumericType::I32), Type::Bool]
        ));
        assert!(matches!(
            &function.body[0],
            waluau_ast::Stmt::ReturnMulti(values) if values.len() == 2
        ));
    }

    #[test]
    fn parses_multi_local_and_multi_assignment() {
        let source = r#"
            function entry(x: i32, y: i32): i32
                local a: i32, b: i32 = x, y
                a, b = b, a
                return a
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let function = &program.functions[0];
        assert!(matches!(
            &function.body[0],
            waluau_ast::Stmt::LetMulti { bindings, values } if bindings.len() == 2 && values.len() == 2
        ));
        assert!(matches!(
            &function.body[1],
            waluau_ast::Stmt::AssignMulti { targets, values } if targets.len() == 2 && values.len() == 2
        ));
    }

    #[test]
    fn parses_untyped_multi_local() {
        let source = r#"
            function entry(x: i32, y: i32): i32
                local a, b = x, y
                return a
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let function = &program.functions[0];
        assert!(matches!(
            &function.body[0],
            waluau_ast::Stmt::LetMulti { bindings, values }
                if bindings.len() == 2
                    && values.len() == 2
                    && bindings.iter().all(|binding| binding.ty.is_none())
        ));
    }

    #[test]
    fn rejects_if_expression_without_else() {
        let source = r#"
            function entry(flag: bool, x: i32): i32
                return if flag then x
            end
        "#;
        let error = parse(source).expect_err("parse should fail");
        assert!(
            error
                .to_string()
                .contains("expected 'else' in if expression")
        );
    }

    #[test]
    fn rejects_incomplete_call_expression_without_hanging() {
        let error = parse("a(").expect_err("parse should fail");
        let message = error.to_string();
        assert!(
            message.contains("unexpected end of input")
                || message.contains("expected ')' after call arguments")
        );
    }

    #[test]
    fn parses_top_level_statements_with_functions() {
        let source = r#"
            local x: i32 = 41
            function add1(v: i32): i32
                return v + 1
            end
            x += 1
        "#;
        let program = parse(source).expect("parse should succeed");
        assert_eq!(program.functions.len(), 1);
        assert_eq!(program.top_level.len(), 2);
    }

    #[test]
    fn captures_trailing_top_level_return_as_module_export() {
        let source = r#"
            function helper(): i32
                return 1
            end
            return helper
        "#;
        let program = parse(source).expect("parse should succeed");
        assert!(matches!(program.export, Some(waluau_ast::Expr::Name(name)) if name == "helper"));
    }

    #[test]
    fn rejects_top_level_return_that_is_not_last() {
        let source = r#"
            return 1
            local x: i32 = 2
        "#;
        let error = parse(source).expect_err("parse should fail");
        assert!(
            error
                .to_string()
                .contains("top-level return must be the final statement")
        );
    }

    #[test]
    fn parses_require_as_a_dedicated_node() {
        let source = r#"
            local add: (i32, i32) -> i32 = require("./add")
        "#;
        let program = parse(source).expect("parse should succeed");
        let waluau_ast::Stmt::Let { value, .. } = &program.top_level[0] else {
            panic!("expected a let binding");
        };
        assert!(matches!(value, waluau_ast::Expr::Require(path) if path == "./add"));
    }

    #[test]
    fn parses_string_literals_as_values() {
        let source = r#"local x: string = "ok""#;
        let program = parse(source).expect("parse should succeed");
        let waluau_ast::Stmt::Let { value, .. } = &program.top_level[0] else {
            panic!("expected a let binding");
        };
        assert!(matches!(value, waluau_ast::Expr::String(value) if value == "ok"));
    }

    #[test]
    fn parses_break_and_continue_in_loops() {
        let source = r#"
            function entry(): i32
                while true do
                    break
                end
                repeat
                    continue
                until true
                return 0
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let function = &program.functions[0];
        let mut saw_break = false;
        let mut saw_continue = false;
        for stmt in &function.body {
            match stmt {
                Stmt::While { body, .. } if matches!(body.first(), Some(Stmt::Break)) => {
                    saw_break = true;
                }
                Stmt::Repeat { body, .. } if matches!(body.first(), Some(Stmt::Continue)) => {
                    saw_continue = true;
                }
                _ => {}
            }
        }
        assert!(saw_break, "expected a while loop containing a break");
        assert!(
            saw_continue,
            "expected a repeat-until loop containing a continue"
        );
    }
}
