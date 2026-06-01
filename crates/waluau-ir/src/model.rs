#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BlockId(pub usize);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ValueId(pub usize);

#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub functions: Vec<Function>,
    pub start: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub name: String,
    pub params: Vec<(String, Type)>,
    pub return_type: Type,
    pub entry: BlockId,
    pub blocks: BTreeMap<BlockId, BasicBlock>,
    pub(crate) next_value: usize,
    /// Number of leading `params` entries that are capture-cell arrays passed by the caller.
    /// Zero for top-level functions; equal to the number of captured variables for lifted closures.
    pub capture_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BasicBlock {
    pub id: BlockId,
    pub instructions: Vec<(ValueId, Instruction)>,
    pub terminator: Terminator,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Instruction {
    Param(usize),
    Number {
        ty: NumericType,
        literal: NumberLiteral,
    },
    Unit,
    Bool(bool),
    String(String),
    Cast {
        value: ValueId,
        from: Type,
        to: Type,
    },
    Binary {
        op: BinaryOp,
        left: ValueId,
        right: ValueId,
        operand_ty: Type,
        result_ty: Type,
    },
    MathIntrinsic {
        intrinsic: MathIntrinsic,
        args: Vec<ValueId>,
        operand_ty: Type,
        result_ty: Type,
    },
    ToString {
        value: ValueId,
        from: Type,
    },
    Print {
        value: ValueId,
    },
    Call {
        name: String,
        args: Vec<ValueId>,
    },
    CallValue {
        callee: ValueId,
        args: Vec<ValueId>,
        params: Vec<Type>,
        return_type: Type,
    },
    /// Create a coroutine from a zero-argument, i32-returning function value.
    /// Result type: Thread.
    CoroutineCreate {
        callee: ValueId,
    },
    /// Advance the coroutine to its next yield or return.
    /// Result type: Multi([Bool, I32]) — `(ok, value)`; `(false, 0)` when dead/errored.
    CoroutineResume {
        coroutine: ValueId,
    },
    /// Transition a suspended or dead coroutine to the dead state.
    /// Result type: Bool — true if closed cleanly or already dead, false if errored.
    CoroutineClose {
        coroutine: ValueId,
    },
    Closure {
        name: String,
        captures: Vec<ValueId>,
        params: Vec<Type>,
        return_type: Type,
    },
    ArrayNew {
        element_ty: Type,
        elements: Vec<ValueId>,
    },
    ArrayGet {
        array: ValueId,
        index: ValueId,
        element_ty: Type,
    },
    ArraySet {
        array: ValueId,
        index: ValueId,
        value: ValueId,
        element_ty: Type,
    },
    ArrayLen {
        array: ValueId,
    },
    PackMulti {
        values: Vec<ValueId>,
        types: Vec<Type>,
    },
    MultiGet {
        value: ValueId,
        index: usize,
        ty: Type,
    },
    Phi(Vec<(BlockId, ValueId)>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MathIntrinsic {
    Abs,
    Min,
    Max,
    Sqrt,
    Floor,
    Ceil,
    Trunc,
    Nearest,
    Copysign,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Terminator {
    Jump(BlockId),
    Branch {
        condition: ValueId,
        then_block: BlockId,
        else_block: BlockId,
    },
    CoroutineYield {
        value: ValueId,
        resume_block: BlockId,
    },
    Return(ValueId),
    Unreachable {
        span: Option<waluau_ast::Span>,
    },
}

impl Function {
    pub fn dump(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("fn {}:\n", self.name));
        for (id, block) in &self.blocks {
            out.push_str(&format!("  b{}:\n", id.0));
            for (value, instruction) in &block.instructions {
                out.push_str(&format!("    v{} = {:?}\n", value.0, instruction));
            }
            out.push_str(&format!("    {:?}\n", block.terminator));
        }
        out
    }

    pub(crate) fn next_value(&mut self) -> ValueId {
        let value = ValueId(self.next_value);
        self.next_value += 1;
        value
    }
}
