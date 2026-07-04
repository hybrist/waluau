//! Host import ABI shared by codegen and runtimes (driver, browser).

use std::collections::HashMap;

use waluau_ast::{BinaryOp, NumericType, Type};
use waluau_diagnostics::Diagnostic;
use waluau_ir::{Function as IrFunction, Instruction as IrInstruction, Module};

pub const IMPORTED_STRING_CONSTANTS_MODULE: &str = "string_constants";
pub const JS_STRING_BUILTINS_MODULE: &str = "wasm:js-string";
pub const IMPORT_MODULE: &str = "waluau";
pub const BYTES_CUSTOM_SECTION_NAME: &str = "waluau.bytc";

pub const IMPORT_JS_STRING_EQ: &str = "equals";
pub const IMPORT_JS_STRING_CONCAT: &str = "concat";
pub const IMPORT_JS_STRING_COMPARE: &str = "compare";
pub const IMPORT_BYTES_LITERAL: &str = "bytes_literal";
pub const IMPORT_BYTES_GET: &str = "bytes_get";
pub const IMPORT_BYTES_LEN: &str = "bytes_len";
pub const IMPORT_BYTES_CONCAT: &str = "bytes_concat";
pub const IMPORT_BYTES_EQ: &str = "bytes_eq";
pub const IMPORT_BYTES_COMPARE: &str = "bytes_compare";
pub const IMPORT_JS_TOSTRING_I32: &str = "js_tostring_i32";
pub const IMPORT_JS_TOSTRING_U32: &str = "js_tostring_u32";
pub const IMPORT_JS_TOSTRING_I64: &str = "js_tostring_i64";
pub const IMPORT_JS_TOSTRING_U64: &str = "js_tostring_u64";
pub const IMPORT_JS_TOSTRING_F32: &str = "js_tostring_f32";
pub const IMPORT_JS_TOSTRING_F64: &str = "js_tostring_f64";
pub const IMPORT_JS_TOSTRING_BOOL: &str = "js_tostring_bool";
pub const IMPORT_JS_TOSTRING_UNKNOWN: &str = "js_tostring_unknown";
pub const IMPORT_PRINT: &str = "print";
pub const IMPORT_EXTERN_IS: &str = "extern_is";
pub const IMPORT_ATTACH_PROMISE: &str = "__waluau_attach_promise";
pub const IMPORT_MATH_POW: &str = "math_pow";

/// Maximum number of host function imports (when all are used).
pub const HOST_IMPORT_COUNT: u32 = 21;

/// Canonical function-index slot for each host import.
/// These are stable identifiers used as keys into [`HostImportMap`].
pub const IMPORT_JS_STRING_EQ_FUNC: u32 = 0;
pub const IMPORT_JS_STRING_CONCAT_FUNC: u32 = 1;
pub const IMPORT_JS_STRING_COMPARE_FUNC: u32 = 2;
pub const IMPORT_BYTES_LITERAL_FUNC: u32 = 3;
pub const IMPORT_BYTES_GET_FUNC: u32 = 4;
pub const IMPORT_BYTES_LEN_FUNC: u32 = 5;
pub const IMPORT_BYTES_CONCAT_FUNC: u32 = 6;
pub const IMPORT_BYTES_EQ_FUNC: u32 = 7;
pub const IMPORT_BYTES_COMPARE_FUNC: u32 = 8;
pub const IMPORT_PRINT_FUNC: u32 = 9;
pub const IMPORT_JS_TOSTRING_I32_FUNC: u32 = 10;
pub const IMPORT_JS_TOSTRING_U32_FUNC: u32 = 11;
pub const IMPORT_JS_TOSTRING_I64_FUNC: u32 = 12;
pub const IMPORT_JS_TOSTRING_U64_FUNC: u32 = 13;
pub const IMPORT_JS_TOSTRING_F32_FUNC: u32 = 14;
pub const IMPORT_JS_TOSTRING_F64_FUNC: u32 = 15;
pub const IMPORT_JS_TOSTRING_BOOL_FUNC: u32 = 16;
pub const IMPORT_JS_TOSTRING_UNKNOWN_FUNC: u32 = 17;
pub const IMPORT_EXTERN_IS_FUNC: u32 = 18;
pub const IMPORT_ATTACH_PROMISE_FUNC: u32 = 19;
pub const IMPORT_MATH_POW_FUNC: u32 = 20;

