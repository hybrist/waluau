use crate::{Expr, Function, FunctionExpr, Program, Rebindability, Stmt};

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
    /// `export function f`.
    Export,
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

/// Authored exposure of a function declaration.
///
/// Tooling-only browser exports are a compiler option, not part of this
/// language fact. Legacy trailing returns select private declarations through
/// [`Program::module_interface`] rather than changing their exposure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FunctionExposure {
    Private,
    Exported,
}

/// Semantic facts shared by compiler phases and tooling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FunctionDeclarationFacts {
    pub binding: FunctionBindingClass,
    pub hoisted: bool,
    pub rebindability: Rebindability,
    pub exposure: FunctionExposure,
}

/// The one dependency-facing interface authored by a module.
///
/// Exported types make an empty declaration namespace require-able, while
/// exported functions add named value members. Legacy trailing returns remain
/// a separate interface and may coexist with exported types, but never with an
/// exported function declaration.
#[derive(Clone, Debug)]
pub enum ModuleInterface<'a> {
    Legacy(&'a Expr),
    Declarations { functions: Vec<&'a Function> },
    Missing,
    Conflict,
}

impl FunctionDeclarationClass {
    pub const fn facts(self) -> FunctionDeclarationFacts {
        match self {
            Self::Module => FunctionDeclarationFacts {
                binding: FunctionBindingClass::Module,
                hoisted: true,
                rebindability: Rebindability::Const,
                exposure: FunctionExposure::Private,
            },
            Self::Export => FunctionDeclarationFacts {
                binding: FunctionBindingClass::Module,
                hoisted: true,
                rebindability: Rebindability::Const,
                exposure: FunctionExposure::Exported,
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
    /// Read the canonical declaration class, checking the direct-function AST
    /// invariant in debug builds.
    pub const fn declaration_class(&self) -> FunctionDeclarationClass {
        debug_assert!(matches!(
            self.declaration_class,
            FunctionDeclarationClass::Module | FunctionDeclarationClass::Export
        ));
        self.declaration_class
    }
}

impl Program {
    /// Authored named value declarations in this module's public interface.
    pub fn exported_functions(&self) -> impl Iterator<Item = &Function> {
        self.functions.iter().filter(|function| {
            function.declaration_class().facts().exposure == FunctionExposure::Exported
        })
    }

    /// Resolve the authored module interface before either linker applies
    /// module-specific name mangling or diagnostic adaptation.
    pub fn module_interface(&self) -> ModuleInterface<'_> {
        let functions = self.exported_functions().collect::<Vec<_>>();
        if let Some(export) = self.export.as_ref() {
            return if functions.is_empty() {
                ModuleInterface::Legacy(export)
            } else {
                ModuleInterface::Conflict
            };
        }
        if !functions.is_empty()
            || self
                .type_declarations
                .iter()
                .any(|declaration| declaration.exported)
        {
            ModuleInterface::Declarations { functions }
        } else {
            ModuleInterface::Missing
        }
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
                exposure: FunctionExposure::Private,
            }
        );
        assert_eq!(
            FunctionDeclarationClass::Export.facts(),
            FunctionDeclarationFacts {
                binding: FunctionBindingClass::Module,
                hoisted: true,
                rebindability: Rebindability::Const,
                exposure: FunctionExposure::Exported,
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
            declaration_class: FunctionDeclarationClass::Module,
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

    #[test]
    fn explicit_simple_function_is_an_exported_module_declaration() {
        let function = Function {
            name: FunctionName::Simple("run".to_string()),
            declaration_class: FunctionDeclarationClass::Export,
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
            FunctionDeclarationClass::Export
        );

        let mut program = Program {
            functions: vec![function],
            declared_imports: Vec::new(),
            declared_constants: Vec::new(),
            type_declarations: Vec::new(),
            top_level: Vec::new(),
            top_level_file_paths: Vec::new(),
            export: None,
            sources: std::collections::BTreeMap::new(),
            entry_file_path: "test.walu".to_string(),
        };
        assert!(matches!(
            program.module_interface(),
            ModuleInterface::Declarations { functions } if functions.len() == 1
        ));
        program.export = Some(Expr::Name("run".to_string(), None, None));
        assert!(matches!(
            program.module_interface(),
            ModuleInterface::Conflict
        ));
    }
}
