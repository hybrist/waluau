pub use waluau_span::Span;

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub functions: Vec<Function>,
    pub top_level: Vec<Stmt>,
    /// The value a module exports through a trailing top-level `return`.
    ///
    /// The value a module exports: a function name or a table of functions.
    /// Consumed by the module linker in `waluau-driver` and ignored when a
    /// program is compiled as a standalone entry point.
    pub export: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableField {
    pub name: String,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
    pub body: Vec<Stmt>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionExpr {
    pub name: Option<String>,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
    pub body: Vec<Stmt>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Type,
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
    Bool,
    String,
    Array(Box<Type>),
    Multi(Vec<Type>),
    Function {
        params: Vec<Type>,
        return_type: Box<Type>,
    },
    /// A fixed-shape record used for module namespaces (`require` results).
    Record(BTreeMap<String, Type>),
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
            Self::Bool => f.write_str("bool"),
            Self::String => f.write_str("string"),
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
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Let {
        name: String,
        rebindability: Rebindability,
        ty: Option<Type>,
        value: Expr,
    },
    Assign {
        op: AssignOp,
        name: String,
        value: Expr,
    },
    IndexAssign {
        op: AssignOp,
        base: Box<Expr>,
        index: Box<Expr>,
        value: Expr,
    },
    If {
        condition: Expr,
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
        values: Vec<Expr>,
    },
    Expr(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub name: String,
    pub rebindability: Rebindability,
    pub ty: Option<Type>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssignOp {
    Set,
    Add,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rebindability {
    Rebindable,
    Const,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Number(NumberLiteral),
    Bool(bool),
    String(String),
    Name(String),
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Cast {
        expr: Box<Expr>,
        ty: Type,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    If {
        condition: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Function(FunctionExpr),
    /// A relative module import, e.g. `require("./math")`.
    ///
    /// The string is the raw path as written in source. The module linker in
    /// `waluau-driver` resolves it and replaces this node with a reference to
    /// the imported module's exported function, so later compiler stages never
    /// observe a `Require` node.
    Require(String),
    ArrayLiteral {
        elements: Vec<Expr>,
    },
    /// A table literal with named fields, e.g. `{ add = fn, sub = other }`.
    ///
    /// Supported for module exports and `require` results only in the MVP.
    TableLiteral {
        fields: Vec<TableField>,
    },
    /// Field access on a namespace value, e.g. `m.add`.
    Field {
        base: Box<Expr>,
        name: String,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NumberLiteral {
    pub raw: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Eq,
    Less,
    Greater,
    And,
    Or,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOp {
    Neg,
    Not,
    Len,
}
