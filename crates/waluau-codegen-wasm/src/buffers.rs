//! Linear-memory typed arrays (`Float32Array` & friends) and Luau buffers.
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
//!
//! Luau buffers deliberately use a distinct Wasm-GC handle containing a raw
//! data pointer and byte length. Their bytes share this browser module's bump
//! heap, but they do not have typed-array headers or typed-array semantics.
//! Allocations live for the lifetime of the module; there is no free API.

use waluau_ast::{Type, TypedArrayKind};
use waluau_diagnostics::Diagnostic;
use waluau_ir::{Instruction as IrInstruction, Module};
use wasm_encoder::{BlockType, Function, Instruction, ValType};

use crate::locals::BufferBitScratch;

/// Byte offset from the data pointer back to the element-count header field.
pub(crate) const BUFFER_HEADER_SIZE: i32 = 8;

/// First byte of the bump heap. Leaves address 0 unused so a zeroed pointer
/// never aliases a live allocation.
pub(crate) const BUFFER_HEAP_BASE: i32 = 16;

/// The wasm export name of the linear memory (JS view helpers read it).
pub(crate) const MEMORY_EXPORT_NAME: &str = "memory";
pub(crate) const LUAU_BUFFER_DATA_FIELD: u32 = 0;
pub(crate) const LUAU_BUFFER_LEN_FIELD: u32 = 1;

