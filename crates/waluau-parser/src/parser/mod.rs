use waluau_ast::{
    DeclaredConstant, DeclaredImport, Function, FunctionExpr, FunctionName, Param, Program, Span,
    Stmt, Type, TypeDeclaration,
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

    pub(super) fn parse_program(&mut self, source: &str) -> Result<Program, Diagnostic> {
        let mut functions = Vec::new();
        let mut declared_imports = Vec::new();
        let mut declared_constants = Vec::new();
        let mut type_declarations = Vec::new();
        let mut top_level = Vec::new();
        let mut export = None;
        while self.peek().is_some() {
            // Luau allows `;` as an optional statement separator.
            if self.check_simple(&TokenKind::Semicolon) {
                self.advance();
                continue;
            }
            if self.is_type_decl_start() {
                match self.parse_type_decl() {
                    Ok(type_decl) => type_declarations.push(type_decl),
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
            } else if self.is_declare_function_start() {
                match self.parse_declared_import() {
                    Ok(declared) => declared_imports.push(declared),
                    Err(error) => {
                        self.record_error(error);
                        self.synchronize_statement(&[], self.index);
                    }
                }
            } else if self.is_declare_property_start() {
                match self.parse_declared_property() {
                    Ok(mut declared) => declared_imports.append(&mut declared),
                    Err(error) => {
                        self.record_error(error);
                        self.synchronize_statement(&[], self.index);
                    }
                }
            } else if self.is_declare_const_start() {
                match self.parse_declared_const() {
                    Ok(declared) => declared_constants.push(declared),
                    Err(error) => {
                        self.record_error(error);
                        self.synchronize_statement(&[], self.index);
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
                declared_imports,
                declared_constants,
                type_declarations,
                top_level,
                export,
                sources: std::collections::BTreeMap::from([(
                    self.file_path.clone(),
                    source.to_string(),
                )]),
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
            symbol_id: None,
            type_params: function_expr.type_params,
            params: function_expr.params,
            vararg: function_expr.vararg,
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
        } else if self.check_simple(&TokenKind::Dot) {
            // A dot-named function (`function State.new(...)`) is a plain
            // function under the dotted name — no implicit self parameter,
            // unlike `:` method sugar. `.` cannot appear in identifiers, so
            // the name never collides with user bindings; call sites resolve
            // `State.new(...)` through the qualified-name lookup.
            self.advance();
            let member = self.expect_identifier()?;
            let dotted = format!("{name}.{member}");
            if self.check_simple(&TokenKind::Colon) {
                return Err(Diagnostic::new(format!(
                    "cannot declare a method on dot-named function '{dotted}'"
                )));
            }
            Ok(FunctionName::Simple(dotted))
        } else {
            Ok(FunctionName::Simple(name))
        }
    }

    fn is_declare_function_start(&self) -> bool {
        matches!(
            (
                self.peek().map(|token| &token.kind),
                self.peek_n(1).map(|token| &token.kind),
            ),
            (
                Some(TokenKind::Identifier(keyword)),
                Some(TokenKind::Function)
            ) if keyword == "declare"
        )
    }

    fn is_declare_property_start(&self) -> bool {
        matches!(
            (
                self.peek().map(|token| &token.kind),
                self.peek_n(1).map(|token| &token.kind),
            ),
            (
                Some(TokenKind::Identifier(keyword)),
                Some(TokenKind::Identifier(kind))
            ) if keyword == "declare" && kind == "property"
        )
    }

    fn is_declare_const_start(&self) -> bool {
        matches!(
            (
                self.peek().map(|token| &token.kind),
                self.peek_n(1).map(|token| &token.kind),
            ),
            (
                Some(TokenKind::Identifier(keyword)),
                Some(TokenKind::Identifier(kind))
            ) if keyword == "declare" && kind == "const"
        )
    }

    /// Parses `declare const math.pi: f64 = 3.141592653589793`: a named
    /// compile-time constant on a builtin namespace. Reads fold to the
    /// literal during lowering, so nothing is imported from the host.
    fn parse_declared_const(&mut self) -> Result<DeclaredConstant, Diagnostic> {
        let keyword = self.expect_identifier()?;
        if keyword != "declare" {
            return Err(Diagnostic::new("expected 'declare'"));
        }
        let kind = self.expect_identifier()?;
        if kind != "const" {
            return Err(Diagnostic::new("expected 'const' after 'declare'"));
        }
        let namespace = self.expect_identifier()?;
        self.expect_simple(TokenKind::Dot, "expected '.' after constant namespace")?;
        let member = self.expect_identifier()?;
        let name = format!("{namespace}.{member}");
        self.expect_simple(TokenKind::Colon, "expected ':' before constant type")?;
        let ty = self.parse_type()?;
        if !matches!(ty, Type::Numeric(_)) {
            return Err(Diagnostic::new(format!(
                "declared constant '{name}' must have a numeric type"
            )));
        }
        self.expect_simple(TokenKind::Equal, "expected '=' after constant type")?;
        let value = match self.peek().map(|token| token.kind.clone()) {
            Some(TokenKind::Number(raw)) => {
                self.advance();
                waluau_ast::NumberLiteral { raw }
            }
            _ => {
                return Err(Diagnostic::new(format!(
                    "declared constant '{name}' must be initialized with a number literal"
                )));
            }
        };
        Ok(DeclaredConstant { name, ty, value })
    }

    fn parse_declared_import(&mut self) -> Result<DeclaredImport, Diagnostic> {
        let keyword = self.expect_identifier()?;
        if keyword != "declare" {
            return Err(Diagnostic::new("expected 'declare'"));
        }
        self.expect_simple(TokenKind::Function, "expected 'function' after 'declare'")?;
        let receiver = self.expect_identifier()?;
        let (name, receiver_param) = if self.check_simple(&TokenKind::Colon) {
            self.advance();
            let method = self.expect_identifier()?;
            let name = format!("{receiver}.{method}");
            let receiver_param = Param {
                name: "self".to_string(),
                symbol_id: None,
                ty: Type::Named {
                    name: receiver,
                    type_args: Vec::new(),
                },
            };
            (name, Some(receiver_param))
        } else if self.check_simple(&TokenKind::Dot) {
            // Namespaced host function (`declare function math.abs(...)`):
            // the dotted name is the function's identity; unlike `:` method
            // sugar there is no implicit receiver parameter.
            self.advance();
            let member = self.expect_identifier()?;
            (format!("{receiver}.{member}"), None)
        } else {
            (receiver, None)
        };
        let function_expr = self.parse_function_signature_tail(None, true, name.clone())?;
        if !function_expr.type_params.is_empty() {
            return Err(Diagnostic::new(format!(
                "declared host function '{name}' cannot be generic"
            )));
        }
        let return_type = function_expr.return_type.ok_or_else(|| {
            Diagnostic::new(format!(
                "declared host function '{name}' must have a return type"
            ))
        })?;
        let mut params = function_expr.params;
        if let Some(receiver_param) = receiver_param {
            params.insert(0, receiver_param);
        }
        Ok(DeclaredImport {
            host_name: name.clone(),
            name,
            symbol_id: None,
            params,
            return_type,
        })
    }

    fn parse_declared_property(&mut self) -> Result<Vec<DeclaredImport>, Diagnostic> {
        let keyword = self.expect_identifier()?;
        if keyword != "declare" {
            return Err(Diagnostic::new("expected 'declare'"));
        }
        let kind = self.expect_identifier()?;
        if kind != "property" {
            return Err(Diagnostic::new("expected 'property' after 'declare'"));
        }
        let receiver = self.expect_identifier()?;
        self.expect_simple(TokenKind::Colon, "expected ':' after property receiver")?;
        let property = self.expect_identifier()?;
        self.expect_simple(TokenKind::Colon, "expected ':' before property type")?;
        let property_type = self.parse_type()?;
        let receiver_ty = Type::Named {
            name: receiver.clone(),
            type_args: Vec::new(),
        };
        Ok(vec![
            DeclaredImport {
                host_name: format!("{receiver}.get/{property}"),
                name: format!("{receiver}.get/{property}"),
                symbol_id: None,
                params: vec![Param {
                    name: "self".to_string(),
                    symbol_id: None,
                    ty: receiver_ty.clone(),
                }],
                return_type: property_type.clone(),
            },
            DeclaredImport {
                host_name: format!("{receiver}.set/{property}"),
                name: format!("{receiver}.set/{property}"),
                symbol_id: None,
                params: vec![
                    Param {
                        name: "self".to_string(),
                        symbol_id: None,
                        ty: receiver_ty,
                    },
                    Param {
                        name: "value".to_string(),
                        symbol_id: None,
                        ty: property_type,
                    },
                ],
                return_type: Type::Unit,
            },
        ])
    }

    fn is_type_decl_start(&self) -> bool {
        matches!(
            (
                self.peek().map(|token| &token.kind),
                self.peek_n(1).map(|token| &token.kind),
                self.peek_n(2).map(|token| &token.kind),
            ),
            (
                Some(TokenKind::Identifier(keyword)),
                Some(TokenKind::Identifier(_)),
                Some(TokenKind::Equal | TokenKind::Less)
            ) if keyword == "type"
        )
    }

    fn parse_type_decl(&mut self) -> Result<TypeDeclaration, Diagnostic> {
        let keyword = self.expect_identifier()?;
        if keyword != "type" {
            return Err(Diagnostic::new("expected 'type'"));
        }
        let name = self.expect_identifier()?;
        let type_params = self.parse_type_param_list()?;
        let scope_token = self.type_param_scope.len();
        self.type_param_scope.extend(type_params.iter().cloned());
        self.expect_simple(TokenKind::Equal, "expected '=' in type declaration")?;
        let parsed = self.parse_type().map(|ty| TypeDeclaration {
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

    fn parse_function_signature_tail(
        &mut self,
        name: Option<String>,
        require_return_type: bool,
        display_name: String,
    ) -> Result<FunctionExpr, Diagnostic> {
        let start_pos = self.peek().map(|t| t.span.start).unwrap_or(0);
        let type_params = self.parse_type_param_list()?;
        let scope_token = self.type_param_scope.len();
        self.type_param_scope.extend(type_params.iter().cloned());
        let parsed = self.parse_function_signature_after_type_params(
            name,
            type_params,
            require_return_type,
            start_pos,
            display_name,
        );
        self.type_param_scope.truncate(scope_token);
        parsed
    }

    fn parse_function_signature_after_type_params(
        &mut self,
        name: Option<String>,
        type_params: Vec<String>,
        require_return_type: bool,
        start_pos: u32,
        display_name: String,
    ) -> Result<FunctionExpr, Diagnostic> {
        self.expect_simple(TokenKind::LParen, "expected '('")?;
        let mut params = Vec::new();
        let mut vararg = false;
        if !self.check_simple(&TokenKind::RParen) {
            loop {
                if self.check_simple(&TokenKind::TripleDot) {
                    self.advance();
                    vararg = true;
                    break;
                }
                let param_name = self.expect_identifier()?;
                let param_type = if self.check_simple(&TokenKind::Colon) {
                    self.advance();
                    match self.parse_type() {
                        Ok(ty) => ty,
                        Err(error) => {
                            self.record_error(error);
                            Type::number()
                        }
                    }
                } else {
                    Type::Unknown
                };
                params.push(Param {
                    name: param_name,
                    symbol_id: None,
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
            return Err(Diagnostic::new(format!(
                "expected ':' before return type for declared host function '{display_name}'"
            )));
        } else {
            None
        };
        let end_pos = self.peek().map(|t| t.span.start).unwrap_or(start_pos);
        Ok(FunctionExpr {
            name,
            symbol_id: None,
            implicit_self: None,
            type_params,
            params,
            vararg,
            return_type,
            body: Vec::new(),
            file_path: self.file_path.clone(),
            span: Some(Span {
                start: start_pos,
                end: end_pos,
            }),
        })
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
        let mut vararg = false;
        if !self.check_simple(&TokenKind::RParen) {
            loop {
                if self.check_simple(&TokenKind::TripleDot) {
                    self.advance();
                    vararg = true;
                    break;
                }
                let param_name = self.expect_identifier()?;
                let param_type = if self.check_simple(&TokenKind::Colon) {
                    self.advance();
                    match self.parse_type() {
                        Ok(ty) => ty,
                        Err(error) => {
                            self.record_error(error);
                            Type::number()
                        }
                    }
                } else {
                    Type::Unknown
                };
                params.push(Param {
                    name: param_name,
                    symbol_id: None,
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
            symbol_id: None,
            implicit_self: None,
            type_params,
            params,
            vararg,
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
