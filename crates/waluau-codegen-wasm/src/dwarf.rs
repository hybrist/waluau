use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use waluau_diagnostics::Diagnostic;
use waluau_ir::{Module, SourceFile, SourceLocation, SourceOrigin};
use wasm_encoder::{CustomSection, Encode};

const DW_TAG_COMPILE_UNIT: u64 = 0x11;
const DW_TAG_SUBPROGRAM: u64 = 0x2e;
const DW_CHILDREN_NO: u8 = 0;
const DW_CHILDREN_YES: u8 = 1;

const DW_AT_NAME: u64 = 0x03;
const DW_AT_STMT_LIST: u64 = 0x10;
const DW_AT_LOW_PC: u64 = 0x11;
const DW_AT_HIGH_PC: u64 = 0x12;
const DW_AT_LANGUAGE: u64 = 0x13;
const DW_AT_COMP_DIR: u64 = 0x1b;
const DW_AT_PRODUCER: u64 = 0x25;
const DW_AT_DECL_FILE: u64 = 0x3a;
const DW_AT_DECL_LINE: u64 = 0x3b;
const DW_AT_EXTERNAL: u64 = 0x3f;

const DW_FORM_ADDR: u64 = 0x01;
const DW_FORM_DATA2: u64 = 0x05;
const DW_FORM_DATA4: u64 = 0x06;
const DW_FORM_STRING: u64 = 0x08;
const DW_FORM_UDATA: u64 = 0x0f;
const DW_FORM_SEC_OFFSET: u64 = 0x17;
const DW_FORM_FLAG_PRESENT: u64 = 0x19;

const DW_LANG_LO_USER: u16 = 0x8000;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct FunctionDebugMap {
    /// Offset of the first instruction after the function's local declarations.
    pub instruction_start: u32,
    /// Function-body-relative instruction offsets and their authored origins.
    pub rows: Vec<(u32, SourceOrigin)>,
}

#[derive(Clone, Copy)]
struct FunctionRange {
    low: u32,
    high: u32,
}

#[derive(Clone)]
struct SourceRow {
    address: u32,
    location: SourceLocation,
}

pub(crate) fn append_sections(
    wasm: &mut Vec<u8>,
    module: &Module,
    bodies: &[Vec<u8>],
    function_maps: &[FunctionDebugMap],
) -> Result<(), Diagnostic> {
    if function_maps.len() != module.functions.len() || bodies.len() < function_maps.len() {
        return Err(Diagnostic::new(
            "internal error: incomplete function layout for DWARF emission",
        ));
    }

    let body_count = u32::try_from(bodies.len())
        .map_err(|_| Diagnostic::new("too many function bodies for a Wasm code section"))?;
    let mut body_cursor = u32_len(body_count) as u32;
    let mut ranges = Vec::with_capacity(function_maps.len());
    let mut rows_by_function = Vec::with_capacity(function_maps.len());
    for (body, map) in bodies.iter().zip(function_maps) {
        let body_len = u32::try_from(body.len())
            .map_err(|_| Diagnostic::new("function body is too large for DWARF32"))?;
        let body_start = body_cursor
            .checked_add(u32_len(body_len) as u32)
            .ok_or_else(|| Diagnostic::new("Wasm code offset overflow during DWARF emission"))?;
        if map.instruction_start >= body_len {
            return Err(Diagnostic::new(
                "invalid instruction start in DWARF function layout",
            ));
        }
        let low = body_start
            .checked_add(map.instruction_start)
            .ok_or_else(|| Diagnostic::new("Wasm code offset overflow during DWARF emission"))?;
        let high = body_start
            .checked_add(body_len)
            .ok_or_else(|| Diagnostic::new("Wasm code offset overflow during DWARF emission"))?;
        ranges.push(FunctionRange { low, high });
        let mut source_rows = Vec::new();
        for (offset, origin) in &map.rows {
            if *offset >= body_len {
                return Err(Diagnostic::new(
                    "invalid instruction offset in DWARF function layout",
                ));
            }
            if let SourceOrigin::Authored(location) = origin {
                source_file(module, location.file)?;
                source_rows.push(SourceRow {
                    address: body_start.checked_add(*offset).ok_or_else(|| {
                        Diagnostic::new("Wasm code offset overflow during DWARF emission")
                    })?,
                    location: *location,
                });
            }
        }
        rows_by_function.push(source_rows);
        body_cursor = high;
    }

    let source_paths = browser_source_paths(module);
    let debug_abbrev = encode_abbrev();
    let debug_info = encode_info(module, &source_paths, &ranges)?;
    let debug_line = encode_line(module, &source_paths, &ranges, &rows_by_function)?;
    append_custom(wasm, ".debug_abbrev", debug_abbrev);
    append_custom(wasm, ".debug_info", debug_info);
    append_custom(wasm, ".debug_line", debug_line);
    Ok(())
}

