//! Position-based language features: hover, go-to-definition, completion.
//!
//! Everything here works from a single-module parse of the queried document
//! plus the parser's [`DefinitionSite`] side table — no linked program and no
//! type inference. The cursor target is classified from the token stream
//! (which also keeps comments and string interiors inert), then resolved
//! positionally against the side table: a reference at offset `o` sees the
//! visible same-name definition with the greatest `visible_from`. Member
//! accesses resolve through builtin-namespace declares (`math.*` from the
//! builtins prelude), HIR intrinsic names (`string.*`, `table.*`, ...), and
//! `require`d modules loaded through the caller-supplied file loader.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use waluau_ast::{Span, Type};
use waluau_lexer::{Token, TokenKind};
use waluau_parser::{DefinitionKind, DefinitionSite};

/// Reads a file's current text (open-buffer contents first, then disk).
/// Returns `None` when the file cannot be read — e.g. in the wasm build,
/// where only open documents exist.
pub type Loader<'a> = &'a dyn Fn(&Path) -> Option<String>;

pub struct Hover {
    /// Markdown contents (a fenced code block plus optional prose).
    pub contents: String,
    /// The reference the hover applies to, for editor highlighting.
    pub span: Span,
}

pub struct CompletionItem {
    pub label: String,
    /// LSP `CompletionItemKind` constant.
    pub kind: i64,
    pub detail: Option<String>,
}

mod completion_kind {
    pub const FUNCTION: i64 = 3;
    pub const FIELD: i64 = 5;
    pub const VARIABLE: i64 = 6;
    pub const CLASS: i64 = 7;
    pub const MODULE: i64 = 9;
    pub const PROPERTY: i64 = 10;
    pub const KEYWORD: i64 = 14;
    pub const CONSTANT: i64 = 21;
}

/// Namespaced builtins implemented as HIR/IR intrinsics rather than prelude
/// declares; they have no [`DefinitionSite`], so completion lists them from
/// this table. Hover shows only the name (their signatures live in checker
/// code, not in any declaration).
const INTRINSIC_MEMBERS: &[&str] = &[
    "bit32.band",
    "bit32.bnot",
    "bit32.bor",
    "bit32.btest",
    "bit32.bxor",
    "bit32.countlz",
    "bit32.countrz",
    "bit32.lrotate",
    "bit32.rrotate",
    "coroutine.close",
    "coroutine.create",
    "coroutine.resume",
    "coroutine.yield",
    "promise.await",
    "string.byte",
    "string.char",
    "string.find",
    "string.format",
    "string.gmatch",
    "string.gsub",
    "string.len",
    "string.lower",
    "string.match",
    "string.rep",
    "string.reverse",
    "string.split",
    "string.sub",
    "string.upper",
    "table.concat",
    "table.create",
    "table.getn",
    "table.insert",
    "table.pack",
    "table.remove",
    "table.sort",
    "table.unpack",
];

/// Globals the resolver pre-declares that have no prelude declaration of
/// their own (namespaces and special forms).
const GLOBAL_NAMES: &[&str] = &[
    "assert",
    "bit32",
    "coroutine",
    "error",
    "math",
    "pcall",
    "print",
    "promise",
    "select",
    "string",
    "table",
    "tostring",
];

const KEYWORDS: &[&str] = &[
    "and", "break", "const", "continue", "do", "else", "elseif", "end", "false", "for", "function",
    "if", "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "type", "until",
    "while",
];

const PRIMITIVE_TYPE_NAMES: &[&str] = &[
    "bool", "bytes", "extern", "f32", "f64", "i32", "i64", "number", "string", "thread", "u32",
    "u64", "unit", "unknown",
];

/// Bare (un-namespaced) prelude declares that user code calls directly.
/// Everything else bare in the prelude is host-bridge plumbing (`pm_find`,
/// `string_format17`, ...) behind intrinsics like `string.format`.
const PUBLIC_BARE_BUILTINS: &[&str] = &["print", "tonumber", "tostring", "type", "typeof"];

/// Prelude declares hidden from completion: host-internal plumbing that user
/// code never calls directly.
fn is_internal_name(name: &str) -> bool {
    if !name.contains('.') && !name.contains(':') {
        return !PUBLIC_BARE_BUILTINS.contains(&name);
    }
    name.starts_with("pm_")
        || name.starts_with("dom_")
        || name.starts_with("js_")
        || name.starts_with("__")
}

/// Definitions from the builtin declaration files that ship inside the
/// compiler (`builtins/core.walu`, `builtins/math.walu`).
fn prelude_definitions() -> &'static [DefinitionSite] {
    static PRELUDE: OnceLock<Vec<DefinitionSite>> = OnceLock::new();
    PRELUDE.get_or_init(|| {
        let mut definitions = Vec::new();
        for (name, source) in [
            (
                "builtin:core.walu",
                include_str!("../../../builtins/core.walu"),
            ),
            (
                "builtin:math.walu",
                include_str!("../../../builtins/math.walu"),
            ),
        ] {
            definitions.extend(waluau_parser::parse_with_recovery(source, name).definitions);
        }
        definitions
    })
}

/// The analyzed shape of one document.
struct DocumentIndex {
    tokens: Vec<Token>,
    definitions: Vec<DefinitionSite>,
}

fn index_document(text: &str, path: &Path) -> DocumentIndex {
    let tokens = waluau_lexer::lex(text).unwrap_or_default();
    let definitions = waluau_parser::parse_with_recovery(text, &path.to_string_lossy()).definitions;
    DocumentIndex {
        tokens,
        definitions,
    }
}

/// The innermost visible same-name definition at `offset`.
fn resolve_name<'a>(
    definitions: &'a [DefinitionSite],
    name: &str,
    offset: u32,
) -> Option<&'a DefinitionSite> {
    definitions
        .iter()
        .filter(|definition| {
            definition.name == name
                && definition.visible_from <= offset
                && offset < definition.scope_end
        })
        .max_by_key(|definition| definition.visible_from)
}

