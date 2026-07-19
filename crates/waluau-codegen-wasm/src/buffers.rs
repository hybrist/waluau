//! Linear-memory typed arrays (`Float32Array` & friends).
//!
//! Runtime layout: a typed-array value is an i32 pointer to its element data.
//! Each allocation is preceded by an 8-byte header whose first 4 bytes hold
//! the element count (the remaining 4 are reserved padding that keeps f64
//! element data 8-byte aligned). Allocation is a bump allocator over the
//! module's single linear memory: a mutable i32 global tracks the heap top,
//! `memory.grow` extends the memory on demand, and nothing is ever freed —
//! which also means freshly allocated regions are always zero, so sized
//! allocations need no explicit fill.
//!
//! Compile-time-constant literals are stored as passive data segments
//! (deduplicated module-wide) and copied into a fresh allocation with
//! `memory.init` on every evaluation, so each evaluation yields an
//! independently mutable array. Segments are never dropped because a literal
//! inside a function body re-initializes on every call.

use waluau_ast::TypedArrayKind;
use waluau_diagnostics::Diagnostic;
use waluau_ir::{Instruction as IrInstruction, Module};
use wasm_encoder::{BlockType, Function, Instruction, ValType};

/// Byte offset from the data pointer back to the element-count header field.
pub(crate) const BUFFER_HEADER_SIZE: i32 = 8;

/// First byte of the bump heap. Leaves address 0 unused so a zeroed pointer
/// never aliases a live allocation.
pub(crate) const BUFFER_HEAP_BASE: i32 = 16;

/// The wasm export name of the linear memory (JS view helpers read it).
pub(crate) const MEMORY_EXPORT_NAME: &str = "memory";

pub(crate) const fn element_size_log2(kind: TypedArrayKind) -> i32 {
    match kind.element_size() {
        1 => 0,
        2 => 1,
        4 => 2,
        _ => 3,
    }
}

/// Linear-memory usage of a module: whether any typed-array instruction is
/// present, and the deduplicated passive data segments backing `BufferConst`
/// instructions (in first-seen order).
#[derive(Default)]
pub(crate) struct BufferPlan {
    pub(crate) uses_memory: bool,
    pub(crate) data_segments: Vec<Vec<u8>>,
}

impl BufferPlan {
    pub(crate) fn new(module: &Module) -> Self {
        let mut plan = Self::default();
        for function in &module.functions {
            for block in function.blocks.values() {
                for (_, instruction) in &block.instructions {
                    match instruction {
                        IrInstruction::BufferConst { bytes, .. } => {
                            plan.uses_memory = true;
                            if !plan.data_segments.iter().any(|seg| seg == bytes) {
                                plan.data_segments.push(bytes.clone());
                            }
                        }
                        IrInstruction::BufferNew { .. }
                        | IrInstruction::BufferNewSized { .. }
                        | IrInstruction::BufferGet { .. }
                        | IrInstruction::BufferSet { .. }
                        | IrInstruction::BufferLen { .. } => {
                            plan.uses_memory = true;
                        }
                        _ => {}
                    }
                }
            }
        }
        plan
    }

    pub(crate) fn data_segment_index(&self, bytes: &[u8]) -> Result<u32, Diagnostic> {
        self.data_segments
            .iter()
            .position(|seg| seg == bytes)
            .map(|index| index as u32)
            .ok_or_else(|| Diagnostic::new("missing typed-array data segment"))
    }
}