fn append_custom(wasm: &mut Vec<u8>, name: &'static str, data: Vec<u8>) {
    wasm.push(0);
    CustomSection {
        name: Cow::Borrowed(name),
        data: Cow::Owned(data),
    }
    .encode(wasm);
}

fn encode_abbrev() -> Vec<u8> {
    let mut out = Vec::new();
    uleb(1, &mut out);
    uleb(DW_TAG_COMPILE_UNIT, &mut out);
    out.push(DW_CHILDREN_YES);
    for (attribute, form) in [
        (DW_AT_PRODUCER, DW_FORM_STRING),
        (DW_AT_LANGUAGE, DW_FORM_DATA2),
        (DW_AT_NAME, DW_FORM_STRING),
        (DW_AT_STMT_LIST, DW_FORM_SEC_OFFSET),
        (DW_AT_COMP_DIR, DW_FORM_STRING),
        (DW_AT_LOW_PC, DW_FORM_ADDR),
        (DW_AT_HIGH_PC, DW_FORM_DATA4),
    ] {
        uleb(attribute, &mut out);
        uleb(form, &mut out);
    }
    uleb(0, &mut out);
    uleb(0, &mut out);

    uleb(2, &mut out);
    uleb(DW_TAG_SUBPROGRAM, &mut out);
    out.push(DW_CHILDREN_NO);
    for (attribute, form) in [
        (DW_AT_NAME, DW_FORM_STRING),
        (DW_AT_DECL_FILE, DW_FORM_UDATA),
        (DW_AT_DECL_LINE, DW_FORM_UDATA),
        (DW_AT_LOW_PC, DW_FORM_ADDR),
        (DW_AT_HIGH_PC, DW_FORM_DATA4),
        (DW_AT_EXTERNAL, DW_FORM_FLAG_PRESENT),
    ] {
        uleb(attribute, &mut out);
        uleb(form, &mut out);
    }
    uleb(0, &mut out);
    uleb(0, &mut out);
    uleb(0, &mut out);
    out
}