/// A namespaced definition (`ns.member` or `ns:member`) from the file or the
/// prelude; file definitions win.
fn resolve_member<'a>(
    file_definitions: &'a [DefinitionSite],
    namespace: &str,
    member: &str,
) -> Option<&'a DefinitionSite> {
    let dotted = format!("{namespace}.{member}");
    let method = format!("{namespace}:{member}");
    file_definitions
        .iter()
        .chain(prelude_definitions())
        .find(|definition| definition.name == dotted || definition.name == method)
}

/// One postfix step of a receiver expression, in source order.
#[derive(Clone, Debug)]
enum Access {
    /// `.name`
    Field(String),
    /// `:name(...)` — a method call; its argument list folds into this step.
    Method(String),
    /// `(...)` applied to whatever precedes it.
    Call,
    /// `[...]`
    Index,
}

/// The receiver of a member access, as written: a root identifier plus the
/// postfix steps applied to it. `m.P.new()` has root `m` and the steps
/// `.P`, `.new`, `()`.
#[derive(Clone, Debug)]
struct Receiver {
    root: String,
    accesses: Vec<Access>,
}

impl Receiver {
    /// The name written immediately before the separator — what a member
    /// access is qualified by in the source text. `None` when the receiver
    /// ends in a call or an index, which name nothing.
    fn qualifier(&self) -> Option<&str> {
        match self.accesses.last() {
            None => Some(&self.root),
            Some(Access::Field(name) | Access::Method(name)) => Some(name),
            Some(Access::Call | Access::Index) => None,
        }
    }
}

/// What the cursor is on, classified from the token stream.
enum RefTarget {
    /// A definition site's own name.
    Definition(usize),
    /// A plain identifier reference.
    Name { name: String, span: Span },
    /// The member part of `receiver.member` or `receiver:member`.
    Member {
        receiver: Receiver,
        name: String,
        span: Span,
    },
    /// The string argument of a `require("...")` call.
    Require { raw: String, span: Span },
}

/// The index of the opening delimiter matching the closer at `close`,
/// counting nesting of that same delimiter pair only (a `[` inside parens
/// does not disturb the paren depth).
fn matching_open(
    tokens: &[Token],
    close: usize,
    open_kind: &TokenKind,
    close_kind: &TokenKind,
) -> Option<usize> {
    let mut depth = 0usize;
    for at in (0..=close).rev() {
        if tokens[at].kind == *close_kind {
            depth += 1;
        } else if tokens[at].kind == *open_kind {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(at);
            }
        }
    }
    None
}

/// Walk backwards from `end` — the last token of a receiver expression —
/// over postfix steps down to the root identifier. A receiver need not be a
/// bare name: `P.new()`, `items[1]` and `p:to_q()` are all chaseable. Any
/// other token (a literal, an operator, a keyword) means the receiver is not
/// a static path and there is nothing to follow.
fn receiver_before(tokens: &[Token], end: usize) -> Option<Receiver> {
    let mut accesses = Vec::new();
    let mut at = end;
    loop {
        match &tokens[at].kind {
            TokenKind::RParen => {
                accesses.push(Access::Call);
                at = matching_open(tokens, at, &TokenKind::LParen, &TokenKind::RParen)?;
            }
            TokenKind::RBracket => {
                accesses.push(Access::Index);
                at = matching_open(tokens, at, &TokenKind::LBracket, &TokenKind::RBracket)?;
            }
            TokenKind::Identifier(name) => {
                match at.checked_sub(1).map(|before| &tokens[before].kind) {
                    Some(TokenKind::Dot) => accesses.push(Access::Field(name.clone())),
                    Some(TokenKind::Colon) => accesses.push(Access::Method(name.clone())),
                    // Nothing qualifies this identifier: it is the root.
                    _ => {
                        accesses.reverse();
                        // `recv:m(...)` scans as a `Call` and then a
                        // `Method`; the call belongs to the method.
                        accesses.dedup_by(|next, previous| {
                            matches!(next, Access::Call) && matches!(previous, Access::Method(_))
                        });
                        return Some(Receiver {
                            root: name.clone(),
                            accesses,
                        });
                    }
                }
                // Step over the identifier onto its separator.
                at = at.checked_sub(1)?;
            }
            _ => return None,
        }
        at = at.checked_sub(1)?;
    }
}

/// The reference at `offset`, also accepting a cursor sitting immediately
/// after the word (editors report the caret position for definition
/// requests, which is at the word's end after a click on its last letter).
fn find_target(index: &DocumentIndex, offset: u32) -> Option<RefTarget> {
    find_target_at(index, offset).or_else(|| {
        offset
            .checked_sub(1)
            .and_then(|before| find_target_at(index, before))
    })
}

fn find_target_at(index: &DocumentIndex, offset: u32) -> Option<RefTarget> {
    // Definition sites take precedence: their spans are identifier tokens
    // (or dotted token runs) and cannot overlap a distinct reference.
    if let Some(position) = index.definitions.iter().position(|definition| {
        definition.name_span.start <= offset && offset < definition.name_span.end
    }) {
        return Some(RefTarget::Definition(position));
    }

    let at = index
        .tokens
        .iter()
        .position(|token| token.span.start <= offset && offset < token.span.end)?;
    let token = &index.tokens[at];
    match &token.kind {
        TokenKind::Identifier(name) => {
            // `receiver.name` / `receiver:name` — the member part of a
            // namespaced or method reference.
            if at >= 2
                && matches!(index.tokens[at - 1].kind, TokenKind::Dot | TokenKind::Colon)
                && let Some(receiver) = receiver_before(&index.tokens, at - 2)
            {
                return Some(RefTarget::Member {
                    receiver,
                    name: name.clone(),
                    span: token.span,
                });
            }
            Some(RefTarget::Name {
                name: name.clone(),
                span: token.span,
            })
        }
        TokenKind::Str(raw) => {
            // `require("./mod")` — jump to the module file itself.
            if at >= 2
                && matches!(index.tokens[at - 1].kind, TokenKind::LParen)
                && matches!(&index.tokens[at - 2].kind, TokenKind::Identifier(name) if name == "require")
            {
                return Some(RefTarget::Require {
                    raw: raw.clone(),
                    span: token.span,
                });
            }
            None
        }
        _ => None,
    }
}

