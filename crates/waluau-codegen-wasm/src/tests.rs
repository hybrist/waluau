use waluau_ast::BinaryOp;
use waluau_ir::Instruction as IrInstruction;
use wasmparser::{Operator, Parser, Payload, Validator};
use wasmprinter::print_bytes;

#[test]
fn emits_valid_wasm_for_scalar_program() {
    let source = r#"
        function entry(x: i32): i32
            return x + 1
        end
    "#;
    let program = waluau_parser::parse(source).expect("parse should succeed");
    let ir = waluau_ir::build(&program).expect("ir should succeed");
    let wasm = super::emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
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
    let wasm = super::emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
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
    let wasm = super::emit(&ir).expect("emit should succeed");
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
    let wasm = super::emit(&ir).expect("capturing closures should compile");
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
    let wasm = super::emit(&ir).expect("emit should succeed");
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
    let wasm = super::emit(&ir).expect("emit should succeed");
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
    let wasm = super::emit(&ir).expect("emit should succeed");
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
    let wasm = super::emit(&ir).expect("emit should succeed");
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
    let wasm = super::emit(&ir).expect("emit should succeed");
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
    let wasm = super::emit(&ir).expect("emit should succeed");
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
    let wasm = super::emit(&ir).expect("emit should succeed");
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
    let array_registry = super::arrays::ArrayTypeRegistry::with_function_type_offset(
        &array_types,
        ir.functions.len() as u32 + u32::from(ir.start.is_some()),
        0, // anyref_array_type placeholder (unused in this test)
        0, // func_val_struct_type placeholder (unused in this test)
    );
    let local_plan = super::build_local_plan(function, &value_types, &array_registry)
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
    let wasm = super::emit(&ir);
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
    let wasm = super::emit(&ir).expect("emit should succeed");
    Validator::new()
        .validate_all(&wasm)
        .expect("emitted module should validate");
}
