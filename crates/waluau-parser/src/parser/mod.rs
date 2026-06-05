use waluau_ast::{
    Function, FunctionExpr, FunctionName, Param, Program, Span, Stmt, Type, TypeAlias,
};
use waluau_diagnostics::Diagnostic;
use waluau_lexer::{Token, TokenKind};

mod expr;
mod stmt;
mod tokens;
mod types;

pub(super) struct Parser {
    tokens: Vec<Token>,
    index: usize,
    diagnostics: Vec<Diagnostic>,
    /// Type parameters visible while parsing a function signature or body.
    type_param_scope: Vec<String>,
    file_path: String,
}

impl Parser {
    pub(super) fn new(tokens: Vec<Token>, file_path: String) -> Self {
        Self {
            tokens,
            index: 0,
            diagnostics: Vec::new(),
            type_param_scope: Vec::new(),
            file_path,
        }
    }

    pub(super) fn parse_program(&mut self) -> Result<Program, Diagnostic> {
        let mut functions = Vec::new();
        let mut type_aliases = Vec::new();
        let mut top_level = Vec::new();
        let mut export = None;
        while self.peek().is_some() {
            if self.check_simple(&TokenKind::Type) {
                match self.parse_type_alias() {
                    Ok(alias) => type_aliases.push(alias),
                    Err(error) => {
                        self.record_error(error);
                        self.synchronize_statement(&[], self.index);
                    }
                }
            } else if self.check_simple(&TokenKind::Function) {
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
                type_aliases,
                top_level,
                export,
                sources: std::collections::BTreeMap::new(),
                entry_file_path: self.file_path.clone(),
            })
        } else if self.diagnostics.len() == 1 {
            Err(self.diagnostics.remove(0))
        } else {
            let messages: Vec<String> = self.diagnostics.iter().map(|d| d.to_string()).collect();
            Err(Diagnostic::new(messages.join("\n")))
        }
    }

    fn parse_function(&mut self) -> Result<Function, Diagnostic> {
        let start_pos = self.peek().map(|t| t.span.start).unwrap_or(0);
        self.expect_simple(TokenKind::Function, "expected 'function'")?;
        let name = self.parse_function_name()?;
        let function_expr = self.parse_function_expr_tail(None, false, start_pos)?;
        Ok(Function {
            name,
            type_params: function_expr.type_params,
            params: function_expr.params,
            return_type: function_expr.return_type,
            body: function_expr.body,
            file_path: self.file_path.clone(),
        })
    }

    fn parse_function_name(&mut self) -> Result<FunctionName, Diagnostic> {
        let name = self.expect_identifier()?;
        if self.check_simple(&TokenKind::Colon) {
            self.advance();
            let method = self.expect_identifier()?;
            Ok(FunctionName::Method {
                table: name,
                method,
            })
        } else {
            Ok(FunctionName::Simple(name))
        }
    }

    fn parse_type_alias(&mut self) -> Result<TypeAlias, Diagnostic> {
        self.expect_simple(TokenKind::Type, "expected 'type'")?;
        let name = self.expect_identifier()?;
        let type_params = self.parse_type_param_list()?;
        let scope_token = self.type_param_scope.len();
        self.type_param_scope.extend(type_params.iter().cloned());
        self.expect_simple(TokenKind::Equal, "expected '=' in type alias declaration")?;
        let parsed = self.parse_type().map(|ty| TypeAlias {
            name,
            type_params,
            ty,
        });
        self.type_param_scope.truncate(scope_token);
        parsed
    }

    pub(super) fn parse_function_expr_tail(
        &mut self,
        name: Option<String>,
        require_return_type: bool,
        start_pos: u32,
    ) -> Result<FunctionExpr, Diagnostic> {
        let type_params = self.parse_type_param_list()?;
        let scope_token = self.type_param_scope.len();
        self.type_param_scope.extend(type_params.iter().cloned());
        let parsed = self.parse_function_expr_after_type_params(
            name,
            type_params,
            require_return_type,
            start_pos,
        );
        self.type_param_scope.truncate(scope_token);
        parsed
    }

    fn parse_function_expr_after_type_params(
        &mut self,
        name: Option<String>,
        type_params: Vec<String>,
        require_return_type: bool,
        start_pos: u32,
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
        let end_token = self.peek().cloned();
        let end_pos = end_token.map(|t| t.span.end).unwrap_or(start_pos);
        self.expect_simple(TokenKind::End, "expected 'end' after function body")?;
        Ok(FunctionExpr {
            name,
            implicit_self: None,
            type_params,
            params,
            return_type,
            body,
            file_path: self.file_path.clone(),
            span: Some(Span {
                start: start_pos,
                end: end_pos,
            }),
        })
    }
}