/// Number of host function types in the canonical type-slot table.
/// The actual number emitted in a given module may be less if some slots are unused.
pub const HOST_TYPE_COUNT: u32 = 11;

/// Records which host functions are actually referenced by a module.
#[derive(Clone, Debug, Default)]
pub struct UsedHostImports {
    pub js_string_eq: bool,
    pub js_string_concat: bool,
    pub js_string_compare: bool,
    pub bytes_literal: bool,
    pub bytes_get: bool,
    pub bytes_len: bool,
    pub bytes_concat: bool,
    pub bytes_eq: bool,
    pub bytes_compare: bool,
    pub print: bool,
    pub js_tostring_i32: bool,
    pub js_tostring_u32: bool,
    pub js_tostring_i64: bool,
    pub js_tostring_u64: bool,
    pub js_tostring_f32: bool,
    pub js_tostring_f64: bool,
    pub js_tostring_bool: bool,
    pub js_tostring_unknown: bool,
    pub extern_is: bool,
    pub attach_promise: bool,
    pub math_pow: bool,
}

/// Maps canonical host-import slot indices (0–16) to the actual Wasm function
/// indices they occupy in the output module's import section.
///
/// Slots whose corresponding import was not used hold `None`.
pub struct HostImportMap {
    /// `indices[canonical]` is `Some(actual_func_idx)` when the import is present.
    indices: [Option<u32>; HOST_IMPORT_COUNT as usize],
    /// Total number of host function imports actually emitted.
    pub count: u32,
}

impl HostImportMap {
    /// Look up the actual Wasm function index for a canonical slot.
    ///
    /// Returns an error if the import was not emitted (i.e., the module tries
    /// to call a host function it never declared a dependency on).
    pub fn func_index(&self, canonical: u32) -> Result<u32, Diagnostic> {
        self.indices[canonical as usize].ok_or_else(|| {
            Diagnostic::new(format!(
                "internal error: host import slot {canonical} used but not emitted"
            ))
        })
    }
}

impl UsedHostImports {
    /// Build the ordered import map: only used imports get consecutive indices.
    pub fn to_import_map(&self) -> HostImportMap {
        let ordered: [(u32, bool); HOST_IMPORT_COUNT as usize] = [
            (IMPORT_JS_STRING_EQ_FUNC, self.js_string_eq),
            (IMPORT_JS_STRING_CONCAT_FUNC, self.js_string_concat),
            (IMPORT_JS_STRING_COMPARE_FUNC, self.js_string_compare),
            (IMPORT_BYTES_LITERAL_FUNC, self.bytes_literal),
            (IMPORT_BYTES_GET_FUNC, self.bytes_get),
            (IMPORT_BYTES_LEN_FUNC, self.bytes_len),
            (IMPORT_BYTES_CONCAT_FUNC, self.bytes_concat),
            (IMPORT_BYTES_EQ_FUNC, self.bytes_eq),
            (IMPORT_BYTES_COMPARE_FUNC, self.bytes_compare),
            (IMPORT_PRINT_FUNC, self.print),
            (IMPORT_JS_TOSTRING_I32_FUNC, self.js_tostring_i32),
            (IMPORT_JS_TOSTRING_U32_FUNC, self.js_tostring_u32),
            (IMPORT_JS_TOSTRING_I64_FUNC, self.js_tostring_i64),
            (IMPORT_JS_TOSTRING_U64_FUNC, self.js_tostring_u64),
            (IMPORT_JS_TOSTRING_F32_FUNC, self.js_tostring_f32),
            (IMPORT_JS_TOSTRING_F64_FUNC, self.js_tostring_f64),
            (IMPORT_JS_TOSTRING_BOOL_FUNC, self.js_tostring_bool),
            (IMPORT_JS_TOSTRING_UNKNOWN_FUNC, self.js_tostring_unknown),
            (IMPORT_EXTERN_IS_FUNC, self.extern_is),
            (IMPORT_ATTACH_PROMISE_FUNC, self.attach_promise),
            (IMPORT_MATH_POW_FUNC, self.math_pow),
        ];
        let mut indices = [None; HOST_IMPORT_COUNT as usize];
        let mut next = 0u32;
        for (canonical, used) in ordered {
            if used {
                indices[canonical as usize] = Some(next);
                next += 1;
            }
        }
        HostImportMap {
            indices,
            count: next,
        }
    }
}