/// Resolve a `require` path the way the module linker does: relative to the
/// requiring file, defaulting the `.walu` extension. Virtual modules
/// (`waluau:engine`, `@waluau/dom`, ...) have no source file and return
/// `None`. The result is normalized lexically (not canonicalized) so it
/// matches open-document keys in the wasm build, where the virtual
/// filesystem has no `canonicalize`.
fn resolve_require_path(current_file: &Path, raw: &str) -> Option<PathBuf> {
    if !(raw.starts_with("./") || raw.starts_with("../")) {
        return None;
    }
    let dir = current_file.parent()?;
    let mut candidate = normalize_lexically(&dir.join(raw));
    if candidate.extension().is_none() {
        candidate.set_extension("walu");
    }
    Some(candidate)
}

/// Remove `.` components and fold `..` into their parent, without touching
/// the filesystem.
fn normalize_lexically(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut parts: Vec<Component> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir if matches!(parts.last(), Some(Component::Normal(_))) => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.iter().collect()
}

/// A resolved hover/definition target.
enum Resolved {
    /// A definition in the queried file.
    File(DefinitionSite),
    /// A definition from the builtins prelude (no navigable location).
    Prelude(DefinitionSite),
    /// A definition in another module of the workspace.
    Module { file: PathBuf, def: DefinitionSite },
    /// Hover-only information with no definition site.
    Info(String),
    /// A record field of a named type: hover text plus the declaring type
    /// declaration as the navigation target.
    MemberOfType {
        summary: String,
        declared_by: Box<Resolved>,
    },
}

/// Exported members of a required module: its file-visible functions.
fn module_exports(file: &Path, load: Loader) -> Vec<DefinitionSite> {
    let Some(scope) = module_scope(file, load) else {
        return Vec::new();
    };
    scope
        .definitions
        .iter()
        .filter(|definition| {
            definition.kind == DefinitionKind::Function
                && !definition.name.contains('.')
                && !definition.name.contains(':')
        })
        .cloned()
        .collect()
}

/// How deep the static-type engine chases initializer/type-name chains
/// before giving up (guards against pathological or cyclic definitions).
const TYPE_CHASE_DEPTH: u8 = 12;

/// The document whose definition table a name resolves in.
#[derive(Clone)]
struct TypeScope {
    file: PathBuf,
    /// True for the builtins prelude: definitions with no navigable file.
    prelude: bool,
    definitions: std::rc::Rc<Vec<DefinitionSite>>,
}

impl TypeScope {
    fn current(index: &DocumentIndex, path: &Path) -> Self {
        Self {
            file: path.to_path_buf(),
            prelude: false,
            definitions: std::rc::Rc::new(index.definitions.clone()),
        }
    }

    fn prelude() -> Self {
        Self {
            file: PathBuf::from("builtin:prelude"),
            prelude: true,
            definitions: std::rc::Rc::new(prelude_definitions().to_vec()),
        }
    }
}

fn module_scope(file: &Path, load: Loader) -> Option<TypeScope> {
    let text = load(file)?;
    let definitions =
        waluau_parser::parse_with_recovery(&text, &file.to_string_lossy()).definitions;
    Some(TypeScope {
        file: file.to_path_buf(),
        prelude: false,
        definitions: std::rc::Rc::new(definitions),
    })
}

/// A definition found in some scope, mapped to a [`Resolved`] target against
/// the queried document.
fn resolved_in_scope(def: &DefinitionSite, scope: &TypeScope, current: &Path) -> Resolved {
    if scope.prelude {
        Resolved::Prelude(def.clone())
    } else if scope.file == current {
        Resolved::File(def.clone())
    } else {
        Resolved::Module {
            file: scope.file.clone(),
            def: def.clone(),
        }
    }
}

/// A record type's fields plus the type declaration that provided them (for
/// go-to-definition on field names).
struct RecordInfo {
    fields: std::collections::BTreeMap<String, Type>,
    declared_by: Option<(DefinitionSite, TypeScope)>,
}

/// Split a possibly module-qualified type name (`game.State`) into the local
/// type name and the scope it resolves in.
fn type_name_scope(name: &str, scope: &TypeScope, load: Loader) -> Option<(String, TypeScope)> {
    if let Some((alias, member)) = name.split_once('.') {
        let alias_def = scope
            .definitions
            .iter()
            .find(|definition| definition.name == alias && definition.require_path.is_some())?;
        let raw = alias_def.require_path.as_ref()?;
        let file = resolve_require_path(&scope.file, raw)?;
        let module = module_scope(&file, load)?;
        Some((member.to_string(), module))
    } else {
        Some((name.to_string(), scope.clone()))
    }
}

/// Follow a type to its record shape, resolving `Named` types through type
/// declarations (possibly in a required module).
fn resolve_type_to_record(
    ty: &Type,
    scope: &TypeScope,
    load: Loader,
    depth: u8,
) -> Option<RecordInfo> {
    if depth == 0 {
        return None;
    }
    match ty {
        Type::Record(fields) => Some(RecordInfo {
            fields: fields.clone(),
            declared_by: None,
        }),
        Type::Opaque { ty: inner, .. } | Type::Nullable(inner) | Type::Readonly(inner) => {
            resolve_type_to_record(inner, scope, load, depth - 1)
        }
        Type::Named { name, type_args } if name == "readonly" && type_args.len() == 1 => {
            resolve_type_to_record(&type_args[0], scope, load, depth - 1)
        }
        Type::Named { name, .. } => {
            let (type_name, type_scope) = type_name_scope(name, scope, load)?;
            let type_def = type_scope
                .definitions
                .iter()
                .find(|definition| {
                    definition.kind == DefinitionKind::TypeName && definition.name == type_name
                })?
                .clone();
            let inner = type_def.ty.clone()?;
            let mut info = resolve_type_to_record(&inner, &type_scope, load, depth - 1)?;
            if info.declared_by.is_none() {
                info.declared_by = Some((type_def, type_scope));
            }
            Some(info)
        }
        _ => None,
    }
}

