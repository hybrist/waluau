use waluau_ast::{AssignOp, BinaryOp, Binding, Expr, Rebindability, Stmt, Type};
use waluau_diagnostics::Diagnostic;
use waluau_lexer::TokenKind;

use super::Parser;

impl Parser {
    pub(super) fn parse_block_until(&mut self, end_markers: &[TokenKind]) -> Vec<Stmt> {
        let mut statements = Vec::new();
        while let Some(token) = self.peek() {
            // Luau allows `;` as an optional statement separator.
            if matches!(token.kind, TokenKind::Semicolon) {
                self.advance();
                continue;
            }
            if end_markers
                .iter()
                .any(|marker| super::tokens::same_variant(&token.kind, marker))
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

    pub(super) fn parse_stmt(&mut self) -> Result<Stmt, Diagnostic> {
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
        if self.check_simple(&TokenKind::For) {
            self.advance();
            let first_name = self.expect_identifier()?;
            if self.check_simple(&TokenKind::Equal) {
                self.advance();
                let start = self.parse_expr()?;
                self.expect_simple(TokenKind::Comma, "expected ',' after for loop start")?;
                let stop = self.parse_expr()?;
                let step = if self.check_simple(&TokenKind::Comma) {
                    self.advance();
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                self.expect_simple(TokenKind::Do, "expected 'do' after for loop range")?;
                let body = self.parse_block_until(&[TokenKind::End]);
                self.expect_simple(TokenKind::End, "expected 'end' after for loop body")?;
                return Ok(Stmt::NumericFor {
                    name: first_name,
                    symbol_id: None,
                    start,
                    stop,
                    step,
                    body,
                });
            }

            let mut names = vec![first_name];
            while self.check_simple(&TokenKind::Comma) {
                self.advance();
                names.push(self.expect_identifier()?);
            }
            self.expect_simple(TokenKind::In, "expected 'in' after for loop variables")?;
            let iterator = self.parse_expr()?;
            self.expect_simple(TokenKind::Do, "expected 'do' after for loop iterator")?;
            let body = self.parse_block_until(&[TokenKind::End]);
            self.expect_simple(TokenKind::End, "expected 'end' after for loop body")?;
            return Ok(Stmt::ForIn {
                names,
                symbol_ids: None,
                iterator,
                body,
            });
        }
        if self.check_simple(&TokenKind::Do) {
            self.advance();
            let body = self.parse_block_until(&[TokenKind::End]);
            self.expect_simple(TokenKind::End, "expected 'end' after do block")?;
            // A standalone `do ... end` block only introduces a scope; reuse
            // the if-statement machinery (which scopes its branch bodies
            // everywhere downstream) instead of adding a dedicated AST node.
            return Ok(Stmt::If {
                condition: Expr::Bool(true, None),
                then_body: body,
                else_body: Vec::new(),
            });
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
            if self.is_end_marker(&[
                TokenKind::ElseIf,
                TokenKind::Else,
                TokenKind::End,
                TokenKind::Until,
            ]) {
                return Ok(Stmt::Return(Expr::Nil(None)));
            }
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
        if self.check_simple(&TokenKind::Function) {
            return self.parse_local_function_decl();
        }
        let name = self.expect_identifier()?;
        let rebindability = if self.check_simple(&TokenKind::Less) {
            self.advance();
            let attr_name = self.expect_identifier()?;
            self.expect_greater("expected '>' after local attribute")?;
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
                symbol_id: None,
                rebindability,
                ty,
            }];
            self.advance();
            bindings.extend(self.parse_binding_list()?);
            // An uninitialized local declaration omits the '=' initializer
            // entirely; every binding defaults to nil.
            if !self.check_simple(&TokenKind::Equal) {
                let values = bindings.iter().map(|_| Expr::Nil(None)).collect();
                return Ok(Stmt::LetMulti { bindings, values });
            }
            self.advance();
            let values = self.parse_expr_list()?;
            return Ok(Stmt::LetMulti { bindings, values });
        }
        // Without an '=' the local is declared but uninitialized, defaulting to nil.
        let value = if self.check_simple(&TokenKind::Equal) {
            self.advance();
            self.parse_expr()?
        } else {
            Expr::Nil(None)
        };
        Ok(Stmt::Let {
            name,
            symbol_id: None,
            rebindability,
            ty,
            value,
        })
    }

    /// `local function f(...) ... end` declares a local binding whose name is
    /// visible inside its own body. Desugars to a `let` of a *named* function
    /// expression: the resolver and IR lowering already bind a named function
    /// expression's own name inside its body, which is exactly Luau's
    /// recursion semantics for `local function`.
    fn parse_local_function_decl(&mut self) -> Result<Stmt, Diagnostic> {
        let start_pos = self.peek().map(|t| t.span.start).unwrap_or(0);
        self.expect_simple(TokenKind::Function, "expected 'function' after 'local'")?;
        let name = self.expect_identifier()?;
        let function = self.parse_function_expr_tail(Some(name.clone()), false, start_pos)?;
        Ok(Stmt::Let {
            name,
            symbol_id: None,
            rebindability: Rebindability::Rebindable,
            ty: None,
            value: Expr::Function(function),
        })
    }

    /// `const` is contextual, not reserved. Only an identifier or `function`
    /// after it can begin a declaration: two adjacent identifiers are never
    /// valid Lua otherwise, so `const = 1`, `const, x = ...`, and `const(...)`
    /// keep parsing as ordinary assignment/expression statements.
    fn is_const_decl_start(&self) -> bool {
        matches!(
            (
                self.peek().map(|token| &token.kind),
                self.peek_n(1).map(|token| &token.kind),
            ),
            (
                Some(TokenKind::Identifier(keyword)),
                Some(TokenKind::Identifier(_) | TokenKind::Function)
            ) if keyword == "const"
        )
    }

    fn parse_const_decl(&mut self) -> Result<Stmt, Diagnostic> {
        let keyword = self.expect_identifier()?;
        if keyword != "const" {
            return Err(Diagnostic::new("expected 'const'"));
        }
        if self.check_simple(&TokenKind::Function) {
            let start_pos = self.peek().map(|t| t.span.start).unwrap_or(0);
            self.advance();
            let name = self.expect_identifier()?;
            let function = self.parse_function_expr_tail(Some(name.clone()), false, start_pos)?;
            return Ok(Stmt::Let {
                name,
                symbol_id: None,
                rebindability: Rebindability::Const,
                ty: None,
                value: Expr::Function(function),
            });
        }
        let mut bindings = Vec::new();
        loop {
            let name = self.expect_identifier()?;
            let ty = if self.check_simple(&TokenKind::Colon) {
                self.advance();
                Some(self.parse_type()?)
            } else {
                None
            };
            bindings.push(Binding {
                name,
                symbol_id: None,
                rebindability: Rebindability::Const,
                ty,
            });
            if !self.check_simple(&TokenKind::Comma) {
                break;
            }
            self.advance();
        }
        // Unlike `local`, a const binding must be initialized at declaration.
        self.expect_simple(
            TokenKind::Equal,
            "expected '=' in const declaration (const bindings must be initialized)",
        )?;
        let values = self.parse_expr_list()?;
        Ok(if bindings.len() == 1 && values.len() == 1 {
            let binding = bindings.into_iter().next().expect("len checked");
            Stmt::Let {
                name: binding.name,
                symbol_id: None,
                rebindability: Rebindability::Const,
                ty: binding.ty,
                value: values.into_iter().next().expect("len checked"),
            }
        } else {
            Stmt::LetMulti { bindings, values }
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
            AssignOp::Compound(BinaryOp::Add)
        } else if self.check_simple(&TokenKind::MinusEqual) {
            AssignOp::Compound(BinaryOp::Sub)
        } else if self.check_simple(&TokenKind::StarEqual) {
            AssignOp::Compound(BinaryOp::Mul)
        } else if self.check_simple(&TokenKind::SlashEqual) {
            AssignOp::Compound(BinaryOp::Div)
        } else if self.check_simple(&TokenKind::DoubleSlashEqual) {
            AssignOp::Compound(BinaryOp::FloorDiv)
        } else if self.check_simple(&TokenKind::PercentEqual) {
            AssignOp::Compound(BinaryOp::Mod)
        } else if self.check_simple(&TokenKind::CaretEqual) {
            AssignOp::Compound(BinaryOp::Pow)
        } else if self.check_simple(&TokenKind::DoubleDotEqual) {
            AssignOp::Compound(BinaryOp::Concat)
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
                Expr::Name(name, _, _) => Stmt::Assign {
                    op,
                    name,
                    symbol_id: None,
                    value: values.into_iter().next().expect("len checked"),
                },
                Expr::Index { base, index, .. } => Stmt::IndexAssign {
                    op,
                    base,
                    index,
                    value: values.into_iter().next().expect("len checked"),
                },
                Expr::Field { base, name, .. } => Stmt::FieldAssign {
                    op,
                    base,
                    name,
                    resolved_name: None,
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
                    Expr::Name(name, _, _) => names.push(name),
                    _ => return Err(Diagnostic::new("multi-assignment targets must be names")),
                }
            }
            Stmt::AssignMulti {
                targets: names,
                symbol_ids: None,
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
                self.expect_greater("expected '>' after local attribute")?;
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
                symbol_id: None,
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

    pub(super) fn parse_expr_list(&mut self) -> Result<Vec<Expr>, Diagnostic> {
        let mut values = vec![self.parse_expr()?];
        while self.check_simple(&TokenKind::Comma) {
            self.advance();
            values.push(self.parse_expr()?);
        }
        Ok(values)
    }

    fn is_end_marker(&self, markers: &[TokenKind]) -> bool {
        let Some(token) = self.peek() else {
            return true;
        };
        markers
            .iter()
            .any(|marker| super::tokens::same_variant(&token.kind, marker))
    }

    fn parse_if_stmt(&mut self) -> Result<Stmt, Diagnostic> {
        self.expect_simple(TokenKind::If, "expected 'if'")?;
        let stmt = self.parse_if_clause()?;
        self.expect_simple(TokenKind::End, "expected 'end' after if")?;
        Ok(stmt)
    }

    fn parse_if_clause(&mut self) -> Result<Stmt, Diagnostic> {
        if let Some(stmt) = self.try_parse_if_cast_clause()? {
            return Ok(stmt);
        }
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

    fn try_parse_if_cast_clause(&mut self) -> Result<Option<Stmt>, Diagnostic> {
        let (
            Some(TokenKind::Identifier(target_name)),
            Some(TokenKind::LParen),
            Some(TokenKind::Identifier(binding)),
            Some(TokenKind::RParen),
            Some(TokenKind::Equal),
        ) = (
            self.peek().map(|token| &token.kind),
            self.peek_n(1).map(|token| &token.kind),
            self.peek_n(2).map(|token| &token.kind),
            self.peek_n(3).map(|token| &token.kind),
            self.peek_n(4).map(|token| &token.kind),
        )
        else {
            return Ok(None);
        };

        let target_name = target_name.clone();
        let binding = binding.clone();
        self.advance();
        self.advance();
        self.advance();
        self.advance();
        self.advance();
        let value = self.parse_expr()?;
        self.expect_simple(TokenKind::Then, "expected 'then' after if-cast")?;
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

        Ok(Some(Stmt::IfCast {
            target_name: target_name.clone(),
            target_ty: Type::Named {
                name: target_name,
                type_args: Vec::new(),
            },
            binding,
            binding_symbol_id: None,
            value,
            then_body,
            else_body,
        }))
    }
}
