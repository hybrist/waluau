pub use waluau_span::Span;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SymbolId(pub usize);

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub functions: Vec<Function>,
    pub declared_imports: Vec<DeclaredImport>,
    pub type_declarations: Vec<TypeDeclaration>,
    pub top_level: Vec<Stmt>,
    /// The value a module exports through a trailing top-level `return`.
    ///
    /// The value a module exports: a function name or a table of functions.
    /// Consumed by the module linker in `waluau-driver` and ignored when a
    /// program is compiled as a standalone entry point.
    pub export: Option<Expr>,
    pub sources: BTreeMap<String, String>,
    pub entry_file_path: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeDeclaration {
    pub name: String,
    pub type_params: Vec<String>,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeclaredImport {
    pub name: String,
    pub host_name: String,
    pub symbol_id: Option<SymbolId>,
    pub params: Vec<Param>,
    pub return_type: Type,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableField {
    pub name: String,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub name: FunctionName,
    pub symbol_id: Option<SymbolId>,
    pub type_params: Vec<String>,
    pub params: Vec<Param>,
    pub vararg: bool,
    pub return_type: Option<Type>,
    pub body: Vec<Stmt>,
    pub file_path: String,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum FunctionName {
    Simple(String),
    Method { table: String, method: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionExpr {
    pub name: Option<String>,
    pub symbol_id: Option<SymbolId>,
    pub implicit_self: Option<String>,
    pub type_params: Vec<String>,
    pub params: Vec<Param>,
    pub vararg: bool,
    pub return_type: Option<Type>,
    pub body: Vec<Stmt>,
    pub file_path: String,
    pub span: Option<Span>,
}

impl FunctionName {
    pub fn simple_name(&self) -> Option<&str> {
        match self {
            Self::Simple(name) => Some(name),
            Self::Method { .. } => None,
        }
    }
}

impl std::fmt::Display for FunctionName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Simple(name) => f.write_str(name),
            Self::Method { table, method } => write!(f, "{table}:{method}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    pub symbol_id: Option<SymbolId>,
    pub ty: Type,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TaggedVariant {
    pub tag: String,
    pub payload: Box<Type>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MethodCallOrigin {
    /// The original receiver expression from the method call
    pub original_receiver: Box<Expr>,
    /// The method name that was called
    pub method_name: String,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NumericType {
    U32,
    U64,
    I32,
    I64,
    F32,
    F64,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Type {
    Numeric(NumericType),
    Unit,
    Bool,
    String,
    Bytes,
    Extern,
    ExternSubtype(Box<Type>),
    Nil,
    Nullable(Box<Type>),
    TaggedVariant(TaggedVariant),
    TaggedUnion(Vec<TaggedVariant>),
    Named {
        name: String,
        type_args: Vec<Type>,
    },
    Opaque {
        name: String,
        ty: Box<Type>,
    },
    Array(Box<Type>),
    Multi(Vec<Type>),
    Function {
        params: Vec<Type>,
        return_type: Box<Type>,
    },
    /// A fixed-shape record used for module namespaces (`require` results).
    Record(BTreeMap<String, Type>),
    /// Reference to an in-scope generic type parameter (e.g. `T` in `function f<T>(x: T)`).
    TypeParam(String),
    /// A coroutine handle. Yield/resume values are always `i32` (see design 0007).
    Thread,
    Unknown,
}

impl Type {
    pub const fn number() -> Self {
        Self::Numeric(NumericType::F64)
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, Self::Numeric(_))
    }

    pub fn is_array(&self) -> bool {
        matches!(self, Self::Array(_))
    }

    pub fn is_record(&self) -> bool {
        matches!(self, Self::Record(_))
    }

    pub fn element_type(&self) -> Option<Type> {
        match self {
            Self::Array(element) => Some(*element.clone()),
            _ => None,
        }
    }

    pub fn record_field(&self, name: &str) -> Option<Type> {
        match self {
            Self::Record(fields) => fields.get(name).cloned(),
            Self::Opaque { ty, .. } => ty.record_field(name),
            Self::TaggedVariant(variant) if name == "value" => Some((*variant.payload).clone()),
            _ => None,
        }
    }

    pub fn nullable_inner(&self) -> Option<Type> {
        match self {
            Self::Nullable(inner) => Some(*inner.clone()),
            _ => None,
        }
    }

    pub fn tagged_variant(&self, tag: &str) -> Option<TaggedVariant> {
        match self {
            Self::TaggedVariant(variant) if variant.tag == tag => Some(variant.clone()),
            Self::TaggedUnion(variants) => {
                variants.iter().find(|variant| variant.tag == tag).cloned()
            }
            Self::Opaque { ty, .. } => ty.tagged_variant(tag),
            _ => None,
        }
    }

    /// The canonical GC record used at runtime to represent any tagged-union value.
    /// Layout: `{ tag: i32, value: unknown }` where `tag` is the variant discriminant
    /// and `value` holds the boxed payload (anyref / i31ref).
    pub fn canonical_tagged_union_record() -> Self {
        let mut fields = BTreeMap::new();
        fields.insert("tag".to_string(), Type::Numeric(NumericType::I32));
        fields.insert("value".to_string(), Type::Unknown);
        Type::Record(fields)
    }

    pub fn remove_tagged_variant(&self, tag: &str) -> Option<Type> {
        match self {
            Self::TaggedVariant(variant) if variant.tag == tag => None,
            Self::TaggedUnion(variants) => {
                let remaining = variants
                    .iter()
                    .filter(|variant| variant.tag != tag)
                    .cloned()
                    .collect::<Vec<_>>();
                match remaining.len() {
                    0 => None,
                    1 => Some(Self::TaggedVariant(
                        remaining.into_iter().next().expect("len checked"),
                    )),
                    _ => Some(Self::TaggedUnion(remaining)),
                }
            }
            Self::Opaque { name, ty } => ty.remove_tagged_variant(tag).map(|inner| Self::Opaque {
                name: name.clone(),
                ty: Box::new(inner),
            }),
            _ => None,
        }
    }
}

impl NumericType {
    pub fn can_implicitly_widen_to(self, target: Self) -> bool {
        use NumericType::{F32, F64, I32, I64, U32, U64};

        match (self, target) {
            (from, to) if from == to => true,
            (U32, U64 | I64 | F64) => true,
            (I32, I64 | F64) => true,
            (F32, F64) => true,
            _ => false,
        }
    }

    pub fn common(self, other: Self) -> Option<Self> {
        if self.can_implicitly_widen_to(other) {
            Some(other)
        } else if other.can_implicitly_widen_to(self) {
            Some(self)
        } else {
            None
        }
    }
}

impl std::fmt::Display for NumericType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::F32 => "f32",
            Self::F64 => "f64",
        };
        f.write_str(name)
    }
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Numeric(ty) => ty.fmt(f),
            Self::Unit => f.write_str("unit"),
            Self::Bool => f.write_str("bool"),
            Self::Unknown => f.write_str("unknown"),
            Self::String => f.write_str("string"),
            Self::Bytes => f.write_str("bytes"),
            Self::Extern => f.write_str("extern"),
            Self::ExternSubtype(parent) => write!(f, "extern extends {parent}"),
            Self::Nil => f.write_str("nil"),
            Self::Nullable(inner) => write!(f, "{inner}?"),
            Self::TaggedVariant(variant) => write!(f, "{}({})", variant.tag, variant.payload),
            Self::TaggedUnion(variants) => {
                for (index, variant) in variants.iter().enumerate() {
                    if index > 0 {
                        write!(f, " | ")?;
                    }
                    write!(f, "{}({})", variant.tag, variant.payload)?;
                }
                Ok(())
            }
            Self::Named { name, type_args } => {
                f.write_str(name)?;
                if !type_args.is_empty() {
                    write!(f, "<")?;
                    for (index, ty) in type_args.iter().enumerate() {
                        if index > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{ty}")?;
                    }
                    write!(f, ">")?;
                }
                Ok(())
            }
            Self::Opaque { name, .. } => f.write_str(name),
            Self::Array(element) => write!(f, "{{{element}}}"),
            Self::Multi(types) => {
                for (index, ty) in types.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{ty}")?;
                }
                Ok(())
            }
            Self::Function {
                params,
                return_type,
            } => {
                write!(f, "(")?;
                for (index, param) in params.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{param}")?;
                }
                write!(f, ") -> {return_type}")
            }
            Self::Record(fields) => {
                write!(f, "{{")?;
                for (index, (name, ty)) in fields.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{name}: {ty}")?;
                }
                write!(f, "}}")
            }
            Self::TypeParam(name) => f.write_str(name),
            Self::Thread => f.write_str("thread"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum Stmt {
    Let {
        name: String,
        symbol_id: Option<SymbolId>,
        rebindability: Rebindability,
        ty: Option<Type>,
        value: Expr,
    },
    Assign {
        op: AssignOp,
        name: String,
        symbol_id: Option<SymbolId>,
        value: Expr,
    },
    IndexAssign {
        op: AssignOp,
        base: Box<Expr>,
        index: Box<Expr>,
        value: Expr,
    },
    FieldAssign {
        op: AssignOp,
        base: Box<Expr>,
        name: String,
        resolved_name: Option<String>,
        value: Expr,
    },
    If {
        condition: Expr,
        then_body: Vec<Stmt>,
        else_body: Vec<Stmt>,
    },
    IfCast {
        target_name: String,
        target_ty: Type,
        binding: String,
        binding_symbol_id: Option<SymbolId>,
        value: Expr,
        then_body: Vec<Stmt>,
        else_body: Vec<Stmt>,
    },
    While {
        condition: Expr,
        body: Vec<Stmt>,
    },
    Repeat {
        body: Vec<Stmt>,
        condition: Expr,
    },
    NumericFor {
        name: String,
        symbol_id: Option<SymbolId>,
        start: Expr,
        stop: Expr,
        step: Option<Expr>,
        body: Vec<Stmt>,
    },
    ForIn {
        names: Vec<String>,
        symbol_ids: Option<Vec<SymbolId>>,
        iterator: Expr,
        body: Vec<Stmt>,
    },
    Break,
    Continue,
    Return(Expr),
    ReturnMulti(Vec<Expr>),
    LetMulti {
        bindings: Vec<Binding>,
        values: Vec<Expr>,
    },
    AssignMulti {
        targets: Vec<String>,
        symbol_ids: Option<Vec<SymbolId>>,
        values: Vec<Expr>,
    },
    Expr(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub name: String,
    pub symbol_id: Option<SymbolId>,
    pub rebindability: Rebindability,
    pub ty: Option<Type>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssignOp {
    Set,
    /// A compound assignment `target op= value`, desugaring to
    /// `target = target op value` while evaluating `target` only once.
    Compound(BinaryOp),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rebindability {
    Rebindable,
    Const,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Number(NumberLiteral, Option<Span>),
    Bool(bool, Option<Span>),
    Nil(Option<Span>),
    String(String, Option<Span>),
    Bytes(Vec<u8>, Option<Span>),
    Name(String, Option<SymbolId>, Option<Span>),
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
        resolved_name: Option<String>,
        span: Option<Span>,
    },
    Cast {
        expr: Box<Expr>,
        ty: Type,
        span: Option<Span>,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
        resolved_name: Option<String>,
        span: Option<Span>,
    },
    IsVariant {
        expr: Box<Expr>,
        tag: String,
        span: Option<Span>,
    },
    If {
        condition: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
        span: Option<Span>,
    },
    Call {
        callee: Box<Expr>,
        type_args: Vec<Type>,
        args: Vec<Expr>,
        span: Option<Span>,
        /// If this call originated from a generic method call, this contains
        /// information needed to perform receiver mutation writeback.
        /// The receiver is always the first argument when this is Some.
        method_call_origin: Option<MethodCallOrigin>,
    },
    Vararg(Option<Span>),
    MethodCall {
        receiver: Box<Expr>,
        name: String,
        resolved_name: Option<String>,
        type_args: Vec<Type>,
        args: Vec<Expr>,
        span: Option<Span>,
    },
    Function(FunctionExpr),
    /// A relative module import, e.g. `require("./math")`.
    ///
    /// The string is the raw path as written in source. The module linker in
    /// `waluau-driver` resolves it and replaces this node with a reference to
    /// the imported module's exported function, so later compiler stages never
    /// observe a `Require` node.
    Require(String, Option<Span>),
    ArrayLiteral {
        elements: Vec<Expr>,
        span: Option<Span>,
    },
    /// A table literal with named fields, e.g. `{ add = fn, sub = other }`.
    TableLiteral {
        fields: Vec<TableField>,
        span: Option<Span>,
    },
    /// Field access on a namespace value, e.g. `m.add`.
    Field {
        base: Box<Expr>,
        name: String,
        resolved_name: Option<String>,
        span: Option<Span>,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
        span: Option<Span>,
    },
}

impl Expr {
    pub fn span(&self) -> Option<Span> {
        match self {
            Expr::Number(_, span) => *span,
            Expr::Bool(_, span) => *span,
            Expr::Nil(span) => *span,
            Expr::String(_, span) => *span,
            Expr::Bytes(_, span) => *span,
            Expr::Name(_, _, span) => *span,
            Expr::Vararg(span) => *span,
            Expr::Unary { span, .. } => *span,
            Expr::Cast { span, .. } => *span,
            Expr::Binary { span, .. } => *span,
            Expr::IsVariant { span, .. } => *span,
            Expr::If { span, .. } => *span,
            Expr::Call { span, .. } => *span,
            Expr::MethodCall { span, .. } => *span,
            Expr::Function(f) => f.span,
            Expr::Require(_, span) => *span,
            Expr::ArrayLiteral { span, .. } => *span,
            Expr::TableLiteral { span, .. } => *span,
            Expr::Field { span, .. } => *span,
            Expr::Index { span, .. } => *span,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NumberLiteral {
    pub raw: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOp {
    Add,
    Concat,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    Eq,
    NotEq,
    Less,
    LessEq,
    Greater,
    GreaterEq,
    And,
    Or,
}

impl BinaryOp {
    /// Whether `target op= value` is a legal compound assignment for a target of
    /// type `ty`. Arithmetic ops require a numeric target; `..` requires a
    /// string target. Other operators are never used in compound assignment.
    pub fn compound_target_ok(self, ty: &Type) -> bool {
        match self {
            BinaryOp::Concat => matches!(ty, Type::String),
            BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::FloorDiv
            | BinaryOp::Mod
            | BinaryOp::Pow => ty.is_numeric(),
            BinaryOp::Eq
            | BinaryOp::NotEq
            | BinaryOp::Less
            | BinaryOp::LessEq
            | BinaryOp::Greater
            | BinaryOp::GreaterEq
            | BinaryOp::And
            | BinaryOp::Or => false,
        }
    }

    /// Human-readable description of the target type a compound assignment with
    /// this operator requires, used in diagnostics.
    pub fn compound_target_kind(self) -> &'static str {
        match self {
            BinaryOp::Concat => "string",
            _ => "numeric",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOp {
    Neg,
    Not,
    Len,
}

use std::collections::HashMap;
use waluau_diagnostics::Diagnostic;

struct Resolver {
    scopes: Vec<HashMap<String, SymbolId>>,
    next_symbol_id: usize,
}

impl Resolver {
    fn new() -> Self {
        let mut global_bindings = HashMap::new();
        let mut resolver = Self {
            scopes: Vec::new(),
            next_symbol_id: 1,
        };

        // Populate builtins
        for builtin in &[
            "print",
            "assert",
            "tostring",
            "select",
            "math",
            "coroutine",
            "promise",
            "table",
            "string",
            "bit32",
        ] {
            let id = resolver.next_id();
            global_bindings.insert(builtin.to_string(), id);
        }

        resolver.scopes.push(global_bindings);
        resolver
    }

    fn next_id(&mut self) -> SymbolId {
        let id = SymbolId(self.next_symbol_id);
        self.next_symbol_id += 1;
        id
    }

    fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn exit_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, name: &str) -> SymbolId {
        let id = self.next_id();
        if let Some(current) = self.scopes.last_mut() {
            current.insert(name.to_string(), id);
        }
        id
    }

    fn lookup(&self, name: &str) -> Option<SymbolId> {
        for scope in self.scopes.iter().rev() {
            if let Some(id) = scope.get(name) {
                return Some(*id);
            }
        }
        None
    }

    fn resolve_function(&mut self, function: &mut Function) -> Result<(), Diagnostic> {
        self.enter_scope();
        for param in &mut function.params {
            let id = self.declare(&param.name);
            param.symbol_id = Some(id);
        }
        for stmt in &mut function.body {
            self.resolve_stmt(stmt)?;
        }
        self.exit_scope();
        Ok(())
    }

    fn resolve_stmt(&mut self, stmt: &mut Stmt) -> Result<(), Diagnostic> {
        match stmt {
            Stmt::Let {
                name,
                symbol_id,
                value,
                ..
            } => {
                self.resolve_expr(value)?;
                let id = self.declare(name);
                *symbol_id = Some(id);
            }
            Stmt::Assign {
                name,
                symbol_id,
                value,
                ..
            } => {
                self.resolve_expr(value)?;
                let id = self
                    .lookup(name)
                    .ok_or_else(|| Diagnostic::new(format!("unknown local/global '{name}'")))?;
                *symbol_id = Some(id);
            }
            Stmt::LetMulti { bindings, values } => {
                for value in values {
                    self.resolve_expr(value)?;
                }
                for binding in bindings {
                    let id = self.declare(&binding.name);
                    binding.symbol_id = Some(id);
                }
            }
            Stmt::AssignMulti {
                targets,
                symbol_ids,
                values,
            } => {
                for value in values {
                    self.resolve_expr(value)?;
                }
                let mut ids = Vec::new();
                for target in targets {
                    let id = self.lookup(target).ok_or_else(|| {
                        Diagnostic::new(format!("unknown local/global '{target}'"))
                    })?;
                    ids.push(id);
                }
                *symbol_ids = Some(ids);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                self.resolve_expr(condition)?;
                self.enter_scope();
                for s in then_body {
                    self.resolve_stmt(s)?;
                }
                self.exit_scope();
                self.enter_scope();
                for s in else_body {
                    self.resolve_stmt(s)?;
                }
                self.exit_scope();
            }
            Stmt::IfCast {
                binding,
                binding_symbol_id,
                value,
                then_body,
                else_body,
                ..
            } => {
                self.resolve_expr(value)?;
                self.enter_scope();
                let id = self.declare(binding);
                *binding_symbol_id = Some(id);
                for s in then_body {
                    self.resolve_stmt(s)?;
                }
                self.exit_scope();
                self.enter_scope();
                for s in else_body {
                    self.resolve_stmt(s)?;
                }
                self.exit_scope();
            }
            Stmt::While { condition, body } => {
                self.resolve_expr(condition)?;
                self.enter_scope();
                for s in body {
                    self.resolve_stmt(s)?;
                }
                self.exit_scope();
            }
            Stmt::Repeat { body, condition } => {
                self.enter_scope();
                for s in body {
                    self.resolve_stmt(s)?;
                }
                self.resolve_expr(condition)?;
                self.exit_scope();
            }
            Stmt::NumericFor {
                name,
                symbol_id,
                start,
                stop,
                step,
                body,
            } => {
                self.resolve_expr(start)?;
                self.resolve_expr(stop)?;
                if let Some(s) = step {
                    self.resolve_expr(s)?;
                }
                self.enter_scope();
                let id = self.declare(name);
                *symbol_id = Some(id);
                for s in body {
                    self.resolve_stmt(s)?;
                }
                self.exit_scope();
            }
            Stmt::ForIn {
                names,
                symbol_ids,
                iterator,
                body,
            } => {
                self.resolve_expr(iterator)?;
                self.enter_scope();
                let mut ids = Vec::new();
                for name in names {
                    let id = self.declare(name);
                    ids.push(id);
                }
                *symbol_ids = Some(ids);
                for s in body {
                    self.resolve_stmt(s)?;
                }
                self.exit_scope();
            }
            Stmt::Return(expr) => {
                self.resolve_expr(expr)?;
            }
            Stmt::ReturnMulti(exprs) => {
                for expr in exprs {
                    self.resolve_expr(expr)?;
                }
            }
            Stmt::Expr(expr) => {
                self.resolve_expr(expr)?;
            }
            Stmt::IndexAssign {
                base, index, value, ..
            } => {
                self.resolve_expr(base)?;
                self.resolve_expr(index)?;
                self.resolve_expr(value)?;
            }
            Stmt::FieldAssign { base, value, .. } => {
                self.resolve_expr(base)?;
                self.resolve_expr(value)?;
            }
            Stmt::Break | Stmt::Continue => {}
        }
        Ok(())
    }

    fn resolve_expr(&mut self, expr: &mut Expr) -> Result<(), Diagnostic> {
        match expr {
            Expr::Name(name, symbol_id, _) => {
                let id = self
                    .lookup(name)
                    .ok_or_else(|| Diagnostic::new(format!("unknown local/global '{name}'")))?;
                *symbol_id = Some(id);
            }
            Expr::Unary { expr, .. } | Expr::Cast { expr, .. } | Expr::IsVariant { expr, .. } => {
                self.resolve_expr(expr)?;
            }
            Expr::Binary { left, right, .. }
            | Expr::Index {
                base: left,
                index: right,
                ..
            } => {
                self.resolve_expr(left)?;
                self.resolve_expr(right)?;
            }
            Expr::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => {
                self.resolve_expr(condition)?;
                self.resolve_expr(then_expr)?;
                self.resolve_expr(else_expr)?;
            }
            Expr::Call {
                callee,
                args,
                method_call_origin,
                ..
            } => {
                // `Tag(expr)` may be a tagged-union constructor rather than a call to a
                // known function/local. HIR has already validated such names (rejecting
                // genuinely unknown names with "unknown name '...'"), so here we simply
                // leave the callee's symbol_id as `None` and let IR lowering recognize it
                // as a constructor via the expected tagged-union type.
                let is_potential_constructor = matches!(
                    (callee.as_ref(), args.as_slice()),
                    (Expr::Name(name, _, _), [_]) if self.lookup(name).is_none()
                );
                if !is_potential_constructor {
                    self.resolve_expr(callee)?;
                }
                for arg in args {
                    self.resolve_expr(arg)?;
                }
                if let Some(origin) = method_call_origin {
                    self.resolve_expr(&mut origin.original_receiver)?;
                }
            }
            Expr::MethodCall { receiver, args, .. } => {
                self.resolve_expr(receiver)?;
                for arg in args {
                    self.resolve_expr(arg)?;
                }
            }
            Expr::Function(function) => {
                self.enter_scope();
                if let Some(name) = &function.name {
                    let id = self.declare(name);
                    function.symbol_id = Some(id);
                }
                for param in &mut function.params {
                    let id = self.declare(&param.name);
                    param.symbol_id = Some(id);
                }
                for s in &mut function.body {
                    self.resolve_stmt(s)?;
                }
                self.exit_scope();
            }
            Expr::ArrayLiteral { elements, .. } => {
                for element in elements {
                    self.resolve_expr(element)?;
                }
            }
            Expr::TableLiteral { fields, .. } => {
                for field in fields {
                    self.resolve_expr(&mut field.value)?;
                }
            }
            Expr::Field { base, .. } => {
                self.resolve_expr(base)?;
            }
            Expr::Number(..)
            | Expr::Bool(..)
            | Expr::Nil(..)
            | Expr::String(..)
            | Expr::Bytes(..)
            | Expr::Vararg(..)
            | Expr::Require(..) => {}
        }
        Ok(())
    }
}

pub fn resolve_symbols(program: &mut Program) -> Result<(), Diagnostic> {
    let mut resolver = Resolver::new();

    // Declare all top-level functions first
    for function in &mut program.functions {
        if let FunctionName::Simple(name) = &function.name {
            let id = resolver.declare(name);
            function.symbol_id = Some(id);
        }
    }
    for declared in &mut program.declared_imports {
        let id = resolver.declare(&declared.name);
        declared.symbol_id = Some(id);
    }

    // Resolve top-level statements
    for stmt in &mut program.top_level {
        resolver.resolve_stmt(stmt)?;
    }

    // Resolve export expression
    if let Some(export) = &mut program.export {
        resolver.resolve_expr(export)?;
    }

    // Resolve each function's body
    for function in &mut program.functions {
        resolver.resolve_function(function)?;
    }

    Ok(())
}
