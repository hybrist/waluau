//! A lossless concrete syntax tree for Waluau.
//!
//! Unlike the compiler AST (which reorders top-level items, desugars string
//! interpolation, and drops comments), this tree preserves everything the
//! formatter needs: original item order, comments, blank-line grouping, and the
//! exact lexeme text of every token. It is a homogeneous "green tree" — every
//! node is a [`Tree`] tagged with a [`SyntaxKind`] whose children are either
//! sub-[`Tree`]s or [`SynToken`] leaves — which keeps the parser and the
//! CST→[`Doc`](crate::doc) lowering small.

use waluau_lexer::{Span, TokenKind};

/// The role a [`Tree`] plays in the grammar. Tokens keep their lexer
/// [`TokenKind`]; only interior nodes need a `SyntaxKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyntaxKind {
    /// The whole file: a sequence of top-level items.
    Root,

    // Top-level items.
    FunctionDecl,
    DeclareFunction,
    DeclareProperty,
    DeclareConst,
    TypeDecl,

    // Statements.
    LetStmt,
    ConstStmt,
    AssignStmt,
    IfStmt,
    IfClause,
    /// A type-narrowing `if Name(binding) = value then` clause.
    IfCastClause,
    WhileStmt,
    RepeatStmt,
    NumericForStmt,
    ForInStmt,
    DoStmt,
    ReturnStmt,
    BreakStmt,
    ContinueStmt,
    ExprStmt,
    /// A `{ ... }` sequence of statements inside a block-bearing construct.
    Block,

    // Shared pieces.
    Binding,
    BindingList,
    NameList,
    ExprList,
    TypeParams,
    ParamList,
    Param,
    FunctionName,
    ReturnAnnotation,
    Attribute,

    // Expressions.
    BinaryExpr,
    UnaryExpr,
    CastExpr,
    IsExpr,
    IfExpr,
    CallExpr,
    MethodCallExpr,
    FieldExpr,
    IndexExpr,
    ParenExpr,
    NameExpr,
    LiteralExpr,
    VarargExpr,
    FunctionExpr,
    ArrayLiteral,
    TableLiteral,
    TableField,
    ArgList,
    TypeArgs,

    // Types.
    NamedType,
    NullableType,
    ArrayType,
    RecordType,
    RecordField,
    FunctionType,
    TypeList,
    TaggedVariantType,
    TaggedUnionType,
    ParenType,
    PrimitiveType,
}

/// A comment attached to a token as leading or trailing trivia.
#[derive(Clone, Debug)]
pub struct Comment {
    /// The raw comment lexeme, e.g. `-- hi` or `--[[ block ]]`.
    pub text: String,
    /// A `--[[ ]]` block comment (vs a `--` line comment). A line comment
    /// forces a newline after it; a block comment can stay inline.
    pub block: bool,
    /// Whether a blank line preceded this comment in the source.
    pub blank_before: bool,
}

/// A token leaf carrying its exact source text plus attached comment trivia.
#[derive(Clone, Debug)]
pub struct SynToken {
    /// The lexer classification, used by the parser and by layout decisions.
    pub kind: TokenKind,
    /// The token's span in the original source (char offsets). Synthetic
    /// tokens use an empty span.
    pub span: Span,
    /// The exact source lexeme (verbatim, including quotes/escapes).
    pub text: String,
    /// Comments that appear before this token on their own line(s).
    pub leading: Vec<Comment>,
    /// A comment on the same line, after this token.
    pub trailing: Option<Comment>,
    /// Whether a blank line separated this token from the previous one.
    pub blank_before: bool,
}

/// Either an interior node or a token leaf.
#[derive(Clone, Debug)]
pub enum Node {
    Token(SynToken),
    Tree(Tree),
}

/// An interior node: a [`SyntaxKind`] and its ordered children.
#[derive(Clone, Debug)]
pub struct Tree {
    pub kind: SyntaxKind,
    pub children: Vec<Node>,
}

impl Tree {
    pub fn new(kind: SyntaxKind, children: Vec<Node>) -> Self {
        Tree { kind, children }
    }

    /// The first token in this subtree, if any (depth-first).
    pub fn first_token(&self) -> Option<&SynToken> {
        for child in &self.children {
            match child {
                Node::Token(t) => return Some(t),
                Node::Tree(t) => {
                    if let Some(tok) = t.first_token() {
                        return Some(tok);
                    }
                }
            }
        }
        None
    }

    /// The last token in this subtree, if any (depth-first).
    pub fn last_token(&self) -> Option<&SynToken> {
        for child in self.children.iter().rev() {
            match child {
                Node::Token(t) => return Some(t),
                Node::Tree(t) => {
                    if let Some(tok) = t.last_token() {
                        return Some(tok);
                    }
                }
            }
        }
        None
    }

    /// The source span covered by this subtree (first token start to last token
    /// end), if it contains any non-synthetic tokens.
    pub fn span(&self) -> Option<Span> {
        let start = self.first_token()?.span.start;
        let end = self.last_token()?.span.end;
        Some(Span { start, end })
    }
}

impl Node {
    pub fn as_tree(&self) -> Option<&Tree> {
        match self {
            Node::Tree(t) => Some(t),
            Node::Token(_) => None,
        }
    }

    pub fn as_token(&self) -> Option<&SynToken> {
        match self {
            Node::Token(t) => Some(t),
            Node::Tree(_) => None,
        }
    }

    /// The first token in this element (depth-first).
    pub fn first_token(&self) -> Option<&SynToken> {
        match self {
            Node::Token(t) => Some(t),
            Node::Tree(t) => t.first_token(),
        }
    }

    /// Debug helper: reconstruct source text ignoring layout (tokens joined by
    /// single spaces, comments dropped). Used only to sanity-check the parse
    /// covered every token; real output comes from the [`Doc`](crate::doc)
    /// lowering.
    #[cfg(test)]
    pub fn token_texts(&self, out: &mut Vec<String>) {
        match self {
            Node::Token(t) => out.push(t.text.clone()),
            Node::Tree(tree) => {
                for child in &tree.children {
                    child.token_texts(out);
                }
            }
        }
    }
}
