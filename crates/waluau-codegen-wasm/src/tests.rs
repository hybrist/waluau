use waluau_ast::BinaryOp;
use waluau_ir::Instruction as IrInstruction;
use wasmparser::{Operator, Parser, Payload, TypeRef, Validator};
use wasmprinter::print_bytes;

fn emit(module: &waluau_ir::Module) -> Result<Vec<u8>, waluau_diagnostics::Diagnostic> {
    super::emit(module).map(|r| r.wasm)
}

fn wasm_export_func_index(wasm: &[u8], name: &str) -> Option<u32> {
    for payload in Parser::new(0).parse_all(wasm) {
        let payload = payload.expect("wasm should parse");
        if let Payload::ExportSection(reader) = payload {
            for export in reader {
                let export = export.expect("export should decode");
                if export.name == name {
                    return Some(export.index);
                }
            }
        }
    }
    None
}

fn wasm_has_start_section(wasm: &[u8]) -> bool {
    Parser::new(0).parse_all(wasm).any(|payload| {
        matches!(
            payload.expect("wasm should parse"),
            Payload::StartSection { .. }
        )
    })
}

fn custom_section(wasm: &[u8], name: &str) -> Option<Vec<u8>> {
    Parser::new(0).parse_all(wasm).find_map(|payload| {
        let payload = payload.expect("wasm should parse");
        match payload {
            Payload::CustomSection(section) if section.name() == name => {
                Some(section.data().to_vec())
            }
            _ => None,
        }
    })
}

fn contains_cstring(bytes: &[u8], value: &str) -> bool {
    let mut encoded = value.as_bytes().to_vec();
    encoded.push(0);
    bytes.windows(encoded.len()).any(|window| window == encoded)
}

fn read_uleb(bytes: &[u8], cursor: &mut usize) -> u64 {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = bytes[*cursor];
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}

fn read_sleb(bytes: &[u8], cursor: &mut usize) -> i64 {
    let mut value = 0i64;
    let mut shift = 0;
    let mut byte;
    loop {
        byte = bytes[*cursor];
        *cursor += 1;
        value |= i64::from(byte & 0x7f) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            break;
        }
    }
    if shift < 64 && byte & 0x40 != 0 {
        value |= !0 << shift;
    }
    value
}

fn dwarf_line_rows(section: &[u8]) -> Vec<(u32, u32, u32, u32)> {
    let header_length = u32::from_le_bytes(section[6..10].try_into().unwrap()) as usize;
    let mut cursor = 10 + header_length;
    let mut address = 0u32;
    let mut file = 1u32;
    let mut line = 1u32;
    let mut column = 0u32;
    let mut rows = Vec::new();
    while cursor < section.len() {
        match section[cursor] {
            0 => {
                cursor += 1;
                let length = read_uleb(section, &mut cursor) as usize;
                let end = cursor + length;
                match section[cursor] {
                    1 => {
                        address = 0;
                        file = 1;
                        line = 1;
                        column = 0;
                    }
                    2 => {
                        address =
                            u32::from_le_bytes(section[cursor + 1..cursor + 5].try_into().unwrap());
                    }
                    other => panic!("unexpected extended line opcode {other}"),
                }
                cursor = end;
            }
            1 => {
                cursor += 1;
                rows.push((address, file, line, column));
            }
            2 => {
                cursor += 1;
                address += read_uleb(section, &mut cursor) as u32;
            }
            3 => {
                cursor += 1;
                line = (i64::from(line) + read_sleb(section, &mut cursor)) as u32;
            }
            4 => {
                cursor += 1;
                file = read_uleb(section, &mut cursor) as u32;
            }
            5 => {
                cursor += 1;
                column = read_uleb(section, &mut cursor) as u32;
            }
            other => panic!("unexpected standard/special line opcode {other}"),
        }
    }
    rows
}

fn wasm_instruction_offsets(wasm: &[u8]) -> std::collections::BTreeSet<u32> {
    let mut code_start = None;
    let mut offsets = std::collections::BTreeSet::new();
    for payload in Parser::new(0).parse_all(wasm) {
        match payload.expect("wasm should parse") {
            Payload::CodeSectionStart { range, .. } => code_start = Some(range.start),
            Payload::CodeSectionEntry(body) => {
                let mut reader = body.get_operators_reader().expect("operators should parse");
                while !reader.eof() {
                    offsets.insert(
                        (reader.original_position() - code_start.expect("code section")) as u32,
                    );
                    reader.read().expect("instruction should parse");
                }
            }
            _ => {}
        }
    }
    offsets
}

#[test]
fn development_dwarf_is_explicit_and_preserves_default_bytes() {
    let source = "function answer(): i32\n    return 42\nend\nanswer()\n";
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");

    let existing = super::emit(&ir).expect("default emit should succeed").wasm;
    let explicit_default = super::emit_with_options(&ir, super::EmitOptions::default())
        .expect("explicit default emit should succeed");
    assert!(explicit_default.development_sources.is_empty());
    let explicit_default = explicit_default.wasm;
    assert_eq!(
        existing, explicit_default,
        "default output bytes must not change"
    );
    assert!(
        Parser::new(0).parse_all(&existing).all(|payload| !matches!(
            payload.expect("default Wasm should parse"),
            Payload::CustomSection(section) if section.name().starts_with(".debug_")
        )),
        "default output must omit DWARF"
    );

    let emitted = super::emit_with_options(
        &ir,
        super::EmitOptions {
            development_dwarf: true,
            ..Default::default()
        },
    )
    .expect("development emit should succeed");
    let debug = emitted
        .development_dwarf
        .expect("development DWARF companion");
    assert!(custom_section(&emitted.wasm, ".debug_info").is_none());
    let reference = custom_section(&emitted.wasm, "external_debug_info")
        .expect("external_debug_info reference");
    let mut reference_cursor = 0;
    let url_length = read_uleb(&reference, &mut reference_cursor) as usize;
    assert_eq!(
        &reference[reference_cursor..reference_cursor + url_length],
        b"program.debug.wasm"
    );
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&debug)
        .expect("debug Wasm should validate");
    let abbrev = custom_section(&debug, ".debug_abbrev").expect(".debug_abbrev");
    let info = custom_section(&debug, ".debug_info").expect(".debug_info");
    let line = custom_section(&debug, ".debug_line").expect(".debug_line");
    assert!(!abbrev.is_empty());
    assert_eq!(&info[4..6], &4u16.to_le_bytes(), "DWARF v4 info unit");
    assert_eq!(&line[4..6], &4u16.to_le_bytes(), "DWARF v4 line unit");
    assert!(contains_cstring(&info, "answer"));
    assert!(contains_cstring(
        &info,
        "waluau compiler (development DWARF)"
    ));
    assert!(contains_cstring(&line, "__waluau/sources/virtual/s-source"));
    assert!(custom_section(&debug, ".debug_str").is_none());
    assert!(custom_section(&emitted.wasm, "name").is_some());
    assert!(custom_section(&debug, "name").is_some());
    assert!(
        !contains_cstring(&info, "__waluau_top_level_init"),
        "synthetic top-level helper must not receive a subprogram DIE"
    );
    let rows = dwarf_line_rows(&line);
    assert!(!rows.is_empty(), "authored instructions need line rows");
    let instruction_offsets = wasm_instruction_offsets(&emitted.wasm);
    for (address, _file, _line, column) in rows {
        assert!(
            column > 0,
            "Chrome reverse mapping requires nonzero columns"
        );
        assert!(
            instruction_offsets.contains(&address),
            "DWARF address {address} must be a final Wasm instruction boundary"
        );
    }
}

#[test]
fn development_dwarf_rewrites_package_sources_to_relative_browser_paths() {
    let source = "function draw(): i32\n    return 1\nend\n";
    let program =
        waluau_parser::parse_with_path(source, "package:waluau-engine/v1/graphics#debug?100%.walu")
            .expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let emitted = super::emit_with_options(
        &ir,
        super::EmitOptions {
            development_dwarf: true,
            ..Default::default()
        },
    )
    .expect("development emit should succeed");

    assert_eq!(emitted.development_sources.len(), 1);
    assert_eq!(
        emitted.development_sources[0].path,
        "__waluau/sources/packages/s-waluau-engine/s-v1/s-graphics~23debug~3F100~25.walu"
    );
    assert_eq!(emitted.development_sources[0].source, source);
    let debug = emitted.development_dwarf.expect("DWARF companion");
    let line = custom_section(&debug, ".debug_line").expect(".debug_line");
    assert!(contains_cstring(
        &line,
        "__waluau/sources/packages/s-waluau-engine/s-v1/s-graphics~23debug~3F100~25.walu"
    ));
    assert!(!contains_cstring(&line, "package:waluau-engine"));
}

#[test]
fn incremental_development_dwarf_matches_a_cold_emit() {
    fn lower(source: &str) -> waluau_ir::Module {
        let program = waluau_parser::parse(source).expect("parse should succeed");
        let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
        waluau_ir::build(&typed).expect("ir should succeed")
    }

    let options = super::EmitOptions {
        development_dwarf: true,
        ..Default::default()
    };
    let first = lower("function answer(): i32\n    return 41\nend\n");
    let changed = lower("function answer(): i32\n    return 42\nend\n");
    let mut cache = super::EmitCache::default();
    super::emit_cached_with_options(&first, &mut cache, options).expect("cold cached emit");
    let incremental =
        super::emit_cached_with_options(&changed, &mut cache, options).expect("incremental emit");
    assert!(cache.last_emit_was_incremental());
    let cold = super::emit_with_options(&changed, options).expect("cold comparison emit");
    assert_eq!(incremental.wasm, cold.wasm, "code bytes must be equivalent");
    assert_eq!(
        incremental.development_dwarf, cold.development_dwarf,
        "external DWARF bytes must be equivalent"
    );
    assert_eq!(
        incremental.development_sources, cold.development_sources,
        "development source snapshots must be equivalent"
    );
}

