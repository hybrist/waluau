use waluau_ast::{
    NumberLiteral, NumberLiteralUnion, NumberUnionMember, NumericType, TaggedVariant, Type,
    TypedArrayKind,
};
use waluau_diagnostics::Diagnostic;
use waluau_lexer::{Token, TokenKind};

use super::Parser;

/// The narrowest integer numeric type covering every member, mirroring the
/// nominal-enum choice of i32 for ordinal-sized values.
fn int_union_numeric(members: &[NumberUnionMember]) -> NumericType {
    let all_i32 = members.iter().all(|member| match member {
        NumberUnionMember::Int(value) => i32::try_from(*value).is_ok(),
        NumberUnionMember::FloatBits(_) => false,
    });
    if all_i32 {
        NumericType::I32
    } else {
        NumericType::I64
    }
}

impl Parser {
    pub(super) fn parse_type_param_list(&mut self) -> Result<Vec<String>, Diagnostic> {
        if !self.check_simple(&TokenKind::Less) {
            return Ok(Vec::new());
        }
        self.advance();
        let mut params = Vec::new();
        if !self.check_greater() {
            loop {
                params.push(self.expect_identifier()?);
                if self.check_simple(&TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.expect_greater("expected '>' after type parameters")?;
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
        let first = self.parse_nullable_type()?;
        if !self.check_simple(&TokenKind::Pipe) {
            return Ok(first);
        }

        match first {
            Type::StringLiteralUnion(members) => self.parse_string_union_tail(members),
            Type::NumberLiteralUnion(union) => self.parse_number_union_tail(union),
            Type::TaggedVariant(variant) => {
                let mut variants = vec![variant];
                while self.check_simple(&TokenKind::Pipe) {
                    self.advance();
                    match self.parse_nullable_type()? {
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
            other => Err(Diagnostic::new(format!(
                "union member must be a tagged variant, a string literal, \
                 or a number literal, got {other}"
            ))),
        }
    }

    fn parse_string_union_tail(&mut self, mut members: Vec<String>) -> Result<Type, Diagnostic> {
        while self.check_simple(&TokenKind::Pipe) {
            self.advance();
            match self.parse_nullable_type()? {
                Type::StringLiteralUnion(mut next) if next.len() == 1 => {
                    let member = next.remove(0);
                    if members.contains(&member) {
                        return Err(Diagnostic::new(format!(
                            "duplicate string literal union member \"{member}\""
                        )));
                    }
                    members.push(member);
                }
                other => {
                    return Err(Diagnostic::new(format!(
                        "string literal union member must be a string literal, got {other}"
                    )));
                }
            }
        }
        Ok(Type::StringLiteralUnion(members))
    }

    fn parse_number_union_tail(
        &mut self,
        mut union: NumberLiteralUnion,
    ) -> Result<Type, Diagnostic> {
        while self.check_simple(&TokenKind::Pipe) {
            self.advance();
            match self.parse_nullable_type()? {
                Type::NumberLiteralUnion(next) if next.members.len() == 1 => {
                    let member = next.members[0];
                    let mixes_int_and_float = matches!(member, NumberUnionMember::Int(_))
                        != matches!(union.members[0], NumberUnionMember::Int(_));
                    if mixes_int_and_float {
                        return Err(Diagnostic::new(
                            "number literal union members must all be integers \
                             or all be floats, not a mix",
                        ));
                    }
                    if union.members.contains(&member) {
                        return Err(Diagnostic::new(format!(
                            "duplicate number literal union member {member}"
                        )));
                    }
                    union.members.push(member);
                }
                other => {
                    return Err(Diagnostic::new(format!(
                        "number literal union member must be a number literal, got {other}"
                    )));
                }
            }
        }
        if matches!(union.members[0], NumberUnionMember::Int(_)) {
            union.numeric = int_union_numeric(&union.members);
        }
        Ok(Type::NumberLiteralUnion(union))
    }

    /// A single number literal in type position, as a one-member union.
    /// Integer-form literals become integer members (widened union-wide to
    /// i32 or i64 by `parse_number_union_tail`); float-form literals become
    /// f64 members.
    fn number_union_atom(&mut self, raw: String, negative: bool) -> Result<Type, Diagnostic> {
        let literal = NumberLiteral { raw };
        if let Some(value) = literal.int_value() {
            let value = if negative { -value } else { value };
            let value = i64::try_from(value).map_err(|_| {
                Diagnostic::new(format!(
                    "number literal union member {value} is out of range for i64"
                ))
            })?;
            let members = vec![NumberUnionMember::Int(value)];
            return Ok(Type::NumberLiteralUnion(NumberLiteralUnion {
                numeric: int_union_numeric(&members),
                members,
            }));
        }
        let Some(value) = literal.float_value() else {
            return Err(Diagnostic::new(format!(
                "invalid number literal '{}' in type position",
                literal.raw
            )));
        };
        let value = if negative { -value } else { value };
        Ok(Type::NumberLiteralUnion(NumberLiteralUnion {
            numeric: NumericType::F64,
            members: vec![NumberUnionMember::float(value)],
        }))
    }

    fn parse_nullable_type(&mut self) -> Result<Type, Diagnostic> {
        let mut ty = self.parse_type_atom()?;
        while self.check_simple(&TokenKind::Question) {
            self.advance();
            ty = Type::Nullable(Box::new(ty));
        }
        Ok(ty)
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
                        if self.check_simple(&TokenKind::RBrace) {
                            break;
                        }
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
            if !self.check_simple(&TokenKind::Arrow) {
                return match params.len() {
                    0 => Ok(Type::Unit),
                    1 => Ok(params.remove(0)),
                    _ => Err(self.diagnostic_at_current(
                        "parenthesized type grouping must contain exactly one type",
                    )),
                };
            }
            self.advance();
            let return_type = self.parse_return_type()?;
            return Ok(Type::Function {
                params,
                return_type: Box::new(return_type),
            });
        }

        let token = self.advance();
        match token.map(|t| t.kind.clone()) {
            Some(TokenKind::Str(value)) => Ok(Type::StringLiteralUnion(vec![value])),
            Some(TokenKind::Number(raw)) => self.number_union_atom(raw, false),
            Some(TokenKind::Minus) => match self.advance().map(|t| t.kind.clone()) {
                Some(TokenKind::Number(raw)) => self.number_union_atom(raw, true),
                _ => Err(self.diagnostic_at_current(
                    "expected a number literal after '-' in type position",
                )),
            },
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
            Some(TokenKind::ExternType) => {
                if matches!(
                    self.peek().map(|token| &token.kind),
                    Some(TokenKind::Identifier(keyword)) if keyword == "extends"
                ) {
                    self.advance();
                    let parent = self.parse_type()?;
                    Ok(Type::ExternSubtype(Box::new(parent)))
                } else {
                    Ok(Type::Extern)
                }
            }
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
            Some(TokenKind::Identifier(name))
                if TypedArrayKind::from_type_name(&name).is_some() =>
            {
                Ok(Type::TypedArray(
                    TypedArrayKind::from_type_name(&name).expect("checked in guard"),
                ))
            }
            Some(TokenKind::Identifier(name)) => {
                // A dotted name (`game.State`) references a type alias
                // exported by a required module; the linker resolves the
                // namespace against the module's require bindings.
                let name = if self.check_simple(&TokenKind::Dot) {
                    self.advance();
                    let member = self.expect_identifier()?;
                    format!("{name}.{member}")
                } else {
                    name
                };
                let type_args = if self.check_simple(&TokenKind::Less) {
                    self.parse_type_arg_list()?
                } else {
                    Vec::new()
                };
                Ok(Type::Named { name, type_args })
            }
            _ => Err(self.diagnostic_at_current(
                "expected type (number, u32, u64, i32, i64, f32, f64, unit, bool, unknown, string, bytes, extern, thread, Tag(T), a named type, Foo<T>, {T}, { x: T }, (T1, T2) -> R, or a literal union member like \"red\" or 0)",
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
        if !self.check_greater() {
            loop {
                type_args.push(self.parse_type()?);
                if self.check_simple(&TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.expect_greater("expected '>' after type arguments")?;
        Ok(type_args)
    }
}