/// A method-style member (`T:m` / `T.m`) of a named type, looked up in the
/// scope the type is declared in.
fn named_type_member(
    ty: &Type,
    member: &str,
    scope: &TypeScope,
    load: Loader,
) -> Option<(DefinitionSite, TypeScope)> {
    if let Type::Readonly(inner) = ty {
        return named_type_member(inner, member, scope, load);
    }
    if let Type::Named { name, type_args } = ty
        && name == "readonly"
        && type_args.len() == 1
    {
        return named_type_member(&type_args[0], member, scope, load);
    }
    let Type::Named { name, .. } = ty else {
        return None;
    };
    let (type_name, type_scope) = type_name_scope(name, scope, load)?;
    let method = format!("{type_name}:{member}");
    let dotted = format!("{type_name}.{member}");
    let def = type_scope
        .definitions
        .iter()
        .find(|definition| definition.name == method || definition.name == dotted)?
        .clone();
    Some((def, type_scope))
}

/// A parser initializer hint restated as a receiver path, so an initializer
/// and a written member access chase through exactly the same code.
fn initializer_receiver(hint: &waluau_parser::InitializerHint) -> Receiver {
    use waluau_parser::InitializerHint;
    let path_receiver = |path: &str| {
        let mut parts = path.split('.');
        let root = parts.next().unwrap_or_default().to_string();
        let accesses = parts
            .map(|part| Access::Field(part.to_string()))
            .collect::<Vec<_>>();
        Receiver { root, accesses }
    };
    match hint {
        InitializerHint::Call { callee } => {
            let mut receiver = path_receiver(callee);
            receiver.accesses.push(Access::Call);
            receiver
        }
        InitializerHint::MethodCall {
            receiver: base,
            method,
        } => {
            let mut receiver = path_receiver(base);
            receiver.accesses.push(Access::Method(method.clone()));
            receiver
        }
        InitializerHint::Field {
            base,
            field,
            indexed,
        } => {
            let mut receiver = path_receiver(base);
            receiver.accesses.push(Access::Field(field.clone()));
            if *indexed {
                receiver.accesses.push(Access::Index);
            }
            receiver
        }
        InitializerHint::Index { base } => {
            let mut receiver = path_receiver(base);
            receiver.accesses.push(Access::Index);
            receiver
        }
    }
}

/// The static type of a definition's value: its annotation when present,
/// otherwise the chased shape of its initializer (`local state = game.new()`
/// gets `new`'s declared return type). Returns the type plus the scope its
/// type names resolve in.
fn definition_static_type(
    def: &DefinitionSite,
    scope: &TypeScope,
    load: Loader,
    depth: u8,
) -> Option<(Type, TypeScope)> {
    if depth == 0 {
        return None;
    }
    if let Some(ty) = &def.ty {
        return Some((ty.clone(), scope.clone()));
    }
    let receiver = initializer_receiver(def.initializer.as_ref()?);
    match receiver_standing(&receiver, def.visible_from, scope, load, depth - 1)? {
        Standing::Value(ty, type_scope) => Some((ty, type_scope)),
        Standing::Module(_) | Standing::Namespace(..) => None,
    }
}

/// What a receiver expression denotes once its path has been followed.
enum Standing {
    /// A value whose static type is known, plus the scope its type names
    /// resolve in.
    Value(Type, TypeScope),
    /// A `require`d module: its members are that file's own definitions.
    Module(TypeScope),
    /// A name prefix inside a scope — a type or a builtin namespace — whose
    /// members are the scope's `ns.member` / `ns:member` definitions.
    Namespace(String, TypeScope),
}

/// A `type Name = ...` declaration binds a namespace, not a value; receiver
/// roots and module members must not mistake one for a variable.
fn binds_a_value(definition: &DefinitionSite) -> bool {
    definition.kind != DefinitionKind::TypeName
}

/// Classify the root identifier of a receiver path: a `require` alias stands
/// for its module, a typed binding for its value, and anything else — a type
/// name, a builtin namespace, a table that methods hang off — for a namespace
/// of the same name.
fn root_standing(root: &str, offset: u32, scope: &TypeScope, load: Loader, depth: u8) -> Standing {
    if let Some(definition) = resolve_name(&scope.definitions, root, offset)
        && binds_a_value(definition)
    {
        if let Some(raw) = &definition.require_path
            && let Some(file) = resolve_require_path(&scope.file, raw)
            && let Some(module) = module_scope(&file, load)
        {
            return Standing::Module(module);
        }
        if let Some((ty, type_scope)) = definition_static_type(definition, scope, load, depth) {
            return Standing::Value(ty, type_scope);
        }
    } else if let Some(ty) = prelude_definitions()
        .iter()
        .find(|definition| definition.name == root && binds_a_value(definition))
        .and_then(|definition| definition.ty.clone())
    {
        // A bare prelude declare as the root (`tostring(x)`).
        return Standing::Value(ty, TypeScope::prelude());
    }
    Standing::Namespace(root.to_string(), scope.clone())
}

/// The definition a `.name` / `:name` step names, plus the scope it lives in.
fn member_definition(
    standing: &Standing,
    name: &str,
    load: Loader,
) -> Option<(DefinitionSite, TypeScope)> {
    match standing {
        Standing::Module(module) => {
            let definition = module
                .definitions
                .iter()
                .find(|definition| definition.name == name && binds_a_value(definition))?;
            Some((definition.clone(), module.clone()))
        }
        Standing::Namespace(namespace, scope) => {
            let dotted = format!("{namespace}.{name}");
            let method = format!("{namespace}:{name}");
            let matches = |definition: &&DefinitionSite| {
                definition.name == dotted || definition.name == method
            };
            if let Some(definition) = scope.definitions.iter().find(matches) {
                return Some((definition.clone(), scope.clone()));
            }
            let prelude = TypeScope::prelude();
            let definition = prelude.definitions.iter().find(matches)?.clone();
            Some((definition, prelude))
        }
        Standing::Value(ty, scope) => named_type_member(ty, name, scope, load),
    }
}