/// Returns a bitmask of which host type slots (0–8) are needed by `used`.
///
/// The 9 canonical host type slots correspond to the function signatures used
/// by host imports (in the order they appear in the type section):
///
/// | Slot | Signature                             | Used by                                      |
/// |------|---------------------------------------|----------------------------------------------|
/// | 0    | (externref, externref) → i32          | js_string_eq, js_string_compare, bytes_eq, bytes_compare, extern_is |
/// | 1    | (externref, externref) → externref_nn | js_string_concat, bytes_concat               |
/// | 2    | (i32) → externref                     | bytes_literal, js_tostring_i32/u32/bool      |
/// | 3    | (i64) → externref                     | js_tostring_i64, js_tostring_u64             |
/// | 4    | (f32) → externref                     | js_tostring_f32                              |
/// | 5    | (f64) → externref                     | js_tostring_f64                              |
/// | 6    | (externref) → []                      | print                                        |
/// | 7    | (externref, i32) → i32                | bytes_get                                    |
/// | 8    | (externref) → i32                     | bytes_len                                    |
/// | 9    | (anyref) → externref                  | js_tostring_unknown                         |
/// | 10   | (f64, f64) → f64                      | math_pow                                     |
pub fn needed_host_type_slots(used: &UsedHostImports) -> [bool; HOST_TYPE_COUNT as usize] {
    let mut slots = [false; HOST_TYPE_COUNT as usize];
    if used.js_string_eq
        || used.js_string_compare
        || used.bytes_eq
        || used.bytes_compare
        || used.extern_is
    {
        slots[0] = true;
    }
    if used.js_string_concat || used.bytes_concat {
        slots[1] = true;
    }
    if used.bytes_literal || used.js_tostring_i32 || used.js_tostring_u32 || used.js_tostring_bool {
        slots[2] = true;
    }
    if used.js_tostring_i64 || used.js_tostring_u64 {
        slots[3] = true;
    }
    if used.js_tostring_f32 {
        slots[4] = true;
    }
    if used.js_tostring_f64 {
        slots[5] = true;
    }
    if used.js_tostring_unknown {
        slots[9] = true;
    }
    if used.print {
        slots[6] = true;
    }
    if used.bytes_get {
        slots[7] = true;
    }
    if used.bytes_len {
        slots[8] = true;
    }
    if used.math_pow {
        slots[10] = true;
    }
    slots
}

/// Scan `module` and return the set of host functions it actually references.
pub fn collect_used_host_imports(module: &Module) -> UsedHostImports {
    let mut used = UsedHostImports::default();
    for function in &module.functions {
        for block in function.blocks.values() {
            for (_, instruction) in &block.instructions {
                mark_used_by_instruction(instruction, &mut used);
            }
            mark_used_by_terminator(&block.terminator, &mut used);
        }
    }
    used
}

