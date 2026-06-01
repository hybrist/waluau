use waluau_ast::{NumericType, Type};
use waluau_diagnostics::{Diagnostic, DiagnosticCategory};
use waluau_lexer::{Token, TokenKind};

use super::Parser;

impl Parser {
    pub(super) fn parse_type_param_list(&mut self) -> Result<Vec<String>, Diagnostic> {
        if !self.check_simple(&TokenKind::Less) {
            return Ok(Vec::new());
        }
        self.advance();
        let mut params = Vec::new();
        if !self.check_simple(&TokenKind::Greater) {
            loop {
                params.push(self.expect_identifier()?);
                if self.check_simple(&TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.expect_simple(TokenKind::Greater, "expected '>' after type parameters")?;
        Ok(params)
    }

    pub(super) fn parse_return_type_list(&mut self) -> Result<Type, Diagnostic> {
        let first = self.parse_return_type()?;
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

    pub(super) fn parse_type(&mut self) -> Result<Type, Diagnostic> {
        if self.check_simple(&TokenKind::LBrace) {
            self.advance();
            let is_record_type = matches!(
                (self.tokens.get(self.index), self.tokens.get(self.index + 1)),
                (
                    Some(Token {
                        kind: TokenKind::Identifier(_),
                        ..
                    }),
                    Some(Token {
                        kind: TokenKind::Colon,
                        ..
                    })
                )
            );
            if is_record_type {
                let mut fields = std::collections::BTreeMap::new();
                loop {
                    let name = self.expect_identifier()?;
                    self.expect_simple(TokenKind::Colon, "expected ':' after record field name")?;
                    let field_ty = self.parse_type()?;
                    fields.insert(name, field_ty);
                    if self.check_simple(&TokenKind::Comma) {
                        self.advance();
                    } else {
                        break;
                    }
                }
                self.expect_simple(TokenKind::RBrace, "expected '}' after record type")?;
                return Ok(Type::Record(fields));
            }

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
            if params.is_empty() && !self.check_simple(&TokenKind::Arrow) {
                return Ok(Type::Unit);
            }
            self.expect_simple(TokenKind::Arrow, "expected '->' in function type")?;
            let return_type = self.parse_return_type()?;
            return Ok(Type::Function {
                params,
                return_type: Box::new(return_type),
            });
        }

        let token = self.advance();
        match token.map(|t| t.kind.clone()) {
            Some(TokenKind::NumberType | TokenKind::F64Type) => Ok(Type::number()),
            Some(TokenKind::U32Type) => Ok(Type::Numeric(NumericType::U32)),
            Some(TokenKind::U64Type) => Ok(Type::Numeric(NumericType::U64)),
            Some(TokenKind::I32Type) => Ok(Type::Numeric(NumericType::I32)),
            Some(TokenKind::I64Type) => Ok(Type::Numeric(NumericType::I64)),
            Some(TokenKind::F32Type) => Ok(Type::Numeric(NumericType::F32)),
            Some(TokenKind::UnitType) => Ok(Type::Unit),
            Some(TokenKind::BoolType) => Ok(Type::Bool),
            Some(TokenKind::StringType) => Ok(Type::String),
            Some(TokenKind::ThreadType) => Ok(Type::Thread),
            Some(TokenKind::Identifier(name)) if self.check_simple(&TokenKind::Less) => {
                if self.type_param_scope.contains(&name) {
                    Err(self.diagnostic_at_current(&format!(
                        "type parameter '{name}' cannot be used with type arguments"
                    )))
                } else {
                    self.reject_generic_type_annotation(&name)
                }
            }
            Some(TokenKind::Identifier(name)) if self.type_param_scope.contains(&name) => {
                Ok(Type::TypeParam(name))
            }
            _ => Err(self.diagnostic_at_current(
                "expected type (number, u32, u64, i32, i64, f32, f64, unit, bool, string, thread, {T}, { x: T }, or (T1, T2) -> R)",
            )),
        }
    }

    /// Parse the return-type position of a function type annotation.
    ///
    /// `(T1, T2)` not followed by `->` becomes `Type::Multi([T1, T2])`.
    /// `(T1, T2) -> R` becomes a nested `Type::Function`.
    /// `()` becomes `Type::Unit`.
    /// Anything else delegates to `parse_type`.
    fn parse_return_type(&mut self) -> Result<Type, Diagnostic> {
        if !self.check_simple(&TokenKind::LParen) {
            return self.parse_type();
        }
        self.advance(); // consume '('
        let mut types = Vec::new();
        if !self.check_simple(&TokenKind::RParen) {
            loop {
                types.push(self.parse_type()?);
                if self.check_simple(&TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.expect_simple(TokenKind::RParen, "expected ')' in return type")?;
        if self.check_simple(&TokenKind::Arrow) {
            self.advance(); // consume '->'
            let nested_return = self.parse_return_type()?;
            return Ok(Type::Function {
                params: types,
                return_type: Box::new(nested_return),
            });
        }
        Ok(match types.len() {
            0 => Type::Unit,
            1 => types.remove(0),
            _ => Type::Multi(types),
        })
    }

    fn reject_generic_type_annotation(&mut self, type_name: &str) -> Result<Type, Diagnostic> {
        let angle_start = self.peek().map(|t| t.span.start).unwrap_or(0);
        self.advance();
        let mut depth = 1u32;
        while depth > 0 {
            match self.peek().map(|t| &t.kind) {
                Some(TokenKind::Less) => {
                    depth += 1;
                    self.advance();
                }
                Some(TokenKind::Greater) => {
                    depth -= 1;
                    self.advance();
                }
                None => break,
                _ => {
                    self.advance();
                }
            }
        }
        let angle_end = self
            .tokens
            .get(self.index.saturating_sub(1))
            .map(|t| t.span.end)
            .unwrap_or(angle_start + 1);
        Err(Diagnostic::new_with_code(
            "generic/unsupported-type",
            format!("generic types are not supported in this MVP: '{type_name}<...>'"),
        )
        .with_category(DiagnosticCategory::Unsupported)
        .with_span(waluau_ast::Span {
            start: angle_start,
            end: angle_end,
        })
        .with_action("use a concrete type like {i32} for arrays"))
    }
}