/// A module member that names a namespace rather than a value: a type
/// declaration, or a name that exists only as the `name.member` prefix of
/// other definitions. This is what lets `m.P.new()` walk from the module to
/// the type `P` and on to `P.new`.
fn namespace_step(standing: &Standing, name: &str) -> Option<Standing> {
    let Standing::Module(module) = standing else {
        return None;
    };
    let dotted = format!("{name}.");
    let method = format!("{name}:");
    let is_namespace = module.definitions.iter().any(|definition| {
        (definition.kind == DefinitionKind::TypeName && definition.name == name)
            || definition.name.starts_with(&dotted)
            || definition.name.starts_with(&method)
    });
    is_namespace.then(|| Standing::Namespace(name.to_string(), module.clone()))
}

/// Follow one postfix step of a receiver path.
fn apply_access(standing: Standing, access: &Access, load: Loader, depth: u8) -> Option<Standing> {
    match access {
        Access::Field(name) => {
            if let Some((definition, definition_scope)) = member_definition(&standing, name, load)
                && let Some(ty) = definition.ty
            {
                return Some(Standing::Value(ty, definition_scope));
            }
            if let Some(next) = namespace_step(&standing, name) {
                return Some(next);
            }
            // A record field of a typed value.
            let Standing::Value(ty, scope) = &standing else {
                return None;
            };
            let record = resolve_type_to_record(ty, scope, load, depth)?;
            let field_ty = record.fields.get(name)?.clone();
            let field_scope = record
                .declared_by
                .map(|(_, declaring)| declaring)
                .unwrap_or_else(|| scope.clone());
            Some(Standing::Value(field_ty, field_scope))
        }
        Access::Method(name) => {
            let (definition, definition_scope) = member_definition(&standing, name, load)?;
            let Some(Type::Function { return_type, .. }) = definition.ty else {
                return None;
            };
            Some(Standing::Value(*return_type, definition_scope))
        }
        Access::Call => {
            let Standing::Value(Type::Function { return_type, .. }, scope) = standing else {
                return None;
            };
            Some(Standing::Value(*return_type, scope))
        }
        Access::Index => {
            let Standing::Value(ty, scope) = standing else {
                return None;
            };
            Some(Standing::Value(ty.element_type()?, scope))
        }
    }
}

/// Follow a whole receiver path to what it denotes, giving up as soon as a
/// step cannot be chased statically. `depth` bounds the mutual recursion
/// through [`definition_static_type`] on cyclic or pathological sources.
fn receiver_standing(
    receiver: &Receiver,
    offset: u32,
    scope: &TypeScope,
    load: Loader,
    depth: u8,
) -> Option<Standing> {
    if depth == 0 {
        return None;
    }
    let mut standing = root_standing(&receiver.root, offset, scope, load, depth);
    for access in &receiver.accesses {
        standing = apply_access(standing, access, load, depth)?;
    }
    Some(standing)
}

fn resolve_target(
    index: &DocumentIndex,
    target: &RefTarget,
    path: &Path,
    offset: u32,
    load: Loader,
) -> Option<Resolved> {
    match target {
        RefTarget::Definition(position) => {
            Some(Resolved::File(index.definitions[*position].clone()))
        }
        RefTarget::Name { name, span: _ } => {
            if let Some(definition) = resolve_name(&index.definitions, name, offset) {
                return Some(Resolved::File(definition.clone()));
            }
            if let Some(definition) = prelude_definitions()
                .iter()
                .find(|definition| &definition.name == name)
            {
                return Some(Resolved::Prelude(definition.clone()));
            }
            // A namespace root (`math`, `string`, ...): recognized when any
            // known member lives under it.
            let dotted_prefix = format!("{name}.");
            let is_namespace = index
                .definitions
                .iter()
                .chain(prelude_definitions())
                .map(|definition| definition.name.as_str())
                .chain(INTRINSIC_MEMBERS.iter().copied())
                .any(|known| known.starts_with(&dotted_prefix));
            is_namespace.then(|| Resolved::Info(format!("(builtin namespace) {name}")))
        }
        RefTarget::Member { receiver, name, .. } => {
            // Follow the receiver path — a name, a require alias, a call
            // chain — and look the member up in whatever it lands on.
            // Methods resolve as `T:name`/`T.name` in the type's declaring
            // module; fields resolve through the type's record shape.
            let scope = TypeScope::current(index, path);
            if let Some(standing) =
                receiver_standing(receiver, offset, &scope, load, TYPE_CHASE_DEPTH)
            {
                if let Some((definition, definition_scope)) =
                    member_definition(&standing, name, load)
                {
                    return Some(resolved_in_scope(&definition, &definition_scope, path));
                }
                if let Standing::Value(base_ty, base_scope) = &standing {
                    if let Some(record) =
                        resolve_type_to_record(base_ty, base_scope, load, TYPE_CHASE_DEPTH)
                        && let Some(field_ty) = record.fields.get(name)
                    {
                        // Fields whose type the table literal doesn't reveal
                        // are shown without the unhelpful `: unknown`.
                        let qualified = match receiver.qualifier() {
                            Some(base) => format!("{base}.{name}"),
                            None => name.clone(),
                        };
                        let summary = if matches!(field_ty, Type::Unknown) {
                            format!("(field) {qualified}")
                        } else {
                            format!("{name}: {field_ty}")
                        };
                        return Some(match record.declared_by {
                            Some((type_def, type_scope)) => Resolved::MemberOfType {
                                summary,
                                declared_by: Box::new(resolved_in_scope(
                                    &type_def,
                                    &type_scope,
                                    path,
                                )),
                            },
                            None => Resolved::Info(summary),
                        });
                    }
                    // A method on a string value (`s:upper()`).
                    if matches!(base_ty, Type::String)
                        && INTRINSIC_MEMBERS.contains(&format!("string.{name}").as_str())
                    {
                        return Some(Resolved::Info(format!("(builtin) string.{name}")));
                    }
                }
            }
            // The path could not be typed. Fall back to the name written
            // right before the separator: a namespaced definition on it
            // (`math.abs`, `State.new`, a method hung off an untyped table),
            // then an intrinsic namespace member.
            if let Some(base) = receiver.qualifier() {
                if let Some(definition) = resolve_member(&index.definitions, base, name) {
                    let resolved = definition.clone();
                    let from_file = index
                        .definitions
                        .iter()
                        .any(|candidate| candidate.name == resolved.name);
                    return Some(if from_file {
                        Resolved::File(resolved)
                    } else {
                        Resolved::Prelude(resolved)
                    });
                }
                let dotted = format!("{base}.{name}");
                if INTRINSIC_MEMBERS.contains(&dotted.as_str()) {
                    return Some(Resolved::Info(format!("(builtin) {dotted}")));
                }
            }
            // Last resort: a unique method/member name anywhere in scope
            // (covers `obj:method()` calls on values with inferred types).
            let dotted_suffix = format!(".{name}");
            let method_suffix = format!(":{name}");
            let mut candidates = index
                .definitions
                .iter()
                .map(|definition| (true, definition))
                .chain(
                    prelude_definitions()
                        .iter()
                        .map(|definition| (false, definition)),
                )
                .filter(|(_, definition)| {
                    definition.name.ends_with(&dotted_suffix)
                        || definition.name.ends_with(&method_suffix)
                });
            let (from_file, first) = candidates.next()?;
            if candidates.next().is_some() {
                return None;
            }
            Some(if from_file {
                Resolved::File(first.clone())
            } else {
                Resolved::Prelude(first.clone())
            })
        }
        RefTarget::Require { raw, .. } => {
            let file = resolve_require_path(path, raw)?;
            Some(Resolved::Module {
                file,
                def: DefinitionSite {
                    name: raw.clone(),
                    name_span: Span { start: 0, end: 0 },
                    kind: DefinitionKind::Local,
                    ty: None,
                    detail: Some(format!("module {raw}")),
                    visible_from: 0,
                    scope_end: u32::MAX,
                    require_path: None,
                    initializer: None,
                },
            })
        }
    }
}

