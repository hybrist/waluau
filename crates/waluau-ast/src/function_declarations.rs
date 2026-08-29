use crate::{Expr, Function, FunctionExpr, Rebindability, Stmt};

/// The authored forms that introduce a function declaration.
///
/// Keeping this classification in the AST crate gives compiler phases and
/// editor tooling one vocabulary for the semantic distinction that already
/// exists today. It deliberately does not change either AST representation:
/// module functions remain in `Program::functions`, while lexical functions
/// remain named function expressions stored by `Stmt::Let`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FunctionDeclarationClass {
    /// `function f`, `function T.f`, or `function T:m`.
    Module,
    /// `local function f`.
    Local,
    /// `const function f`.
    Const,
}

/// The scope in which a function declaration introduces its binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FunctionBindingClass {
    Module,
    Lexical,
}

/// Current compatibility exposure of a function declaration.
///
/// This is deliberately distinct from an authored module interface. Module
/// functions are still private declarations, but editor tooling currently
/// presents them as module members and non-minimal entry builds export them
/// for debugging. Lexical declarations have no such compatibility exposure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FunctionExposure {
    Private,
    PrivateWithCompatibilityExposure,
}

/// Semantic facts shared by compiler phases and tooling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FunctionDeclarationFacts {
    pub binding: FunctionBindingClass,
    pub hoisted: bool,
    pub rebindability: Rebindability,
    pub exposure: FunctionExposure,
}

impl FunctionDeclarationClass {
    pub const fn facts(self) -> FunctionDeclarationFacts {
        match self {
            Self::Module => FunctionDeclarationFacts {
                binding: FunctionBindingClass::Module,
                hoisted: true,
                rebindability: Rebindability::Const,
                exposure: FunctionExposure::PrivateWithCompatibilityExposure,
            },
            Self::Local => FunctionDeclarationFacts {
                binding: FunctionBindingClass::Lexical,
                hoisted: false,
                rebindability: Rebindability::Rebindable,
                exposure: FunctionExposure::Private,
            },
            Self::Const => FunctionDeclarationFacts {
                binding: FunctionBindingClass::Lexical,
                hoisted: false,
                rebindability: Rebindability::Const,
                exposure: FunctionExposure::Private,
            },
        }
    }
}

/// A lexical function declaration recovered from its behavior-preserving AST
/// representation.
#[derive(Clone, Copy, Debug)]
pub struct LexicalFunctionDeclaration<'a> {
    pub name: &'a str,
    pub function: &'a FunctionExpr,
    pub class: FunctionDeclarationClass,
}

impl Function {
    /// Plain named functions are module declarations, including qualified
    /// static and method forms.
    pub const fn declaration_class(&self) -> FunctionDeclarationClass {
        FunctionDeclarationClass::Module
    }
}

impl Stmt {
    /// Classify the lexical function declaration represented by this
    /// statement. Ordinary locals containing anonymous function values are
    /// intentionally not declarations.
    pub fn lexical_function_declaration(&self) -> Option<LexicalFunctionDeclaration<'_>> {
        let Self::Let {
            name,
            rebindability,
            value: Expr::Function(function),
            ..
        } = self
        else {
            return None;
        };
        let class = function.declaration_class?;
        if function.name.as_deref() != Some(name)
            || !matches!(
                class,
                FunctionDeclarationClass::Local | FunctionDeclarationClass::Const
            )
        {
            return None;
        }
        debug_assert_eq!(class.facts().rebindability, *rebindability);
        Some(LexicalFunctionDeclaration {
            name,
            function,
            class,
        })
    }

    /// Rebindability of a single-binding declaration, using authored
    /// function-declaration metadata when present.
    pub fn declaration_rebindability(&self) -> Option<Rebindability> {
        let Self::Let { rebindability, .. } = self else {
            return None;
        };
        Some(
            self.lexical_function_declaration()
                .map(|declaration| declaration.class.facts().rebindability)
                .unwrap_or(*rebindability),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FunctionName, Span};

    fn function_expr(name: Option<&str>) -> FunctionExpr {
        FunctionExpr {
            name: name.map(str::to_string),
            declaration_class: None,
            symbol_id: None,
            implicit_self: None,
            type_params: Vec::new(),
            params: Vec::new(),
            vararg: None,
            return_type: None,
            body: Vec::new(),
            file_path: "test.walu".to_string(),
            span: Some(Span { start: 0, end: 1 }),
        }
    }

    #[test]
    fn declaration_classes_pin_the_current_semantic_matrix() {
        assert_eq!(
            FunctionDeclarationClass::Module.facts(),
            FunctionDeclarationFacts {
                binding: FunctionBindingClass::Module,
                hoisted: true,
                rebindability: Rebindability::Const,
                exposure: FunctionExposure::PrivateWithCompatibilityExposure,
            }
        );
        assert_eq!(
            FunctionDeclarationClass::Local.facts(),
            FunctionDeclarationFacts {
                binding: FunctionBindingClass::Lexical,
                hoisted: false,
                rebindability: Rebindability::Rebindable,
                exposure: FunctionExposure::Private,
            }
        );
        assert_eq!(
            FunctionDeclarationClass::Const.facts(),
            FunctionDeclarationFacts {
                binding: FunctionBindingClass::Lexical,
                hoisted: false,
                rebindability: Rebindability::Const,
                exposure: FunctionExposure::Private,
            }
        );
    }

    #[test]
    fn lexical_classifier_rejects_anonymous_function_values() {
        let stmt = Stmt::Let {
            name: "f".to_string(),
            symbol_id: None,
            rebindability: Rebindability::Rebindable,
            ty: None,
            value: Expr::Function(function_expr(None)),
        };
        assert!(stmt.lexical_function_declaration().is_none());
    }

    #[test]
    fn lexical_classifier_rejects_explicitly_named_function_values() {
        let stmt = Stmt::Let {
            name: "f".to_string(),
            symbol_id: None,
            rebindability: Rebindability::Rebindable,
            ty: None,
            value: Expr::Function(function_expr(Some("f"))),
        };
        assert!(stmt.lexical_function_declaration().is_none());
    }

    #[test]
    fn qualified_named_functions_are_module_declarations() {
        let function = Function {
            name: FunctionName::Method {
                table: "State".to_string(),
                method: "step".to_string(),
            },
            symbol_id: None,
            type_params: Vec::new(),
            params: Vec::new(),
            vararg: None,
            return_type: None,
            body: Vec::new(),
            file_path: "test.walu".to_string(),
            span: None,
        };
        assert_eq!(
            function.declaration_class(),
            FunctionDeclarationClass::Module
        );
    }
}
