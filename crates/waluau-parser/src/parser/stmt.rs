use waluau_ast::{AssignOp, BinaryOp, Binding, Expr, Rebindability, Span, Stmt, Type};
use waluau_diagnostics::Diagnostic;
use waluau_lexer::TokenKind;

use crate::{DefinitionKind, InitializerHint};

use super::Parser;

/// The record shape of a table-literal initializer: field names plus the
/// types statically visible in the literal (casts like `10::i32`, string and
/// bool literals, nested tables). Anything else — bare number literals whose
/// width the type checker decides, call results — stays `unknown`.
fn table_literal_record(fields: &[waluau_ast::TableField]) -> Type {
    let mut shape = std::collections::BTreeMap::new();
    for field in fields {
        shape.insert(field.name.clone(), literal_value_type(&field.value));
    }
    Type::Record(shape)
}

fn literal_value_type(value: &Expr) -> Type {
    match value {
        Expr::Cast { ty, .. } => ty.clone(),
        Expr::String(..) => Type::String,
        Expr::Bool(..) => Type::Bool,
        Expr::TableLiteral { fields, .. } => table_literal_record(fields),
        _ => Type::Unknown,
    }
}

/// The statically chaseable shape of an initializer expression, if any.
fn initializer_hint(value: &Expr) -> Option<InitializerHint> {
    let simple_name = |expr: &Expr| match expr {
        Expr::Name(name, _, _) => Some(name.clone()),
        _ => None,
    };
    match value {
        Expr::Call { callee, .. } => match callee.as_ref() {
            Expr::Name(name, _, _) => Some(InitializerHint::Call {
                callee: name.clone(),
            }),
            Expr::Field { base, name, .. } => simple_name(base).map(|base| InitializerHint::Call {
                callee: format!("{base}.{name}"),
            }),
            _ => None,
        },
        Expr::MethodCall { receiver, name, .. } => {
            simple_name(receiver).map(|receiver| InitializerHint::MethodCall {
                receiver,
                method: name.clone(),
            })
        }
        Expr::Field { base, name, .. } => simple_name(base).map(|base| InitializerHint::Field {
            base,
            field: name.clone(),
            indexed: false,
        }),
        Expr::Index { base, .. } => match base.as_ref() {
            Expr::Name(name, _, _) => Some(InitializerHint::Index { base: name.clone() }),
            Expr::Field {
                base: inner, name, ..
            } => simple_name(inner).map(|base| InitializerHint::Field {
                base,
                field: name.clone(),
                indexed: true,
            }),
            _ => None,
        },
        _ => None,
    }
}

