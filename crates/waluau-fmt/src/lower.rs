//! Lower the concrete syntax tree to a [`Doc`], encoding the formatter's style:
//! indentation, spacing, and where lines may break to fit the target width.
//!
//! Comment trivia carried on tokens is emitted here — leading comments on their
//! own lines above their token, trailing comments after it — so comments are
//! preserved through formatting.

use waluau_lexer::TokenKind;

use crate::cst::{Comment, Node, SynToken, SyntaxKind, Tree};
use crate::doc::{Doc, concat, group, hardline, indent, join, line, nil, raw_text, softline, text};

/// Render the root tree to a document.
pub fn lower_root(root: &Tree) -> Doc {
    debug_assert_eq!(root.kind, SyntaxKind::Root);
    concat([sequence(&root.children), hardline()])
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

fn node(n: &Node) -> Doc {
    match n {
        Node::Token(t) => tok(t),
        Node::Tree(t) => tree(t),
    }
}

fn tree(t: &Tree) -> Doc {
    use SyntaxKind as K;
    match t.kind {
        K::Root | K::Block => sequence(&t.children),

        // Top-level items.
        K::FunctionDecl | K::FunctionExpr => function_like(t),
        K::DeclareFunction => spaced_prefix_then_signature(t),
        K::DeclareProperty => declare_property(t),
        K::DeclareConst => declare_const(t),
        K::TypeDecl => type_decl(t),

        // Statements.
        K::LetStmt | K::ConstStmt => let_like(t),
        K::AssignStmt => assign_stmt(t),
        K::IfStmt => if_stmt(t),
        K::IfClause | K::IfCastClause => if_clause(t),
        K::WhileStmt => while_stmt(t),
        K::RepeatStmt => repeat_stmt(t),
        K::NumericForStmt | K::ForInStmt => for_stmt(t),
        K::DoStmt => do_stmt(t),
        K::ReturnStmt => return_stmt(t),
        K::BreakStmt | K::ContinueStmt => node_first_token(t),
        K::ExprStmt => node(&t.children[0]),

        // Pieces.
        K::BindingList => join(text(", "), t.children.iter().map(node)),
        K::Binding => binding(t),
        K::ExprList | K::TypeList => join(text(", "), t.children.iter().map(node)),
        K::ParamList => delimited("(", ")", t, false),
        K::Param => param(t),
        K::FunctionName => node_join_tight(t),
        K::ReturnAnnotation => return_annotation(t),
        K::Attribute => attribute(t),
        K::TypeParams => delimited("<", ">", t, false),

        // Expressions.
        K::BinaryExpr => binary_expr(t),
        K::UnaryExpr => unary_expr(t),
        K::CastExpr => cast_expr(t),
        K::IsExpr => is_expr(t),
        K::IfExpr => if_expr(t),
        K::CallExpr | K::MethodCallExpr => call_expr(t),
        K::FieldExpr => node_join_tight(t),
        K::IndexExpr => index_expr(t),
        K::ParenExpr => paren(t),
        K::NameExpr | K::LiteralExpr | K::VarargExpr => node(&t.children[0]),
        K::ArrayLiteral => delimited("{", "}", t, false),
        K::TableLiteral => delimited("{", "}", t, true),
        K::TableField | K::RecordField => field(t),
        K::ArgList => arg_list(t),
        K::TypeArgs => delimited("<", ">", t, false),

        // Types.
        K::NamedType => node_join_tight(t),
        K::NullableType => node_join_tight(t),
        K::ArrayType => node_join_tight(t),
        K::RecordType => delimited("{", "}", t, true),
        K::FunctionType => function_type(t),
        K::TaggedVariantType => node_join_tight(t),
        K::TaggedUnionType => join(text(" | "), t.children.iter().map(node)),
        K::ParenType => paren_type(t),
        K::PrimitiveType => join(text(" "), t.children.iter().map(node)),
        K::NameList => join(text(", "), t.children.iter().map(node)),
    }
}

// ---------------------------------------------------------------------------
// Tokens & comments
// ---------------------------------------------------------------------------

/// A token plus its attached comment trivia.
fn tok(t: &SynToken) -> Doc {
    let mut parts = Vec::new();
    for c in &t.leading {
        parts.push(comment_doc(c));
        parts.push(hardline());
    }
    if !t.text.is_empty() {
        parts.push(literal_text(&t.text));
    }
    if let Some(tc) = &t.trailing {
        parts.push(text(" "));
        parts.push(comment_doc(tc));
    }
    concat(parts)
}

/// Text that may span multiple lines (long strings, multi-line comments) is
/// emitted verbatim; single-line text uses the normal path.
fn literal_text(s: &str) -> Doc {
    if s.contains('\n') {
        raw_text(s.to_string())
    } else {
        text(s.to_string())
    }
}

fn comment_doc(c: &Comment) -> Doc {
    literal_text(&c.text)
}

// ---------------------------------------------------------------------------
// Sequences (statement / item lists)
// ---------------------------------------------------------------------------

/// Join a list of statements/items with newlines, preserving a single blank
/// line where the source had one or more.
fn sequence(items: &[Node]) -> Doc {
    let mut parts = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            parts.push(hardline());
            if has_blank_before(item) {
                parts.push(hardline());
            }
            // A statement beginning with `(` would otherwise be parsed as a
            // call on the previous statement's trailing value (Lua's ambiguity,
            // e.g. `a = 1 \n (f)()`). A leading `;` keeps them separate.
            if starts_with_lparen(item) {
                parts.push(text(";"));
            }
        }
        parts.push(node(item));
    }
    concat(parts)
}

