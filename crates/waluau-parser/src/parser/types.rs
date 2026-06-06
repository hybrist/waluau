use waluau_ast::{NumericType, TaggedVariant, Type};
use waluau_diagnostics::Diagnostic;
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
        let first = self.parse_type_atom()?;
        if !self.check_simple(&TokenKind::Pipe) {
            return Ok(first);
        }

        let mut variants = vec![match first {
            Type::TaggedVariant(variant) => variant,
            other => {
                return Err(Diagnostic::new(format!(
                    "tagged union member must be a tagged variant, got {other}"
                )));
            }
        }];

        while self.check_simple(&TokenKind::Pipe) {
            self.advance();
            match self.parse_type_atom()? {
                Type::TaggedVariant(variant) => variants.push(variant),
                other => {
                    return Err(Diagnostic::new(format!(
                        "tagged union member must be a tagged variant, got {other}"
                    )));
                }
            }
        }

        Ok(Type::TaggedUnion(variants))
    }

    fn parse_type_atom(&mut self) -> Result<Type, Diagnostic> {
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
            Some(TokenKind::BytesType) => Ok(Type::Bytes),
            Some(TokenKind::ExternType) => Ok(Type::Extern),
            Some(TokenKind::ThreadType) => Ok(Type::Thread),
            Some(TokenKind::UnknownType) => Ok(Type::Unknown),
            Some(TokenKind::Identifier(name)) if self.check_simple(&TokenKind::LParen) => {
                self.advance();
                let payload = self.parse_type()?;
                self.expect_simple(TokenKind::RParen, "expected ')' after tagged variant payload")?;
                Ok(Type::TaggedVariant(TaggedVariant {
                    tag: name,
                    payload: Box::new(payload),
                }))
            }
            Some(TokenKind::Identifier(name)) if self.type_param_scope.contains(&name) => {
                if self.check_simple(&TokenKind::Less) {
                    Err(self.diagnostic_at_current(&format!(
                        "type parameter '{name}' cannot be used with type arguments"
                    )))
                } else {
                    Ok(Type::TypeParam(name))
                }
            }
            Some(TokenKind::Identifier(name)) => {
                let type_args = if self.check_simple(&TokenKind::Less) {
                    self.parse_type_arg_list()?
                } else {
                    Vec::new()
                };
                Ok(Type::Named { name, type_args })
            }
            _ => Err(self.diagnostic_at_current(
                "expected type (number, u32, u64, i32, i64, f32, f64, unit, bool, unknown, string, bytes, extern, thread, Tag(T), a named type, Foo<T>, {T}, { x: T }, or (T1, T2) -> R)",
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

    fn parse_type_arg_list(&mut self) -> Result<Vec<Type>, Diagnostic> {
        self.expect_simple(TokenKind::Less, "expected '<' before type arguments")?;
        let mut type_args = Vec::new();
        if !self.check_simple(&TokenKind::Greater) {
            loop {
                type_args.push(self.parse_type()?);
                if self.check_simple(&TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.expect_simple(TokenKind::Greater, "expected '>' after type arguments")?;
        Ok(type_args)
    }
}