/// One-line declaration rendering for hover and completion detail.
fn definition_summary(definition: &DefinitionSite) -> String {
    if let Some(detail) = &definition.detail {
        return detail.clone();
    }
    let annotation = definition
        .ty
        .as_ref()
        .map(|ty| format!(": {ty}"))
        .unwrap_or_default();
    match definition.kind {
        DefinitionKind::Param => format!("(parameter) {}{annotation}", definition.name),
        DefinitionKind::LoopVar => format!("(loop variable) {}{annotation}", definition.name),
        DefinitionKind::IfCastBinding => format!("{}{annotation}", definition.name),
        _ => format!("local {}{annotation}", definition.name),
    }
}

fn markdown_code_block(text: &str) -> String {
    format!("```waluau\n{text}\n```")
}

pub fn hover(text: &str, path: &Path, offset: u32, load: Loader) -> Option<Hover> {
    let index = index_document(text, path);
    let target = find_target(&index, offset)?;
    let span = match &target {
        RefTarget::Definition(position) => index.definitions[*position].name_span,
        RefTarget::Name { span, .. }
        | RefTarget::Member { span, .. }
        | RefTarget::Require { span, .. } => *span,
    };
    let resolved = resolve_target(&index, &target, path, offset, load)?;
    let contents = match &resolved {
        Resolved::File(definition) | Resolved::Prelude(definition) => {
            // Unannotated locals still often have a statically chaseable
            // type (`local state = game.new()`): show it as if annotated.
            let mut summary = definition_summary(definition);
            if definition.kind == DefinitionKind::Local && definition.ty.is_none() {
                let scope = TypeScope::current(&index, path);
                if let Some((ty, _)) =
                    definition_static_type(definition, &scope, load, TYPE_CHASE_DEPTH)
                {
                    summary = format!("local {}: {ty}", definition.name);
                }
            }
            markdown_code_block(&summary)
        }
        Resolved::Module { file, def } => format!(
            "{}\n\n{}",
            markdown_code_block(&definition_summary(def)),
            file.display()
        ),
        Resolved::Info(info) => markdown_code_block(info),
        Resolved::MemberOfType { summary, .. } => markdown_code_block(summary),
    };
    Some(Hover { contents, span })
}

/// The definition location for the reference at `offset`, as a file path and
/// byte span within that file's current text.
pub fn definition(text: &str, path: &Path, offset: u32, load: Loader) -> Option<(PathBuf, Span)> {
    let index = index_document(text, path);
    let target = find_target(&index, offset)?;
    let mut resolved = resolve_target(&index, &target, path, offset, load)?;
    // A record field navigates to the type declaration that declares it.
    if let Resolved::MemberOfType { declared_by, .. } = resolved {
        resolved = *declared_by;
    }
    match resolved {
        Resolved::File(definition) => Some((path.to_path_buf(), definition.name_span)),
        Resolved::Module { file, def } => Some((file, def.name_span)),
        Resolved::Prelude(_) | Resolved::Info(_) | Resolved::MemberOfType { .. } => None,
    }
}

/// One occurrence of a symbol, as the exact identifier token that spells it.
pub struct Reference {
    pub file: PathBuf,
    pub span: Span,
}

/// The definition a find-references request is anchored to. A definition is
/// identified by where it is written — file plus name span — so two symbols
/// sharing a name never collapse into one.
struct Anchor {
    file: PathBuf,
    name_span: Span,
    /// The identifier token an occurrence must spell: the last dotted or
    /// method segment of the display name (`get` for a `P:get` method).
    token: String,
}