#[test]
fn development_dwarf_rejects_malformed_function_layouts() {
    let program =
        waluau_parser::parse("function answer(): i32 return 42 end").expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let mut maps = vec![super::dwarf::FunctionDebugMap::default(); ir.functions.len()];
    maps[0].instruction_start = 2;
    let bodies = vec![vec![0]; ir.functions.len()];
    let error = super::dwarf::encode_external_module(b"\0asm\x01\0\0\0", &ir, &bodies, &maps)
        .expect_err("out-of-body instruction start should fail");
    assert!(error.to_string().contains("invalid instruction start"));
}

#[test]
fn recursive_record_fields_dispatch_resolved_methods() {
    let source = include_str!("../../../conformance/recursive_record_field_method_dispatch.walu");
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("recursive type emission should terminate");

    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted recursive-tree module should validate");
}

#[test]
fn exports_top_level_code_as_main_without_a_start_section() {
    let source = r#"
        local value: i32 = 1
        assert(value == 1)
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");

    assert!(
        wasm_export_func_index(&wasm, "main").is_some(),
        "top-level code should have an explicit main entry point"
    );
    assert_eq!(
        wasm_export_func_index(&wasm, "main"),
        wasm_export_func_index(&wasm, "__waluau_main"),
        "hosts should be able to identify the generated main entry point"
    );
    assert!(
        !wasm_has_start_section(&wasm),
        "top-level code must not execute during module instantiation"
    );
}

fn wasm_has_export_with_prefix(wasm: &[u8], prefix: &str) -> bool {
    for payload in Parser::new(0).parse_all(wasm) {
        let payload = payload.expect("wasm should parse");
        if let Payload::ExportSection(reader) = payload {
            for export in reader {
                if export
                    .expect("export should decode")
                    .name
                    .starts_with(prefix)
                {
                    return true;
                }
            }
        }
    }
    false
}

#[test]
fn minimal_exports_keep_only_the_entry_points() {
    let source = r#"
        type Point = {x: i32, y: i32}

        function helper(point: Point): i32
            return point.x + point.y
        end

        assert(helper({x = 1, y = 2}) == 3)
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");

    let full = emit(&ir).expect("emit should succeed");
    assert!(
        wasm_export_func_index(&full, "helper").is_some(),
        "default emission keeps the playground/debugging exports"
    );
    assert!(
        wasm_has_export_with_prefix(&full, "__waluau_new_record_")
            && wasm_has_export_with_prefix(&full, "__waluau_get_record_"),
        "default emission keeps the record marshalling helpers"
    );

    let minimal = super::emit_with_options(
        &ir,
        super::EmitOptions {
            minimal_exports: true,
            ..Default::default()
        },
    )
    .expect("minimal emit should succeed")
    .wasm;
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&minimal)
        .expect("minimal-export module should validate");
    assert!(
        wasm_export_func_index(&minimal, "helper").is_none(),
        "user functions must not pin unreachable code in minimal-export builds"
    );
    assert!(
        !wasm_has_export_with_prefix(&minimal, "__waluau_new_record_")
            && !wasm_has_export_with_prefix(&minimal, "__waluau_get_record_"),
        "record marshalling helpers must not pin record types in minimal-export builds"
    );
    assert!(
        wasm_export_func_index(&minimal, "main").is_some()
            && wasm_export_func_index(&minimal, "__waluau_main").is_some(),
        "the runtime entry points must survive minimal-export builds"
    );
}

#[test]
fn emits_valid_wasm_for_nominal_enum_match() {
    let source = r#"
        enum Direction { north, east, south }
        function score(direction: Direction): i32
            match direction do
            case Direction.north then return 1
            case Direction.east then return 2
            case Direction.south then return 3
            end
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("nominal enum match module should validate");
    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(
        wat.contains("i32.eq"),
        "enum dispatch should compare i32 tags:\n{wat}"
    );
}

#[test]
fn top_level_main_entry_point_takes_precedence_over_a_declared_main_export() {
    let source = r#"
        function main(): i32
            return 42
        end

        assert(true)
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");

    let wat = print_bytes(&wasm).expect("wat should print");
    assert_eq!(
        wat.matches("(export \"main\"").count(),
        1,
        "the module should expose exactly one main entry point:\n{wat}"
    );
}

fn wasm_function_import_count(wasm: &[u8]) -> u32 {
    let mut count = 0;
    for payload in Parser::new(0).parse_all(wasm) {
        let payload = payload.expect("wasm should parse");
        if let Payload::ImportSection(reader) = payload {
            for import in reader {
                let import = import.expect("import should decode");
                if matches!(import.ty, TypeRef::Func(_)) {
                    count += 1;
                }
            }
        }
    }
    count
}

fn wasm_type_count(wasm: &[u8]) -> u32 {
    for payload in Parser::new(0).parse_all(wasm) {
        if let Payload::TypeSection(reader) = payload.expect("wasm should parse") {
            return reader.count();
        }
    }
    0
}

fn wasm_function_body_has_call_indirect(wasm: &[u8], func_index: u32) -> bool {
    let import_count = wasm_function_import_count(wasm);
    let target_body = func_index
        .checked_sub(import_count)
        .expect("exported function should be defined in this module");
    let mut body_index = 0;
    for payload in Parser::new(0).parse_all(wasm) {
        let payload = payload.expect("wasm should parse");
        if let Payload::CodeSectionEntry(body) = payload {
            if body_index == target_body {
                let mut reader = body.get_operators_reader().expect("ops should decode");
                while !reader.eof() {
                    if matches!(
                        reader.read().expect("op should decode"),
                        Operator::CallIndirect { .. }
                    ) {
                        return true;
                    }
                }
                return false;
            }
            body_index += 1;
        }
    }
    false
}