impl Parser {
    /// Record a just-parsed `local`/`const` binding. Visibility starts at the
    /// current parse position (the end of the declaring statement), so an
    /// initializer mentioning the same name still resolves to the outer
    /// binding (`local x = x`).
    fn record_local_binding(
        &mut self,
        name: &str,
        name_span: Span,
        ty: Option<Type>,
        value: Option<&Expr>,
    ) {
        let visible_from = self.prev_token_end();
        let index = self.record_definition(
            name.to_string(),
            name_span,
            DefinitionKind::Local,
            ty,
            visible_from,
        );
        if let Some(Expr::Require(path, _)) = value {
            self.definitions[index].require_path = Some(path.clone());
        }
        if let Some(value) = value {
            self.definitions[index].initializer = initializer_hint(value);
            // An unannotated table-literal binding still has a statically
            // known field shape (`local point = { x = 10::i32 }`).
            if self.definitions[index].ty.is_none()
                && let Expr::TableLiteral { fields, .. } = value
            {
                self.definitions[index].ty = Some(table_literal_record(fields));
            }
        }
    }
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
            let scope_mark = self.definition_scope_mark();
            let body = self.parse_block_until(&[TokenKind::End]);
            let scope_end = self.current_scope_boundary();
            self.close_definition_scope(scope_mark, scope_end);
            self.expect_simple(TokenKind::End, "expected 'end' after while")?;
            return Ok(Stmt::While { condition, body });
        }
        if self.check_simple(&TokenKind::Repeat) {
            self.advance();
            let scope_mark = self.definition_scope_mark();
            let body = self.parse_block_until(&[TokenKind::Until]);
            self.expect_simple(TokenKind::Until, "expected 'until' after repeat body")?;
            // The until condition can still see the body's bindings; the
            // scope closes only after it.
            let condition = self.parse_expr()?;
            self.close_definition_scope(scope_mark, self.prev_token_end());
            return Ok(Stmt::Repeat { body, condition });
        }
        if self.check_simple(&TokenKind::For) {
            self.advance();
            let (first_name, first_span) = self.expect_identifier_spanned()?;
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
                // The loop variable becomes visible at the body: the range
                // expressions still see any outer binding of the same name.
                let scope_mark = self.definition_scope_mark();
                self.record_definition(
                    first_name.clone(),
                    first_span,
                    DefinitionKind::LoopVar,
                    None,
                    self.prev_token_end(),
                );
                let body = self.parse_block_until(&[TokenKind::End]);
                let scope_end = self.current_scope_boundary();
                self.close_definition_scope(scope_mark, scope_end);
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
            let mut name_spans = vec![first_span];
            while self.check_simple(&TokenKind::Comma) {
                self.advance();
                let (name, name_span) = self.expect_identifier_spanned()?;
                names.push(name);
                name_spans.push(name_span);
            }
            self.expect_simple(TokenKind::In, "expected 'in' after for loop variables")?;
            let iterator = self.parse_expr()?;
            self.expect_simple(TokenKind::Do, "expected 'do' after for loop iterator")?;
            let scope_mark = self.definition_scope_mark();
            let visible_from = self.prev_token_end();
            for (name, name_span) in names.iter().zip(&name_spans) {
                self.record_definition(
                    name.clone(),
                    *name_span,
                    DefinitionKind::LoopVar,
                    None,
                    visible_from,
                );
            }
            let body = self.parse_block_until(&[TokenKind::End]);
            let scope_end = self.current_scope_boundary();
            self.close_definition_scope(scope_mark, scope_end);
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
            let scope_mark = self.definition_scope_mark();
            let body = self.parse_block_until(&[TokenKind::End]);
            let scope_end = self.current_scope_boundary();
            self.close_definition_scope(scope_mark, scope_end);
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
        let (name, name_span) = self.expect_identifier_spanned()?;
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
            let mut binding_spans = vec![name_span];
            self.advance();
            for (binding, span) in self.parse_binding_list()? {
                bindings.push(binding);
                binding_spans.push(span);
            }
            // An uninitialized local declaration omits the '=' initializer
            // entirely; every binding defaults to nil.
            let values: Vec<Expr> = if self.check_simple(&TokenKind::Equal) {
                self.advance();
                self.parse_expr_list()?
            } else {
                bindings.iter().map(|_| Expr::Nil(None)).collect()
            };
            for (index, binding) in bindings.iter().enumerate() {
                self.record_local_binding(
                    &binding.name,
                    binding_spans[index],
                    binding.ty.clone(),
                    values.get(index),
                );
            }
            return Ok(Stmt::LetMulti { bindings, values });
        }
        // Without an '=' the local is declared but uninitialized, defaulting to nil.
        let value = if self.check_simple(&TokenKind::Equal) {
            self.advance();
            self.parse_expr()?
        } else {
            Expr::Nil(None)
        };
        self.record_local_binding(&name, name_span, ty.clone(), Some(&value));
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
        let (name, name_span) = self.expect_identifier_spanned()?;
        let function = self.parse_function_expr_tail(Some(name.clone()), false, start_pos)?;
        // Visible from the name itself so recursive calls in the body resolve.
        let index = self.record_definition(
            name.clone(),
            name_span,
            DefinitionKind::Function,
            Self::function_signature_type(&function),
            name_span.end,
        );
        self.definitions[index].detail = Some(Self::function_signature_detail(&name, &function));
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
            let (name, name_span) = self.expect_identifier_spanned()?;
            let function = self.parse_function_expr_tail(Some(name.clone()), false, start_pos)?;
            // Visible from the name itself so recursive calls resolve.
            let index = self.record_definition(
                name.clone(),
                name_span,
                DefinitionKind::Function,
                Self::function_signature_type(&function),
                name_span.end,
            );
            self.definitions[index].detail =
                Some(Self::function_signature_detail(&name, &function));
            return Ok(Stmt::Let {
                name,
                symbol_id: None,
                rebindability: Rebindability::Const,
                ty: None,
                value: Expr::Function(function),
            });
        }
        let mut bindings = Vec::new();
        let mut binding_spans = Vec::new();
        loop {
            let (name, name_span) = self.expect_identifier_spanned()?;
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
            binding_spans.push(name_span);
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
        for (index, binding) in bindings.iter().enumerate() {
            self.record_local_binding(
                &binding.name,
                binding_spans[index],
                binding.ty.clone(),
                values.get(index),
            );
        }
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

    fn parse_binding_list(&mut self) -> Result<Vec<(Binding, Span)>, Diagnostic> {
        let mut bindings = Vec::new();
        loop {
            let (name, name_span) = self.expect_identifier_spanned()?;
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
            bindings.push((
                Binding {
                    name,
                    symbol_id: None,
                    rebindability,
                    ty,
                },
                name_span,
            ));
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
        let scope_mark = self.definition_scope_mark();
        let then_body =
            self.parse_block_until(&[TokenKind::ElseIf, TokenKind::Else, TokenKind::End]);
        self.close_definition_scope(scope_mark, self.current_scope_boundary());
        let else_body = if self.check_simple(&TokenKind::ElseIf) {
            self.advance();
            vec![self.parse_if_clause()?]
        } else if self.check_simple(&TokenKind::Else) {
            self.advance();
            let scope_mark = self.definition_scope_mark();
            let body = self.parse_block_until(&[TokenKind::End]);
            self.close_definition_scope(scope_mark, self.current_scope_boundary());
            body
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
        let binding_span = self.peek_n(2).map(|token| token.span).unwrap_or_default();
        self.advance();
        self.advance();
        self.advance();
        self.advance();
        self.advance();
        let value = self.parse_expr()?;
        self.expect_simple(TokenKind::Then, "expected 'then' after if-cast")?;
        // The narrowed binding exists in the then-branch only.
        let scope_mark = self.definition_scope_mark();
        self.record_definition(
            binding.clone(),
            binding_span,
            DefinitionKind::IfCastBinding,
            Some(Type::Named {
                name: target_name.clone(),
                type_args: Vec::new(),
            }),
            self.prev_token_end(),
        );
        let then_body =
            self.parse_block_until(&[TokenKind::ElseIf, TokenKind::Else, TokenKind::End]);
        self.close_definition_scope(scope_mark, self.current_scope_boundary());
        let else_body = if self.check_simple(&TokenKind::ElseIf) {
            self.advance();
            vec![self.parse_if_clause()?]
        } else if self.check_simple(&TokenKind::Else) {
            self.advance();
            let scope_mark = self.definition_scope_mark();
            let body = self.parse_block_until(&[TokenKind::End]);
            self.close_definition_scope(scope_mark, self.current_scope_boundary());
            body
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
