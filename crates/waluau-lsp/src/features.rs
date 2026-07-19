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

/// What the cursor is on, classified from the token stream.
enum RefTarget {
    /// A definition site's own name.
    Definition(usize),
    /// A plain identifier reference.
    Name { name: String, span: Span },
    /// The member part of `base.member` or `base:member`.
    Member {
        base: String,
        name: String,
        span: Span,
    },
    /// The string argument of a `require("...")` call.
    Require { raw: String, span: Span },
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
            // `base.name` / `base:name` — the member part of a namespaced or
            // method reference.
            if at >= 2
                && matches!(index.tokens[at - 1].kind, TokenKind::Dot | TokenKind::Colon)
                && let TokenKind::Identifier(base) = &index.tokens[at - 2].kind
            {
                return Some(RefTarget::Member {
                    base: base.clone(),
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

/// Resolve a value member path (`new` or `game.new`) to its definition:
/// bare names in the scope (then the prelude), dotted names through a
/// require alias's module or a namespace definition.
fn resolve_value_path(
    path_str: &str,
    offset: u32,
    scope: &TypeScope,
    load: Loader,
) -> Option<(DefinitionSite, TypeScope)> {
    if let Some((alias, member)) = path_str.split_once('.') {
        if let Some(alias_def) = resolve_name(&scope.definitions, alias, offset)
            && let Some(raw) = &alias_def.require_path
            && let Some(file) = resolve_require_path(&scope.file, raw)
            && let Some(module) = module_scope(&file, load)
        {
            let def = module
                .definitions
                .iter()
                .find(|definition| definition.name == member)?
                .clone();
            return Some((def, module));
        }
        // A namespace member (`math.abs`, dot-named `State.new`).
        if let Some(def) = scope
            .definitions
            .iter()
            .find(|definition| definition.name == path_str)
        {
            return Some((def.clone(), scope.clone()));
        }
        let prelude = TypeScope::prelude();
        let def = prelude
            .definitions
            .iter()
            .find(|definition| definition.name == path_str)?
            .clone();
        return Some((def, prelude));
    }
    if let Some(def) = resolve_name(&scope.definitions, path_str, offset) {
        return Some((def.clone(), scope.clone()));
    }
    let prelude = TypeScope::prelude();
    let def = prelude
        .definitions
        .iter()
        .find(|definition| definition.name == path_str)?
        .clone();
    Some((def, prelude))
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
        Type::Opaque { ty: inner, .. } | Type::Nullable(inner) => {
            resolve_type_to_record(inner, scope, load, depth - 1)
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
    let at = def.visible_from;
    match def.initializer.as_ref()? {
        waluau_parser::InitializerHint::Call { callee } => {
            let (function_def, function_scope) = resolve_value_path(callee, at, scope, load)?;
            let Some(Type::Function { return_type, .. }) = function_def.ty else {
                return None;
            };
            Some((*return_type, function_scope))
        }
        waluau_parser::InitializerHint::MethodCall { receiver, method } => {
            let receiver_def = resolve_name(&scope.definitions, receiver, at)?.clone();
            let (receiver_ty, receiver_scope) =
                definition_static_type(&receiver_def, scope, load, depth - 1)?;
            let (method_def, method_scope) =
                named_type_member(&receiver_ty, method, &receiver_scope, load)?;
            let Some(Type::Function { return_type, .. }) = method_def.ty else {
                return None;
            };
            Some((*return_type, method_scope))
        }
        waluau_parser::InitializerHint::Field {
            base,
            field,
            indexed,
        } => {
            let base_def = resolve_name(&scope.definitions, base, at)?.clone();
            let (base_ty, base_scope) = definition_static_type(&base_def, scope, load, depth - 1)?;
            let record = resolve_type_to_record(&base_ty, &base_scope, load, depth - 1)?;
            let field_scope = record
                .declared_by
                .as_ref()
                .map(|(_, scope)| scope.clone())
                .unwrap_or(base_scope);
            let mut ty = record.fields.get(field)?.clone();
            if *indexed {
                ty = ty.element_type()?;
            }
            Some((ty, field_scope))
        }
        waluau_parser::InitializerHint::Index { base } => {
            let base_def = resolve_name(&scope.definitions, base, at)?.clone();
            let (base_ty, base_scope) = definition_static_type(&base_def, scope, load, depth - 1)?;
            Some((base_ty.element_type()?, base_scope))
        }
    }
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
        RefTarget::Member { base, name, .. } => {
            // A local bound to `require(...)`: resolve into that module.
            if let Some(base_def) = resolve_name(&index.definitions, base, offset)
                && let Some(raw) = &base_def.require_path
            {
                let file = resolve_require_path(path, raw)?;
                let def = module_exports(&file, load)
                    .into_iter()
                    .find(|definition| &definition.name == name)?;
                return Some(Resolved::Module { file, def });
            }
            // A base with a statically known type — annotated, or chased
            // through its initializer (`local state = game.new()`). Methods
            // resolve as `T:name`/`T.name` in the type's declaring module;
            // fields resolve through the type declaration's record shape.
            if let Some(base_def) = resolve_name(&index.definitions, base, offset) {
                let scope = TypeScope::current(index, path);
                if let Some((base_ty, base_scope)) =
                    definition_static_type(base_def, &scope, load, TYPE_CHASE_DEPTH)
                {
                    if let Some((method_def, method_scope)) =
                        named_type_member(&base_ty, name, &base_scope, load)
                    {
                        return Some(resolved_in_scope(&method_def, &method_scope, path));
                    }
                    if let Some(record) =
                        resolve_type_to_record(&base_ty, &base_scope, load, TYPE_CHASE_DEPTH)
                        && let Some(field_ty) = record.fields.get(name)
                    {
                        return Some(match record.declared_by {
                            Some((type_def, type_scope)) => Resolved::MemberOfType {
                                summary: format!("{name}: {field_ty}"),
                                declared_by: Box::new(resolved_in_scope(
                                    &type_def,
                                    &type_scope,
                                    path,
                                )),
                            },
                            None => Resolved::Info(format!("{base}.{name}: {field_ty}")),
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
            // A builtin/user namespace member (`math.abs`, `State.new`).
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
            // An intrinsic namespace member (`string.upper`, `table.insert`).
            let dotted = format!("{base}.{name}");
            if INTRINSIC_MEMBERS.contains(&dotted.as_str()) {
                return Some(Resolved::Info(format!("(builtin) {dotted}")));
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
/// intrinsics), labeled by member name.
fn namespace_member_items(index: &DocumentIndex, namespace: &str, items: &mut Vec<CompletionItem>) {
    let dotted_prefix = format!("{namespace}.");
    let method_prefix = format!("{namespace}:");
    for definition in index.definitions.iter().chain(prelude_definitions()) {
        if is_internal_name(&definition.name) {
            continue;
        }
        if let Some(member) = definition
            .name
            .strip_prefix(&dotted_prefix)
            .or_else(|| definition.name.strip_prefix(&method_prefix))
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
            push_item(
                items,
                name,
                completion_kind::FIELD,
                Some(format!("{name}: {field_ty}")),
            );
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
                // A statically typed base: record fields and type members.
                if typed_member_items(base_def, &index, path, load, false, &mut items) {
                    items.sort_by(|a, b| a.label.cmp(&b.label));
                    return items;
                }
            }
            namespace_member_items(&index, &base, &mut items);
        }
        CompletionContext::Method { base } => {
            let produced = resolve_name(&index.definitions, &base, offset)
                .cloned()
                .is_some_and(|base_def| {
                    typed_member_items(&base_def, &index, path, load, true, &mut items)
                });
            // No known receiver type: this is most likely a type annotation
            // position (`local x: ...`).
            if !produced {
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
