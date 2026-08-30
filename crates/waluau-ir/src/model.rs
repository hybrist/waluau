#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BlockId(pub usize);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ValueId(pub usize);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SourceFileId(pub u32);

/// One authored input, stored once per IR module. Instruction locations refer
/// to this table instead of copying paths or source text into every record.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceFile {
    pub path: String,
    pub source: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceLocation {
    pub file: SourceFileId,
    pub span: waluau_ast::Span,
}

/// Whether generated code can be attributed to authored Waluau source.
///
/// Lowering records authored locations in its compact maps. Instructions that
/// directly implement an authored expression or statement intentionally share
/// that construct's span, including control-flow scaffolding needed to
/// implement it. Emission outside an authored construct has no entry and
/// resolves to `Synthetic`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceOrigin {
    Authored(SourceLocation),
    Synthetic,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionSourceMap {
    pub definition: SourceOrigin,
    pub(crate) instruction_locations: BTreeMap<ValueId, SourceLocation>,
    pub(crate) terminator_locations: BTreeMap<BlockId, SourceLocation>,
}

impl FunctionSourceMap {
    pub fn synthetic() -> Self {
        Self {
            definition: SourceOrigin::Synthetic,
            instruction_locations: BTreeMap::new(),
            terminator_locations: BTreeMap::new(),
        }
    }

    pub fn instruction_origin(&self, value: ValueId) -> SourceOrigin {
        self.instruction_locations
            .get(&value)
            .copied()
            .map_or(SourceOrigin::Synthetic, SourceOrigin::Authored)
    }

    pub fn terminator_origin(&self, block: BlockId) -> SourceOrigin {
        self.terminator_locations
            .get(&block)
            .copied()
            .map_or(SourceOrigin::Synthetic, SourceOrigin::Authored)
    }

    pub(crate) fn record_instruction(&mut self, value: ValueId, location: SourceLocation) {
        self.instruction_locations.insert(value, location);
    }