/// Emit the shared bump-allocation helper:
///
/// `__waluau_buffer_alloc(len: i32, elem_size_log2: i32) -> i32 (data ptr)`
///
/// Traps when `len` is negative or the allocation cannot fit in memory.
/// Writes the element count into the allocation header and returns the
/// pointer to the (zeroed) element data.
pub(crate) fn emit_buffer_alloc_function(heap_ptr_global: u32) -> Function {
    // Locals (after the two i32 params): 0=len 1=log2 2=header_ptr 3=end
    let mut out = Function::new(vec![(2, ValType::I32)]);
    let header_ptr = 2u32;
    let end = 3u32;

    // Trap when len (as unsigned) exceeds the per-allocation byte budget for
    // this element size; this also rejects negative lengths. The budget keeps
    // `len << log2` well below i32 overflow.
    out.instruction(&Instruction::LocalGet(0));
    out.instruction(&Instruction::I32Const(0x1000_0000));
    out.instruction(&Instruction::LocalGet(1));
    out.instruction(&Instruction::I32ShrU);
    out.instruction(&Instruction::I32GtU);
    out.instruction(&Instruction::If(BlockType::Empty));
    out.instruction(&Instruction::Unreachable);
    out.instruction(&Instruction::End);

    // header_ptr = (heap_ptr + 7) & ~7 — 8-byte alignment keeps f64 data
    // aligned (the data pointer is header_ptr + 8).
    out.instruction(&Instruction::GlobalGet(heap_ptr_global));
    out.instruction(&Instruction::I32Const(7));
    out.instruction(&Instruction::I32Add);
    out.instruction(&Instruction::I32Const(!7));
    out.instruction(&Instruction::I32And);
    out.instruction(&Instruction::LocalSet(header_ptr));

    // end = header_ptr + HEADER + (len << log2)
    out.instruction(&Instruction::LocalGet(header_ptr));
    out.instruction(&Instruction::I32Const(BUFFER_HEADER_SIZE));
    out.instruction(&Instruction::I32Add);
    out.instruction(&Instruction::LocalGet(0));
    out.instruction(&Instruction::LocalGet(1));
    out.instruction(&Instruction::I32Shl);
    out.instruction(&Instruction::I32Add);
    out.instruction(&Instruction::LocalSet(end));

    // Grow memory when `end` exceeds the current size (memory.size is in
    // 64 KiB pages). Newly grown pages are zero.
    out.instruction(&Instruction::LocalGet(end));
    out.instruction(&Instruction::MemorySize(0));
    out.instruction(&Instruction::I32Const(16));
    out.instruction(&Instruction::I32Shl);
    out.instruction(&Instruction::I32GtU);
    out.instruction(&Instruction::If(BlockType::Empty));
    // pages_needed = (end - memory.size*64K + 0xFFFF) >> 16
    out.instruction(&Instruction::LocalGet(end));
    out.instruction(&Instruction::MemorySize(0));
    out.instruction(&Instruction::I32Const(16));
    out.instruction(&Instruction::I32Shl);
    out.instruction(&Instruction::I32Sub);
    out.instruction(&Instruction::I32Const(0xFFFF));
    out.instruction(&Instruction::I32Add);
    out.instruction(&Instruction::I32Const(16));
    out.instruction(&Instruction::I32ShrU);
    out.instruction(&Instruction::MemoryGrow(0));
    out.instruction(&Instruction::I32Const(-1));
    out.instruction(&Instruction::I32Eq);
    out.instruction(&Instruction::If(BlockType::Empty));
    out.instruction(&Instruction::Unreachable);
    out.instruction(&Instruction::End);
    out.instruction(&Instruction::End);

    // Store the element count in the header; zero the reserved padding word
    // explicitly (the bump region is normally already zero, but being
    // deliberate here costs one store and keeps the invariant local).
    out.instruction(&Instruction::LocalGet(header_ptr));
    out.instruction(&Instruction::LocalGet(0));
    out.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
        offset: 0,
        align: 2,
        memory_index: 0,
    }));
    out.instruction(&Instruction::LocalGet(header_ptr));
    out.instruction(&Instruction::I32Const(0));
    out.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
        offset: 4,
        align: 2,
        memory_index: 0,
    }));

    // heap_ptr = end; return the data pointer.
    out.instruction(&Instruction::LocalGet(end));
    out.instruction(&Instruction::GlobalSet(heap_ptr_global));
    out.instruction(&Instruction::LocalGet(header_ptr));
    out.instruction(&Instruction::I32Const(BUFFER_HEADER_SIZE));
    out.instruction(&Instruction::I32Add);
    out.instruction(&Instruction::End);
    out
}

