pub use waluau_span::Span;

#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub functions: Vec<Function>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Type,
    pub body: Vec<Stmt>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NumericType {
    U32,
    U64,
    I32,
    I64,
    F32,
    F64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Type {
    Numeric(NumericType),
    Bool,
}

impl Type {
    pub const fn number() -> Self {
        Self::Numeric(NumericType::F64)
    }

    pub const fn is_numeric(self) -> bool {
        matches!(self, Self::Numeric(_))
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
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Let {
        name: String,
        ty: Type,
        value: Expr,
    },
    Assign {
        name: String,
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
    Return(Expr),
    Expr(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Number(f64),
    Bool(bool),
    Name(String),
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Call {
        name: String,
        args: Vec<Expr>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Less,
    Greater,
    And,
    Or,
}