#[test]
fn emits_valid_wasm_for_scalar_program() {
    let source = r#"
        function entry(x: i32): i32
            return x + 1
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn emits_valid_wasm_for_empty_record_type() {
    // `type Marker = {}` compiles to a struct type with zero fields; the
    // module must still validate and construct/pass values of it.
    let source = r#"
        type Marker = {}

        function tag(m: Marker): i32
            return 7
        end

        function entry(): i32
            local m: Marker = {}
            local maybe: Marker? = {}
            if maybe == nil then
                return 0
            end
            return tag(m) + tag({}) + tag(maybe)
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn omits_unused_declared_imports_and_their_function_types() {
    fn compile(source: &str) -> Vec<u8> {
        let program = waluau_parser::parse(source).expect("parse should succeed");
        let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
        let ir = waluau_ir::build(&typed).expect("ir should succeed");
        emit(&ir).expect("emit should succeed")
    }

    let baseline = compile(
        r#"
            function identity(value: i32): i32
                return value
            end
        "#,
    );
    let with_unused_declarations = compile(
        r#"
            type Resource = extern
            declare function unused_resource(name: string): Resource
            declare function unused_number(value: f64): f64

            function identity(value: i32): i32
                return value
            end
        "#,
    );

    let wat = print_bytes(&with_unused_declarations).expect("wat should print");
    assert!(
        !wat.contains("unused_resource"),
        "unused import emitted:\n{wat}"
    );
    assert!(
        !wat.contains("unused_number"),
        "unused import emitted:\n{wat}"
    );
    assert_eq!(
        wasm_type_count(&with_unused_declarations),
        wasm_type_count(&baseline),
        "unused declarations should not add function type definitions"
    );
}

#[test]
fn emits_valid_wasm_for_exponentiation() {
    // `^` is supported for every numeric type; floats stay in f64, integer
    // operands are widened through the host pow and truncated back.
    let source = r#"
        function powf(base: f64, exp: f64): f64
            return base ^ exp
        end

        function powi(base: i32, exp: i32): i32
            return base ^ exp
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(
        wat.contains("\"math_pow\""),
        "exponentiation should import the host 'math_pow' function:\n{wat}"
    );
}

#[test]
fn omits_math_pow_import_when_unused() {
    let source = r#"
        function add(a: f64, b: f64): f64
            return a + b
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(
        !wat.contains("\"math_pow\""),
        "programs without '^' should not import 'math_pow'"
    );
}

#[test]
fn emits_valid_wasm_for_array_program() {
    let source = r#"
        function score_count(): i32
            local scores: {number} = {100, 250, 300}
            local first: number = scores[0]
            scores[1] = first + 1
            return #scores
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn emits_valid_wasm_for_bytes_program() {
    let source = r#"
        function entry(data: bytes): i32
            local prefix: bytes = b"AB"
            local merged: bytes = prefix .. data
            if merged == b"AB" then
                return 0
            end
            if merged > b"A" then
                return merged[0] + #merged
            end
            return 0
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    let bytes_constants =
        super::host::parse_bytes_constants_from_wasm(&wasm).expect("bytes section should parse");
    assert_eq!(bytes_constants, vec![b"AB".to_vec(), b"A".to_vec()]);
}

#[test]
fn emits_valid_wasm_for_non_capturing_indirect_call() {
    let source = r#"
        function entry(x: i32): i32
            local f: (i32) -> i32 = function(y: i32): i32
                return y + 1
            end
            return f(x)
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn emits_valid_wasm_for_capturing_closure_values() {
    let source = r#"
        function entry(x: i32): i32
            local f: (i32) -> i32 = function(y: i32): i32
                return x + y
            end
            return f(1)
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("capturing closures should compile");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn emits_valid_wasm_for_negative_literal_in_typed_i32_context() {
    let source = r#"
        function entry(): i32
            return -1
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn emits_structured_if_for_simple_branch() {
    let source = r#"
        function choose(x: i32, y: i32): i32
            if x > y then
                return x
            else
                return y
            end
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(wat.contains(" if"));
    assert!(!wat.contains("i32.eq\n    if"));
}

#[test]
fn emits_structured_loop_for_simple_while() {
    let source = r#"
        function sum_to(n: i32): i32
            local acc: i32 = 0
            local i: i32 = n
            while i > 0 do
                acc = acc + i
                i = i - 1
            end
            return acc
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(wat.contains(" loop"));
    assert!(!wat.contains("i32.eq\n    if"));
}

#[test]
fn keeps_immediate_return_value_on_stack() {
    let source = r#"
        function entry(x: i32): i32
            return x + 1
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    let mut saw_add_then_return = false;
    for payload in Parser::new(0).parse_all(&wasm) {
        let payload = payload.expect("wasm should parse");
        if let Payload::CodeSectionEntry(body) = payload {
            let mut reader = body.get_operators_reader().expect("ops should decode");
            let mut prev_was_add = false;
            while !reader.eof() {
                let op = reader.read().expect("op should decode");
                match op {
                    Operator::I32Add => prev_was_add = true,
                    Operator::Return if prev_was_add => {
                        saw_add_then_return = true;
                        break;
                    }
                    _ => prev_was_add = false,
                }
            }
            break;
        }
    }
    assert!(saw_add_then_return);
}

#[test]
fn emits_valid_wasm_for_multi_return() {
    let source = r#"
        function pair(x: i32, y: i32): i32, i32
            return x, y
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn emits_valid_wasm_for_multi_let_binding() {
    let source = r#"
        function swap(x: i32, y: i32): i32, i32
            return y, x
        end
        function entry(a: i32, b: i32): i32
            local x: i32, y: i32 = swap(a, b)
            return x + y
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn emits_valid_wasm_for_multi_assign() {
    let source = r#"
        function swap(x: i32, y: i32): i32, i32
            return y, x
        end
        function entry(a: i32, b: i32): i32
            local x: i32, y: i32 = a, b
            x, y = swap(x, y)
            return x + y
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn emits_valid_wasm_for_for_in_closure_iterator() {
    let source = r#"
        function entry(): i32
            local i: i32 = 0
            local iter = function(): bool, i32, i32
                i = i + 1
                if i > 3 then
                    return false, 0, 0
                end
                return true, i, i + 10
            end
            local acc: i32 = 0
            for a, b in iter do
                acc = acc + a + b
            end
            return acc
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn reuses_i32_local_slots_for_disjoint_live_ranges() {
    let source = r#"
        function reuse(x: i32): i32
            local a: i32 = x + x
            local b: i32 = a + a
            local c: i32 = x - x
            local d: i32 = c + c
            return b + d
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let function = &ir.functions[0];
    let signatures = std::iter::once((
        function.name.clone(),
        super::FunctionSignature {
            index: 0,
            params: function.params.iter().map(|(_, ty)| ty.clone()).collect(),
            result: function.return_type.clone(),
        },
    ))
    .collect::<std::collections::HashMap<_, _>>();
    let value_types = super::infer_value_types(function, &signatures).expect("types should infer");
    let array_types = super::collect_array_types(&ir);
    let record_types = super::collect_record_types(&ir);
    let array_registry = super::arrays::ArrayTypeRegistry::with_function_type_offset(
        &array_types,
        &record_types,
        ir.functions.len() as u32 + u32::from(ir.start.is_some()),
        0, // record_type_offset placeholder (unused in this test)
        super::arrays::RuntimeGcTypes {
            anyref_array_type: 0,
            func_val_struct_type: 0,
            boxed_f64_struct_type: 0,
            boxed_bool_struct_type: 0,
        },
    );
    let local_plan = super::build_local_plan(function, &value_types, &array_registry, None)
        .expect("plan should build");

    let block = function
        .blocks
        .get(&function.entry)
        .expect("entry block should exist");
    let param = block
        .instructions
        .iter()
        .find_map(|(value, instruction)| match instruction {
            IrInstruction::Param(_) => Some(*value),
            _ => None,
        })
        .expect("param should exist");
    let a = block
        .instructions
        .iter()
        .find_map(|(value, instruction)| match instruction {
            IrInstruction::Binary {
                op: BinaryOp::Add,
                left,
                right,
                ..
            } if *left == param && *right == param => Some(*value),
            _ => None,
        })
        .expect("a should exist");
    let c = block
        .instructions
        .iter()
        .find_map(|(value, instruction)| match instruction {
            IrInstruction::Binary {
                op: BinaryOp::Sub,
                left,
                right,
                ..
            } if *left == param && *right == param => Some(*value),
            _ => None,
        })
        .expect("c should exist");

    assert_eq!(local_plan.slots.get(&a), local_plan.slots.get(&c));
}

#[test]
fn test_array_for_in_tostring_bug() {
    let source = r#"
        function test_loop(): i32
            for x in {1, 2, 3} do
                print("hello" .. tostring(x))
            end
            return 0
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir);
    assert!(wasm.is_ok(), "Wasm emission failed: {:?}", wasm.err());
}

#[test]
fn emits_valid_wasm_for_capturing_closure_through_phi() {
    // A capturing closure that flows through a Phi (branch merge) is called
    // via call_indirect.  Previously this trapped because call_indirect used
    // the logical signature without the capture-cell parameters.
    let source = r#"
        function entry(n: i32): i32
            local i: i32 = 0
            local cap = function(): bool, i32
                i = i + 1
                if i > n then
                    return false, 0
                end
                return true, i
            end
            local noop = function(): bool, i32
                return false, 0
            end
            local use_cap: bool = true
            local iter = noop
            if use_cap then
                iter = cap
            end
            local acc: i32 = 0
            for v in iter do
                acc = acc + v
            end
            return acc
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn emits_valid_wasm_for_record_struct_ops() {
    let source = r#"
        function entry(seed: i32): i32
            local point: { x: i32, y: i32 } = { x = seed, y = seed + 1 }
            point.x = point.x + 41
            return point.x
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("record structs should compile");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn devirtualized_method_call_avoids_call_indirect() {
    let source = r#"
        local point = { x = 41::i32 }

        function point:get_x(): i32
            return self.x
        end

        assert(point:get_x() == 41)
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");

    let mut saw_call_indirect = false;
    for payload in Parser::new(0).parse_all(&wasm) {
        let payload = payload.expect("wasm should parse");
        if let Payload::CodeSectionEntry(body) = payload {
            let mut reader = body.get_operators_reader().expect("ops should decode");
            while !reader.eof() {
                if matches!(
                    reader.read().expect("op should decode"),
                    Operator::CallIndirect { .. }
                ) {
                    saw_call_indirect = true;
                    break;
                }
            }
        }
    }

    assert!(
        !saw_call_indirect,
        "expected direct call for method dispatch"
    );
}

#[test]
fn declared_host_method_call_imports_declared_method() {
    let source = r#"
        type Element = extern
        declare function getElement(): Element
        declare function Element:value(delta: i32): i32

        assert(getElement():value(7::i32) == 49)
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(
        wat.contains(r#"(import "waluau" "Element.value""#),
        "expected declared host method import in:\n{wat}"
    );

    let mut saw_call_indirect = false;
    for payload in Parser::new(0).parse_all(&wasm) {
        let payload = payload.expect("wasm should parse");
        if let Payload::CodeSectionEntry(body) = payload {
            let mut reader = body.get_operators_reader().expect("ops should decode");
            while !reader.eof() {
                if matches!(
                    reader.read().expect("op should decode"),
                    Operator::CallIndirect { .. }
                ) {
                    saw_call_indirect = true;
                    break;
                }
            }
        }
    }

    assert!(
        !saw_call_indirect,
        "expected direct host import call for extern method dispatch"
    );
}

#[test]
fn declared_extern_operator_overload_imports_declared_method() {
    let source = r#"
        type Tensor = extern
        declare function make_tensor(): Tensor
        declare function Tensor:__add(rhs: Tensor): Tensor

        function entry(): Tensor
            return make_tensor() + make_tensor()
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(
        wat.contains(r#"(import "waluau" "Tensor.__add""#),
        "expected declared host operator import in:\n{wat}"
    );
}

#[test]
fn safe_extern_if_cast_imports_runtime_check() {
    let source = r#"
        type Node = extern
        type Element = extern extends Node
        type HTMLHeadingElement = extern extends Element

        declare function getElement(): Element

        function entry(): i32
            local value: Element = getElement()
            if HTMLHeadingElement(heading) = value then
                return 1
            else
                return 0
            end
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(
        wat.contains(r#"(import "waluau" "extern_is""#),
        "expected safe extern cast runtime import in:\n{wat}"
    );
}

#[test]
fn emits_valid_wasm_for_unknown_boxing() {
    // `unknown` lowers to anyref; primitives box (i32/bool via i31ref, f64 via a
    // boxed struct) and unbox via explicit cast, including through calls and
    // record fields.
    let source = r#"
        function roundtrip_i32(x: i32): i32
            local boxed: unknown = x
            return boxed::i32
        end

        function roundtrip_f64(x: f64): f64
            local boxed: unknown = x
            return boxed::f64
        end

        function identity(v: unknown): unknown
            return v
        end

        function call_through(x: i32): i32
            return identity(x)::i32
        end

        function store_in_field(x: i32): i32
            local r: { v: unknown } = { v = x }
            return r.v::i32
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");

    let mut saw_ref_i31 = false;
    let mut saw_i31_get = false;
    let mut saw_struct_new = false;
    for payload in Parser::new(0).parse_all(&wasm) {
        let payload = payload.expect("wasm should parse");
        if let Payload::CodeSectionEntry(body) = payload {
            let mut reader = body.get_operators_reader().expect("ops should decode");
            while !reader.eof() {
                match reader.read().expect("op should decode") {
                    Operator::RefI31 => saw_ref_i31 = true,
                    Operator::I31GetS | Operator::I31GetU => saw_i31_get = true,
                    Operator::StructNew { .. } => saw_struct_new = true,
                    _ => {}
                }
            }
        }
    }

    assert!(saw_ref_i31, "expected ref.i31 for i32/bool boxing");
    assert!(saw_i31_get, "expected i31.get for i32/bool unboxing");
    assert!(saw_struct_new, "expected struct.new for f64 boxing");
}

#[test]
fn allows_implicit_unbox_from_unknown() {
    // `unknown` values (e.g. unannotated Lua parameters) implicitly unbox to
    // concrete types with a runtime-checked cast, mirroring Lua's dynamic
    // typing.
    let source = r#"
        function pass(v: unknown): i32
            local x: i32 = v
            return x
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed =
        waluau_hir::type_check_and_infer(&program).expect("implicit unbox should type-check");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn omits_unused_host_imports() {
    // A purely scalar program should not import any host functions at all.
    let source = r#"
        function add(a: i32, b: i32): i32
            return a + b
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    let wat = print_bytes(&wasm).expect("wat should print");

    // None of the host import module names should appear for a plain arithmetic function.
    assert!(
        !wat.contains("\"waluau\""),
        "scalar program should not import from 'waluau'"
    );
    assert!(
        !wat.contains("\"wasm:js-string\""),
        "scalar program should not import from 'wasm:js-string'"
    );
}

#[test]
fn only_imports_used_host_functions() {
    // A program using only print should import exactly 'print', not all host functions.
    let source = r#"
        function greet(msg: string): i32
            print(msg)
            return 0
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    let wat = print_bytes(&wasm).expect("wat should print");

    assert!(wat.contains("\"print\""), "should import 'print'");
    assert!(
        !wat.contains("\"bytes_literal\""),
        "should not import 'bytes_literal' when bytes are unused"
    );
    assert!(
        !wat.contains("\"js_tostring_i32\""),
        "should not import 'js_tostring_i32' when tostring(i32) is unused"
    );
    assert!(
        !wat.contains("\"wasm:js-string\""),
        "should not import from 'wasm:js-string' when string ops are unused"
    );
}

#[test]
fn scalar_program_has_no_externref_types() {
    // A plain arithmetic program should not declare any externref types in its
    // type section — there is no string/bytes/print usage that needs them.
    let source = r#"
        function add(x: number, y: number): number
            local z: number = x + y
            return z
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    let wat = print_bytes(&wasm).expect("wat should print");

    assert!(
        !wat.contains("externref"),
        "scalar program type section should not contain externref types"
    );
}

#[test]
fn extern_type_alias_lowers_to_externref() {
    let source = r#"
        type Element = extern

        function identity(value: Element): Element
            return value
        end

    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    let wat = print_bytes(&wasm).expect("wat should print");

    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    assert!(
        wat.contains("externref"),
        "extern alias should lower to externref in Wasm signatures"
    );
}

#[test]
fn generic_extern_specializations_lower_to_distinct_imports_with_externref_signatures() {
    let source = r#"
        type Response = extern
        type Promise<T> = extern

        declare function take_response(value: Promise<Response>): Promise<Response>
        declare function take_string(value: Promise<string>): Promise<string>

        function use_response(value: Promise<Response>): Promise<Response>
            return take_response(value)
        end

        function use_string(value: Promise<string>): Promise<string>
            return take_string(value)
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    let wat = print_bytes(&wasm).expect("wat should print");

    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    assert!(
        wat.contains(r#"(import "waluau" "take_response""#),
        "expected take_response import in:\n{wat}"
    );
    assert!(
        wat.contains(r#"(import "waluau" "take_string""#),
        "expected take_string import in:\n{wat}"
    );
    assert!(
        wat.contains("externref"),
        "generic extern specializations should lower to externref in Wasm signatures"
    );
}

#[test]
fn promise_extern_api_imports_lower_to_externref_signatures() {
    let source = r#"
        type Response = extern
        type Promise<T> = extern

        declare function fetch(url: string): Promise<Response>
        declare function Response:text(): Promise<string>

        function request(url: string): Promise<Response>
            return fetch(url)
        end

        function read_text(response: Response): Promise<string>
            return response:text()
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    let wat = print_bytes(&wasm).expect("wat should print");

    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    assert!(
        wat.contains(r#"(import "waluau" "fetch""#),
        "expected fetch import in:\n{wat}"
    );
    assert!(
        wat.contains(r#"(import "waluau" "Response.text""#),
        "expected Response.text import in:\n{wat}"
    );
    assert!(
        wat.contains("externref"),
        "Promise<T> API imports should lower to externref-compatible signatures"
    );
}

#[test]
fn nullable_extern_nil_check_lowers_to_ref_is_null() {
    let source = r#"
        type Element = extern

        function score(value: Element?): i32
            if value ~= nil then
                return 20
            end
            return 10
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    let wat = print_bytes(&wasm).expect("wat should print");

    assert!(
        wat.contains("ref.is_null"),
        "nullable extern nil checks should lower to ref.is_null in:\n{wat}"
    );
}

#[test]
fn boxed_nullable_pair_equality_validates() {
    // `string.byte` out of range yields nil, so comparing two of its results
    // exercises nullable-vs-nullable equality on a boxed numeric type.
    let source = format!(
        "{}\n{}",
        include_str!("../../../builtins/core.walu"),
        r#"
        function same(text: string, a: i32, b: i32): bool
            return string.byte(text, a) == string.byte(text, b)
        end

        function differs(text: string, a: i32, b: i32): bool
            return string.byte(text, a) ~= string.byte(text, b)
        end
    "#
    );
    let program = waluau_parser::parse(&source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    let wat = print_bytes(&wasm).expect("wat should print");

    assert!(
        wat.contains("ref.is_null"),
        "nullable numeric equality should branch on nil in:\n{wat}"
    );
    assert!(
        wat.contains("struct.get"),
        "present operands should be unboxed before comparing in:\n{wat}"
    );
}

#[test]
fn boxed_nullable_pair_equality_conformance_compiles() {
    let source = format!(
        "{}\n{}",
        include_str!("../../../builtins/core.walu"),
        include_str!("../../../conformance/nullable_numeric_equality.walu"),
    );
    let program = waluau_parser::parse(&source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn nullable_numeric_record_field_conformance_validates() {
    let source = format!(
        "{}\n{}",
        include_str!("../../../builtins/core.walu"),
        include_str!("../../../conformance/nullable_numeric_record_fields.walu"),
    );
    let program = waluau_parser::parse(&source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("nullable numeric record field reads should emit valid wasm");
}

#[test]
fn declared_host_event_callback_import_exports_trampoline() {
    let source = r#"
        type Event = extern

        declare function addEventListener(handler: (Event) -> unit): unit

        function register(): unit
            addEventListener(function(event: Event): unit
            end)
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    let wat = print_bytes(&wasm).expect("wat should print");

    assert!(
        wat.contains(r#"(import "waluau" "addEventListener""#),
        "expected declared host callback import in:\n{wat}"
    );
    let trampoline_idx =
        wasm_export_func_index(&wasm, super::CALLBACK_EVENT_UNIT_TRAMPOLINE_EXPORT)
            .expect("callback trampoline should be exported");
    assert!(
        wasm_function_body_has_call_indirect(&wasm, trampoline_idx),
        "callback trampoline should dispatch through the closure wrapper table"
    );
}

#[test]
fn declared_nullable_host_callback_accepts_callback_and_nil() {
    let source = r#"
        type Event = extern

        declare function listen(handler: ((Event) -> unit)?): unit

        function register(): unit
            listen(function(event: Event): unit
            end)
        end

        function clear(): unit
            listen(nil)
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    let wat = print_bytes(&wasm).expect("wat should print");

    assert!(
        wat.contains(r#"(import "waluau" "listen""#),
        "expected declared nullable callback import in:\n{wat}"
    );
    assert!(
        wasm_export_func_index(&wasm, super::CALLBACK_EVENT_UNIT_TRAMPOLINE_EXPORT).is_some(),
        "nullable callback import should export the callback trampoline"
    );
    assert!(
        wat.contains("ref.null"),
        "nil callback should emit a null reference:\n{wat}"
    );
}

#[test]
fn declared_host_unit_callback_import_exports_trampoline() {
    let source = r#"
        declare function run_test(body: () -> unit): unit
        declare function record(value: i32): unit

        function register(seed: i32): unit
            local count: i32 = seed
            run_test(function(): unit
                count = count + 1
                record(count)
            end)
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    let wat = print_bytes(&wasm).expect("wat should print");

    assert!(
        wat.contains(r#"(import "waluau" "run_test""#),
        "expected declared host callback import in:\n{wat}"
    );
    let trampoline_idx = wasm_export_func_index(&wasm, super::CALLBACK_UNIT_TRAMPOLINE_EXPORT)
        .expect("() -> unit callback trampoline should be exported");
    assert!(
        wasm_function_body_has_call_indirect(&wasm, trampoline_idx),
        "callback trampoline should dispatch through the closure wrapper table"
    );
}

#[test]
fn unused_host_unit_callback_declaration_omits_import_and_trampoline() {
    let source = r#"
        declare function run_test(body: () -> unit): unit

        function entry(): i32
            return 1
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    let wat = print_bytes(&wasm).expect("wat should print");

    assert!(
        !wat.contains(r#"(import "waluau" "run_test""#),
        "unused callback declaration should not emit an import:\n{wat}"
    );
    assert!(
        wasm_export_func_index(&wasm, super::CALLBACK_UNIT_TRAMPOLINE_EXPORT).is_none(),
        "unused callback declaration should not emit a trampoline"
    );
}

#[test]
fn unused_host_callback_declaration_omits_import_and_trampoline() {
    let source = r#"
        type Event = extern

        declare function addEventListener(handler: (Event) -> unit): unit

        function entry(): i32
            return 1
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    let wat = print_bytes(&wasm).expect("wat should print");

    assert!(
        !wat.contains(r#"(import "waluau" "addEventListener""#),
        "unused callback declaration should not emit an import:\n{wat}"
    );
    assert!(
        wasm_export_func_index(&wasm, super::CALLBACK_EVENT_UNIT_TRAMPOLINE_EXPORT).is_none(),
        "unused callback declaration should not emit a trampoline"
    );
}

#[test]
fn declared_host_event_callback_supports_captured_closure_wrapper() {
    let source = r#"
        type Event = extern

        declare function addEventListener(handler: (Event) -> unit): unit
        declare function record(value: i32): unit

        function register(seed: i32): unit
            local count: i32 = seed
            addEventListener(function(event: Event): unit
                count = count + 1
                record(count)
            end)
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");

    assert!(
        wasm_export_func_index(&wasm, super::CALLBACK_EVENT_UNIT_TRAMPOLINE_EXPORT).is_some(),
        "callback trampoline should be exported for captured event handlers"
    );
    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(
        wat.contains("array.get"),
        "captured callback wrapper should reload captured state from the env array:\n{wat}"
    );
    assert!(
        wat.contains("call_indirect"),
        "trampoline should use the same indirect wrapper path as Waluau CallValue:\n{wat}"
    );
}

#[test]
fn scalar_program_has_no_closure_gc_types() {
    // A program with no closures or function values should not emit the
    // $anyref_array, $func_val, or $boxed_f64 GC struct/array types.
    let source = r#"
        function add(x: i32, y: i32): i32
            return x + y
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    let wat = print_bytes(&wasm).expect("wat should print");

    assert!(
        !wat.contains("(array"),
        "scalar program should not emit GC array types"
    );
    assert!(
        !wat.contains("(struct"),
        "scalar program should not emit GC struct types"
    );
}

#[test]
fn closure_program_still_emits_closure_gc_types() {
    // Programs that use closures must still emit the closure GC types.
    let source = r#"
        function entry(x: i32): i32
            local f: (i32) -> i32 = function(y: i32): i32
                return x + y
            end
            return f(1)
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    let wat = print_bytes(&wasm).expect("wat should print");

    assert!(
        wat.contains("(array"),
        "closure program should emit $anyref_array GC type"
    );
    assert!(
        wat.contains("(struct"),
        "closure program should emit $func_val GC struct type"
    );
}

#[test]
fn emits_no_loop_for_straight_line_function() {
    // A simple straight-line function (single basic block) must not be wrapped in
    // the PC-dispatch loop.
    let source = r#"
        function add(x: i32, y: i32): i32
            return x + y
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(
        !wat.contains(" loop"),
        "straight-line function should not contain a loop"
    );
    assert!(
        !wat.contains("i32.eq"),
        "straight-line function should not use pc dispatch"
    );
}

#[test]
fn emits_structured_if_for_if_else_both_return() {
    // An if/else where both branches return should produce a structured if/else,
    // not a PC-dispatch loop.  The IR builder always creates a dead merge block,
    // which previously caused the function to fall through to the loop path.
    let source = r#"
        function choose(x: i32, y: i32): i32
            if x > y then
                return x
            else
                return y
            end
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(wat.contains(" if"), "should emit structured if");
    assert!(
        !wat.contains(" loop"),
        "if/else function should not contain a loop"
    );
    assert!(!wat.contains("i32.eq"), "should not use pc dispatch");
}

#[test]
fn emits_structured_if_for_early_return() {
    // A one-sided if with an early return followed by a fallthrough should produce
    // a structured `if (then ...) end; ...` without a PC-dispatch loop.
    let source = r#"
        function abs_val(x: i32): i32
            if x < 0 then
                return 0 - x
            end
            return x
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(wat.contains(" if"), "should emit structured if");
    assert!(
        !wat.contains(" loop"),
        "early-return function should not contain a loop"
    );
    assert!(!wat.contains("i32.eq"), "should not use pc dispatch");
}

#[test]
fn emits_valid_wasm_for_tagged_union_coroutine_resume() {
    let source = r#"
        function run(): i32
            local co: thread = coroutine.create(function(): i32
                coroutine.yield(10)
                return 42
            end)
            local result: Finished(i32) | Yielded(i32) | Error(string) = coroutine.resume(co)
            if result is Finished then
                return result.value
            else
                return 0
            end
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    waluau_ir::verify(&ir).expect("ir should verify");

    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");

    let wat = print_bytes(&wasm).expect("wat should print");
    // Tagged resume should produce a struct.new with the canonical record type.
    assert!(
        wat.contains("struct.new"),
        "should emit struct.new for tagged-union record"
    );
}

#[test]
fn emits_valid_wasm_for_unknown_coroutine_payloads() {
    let source = r#"
        function run(): i32
            local co: thread = coroutine.create(function(): i32
                coroutine.yield("hello")
                coroutine.yield(3.5)
                return 7
            end)
            local ok1: bool, value1: unknown = coroutine.resume(co)
            local ok2: bool, value2: unknown = coroutine.resume(co)
            local ok3: bool, value3: unknown = coroutine.resume(co)
            if ok1 and ok2 and ok3 then
                if value1::string == "hello" and value2::f64 == 3.5 and value3::i32 == 7 then
                    return 1
                end
            end
            return 0
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    waluau_ir::verify(&ir).expect("ir should verify");

    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");

    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(
        wat.contains("any.convert_extern"),
        "should emit externref->anyref boxing for string payloads"
    );
    assert!(
        wat.contains("extern.convert_any"),
        "should emit anyref->externref unboxing for string payloads"
    );
}

#[test]
fn emits_valid_wasm_for_reentrant_recursive_coroutine_activations() {
    // Regression for waluau-qsvy: the same suspension-capable function active
    // at several nested call depths (directly and mutually recursive), with a
    // suspension point both before and after the recursive call.
    let source = r#"
        function updown(n: i32): i32
            if n <= 0 then
                return 0
            end
            coroutine.yield(n)
            local rest: i32 = updown(n - 1)
            coroutine.yield(100 + n)
            return rest * 10 + n
        end

        function ping(n: i32): i32
            if n <= 0 then
                return 0
            end
            coroutine.yield(1000 + n)
            return pong(n - 1) + 1
        end

        function pong(n: i32): i32
            if n <= 0 then
                return 0
            end
            coroutine.yield(2000 + n)
            return ping(n - 1) + 2
        end

        function run(): i32
            return updown(3) + ping(4)
        end

        local co: thread = coroutine.create(run)
        local ok: bool, value: unknown = coroutine.resume(co)
        assert(ok)
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    waluau_ir::verify(&ir).expect("ir should verify");

    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");

    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(
        wat.contains("struct.new_default"),
        "suspension points should lazily allocate per-activation frames:\n{wat}"
    );
    assert!(
        wat.contains("array.copy"),
        "the frame-push helper should grow the shadow stack:\n{wat}"
    );
}

#[test]
fn pcall_emits_try_table_and_exception_tag() {
    let source = r#"
        function run(): i32
            local ok: bool, value: unknown = pcall(function(): f64
                return 42.0
            end)
            local arg_ok: bool, arg_value: unknown = pcall(function(x: number): number
                return x + 3
            end, 2)
            if ok and value::f64 == 42.0 and arg_ok and arg_value::f64 == 5 then
                return 1
            end
            return 0
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    waluau_ir::verify(&ir).expect("ir should verify");

    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");

    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(wat.contains("tag"), "should emit a Lua error tag:\n{wat}");
    assert!(
        wat.contains("try_table"),
        "pcall should lower to try_table:\n{wat}"
    );
    assert!(
        wat.contains(&format!("(export \"{}\"", super::LUA_ERROR_TAG_EXPORT)),
        "the Lua error tag should be exported for JS hosts:\n{wat}"
    );
}

#[test]
fn emits_valid_wasm_for_pcall_discriminated_union() {
    let source = include_str!("../../../conformance/pcall_discriminated_union.walu");
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    waluau_ir::verify(&ir).expect("ir should verify");

    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn emits_valid_wasm_for_pcall_catches_faults_fixture() {
    let source = include_str!("../../../conformance/pcall_catches_faults.walu");
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    waluau_ir::verify(&ir).expect("ir should verify");

    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn checked_array_access_throws_lua_error_instead_of_trapping() {
    let source = r#"
        function read(xs: {i32}, index: i32): i32
            return xs[index]
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    waluau_ir::verify(&ir).expect("ir should verify");

    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");

    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(
        wat.contains("array index out of bounds"),
        "bounds failure should throw a Lua-style error message:\n{wat}"
    );
    assert!(
        wat.contains("throw"),
        "bounds failure should throw the Lua error tag:\n{wat}"
    );
    assert!(
        !wat.contains("unreachable"),
        "checked array access should not trap:\n{wat}"
    );
}

#[test]
fn checked_integer_division_throws_lua_error_instead_of_trapping() {
    let source = r#"
        function ratio(a: i32, b: i32): i32
            return a // b
        end

        function remainder(a: i32, b: i32): i32
            return a % b
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    waluau_ir::verify(&ir).expect("ir should verify");

    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");

    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(
        wat.contains("attempt to perform 'n//0'"),
        "integer floor division should carry the Lua 5.4 zero-divisor message:\n{wat}"
    );
    assert!(
        wat.contains("attempt to perform 'n%0'"),
        "integer modulo should carry the Lua 5.4 zero-divisor message:\n{wat}"
    );
    assert!(
        wat.contains("throw"),
        "zero divisors should throw the Lua error tag:\n{wat}"
    );
}

#[test]
fn pcall_catches_foreign_exceptions_and_exports_the_error_tag() {
    let source = r#"
        local ok: bool, value: unknown = pcall(function(): f64
            return 1.0
        end)
        assert(ok)
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    waluau_ir::verify(&ir).expect("ir should verify");

    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");

    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(
        wat.contains("catch_all"),
        "pcall should also catch foreign (host) exceptions:\n{wat}"
    );
    assert!(
        wat.contains("uncaught host exception"),
        "foreign exceptions should map to a fallback error payload:\n{wat}"
    );
    assert!(
        wat.contains("(export \"__waluau_error_tag\" (tag"),
        "the Lua error tag should be exported for host-thrown errors:\n{wat}"
    );
}

#[test]
fn assert_failure_message_throws_lua_error_tag() {
    let source = r#"
        function run(): unit
            assert(false, "boom")
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    waluau_ir::verify(&ir).expect("ir should verify");

    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");

    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(wat.contains("throw"), "assert should emit throw:\n{wat}");
}

#[test]
fn emits_valid_wasm_for_unknown_coroutine_extern_payloads() {
    let source = r#"
        function yield_extern(value: extern): extern
            local co: thread = coroutine.create(function(): i32
                coroutine.yield(value)
                return 11
            end)
            local ok: bool, payload: unknown = coroutine.resume(co)
            if ok then
                return payload::extern
            end
            return value
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    waluau_ir::verify(&ir).expect("ir should verify");

    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");

    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(
        wat.contains("any.convert_extern"),
        "should emit externref->anyref boxing for extern coroutine payloads"
    );
    assert!(
        wat.contains("extern.convert_any"),
        "should emit anyref->externref unboxing for extern coroutine payloads"
    );
}

#[test]
fn promise_await_bridge_imports_and_exports_runtime_helpers() {
    let source = r#"
        declare function makePromise(): extern
        declare function record_string(value: string): unit

        function run(): unit
            local co: thread = coroutine.create(function(): i32
                local value: unknown = coroutine.await_promise(makePromise())
                record_string(value::string)
                return 0
            end)
            coroutine.resume(co)
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    waluau_ir::verify(&ir).expect("ir should verify");

    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");

    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(
        wat.contains(r#"(import "waluau" "__waluau_attach_promise""#),
        "should import the Promise-attachment runtime helper"
    );
    assert!(
        wasm_export_func_index(&wasm, super::PROMISE_RESUME_TRAMPOLINE_EXPORT).is_some(),
        "promise settlement resume helper should be exported"
    );
    assert!(
        wasm_export_func_index(&wasm, super::PROMISE_RESET_ACTIVE_EXPORT).is_some(),
        "promise active-reset helper should be exported"
    );
}

#[test]
fn await_capable_function_without_coroutine_creation_emits_valid_runtime_types() {
    let source = r#"
        declare function makePromise(): extern

        function await_but_do_not_start(): i32
            return coroutine.await_promise(makePromise())::i32
        end

        function main(): i32
            return 1
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    waluau_ir::verify(&ir).expect("ir should verify");

    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn typed_promise_await_forms_lower_through_coroutine_bridge() {
    let source = r#"
        type Response = extern
        type Promise<T> = extern

        declare function fetch(url: string): Promise<Response>
        declare function make_text(): Promise<string>
        declare function record_response(value: Response): unit
        declare function record_string(value: string): unit

        function run_function_form(): unit
            local co: thread = coroutine.create(function(): i32
                local res = promise.await(fetch("/test.json"))
                local body = promise.await(make_text())
                record_response(res)
                record_string(body)
                return 0
            end)
            coroutine.resume(co)
        end

        function run_method_form(): unit
            local co: thread = coroutine.create(function(): i32
                local res = fetch("/test.json"):await()
                local body = make_text():await()
                record_response(res)
                record_string(body)
                return 0
            end)
            coroutine.resume(co)
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    waluau_ir::verify(&ir).expect("ir should verify");

    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");

    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(
        wat.contains(r#"(import "waluau" "__waluau_attach_promise""#),
        "typed Promise await forms should import the Promise bridge helper"
    );
    assert!(
        wasm_export_func_index(&wasm, super::PROMISE_RESUME_TRAMPOLINE_EXPORT).is_some(),
        "promise settlement resume helper should be exported"
    );
}

#[test]
fn emits_valid_wasm_for_coroutine_await_promise_conformance_fixture() {
    let source = r#"
        declare function make_string_promise(): extern
        declare function make_object_promise(): extern
        declare function make_rejected_promise(): extern
        declare function record_string(value: string): unit
        declare function record_object(value: extern): unit
        declare function record_nested(value: string): unit
        declare function record_status(value: string): unit

        function run_string(): unit
            local co: thread = coroutine.create(function(): i32
                local value: string = coroutine.await_promise(make_string_promise())::string
                record_string(value)
                return 0
            end)
            coroutine.resume(co)
        end

        function run_object(): unit
            local co: thread = coroutine.create(function(): i32
                local value: extern = coroutine.await_promise(make_object_promise())::extern
                record_object(value)
                return 0
            end)
            coroutine.resume(co)
        end

        function run_nested(): unit
            local inner: thread = coroutine.create(function(): i32
                coroutine.yield("inner-yield")
                return 17
            end)
            local outer: thread = coroutine.create(function(): i32
                local value: string = coroutine.await_promise(make_string_promise())::string
                local ok1: bool, payload1: unknown = coroutine.resume(inner)
                assert(ok1)
                local ok2: bool, payload2: unknown = coroutine.resume(inner)
                assert(ok2)
                record_nested(value .. ":" .. payload1::string .. ":" .. tostring(payload2::i32))
                return 0
            end)
            coroutine.resume(outer)
        end

        function run_rejected(): unit
            local co: thread = coroutine.create(function(): i32
                coroutine.await_promise(make_rejected_promise())
                record_status("unexpected")
                return 0
            end)
            coroutine.resume(co)
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    waluau_ir::verify(&ir).expect("ir should verify");

    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn emits_valid_wasm_for_tagged_union_constructor() {
    let source = include_str!("../../../conformance/tagged_union_constructor.walu");
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    waluau_ir::verify(&ir).expect("ir should verify");

    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");

    let wat = print_bytes(&wasm).expect("wat should print");
    // Constructor should emit struct.new for the canonical tagged-union record.
    assert!(
        wat.contains("struct.new"),
        "should emit struct.new for tagged-union constructor"
    );
}

#[test]
fn emits_valid_wasm_for_tagged_union_alias_cast() {
    let source = include_str!("../../../conformance/tagged_union_alias_cast.walu");
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    waluau_ir::verify(&ir).expect("ir should verify");

    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn emits_valid_wasm_for_tagged_union_pattern_match_binding() {
    let source = include_str!("../../../conformance/tagged_union_pattern_match_binding.walu");
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    waluau_ir::verify(&ir).expect("ir should verify");

    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");

    let wat = print_bytes(&wasm).expect("wat should print");
    // The pattern-match condition lowers to a tag check followed by an unbox cast.
    assert!(
        wat.contains("struct.get"),
        "should emit struct.get for the tag check and payload unbox"
    );
}

#[test]
fn emits_valid_wasm_for_varargs_and_table_pack_conformance_fixtures() {
    for source in [
        include_str!("../../../conformance/varargs_forwarding.walu"),
        include_str!("../../../conformance/varargs_return_spread.walu"),
        include_str!("../../../conformance/varargs_table_literal.walu"),
        include_str!("../../../conformance/table_pack.walu"),
        include_str!("../../../conformance/select_n.walu"),
        include_str!("../../../conformance/varargs_checkresults.walu"),
    ] {
        let program = waluau_parser::parse(source).expect("parse should succeed");
        let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
        let ir = waluau_ir::build(&typed).expect("ir should succeed");
        waluau_ir::verify(&ir).expect("ir should verify");

        let wasm = emit(&ir).expect("emit should succeed");
        Validator::new_with_features(wasmparser::WasmFeatures::all())
            .validate_all(&wasm)
            .expect("emitted module should validate");
    }
}

#[test]
fn emits_valid_wasm_for_table_create_and_static_unpack() {
    // Mirrors conformance/table_create_unpack.walu minus the string.format /
    // string.char checks, which need host imports this harness does not
    // declare. Covers the fill loop, count zero, the empty one-argument form,
    // literal unpack bounds, and expected-arity unpack from an annotated
    // multi-binding declaration.
    let source = r#"
local filled: {i32} = table.create(3, 7)
assert(#filled == 3)
assert(filled[0] == 7)
assert(filled[2] == 7)

local empty_filled: {f64} = table.create(0, 1.5)
assert(#empty_filled == 0)

local hinted: {string} = table.create(8)
assert(#hinted == 0)

local ys: {i32} = {10, 20, 30, 40, 50}
local second: i32, third: i32 = table.unpack(ys, 1, 2)
assert(second == 20)
assert(third == 30)

local fourth: i32, fifth: i32 = table.unpack(ys, 3)
assert(fourth == 40)
assert(fifth == 50)

local first: f64 = table.unpack(table.create(4, 6.5))
assert(first == 6.5)
"#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    waluau_ir::verify(&ir).expect("ir should verify");

    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn emits_valid_wasm_for_dynamic_unknown_operations() {
    for source in [
        include_str!("../../../conformance/unknown_equality.walu"),
        include_str!("../../../conformance/unknown_len_index.walu"),
    ] {
        let program = waluau_parser::parse(source).expect("parse should succeed");
        let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
        let ir = waluau_ir::build(&typed).expect("ir should succeed");
        waluau_ir::verify(&ir).expect("ir should verify");

        let wasm = emit(&ir).expect("emit should succeed");
        Validator::new_with_features(wasmparser::WasmFeatures::all())
            .validate_all(&wasm)
            .expect("emitted module should validate");
    }
}

#[test]
fn unknown_equality_imports_js_eq_unknown() {
    let source = r#"
        function eq(a, b)
            return a == b
        end
        assert(eq("x", "x"))
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");

    let mut found = false;
    for payload in Parser::new(0).parse_all(&wasm) {
        if let Payload::ImportSection(imports) = payload.expect("wasm should parse") {
            for import in imports {
                let import = import.expect("import should parse");
                if import.module == "waluau" && import.name == "js_eq_unknown" {
                    found = true;
                }
            }
        }
    }
    assert!(found, "unknown equality should import waluau.js_eq_unknown");
}

#[test]
fn extern_reference_equality_imports_js_eq_unknown() {
    let source = r#"
        type Node = extern
        type Element = extern extends Node
        declare function make_element(): Element

        local a: Element = make_element()
        local b: Element = make_element()
        local c: Element? = a
        assert(a == a)
        assert(a ~= b)
        assert(c == a)
        assert(c ~= b)
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    waluau_ir::verify(&ir).expect("ir should verify");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");

    let mut found = false;
    for payload in Parser::new(0).parse_all(&wasm) {
        if let Payload::ImportSection(imports) = payload.expect("wasm should parse") {
            for import in imports {
                let import = import.expect("import should parse");
                if import.module == "waluau" && import.name == "js_eq_unknown" {
                    found = true;
                }
            }
        }
    }
    assert!(found, "extern equality should import waluau.js_eq_unknown");
}

#[test]
fn string_and_bytes_inequality_import_host_eq_helpers() {
    let source = r#"
        local a: string = "a"
        local b: string = "b"
        assert(a ~= b)
        local c = b"ab"
        local d = b"cd"
        assert(c ~= d)
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");

    let mut found_string_eq = false;
    let mut found_bytes_eq = false;
    for payload in Parser::new(0).parse_all(&wasm) {
        if let Payload::ImportSection(imports) = payload.expect("wasm should parse") {
            for import in imports {
                let import = import.expect("import should parse");
                if import.module == "wasm:js-string" && import.name == "equals" {
                    found_string_eq = true;
                }
                if import.module == "waluau" && import.name == "bytes_eq" {
                    found_bytes_eq = true;
                }
            }
        }
    }
    assert!(
        found_string_eq,
        "string inequality should import the js-string equals builtin"
    );
    assert!(
        found_bytes_eq,
        "bytes inequality should import waluau.bytes_eq"
    );
}

#[test]
fn declared_import_overloads_emit_one_host_import_per_overload() {
    let source = r#"
        type Ctx = extern
        declare function get_ctx(): Ctx
        declare function Ctx:fill(): unit
        declare function Ctx:fill(rule: string): unit
        declare function pick(x: f32): f32
        declare function pick(x: f64): f64

        function paint(): f64
            local c: Ctx = get_ctx()
            c:fill()
            c:fill("evenodd")
            local narrow: f32 = pick(1.5::f32)
            return pick(2.5) + narrow::f64
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");

    // Each overload becomes its own host import under the shared external
    // name, with the overload's own signature.
    let mut fill_imports = 0;
    let mut pick_imports = 0;
    for payload in Parser::new(0).parse_all(&wasm) {
        let payload = payload.expect("wasm should parse");
        if let Payload::ImportSection(reader) = payload {
            for import in reader {
                let import = import.expect("import should decode");
                if import.module != "waluau" {
                    continue;
                }
                match import.name {
                    "Ctx.fill" => fill_imports += 1,
                    "pick" => pick_imports += 1,
                    _ => {}
                }
            }
        }
    }
    assert_eq!(fill_imports, 2, "expected one import per fill overload");
    assert_eq!(pick_imports, 2, "expected one import per pick overload");
}

#[test]
fn numeric_for_over_array_length_with_untyped_literal_bound_emits_valid_wasm() {
    // Regression: `for i = 0, #a - 1` used to infer the loop variable as f64
    // from the untyped `0` while the `#a - 1` bound stayed i32, emitting
    // invalid wasm ("type mismatch: expected f64, found i32"). The untyped
    // literal now adopts the i32 type of the typed bound.
    let source = r#"
        local a: {i32} = {10, 20, 30}
        local sum: i32 = 0
        for i = 0, #a - 1 do
            sum += a[i]
        end
        assert(sum == 60)
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");

    // The loop runs entirely on i32 values; an f64 loop variable would show
    // up as float comparisons in the loop header.
    let wat = print_bytes(&wasm).expect("wat should print");
    assert!(
        !wat.contains("f64.lt") && !wat.contains("f64.gt"),
        "loop over i32 bounds should not compare as f64:\n{wat}"
    );
}

#[test]
fn countdown_numeric_for_with_untyped_literal_bounds_emits_valid_wasm() {
    // Regression: the i32-typed `#a - 1` start used to be labelled f64 when
    // the untyped `0` stop defaulted the loop type to f64.
    let source = r#"
        local a: {string} = {"x", "y", "z"}
        local count: i32 = 0
        for i = #a - 1, 0, -1 do
            count += 1
        end
        assert(count == 3)
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn binary_expression_result_coerces_to_a_wider_expected_type() {
    // Regression: lowering a binary expression against a wider expected type
    // (`local x: f64 = m + m` with i32 operands) used to stamp the i32 result
    // as f64 without emitting a conversion, producing invalid wasm. The same
    // shape boxed the raw i32 into anyref for `unknown` targets.
    let source = r#"
        local m: i32 = 2
        local x: f64 = m + m
        local u: unknown = m + m
        assert(x == 4)
        assert((u :: i32) == 4)
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
}

#[test]
fn nullable_primitive_nil_check_lowers_to_ref_is_null() {
    let source = r#"
        function unwrap_or_zero(value: i32?): i32
            if value ~= nil then
                return value + 1
            end
            return 0
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    let wat = print_bytes(&wasm).expect("wat should print");

    assert!(
        wat.contains("ref.is_null"),
        "nullable primitive nil checks should lower to ref.is_null in:\n{wat}"
    );
    assert!(
        wat.contains("struct.get"),
        "narrowed nullable primitive reads should unwrap the box via struct.get in:\n{wat}"
    );
}

#[test]
fn nullable_primitive_wrap_lowers_to_struct_new() {
    let source = r#"
        function wrap(flag: bool, value: f64): f64?
            if flag then
                return value
            end
            return nil
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    let wat = print_bytes(&wasm).expect("wat should print");

    assert!(
        wat.contains("struct.new"),
        "wrapping f64 into f64? should lower to struct.new in:\n{wat}"
    );
    assert!(
        wat.contains("ref.null"),
        "the nil arm of f64? should lower to a typed null reference in:\n{wat}"
    );
}

#[test]
fn nullable_primitive_module_exports_box_helpers() {
    let source = r#"
        function passthrough(value: u32?): u32?
            return value
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");

    assert!(
        wasm_export_func_index(&wasm, "__waluau_box_nullable_i32").is_some(),
        "modules using u32? should export the i32 nullable box constructor"
    );
    assert!(
        wasm_export_func_index(&wasm, "__waluau_unbox_nullable_i32").is_some(),
        "modules using u32? should export the i32 nullable box reader"
    );
}

#[test]
fn dyn_index_over_nullable_primitive_array_branches_on_null() {
    // Dynamically indexing an `unknown` that holds a `{i32?}` must not trap:
    // the element is a typed nullable box, so the dispatch branches on null
    // (nil becomes the `unknown` nil) and reboxes a present payload into the
    // canonical `unknown` representation.
    let source = r#"
        function get(v, i)
            return v[i]
        end

        function first(values: {i32?}): unknown
            return get(values, 0)
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    let wat = print_bytes(&wasm).expect("wat should print");

    assert!(
        wat.contains("br_on_null"),
        "dyn index over a nullable primitive array should branch on the null box in:\n{wat}"
    );
    assert!(
        wat.contains("ref.i31"),
        "a present i32? element should rebox into the canonical i31 unknown form in:\n{wat}"
    );
}

#[test]
fn dyn_index_over_nullable_f64_array_reboxes_payload() {
    let source = r#"
        function get(v, i)
            return v[i]
        end

        function first(values: {f64?}): unknown
            return get(values, 0)
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    let wat = print_bytes(&wasm).expect("wat should print");

    assert!(
        wat.contains("br_on_null"),
        "dyn index over {{f64?}} should branch on the null box in:\n{wat}"
    );
    // The immutable `(struct (field f64))` is the canonical unknown box; the
    // mutable one is the typed nullable box the payload is read out of.
    assert!(
        wat.contains("(struct (field f64))"),
        "a present f64? element should rebox into $boxed_f64 in:\n{wat}"
    );
}

#[test]
fn dyn_index_over_nullable_i64_array_still_fails() {
    // `i64?`/`u64?`/`f32?` payloads have no canonical `unknown` boxed form
    // (waluau-agmp / design 0010), so those element kinds stay out of the
    // dispatch and fall through to the failure path, exactly like plain
    // i64/f32 elements. Since #408 that path throws the catchable Lua error
    // tag rather than emitting `unreachable`. The module must still validate.
    let source = r#"
        function get(v, i)
            return v[i]
        end

        function first(values: {i64?}): unknown
            return get(values, 0)
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
    let wat = print_bytes(&wasm).expect("wat should print");

    assert!(
        !wat.contains("br_on_null"),
        "an i64? element kind must not enter the dyn index dispatch in:\n{wat}"
    );
    assert!(
        wat.contains("throw"),
        "dyn index over an unsupported element kind should raise a Lua error in:\n{wat}"
    );
}

#[test]
fn arrays_and_records_with_nullable_primitives_compile() {
    let source = r#"
        type Slot = { count: u32?, label: string }

        function sum_present(values: {i32?}): i32
            local total: i32 = 0
            for _, value in values do
                if value ~= nil then
                    total += value
                end
            end
            return total
        end

        function slot_count(slot: Slot): u32
            local count: u32? = slot.count
            if count ~= nil then
                return count
            end
            return 0
        end

        function make(): {i32?}
            local values: {i32?} = { 1, nil, 3 }
            return values
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module with nullable-primitive storage should validate");
    let wat = print_bytes(&wasm).expect("wat should print");

    // The array and record storage must hold the nullable box ref, not the
    // raw scalar: the nullable box struct type appears in the type section
    // and the array element type references it.
    assert!(
        wat.contains("(struct (field (mut i32)))"),
        "expected the nullable i32 box struct type in:\n{wat}"
    );
}

#[test]
fn declared_import_metadata_carries_nullable_primitive_types() {
    let source = r#"
        type HTMLInputElement = extern

        declare property HTMLInputElement:selectionStart: u32?

        function probe(input: HTMLInputElement): u32
            local start: u32? = input.selectionStart
            if start ~= nil then
                return start
            end
            return 0
        end

        function reset(input: HTMLInputElement): unit
            input.selectionStart = nil
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let emitted = super::emit(&ir).expect("emit should succeed");

    let getter = emitted
        .required_imports
        .iter()
        .find(|import| import.name == "HTMLInputElement.get/selectionStart")
        .expect("getter import should be required");
    assert_eq!(getter.return_type.as_deref(), Some("u32?"));

    let setter = emitted
        .required_imports
        .iter()
        .find(|import| import.name == "HTMLInputElement.set/selectionStart")
        .expect("setter import should be required");
    // Opaque extern names are erased before codegen; the receiver reads as
    // `extern`, but the nullable primitive keeps its surface syntax.
    assert_eq!(
        setter.param_types.as_deref(),
        Some(&["extern".to_string(), "u32?".to_string()][..])
    );
}

#[test]
fn literal_unions_emit_valid_wasm_at_their_representation_types() {
    let source = r#"
        type CardColor = "red" | "black"
        type Volume = 0 | 1 | 2

        function flip(color: CardColor): CardColor
            if color == "red" then
                return "black"
            end
            return "red"
        end

        function louder(volume: Volume): Volume
            if volume < 2 then
                return ((volume + 1) :: Volume)
            end
            return volume
        end

        local color: CardColor = "red"
        assert(flip(color) == "black")
        local volume: Volume = 1
        assert(louder(volume) == 2)
        local as_number: f64 = volume
        assert(as_number == 1)
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("literal unions should emit");

    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted literal-union module should validate");
}

/// Decode the `name` custom section: function names plus per-function local names.
fn wasm_debug_names(
    wasm: &[u8],
) -> (
    std::collections::BTreeMap<u32, String>,
    std::collections::BTreeMap<u32, std::collections::BTreeMap<u32, String>>,
) {
    let mut function_names = std::collections::BTreeMap::new();
    let mut local_names = std::collections::BTreeMap::new();
    for payload in Parser::new(0).parse_all(wasm) {
        let payload = payload.expect("wasm should parse");
        let Payload::CustomSection(reader) = payload else {
            continue;
        };
        let wasmparser::KnownCustom::Name(name_reader) = reader.as_known() else {
            continue;
        };
        for name in name_reader {
            match name.expect("name subsection should decode") {
                wasmparser::Name::Function(map) => {
                    for naming in map {
                        let naming = naming.expect("function name should decode");
                        function_names.insert(naming.index, naming.name.to_string());
                    }
                }
                wasmparser::Name::Local(indirect) => {
                    for entry in indirect {
                        let entry = entry.expect("local name entry should decode");
                        let mut names = std::collections::BTreeMap::new();
                        for naming in entry.names {
                            let naming = naming.expect("local name should decode");
                            names.insert(naming.index, naming.name.to_string());
                        }
                        local_names.insert(entry.index, names);
                    }
                }
                _ => {}
            }
        }
    }
    (function_names, local_names)
}

#[test]
fn emits_debug_names_for_functions_params_and_locals() {
    let source = r#"
        function accumulate(seed: i32, count: i32): i32
            local total: i32 = seed
            for step = 1, count do
                total += step
            end
            return total
        end

        assert(accumulate(2, 3) == 8)
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");

    let (function_names, local_names) = wasm_debug_names(&wasm);
    let accumulate_index = *function_names
        .iter()
        .find(|(_, name)| name.as_str() == "accumulate")
        .expect("accumulate should be named in the name section")
        .0;
    assert!(
        function_names
            .values()
            .any(|name| name == "__waluau_top_level_init"),
        "top-level init should be named, got: {function_names:?}"
    );
    let export_index =
        wasm_export_func_index(&wasm, "accumulate").expect("accumulate should be exported");
    assert_eq!(accumulate_index, export_index);

    let locals = local_names
        .get(&accumulate_index)
        .expect("accumulate should have local names");
    assert_eq!(locals.get(&0).map(String::as_str), Some("seed"));
    assert_eq!(locals.get(&1).map(String::as_str), Some("count"));
    let named: Vec<&str> = locals.values().map(String::as_str).collect();
    assert!(
        named.contains(&"total"),
        "expected a local named 'total', got: {named:?}"
    );
    assert!(
        named.contains(&"step"),
        "expected a local named 'step', got: {named:?}"
    );
}

#[test]
fn incremental_emit_preserves_the_name_section() {
    let source_v1 = r#"
        function answer(base: i32): i32
            local result: i32 = base + 41
            result += 1
            return result
        end

        assert(answer(0) == 42)
    "#;
    // Same shape, numeric literals only: stays on the incremental emit path.
    let source_v2 = r#"
        function answer(base: i32): i32
            local result: i32 = base + 40
            result += 1
            return result
        end

        assert(answer(0) == 42)
    "#;
    let mut cache = super::EmitCache::default();
    let build = |source: &str| {
        let program = waluau_parser::parse(source).expect("parse should succeed");
        let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
        waluau_ir::build(&typed).expect("ir should succeed")
    };
    super::emit_cached(&build(source_v1), &mut cache).expect("full emit should succeed");
    assert!(!cache.last_emit_was_incremental());
    let incremental =
        super::emit_cached(&build(source_v2), &mut cache).expect("incremental emit should succeed");
    assert!(
        cache.last_emit_was_incremental(),
        "numeric-literal-only change should re-emit incrementally"
    );

    let (function_names, local_names) = wasm_debug_names(&incremental.wasm);
    let answer_index = *function_names
        .iter()
        .find(|(_, name)| name.as_str() == "answer")
        .expect("answer should stay named after an incremental emit")
        .0;
    let locals = local_names
        .get(&answer_index)
        .expect("answer should keep local names after an incremental emit");
    assert!(
        locals.values().any(|name| name == "result"),
        "expected a local named 'result', got: {locals:?}"
    );
}

#[test]
fn structured_fast_path_emits_inverted_polarity_loop_headers_faithfully() {
    // A generic-for protocol loop lowers to a 4-block header-check loop whose
    // header branches then = exit, else = body — the opposite polarity of a
    // `while` header. The second fast-path loop arm once claimed this shape
    // but emitted the body before the check and could even select the entry
    // block as the body (dropping the real one), which validated fine and ran
    // as an infinite loop. Guard the faithful emission: the fast path is
    // taken (one structured loop, no PC-dispatch `unreachable` backstops),
    // the header's null check appears exactly once, and the loop body is
    // present (the control unbox reads the nullable box struct).
    let source = r#"
function iter(a: {i32}, i: i32): (i32?, i32)
    local n = i + 1
    if n < #a then
        return n, a[n]
    end
    return nil, 0
end

function sum_plain(a: {i32}): i32
    local total: i32 = 0
    for i, v in iter, a, -1 do
        total = total + v
    end
    return total
end
"#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let ir = waluau_ir::build(&typed).expect("ir should succeed");
    let wasm = emit(&ir).expect("emit should succeed");
    Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&wasm)
        .expect("emitted module should validate");

    let func_index = wasm_export_func_index(&wasm, "sum_plain").expect("sum_plain export");
    let code_index = func_index - wasm_function_import_count(&wasm);
    let mut entry = 0u32;
    let mut loops = 0;
    let mut null_checks = 0;
    let mut struct_gets = 0;
    let mut unreachables = 0;
    for payload in Parser::new(0).parse_all(&wasm) {
        let payload = payload.expect("wasm should parse");
        if let Payload::CodeSectionEntry(body) = payload {
            if entry != code_index {
                entry += 1;
                continue;
            }
            let mut reader = body.get_operators_reader().expect("ops should decode");
            while !reader.eof() {
                match reader.read().expect("op should decode") {
                    Operator::Loop { .. } => loops += 1,
                    Operator::RefIsNull => null_checks += 1,
                    Operator::StructGet { .. } => struct_gets += 1,
                    Operator::Unreachable => unreachables += 1,
                    _ => {}
                }
            }
            break;
        }
    }
    assert_eq!(loops, 1, "the loop must emit as one structured wasm loop");
    assert_eq!(
        null_checks, 1,
        "the header's nil check must appear exactly once — a duplicated or dropped check means the entry block was mistaken for the loop body"
    );
    assert!(
        struct_gets >= 1,
        "the loop body must be emitted: narrowing the control value reads the nullable box struct"
    );
    assert_eq!(
        unreachables, 0,
        "the structured fast path should handle this shape rather than falling back to PC dispatch"
    );
}
