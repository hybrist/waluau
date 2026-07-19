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
    let local_plan = super::build_local_plan(function, &value_types, &array_registry, false)
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
        wat.contains("(export \"__waluau_error\" (tag"),
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
