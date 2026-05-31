//! Host import ABI shared by codegen and runtimes (driver, browser).

use std::collections::HashMap;

use waluau_diagnostics::Diagnostic;
use waluau_ir::{Function as IrFunction, Instruction as IrInstruction, Module};

pub const IMPORT_MODULE: &str = "waluau";
pub const CUSTOM_SECTION_NAME: &str = "waluau.strc";

pub const IMPORT_JS_STRING_CONST: &str = "js_string_const";
pub const IMPORT_JS_STRING_EQ: &str = "js_string_eq";
pub const IMPORT_JS_STRING_CONCAT: &str = "js_string_concat";
pub const IMPORT_PRINT: &str = "print";

/// Number of imported host functions emitted before user-defined functions.
pub const HOST_IMPORT_COUNT: u32 = 4;

/// Function index of the first user-defined function in the combined import+defined index space.
pub const fn defined_func_index(user_index: u32) -> u32 {
    HOST_IMPORT_COUNT + user_index
}

/// Index of `IMPORT_JS_STRING_CONST` in the combined function index space.
pub const IMPORT_JS_STRING_CONST_FUNC: u32 = 0;
/// Index of `IMPORT_JS_STRING_EQ` in the combined function index space.
pub const IMPORT_JS_STRING_EQ_FUNC: u32 = 1;
/// Index of `IMPORT_JS_STRING_CONCAT` in the combined function index space.
pub const IMPORT_JS_STRING_CONCAT_FUNC: u32 = 2;
/// Index of `IMPORT_PRINT` in the combined function index space.
pub const IMPORT_PRINT_FUNC: u32 = 3;

/// Number of host function types emitted after array types in the type section.
pub const HOST_TYPE_COUNT: u32 = 4;

pub fn encode_string_constants_section(strings: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    push_u32(&mut out, strings.len() as u32);
    for string in strings {
        let bytes = string.as_bytes();
        push_u32(&mut out, bytes.len() as u32);
        out.extend_from_slice(bytes);
    }
    out
}

pub fn decode_string_constants_section(data: &[u8]) -> Result<Vec<String>, Diagnostic> {
    let mut offset = 0usize;
    let count = read_u32(data, &mut offset)?;
    let mut strings = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let len = read_u32(data, &mut offset)? as usize;
        let end = offset
            .checked_add(len)
            .ok_or_else(|| Diagnostic::new("waluau.strc section length overflow"))?;
        if end > data.len() {
            return Err(Diagnostic::new("waluau.strc section truncated"));
        }
        let value = std::str::from_utf8(&data[offset..end])
            .map_err(|_| Diagnostic::new("waluau.strc section contains invalid UTF-8"))?
            .to_string();
        offset = end;
        strings.push(value);
    }
    if offset != data.len() {
        return Err(Diagnostic::new("waluau.strc section has trailing bytes"));
    }
    Ok(strings)
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

fn collect_from_function<'a>(
    function: &'a IrFunction,
    strings: &mut Vec<String>,
    indices: &mut HashMap<&'a str, u32>,
) {
    for block in function.blocks.values() {
        for (_, instruction) in &block.instructions {
            if let IrInstruction::String(literal) = instruction
                && indices
                    .insert(literal.as_str(), strings.len() as u32)
                    .is_none()
            {
                strings.push(literal.clone());
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

/// Parse the `waluau.strc` custom section from a compiled Wasm module.
pub fn parse_string_constants_from_wasm(wasm: &[u8]) -> Result<Vec<String>, Diagnostic> {
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
            if name == CUSTOM_SECTION_NAME {
                return decode_string_constants_section(&wasm[name_end..section_end]);
            }
        }
        offset = section_end;
    }
    Ok(Vec::new())
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
    fn round_trips_string_constants_section() {
        let strings = vec!["hello".to_string(), "world".to_string(), "".to_string()];
        let encoded = encode_string_constants_section(&strings);
        let decoded = decode_string_constants_section(&encoded).expect("decode should succeed");
        assert_eq!(decoded, strings);
    }
}