/// Push the element-count of the typed-array data pointer on the stack.
pub(crate) fn emit_buffer_len_from_stack(out: &mut Function) {
    out.instruction(&Instruction::I32Const(BUFFER_HEADER_SIZE));
    out.instruction(&Instruction::I32Sub);
    out.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
        offset: 0,
        align: 2,
        memory_index: 0,
    }));
}

/// Emit the bounds check `index u>= len → throw` followed by the element
/// address computation, leaving `ptr + (index << log2)` on the stack.
/// `buffer_local`/`index_local` are the operands' local slots.
/// `oob_message_global` is the string-constant global holding the
/// out-of-bounds error message thrown with the Lua error tag (catchable by
/// `pcall`, unlike the trap this used to be).
pub(crate) fn emit_buffer_element_address(
    out: &mut Function,
    kind: TypedArrayKind,
    buffer_local: u32,
    index_local: u32,
    oob_message_global: u32,
) {
    // Unsigned compare rejects negative indices as huge values.
    out.instruction(&Instruction::LocalGet(index_local));
    out.instruction(&Instruction::LocalGet(buffer_local));
    emit_buffer_len_from_stack(out);
    out.instruction(&Instruction::I32GeU);
    out.instruction(&Instruction::If(BlockType::Empty));
    out.instruction(&Instruction::GlobalGet(oob_message_global));
    out.instruction(&Instruction::AnyConvertExtern);
    out.instruction(&Instruction::Throw(crate::ERROR_TAG_INDEX));
    out.instruction(&Instruction::End);

    out.instruction(&Instruction::LocalGet(buffer_local));
    out.instruction(&Instruction::LocalGet(index_local));
    let log2 = element_size_log2(kind);
    if log2 > 0 {
        out.instruction(&Instruction::I32Const(log2));
        out.instruction(&Instruction::I32Shl);
    }
    out.instruction(&Instruction::I32Add);
}

fn mem_arg(kind: TypedArrayKind, offset: u64) -> wasm_encoder::MemArg {
    wasm_encoder::MemArg {
        offset,
        align: element_size_log2(kind) as u32,
        memory_index: 0,
    }
}

/// Emit the typed load for one element; expects the element address on the
/// stack. Sub-word integers widen to their 32-bit signedness.
pub(crate) fn emit_buffer_load(out: &mut Function, kind: TypedArrayKind, offset: u64) {
    let arg = mem_arg(kind, offset);
    match kind {
        TypedArrayKind::I8 => out.instruction(&Instruction::I32Load8S(arg)),
        TypedArrayKind::U8 => out.instruction(&Instruction::I32Load8U(arg)),
        TypedArrayKind::I16 => out.instruction(&Instruction::I32Load16S(arg)),
        TypedArrayKind::U16 => out.instruction(&Instruction::I32Load16U(arg)),
        TypedArrayKind::I32 | TypedArrayKind::U32 => out.instruction(&Instruction::I32Load(arg)),
        TypedArrayKind::F32 => out.instruction(&Instruction::F32Load(arg)),
        TypedArrayKind::F64 => out.instruction(&Instruction::F64Load(arg)),
    };
}

/// Emit the typed store for one element; expects the element address and the
/// value on the stack. Sub-word integer stores truncate.
pub(crate) fn emit_buffer_store(out: &mut Function, kind: TypedArrayKind, offset: u64) {
    let arg = mem_arg(kind, offset);
    match kind {
        TypedArrayKind::I8 | TypedArrayKind::U8 => {
            out.instruction(&Instruction::I32Store8(arg));
        }
        TypedArrayKind::I16 | TypedArrayKind::U16 => {
            out.instruction(&Instruction::I32Store16(arg));
        }
        TypedArrayKind::I32 | TypedArrayKind::U32 => {
            out.instruction(&Instruction::I32Store(arg));
        }
        TypedArrayKind::F32 => {
            out.instruction(&Instruction::F32Store(arg));
        }
        TypedArrayKind::F64 => {
            out.instruction(&Instruction::F64Store(arg));
        }
    };
}