fn encode_info(
    module: &Module,
    paths: &[String],
    ranges: &[FunctionRange],
) -> Result<Vec<u8>, Diagnostic> {
    let authored = module
        .functions
        .iter()
        .enumerate()
        .filter_map(|(index, function)| match function.source_map.definition {
            SourceOrigin::Authored(location) => Some((index, function, location)),
            SourceOrigin::Synthetic => None,
        })
        .collect::<Vec<_>>();
    let low_pc = authored
        .iter()
        .map(|(index, _, _)| ranges[*index].low)
        .min()
        .unwrap_or(0);
    let high_pc = authored
        .iter()
        .map(|(index, _, _)| ranges[*index].high)
        .max()
        .unwrap_or(low_pc);

    let mut unit = Vec::new();
    unit.extend_from_slice(&4u16.to_le_bytes());
    unit.extend_from_slice(&0u32.to_le_bytes());
    unit.push(4);
    uleb(1, &mut unit);
    cstring("waluau compiler (development DWARF)", &mut unit);
    unit.extend_from_slice(&DW_LANG_LO_USER.to_le_bytes());
    cstring(
        paths.first().map_or("program.walu", String::as_str),
        &mut unit,
    );
    unit.extend_from_slice(&0u32.to_le_bytes());
    cstring(".", &mut unit);
    unit.extend_from_slice(&low_pc.to_le_bytes());
    unit.extend_from_slice(&(high_pc - low_pc).to_le_bytes());

    for (index, function, location) in authored {
        let range = ranges[index];
        let (line, _) = line_column(&source_file(module, location.file)?.source, location)?;
        uleb(2, &mut unit);
        cstring(&function.name, &mut unit);
        let file_index = location
            .file
            .0
            .checked_add(1)
            .ok_or_else(|| Diagnostic::new("DWARF source file index overflow"))?;
        uleb(u64::from(file_index), &mut unit);
        uleb(line.into(), &mut unit);
        unit.extend_from_slice(&range.low.to_le_bytes());
        unit.extend_from_slice(&(range.high - range.low).to_le_bytes());
    }
    uleb(0, &mut unit);

    let unit_length = u32::try_from(unit.len())
        .map_err(|_| Diagnostic::new(".debug_info is too large for DWARF32"))?;
    let mut out = Vec::with_capacity(unit.len() + 4);
    out.extend_from_slice(&unit_length.to_le_bytes());
    out.extend_from_slice(&unit);
    Ok(out)
}

fn encode_line(
    module: &Module,
    paths: &[String],
    ranges: &[FunctionRange],
    rows_by_function: &[Vec<SourceRow>],
) -> Result<Vec<u8>, Diagnostic> {
    let mut header = vec![1, 1, 1, (-5i8) as u8, 14, 13];
    header.extend_from_slice(&[0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1]);
    header.push(0); // include-directory table terminator; paths are relative to comp_dir
    for path in paths {
        cstring(path, &mut header);
        uleb(0, &mut header); // current directory (`.`)
        uleb(0, &mut header); // modification time unknown
        uleb(0, &mut header); // source length unknown
    }
    header.push(0);

    let mut program = Vec::new();
    for (function_index, rows) in rows_by_function.iter().enumerate() {
        if rows.is_empty()
            || !matches!(
                module.functions[function_index].source_map.definition,
                SourceOrigin::Authored(_)
            )
        {
            continue;
        }
        let range = ranges[function_index];
        let mut by_address = BTreeMap::new();
        for row in rows {
            by_address.entry(row.address).or_insert(row.location);
        }

        let first_address = *by_address.keys().next().expect("non-empty rows");
        extended_set_address(first_address, &mut program);
        let mut address = first_address;
        let mut line = 1u32;
        let mut file = 1u32;
        for (row_address, location) in by_address {
            let (row_line, column) =
                line_column(&source_file(module, location.file)?.source, location)?;
            let row_file = location
                .file
                .0
                .checked_add(1)
                .ok_or_else(|| Diagnostic::new("DWARF source file index overflow"))?;
            if row_address != address {
                program.push(2); // DW_LNS_advance_pc
                uleb(u64::from(row_address - address), &mut program);
                address = row_address;
            }
            if row_file != file {
                program.push(4); // DW_LNS_set_file
                uleb(u64::from(row_file), &mut program);
                file = row_file;
            }
            if row_line != line {
                program.push(3); // DW_LNS_advance_line
                sleb(i64::from(row_line) - i64::from(line), &mut program);
                line = row_line;
            }
            program.push(5); // DW_LNS_set_column
            uleb(u64::from(column), &mut program);
            program.push(1); // DW_LNS_copy
        }
        if range.high > address {
            program.push(2);
            uleb(u64::from(range.high - address), &mut program);
        }
        program.extend_from_slice(&[0, 1, 1]); // DW_LNE_end_sequence
    }

    let header_length = u32::try_from(header.len())
        .map_err(|_| Diagnostic::new(".debug_line header is too large for DWARF32"))?;
    let unit_length = 2usize
        .checked_add(4)
        .and_then(|length| length.checked_add(header.len()))
        .and_then(|length| length.checked_add(program.len()))
        .ok_or_else(|| Diagnostic::new(".debug_line length overflow"))?;
    let mut out = Vec::with_capacity(unit_length + 4);
    out.extend_from_slice(
        &u32::try_from(unit_length)
            .map_err(|_| Diagnostic::new(".debug_line is too large for DWARF32"))?
            .to_le_bytes(),
    );
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&header_length.to_le_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(&program);
    Ok(out)
}