/// The identifier token a definition's display name ends in.
fn last_segment(name: &str) -> &str {
    name.rsplit([':', '.']).next().unwrap_or(name)
}

/// The written definition a [`Resolved`] points at, if any. Prelude declares
/// and pure hover info have no navigable site and cannot anchor or match.
fn resolved_site(resolved: Resolved, current: &Path) -> Option<(PathBuf, DefinitionSite)> {
    let resolved = match resolved {
        Resolved::MemberOfType { declared_by, .. } => *declared_by,
        other => other,
    };
    match resolved {
        Resolved::File(def) => Some((current.to_path_buf(), def)),
        Resolved::Module { file, def } => Some((file, def)),
        Resolved::Prelude(_) | Resolved::Info(_) | Resolved::MemberOfType { .. } => None,
    }
}

fn reference_anchor(text: &str, path: &Path, offset: u32, load: Loader) -> Option<Anchor> {
    let index = index_document(text, path);
    let target = find_target(&index, offset)?;
    let resolved = resolve_target(&index, &target, path, offset, load)?;
    let (file, def) = resolved_site(resolved, path)?;
    // A `require("./mod")` target is a whole file, not a name; it has no
    // identifier occurrences to find.
    (def.name_span.end > def.name_span.start).then(|| Anchor {
        token: last_segment(&def.name).to_string(),
        file,
        name_span: def.name_span,
    })
}

/// Every occurrence of the symbol at `offset`, searched across `files`.
///
/// Each candidate identifier is resolved the same way go-to-definition
/// resolves it and kept only when it lands on the same definition site. That
/// costs a resolve per name match, but it is what makes shadowed locals,
/// same-named methods on different types, and module members resolve to
/// distinct symbols instead of one text-matched blur.
pub fn references(
    text: &str,
    path: &Path,
    offset: u32,
    files: &[PathBuf],
    load: Loader,
    include_declaration: bool,
) -> Vec<Reference> {
    let Some(anchor) = reference_anchor(text, path, offset, load) else {
        return Vec::new();
    };
    let mut found: Vec<Reference> = Vec::new();
    for file in files {
        let Some(file_text) = load(file) else {
            continue;
        };
        let index = index_document(&file_text, file);
        for token in &index.tokens {
            let TokenKind::Identifier(name) = &token.kind else {
                continue;
            };
            if name != &anchor.token {
                continue;
            }
            // The declaration itself: its identifier sits inside the name
            // span (`get` within a `P:get` method name).
            let is_declaration = *file == anchor.file
                && token.span.start >= anchor.name_span.start
                && token.span.end <= anchor.name_span.end;
            if is_declaration && !include_declaration {
                continue;
            }
            let at = token.span.start;
            let Some(target) = find_target_at(&index, at) else {
                continue;
            };
            let Some(resolved) = resolve_target(&index, &target, file, at, load) else {
                continue;
            };
            let Some((site_file, site_def)) = resolved_site(resolved, file) else {
                continue;
            };
            if site_file == anchor.file && site_def.name_span == anchor.name_span {
                found.push(Reference {
                    file: file.clone(),
                    span: token.span,
                });
            }
        }
    }
    found.sort_by(|a, b| a.file.cmp(&b.file).then(a.span.start.cmp(&b.span.start)));
    found.dedup_by(|a, b| a.file == b.file && a.span == b.span);
    found
}

/// What kind of completion the cursor position asks for.
enum CompletionContext {
    /// `base.` — members of a namespace, module, or record.
    Member {
        base: String,
    },
    /// `base:` — methods of a receiver, or a type annotation.
    Method {
        base: String,
    },
    Plain,
}

fn completion_context(index: &DocumentIndex, offset: u32) -> CompletionContext {
    // The identifier the cursor is typing (a word the cursor touches from
    // inside or from its end); a cursor inside any other token — a string,
    // a number — gets no completion context.
    let word = index.tokens.iter().position(|token| {
        token.span.start < offset
            && (offset < token.span.end
                || (offset == token.span.end && matches!(token.kind, TokenKind::Identifier(_))))
    });
    let separator_index = match word {
        Some(at) => {
            if !matches!(index.tokens[at].kind, TokenKind::Identifier(_)) {
                return CompletionContext::Plain;
            }
            at.checked_sub(1)
        }
        // No word yet: the token right before the cursor (e.g. a just-typed
        // `.` or `:`) decides the context.
        None => index
            .tokens
            .iter()
            .rposition(|token| token.span.end <= offset),
    };
    let Some(separator_index) = separator_index else {
        return CompletionContext::Plain;
    };
    let is_method = match index.tokens[separator_index].kind {
        TokenKind::Dot => false,
        TokenKind::Colon => true,
        _ => return CompletionContext::Plain,
    };
    let Some(TokenKind::Identifier(base)) = separator_index
        .checked_sub(1)
        .map(|index_before| &index.tokens[index_before].kind)
    else {
        return CompletionContext::Plain;
    };
    if is_method {
        CompletionContext::Method { base: base.clone() }
    } else {
        CompletionContext::Member { base: base.clone() }
    }
}

fn completion_kind_for(definition: &DefinitionSite) -> i64 {
    match definition.kind {
        DefinitionKind::Function | DefinitionKind::DeclaredFunction => completion_kind::FUNCTION,
        DefinitionKind::DeclaredConstant => completion_kind::CONSTANT,
        DefinitionKind::Property => completion_kind::PROPERTY,
        DefinitionKind::TypeName => completion_kind::CLASS,
        _ => completion_kind::VARIABLE,
    }
}

fn push_item(items: &mut Vec<CompletionItem>, label: &str, kind: i64, detail: Option<String>) {
    if items.iter().any(|item| item.label == label) {
        return;
    }
    items.push(CompletionItem {
        label: label.to_string(),
        kind,
        detail,
    });
}