    pub(crate) fn record_terminator(&mut self, block: BlockId, location: SourceLocation) {
        self.terminator_locations.insert(block, location);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub functions: Vec<Function>,
    /// Non-generic authored module functions that browser tooling may expose,
    /// keyed by resolved declaration identity. This is a compiler-produced
    /// export plan: code generation must not reconstruct authored status from
    /// generated names.
    pub tooling_function_exports: BTreeMap<SymbolId, String>,
    /// Entry-file `export function` declarations keyed by resolved function
    /// identity. Code generation uses the authored name as the stable browser
    /// Wasm export name; linked dependency declarations never enter this map.
    pub authored_function_exports: BTreeMap<SymbolId, String>,
    pub globals: Vec<Global>,
    pub declared_imports: Vec<DeclaredImport>,
    pub start: Option<usize>,
    pub tag_ids: std::collections::BTreeMap<String, i32>,
    /// Source name of each resolved symbol. Debug metadata only: Wasm emission
    /// combines this with [`Function::value_symbols`] to label locals in the
    /// emitted `name` custom section.
    pub symbol_names: BTreeMap<SymbolId, String>,
    /// Authored files referenced by function source maps, interned by index.
    pub source_files: Vec<SourceFile>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Global {
    pub name: String,
    pub symbol_id: SymbolId,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeclaredImport {
    pub module: String,
    pub name: String,
    pub host_name: String,
    pub params: Vec<Type>,
    pub return_type: Type,
    pub symbol_id: SymbolId,
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
    pub value_symbols: BTreeMap<ValueId, SymbolId>,
    pub symbol_id: Option<SymbolId>,
    pub source_map: FunctionSourceMap,
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
    GlobalGet {
        global: usize,
        ty: Type,
    },
    GlobalSet {
        global: usize,
        value: ValueId,
        ty: Type,
    },
    Number {
        ty: NumericType,
        literal: NumberLiteral,
    },
    Unit,
    Bool(bool),
    Null {
        ty: Type,
    },
    String(String),
    Bytes(Vec<u8>),
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
    IsNull {
        value: ValueId,
        ty: Type,
    },
    ExternCastTest {
        value: ValueId,
        target_name: String,
    },
    MathIntrinsic {
        intrinsic: MathIntrinsic,
        args: Vec<ValueId>,
        operand_ty: Type,
        result_ty: Type,
    },
    BitwiseIntrinsic {
        intrinsic: BitwiseIntrinsic,
        args: Vec<ValueId>,
        result_ty: Type,
    },
    ToString {
        value: ValueId,
        from: Type,
    },
    TypeName {
        value: ValueId,
        from: Type,
    },
    ToNumber {
        value: ValueId,
        from: Type,
        base: Option<ValueId>,
    },
    Print {
        value: ValueId,
    },
    Throw {
        error: ValueId,
    },
    Call {
        name: String,
        symbol_id: Option<SymbolId>,
        args: Vec<ValueId>,
    },
    HostCall {
        name: String,
        symbol_id: SymbolId,
        args: Vec<ValueId>,
        return_type: Type,
    },
    CallValue {
        callee: ValueId,
        args: Vec<ValueId>,
        params: Vec<Type>,
        return_type: Type,
    },
    ProtectedCall {
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
    /// Result type: Multi([Bool, Unknown]) — `(ok, value)`; `value` is null when dead/errored.
    CoroutineResume {
        coroutine: ValueId,
    },
    /// Advance the coroutine and return a canonical tagged-union record `{ tag: i32, value: unknown }`.
    /// `yielded_tag`/`finished_tag`/`error_tag` are the stable i32 discriminants for each variant.
    /// Result type: `Type::canonical_tagged_union_record()`.
    CoroutineResumeTagged {
        coroutine: ValueId,
        yielded_tag: i32,
        finished_tag: i32,
        error_tag: i32,
    },
    /// Consume the settled payload for a previously-suspended `coroutine.await_promise`.
    /// Result type: Unknown.
    CoroutineAwaitResult,
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
    /// Drop the last element of a growable array by decrementing its logical
    /// length (traps when the array is empty). Unit-valued; callers that need
    /// the removed element read it with `ArrayGet` first. The storage slot is
    /// not cleared, so a reference-typed element stays reachable until the
    /// slot is overwritten by a later append.
    ArrayPop {
        array: ValueId,
        element_ty: Type,
    },
    ArraySlice {
        array: ValueId,
        start: ValueId,
        element_ty: Type,
    },
    /// Length of an `unknown` value that holds an array at runtime.
    /// Codegen dispatches over the module's growable array wrapper types;
    /// traps when the value is not an array.
    DynLen {
        value: ValueId,
    },
    /// Read `value[index]` where `value` is `unknown`; the element is boxed
    /// into `unknown`. Traps when the value is not an array or the index is
    /// out of bounds.
    DynIndex {
        value: ValueId,
        index: ValueId,
    },
    BytesGet {
        bytes: ValueId,
        index: ValueId,
    },
    BytesLen {
        bytes: ValueId,
    },
    /// Allocate a fresh typed array in linear memory and store each element.
    /// Result type: TypedArray(kind); the value is the i32 data pointer.
    BufferNew {
        kind: TypedArrayKind,
        elements: Vec<ValueId>,
    },
    /// Allocate a fresh typed array and copy its compile-time-constant
    /// contents (little-endian element bytes) from a passive data segment.
    /// Each evaluation produces a fresh, independently mutable copy.
    BufferConst {
        kind: TypedArrayKind,
        bytes: Vec<u8>,
    },
    /// Allocate a zero-filled typed array of `len` elements (traps when `len`
    /// is negative). Result type: TypedArray(kind).
    BufferNewSized {
        kind: TypedArrayKind,
        len: ValueId,
    },
    /// Bounds-checked element read; traps when `index` is out of range.
    /// Sub-word integer kinds widen to their 32-bit signedness.
    BufferGet {
        buffer: ValueId,
        index: ValueId,
        kind: TypedArrayKind,
    },
    /// Bounds-checked element write; traps when `index` is out of range.
    /// `value` has the kind's element numeric type; integer stores truncate.
    BufferSet {
        buffer: ValueId,
        index: ValueId,
        value: ValueId,
        kind: TypedArrayKind,
    },
    /// Element count of a typed array (read from the allocation header).
    BufferLen {
        buffer: ValueId,
    },
    /// Allocate a zero-filled fixed-size Luau buffer of `len` bytes.
    LuauBufferNew {
        len: ValueId,
    },
    /// Return the byte length stored in a Luau buffer handle.
    LuauBufferLen {
        buffer: ValueId,
    },
    /// Bounds-checked scalar load at a zero-based byte offset.
    LuauBufferGet {
        buffer: ValueId,
        offset: ValueId,
        kind: TypedArrayKind,
    },
    /// Bounds-checked scalar store at a zero-based byte offset.
    LuauBufferSet {
        buffer: ValueId,
        offset: ValueId,
        value: ValueId,
        kind: TypedArrayKind,
    },
    StructNew {
        struct_ty: Type,
        fields: Vec<ValueId>,
    },
    StructGet {
        base: ValueId,
        field: String,
        field_ty: Type,
    },
    StructSet {
        base: ValueId,
        field: String,
        value: ValueId,
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
    /// Unary float negation (`f32.neg`/`f64.neg`); preserves -0 and NaN sign,
    /// unlike lowering `-x` as `0 - x`.
    ///
    /// The `math.*` builtins (abs, sqrt, min, ...) are no longer intrinsics:
    /// they are declared host imports (builtins/math.walu) lowered as
    /// ordinary host calls.
    Neg,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BitwiseIntrinsic {
    Not,
    And,
    Or,
    Xor,
    Test,
    LRotate,
    RRotate,
    LShift,
    RShift,
    ArithmeticRShift,
    Extract,
    Replace,
    ByteSwap,
    CountLeadingZeros,
    CountTrailingZeros,
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
    CoroutineAwaitPromise {
        promise: ValueId,
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
        if let Some(sym_id) = self.symbol_id {
            out.push_str(&format!("fn {} ; @{}:\n", self.name, sym_id.0));
        } else {
            out.push_str(&format!("fn {}:\n", self.name));
        }
        for (id, block) in &self.blocks {
            out.push_str(&format!("  b{}:\n", id.0));
            for (value, instruction) in &block.instructions {
                let sym_str = if let Some(sym_id) = self.value_symbols.get(value) {
                    format!(" ; @{}", sym_id.0)
                } else {
                    "".to_string()
                };
                out.push_str(&format!("    v{} = {:?}{}\n", value.0, instruction, sym_str));
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
