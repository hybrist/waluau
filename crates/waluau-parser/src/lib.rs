use waluau_ast::{FunctionDeclarationClass, Program, Span, Type};
use waluau_diagnostics::Diagnostic;

mod parser;

/// Result of parsing with error recovery: a best-effort (possibly partial)
/// program plus every diagnostic encountered. The program is complete exactly
/// when `diagnostics` contains no errors.
#[derive(Debug)]
pub struct ParseOutcome {
    pub program: Program,
    pub diagnostics: Vec<Diagnostic>,
    /// Every name-introducing site in the file, in source order, with exact
    /// identifier-token spans and lexical visibility ranges. This side table
    /// exists for editor tooling (hover, go-to-definition, completion): the
    /// AST does not carry definition-site spans, and threading them through
    /// every later compiler stage would be invasive for no compiler benefit.
    pub definitions: Vec<DefinitionSite>,
    /// Named types referenced by source syntax, with exact spans. Keeping
    /// this parser fact separate from the span-free semantic [`Type`] lets
    /// editor tooling distinguish type and value namespaces even inside
    /// grouped, array, generic, and function type expressions.
    pub type_references: Vec<TypeReference>,
}

/// One source-level named type reference recorded while parsing.
#[derive(Clone, Debug, PartialEq)]
pub struct TypeReference {
    /// The source spelling, including a module qualifier when present.
    pub name: String,
    /// Exact span of the bare or dotted name, excluding type arguments.
    pub span: Span,
}

/// What kind of binding a [`DefinitionSite`] introduces.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DefinitionKind {
    /// `local x` / `const x` / a `local` multi-binding.
    Local,
    /// A function parameter (including the synthetic method `self`).
    Param,
    /// A top-level `function f`, `function T.f`, or `function T:m`, plus
    /// `local function f` / `const function f` bindings.
    Function,
    /// A numeric-for or for-in loop variable.
    LoopVar,
    /// The narrowed binding of an if-cast clause (`if T(v) = x then`).
    IfCastBinding,
    /// A `declare function` host import (name may be dotted, e.g. `math.abs`).
    DeclaredFunction,
    /// A `declare const ns.name` compile-time constant.
    DeclaredConstant,
    /// A `declare property T:name` host property (recorded as `T.name`).
    Property,
    /// A `type Name = ...` declaration.
    TypeName,
    /// A nominal enum member, recorded as `Enum.variant` for tooling.
    EnumVariant,
}

/// One name-introducing site recorded by the parser.
///
/// A reference at byte offset `o` can see this binding exactly when
/// `visible_from <= o < scope_end`; among visible same-name candidates the
/// one with the greatest `visible_from` shadows the rest. `visible_from` is
/// the end of the introducing statement for plain locals (so `local x = x`
/// keeps the initializer resolving to the outer `x`) and the start of the
/// scoped body for loop variables, parameters, and function-name recursion
/// bindings. File-scoped names use `0..u32::MAX`.
#[derive(Clone, Debug, PartialEq)]
pub struct DefinitionSite {
    /// The name as referenced: a bare identifier for locals/params/loop vars,
    /// the dotted or `T:m` display name for namespaced functions and declares.
    pub name: String,
    /// Exact span of the defining identifier token(s).
    pub name_span: Span,
    pub kind: DefinitionKind,
    /// Canonical function-declaration semantics. `None` for non-function
    /// definitions, including declared host imports.
    pub function_declaration: Option<FunctionDeclarationClass>,
    /// Whether this definition is authored into its module's public interface.
    /// This covers explicit function, type, and enum exports; legacy value
    /// interfaces continue to use the module's trailing return.
    pub exported: bool,
    /// Declaration-order variants when this definition names an enum.
    pub enum_variants: Option<Vec<String>>,
    /// The annotated type (locals/params/declares), when syntactically
    /// present. Functions carry their rendered signature in `detail` instead.
    pub ty: Option<Type>,
    /// Pre-rendered signature for function-like definitions, e.g.
    /// `function add(a: i32, b: i32): i32`.
    pub detail: Option<String>,
    /// Byte offset from which references resolve to this binding.
    pub visible_from: u32,
    /// Byte offset at which the binding's scope closes (`u32::MAX` = file).
    pub scope_end: u32,
    /// For `local m = require("./mod")` bindings: the raw require path, so
    /// tooling can resolve member accesses on `m` into the target module.
    pub require_path: Option<String>,
    /// The syntactic shape of a local binding's initializer, when it is one
    /// tooling can chase statically (a call, a member read, an index). This
    /// lets an editor derive the type of `local state = game.new()` from
    /// `new`'s declared return type without running type inference.
    pub initializer: Option<InitializerHint>,
}

/// The statically chaseable shape of a local's initializer expression.
///
/// Every `base`/`callee`/`receiver` is a dotted path rooted at a name — `f`,
/// `ns.f`, `m.State.new` — so a value reached through a module alias chases
/// as readily as a local one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InitializerHint {
    /// `local x = f(...)`, `local x = ns.f(...)`, `local x = m.T.new(...)`.
    Call { callee: String },
    /// `local x = recv:m(...)`.
    MethodCall { receiver: String, method: String },
    /// `local x = base.field` (`indexed` for `local x = base.field[i]`).
    Field {
        base: String,
        field: String,
        indexed: bool,
    },
    /// `local x = base[i]`.
    Index { base: String },
}

pub fn parse(source: &str) -> Result<Program, Diagnostic> {
    parse_with_path(source, "source")
}

pub fn parse_with_path(source: &str, file_path: &str) -> Result<Program, Diagnostic> {
    let ParseOutcome {
        program,
        mut diagnostics,
        ..
    } = parse_with_recovery(source, file_path);
    match diagnostics.len() {
        0 => Ok(program),
        1 => Err(diagnostics.remove(0)),
        // Join with the legacy `message at start..end` rendering so existing
        // string-based consumers keep seeing the format they parse today.
        _ => Err(Diagnostic::new(
            diagnostics
                .iter()
                .map(Diagnostic::render_for_playground)
                .collect::<Vec<_>>()
                .join("\n"),
        )),
    }
}

/// Parse with statement-boundary error recovery. Always produces a program;
/// diagnostics carry structural spans resolved to line/column against
/// `source` so they can drive editor markers directly.
pub fn parse_with_recovery(source: &str, file_path: &str) -> ParseOutcome {
    let tokens = match waluau_lexer::lex(source) {
        Ok(tokens) => tokens,
        Err(error) => {
            return ParseOutcome {
                program: empty_program(source, file_path),
                diagnostics: vec![error.with_source(file_path, source)],
                definitions: Vec::new(),
                type_references: Vec::new(),
            };
        }
    };
    let (program, diagnostics, definitions, type_references) =
        parser::Parser::new(tokens, file_path.to_string()).parse_program(source);
    ParseOutcome {
        program,
        diagnostics: diagnostics
            .into_iter()
            .map(|diagnostic| diagnostic.with_source(file_path, source))
            .collect(),
        definitions,
        type_references,
    }
}

fn empty_program(source: &str, file_path: &str) -> Program {
    Program {
        functions: Vec::new(),
        declared_imports: Vec::new(),
        declared_constants: Vec::new(),
        type_declarations: Vec::new(),
        top_level: Vec::new(),
        top_level_file_paths: Vec::new(),
        export: None,
        sources: std::collections::BTreeMap::from([(file_path.to_string(), source.to_string())]),
        entry_file_path: file_path.to_string(),
    }
}

#[cfg(test)]
mod tests;