fn extended_set_address(address: u32, out: &mut Vec<u8>) {
    out.push(0);
    uleb(5, out);
    out.push(2); // DW_LNE_set_address
    out.extend_from_slice(&address.to_le_bytes());
}

fn line_column(source: &str, location: SourceLocation) -> Result<(u32, u32), Diagnostic> {
    let start = location.span.start as usize;
    let chars = source.chars().collect::<Vec<_>>();
    if start > chars.len() {
        return Err(Diagnostic::new(
            "source span is outside its DWARF source file",
        ));
    }
    let preceding = &chars[..start];
    let line = preceding
        .iter()
        .filter(|character| **character == '\n')
        .count()
        + 1;
    let column = preceding
        .iter()
        .rev()
        .take_while(|character| **character != '\n')
        .count()
        + 1;
    Ok((
        u32::try_from(line).map_err(|_| Diagnostic::new("DWARF line number overflow"))?,
        u32::try_from(column).map_err(|_| Diagnostic::new("DWARF column number overflow"))?,
    ))
}

fn source_file(module: &Module, id: waluau_ir::SourceFileId) -> Result<&SourceFile, Diagnostic> {
    module
        .source_files
        .get(id.0 as usize)
        .ok_or_else(|| Diagnostic::new("invalid source file id during DWARF emission"))
}

fn browser_source_paths(module: &Module) -> Vec<String> {
    let absolute = module
        .source_files
        .iter()
        .map(|source| PathBuf::from(&source.path))
        .filter(|path| path.is_absolute())
        .collect::<Vec<_>>();
    let common_dir = common_parent(&absolute);
    module
        .source_files
        .iter()
        .map(|source| {
            let path = Path::new(&source.path);
            let relative = common_dir
                .as_deref()
                .and_then(|prefix| path.strip_prefix(prefix).ok())
                .unwrap_or(path);
            slash_path(relative)
        })
        .collect()
}

fn common_parent(paths: &[PathBuf]) -> Option<PathBuf> {
    let first = paths.first()?.parent()?.to_path_buf();
    let mut components = first.components().collect::<Vec<_>>();
    for path in &paths[1..] {
        let parent = path.parent()?;
        let other = parent.components().collect::<Vec<_>>();
        let shared = components
            .iter()
            .zip(other.iter())
            .take_while(|(left, right)| left == right)
            .count();
        components.truncate(shared);
    }
    let mut result = PathBuf::new();
    for component in components {
        result.push(component.as_os_str());
    }
    (!result.as_os_str().is_empty()).then_some(result)
}

fn slash_path(path: &Path) -> String {
    let components = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            Component::ParentDir => Some("..".to_string()),
            Component::CurDir => None,
            Component::RootDir | Component::Prefix(_) => None,
        })
        .collect::<Vec<_>>();
    if components.is_empty() {
        "program.walu".to_string()
    } else {
        components.join("/")
    }
}

fn cstring(value: &str, out: &mut Vec<u8>) {
    out.extend(value.bytes().filter(|byte| *byte != 0));
    out.push(0);
}

fn u32_len(value: u32) -> usize {
    let mut encoded = Vec::new();
    uleb(u64::from(value), &mut encoded);
    encoded.len()
}

fn uleb(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            return;
        }
    }
}

fn sleb(mut value: i64, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        let done = (value == 0 && byte & 0x40 == 0) || (value == -1 && byte & 0x40 != 0);
        out.push(if done { byte } else { byte | 0x80 });
        if done {
            return;
        }
    }
}