/// Members under `namespace` (`ns.member` and `ns:member` definitions plus
/// intrinsics), labeled by member name. `methods_only` restricts to the
/// `ns:member` form, for completion after `:`.
fn namespace_member_items(
    index: &DocumentIndex,
    namespace: &str,
    methods_only: bool,
    items: &mut Vec<CompletionItem>,
) {
    let dotted_prefix = format!("{namespace}.");
    let method_prefix = format!("{namespace}:");
    for definition in index.definitions.iter().chain(prelude_definitions()) {
        if is_internal_name(&definition.name) {
            continue;
        }
        let member = definition.name.strip_prefix(&method_prefix).or_else(|| {
            (!methods_only)
                .then(|| definition.name.strip_prefix(&dotted_prefix))
                .flatten()
        });
        if let Some(member) = member
            && !member.contains('.')
        {
            push_item(
                items,
                member,
                completion_kind_for(definition),
                Some(definition_summary(definition)),
            );
        }
    }
    if methods_only {
        return;
    }
    for intrinsic in INTRINSIC_MEMBERS {
        if let Some(member) = intrinsic.strip_prefix(&dotted_prefix) {
            push_item(items, member, completion_kind::FUNCTION, None);
        }
    }
}

/// Completion items derived from a base definition's static type: record
/// fields (unless `methods_only`) plus `T:`/`T.` members of a named type,
/// plus string builtins for string-typed bases. Returns whether anything was
/// produced.
fn typed_member_items(
    base_def: &DefinitionSite,
    index: &DocumentIndex,
    path: &Path,
    load: Loader,
    methods_only: bool,
    items: &mut Vec<CompletionItem>,
) -> bool {
    let scope = TypeScope::current(index, path);
    let Some((ty, type_scope)) = definition_static_type(base_def, &scope, load, TYPE_CHASE_DEPTH)
    else {
        return false;
    };
    let before = items.len();
    if !methods_only
        && let Some(record) = resolve_type_to_record(&ty, &type_scope, load, TYPE_CHASE_DEPTH)
    {
        for (name, field_ty) in &record.fields {
            let detail =
                (!matches!(field_ty, Type::Unknown)).then(|| format!("{name}: {field_ty}"));
            push_item(items, name, completion_kind::FIELD, detail);
        }
    }
    if let Type::Named { name, .. } = &ty
        && let Some((type_name, declaring_scope)) = type_name_scope(name, &type_scope, load)
    {
        let method_prefix = format!("{type_name}:");
        let dotted_prefix = format!("{type_name}.");
        for definition in declaring_scope.definitions.iter() {
            if let Some(member) = definition
                .name
                .strip_prefix(&method_prefix)
                .or_else(|| definition.name.strip_prefix(&dotted_prefix))
                && !member.contains('.')
            {
                push_item(
                    items,
                    member,
                    completion_kind_for(definition),
                    Some(definition_summary(definition)),
                );
            }
        }
    }
    if matches!(ty, Type::String) {
        for intrinsic in INTRINSIC_MEMBERS {
            if let Some(member) = intrinsic.strip_prefix("string.") {
                push_item(items, member, completion_kind::FUNCTION, None);
            }
        }
    }
    items.len() > before
}

fn type_name_items(index: &DocumentIndex, items: &mut Vec<CompletionItem>) {
    for name in PRIMITIVE_TYPE_NAMES {
        push_item(items, name, completion_kind::KEYWORD, None);
    }
    for definition in index.definitions.iter().chain(prelude_definitions()) {
        if definition.kind == DefinitionKind::TypeName {
            push_item(
                items,
                &definition.name,
                completion_kind::CLASS,
                Some(definition_summary(definition)),
            );
        }
    }
}

pub fn completion(text: &str, path: &Path, offset: u32, load: Loader) -> Vec<CompletionItem> {
    let index = index_document(text, path);
    let mut items = Vec::new();
    match completion_context(&index, offset) {
        CompletionContext::Member { base } => {
            // Module members for `require`d namespaces.
            if let Some(base_def) = resolve_name(&index.definitions, &base, offset) {
                if let Some(raw) = &base_def.require_path {
                    if let Some(file) = resolve_require_path(path, raw) {
                        for export in module_exports(&file, load) {
                            let detail = Some(definition_summary(&export));
                            push_item(&mut items, &export.name, completion_kind::FUNCTION, detail);
                        }
                    }
                    return items;
                }
                // A statically typed base contributes its record fields and
                // type members; value-level methods (`function point:m`)
                // live as namespaced definitions and are merged in below.
                typed_member_items(base_def, &index, path, load, false, &mut items);
            }
            namespace_member_items(&index, &base, false, &mut items);
        }
        CompletionContext::Method { base } => {
            if let Some(base_def) = resolve_name(&index.definitions, &base, offset).cloned() {
                typed_member_items(&base_def, &index, path, load, true, &mut items);
            }
            // Value-level methods declared directly on the base
            // (`function point:bump_x`).
            namespace_member_items(&index, &base, true, &mut items);
            // Still nothing: this is most likely a type annotation position
            // (`local x: ...`).
            if items.is_empty() {
                type_name_items(&index, &mut items);
            }
        }
        CompletionContext::Plain => {
            for definition in &index.definitions {
                if definition.visible_from <= offset
                    && offset < definition.scope_end
                    && !definition.name.contains('.')
                    && !definition.name.contains(':')
                {
                    push_item(
                        &mut items,
                        &definition.name.clone(),
                        completion_kind_for(definition),
                        Some(definition_summary(definition)),
                    );
                }
            }
            for definition in prelude_definitions() {
                if !definition.name.contains('.') && !is_internal_name(&definition.name) {
                    push_item(
                        &mut items,
                        &definition.name.clone(),
                        completion_kind_for(definition),
                        Some(definition_summary(definition)),
                    );
                }
            }
            for name in GLOBAL_NAMES {
                push_item(&mut items, name, completion_kind::MODULE, None);
            }
            for kind in waluau_ast::TypedArrayKind::ALL {
                push_item(&mut items, kind.type_name(), completion_kind::CLASS, None);
            }
            for keyword in KEYWORDS {
                push_item(&mut items, keyword, completion_kind::KEYWORD, None);
            }
        }
    }
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}
