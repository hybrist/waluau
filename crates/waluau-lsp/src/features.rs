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

fn find_target(index: &DocumentIndex, offset: u32) -> Option<RefTarget> {
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
/// `None`.
fn resolve_require_path(current_file: &Path, raw: &str) -> Option<PathBuf> {
    if !(raw.starts_with("./") || raw.starts_with("../")) {
        return None;
    }
    let dir = current_file.parent()?;
    let mut candidate = dir.join(raw);
    if candidate.extension().is_none() {
        candidate.set_extension("walu");
    }
    Some(candidate.canonicalize().unwrap_or(candidate))
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
}

/// Exported members of a required module: its file-visible functions.
fn module_exports(file: &Path, load: Loader) -> Vec<DefinitionSite> {
    let Some(text) = load(file) else {
        return Vec::new();
    };
    waluau_parser::parse_with_recovery(&text, &file.to_string_lossy())
        .definitions
        .into_iter()
        .filter(|definition| {
            definition.kind == DefinitionKind::Function
                && !definition.name.contains('.')
                && !definition.name.contains(':')
        })
        .collect()
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
            // A method receiver with a declared named type: `r:text()` where
            // `r: Response` resolves through `Response.text`.
            if let Some(base_def) = resolve_name(&index.definitions, base, offset)
                && let Some(Type::Named {
                    name: type_name, ..
                }) = &base_def.ty
                && let Some(definition) = resolve_member(&index.definitions, type_name, name)
            {
                return Some(Resolved::File(definition.clone()));
            }
            // A record-typed base with an annotated field.
            if let Some(base_def) = resolve_name(&index.definitions, base, offset)
                && let Some(ty) = &base_def.ty
                && let Some(field_ty) = ty.record_field(name)
            {
                return Some(Resolved::Info(format!("{base}.{name}: {field_ty}")));
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
            // A method on a string value (`s:upper()`).
            if resolve_name(&index.definitions, base, offset)
                .and_then(|definition| definition.ty.as_ref())
                .is_some_and(|ty| matches!(ty, Type::String))
                && INTRINSIC_MEMBERS.contains(&format!("string.{name}").as_str())
            {
                return Some(Resolved::Info(format!("(builtin) string.{name}")));
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
            markdown_code_block(&definition_summary(definition))
        }
        Resolved::Module { file, def } => format!(
            "{}\n\n{}",
            markdown_code_block(&definition_summary(def)),
            file.display()
        ),
        Resolved::Info(info) => markdown_code_block(info),
    };
    Some(Hover { contents, span })
}

/// The definition location for the reference at `offset`, as a file path and
/// byte span within that file's current text.
pub fn definition(text: &str, path: &Path, offset: u32, load: Loader) -> Option<(PathBuf, Span)> {
    let index = index_document(text, path);
    let target = find_target(&index, offset)?;
    match resolve_target(&index, &target, path, offset, load)? {
        Resolved::File(definition) => Some((path.to_path_buf(), definition.name_span)),
        Resolved::Module { file, def } => Some((file, def.name_span)),
        Resolved::Prelude(_) | Resolved::Info(_) => None,
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
                // Annotated record fields.
                if let Some(Type::Record(fields)) = &base_def.ty {
                    for (name, ty) in fields {
                        push_item(
                            &mut items,
                            name,
                            completion_kind::FIELD,
                            Some(format!("{name}: {ty}")),
                        );
                    }
                    return items;
                }
            }
            namespace_member_items(&index, &base, &mut items);
        }
        CompletionContext::Method { base } => {
            let base_type = resolve_name(&index.definitions, &base, offset)
                .and_then(|definition| definition.ty.clone());
            match base_type {
                Some(Type::Named { name, .. }) => {
                    namespace_member_items(&index, &name, &mut items);
                }
                Some(Type::String) => namespace_member_items(&index, "string", &mut items),
                // No known receiver type: this is most likely a type
                // annotation position (`local x: ...`).
                _ => type_name_items(&index, &mut items),
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