fn mark_used_by_instruction(instruction: &IrInstruction, used: &mut UsedHostImports) {
    match instruction {
        IrInstruction::Bytes(_) => {
            used.bytes_literal = true;
        }
        IrInstruction::BytesGet { .. } => {
            used.bytes_get = true;
        }
        IrInstruction::BytesLen { .. } => {
            used.bytes_len = true;
        }
        IrInstruction::Print { .. } => {
            used.print = true;
        }
        IrInstruction::ExternCastTest { .. } => {
            used.extern_is = true;
        }
        IrInstruction::ToString { from, .. } => match from {
            Type::Numeric(NumericType::I32) => used.js_tostring_i32 = true,
            Type::Numeric(NumericType::U32) => used.js_tostring_u32 = true,
            Type::Numeric(NumericType::I64) => used.js_tostring_i64 = true,
            Type::Numeric(NumericType::U64) => used.js_tostring_u64 = true,
            Type::Numeric(NumericType::F32) => used.js_tostring_f32 = true,
            Type::Numeric(NumericType::F64) => used.js_tostring_f64 = true,
            Type::Bool => used.js_tostring_bool = true,
            Type::Unknown => {
                used.js_tostring_unknown = true;
                // The unknown path dispatches boxed f64 values to the f64
                // stringifier when closure GC types are present.
                used.js_tostring_f64 = true;
            }
            _ => {}
        },
        IrInstruction::Binary { op, operand_ty, .. } => match (op, operand_ty) {
            (BinaryOp::Eq, Type::String) => used.js_string_eq = true,
            (BinaryOp::Eq, Type::Bytes) => used.bytes_eq = true,
            (BinaryOp::Concat, Type::String) => used.js_string_concat = true,
            (BinaryOp::Concat, Type::Bytes) => used.bytes_concat = true,
            (
                BinaryOp::Less | BinaryOp::LessEq | BinaryOp::Greater | BinaryOp::GreaterEq,
                Type::String,
            ) => {
                used.js_string_compare = true;
            }
            (
                BinaryOp::Less | BinaryOp::LessEq | BinaryOp::Greater | BinaryOp::GreaterEq,
                Type::Bytes,
            ) => {
                used.bytes_compare = true;
            }
            (BinaryOp::Pow, _) => used.math_pow = true,
            _ => {}
        },
        _ => {}
    }
}

fn mark_used_by_terminator(terminator: &waluau_ir::Terminator, used: &mut UsedHostImports) {
    if matches!(
        terminator,
        waluau_ir::Terminator::CoroutineAwaitPromise { .. }
    ) {
        used.attach_promise = true;
    }
}

pub fn encode_bytes_constants_section(values: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    push_u32(&mut out, values.len() as u32);
    for value in values {
        push_u32(&mut out, value.len() as u32);
        out.extend_from_slice(value);
    }
    out
}

pub fn decode_bytes_constants_section(data: &[u8]) -> Result<Vec<Vec<u8>>, Diagnostic> {
    let mut offset = 0usize;
    let count = read_u32(data, &mut offset)?;
    let mut values = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let len = read_u32(data, &mut offset)? as usize;
        let end = offset
            .checked_add(len)
            .ok_or_else(|| Diagnostic::new("waluau.bytc section length overflow"))?;
        if end > data.len() {
            return Err(Diagnostic::new("waluau.bytc section truncated"));
        }
        values.push(data[offset..end].to_vec());
        offset = end;
    }
    if offset != data.len() {
        return Err(Diagnostic::new("waluau.bytc section has trailing bytes"));
    }
    Ok(values)
}

/// Collect every string literal in `module`, deduplicated in first-seen order.
pub fn collect_string_constants(module: &Module) -> Vec<String> {
    let mut strings = Vec::new();
    let mut indices = HashMap::<&str, u32>::new();
    for function in &module.functions {
        collect_from_function(function, &mut strings, &mut indices);
    }
    strings
}

pub fn collect_bytes_constants(module: &Module) -> Vec<Vec<u8>> {
    let mut values = Vec::new();
    let mut indices = HashMap::<Vec<u8>, u32>::new();
    for function in &module.functions {
        collect_bytes_from_function(function, &mut values, &mut indices);
    }
    values
}

fn collect_from_function<'a>(
    function: &'a IrFunction,
    strings: &mut Vec<String>,
    indices: &mut HashMap<&'a str, u32>,
) {
    for block in function.blocks.values() {
        for (_, instruction) in &block.instructions {
            match instruction {
                IrInstruction::String(literal) => {
                    if indices
                        .insert(literal.as_str(), strings.len() as u32)
                        .is_none()
                    {
                        strings.push(literal.clone());
                    }
                }
                IrInstruction::ExternCastTest { target_name, .. }
                    if indices
                        .insert(target_name.as_str(), strings.len() as u32)
                        .is_none() =>
                {
                    strings.push(target_name.clone());
                }
                _ => {}
            }
        }
    }
}