fn starts_with_lparen(item: &Node) -> bool {
    matches!(item.first_token().map(|t| &t.kind), Some(TokenKind::LParen))
}

/// Whether a blank line preceded this item in the source (drives paragraph
/// spacing). Uses the item's first comment if it has leading comments,
/// otherwise its first token.
fn has_blank_before(item: &Node) -> bool {
    match item.first_token() {
        Some(t) => match t.leading.first() {
            Some(c) => c.blank_before,
            None => t.blank_before,
        },
        None => false,
    }
}

/// Wrap a block body: nothing when empty, otherwise indented on its own lines.
fn block_body(block: &Node) -> Doc {
    let Some(tree) = block.as_tree() else {
        return nil();
    };
    if tree.children.is_empty() {
        return nil();
    }
    indent(concat([hardline(), sequence(&tree.children)]))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Concatenate a node's children with no separators (for tight constructs like
/// `a.b`, `T?`, `{T}`).
fn node_join_tight(t: &Tree) -> Doc {
    concat(t.children.iter().map(node))
}

fn node_first_token(t: &Tree) -> Doc {
    node(&t.children[0])
}

/// A delimited, reflowable list from already-lowered `items`. `pad` adds inner
/// spaces in the flat form (`{ a, b }` vs `{a, b}`).
fn bracketed_docs(open: &str, close: &str, items: Vec<Doc>, pad: bool) -> Doc {
    if items.is_empty() {
        return concat([text(open.to_string()), text(close.to_string())]);
    }
    let gap = if pad { line() } else { softline() };
    let sep = concat([text(","), line()]);
    group(concat([
        text(open.to_string()),
        indent(concat([gap.clone(), join(sep, items)])),
        gap,
        text(close.to_string()),
    ]))
}

/// A delimited list whose node still contains its own bracket tokens: the
/// element children (always sub-trees) are extracted and the brackets from the
/// parse are discarded in favour of `open`/`close`. Comments attached to the
/// discarded bracket tokens are salvaged so they are not lost.
fn delimited(open: &str, close: &str, t: &Tree, pad: bool) -> Doc {
    let items: Vec<Doc> = t
        .children
        .iter()
        .filter(|n| n.as_tree().is_some())
        .map(node)
        .collect();
    let mut parts = Vec::new();
    // Leading comments on the opening bracket, emitted before the list.
    if let Some(open_tok) = t.children.first().and_then(Node::as_token) {
        for c in &open_tok.leading {
            parts.push(comment_doc(c));
            parts.push(hardline());
        }
    }
    parts.push(bracketed_docs(open, close, items, pad));
    // A trailing comment on the closing bracket, emitted after the list.
    if let Some(close_tok) = t.children.last().and_then(Node::as_token) {
        if let Some(tc) = &close_tok.trailing {
            parts.push(text(" "));
            parts.push(comment_doc(tc));
        }
    }
    concat(parts)
}

fn kind_at(t: &Tree, i: usize) -> Option<SyntaxKind> {
    t.children.get(i).and_then(|n| n.as_tree()).map(|t| t.kind)
}

fn tok_kind_at(t: &Tree, i: usize) -> Option<&TokenKind> {
    t.children
        .get(i)
        .and_then(|n| n.as_token())
        .map(|t| &t.kind)
}

// ---------------------------------------------------------------------------
// Items
// ---------------------------------------------------------------------------

/// `function name<T>(params): ret <body> end` and anonymous function exprs.
/// Children: keyword, [name], [TypeParams], ParamList, [ReturnAnnotation],
/// Block, `end`.
fn function_like(t: &Tree) -> Doc {
    let mut parts = Vec::new();
    let mut idx = 0;
    // `function` keyword.
    parts.push(node(&t.children[idx]));
    idx += 1;
    // Optional name (FunctionName node for decls, bare token for exprs).
    if matches!(
        t.children.get(idx),
        Some(Node::Tree(tr)) if tr.kind == SyntaxKind::FunctionName
    ) || matches!(t.children.get(idx), Some(Node::Token(tok)) if matches!(tok.kind, TokenKind::Identifier(_)))
    {
        parts.push(text(" "));
        parts.push(node(&t.children[idx]));
        idx += 1;
    }
    // Optional type params, then params, then optional return annotation.
    if kind_at(t, idx) == Some(SyntaxKind::TypeParams) {
        parts.push(node(&t.children[idx]));
        idx += 1;
    }
    parts.push(node(&t.children[idx])); // ParamList
    idx += 1;
    if kind_at(t, idx) == Some(SyntaxKind::ReturnAnnotation) {
        parts.push(node(&t.children[idx]));
        idx += 1;
    }
    // Block, then `end`.
    if kind_at(t, idx) == Some(SyntaxKind::Block) {
        parts.push(block_body(&t.children[idx]));
        idx += 1;
    }
    parts.push(hardline());
    if let Some(end) = t.children.get(idx) {
        parts.push(node(end)); // `end`
    }
    concat(parts)
}

/// `declare function name(params): ret` — signature only, joined tightly with
/// spaces after keywords.
fn spaced_prefix_then_signature(t: &Tree) -> Doc {
    // Children: `declare`, `function`, FunctionName, [TypeParams], ParamList,
    // [ReturnAnnotation].
    let mut parts = Vec::new();
    for (i, child) in t.children.iter().enumerate() {
        let is_param_or_after = child
            .as_tree()
            .map(|tr| {
                matches!(
                    tr.kind,
                    SyntaxKind::TypeParams | SyntaxKind::ParamList | SyntaxKind::ReturnAnnotation
                )
            })
            .unwrap_or(false);
        if i > 0 && !is_param_or_after {
            parts.push(text(" "));
        }
        parts.push(node(child));
    }
    concat(parts)
}

/// `declare property Recv: name: Type`.
/// Positions: 0 declare, 1 property, 2 recv, 3 `:`, 4 name, 5 `:`, 6 type.
fn declare_property(t: &Tree) -> Doc {
    concat([
        node(&t.children[0]), // declare
        text(" "),
        node(&t.children[1]), // property
        text(" "),
        node(&t.children[2]), // recv
        text(": "),
        node(&t.children[4]), // name
        text(": "),
        node(&t.children[6]), // type
    ])
}

/// `declare const ns.member: Type = value`.
fn declare_const(t: &Tree) -> Doc {
    // 0 declare,1 const,2 ns,3 `.`,4 member,5 `:`,6 type,7 `=`,8 value
    concat([
        node(&t.children[0]),
        text(" "),
        node(&t.children[1]),
        text(" "),
        node(&t.children[2]),
        node(&t.children[3]),
        node(&t.children[4]),
        text(": "),
        node(&t.children[6]),
        text(" = "),
        node(&t.children[8]),
    ])
}

/// `type Name<T> = Type`.
fn type_decl(t: &Tree) -> Doc {
    // 0 `type`,1 name,[TypeParams],`=`,type
    let mut parts = vec![node(&t.children[0]), text(" "), node(&t.children[1])];
    let mut idx = 2;
    if kind_at(t, idx) == Some(SyntaxKind::TypeParams) {
        parts.push(node(&t.children[idx]));
        idx += 1;
    }
    // idx now at `=` token; skip it, emit ` = ` + type.
    parts.push(text(" = "));
    parts.push(node(&t.children[idx + 1]));
    concat(parts)
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

/// `local`/`const` declarations, including `local function`.
fn let_like(t: &Tree) -> Doc {
    // `local function` / `const function` share the function tail shape.
    if matches!(tok_kind_at(t, 1), Some(TokenKind::Function)) {
        // 0 kw,1 function,2 name,[TypeParams],ParamList,[ReturnAnnotation],Block,end
        let mut parts = vec![node(&t.children[0]), text(" ")];
        // Reuse function_like on the tail by faking a sub-slice.
        let tail = Tree {
            kind: SyntaxKind::FunctionExpr,
            children: t.children[1..].to_vec(),
        };
        parts.push(function_like(&tail));
        return concat(parts);
    }
    // 0 kw, 1 BindingList, [ `=` , ExprList ]
    let mut parts = vec![node(&t.children[0]), text(" "), node(&t.children[1])];
    if t.children.len() > 2 {
        // children[2] is `=`, children[3] is ExprList.
        parts.push(text(" = "));
        parts.push(node(&t.children[3]));
    }
    concat(parts)
}

/// `targets op values`.
fn assign_stmt(t: &Tree) -> Doc {
    let op = t
        .children
        .get(1)
        .and_then(|n| n.as_token())
        .map(|tk| tk.text.clone())
        .unwrap_or_else(|| "=".to_string());
    concat([
        node(&t.children[0]),
        text(format!(" {op} ")),
        node(&t.children[2]),
    ])
}

/// `local x <const>` etc.
fn binding(t: &Tree) -> Doc {
    let mut parts = Vec::new();
    let mut i = 0;
    while i < t.children.len() {
        let child = &t.children[i];
        if let Some(tr) = child.as_tree() {
            if tr.kind == SyntaxKind::Attribute {
                parts.push(text(" "));
                parts.push(node(child));
                i += 1;
                continue;
            }
        }
        if matches!(tok_kind_at(t, i), Some(TokenKind::Colon)) {
            parts.push(text(": "));
            parts.push(node(&t.children[i + 1]));
            i += 2;
            continue;
        }
        parts.push(node(child)); // name or type
        i += 1;
    }
    concat(parts)
}

/// `name: Type` or `...` (a parameter).
fn param(t: &Tree) -> Doc {
    if matches!(tok_kind_at(t, 1), Some(TokenKind::Colon)) {
        concat([node(&t.children[0]), text(": "), node(&t.children[2])])
    } else {
        node_join_tight(t)
    }
}

fn attribute(t: &Tree) -> Doc {
    // `<` const `>` → `<const>`.
    node_join_tight(t)
}

fn return_annotation(t: &Tree) -> Doc {
    // `:` type-or-typelist → `: type`.
    concat([text(": "), node(&t.children[1])])
}

fn if_stmt(t: &Tree) -> Doc {
    // `if` <clause> `end`
    concat([
        node(&t.children[0]),
        text(" "),
        node(&t.children[1]),
        hardline(),
        node(&t.children[2]),
    ])
}

/// A clause: `cond then <body> [elseif <clause> | else <body>]`, or the
/// type-cast form `Name(binding) = value then <body> ...`.
fn if_clause(t: &Tree) -> Doc {
    let mut parts = Vec::new();
    let mut i;
    if t.kind == SyntaxKind::IfCastClause {
        // Name ( binding ) = value then ...
        parts.push(node(&t.children[0])); // Name
        parts.push(node(&t.children[1])); // (
        parts.push(node(&t.children[2])); // binding
        parts.push(node(&t.children[3])); // )
        parts.push(text(" = "));
        parts.push(node(&t.children[4])); // value
        parts.push(text(" "));
        parts.push(node(&t.children[5])); // then
        i = 6;
    } else {
        parts.push(node(&t.children[0])); // condition
        parts.push(text(" "));
        parts.push(node(&t.children[1])); // then
        i = 2;
    }
    // then-body block.
    parts.push(block_body(&t.children[i]));
    i += 1;
    // Optional else / elseif tail.
    while i < t.children.len() {
        match tok_kind_at(t, i) {
            Some(TokenKind::ElseIf) => {
                parts.push(hardline());
                parts.push(node(&t.children[i])); // elseif
                parts.push(text(" "));
                parts.push(node(&t.children[i + 1])); // nested clause
                i += 2;
            }
            Some(TokenKind::Else) => {
                parts.push(hardline());
                parts.push(node(&t.children[i])); // else
                parts.push(block_body(&t.children[i + 1]));
                i += 2;
            }
            _ => i += 1,
        }
    }
    concat(parts)
}

fn while_stmt(t: &Tree) -> Doc {
    // while cond do <body> end
    concat([
        node(&t.children[0]),
        text(" "),
        node(&t.children[1]),
        text(" "),
        node(&t.children[2]), // do
        block_body(&t.children[3]),
        hardline(),
        node(&t.children[4]), // end
    ])
}

fn repeat_stmt(t: &Tree) -> Doc {
    // repeat <body> until cond
    concat([
        node(&t.children[0]), // repeat
        block_body(&t.children[1]),
        hardline(),
        node(&t.children[2]), // until
        text(" "),
        node(&t.children[3]), // cond
    ])
}

fn for_stmt(t: &Tree) -> Doc {
    // Emit the header tokens/exprs up to `do` with contextual spacing, then the
    // body and `end`.
    let mut parts = Vec::new();
    let mut i = 0;
    while i < t.children.len() {
        let is_do = matches!(tok_kind_at(t, i), Some(TokenKind::Do));
        if is_do {
            parts.push(text(" "));
            parts.push(node(&t.children[i])); // do
            parts.push(block_body(&t.children[i + 1]));
            parts.push(hardline());
            parts.push(node(&t.children[i + 2])); // end
            break;
        }
        parts.push(for_header_piece(t, i));
        i += 1;
    }
    concat(parts)
}

/// Spacing for one token/expr in a `for` header.
fn for_header_piece(t: &Tree, i: usize) -> Doc {
    match tok_kind_at(t, i) {
        Some(TokenKind::For) => node(&t.children[i]),
        Some(TokenKind::Equal) => text(" = "),
        Some(TokenKind::In) => text(" in "),
        Some(TokenKind::Identifier(_)) => {
            // Loop variable: preceded by a space unless it directly follows
            // `for`, and comma-separated names get ", ".
            if matches!(tok_kind_at(t, i - 1), Some(TokenKind::For)) {
                concat([text(" "), node(&t.children[i])])
            } else {
                concat([text(", "), node(&t.children[i])])
            }
        }
        // Expressions (start/stop/step/iterator).
        _ => {
            let prev = tok_kind_at(t, i - 1);
            match prev {
                Some(TokenKind::Equal | TokenKind::In) => node(&t.children[i]),
                // A second/third numeric-for expression follows a comma.
                _ if t.children[i].as_tree().is_some() => {
                    concat([text(", "), node(&t.children[i])])
                }
                _ => node(&t.children[i]),
            }
        }
    }
}

fn do_stmt(t: &Tree) -> Doc {
    concat([
        node(&t.children[0]), // do
        block_body(&t.children[1]),
        hardline(),
        node(&t.children[2]), // end
    ])
}

fn return_stmt(t: &Tree) -> Doc {
    if t.children.len() == 1 {
        node(&t.children[0]) // just `return`
    } else {
        concat([node(&t.children[0]), text(" "), node(&t.children[1])])
    }
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

fn binary_expr(t: &Tree) -> Doc {
    // [left, op, right]
    concat([
        node(&t.children[0]),
        text(" "),
        node(&t.children[1]),
        text(" "),
        node(&t.children[2]),
    ])
}

fn unary_expr(t: &Tree) -> Doc {
    // [op, operand]. `not` always needs a space. A `-` needs a space before an
    // operand that itself starts with `-`, otherwise `- -x` would collapse to
    // `--x`, which lexes as a comment.
    let op = tok_kind_at(t, 0);
    let operand_starts_with_minus = matches!(
        t.children[1].first_token().map(|t| &t.kind),
        Some(TokenKind::Minus)
    );
    let space = matches!(op, Some(TokenKind::Not))
        || (matches!(op, Some(TokenKind::Minus)) && operand_starts_with_minus);
    if space {
        concat([node(&t.children[0]), text(" "), node(&t.children[1])])
    } else {
        concat([node(&t.children[0]), node(&t.children[1])])
    }
}

fn cast_expr(t: &Tree) -> Doc {
    // [expr, `::`, type] → `expr::Type` (the corpus's dominant style).
    concat([node(&t.children[0]), text("::"), node(&t.children[2])])
}

fn is_expr(t: &Tree) -> Doc {
    // [expr, `is`, tag] → `expr is Tag`
    concat([
        node(&t.children[0]),
        text(" "),
        node(&t.children[1]),
        text(" "),
        node(&t.children[2]),
    ])
}

fn if_expr(t: &Tree) -> Doc {
    // if cond then a else b
    concat([
        node(&t.children[0]),
        text(" "),
        node(&t.children[1]),
        text(" "),
        node(&t.children[2]),
        text(" "),
        node(&t.children[3]),
        text(" "),
        node(&t.children[4]),
        text(" "),
        node(&t.children[5]),
    ])
}

fn call_expr(t: &Tree) -> Doc {
    // Callee/receiver then optional TypeArgs then ArgList. For method calls the
    // receiver is followed by `:` `name`.
    concat(t.children.iter().map(node))
}

fn index_expr(t: &Tree) -> Doc {
    // [base, `[`, index, `]`]
    concat([
        node(&t.children[0]),
        node(&t.children[1]),
        node(&t.children[2]),
        node(&t.children[3]),
    ])
}

fn paren(t: &Tree) -> Doc {
    // [`(`, expr, `)`]
    concat([
        node(&t.children[0]),
        node(&t.children[1]),
        node(&t.children[2]),
    ])
}

fn field(t: &Tree) -> Doc {
    // TableField: [name, `=`, value]  → `name = value`
    // RecordField: [name, `:`, type] → `name: type`
    if matches!(tok_kind_at(t, 1), Some(TokenKind::Colon)) {
        concat([node(&t.children[0]), text(": "), node(&t.children[2])])
    } else {
        concat([node(&t.children[0]), text(" = "), node(&t.children[2])])
    }
}

fn arg_list(t: &Tree) -> Doc {
    // Paren form has `(`/`)` tokens; sugar form is a single string/table arg.
    let has_parens = matches!(tok_kind_at(t, 0), Some(TokenKind::LParen));
    if has_parens {
        delimited("(", ")", t, false)
    } else if let Some(arg) = t.children.first() {
        // `f "str"` / `f { ... }`.
        concat([text(" "), node(arg)])
    } else {
        text("()")
    }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

fn function_type(t: &Tree) -> Doc {
    // [TypeList(params), returnType]
    concat([
        text("("),
        node(&t.children[0]),
        text(") -> "),
        node(&t.children[1]),
    ])
}

fn paren_type(t: &Tree) -> Doc {
    // Inner types only (grouping `(T)` or unit `()`).
    concat([
        text("("),
        join(text(", "), t.children.iter().map(node)),
        text(")"),
    ])
}