#[derive(Clone, Copy)]
pub(crate) struct BufferBitContext {
    pub(crate) buffer_type: u32,
    pub(crate) buffer_local: u32,
    pub(crate) bit_offset_local: u32,
    pub(crate) bit_count_local: u32,
    pub(crate) scratch: BufferBitScratch,
    pub(crate) oob_message_global: u32,
    pub(crate) count_message_global: u32,
}

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
#[derive(Clone, Default)]
pub(crate) struct BufferPlan {
    pub(crate) uses_memory: bool,
    pub(crate) uses_typed_arrays: bool,
    pub(crate) uses_luau_buffer: bool,
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
                            plan.uses_typed_arrays = true;
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
                            plan.uses_typed_arrays = true;
                        }
                        IrInstruction::LuauBufferNew { .. }
                        | IrInstruction::LuauBufferLen { .. }
                        | IrInstruction::LuauBufferGet { .. }
                        | IrInstruction::LuauBufferSet { .. }
                        | IrInstruction::LuauBufferReadBits { .. }
                        | IrInstruction::LuauBufferWriteBits { .. }
                        | IrInstruction::LuauBufferFromString { .. }
                        | IrInstruction::LuauBufferToString { .. }
                        | IrInstruction::LuauBufferReadString { .. }
                        | IrInstruction::LuauBufferWriteString { .. }
                        | IrInstruction::LuauBufferCopy { .. }
                        | IrInstruction::LuauBufferFill { .. } => {
                            plan.uses_memory = true;
                            plan.uses_luau_buffer = true;
                        }
                        _ => {}
                    }
                }
            }
        }
        for global in &module.globals {
            plan.uses_luau_buffer |= type_contains_buffer(&global.ty);
        }
        for function in &module.functions {
            plan.uses_luau_buffer |= function
                .params
                .iter()
                .any(|(_, ty)| type_contains_buffer(ty));
            plan.uses_luau_buffer |= type_contains_buffer(&function.return_type);
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

fn type_contains_buffer(ty: &Type) -> bool {
    match ty {
        Type::Buffer => true,
        Type::Array(inner)
        | Type::Variadic(inner)
        | Type::Nullable(inner)
        | Type::ExternSubtype(inner) => type_contains_buffer(inner),
        Type::Opaque { ty, .. } => type_contains_buffer(ty),
        Type::Multi(types) => types.iter().any(type_contains_buffer),
        Type::Function {
            params,
            return_type,
            ..
        } => params.iter().any(type_contains_buffer) || type_contains_buffer(return_type),
        Type::Record(fields) => fields.values().any(type_contains_buffer),
        Type::Named { type_args, .. } => type_args.iter().any(type_contains_buffer),
        Type::TaggedVariant(variant) => type_contains_buffer(&variant.payload),
        Type::TaggedUnion(variants) => variants
            .iter()
            .any(|variant| type_contains_buffer(&variant.payload)),
        _ => false,
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

/// Emit the checked raw-byte allocator used by `buffer.create`.
/// Invalid sizes and allocation failures use the Lua exception tag so `pcall`
/// can observe them; unlike typed-array allocation this path never traps.
pub(crate) fn emit_luau_buffer_alloc_function(
    heap_ptr_global: u32,
    invalid_size_global: u32,
    allocation_failed_global: u32,
) -> Function {
    // param 0=len; locals 1=header, 2=end, 3=current_bytes, 4=pages_needed
    let mut out = Function::new(vec![(4, ValType::I32)]);
    let header = 1u32;
    let end = 2u32;
    let current_bytes = 3u32;
    let pages_needed = 4u32;

    // Unsigned comparison rejects negative i32 lengths too.
    out.instruction(&Instruction::LocalGet(0));
    out.instruction(&Instruction::I32Const(0x4000_0000));
    out.instruction(&Instruction::I32GtU);
    out.instruction(&Instruction::If(BlockType::Empty));
    emit_lua_error(&mut out, invalid_size_global);
    out.instruction(&Instruction::End);

    // Align the shared heap to 8 bytes. Detect wrap before committing it.
    out.instruction(&Instruction::GlobalGet(heap_ptr_global));
    out.instruction(&Instruction::I32Const(7));
    out.instruction(&Instruction::I32Add);
    out.instruction(&Instruction::I32Const(!7));
    out.instruction(&Instruction::I32And);
    out.instruction(&Instruction::LocalTee(header));
    out.instruction(&Instruction::GlobalGet(heap_ptr_global));
    out.instruction(&Instruction::I32LtU);
    out.instruction(&Instruction::If(BlockType::Empty));
    emit_lua_error(&mut out, allocation_failed_global);
    out.instruction(&Instruction::End);

    // end = header + len. Buffer handles carry their length, so raw buffers do
    // not need the typed-array header. Unsigned wrap is an allocation failure.
    out.instruction(&Instruction::LocalGet(header));
    out.instruction(&Instruction::LocalGet(0));
    out.instruction(&Instruction::I32Add);
    out.instruction(&Instruction::LocalTee(end));
    out.instruction(&Instruction::LocalGet(header));
    out.instruction(&Instruction::I32LtU);
    out.instruction(&Instruction::If(BlockType::Empty));
    emit_lua_error(&mut out, allocation_failed_global);
    out.instruction(&Instruction::End);

    out.instruction(&Instruction::MemorySize(0));
    out.instruction(&Instruction::I32Const(16));
    out.instruction(&Instruction::I32Shl);
    out.instruction(&Instruction::LocalSet(current_bytes));
    out.instruction(&Instruction::LocalGet(end));
    out.instruction(&Instruction::LocalGet(current_bytes));
    out.instruction(&Instruction::I32GtU);
    out.instruction(&Instruction::If(BlockType::Empty));
    // ceil((end-current_bytes)/64KiB), written without an overflowing +65535.
    out.instruction(&Instruction::LocalGet(end));
    out.instruction(&Instruction::LocalGet(current_bytes));
    out.instruction(&Instruction::I32Sub);
    out.instruction(&Instruction::I32Const(1));
    out.instruction(&Instruction::I32Sub);
    out.instruction(&Instruction::I32Const(16));
    out.instruction(&Instruction::I32ShrU);
    out.instruction(&Instruction::I32Const(1));
    out.instruction(&Instruction::I32Add);
    out.instruction(&Instruction::LocalTee(pages_needed));
    out.instruction(&Instruction::MemoryGrow(0));
    out.instruction(&Instruction::I32Const(-1));
    out.instruction(&Instruction::I32Eq);
    out.instruction(&Instruction::If(BlockType::Empty));
    emit_lua_error(&mut out, allocation_failed_global);
    out.instruction(&Instruction::End);
    out.instruction(&Instruction::End);

    out.instruction(&Instruction::LocalGet(end));
    out.instruction(&Instruction::GlobalSet(heap_ptr_global));
    out.instruction(&Instruction::LocalGet(header));
    out.instruction(&Instruction::End);
    out
}

pub(crate) fn emit_lua_error(out: &mut Function, message_global: u32) {
    out.instruction(&Instruction::GlobalGet(message_global));
    out.instruction(&Instruction::AnyConvertExtern);
    out.instruction(&Instruction::Throw(crate::ERROR_TAG_INDEX));
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

fn unaligned_mem_arg(offset: u64) -> wasm_encoder::MemArg {
    wasm_encoder::MemArg {
        offset,
        align: 0,
        memory_index: 0,
    }
}

pub(crate) fn emit_luau_buffer_load(out: &mut Function, kind: TypedArrayKind) {
    let arg = unaligned_mem_arg(0);
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

pub(crate) fn emit_luau_buffer_store(out: &mut Function, kind: TypedArrayKind) {
    let arg = unaligned_mem_arg(0);
    match kind {
        TypedArrayKind::I8 | TypedArrayKind::U8 => out.instruction(&Instruction::I32Store8(arg)),
        TypedArrayKind::I16 | TypedArrayKind::U16 => out.instruction(&Instruction::I32Store16(arg)),
        TypedArrayKind::I32 | TypedArrayKind::U32 => out.instruction(&Instruction::I32Store(arg)),
        TypedArrayKind::F32 => out.instruction(&Instruction::F32Store(arg)),
        TypedArrayKind::F64 => out.instruction(&Instruction::F64Store(arg)),
    };
}

pub(crate) fn emit_luau_buffer_address(
    out: &mut Function,
    buffer_type: u32,
    buffer_local: u32,
    offset_local: u32,
    kind: TypedArrayKind,
    oob_message_global: u32,
) {
    let width = kind.element_size() as i32;
    // First ensure len >= width so len-width cannot underflow.
    out.instruction(&Instruction::LocalGet(buffer_local));
    out.instruction(&Instruction::StructGet {
        struct_type_index: buffer_type,
        field_index: LUAU_BUFFER_LEN_FIELD,
    });
    out.instruction(&Instruction::I32Const(width));
    out.instruction(&Instruction::I32LtU);
    out.instruction(&Instruction::If(BlockType::Empty));
    emit_lua_error(out, oob_message_global);
    out.instruction(&Instruction::End);

    // Unsigned offset > len-width rejects both negative and wrapping offsets.
    out.instruction(&Instruction::LocalGet(offset_local));
    out.instruction(&Instruction::LocalGet(buffer_local));
    out.instruction(&Instruction::StructGet {
        struct_type_index: buffer_type,
        field_index: LUAU_BUFFER_LEN_FIELD,
    });
    out.instruction(&Instruction::I32Const(width));
    out.instruction(&Instruction::I32Sub);
    out.instruction(&Instruction::I32GtU);
    out.instruction(&Instruction::If(BlockType::Empty));
    emit_lua_error(out, oob_message_global);
    out.instruction(&Instruction::End);

    out.instruction(&Instruction::LocalGet(buffer_local));
    out.instruction(&Instruction::StructGet {
        struct_type_index: buffer_type,
        field_index: LUAU_BUFFER_DATA_FIELD,
    });
    out.instruction(&Instruction::LocalGet(offset_local));
    out.instruction(&Instruction::I32Add);
}

/// Validate a Luau buffer bit range and initialize the shared scratch locals.
/// Offset/count deliberately remain f64 until after all range checks so the
/// 1-GiB limit's final valid bit offset (2^33) is representable.
fn emit_luau_buffer_bit_range(out: &mut Function, context: BufferBitContext) {
    let BufferBitContext {
        buffer_type,
        buffer_local,
        bit_offset_local,
        bit_count_local,
        scratch,
        oob_message_global,
        count_message_global,
    } = context;
    // count must be an exact integer in [0, 32]. floor(count) != count also
    // rejects NaN without risking a trapping conversion.
    out.instruction(&Instruction::LocalGet(bit_count_local));
    out.instruction(&Instruction::F64Const(0.0));
    out.instruction(&Instruction::F64Lt);
    out.instruction(&Instruction::If(BlockType::Empty));
    emit_lua_error(out, count_message_global);
    out.instruction(&Instruction::End);
    out.instruction(&Instruction::LocalGet(bit_count_local));
    out.instruction(&Instruction::F64Const(32.0));
    out.instruction(&Instruction::F64Gt);
    out.instruction(&Instruction::If(BlockType::Empty));
    emit_lua_error(out, count_message_global);
    out.instruction(&Instruction::End);
    out.instruction(&Instruction::LocalGet(bit_count_local));
    out.instruction(&Instruction::F64Floor);
    out.instruction(&Instruction::LocalGet(bit_count_local));
    out.instruction(&Instruction::F64Ne);
    out.instruction(&Instruction::If(BlockType::Empty));
    emit_lua_error(out, count_message_global);
    out.instruction(&Instruction::End);

    // Offset must be a nonnegative exact integer, and offset+count must fit in
    // len*8. This permits offset==len*8 only when count==0.
    out.instruction(&Instruction::LocalGet(bit_offset_local));
    out.instruction(&Instruction::F64Const(0.0));
    out.instruction(&Instruction::F64Lt);
    out.instruction(&Instruction::If(BlockType::Empty));
    emit_lua_error(out, oob_message_global);
    out.instruction(&Instruction::End);
    out.instruction(&Instruction::LocalGet(bit_offset_local));
    out.instruction(&Instruction::F64Floor);
    out.instruction(&Instruction::LocalGet(bit_offset_local));
    out.instruction(&Instruction::F64Ne);
    out.instruction(&Instruction::If(BlockType::Empty));
    emit_lua_error(out, oob_message_global);
    out.instruction(&Instruction::End);
    out.instruction(&Instruction::LocalGet(bit_offset_local));
    out.instruction(&Instruction::LocalGet(bit_count_local));
    out.instruction(&Instruction::F64Add);
    out.instruction(&Instruction::LocalGet(buffer_local));
    out.instruction(&Instruction::StructGet {
        struct_type_index: buffer_type,
        field_index: LUAU_BUFFER_LEN_FIELD,
    });
    out.instruction(&Instruction::F64ConvertI32U);
    out.instruction(&Instruction::F64Const(8.0));
    out.instruction(&Instruction::F64Mul);
    out.instruction(&Instruction::F64Gt);
    out.instruction(&Instruction::If(BlockType::Empty));
    emit_lua_error(out, oob_message_global);
    out.instruction(&Instruction::End);

    // The validated offset is at most 2^33. Keep it i64 long enough to derive
    // both the byte address and the within-byte shift without wrapping.
    out.instruction(&Instruction::LocalGet(bit_offset_local));
    out.instruction(&Instruction::I64TruncF64U);
    out.instruction(&Instruction::I64Const(3));
    out.instruction(&Instruction::I64ShrU);
    out.instruction(&Instruction::I32WrapI64);
    out.instruction(&Instruction::LocalSet(scratch.byte_offset));
    out.instruction(&Instruction::LocalGet(bit_offset_local));
    out.instruction(&Instruction::I64TruncF64U);
    out.instruction(&Instruction::I32WrapI64);
    out.instruction(&Instruction::I32Const(7));
    out.instruction(&Instruction::I32And);
    out.instruction(&Instruction::LocalSet(scratch.bit_shift));
    out.instruction(&Instruction::LocalGet(bit_count_local));
    out.instruction(&Instruction::I32TruncF64U);
    out.instruction(&Instruction::LocalSet(scratch.bit_count));

    out.instruction(&Instruction::I32Const(0));
    out.instruction(&Instruction::LocalSet(scratch.byte_count));
    out.instruction(&Instruction::LocalGet(scratch.bit_count));
    out.instruction(&Instruction::I32Eqz);
    out.instruction(&Instruction::If(BlockType::Empty));
    out.instruction(&Instruction::Else);
    out.instruction(&Instruction::LocalGet(scratch.bit_shift));
    out.instruction(&Instruction::LocalGet(scratch.bit_count));
    out.instruction(&Instruction::I32Add);
    out.instruction(&Instruction::I32Const(7));
    out.instruction(&Instruction::I32Add);
    out.instruction(&Instruction::I32Const(3));
    out.instruction(&Instruction::I32ShrU);
    out.instruction(&Instruction::LocalSet(scratch.byte_count));
    out.instruction(&Instruction::End);
}

fn emit_luau_buffer_bit_window_load(
    out: &mut Function,
    buffer_type: u32,
    buffer_local: u32,
    scratch: BufferBitScratch,
) {
    out.instruction(&Instruction::I64Const(0));
    out.instruction(&Instruction::LocalSet(scratch.window));
    for byte in 0..5i32 {
        out.instruction(&Instruction::LocalGet(scratch.byte_count));
        out.instruction(&Instruction::I32Const(byte));
        out.instruction(&Instruction::I32GtU);
        out.instruction(&Instruction::If(BlockType::Empty));
        out.instruction(&Instruction::LocalGet(scratch.window));
        out.instruction(&Instruction::LocalGet(buffer_local));
        out.instruction(&Instruction::StructGet {
            struct_type_index: buffer_type,
            field_index: LUAU_BUFFER_DATA_FIELD,
        });
        out.instruction(&Instruction::LocalGet(scratch.byte_offset));
        out.instruction(&Instruction::I32Add);
        if byte != 0 {
            out.instruction(&Instruction::I32Const(byte));
            out.instruction(&Instruction::I32Add);
        }
        out.instruction(&Instruction::I32Load8U(unaligned_mem_arg(0)));
        out.instruction(&Instruction::I64ExtendI32U);
        if byte != 0 {
            out.instruction(&Instruction::I64Const(i64::from(byte * 8)));
            out.instruction(&Instruction::I64Shl);
        }
        out.instruction(&Instruction::I64Or);
        out.instruction(&Instruction::LocalSet(scratch.window));
        out.instruction(&Instruction::End);
    }
}

fn emit_luau_buffer_bit_mask(out: &mut Function, scratch: BufferBitScratch) {
    // Build the unshifted mask without a shift-by-32 edge case, then place it
    // over the selected field in the at-most-40-bit window.
    out.instruction(&Instruction::I64Const(0xffff_ffff));
    out.instruction(&Instruction::LocalSet(scratch.mask));
    out.instruction(&Instruction::LocalGet(scratch.bit_count));
    out.instruction(&Instruction::I32Const(32));
    out.instruction(&Instruction::I32Ne);
    out.instruction(&Instruction::If(BlockType::Empty));
    out.instruction(&Instruction::I64Const(1));
    out.instruction(&Instruction::LocalGet(scratch.bit_count));
    out.instruction(&Instruction::I64ExtendI32U);
    out.instruction(&Instruction::I64Shl);
    out.instruction(&Instruction::I64Const(1));
    out.instruction(&Instruction::I64Sub);
    out.instruction(&Instruction::LocalSet(scratch.mask));
    out.instruction(&Instruction::End);
}

pub(crate) fn emit_luau_buffer_read_bits(out: &mut Function, context: BufferBitContext) {
    let BufferBitContext {
        buffer_type,
        buffer_local,
        scratch,
        ..
    } = context;
    emit_luau_buffer_bit_range(out, context);
    emit_luau_buffer_bit_window_load(out, buffer_type, buffer_local, scratch);
    emit_luau_buffer_bit_mask(out, scratch);
    out.instruction(&Instruction::LocalGet(scratch.window));
    out.instruction(&Instruction::LocalGet(scratch.bit_shift));
    out.instruction(&Instruction::I64ExtendI32U);
    out.instruction(&Instruction::I64ShrU);
    out.instruction(&Instruction::LocalGet(scratch.mask));
    out.instruction(&Instruction::I64And);
    out.instruction(&Instruction::I32WrapI64);
}

pub(crate) fn emit_luau_buffer_write_bits(
    out: &mut Function,
    context: BufferBitContext,
    value_local: u32,
) {
    let BufferBitContext {
        buffer_type,
        buffer_local,
        scratch,
        ..
    } = context;
    emit_luau_buffer_bit_range(out, context);
    emit_luau_buffer_bit_window_load(out, buffer_type, buffer_local, scratch);
    emit_luau_buffer_bit_mask(out, scratch);

    // Shift the mask into position and merge only the low requested bits.
    out.instruction(&Instruction::LocalGet(scratch.mask));
    out.instruction(&Instruction::LocalGet(scratch.bit_shift));
    out.instruction(&Instruction::I64ExtendI32U);
    out.instruction(&Instruction::I64Shl);
    out.instruction(&Instruction::LocalSet(scratch.mask));
    out.instruction(&Instruction::LocalGet(scratch.window));
    out.instruction(&Instruction::LocalGet(scratch.mask));
    out.instruction(&Instruction::I64Const(-1));
    out.instruction(&Instruction::I64Xor);
    out.instruction(&Instruction::I64And);
    out.instruction(&Instruction::LocalGet(value_local));
    out.instruction(&Instruction::I64ExtendI32U);
    out.instruction(&Instruction::LocalGet(scratch.bit_shift));
    out.instruction(&Instruction::I64ExtendI32U);
    out.instruction(&Instruction::I64Shl);
    out.instruction(&Instruction::LocalGet(scratch.mask));
    out.instruction(&Instruction::I64And);
    out.instruction(&Instruction::I64Or);
    out.instruction(&Instruction::LocalSet(scratch.window));

    for byte in 0..5i32 {
        out.instruction(&Instruction::LocalGet(scratch.byte_count));
        out.instruction(&Instruction::I32Const(byte));
        out.instruction(&Instruction::I32GtU);
        out.instruction(&Instruction::If(BlockType::Empty));
        out.instruction(&Instruction::LocalGet(buffer_local));
        out.instruction(&Instruction::StructGet {
            struct_type_index: buffer_type,
            field_index: LUAU_BUFFER_DATA_FIELD,
        });
        out.instruction(&Instruction::LocalGet(scratch.byte_offset));
        out.instruction(&Instruction::I32Add);
        if byte != 0 {
            out.instruction(&Instruction::I32Const(byte));
            out.instruction(&Instruction::I32Add);
        }
        out.instruction(&Instruction::LocalGet(scratch.window));
        if byte != 0 {
            out.instruction(&Instruction::I64Const(i64::from(byte * 8)));
            out.instruction(&Instruction::I64ShrU);
        }
        out.instruction(&Instruction::I32WrapI64);
        out.instruction(&Instruction::I32Store8(unaligned_mem_arg(0)));
        out.instruction(&Instruction::End);
    }
}

/// Validate an arbitrary byte range without forming `offset + count`, which
/// could wrap. Zero-length ranges at the one-past-end offset are valid.
pub(crate) fn emit_luau_buffer_range_check(
    out: &mut Function,
    buffer_type: u32,
    buffer_local: u32,
    offset_local: u32,
    count_local: u32,
    oob_message_global: u32,
) {
    // Unsigned `count > len` rejects negative counts and avoids underflow in
    // the following `len - count` expression.
    out.instruction(&Instruction::LocalGet(count_local));
    out.instruction(&Instruction::LocalGet(buffer_local));
    out.instruction(&Instruction::StructGet {
        struct_type_index: buffer_type,
        field_index: LUAU_BUFFER_LEN_FIELD,
    });
    out.instruction(&Instruction::I32GtU);
    out.instruction(&Instruction::If(BlockType::Empty));
    emit_lua_error(out, oob_message_global);
    out.instruction(&Instruction::End);

    // Unsigned comparison also rejects negative offsets.
    out.instruction(&Instruction::LocalGet(offset_local));
    out.instruction(&Instruction::LocalGet(buffer_local));
    out.instruction(&Instruction::StructGet {
        struct_type_index: buffer_type,
        field_index: LUAU_BUFFER_LEN_FIELD,
    });
    out.instruction(&Instruction::LocalGet(count_local));
    out.instruction(&Instruction::I32Sub);
    out.instruction(&Instruction::I32GtU);
    out.instruction(&Instruction::If(BlockType::Empty));
    emit_lua_error(out, oob_message_global);
    out.instruction(&Instruction::End);
}

pub(crate) fn emit_luau_buffer_data_address(
    out: &mut Function,
    buffer_type: u32,
    buffer_local: u32,
    offset_local: u32,
) {
    out.instruction(&Instruction::LocalGet(buffer_local));
    out.instruction(&Instruction::StructGet {
        struct_type_index: buffer_type,
        field_index: LUAU_BUFFER_DATA_FIELD,
    });
    out.instruction(&Instruction::LocalGet(offset_local));
    out.instruction(&Instruction::I32Add);
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