fn collect_bytes_from_function(
    function: &IrFunction,
    values: &mut Vec<Vec<u8>>,
    indices: &mut HashMap<Vec<u8>, u32>,
) {
    for block in function.blocks.values() {
        for (_, instruction) in &block.instructions {
            if let IrInstruction::Bytes(literal) = instruction
                && indices
                    .insert(literal.clone(), values.len() as u32)
                    .is_none()
            {
                values.push(literal.clone());
            }
        }
    }
}

pub fn string_constant_index(strings: &[String], literal: &str) -> Result<u32, Diagnostic> {
    strings
        .iter()
        .position(|value| value == literal)
        .map(|index| index as u32)
        .ok_or_else(|| {
            Diagnostic::new(format!(
                "missing string literal '{literal}' in wasm string constants"
            ))
        })
}

pub fn bytes_constant_index(values: &[Vec<u8>], literal: &[u8]) -> Result<u32, Diagnostic> {
    values
        .iter()
        .position(|value| value == literal)
        .map(|index| index as u32)
        .ok_or_else(|| Diagnostic::new("missing bytes literal in wasm bytes constants"))
}

/// Parse the `waluau.bytc` custom section from a compiled Wasm module.
pub fn parse_bytes_constants_from_wasm(wasm: &[u8]) -> Result<Vec<Vec<u8>>, Diagnostic> {
    match find_named_custom_section(wasm, BYTES_CUSTOM_SECTION_NAME)? {
        Some(data) => decode_bytes_constants_section(data),
        None => Ok(Vec::new()),
    }
}

fn find_named_custom_section<'a>(
    wasm: &'a [u8],
    section_name: &str,
) -> Result<Option<&'a [u8]>, Diagnostic> {
    let mut offset = 8usize;
    if wasm.len() < 8 || &wasm[0..4] != b"\0asm" {
        return Err(Diagnostic::new("input is not a wasm module"));
    }
    while offset < wasm.len() {
        let section_id = wasm[offset];
        offset += 1;
        let (section_len, mut section_offset) = read_varu32(wasm, offset)?;
        let section_end = section_offset
            .checked_add(section_len as usize)
            .ok_or_else(|| Diagnostic::new("wasm section length overflow"))?;
        if section_end > wasm.len() {
            return Err(Diagnostic::new("wasm section extends past end of module"));
        }
        if section_id == 0 {
            let (name_len, name_offset) = read_varu32(wasm, section_offset)?;
            section_offset = name_offset;
            let name_end = section_offset
                .checked_add(name_len as usize)
                .ok_or_else(|| Diagnostic::new("wasm custom section name overflow"))?;
            if name_end > section_end {
                return Err(Diagnostic::new("wasm custom section name truncated"));
            }
            let name = std::str::from_utf8(&wasm[section_offset..name_end])
                .map_err(|_| Diagnostic::new("wasm custom section name is not UTF-8"))?;
            if name == section_name {
                return Ok(Some(&wasm[name_end..section_end]));
            }
        }
        offset = section_end;
    }
    Ok(None)
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn read_u32(data: &[u8], offset: &mut usize) -> Result<u32, Diagnostic> {
    if *offset + 4 > data.len() {
        return Err(Diagnostic::new("waluau.strc section truncated"));
    }
    let bytes: [u8; 4] = data[*offset..*offset + 4]
        .try_into()
        .expect("slice length checked");
    *offset += 4;
    Ok(u32::from_le_bytes(bytes))
}

fn read_varu32(data: &[u8], mut offset: usize) -> Result<(u32, usize), Diagnostic> {
    let mut result = 0u32;
    let mut shift = 0u32;
    loop {
        if offset >= data.len() {
            return Err(Diagnostic::new("wasm leb128 extends past end of module"));
        }
        let byte = data[offset];
        offset += 1;
        result |= ((byte & 0x7f) as u32) << shift;
        if byte & 0x80 == 0 {
            return Ok((result, offset));
        }
        shift += 7;
        if shift > 35 {
            return Err(Diagnostic::new("wasm leb128 is too large"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_bytes_constants_section() {
        let values = vec![b"hello".to_vec(), vec![0, 255, 10], Vec::new()];
        let encoded = encode_bytes_constants_section(&values);
        let decoded = decode_bytes_constants_section(&encoded).expect("decode should succeed");
        assert_eq!(decoded, values);
    }
}
