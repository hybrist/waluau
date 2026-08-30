use std::collections::BTreeMap;
use std::sync::Arc;

use super::{
    BasicBlock, BlockId, BuildCache, Function, FunctionSourceMap, Instruction, Module,
    SourceFileId, SourceLocation, SourceOrigin, Terminator, ValueId, build,
    build_cached_with_changes, verify,
};
use waluau_ast::{BinaryOp, NumberLiteral, NumericType, Type};
use waluau_diagnostics::DiagnosticCategory;
use waluau_parser::{parse, parse_with_path};

fn authored(origin: SourceOrigin) -> SourceLocation {
    match origin {
        SourceOrigin::Authored(location) => location,
        SourceOrigin::Synthetic => panic!("expected authored source location"),
    }
}

#[test]
fn preserves_function_instruction_and_terminator_locations() {
    let source = "function entry(x: i32): i32\n    return x + 42\nend\n";
    let program = parse_with_path(source, "src/main.walu").expect("parse should succeed");
    let module = build(&program).expect("IR build should succeed");
    assert_eq!(
        module.source_files,
        vec![super::SourceFile {
            path: "src/main.walu".to_string(),
            source: source.to_string(),
        }]
    );

    let function = &module.functions[0];
    assert_eq!(
        authored(function.source_map.definition).file,
        SourceFileId(0)
    );
    let entry = &function.blocks[&function.entry];
    let (param, _) = entry
        .instructions
        .iter()
        .find(|(_, instruction)| matches!(instruction, Instruction::Param(0)))
        .expect("parameter instruction");
    assert_eq!(
        function.source_map.instruction_origin(*param),
        SourceOrigin::Synthetic,
        "ABI parameter setup is compiler-generated"
    );
    let (literal, _) = entry
        .instructions
        .iter()
        .find(|(_, instruction)| matches!(instruction, Instruction::Number { literal, .. } if literal.raw == "42"))
        .expect("number instruction");
    let literal_location = authored(function.source_map.instruction_origin(*literal));
    assert_eq!(literal_location.file, SourceFileId(0));
    assert_eq!(
        &source[literal_location.span.start as usize..literal_location.span.end as usize],
        "42"
    );
    let return_location = authored(function.source_map.terminator_origin(function.entry));
    assert_eq!(return_location.file, SourceFileId(0));
    assert_eq!(
        &source[return_location.span.start as usize..return_location.span.end as usize],
        "x + 42"
    );
}

#[test]
fn leaves_compiler_generated_function_bodies_unmapped() {
    let source = "local value: i32 = 42\n";
    let program = parse_with_path(source, "src/main.walu").expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("IR build should succeed");
    let init = module
        .functions
        .iter()
        .find(|function| function.name == "__waluau_top_level_init")
        .expect("synthetic top-level initializer");
    assert_eq!(init.source_map.definition, SourceOrigin::Synthetic);
    assert!(init.blocks.values().all(|block| {
        block
            .instructions
            .iter()
            .all(|(value, _)| init.source_map.instruction_origin(*value) == SourceOrigin::Synthetic)
            && init.source_map.terminator_origin(block.id) == SourceOrigin::Synthetic
    }));
}

#[test]
fn lowers_numeric_concat_and_tostring_metamethod() {
    let source = r#"
        enum SpellKind { Firebolt, FreezeRay }

        function SpellKind:__tostring(): string
            if self == SpellKind.Firebolt then return "Firebolt" end
            return "Freeze Ray"
        end

        type Greeting = { text: string }

        function Greeting:__concat(suffix: string): string
            return self.text .. suffix
        end

        function entry(greeting: Greeting, kind: SpellKind, count: i32): string
            return greeting .. (" spell=" .. tostring(kind)
                .. ", number=" .. kind::number
                .. ", i32=" .. kind::i32
                .. ", count=" .. count)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("IR build should succeed");
    let entry = module
        .functions
        .iter()
        .find(|function| function.name == "entry")
        .expect("entry function");
    assert!(entry.blocks.values().any(|block| {
        block.instructions.iter().any(|(_, instruction)| {
            matches!(
                instruction,
                Instruction::ToString {
                    from: Type::Numeric(NumericType::I32),
                    ..
                }
            )
        })
    }));
    assert!(entry.blocks.values().any(|block| {
        block.instructions.iter().any(|(_, instruction)| {
            matches!(
                instruction,
                Instruction::Call { name, .. } if name == "Greeting.__concat"
            )
        })
    }));
    assert!(entry.blocks.values().any(|block| {
        block.instructions.iter().any(|(_, instruction)| {
            matches!(
                instruction,
                Instruction::ToString {
                    from: Type::Numeric(NumericType::F64),
                    ..
                }
            )
        })
    }));
    assert!(entry.blocks.values().any(|block| {
        block.instructions.iter().any(|(_, instruction)| {
            matches!(
                instruction,
                Instruction::Call { name, .. } if name == "SpellKind.__tostring"
            )
        })
    }));
}

#[test]
fn preserves_files_for_linked_and_lifted_functions() {
    let alpha_source = "function alpha(): i32\n    return 11\nend\n";
    let beta_source = concat!(
        "function beta(): i32\n",
        "    local inner: () -> i32 = function(): i32\n",
        "        return 22\n",
        "    end\n",
        "    return inner()\n",
        "end\n",
    );
    let mut program = parse_with_path(alpha_source, "src/alpha.walu").expect("alpha should parse");
    let beta = parse_with_path(beta_source, "src/beta.walu").expect("beta should parse");
    program.functions.extend(beta.functions);
    program.sources.extend(beta.sources);

    let module = build(&program).expect("linked IR build should succeed");
    assert_eq!(module.source_files[0].path, "src/alpha.walu");
    assert_eq!(module.source_files[1].path, "src/beta.walu");
    for name in ["alpha", "beta", "beta$lambda0"] {
        let function = module
            .functions
            .iter()
            .find(|function| function.name == name)
            .unwrap_or_else(|| panic!("missing {name}"));
        let expected_file = if name == "alpha" {
            SourceFileId(0)
        } else {
            SourceFileId(1)
        };
        assert_eq!(authored(function.source_map.definition).file, expected_file);
        assert!(function.blocks.values().any(|block| {
            block.instructions.iter().any(|(value, _)| {
                matches!(
                    function.source_map.instruction_origin(*value),
                    SourceOrigin::Authored(location) if location.file == expected_file
                )
            })
        }));
    }
}

#[test]
fn incremental_rebuild_refreshes_source_table_and_spans() {
    let source_v1 = "function entry(): i32\n    return 1\nend\n";
    let source_v2 = "function entry(): i32\n    return 200\nend\n";
    let program_v1 = parse_with_path(source_v1, "src/main.walu").expect("v1 should parse");
    let program_v2 = parse_with_path(source_v2, "src/main.walu").expect("v2 should parse");
    let mut cache = BuildCache::default();
    build_cached_with_changes(&program_v1, &mut cache, &[0]).expect("full build should succeed");
    let module = build_cached_with_changes(&program_v2, &mut cache, &[0])
        .expect("incremental build should succeed")
        .clone();
    assert!(cache.last_build_was_incremental());
    assert_eq!(module.source_files[0].source, source_v2);
    let function = &module.functions[0];
    let (value, _) = function
        .blocks
        .values()
        .flat_map(|block| &block.instructions)
        .find(|(_, instruction)| matches!(instruction, Instruction::Number { literal, .. } if literal.raw == "200"))
        .expect("updated number instruction");
    let location = authored(function.source_map.instruction_origin(*value));
    assert_eq!(
        &source_v2[location.span.start as usize..location.span.end as usize],
        "200"
    );
}

#[test]
fn inserts_phi_after_if_merge() {
    let source = r#"
        function entry(flag: bool, x: i32): i32
            local y: i32 = x
            if flag then
                y = y + 1
            else
                y = y + 2
            end
            return y
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = &module.functions[0];
    let has_merge_phi = function.blocks.values().any(|block| {
        block
            .instructions
            .iter()
            .any(|(_, instruction)| matches!(instruction, Instruction::Phi(incoming) if incoming.len() == 2))
    });
    assert!(
        has_merge_phi,
        "expected merge phi in function:\n{}",
        function.dump()
    );
}

#[test]
fn lowers_nominal_enum_match_to_i32_branches() {
    let source = r#"
        enum Direction { north, east, south }
        function entry(direction: Direction): i32
            match direction do
            case Direction.north then return 1
            case Direction.east then return 2
            case Direction.south then return 3
            end
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    verify(&module).expect("enum match IR should verify");
    let function = &module.functions[0];
    assert_eq!(function.params[0].1, Type::Numeric(NumericType::I32));
    assert_eq!(
        function
            .blocks
            .values()
            .filter(|block| matches!(block.terminator, Terminator::Branch { .. }))
            .count(),
        2,
        "three enum variants should lower to two comparisons:\n{}",
        function.dump()
    );
}

#[test]
fn lowers_declared_extern_operator_overload_to_host_call() {
    let source = r#"
        type Tensor = extern
        declare function make_tensor(): Tensor
        declare function Tensor:__add(rhs: Tensor): Tensor

        function entry(): Tensor
            return make_tensor() + make_tensor()
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    let function = module
        .functions
        .iter()
        .find(|function| function.name == "entry")
        .expect("entry function should exist");
    let has_operator_host_call = function.blocks.values().any(|block| {
        block.instructions.iter().any(|(_, instruction)| {
            matches!(
                instruction,
                Instruction::HostCall { name, .. } if name == "Tensor.__add"
            )
        })
    });
    assert!(
        has_operator_host_call,
        "expected Tensor.__add host call in function:\n{}",
        function.dump()
    );
}

#[test]
fn lowers_omitted_trailing_nullable_args_as_typed_nulls() {
    let source = r#"
        declare function host(value: string?): unit

        function local_sink(value: string?): unit
        end

        function entry(): unit
            local_sink()
            host()
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    let entry = module
        .functions
        .iter()
        .find(|function| function.name == "entry")
        .expect("entry function should exist");

    let mut null_count = 0;
    let mut saw_local_call = false;
    let mut saw_host_call = false;
    for block in entry.blocks.values() {
        for (_, instruction) in &block.instructions {
            match instruction {
                Instruction::Null { ty } if *ty == Type::String => null_count += 1,
                Instruction::Call { name, args, .. } if name == "local_sink" => {
                    saw_local_call = args.len() == 1;
                }
                Instruction::HostCall { name, args, .. } if name == "host" => {
                    saw_host_call = args.len() == 1;
                }
                _ => {}
            }
        }
    }
    assert_eq!(
        null_count, 2,
        "expected one typed null per omitted argument"
    );
    assert!(saw_local_call, "local call should retain its Wasm arity");
    assert!(saw_host_call, "host call should retain its Wasm arity");
}

#[test]
fn lowers_nil_for_declared_nullable_callback_parameter() {
    let source = r#"
        type Event = extern
        declare function listen(callback: ((Event) -> unit)?): unit

        function clear(): unit
            listen(nil)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    let clear = module
        .functions
        .iter()
        .find(|function| function.name == "clear")
        .expect("clear function should exist");

    let mut saw_function_null = false;
    let mut saw_host_call = false;
    for block in clear.blocks.values() {
        for (_, instruction) in &block.instructions {
            match instruction {
                Instruction::Null {
                    ty: Type::Function { .. },
                } => {
                    saw_function_null = true;
                }
                Instruction::HostCall { name, args, .. } if name == "listen" => {
                    saw_host_call = args.len() == 1;
                }
                _ => {}
            }
        }
    }
    assert!(
        saw_function_null,
        "nil should lower to a typed function null"
    );
    assert!(
        saw_host_call,
        "nullable callback host call should retain its Wasm arity"
    );
}

#[test]
fn lowers_nullable_options_records_and_missing_fields() {
    let source = r#"
        type Node = extern
        type Document = extern
        type Element = extern extends Node

        declare function Document:create_element(tag: string): Element
        declare function get_document(): Document
        declare property Element:id: string
        declare property Element:class: string

        type ElementOptions = {
            id: string?,
            class: string?,
            children: {Node}?
        }
        type h = { doc: Document }

        function h:main(opts: ElementOptions?): Element
            local element: Element = self.doc:create_element("main")
            if opts == nil then
                return element
            end
            if opts.id ~= nil then
                element.id = opts.id
            end
            if opts.class ~= nil then
                element.class = opts.class
            end
            return element
        end

        local h: h = { doc = get_document() }
        h:main()
        h:main { id = "my-el" }
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    build(&typed).expect("options-object program should lower and verify");
}

#[test]
fn lowers_scalar_string_byte_as_nullable_without_changing_host_abi() {
    let source = r#"
        declare function string_byte(value: string, index: i32): i32

        function missing(): bool
            return string.byte("", 1) == nil
        end

        function present(): bool
            return string.byte("A", 1) == 65
        end

        function numeric(value: string): i32
            local byte: i32 = string.byte(value, 1)
            return byte
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");

    let mut string_byte_calls = 0;
    let mut nullable_nulls = 0;
    let mut boxes = 0;
    let mut unboxes = 0;
    for function in &module.functions {
        for block in function.blocks.values() {
            for (_, instruction) in &block.instructions {
                match instruction {
                    Instruction::HostCall {
                        name, return_type, ..
                    } if name == "string_byte" => {
                        string_byte_calls += 1;
                        assert_eq!(
                            return_type,
                            &Type::Numeric(NumericType::I32),
                            "string_byte host ABI must remain i32"
                        );
                    }
                    Instruction::Null {
                        ty: Type::Nullable(inner),
                    } if **inner == Type::Numeric(NumericType::I32) => nullable_nulls += 1,
                    Instruction::Cast {
                        from: Type::Numeric(NumericType::I32),
                        to: Type::Nullable(inner),
                        ..
                    } if **inner == Type::Numeric(NumericType::I32) => boxes += 1,
                    Instruction::Cast {
                        from: Type::Nullable(inner),
                        to: Type::Numeric(NumericType::I32),
                        ..
                    } if **inner == Type::Numeric(NumericType::I32) => unboxes += 1,
                    _ => {}
                }
            }
        }
    }
    assert_eq!(string_byte_calls, 3);
    assert_eq!(nullable_nulls, 3);
    assert_eq!(boxes, 3);
    assert_eq!(
        unboxes, 2,
        "numeric comparison and annotated numeric use should both unbox"
    );
}

#[test]
fn clips_string_byte_ranges_and_adjusts_empty_ranges_to_nil() {
    let source = r#"
        declare function string_byte(value: string, index: i32): i32
        declare function string_char2(first: i32, second: i32): string

        function empty_high(): bool
            return string.byte("hi", 9, 10) == nil
        end

        function empty_inverted(): bool
            return string.byte("hi", 2, 1) ~= nil
        end

        function clipped(): string
            return string.char(string.byte("hi", 1, 5))
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");

    for function in &module.functions {
        let mut constants: BTreeMap<ValueId, i32> = BTreeMap::new();
        let mut byte_indices = Vec::new();
        let mut bool_consts = Vec::new();
        for block in function.blocks.values() {
            for (value, instruction) in &block.instructions {
                match instruction {
                    Instruction::Number {
                        ty: NumericType::I32,
                        literal,
                    } => {
                        if let Ok(parsed) = literal.raw.parse::<i32>() {
                            constants.insert(*value, parsed);
                        }
                    }
                    Instruction::HostCall { name, args, .. } if name == "string_byte" => {
                        byte_indices.push(constants.get(&args[1]).copied());
                    }
                    Instruction::Bool(flag) => bool_consts.push(*flag),
                    _ => {}
                }
            }
        }
        match function.name.as_str() {
            "empty_high" => {
                assert!(
                    byte_indices.is_empty(),
                    "statically empty range must emit no host calls:\n{}",
                    function.dump()
                );
                assert!(
                    bool_consts.contains(&true),
                    "empty range == nil should be statically true:\n{}",
                    function.dump()
                );
            }
            "empty_inverted" => {
                assert!(
                    byte_indices.is_empty(),
                    "statically empty range must emit no host calls:\n{}",
                    function.dump()
                );
                assert!(
                    bool_consts.contains(&false),
                    "empty range ~= nil should be statically false:\n{}",
                    function.dump()
                );
            }
            "clipped" => {
                assert_eq!(
                    byte_indices,
                    vec![Some(1), Some(2)],
                    "range must clip to the string bounds:\n{}",
                    function.dump()
                );
            }
            _ => {}
        }
    }
}

#[test]
fn lowers_scalar_numeric_equality_with_single_value_string_byte_range() {
    let source = r#"
        declare function string_byte(value: string, index: i32): i32

        function clipped_negative_end(): bool
            return string.byte("\n\n", 2, -1) == 10
        end

        function exact_range(): bool
            return string.byte("\n\n", 2, 2) == 10
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    build(&typed).expect("ir build should succeed");
}

#[test]
fn lowers_expected_arity_string_byte_range_with_dynamic_bounds() {
    let source = r#"
        declare function string_byte(value: string, index: i32): i32

        function byte_pair_sum(value: string, first: i32, last: i32): i32
            local a: i32, b: i32 = string.byte(value, first, last)
            return a + b
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("IR build should succeed");
    verify(&module).expect("IR should verify");

    let function = module
        .functions
        .iter()
        .find(|function| function.name == "byte_pair_sum")
        .expect("byte_pair_sum should lower");
    let byte_calls = function
        .blocks
        .values()
        .flat_map(|block| &block.instructions)
        .filter(|(_, instruction)| {
            matches!(instruction, Instruction::HostCall { name, .. } if name == "string_byte")
        })
        .count();
    assert_eq!(byte_calls, 2);
}

#[test]
fn lowers_if_expression_with_phi_result() {
    let source = r#"
        function entry(flag: bool, x: i32, y: i32): i32
            return if flag then x + 1 else y + 2
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = &module.functions[0];
    let has_branch_phi = function.blocks.values().any(|block| {
        block
            .instructions
            .iter()
            .any(|(_, instruction)| matches!(instruction, Instruction::Phi(incoming) if incoming.len() == 2))
    });
    assert!(
        has_branch_phi,
        "expected branch phi in function:\n{}",
        function.dump()
    );
}

#[test]
fn inserts_phi_for_loop_carried_variable() {
    let source = r#"
        function entry(limit: i32): i32
            local i: i32 = 0
            while i < limit do
                i = i + 1
            end
            return i
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = &module.functions[0];
    let loop_phi = function.blocks.values().find_map(|block| {
        block.instructions.iter().find_map(|(_, instruction)| {
            if let Instruction::Phi(incoming) = instruction {
                Some(incoming.len())
            } else {
                None
            }
        })
    });
    assert_eq!(
        loop_phi,
        Some(2),
        "expected loop phi with two incoming edges"
    );
}

#[test]
fn lowers_repeat_until_with_post_test_condition() {
    let source = r#"
        function entry(limit: i32): i32
            local i: i32 = 0
            repeat
                i = i + 1
            until i > limit
            return i
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = &module.functions[0];
    let repeat_branch = function.blocks.values().find(|block| {
        matches!(
            block.terminator,
            Terminator::Branch {
                then_block,
                else_block,
                ..
            } if then_block != else_block
        )
    });
    assert!(
        repeat_branch.is_some(),
        "expected repeat-until branch terminator"
    );
    let loop_phi = function.blocks.values().find_map(|block| {
        block.instructions.iter().find_map(|(_, instruction)| {
            if let Instruction::Phi(incoming) = instruction {
                Some(incoming.len())
            } else {
                None
            }
        })
    });
    assert_eq!(
        loop_phi,
        Some(2),
        "expected repeat-until phi with two incoming edges"
    );
}

// Counts how many blocks jump (or branch) into `target`.
fn predecessor_count(function: &Function, target: BlockId) -> usize {
    function
        .blocks
        .values()
        .filter(|block| match &block.terminator {
            Terminator::Jump(b) => *b == target,
            Terminator::Branch {
                then_block,
                else_block,
                ..
            } => *then_block == target || *else_block == target,
            _ => false,
        })
        .count()
}

// Returns the loop header: the block whose phis carry the most incoming edges.
fn loop_header(function: &Function) -> &BasicBlock {
    function
        .blocks
        .values()
        .filter(|block| {
            block
                .instructions
                .iter()
                .any(|(_, instruction)| matches!(instruction, Instruction::Phi(_)))
        })
        .max_by_key(|block| predecessor_count(function, block.id))
        .expect("function should contain a phi block")
}

// `continue` inside a numeric for-loop jumps straight back to the loop header,
// which carries implicit phis for the loop index and stop bound on top of any
// user phis. Each `continue` site must contribute an incoming edge to all of
// those phis (advancing the index), otherwise the header phis are malformed and
// the module fails to verify. Regression test for the continue phi bug surfaced
// by conformance/luau/basic.6.walu.
#[test]
fn numeric_for_continue_completes_loop_header_phis() {
    let source = r#"
        function entry(): i32
            local a: i32 = 1
            for i = 1, 8 do
                a = a + 1
                if a < 5 then
                    continue
                end
                a = a * 2
            end
            return a
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = &module.functions[0];
    if let Err(err) = verify(&module) {
        panic!("verify failed: {err}\n{}", function.dump());
    }

    let header = loop_header(function);
    let preds = predecessor_count(function, header.id);
    assert_eq!(
        preds,
        3,
        "expected loop header to have a preheader, continue, and fall-through \
         predecessor, got {preds} in:\n{}",
        function.dump()
    );
    for (value, instruction) in &header.instructions {
        if let Instruction::Phi(incoming) = instruction {
            assert_eq!(
                incoming.len(),
                preds,
                "phi {value:?} is missing the continue edge in:\n{}",
                function.dump()
            );
        }
    }
}

// Same defect for an array `for-in` loop, whose header carries implicit phis for
// the array index and length.
#[test]
fn array_for_in_continue_completes_loop_header_phis() {
    let source = r#"
        function entry(xs: {i32}): i32
            local sum: i32 = 0
            for i, x in xs do
                if x < 0 then
                    continue
                end
                sum = sum + x
            end
            return sum
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = &module.functions[0];
    if let Err(err) = verify(&module) {
        panic!("verify failed: {err}\n{}", function.dump());
    }

    let header = loop_header(function);
    let preds = predecessor_count(function, header.id);
    assert_eq!(
        preds,
        3,
        "expected loop header to have a preheader, continue, and fall-through \
         predecessor, got {preds} in:\n{}",
        function.dump()
    );
    for (value, instruction) in &header.instructions {
        if let Instruction::Phi(incoming) = instruction {
            assert_eq!(
                incoming.len(),
                preds,
                "phi {value:?} is missing the continue edge in:\n{}",
                function.dump()
            );
        }
    }
}

#[test]
fn emits_branches_and_returns() {
    let source = r#"
        function entry(flag: bool, x: i32): i32
            if flag then
                return x
            end
            return x + 1
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = &module.functions[0];
    let branch_count = function
        .blocks
        .values()
        .filter(|block| matches!(block.terminator, Terminator::Branch { .. }))
        .count();
    let return_count = function
        .blocks
        .values()
        .filter(|block| matches!(block.terminator, Terminator::Return(_)))
        .count();
    assert_eq!(branch_count, 1);
    assert_eq!(return_count, 2);
}

#[test]
fn method_calls_reach_ir_call_checking() {
    let source = r#"
        function ping(self: { x: f64, y: f64 }): i32
            return 1
        end

        function entry(): i32
            local obj = { x = 1 }
            obj.ping = ping
            return obj:ping()
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = build(&program).expect_err("ir build should fail");
    assert_eq!(
        error.to_string(),
        "call expected {x: f64, y: f64}, got {ping: ({x: f64, y: f64}) -> i32, x: f64}"
    );
}

#[test]
fn lowers_method_call_via_method_declaration() {
    let source = r#"
        local point = { x = 41::i32 }

        function point:get_x(): i32
            return self.x
        end

        assert(point:get_x() == 41)
    "#;

    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    let init = module
        .functions
        .iter()
        .find(|function| function.name == "__waluau_top_level_init")
        .expect("top-level init should exist");
    let direct_targets = init
        .blocks
        .values()
        .flat_map(|block| block.instructions.iter())
        .filter_map(|(_, instruction)| match instruction {
            Instruction::Call { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        init.blocks.values().any(|block| {
            block
                .instructions
                .iter()
                .any(|(_, instruction)| matches!(instruction, Instruction::Call { .. }))
        }),
        "expected direct method call in init:\n{}",
        init.dump()
    );
    assert!(
        !init.blocks.values().any(|block| {
            block
                .instructions
                .iter()
                .any(|(_, instruction)| matches!(instruction, Instruction::CallValue { .. }))
        }),
        "unexpected indirect call in init:\n{}",
        init.dump()
    );
    assert!(
        direct_targets
            .iter()
            .any(|name| name.starts_with("__waluau_top_level_init$lambda")),
        "expected direct call to lifted method closure, got {direct_targets:?}\n{}",
        init.dump()
    );
}

#[test]
fn widened_method_receiver_writes_back_mutations() {
    let source = r#"
        local point = { x = 41::i32 }

        function point:bump(delta: i32): i32
            self.x = self.x + delta
            return self.x
        end

        assert(point:bump(1::i32) == 42::i32)
        assert(point.x == 42::i32)
    "#;

    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    let function = module
        .functions
        .iter()
        .find(|function| function.name == "__waluau_top_level_init")
        .expect("expected synthesized top-level init");

    let writeback_after_call = function.blocks.values().any(|block| {
        let instructions = &block.instructions;
        instructions.windows(3).any(|window| {
            matches!(
                window[0].1,
                Instruction::Call { .. } | Instruction::CallValue { .. }
            ) && matches!(&window[1].1, Instruction::StructGet { field, .. } if field == "x")
                && matches!(&window[2].1, Instruction::StructSet { field, .. } if field == "x")
        })
    });
    assert!(
        writeback_after_call,
        "expected method call lowering to write back receiver mutations:\n{}",
        function.dump()
    );
}

#[test]
fn canonicalizes_snake_case_dom_import_members_without_interface_allowlist() {
    let source = r#"
        type Node = extern
        type Selection = extern

        declare property Selection:anchor_node: Node?
        declare property Selection:focus_node: Node?
        declare function Selection:select_all_children(node: Node): unit
    "#;

    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let host_names = module
        .declared_imports
        .iter()
        .map(|declared| declared.host_name.as_str())
        .collect::<Vec<_>>();

    assert!(host_names.contains(&"Selection.get/anchorNode"));
    assert!(host_names.contains(&"Selection.get/focusNode"));
    assert!(host_names.contains(&"Selection.selectAllChildren"));
}

#[test]
fn erases_generic_extern_specializations_in_declared_import_signatures() {
    let source = r#"
        type Response = extern
        type Promise<T> = extern

        declare function take_response(value: Promise<Response>): Promise<Response>
        declare function take_string(value: Promise<string>): Promise<string>
        declare function take_i32(value: Promise<i32>): Promise<i32>
    "#;

    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");

    let take_response = module
        .declared_imports
        .iter()
        .find(|declared| declared.name == "take_response")
        .expect("take_response import should exist");
    assert_eq!(take_response.params, vec![Type::Extern]);
    assert_eq!(take_response.return_type, Type::Extern);

    let take_string = module
        .declared_imports
        .iter()
        .find(|declared| declared.name == "take_string")
        .expect("take_string import should exist");
    assert_eq!(take_string.params, vec![Type::Extern]);
    assert_eq!(take_string.return_type, Type::Extern);

    let take_i32 = module
        .declared_imports
        .iter()
        .find(|declared| declared.name == "take_i32")
        .expect("take_i32 import should exist");
    assert_eq!(take_i32.params, vec![Type::Extern]);
    assert_eq!(take_i32.return_type, Type::Extern);
}

#[test]
fn erases_tfjs_model_promise_import_signatures() {
    let source = r#"
        type Promise<T> = extern
        type Tensor = extern
        type GraphModel = extern
        type LayersModel = extern
        type TrainingHistory = extern

        declare function load_graph_model(url: string): Promise<GraphModel>
        declare function load_layers_model(url: string): Promise<LayersModel>
        declare function graph_model_predict(model: GraphModel, input: Tensor): Tensor
        declare function layers_model_predict(model: LayersModel, input: Tensor): Tensor
        declare function layers_model_compile_sgd(model: LayersModel, loss: string, learning_rate: f64): unit
        declare function layers_model_fit_one(model: LayersModel, x: Tensor, y: Tensor, epochs: i32, batch_size: i32): Promise<TrainingHistory>
        declare function training_history_len(history: TrainingHistory): i32
        declare function training_history_loss(history: TrainingHistory, index: i32): f64

        function load_graph(url: string): Promise<GraphModel>
            return load_graph_model(url)
        end

        function predict_layers(model: LayersModel, input: Tensor): Tensor
            return layers_model_predict(model, input)
        end

        function train_layers(model: LayersModel, input: Tensor, target: Tensor): Promise<TrainingHistory>
            layers_model_compile_sgd(model, "meanSquaredError", 0.1)
            return layers_model_fit_one(model, input, target, 3, 1)
        end

        function loss_at(history: TrainingHistory, index: i32): f64
            if training_history_len(history) == 0 then
                return 0.0
            end
            return training_history_loss(history, index)
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");

    let load_graph_model = module
        .declared_imports
        .iter()
        .find(|declared| declared.name == "load_graph_model")
        .expect("load_graph_model import should exist");
    assert_eq!(load_graph_model.params, vec![Type::String]);
    assert_eq!(load_graph_model.return_type, Type::Extern);

    let graph_model_predict = module
        .declared_imports
        .iter()
        .find(|declared| declared.name == "graph_model_predict")
        .expect("graph_model_predict import should exist");
    assert_eq!(graph_model_predict.params, vec![Type::Extern, Type::Extern]);
    assert_eq!(graph_model_predict.return_type, Type::Extern);

    let layers_model_compile_sgd = module
        .declared_imports
        .iter()
        .find(|declared| declared.name == "layers_model_compile_sgd")
        .expect("layers_model_compile_sgd import should exist");
    assert_eq!(
        layers_model_compile_sgd.params,
        vec![Type::Extern, Type::String, Type::Numeric(NumericType::F64),]
    );
    assert_eq!(layers_model_compile_sgd.return_type, Type::Unit);

    let layers_model_fit_one = module
        .declared_imports
        .iter()
        .find(|declared| declared.name == "layers_model_fit_one")
        .expect("layers_model_fit_one import should exist");
    assert_eq!(
        layers_model_fit_one.params,
        vec![
            Type::Extern,
            Type::Extern,
            Type::Extern,
            Type::Numeric(NumericType::I32),
            Type::Numeric(NumericType::I32),
        ]
    );
    assert_eq!(layers_model_fit_one.return_type, Type::Extern);

    let training_history_loss = module
        .declared_imports
        .iter()
        .find(|declared| declared.name == "training_history_loss")
        .expect("training_history_loss import should exist");
    assert_eq!(
        training_history_loss.params,
        vec![Type::Extern, Type::Numeric(NumericType::I32)]
    );
    assert_eq!(
        training_history_loss.return_type,
        Type::Numeric(NumericType::F64)
    );
}

#[test]
fn erases_promise_api_import_signatures_and_lowers_response_text_host_calls() {
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

    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");

    let fetch = module
        .declared_imports
        .iter()
        .find(|declared| declared.name == "fetch")
        .expect("fetch import should exist");
    assert_eq!(fetch.params, vec![Type::String]);
    assert_eq!(fetch.return_type, Type::Extern);

    let text = module
        .declared_imports
        .iter()
        .find(|declared| declared.name == "Response.text")
        .expect("Response.text import should exist");
    assert_eq!(text.params, vec![Type::Extern]);
    assert_eq!(text.return_type, Type::Extern);

    let host_calls = module
        .functions
        .iter()
        .flat_map(|function| function.blocks.values())
        .flat_map(|block| block.instructions.iter())
        .filter_map(|(_, instruction)| match instruction {
            Instruction::HostCall { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        host_calls.contains(&"fetch"),
        "expected fetch host call, got {host_calls:?}"
    );
    assert!(
        host_calls.contains(&"Response.text"),
        "expected Response.text host call, got {host_calls:?}"
    );
}

#[test]
fn threads_assert_call_span_to_trap_terminator() {
    let source = r#"
        function entry(): i32
            assert(false)
            return 1
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = &module.functions[0];
    let trap_span = function
        .blocks
        .values()
        .find_map(|block| match block.terminator {
            Terminator::Unreachable { span } => span,
            _ => None,
        });
    let span = trap_span.expect("assert trap should carry source span");
    assert!(span.end > span.start);
}

#[test]
fn records_numeric_scalar_kinds_in_instructions() {
    let source = r#"
        function entry(x: i64, y: u64, z: f64): f64
            local a: i64 = x + 1
            local b: u64 = y + 2
            local c: f64 = z + 3
            return c
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = &module.functions[0];
    assert!(function.blocks.values().any(|block| {
        block.instructions.iter().any(|(_, instruction)| {
            matches!(
                instruction,
                Instruction::Number {
                    ty: NumericType::I64,
                    literal,
                } if literal.raw == "1"
            )
        })
    }));
    assert!(function.blocks.values().any(|block| {
        block.instructions.iter().any(|(_, instruction)| {
            matches!(
                instruction,
                Instruction::Number {
                    ty: NumericType::U64,
                    literal,
                } if literal.raw == "2"
            )
        })
    }));
    assert!(function.blocks.values().any(|block| {
        block.instructions.iter().any(|(_, instruction)| {
            matches!(
                instruction,
                Instruction::Number {
                    ty: NumericType::I64,
                    ..
                }
            )
        })
    }));
    assert!(function.blocks.values().any(|block| {
        block.instructions.iter().any(|(_, instruction)| {
            matches!(
                instruction,
                Instruction::Binary {
                    operand_ty: Type::Numeric(NumericType::F64),
                    result_ty: Type::Numeric(NumericType::F64),
                    ..
                }
            )
        })
    }));
}

#[test]
fn preserves_full_range_integer_literals_in_ir() {
    let source = r#"
        function entry(): u64
            return 18446744073709551615
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = &module.functions[0];
    assert!(function.blocks.values().any(|block| {
        block.instructions.iter().any(|(_, instruction)| {
            matches!(
                instruction,
                Instruction::Number {
                    ty: NumericType::U64,
                    literal,
                } if literal.raw == "18446744073709551615"
            )
        })
    }));
}

#[test]
fn lowers_compound_index_assignment_with_single_target_evaluation() {
    let source = r#"
        function idx(): i32
            return 0
        end

        function entry(xs: {i32}): i32
            xs[idx()] += 5
            return xs[0]
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = module
        .functions
        .iter()
        .find(|function| function.name == "entry")
        .expect("entry function should exist");
    let call_count = function
        .blocks
        .values()
        .flat_map(|block| {
            block
                .instructions
                .iter()
                .map(|(_, instruction)| instruction)
        })
        .filter(|instruction| {
            matches!(
                instruction,
                Instruction::Call { name, .. } if name == "idx"
            )
        })
        .count();
    assert_eq!(call_count, 1, "expected idx() call to be evaluated once");
}

#[test]
fn lowers_and_expression_with_short_circuit_cfg() {
    let source = r#"
        function entry(a: bool, b: bool): bool
            return a and b
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = &module.functions[0];

    assert!(
        !function.blocks.values().any(|block| {
            block.instructions.iter().any(|(_, instruction)| {
                matches!(
                    instruction,
                    Instruction::Binary {
                        op: BinaryOp::And,
                        ..
                    }
                )
            })
        }),
        "expected 'and' to lower to control-flow, not a binary instruction:\n{}",
        function.dump()
    );

    let branch_count = function
        .blocks
        .values()
        .filter(|block| matches!(block.terminator, Terminator::Branch { .. }))
        .count();
    assert!(
        branch_count >= 1,
        "expected at least one branch for short-circuit 'and', got {} in function:\n{}",
        branch_count,
        function.dump()
    );

    let phi_count = function
        .blocks
        .values()
        .flat_map(|block| block.instructions.iter())
        .filter(|(_, instruction)| matches!(instruction, Instruction::Phi(incoming) if incoming.len() == 2))
        .count();
    assert!(
        phi_count >= 1,
        "expected a phi node merging 'and' results, got {} in function:\n{}",
        phi_count,
        function.dump()
    );
}

#[test]
fn lowers_or_expression_with_short_circuit_cfg() {
    let source = r#"
        function entry(a: bool, b: bool): bool
            return a or b
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = &module.functions[0];

    assert!(
        !function.blocks.values().any(|block| {
            block.instructions.iter().any(|(_, instruction)| {
                matches!(
                    instruction,
                    Instruction::Binary {
                        op: BinaryOp::Or,
                        ..
                    }
                )
            })
        }),
        "expected 'or' to lower to control-flow, not a binary instruction:\n{}",
        function.dump()
    );

    let branch_count = function
        .blocks
        .values()
        .filter(|block| matches!(block.terminator, Terminator::Branch { .. }))
        .count();
    assert!(
        branch_count >= 1,
        "expected at least one branch for short-circuit 'or', got {} in function:\n{}",
        branch_count,
        function.dump()
    );

    let phi_count = function
        .blocks
        .values()
        .flat_map(|block| block.instructions.iter())
        .filter(|(_, instruction)| matches!(instruction, Instruction::Phi(incoming) if incoming.len() == 2))
        .count();
    assert!(
        phi_count >= 1,
        "expected a phi node merging 'or' results, got {} in function:\n{}",
        phi_count,
        function.dump()
    );
}

#[test]
fn inserts_casts_for_implicit_and_explicit_conversions() {
    let source = r#"
        function entry(x: i32, y: i64): i32
            local widened: i64 = x
            local sum: i64 = widened + y
            return sum :: i32
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = &module.functions[0];
    let casts = function
        .blocks
        .values()
        .flat_map(|block| {
            block
                .instructions
                .iter()
                .map(|(_, instruction)| instruction)
        })
        .filter(|instruction| matches!(instruction, Instruction::Cast { .. }))
        .count();
    assert_eq!(casts, 2, "expected implicit widen and explicit narrow cast");
}

#[test]
fn lowers_cast_style_initialization_of_named_record_types() {
    let source = r#"
        type MyType = { pos: number }

        function entry(): number
            local t = { pos = 20 }::MyType
            return t.pos
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    let function = &module.functions[0];
    assert!(function.blocks.values().any(|block| {
        block
            .instructions
            .iter()
            .any(|(_, instruction)| matches!(instruction, Instruction::StructNew { .. }))
    }));
}

#[test]
fn rejects_non_bool_branch_condition() {
    let function = Function {
        name: "entry".into(),
        params: vec![],
        return_type: Type::Numeric(NumericType::I64),
        entry: BlockId(0),
        next_value: 2,
        capture_count: 0,
        value_symbols: BTreeMap::new(),
        symbol_id: None,
        source_map: FunctionSourceMap::synthetic(),
        blocks: BTreeMap::from([
            (
                BlockId(0),
                BasicBlock {
                    id: BlockId(0),
                    instructions: vec![(
                        ValueId(0),
                        Instruction::Number {
                            ty: NumericType::I64,
                            literal: NumberLiteral { raw: "1".into() },
                        },
                    )],
                    terminator: Terminator::Branch {
                        condition: ValueId(0),
                        then_block: BlockId(1),
                        else_block: BlockId(1),
                    },
                },
            ),
            (
                BlockId(1),
                BasicBlock {
                    id: BlockId(1),
                    instructions: vec![(
                        ValueId(1),
                        Instruction::Number {
                            ty: NumericType::I64,
                            literal: NumberLiteral { raw: "0".into() },
                        },
                    )],
                    terminator: Terminator::Return(ValueId(1)),
                },
            ),
        ]),
    };
    let err = verify(&Module {
        globals: Vec::new(),
        functions: vec![function],
        tooling_function_exports: std::collections::BTreeMap::new(),
        authored_function_exports: std::collections::BTreeMap::new(),
        declared_imports: Vec::new(),
        start: None,
        tag_ids: std::collections::BTreeMap::new(),
        symbol_names: std::collections::BTreeMap::new(),
        source_files: Vec::new(),
    })
    .expect_err("expected verifier to reject non-bool branch");
    assert!(err.to_string().contains("branch condition"));
}

#[test]
fn rejects_return_type_mismatch() {
    let function = Function {
        name: "entry".into(),
        params: vec![],
        return_type: Type::Bool,
        entry: BlockId(0),
        next_value: 1,
        capture_count: 0,
        value_symbols: BTreeMap::new(),
        symbol_id: None,
        source_map: FunctionSourceMap::synthetic(),
        blocks: BTreeMap::from([(
            BlockId(0),
            BasicBlock {
                id: BlockId(0),
                instructions: vec![(
                    ValueId(0),
                    Instruction::Number {
                        ty: NumericType::I64,
                        literal: NumberLiteral { raw: "1".into() },
                    },
                )],
                terminator: Terminator::Return(ValueId(0)),
            },
        )]),
    };
    let err = verify(&Module {
        globals: Vec::new(),
        functions: vec![function],
        tooling_function_exports: std::collections::BTreeMap::new(),
        authored_function_exports: std::collections::BTreeMap::new(),
        declared_imports: Vec::new(),
        start: None,
        tag_ids: std::collections::BTreeMap::new(),
        symbol_names: std::collections::BTreeMap::new(),
        source_files: Vec::new(),
    })
    .expect_err("expected verifier to reject return type mismatch");
    assert!(err.to_string().contains("return in block"));
}

#[test]
fn lowers_array_literals_indexing_length_and_mutation() {
    let source = r#"
        function score_count(): i32
            local scores: {number} = {100, 250, 300}
            local first: number = scores[1]
            scores[2] = first + 1
            return #scores
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = &module.functions[0];
    assert!(function.blocks.values().any(|block| {
        block
            .instructions
            .iter()
            .any(|(_, instruction)| matches!(instruction, Instruction::ArrayNew { .. }))
    }));

    let instruction = |needle: ValueId| {
        function
            .blocks
            .values()
            .flat_map(|block| block.instructions.iter())
            .find_map(|(value, instruction)| (*value == needle).then_some(instruction))
            .expect("instruction value should exist")
    };
    let authored_indices = function
        .blocks
        .values()
        .flat_map(|block| block.instructions.iter())
        .filter_map(|(_, instruction)| match instruction {
            Instruction::ArrayGet { index, .. } | Instruction::ArraySet { index, .. } => {
                Some(*index)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(authored_indices.len(), 2);
    for index in authored_indices {
        let Instruction::Binary {
            op: BinaryOp::Sub,
            right,
            ..
        } = instruction(index)
        else {
            panic!("authored array access should subtract the source index origin");
        };
        assert!(matches!(
            instruction(*right),
            Instruction::Number { literal, .. } if literal.raw == "1"
        ));
    }
    assert!(function.blocks.values().any(|block| {
        block
            .instructions
            .iter()
            .any(|(_, instruction)| matches!(instruction, Instruction::ArrayGet { .. }))
    }));
    assert!(function.blocks.values().any(|block| {
        block
            .instructions
            .iter()
            .any(|(_, instruction)| matches!(instruction, Instruction::ArraySet { .. }))
    }));
    assert!(function.blocks.values().any(|block| {
        block
            .instructions
            .iter()
            .any(|(_, instruction)| matches!(instruction, Instruction::ArrayLen { .. }))
    }));
}

#[test]
fn rejects_phi_predecessor_order_mismatch() {
    let function = Function {
        name: "entry".into(),
        params: vec![],
        return_type: Type::Numeric(NumericType::I64),
        entry: BlockId(0),
        next_value: 5,
        capture_count: 0,
        value_symbols: BTreeMap::new(),
        symbol_id: None,
        source_map: FunctionSourceMap::synthetic(),
        blocks: BTreeMap::from([
            (
                BlockId(0),
                BasicBlock {
                    id: BlockId(0),
                    instructions: vec![(ValueId(0), Instruction::Bool(true))],
                    terminator: Terminator::Branch {
                        condition: ValueId(0),
                        then_block: BlockId(1),
                        else_block: BlockId(2),
                    },
                },
            ),
            (
                BlockId(1),
                BasicBlock {
                    id: BlockId(1),
                    instructions: vec![(
                        ValueId(1),
                        Instruction::Number {
                            ty: NumericType::I64,
                            literal: NumberLiteral { raw: "1".into() },
                        },
                    )],
                    terminator: Terminator::Jump(BlockId(3)),
                },
            ),
            (
                BlockId(2),
                BasicBlock {
                    id: BlockId(2),
                    instructions: vec![(
                        ValueId(2),
                        Instruction::Number {
                            ty: NumericType::I64,
                            literal: NumberLiteral { raw: "2".into() },
                        },
                    )],
                    terminator: Terminator::Jump(BlockId(3)),
                },
            ),
            (
                BlockId(3),
                BasicBlock {
                    id: BlockId(3),
                    instructions: vec![(
                        ValueId(3),
                        Instruction::Phi(vec![(BlockId(2), ValueId(2)), (BlockId(1), ValueId(1))]),
                    )],
                    terminator: Terminator::Return(ValueId(3)),
                },
            ),
        ]),
    };
    let err = verify(&Module {
        globals: Vec::new(),
        functions: vec![function],
        tooling_function_exports: std::collections::BTreeMap::new(),
        authored_function_exports: std::collections::BTreeMap::new(),
        declared_imports: Vec::new(),
        start: None,
        tag_ids: std::collections::BTreeMap::new(),
        symbol_names: std::collections::BTreeMap::new(),
        source_files: Vec::new(),
    })
    .expect_err("expected verifier to reject phi predecessor ordering");
    assert!(err.to_string().contains("predecessor order mismatch"));
}

#[test]
fn lowers_function_expression_with_capture_and_indirect_call() {
    let source = r#"
        function entry(x: i32): i32
            local addx: (i32) -> i32 = function(y: i32): i32
                return x + y
            end
            return addx(7)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    assert!(
        module
            .functions
            .iter()
            .any(|function| function.name == "entry$lambda0"),
        "expected lifted lambda function in module"
    );
    let entry = module
        .functions
        .iter()
        .find(|function| function.name == "entry")
        .expect("entry function should exist");
    assert!(entry.blocks.values().any(|block| {
        block
            .instructions
            .iter()
            .any(|(_, instruction)| matches!(instruction, Instruction::Closure { .. }))
    }));
    assert!(entry.blocks.values().any(|block| {
        block
            .instructions
            .iter()
            .any(|(_, instruction)| matches!(instruction, Instruction::CallValue { .. }))
    }));
}

#[test]
fn lowers_lifted_unit_function_expression_with_implicit_return() {
    let source = r#"
        type Event = extern

        declare function report_event_count(value: i32): unit

        function entry(seed: i32): unit
            local count: i32 = seed
            local handler: (Event) -> unit = function(event: Event): unit
                count = count + 1
                report_event_count(count)
            end
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    let lambda = module
        .functions
        .iter()
        .find(|function| function.name == "entry$lambda0")
        .expect("expected lifted lambda function in module");
    let entry = lambda
        .blocks
        .get(&lambda.entry)
        .expect("lambda entry block should exist");

    assert!(
        entry.instructions.iter().any(|(_, instruction)| {
            matches!(
                instruction,
                Instruction::HostCall {
                    return_type: Type::Unit,
                    ..
                }
            )
        }),
        "expected lifted lambda to end with unit host-call statement:\n{}",
        lambda.dump()
    );
    assert!(
        matches!(entry.terminator, Terminator::Return(_)),
        "expected lifted unit lambda to return normally:\n{}",
        lambda.dump()
    );
}

#[test]
fn lowers_bare_return_in_unit_function() {
    let source = r#"
        function entry(x: i32): unit
            if x > 0 then
                return
            end
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    verify(&module).expect("ir should verify");
    let function = module
        .functions
        .iter()
        .find(|function| function.name == "entry")
        .expect("entry function should exist");
    assert!(
        function.blocks.values().any(|block| {
            matches!(block.terminator, Terminator::Return(value)
            if block.instructions.iter().any(|(id, instruction)| {
                *id == value && matches!(instruction, Instruction::Unit)
            }))
        }),
        "expected explicit bare return to lower to a unit return:\n{}",
        function.dump()
    );
}

#[test]
fn lowers_duplicate_declared_host_members_across_extern_types() {
    let source = r#"
        type Alpha = extern
        type Beta = extern

        declare function get_alpha(): Alpha
        declare function get_beta(): Beta
        declare property Alpha:size: u32
        declare property Beta:size: u32
        declare function Alpha:value(delta: i32): i32
        declare function Beta:value(delta: i32): i32

        function read_alpha(x: Alpha): u32
            return x.size
        end

        function read_beta(x: Beta): u32
            return x.size
        end

        function write_alpha(x: Alpha): unit
            x.size = 1::u32
        end

        function write_beta(x: Beta): unit
            x.size = 2::u32
        end

        function call_alpha(x: Alpha): i32
            return x:value(1::i32)
        end

        function call_beta(x: Beta): i32
            return x:value(2::i32)
        end

        function entry(): i32
            local alpha = get_alpha()
            local beta = get_beta()
            write_alpha(alpha)
            write_beta(beta)
            return call_alpha(alpha) + call_beta(beta) + read_alpha(alpha)::i32 + read_beta(beta)::i32
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    verify(&module).expect("ir should verify");

    let host_calls = module
        .functions
        .iter()
        .flat_map(|function| function.blocks.values())
        .flat_map(|block| block.instructions.iter())
        .filter_map(|(_, instruction)| match instruction {
            Instruction::HostCall { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert!(
        host_calls.contains(&"Alpha.get/size"),
        "expected Alpha getter host call, got {host_calls:?}"
    );
    assert!(
        host_calls.contains(&"Beta.get/size"),
        "expected Beta getter host call, got {host_calls:?}"
    );
    assert!(
        host_calls.contains(&"Alpha.set/size"),
        "expected Alpha setter host call, got {host_calls:?}"
    );
    assert!(
        host_calls.contains(&"Beta.set/size"),
        "expected Beta setter host call, got {host_calls:?}"
    );
    assert!(
        host_calls.contains(&"Alpha.value"),
        "expected Alpha method host call, got {host_calls:?}"
    );
    assert!(
        host_calls.contains(&"Beta.value"),
        "expected Beta method host call, got {host_calls:?}"
    );
}

#[test]
fn lowers_named_function_expression_recursion() {
    let source = r#"
        function entry(): i32
            local fact: (i32) -> i32 = function self(n: i32): i32
                if n == 0 then
                    return 1
                end
                return n * self(n - 1)
            end
            return fact(5)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let lifted = module
        .functions
        .iter()
        .find(|function| function.name == "entry$lambda0")
        .expect("expected lifted recursive function");
    assert!(lifted.blocks.values().any(|block| {
        block
            .instructions
            .iter()
            .any(|(_, instruction)| matches!(instruction, Instruction::CallValue { .. }))
    }));
}

#[test]
fn lowers_local_function_recursion_through_the_lexical_cell() {
    let source = r#"
        function entry(): i32
            local function recurse(depth: i32): i32
                if depth == 0 then
                    return 1
                end
                return recurse(depth - 1) + 1
            end
            local original = recurse
            recurse = function(depth: i32): i32
                return 40 + depth
            end
            return original(1)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    verify(&module).expect("ir should verify");

    let entry = module
        .functions
        .iter()
        .find(|function| function.name == "entry")
        .expect("entry function");
    assert!(entry.blocks.values().any(|block| {
        block.instructions.iter().any(|(_, instruction)| {
            matches!(instruction, Instruction::ArrayNew { elements, .. } if elements.is_empty())
        })
    }));
    let recursive = module
        .functions
        .iter()
        .find(|function| function.name == "entry$lambda0")
        .expect("lifted recursive closure");
    assert_eq!(recursive.capture_count, 1);
    assert!(recursive.blocks.values().any(|block| {
        block
            .instructions
            .iter()
            .any(|(_, instruction)| matches!(instruction, Instruction::ArrayGet { .. }))
    }));
}

#[test]
fn verifies_loop_with_break_and_continue() {
    let source = r#"
        function entry(xs: {i32}, len: i32): i32
            local i: i32 = 1
            local acc: i32 = 0
            while i <= len do
                local x: i32 = xs[i]
                if x < 0 then
                    i += 1
                    continue
                end
                acc += x
                if acc > 1000 then
                    break
                end
                i += 1
            end
            return acc
        end
    "#;
    let mut program = parse(source).expect("parse should succeed");
    waluau_ast::resolve_symbols(&mut program).expect("resolve symbols should succeed");

    let mut signatures = std::collections::HashMap::new();
    let mut field_call_signatures = std::collections::HashMap::new();
    for function in &program.functions {
        let symbol_id = function.symbol_id.expect("symbol_id resolved");
        let return_type = function.return_type.clone().unwrap();
        let sig = (
            function
                .params
                .iter()
                .map(|param| param.ty.clone())
                .collect(),
            return_type,
        );
        signatures.insert(symbol_id, sig.clone());
        field_call_signatures.insert(function.name.to_string(), sig);
    }

    let tag_ids = std::collections::BTreeMap::new();
    let host_import_signatures = std::collections::HashMap::new();
    let host_import_names = std::collections::HashMap::new();
    let declared_constants = std::collections::HashMap::new();
    let globals = std::collections::HashMap::new();
    let source_file_ids =
        std::collections::HashMap::from([(program.entry_file_path.clone(), SourceFileId(0))]);
    let cx = super::ModuleLoweringContext {
        signatures: &signatures,
        host_import_signatures: &host_import_signatures,
        host_import_names: &host_import_names,
        field_call_signatures: &field_call_signatures,
        declared_constants: &declared_constants,
        globals: &globals,
        sources: &program.sources,
        source_file_ids: &source_file_ids,
        tag_ids: &tag_ids,
    };
    let mut lowered =
        super::build_function(&program.functions[0], &cx).expect("ir lowering should succeed");
    let mut functions = Vec::new();
    functions.push(lowered.remove(0));
    functions.extend(lowered);
    let module = super::Module {
        globals: Vec::new(),
        functions,
        tooling_function_exports: std::collections::BTreeMap::new(),
        authored_function_exports: std::collections::BTreeMap::new(),
        declared_imports: Vec::new(),
        start: None,
        tag_ids: std::collections::BTreeMap::new(),
        symbol_names: std::collections::BTreeMap::new(),
        source_files: program
            .sources
            .iter()
            .map(|(path, source)| super::SourceFile {
                path: path.clone(),
                source: source.clone(),
            })
            .collect(),
    };

    let function = &module.functions[0];
    if let Err(err) = super::verify(&module) {
        panic!("verify failed: {err}\n{}", function.dump());
    }
}

#[test]
fn lowers_string_value_to_ir() {
    let source = r#"
        function entry(): string
            return "hello"
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed for string values");
    let function = &module.functions[0];
    assert_eq!(function.return_type, Type::String);
    let block = function.blocks.get(&function.entry).expect("entry block");
    let (_, instruction) = &block.instructions[0];
    assert!(
        matches!(instruction, Instruction::String(s) if s == "hello"),
        "expected String instruction with 'hello', got {:?}",
        instruction
    );
}

#[test]
fn lowers_bytes_value_index_and_length_to_ir() {
    let source = r#"
        function entry(data: bytes): i32
            local prefix: bytes = b"AB"
            local merged: bytes = prefix .. data
            return merged[0] + #merged
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed for bytes values");
    let function = &module.functions[0];
    assert!(function.blocks.values().any(|block| {
        block.instructions.iter().any(|(_, instruction)| {
            matches!(instruction, Instruction::Bytes(bytes) if bytes == &vec![65, 66])
        })
    }));
    assert!(function.blocks.values().any(|block| {
        block
            .instructions
            .iter()
            .any(|(_, instruction)| matches!(instruction, Instruction::BytesGet { .. }))
    }));
    assert!(function.blocks.values().any(|block| {
        block
            .instructions
            .iter()
            .any(|(_, instruction)| matches!(instruction, Instruction::BytesLen { .. }))
    }));
    assert!(function.blocks.values().any(|block| {
        block.instructions.iter().any(|(_, instruction)| {
            matches!(
                instruction,
                Instruction::Binary {
                    op: BinaryOp::Concat,
                    operand_ty: Type::Bytes,
                    result_ty: Type::Bytes,
                    ..
                }
            )
        })
    }));
}

#[test]
fn lowers_assertion_failure_message_before_trap() {
    let source = r#"
        function check(): i32
            assert(0 == 1)
            return 1
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = module
        .functions
        .iter()
        .find(|function| function.name == "check")
        .expect("check function should exist");
    let trap_block = function
        .blocks
        .values()
        .find(|block| matches!(block.terminator, Terminator::Unreachable { .. }))
        .expect("assert should lower a trap block");
    assert!(trap_block.instructions.iter().any(|(_, instruction)| {
        matches!(
            instruction,
            Instruction::String(message) if message == "Assertion failed: 0 == 1 at source:3"
        )
    }));
    assert!(
        trap_block
            .instructions
            .iter()
            .any(|(_, instruction)| { matches!(instruction, Instruction::Throw { .. }) })
    );
}

#[test]
fn monomorphizes_generic_calls_once_per_type_arguments() {
    let source = r#"
        function identity<T>(value: T): T
            return value
        end

        function forward<T>(value: T): T
            return identity<T>(value)
        end

        function main(): i32
            local a: i32 = forward<i32>(41)
            local b: i32 = forward<i32>(1)
            return a + b
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");

    let forward_specialization_name = module
        .functions
        .iter()
        .find(|function| function.name.starts_with("__waluau_generic$forward"))
        .map(|function| function.name.clone())
        .expect("forward specialization should exist");
    let identity_specialization_name = module
        .functions
        .iter()
        .find(|function| function.name.starts_with("__waluau_generic$identity"))
        .map(|function| function.name.clone())
        .expect("identity specialization should exist");
    assert_eq!(
        module
            .functions
            .iter()
            .filter(|function| function.name == forward_specialization_name)
            .count(),
        1
    );
    assert_eq!(
        module
            .functions
            .iter()
            .filter(|function| function.name == identity_specialization_name)
            .count(),
        1
    );

    let main = module
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function should exist");
    let forward_calls = main
        .blocks
        .values()
        .flat_map(|block| {
            block
                .instructions
                .iter()
                .map(|(_, instruction)| instruction)
        })
        .filter(|instruction| {
            matches!(
                instruction,
                Instruction::Call { name, .. } if name == &forward_specialization_name
            )
        })
        .count();
    assert_eq!(
        forward_calls, 2,
        "expected both calls to reuse one specialization"
    );
}

#[test]
fn monomorphizes_generic_calls_with_inferred_type_arguments() {
    let source = r#"
        function identity<T>(value: T): T
            return value
        end

        function main(): i32
            local a: i32 = identity(41::i32)
            local b: f64 = identity(1.0)
            return a + b :: i32
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    verify(&module).expect("ir should verify");

    let identity_i32 = module
        .functions
        .iter()
        .find(|function| {
            function.name.starts_with("__waluau_generic$identity")
                && function.return_type == Type::Numeric(NumericType::I32)
        })
        .expect("identity<i32> specialization should exist");

    let identity_f64 = module
        .functions
        .iter()
        .find(|function| {
            function.name.starts_with("__waluau_generic$identity")
                && function.return_type == Type::Numeric(NumericType::F64)
        })
        .expect("identity<f64> specialization should exist");

    assert_ne!(identity_i32.name, identity_f64.name);
}

#[test]
fn monomorphizes_top_level_generic_calls_with_nested_generic_arguments() {
    let source = r#"
        function identity<T>(value: T): T
            return value
        end

        function selected<T>(initial: T, choices: {T}): T
            return choices[0]
        end

        local typed = {1, 2, 3}::Float32Array
        local selected_value = selected("red", {identity("red"), identity("blue")})

        function main(): string
            return "red"
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("nested generic calls should monomorphize");
    verify(&module).expect("IR should verify");
}

#[test]
fn monomorphizes_generic_table_builtin_calls_in_multi_bindings() {
    let source = r#"
        function exercise<T>(values: {T}, value: T): i32
            local packed, removed, count, joined =
                table.pack(value),
                table.remove(values),
                table.getn(values),
                table.concat({"a", "b"}, ",")
            local inserted, sorted = table.insert(values, value), table.sort({2, 1})
            return count + #packed + table.getn(values)
        end

        function main(): i32
            local values: {i32} = {1, 2}
            assert(exercise<i32>(values, 3) == 4)
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("IR build should succeed");
    verify(&module).expect("IR should verify");

    assert!(
        module
            .functions
            .iter()
            .any(|function| function.name.starts_with("__waluau_generic$exercise"))
    );
}

#[test]
fn lowers_generic_method_declaration_after_hir_desugaring() {
    let source = r#"
        local point = { x = 41::i32 }

        function point:identity<T>(value: T): T
            local _x: i32 = self.x
            return value
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");

    let init = module
        .functions
        .iter()
        .find(|function| function.name == "__waluau_top_level_init")
        .expect("top-level init should exist");
    assert!(
        init.blocks.values().all(|block| {
            block.instructions.iter().all(|(_, instruction)| {
                !matches!(instruction, Instruction::Closure { name, .. } if name.contains("identity"))
            })
        }),
        "generic method declaration should not lower as an unspecialized closure value"
    );
}

#[test]
fn lowers_generic_method_call_via_colon_syntax() {
    let source = r#"
        local point = { x = 41::i32 }

        function point:identity<T>(value: T): T
            local _x: i32 = self.x
            return value
        end

        local n: i32 = point:identity<i32>(1::i32)
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    verify(&module).expect("ir should verify");

    let init = module
        .functions
        .iter()
        .find(|function| function.name == "__waluau_top_level_init")
        .expect("top-level init should exist");

    // The colon-call generic method must be monomorphized into a direct call to a
    // specialized function, mirroring the dot-call (`point.identity<i32>(...)`) path.
    // Before this was fixed it failed lowering with "unknown record field 'identity'".
    let has_specialized_call = init.blocks.values().any(|block| {
        block.instructions.iter().any(|(_, instruction)| {
            matches!(
                instruction,
                Instruction::CallValue { .. } | Instruction::Call { .. }
            )
        })
    });
    assert!(
        has_specialized_call,
        "expected colon-call generic method to lower to a call instruction:\n{}",
        init.dump()
    );
}

#[test]
fn rejects_cross_specialization_recursive_generics() {
    let source = r#"
        function loop<T>(value: T): {T}
            return loop<{T}>({value})
        end

        function main(): i32
            local xs: {i32} = loop<i32>(1)
            return xs[0]
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = build(&program).expect_err("cross-specialization recursion should fail");
    assert_eq!(error.code(), Some("generic/cross-specialization-recursion"));
}

#[test]
fn tags_ir_inference_failures_with_structured_diagnostics() {
    let source = r#"
        function entry(): i32
            return {}
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = build(&program).expect_err("ir build should fail");
    assert_eq!(error.code(), Some("inference/missing-context"));
    assert_eq!(error.category(), Some(DiagnosticCategory::MissingContext));
    assert_eq!(
        error.action(),
        Some("add an explicit element type annotation, e.g. local xs: {i32} = {}")
    );
    assert_eq!(error.span(), None);
}

#[test]
fn rejects_generic_browser_exports_without_a_concrete_wasm_signature() {
    let program =
        waluau_parser::parse("export function identity<T>(value: T): T\n    return value\nend\n")
            .expect("generic export should parse");
    let typed =
        waluau_hir::type_check_and_infer(&program).expect("generic export should type-check");
    let error = super::build(&typed).expect_err("generic browser export needs a concrete ABI");
    assert!(
        error
            .to_string()
            .contains("browser-exported function 'identity' cannot be generic")
    );
}

#[test]
fn rejects_browser_exports_in_the_compiler_owned_namespace() {
    for source in [
        "export function __waluau_main(): i32\n    return 42\nend\n",
        "local initialized: i32 = 1\nexport function __waluau_main(): i32\n    return initialized\nend\n",
        "export function memory(): bytes\n    return b\"bytes\"\nend\n",
    ] {
        let program = waluau_parser::parse(source).expect("reserved export should parse");
        let typed =
            waluau_hir::type_check_and_infer(&program).expect("reserved export should type-check");
        let error = super::build(&typed).expect_err("compiler-owned export name must be rejected");
        assert!(
            error
                .to_string()
                .contains("uses a compiler-owned export name"),
            "unexpected diagnostic: {error}"
        );
    }
}

#[test]
fn lowers_coroutine_builtins_to_typed_instructions() {
    let source = r#"
        function run(): i32
            local co: thread = coroutine.create(function(): i32
                coroutine.yield(1)
                return 2
            end)
            local ok: bool, value: unknown = coroutine.resume(co)
            local closed: bool = coroutine.close(co)
            return value::i32
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    verify(&module).expect("ir should verify");

    let mut saw_create = false;
    let mut saw_resume = false;
    let mut saw_close = false;
    let mut saw_yield = false;
    for function in &module.functions {
        for block in function.blocks.values() {
            for (_, instruction) in &block.instructions {
                match instruction {
                    Instruction::CoroutineCreate { .. } => saw_create = true,
                    Instruction::CoroutineResume { .. } => saw_resume = true,
                    Instruction::CoroutineClose { .. } => saw_close = true,
                    _ => {}
                }
            }
            if matches!(block.terminator, Terminator::CoroutineYield { .. }) {
                saw_yield = true;
            }
        }
    }
    assert!(saw_create, "expected a CoroutineCreate instruction");
    assert!(saw_resume, "expected a CoroutineResume instruction");
    assert!(saw_close, "expected a CoroutineClose instruction");
    assert!(saw_yield, "expected a CoroutineYield terminator");
}

#[test]
fn rejects_coroutine_create_for_non_i32_function() {
    let source = r#"
        function run(): i32
            local co: thread = coroutine.create(function(): f64
                return 1.0
            end)
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = build(&program).expect_err("ir build should fail");
    assert_eq!(
        error.to_string(),
        "coroutine.create expects a zero-argument i32-returning function"
    );
}

#[test]
fn lowers_array_for_in_loop() {
    let source = r#"
        function test_array_iteration_1_var(): i32
            local arr: {i32} = {10, 20, 30}
            local sum: i32 = 0
            for v in arr do
                sum = sum + v
            end
            return sum
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = &module.functions[0];
    println!("FUNCTION IR: {:#?}", function);
}

#[test]
fn lowers_table_concat_builtin_call_to_naive_concat_loop() {
    let source = r#"
        function entry(): string
            local words: {string} = {"a", "b", "c"}
            return table.concat(words, ", ", 2, 3)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = &module.functions[0];
    assert_eq!(function.return_type, Type::String);
    // No host-level "join" intrinsic exists; this lowers to a loop that reads each
    // element and accumulates the result via string concatenation.
    assert!(function.blocks.values().any(|block| {
        block
            .instructions
            .iter()
            .any(|(_, instruction)| matches!(instruction, Instruction::ArrayLen { .. }))
    }));
    assert!(function.blocks.values().any(|block| {
        block
            .instructions
            .iter()
            .any(|(_, instruction)| matches!(instruction, Instruction::ArrayGet { .. }))
    }));
    assert!(function.blocks.values().any(|block| {
        block.instructions.iter().any(|(_, instruction)| {
            matches!(
                instruction,
                Instruction::Binary {
                    op: BinaryOp::Sub,
                    ..
                }
            )
        })
    }));
    let concat_count: usize = function
        .blocks
        .values()
        .flat_map(|block| &block.instructions)
        .filter(|(_, instruction)| {
            matches!(
                instruction,
                Instruction::Binary {
                    op: BinaryOp::Concat,
                    ..
                }
            )
        })
        .count();
    assert_eq!(
        concat_count, 2,
        "expected two concatenations per loop iteration (accumulator .. separator .. element)"
    );
}

#[test]
fn rejects_table_concat_for_non_string_array() {
    let source = r#"
        function entry(): string
            local nums: {i32} = {1, 2, 3}
            return table.concat(nums, ", ")
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = build(&program).expect_err("table.concat should reject non-string arrays");
    assert!(
        error
            .to_string()
            .contains("table.concat expects an array of strings")
    );
}

#[test]
fn lowers_record_table_literal_and_field_access() {
    let source = r#"
        function entry(): f64
            local t = { x = 41 }
            return t.x + 1
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = &module.functions[0];
    assert!(function.blocks.values().any(|block| {
        block
            .instructions
            .iter()
            .any(|(_, instruction)| matches!(instruction, Instruction::StructNew { .. }))
    }));
    assert!(function.blocks.values().any(|block| {
        block.instructions.iter().any(|(_, instruction)| {
            matches!(instruction, Instruction::StructGet { field, .. } if field == "x")
        })
    }));
}

#[test]
fn lowers_record_literal_fields_in_written_order() {
    let source = r#"
        function kind_value(): i32
            return 1
        end

        function cost_value(): i32
            return 2
        end

        function entry(): i32
            local pair: { kind: i32, cost: i32 } = {
                kind = kind_value(),
                cost = cost_value(),
            }
            return pair.kind
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    let function = module
        .functions
        .iter()
        .find(|function| function.name == "entry")
        .expect("entry function should exist");

    let calls = function
        .blocks
        .values()
        .flat_map(|block| block.instructions.iter())
        .filter_map(|(value, instruction)| match instruction {
            Instruction::Call { name, .. } => Some((*value, name.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        calls.iter().map(|(_, name)| *name).collect::<Vec<_>>(),
        ["kind_value", "cost_value"],
        "record field expressions must evaluate left-to-right as written:\n{}",
        function.dump()
    );

    let struct_fields = function
        .blocks
        .values()
        .flat_map(|block| block.instructions.iter())
        .find_map(|(_, instruction)| match instruction {
            Instruction::StructNew { fields, .. } => Some(fields),
            _ => None,
        })
        .expect("record literal should lower to StructNew");
    assert_eq!(
        struct_fields,
        &[calls[1].0, calls[0].0],
        "StructNew operands must remain in canonical cost/kind storage order"
    );
}

#[test]
fn lowers_record_field_assignment() {
    let source = r#"
        function entry(): f64
            local t = { x = 1 }
            t.x = t.x + 2
            return t.x
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = &module.functions[0];
    assert!(function.blocks.values().any(|block| {
        block.instructions.iter().any(|(_, instruction)| {
            matches!(instruction, Instruction::StructSet { field, .. } if field == "x")
        })
    }));
}

#[test]
fn lowers_negative_literal_in_typed_i32_context() {
    let source = r#"
        function entry(): i32
            local value: i32 = -1
            return value
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    verify(&module).expect("ir should verify");

    let saw_i32_neg = module.functions.iter().any(|function| {
        function.blocks.values().any(|block| {
            block.instructions.iter().any(|(_, instruction)| {
                matches!(
                    instruction,
                    Instruction::Binary {
                        op: BinaryOp::Sub,
                        operand_ty: Type::Numeric(NumericType::I32),
                        result_ty: Type::Numeric(NumericType::I32),
                        ..
                    }
                )
            })
        })
    });
    assert!(saw_i32_neg, "expected i32 subtraction-based unary negation");
}

#[test]
fn lowers_tagged_union_resume_to_coroutine_resume_tagged() {
    let source = r#"
        function run(): i32
            local co: thread = coroutine.create(function(): i32
                coroutine.yield(1)
                return 2
            end)
            local result: Finished(i32) | Yielded(i32) | Error(string) = coroutine.resume(co)
            if result is Finished then
                return result.value
            else
                return 0
            end
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    verify(&module).expect("ir should verify");

    let mut saw_tagged_resume = false;
    let mut saw_is_variant = false;
    for function in &module.functions {
        for block in function.blocks.values() {
            for (_, instruction) in &block.instructions {
                match instruction {
                    Instruction::CoroutineResumeTagged { .. } => saw_tagged_resume = true,
                    Instruction::Binary {
                        op: BinaryOp::Eq, ..
                    } => saw_is_variant = true,
                    _ => {}
                }
            }
        }
    }
    assert!(
        saw_tagged_resume,
        "expected CoroutineResumeTagged instruction"
    );
    assert!(saw_is_variant, "expected binary Eq for IsVariant check");
}

#[test]
fn lowers_coroutine_resume_multi_value_to_unknown_payload() {
    let source = r#"
        function run(): i32
            local co: thread = coroutine.create(function(): i32
                coroutine.yield(1)
                return 2
            end)
            local ok: bool, value: unknown = coroutine.resume(co)
            if ok then
                return value::i32
            end
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    verify(&module).expect("ir should verify");

    let mut saw_resume = false;
    let mut saw_unbox_cast = false;
    for function in &module.functions {
        for block in function.blocks.values() {
            for (_, instruction) in &block.instructions {
                match instruction {
                    Instruction::CoroutineResume { .. } => saw_resume = true,
                    Instruction::Cast {
                        from: Type::Unknown,
                        to,
                        ..
                    } if *to == Type::Numeric(NumericType::I32) => saw_unbox_cast = true,
                    _ => {}
                }
            }
        }
    }

    assert!(saw_resume, "expected CoroutineResume instruction");
    assert!(saw_unbox_cast, "expected explicit unknown->i32 cast");
}

#[test]
fn lowers_coroutine_await_promise_to_suspend_and_resume_result() {
    let source = r#"
        declare function makePromise(): extern

        function run(): string
            local value: unknown = coroutine.await_promise(makePromise())
            return value::string
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    verify(&module).expect("ir should verify");

    let mut saw_await_result = false;
    let mut saw_await_terminator = false;
    for function in &module.functions {
        for block in function.blocks.values() {
            for (_, instruction) in &block.instructions {
                if matches!(instruction, Instruction::CoroutineAwaitResult) {
                    saw_await_result = true;
                }
            }
            if matches!(block.terminator, Terminator::CoroutineAwaitPromise { .. }) {
                saw_await_terminator = true;
            }
        }
    }

    assert!(
        saw_await_result,
        "expected CoroutineAwaitResult instruction"
    );
    assert!(
        saw_await_terminator,
        "expected CoroutineAwaitPromise terminator"
    );
}

#[test]
fn lowers_typed_promise_await_forms_to_suspend_and_resume_result() {
    let source = r#"
        type Response = extern
        type Promise<T> = extern

        declare function fetch(url: string): Promise<Response>
        declare function make_text(): Promise<string>

        function function_form(): Response
            return promise.await(fetch("/test.json"))
        end

        function method_form(): string
            return make_text():await()
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    verify(&module).expect("ir should verify");

    let mut await_result_count = 0;
    let mut await_terminator_count = 0;
    for function in &module.functions {
        for block in function.blocks.values() {
            await_result_count += block
                .instructions
                .iter()
                .filter(|(_, instruction)| matches!(instruction, Instruction::CoroutineAwaitResult))
                .count();
            if matches!(block.terminator, Terminator::CoroutineAwaitPromise { .. }) {
                await_terminator_count += 1;
            }
        }
    }

    assert_eq!(await_result_count, 2);
    assert_eq!(await_terminator_count, 2);
}

#[test]
fn lowers_tagged_union_pattern_match_binding_to_tag_check_and_unbox() {
    let source = r#"
        type Either = Left(i32) | Right(f64)

        function left(either: Either): i32
            if Left(value) = either then
                return value
            end
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    verify(&module).expect("ir should verify");

    let mut saw_tag_check = false;
    let mut saw_unbox_cast = false;
    for function in &module.functions {
        for block in function.blocks.values() {
            for (_, instruction) in &block.instructions {
                match instruction {
                    Instruction::Binary {
                        op: BinaryOp::Eq, ..
                    } => saw_tag_check = true,
                    Instruction::Cast {
                        from: Type::Unknown,
                        to,
                        ..
                    } if *to == Type::Numeric(NumericType::I32) => saw_unbox_cast = true,
                    _ => {}
                }
            }
        }
    }
    assert!(saw_tag_check, "expected binary Eq for the tag check");
    assert!(
        saw_unbox_cast,
        "expected unbox Cast from Unknown to the payload type"
    );
}

#[test]
fn rejects_tagged_union_pattern_match_for_string_payload() {
    let source = r#"
        type Either = Left(i32) | Failed(string)

        function left(either: Either): i32
            if Failed(message) = either then
                return 0
            end
            return 1
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let error = build(&typed).expect_err("ir build should fail for string payload");
    assert!(
        error.to_string().contains("string"),
        "error should mention string payload, got: {}",
        error
    );
}

#[test]
fn erases_aliases_and_literal_unions_inside_variant_payloads() {
    // A payload is stored behind the canonical record's `value` field, so it
    // has to shed nominal aliases and literal unions the way a record field
    // does; left alone, the string literal reaching the payload has nothing to
    // coerce `Control` to.
    let source = r#"
        type Control = "leave" | "cancel"
        type Line = Exit({ control: Control, note: Control? }) | Sale({ slot: i32 })

        function control(line: Line): Control
            if Exit(p) = line then
                return p.control
            end
            return "leave"
        end

        function build(): Control
            return control(Exit({ control = "cancel", note = nil }))
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    verify(&module).expect("ir should verify");
}

#[test]
fn infers_tagged_variant_binding_as_its_payload_record() {
    let source = r#"enum Kind { One }

function Kind:tonumber(): number
    return 1
end

type Goods = Upgrade({ kind: Kind })

function inspect(goods: Goods): number
    if Upgrade(upgrade) = goods then
        return upgrade.kind:tonumber()
    end
    return 0
end
"#;
    let program = parse_with_path(source, "src/shop.walu").expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("IR build should preserve the narrowed payload type");
    verify(&module).expect("IR should verify");
}

#[test]
fn verifies_function_with_tagged_union_return_type() {
    let source = r#"
        function poll(co: thread): Finished(i32) | Yielded(i32) | Error(string)
            return coroutine.resume(co)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    verify(&module).expect("ir should verify a tagged-union return type against the canonical record produced by coroutine.resume");
}

#[test]
fn lowers_alias_cast_over_tagged_union_field_in_runtime_representation() {
    // Both halves of a cast describe runtime values, but they reach lowering by
    // different routes — one from an inferred expression type, the other from a
    // declared one — and only one of those spells a nested tagged union as the
    // canonical `{ tag, value }` record. Emitting either half in its source
    // spelling makes the verifier read a conversion into what converts nothing.
    let source = r#"
        type Goods = Upgrade({ kind: i32 })
        type Offer = { goods: Goods, price: i32 }

        function entry(): i32
            local pending: Offer? = { goods = Upgrade({ kind = 1 }), price = 4 }
            if pending ~= nil then
                local sale: Offer = pending :: Offer
                return sale.price
            end
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    verify(&module).expect("casting a record alias holding a tagged union should verify");

    let casts = module
        .functions
        .iter()
        .flat_map(|function| function.blocks.values())
        .flat_map(|block| &block.instructions)
        .filter_map(|(_, instruction)| match instruction {
            Instruction::Cast { from, to, .. } => Some((from, to)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        !casts.is_empty(),
        "expected the nullable narrowing to emit a cast"
    );
    for (from, to) in casts {
        assert_eq!(
            from,
            &from.runtime_representation(),
            "cast source should be annotated in its runtime representation"
        );
        assert_eq!(
            to,
            &to.runtime_representation(),
            "cast target should be annotated in its runtime representation"
        );
    }
}

#[test]
fn lowers_nullable_recursive_record_traversal_through_unknown_anchor() {
    let source = r#"
        type Node = { value: i32, children: {Node} }

        function find(root: Node, wanted: i32): Node?
            if root.value == wanted then return root end
            for child in root.children do
                local found: Node? = find(child, wanted)
                if found ~= nil then return found end
            end
            return nil
        end

        function value(node: Node): i32
            return node.value
        end

        function find_value(root: Node, wanted: i32): i32
            local found: Node? = find(root, wanted)
            if found ~= nil then return value(found) end
            return -1
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("recursive nullable traversal should lower");
    verify(&module).expect("recursive nullable traversal should verify");

    let find = module
        .functions
        .iter()
        .find(|function| function.name == "find")
        .expect("find function");
    assert!(
        find.blocks.values().any(|block| {
            block.instructions.iter().any(|(_, instruction)| {
                matches!(
                    instruction,
                    Instruction::Cast {
                        from: Type::Record(_),
                        to: Type::Unknown,
                        ..
                    }
                )
            })
        }),
        "recursive record should widen through its unknown runtime anchor"
    );
}

#[test]
fn verifies_null_test_naming_a_nested_union_by_its_source_type() {
    // The value is a record whose field is already the canonical record; the
    // annotation names the same field by the union it came from. Those are one
    // runtime value under two names, and the verifier only ever asks about the
    // runtime value.
    let goods = Type::TaggedVariant(waluau_ast::TaggedVariant {
        tag: "Upgrade".into(),
        payload: Arc::new(Type::record(BTreeMap::from([(
            "kind".to_string(),
            Type::Numeric(NumericType::I32),
        )]))),
    });
    let declared = Type::record(BTreeMap::from([("goods".to_string(), goods)]));
    let represented = Type::record(BTreeMap::from([(
        "goods".to_string(),
        Type::canonical_tagged_union_record(),
    )]));
    let function = Function {
        name: "entry".into(),
        params: vec![("offer".into(), Type::Nullable(Arc::new(represented)))],
        return_type: Type::Bool,
        entry: BlockId(0),
        next_value: 2,
        capture_count: 0,
        value_symbols: BTreeMap::new(),
        symbol_id: None,
        source_map: FunctionSourceMap::synthetic(),
        blocks: BTreeMap::from([(
            BlockId(0),
            BasicBlock {
                id: BlockId(0),
                instructions: vec![
                    (ValueId(0), Instruction::Param(0)),
                    (
                        ValueId(1),
                        Instruction::IsNull {
                            value: ValueId(0),
                            ty: declared,
                        },
                    ),
                ],
                terminator: Terminator::Return(ValueId(1)),
            },
        )]),
    };
    verify(&Module {
        globals: Vec::new(),
        functions: vec![function],
        tooling_function_exports: std::collections::BTreeMap::new(),
        authored_function_exports: std::collections::BTreeMap::new(),
        declared_imports: Vec::new(),
        start: None,
        tag_ids: std::collections::BTreeMap::new(),
        symbol_names: std::collections::BTreeMap::new(),
        source_files: Vec::new(),
    })
    .expect("a nullable record and its inner record should match through the union naming");
}

#[test]
fn verifies_constructor_widened_into_a_nullable_tagged_union() {
    let source = r#"
        type Goods = Upgrade({ kind: i32 }) | Spell({ kind: i32 })

        function find(want: bool): Goods?
            if want then
                return Upgrade({ kind = 1 })
            end
            return nil
        end

        function slot(): i32
            local held: Goods? = Spell({ kind = 2 })
            if held ~= nil then
                return 1
            end
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    verify(&module).expect(
        "constructing into a nullable union widens the canonical record to the nullable union",
    );
}

#[test]
fn rejects_error_variant_value_access_for_string_payload() {
    let source = r#"
        function run(): i32
            local co: thread = coroutine.create(function(): i32
                return 42
            end)
            local result: Finished(i32) | Error(string) = coroutine.resume(co)
            if result is Error then
                local msg: string = result.value
                return 0
            end
            return 1
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = build(&program).expect_err("ir build should fail for string payload");
    assert!(
        error.to_string().contains("string"),
        "error should mention string payload, got: {}",
        error
    );
}

#[test]
fn includes_symbol_ids_in_ir_dump() {
    let source = r#"
        function entry(x: i32): i32
            local y: i32 = x + 1
            return y
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = &module.functions[0];
    let dump = function.dump();

    // The parameter and local variables should be mapped to symbol IDs in the IR dump.
    // e.g., `v0 = Param(0) ; @1`
    assert!(
        dump.contains("; @"),
        "expected symbol IDs in dump:\n{}",
        dump
    );
    // The function header itself should have a symbol ID comment.
    assert!(
        dump.contains("fn entry ; @"),
        "expected function symbol ID in dump:\n{}",
        dump
    );
}

#[test]
fn includes_call_symbol_ids_in_ir_dump() {
    let source = r#"
        function helper(x: i32): i32
            return x + 1
        end
        function entry(x: i32): i32
            return helper(x)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let entry = module
        .functions
        .iter()
        .find(|f| f.name == "entry")
        .expect("entry function should exist");
    let dump = entry.dump();

    assert!(
        dump.contains("Call { name: \"helper\", symbol_id: Some(SymbolId("),
        "expected helper call with symbol ID in dump:\n{}",
        dump
    );
}
#[test]
fn lowers_captured_tagged_union_parameter_narrowing() {
    // Capturing a tagged-union-typed parameter in a closure stores it in an
    // array "cell"; the cell's element type must be the canonical
    // `{ tag: i32, value: unknown }` record (the IR-level runtime
    // representation) so that `is Variant`/`.value` lowering on the value read
    // back from the cell can find the `tag`/`value` fields.
    let source = r#"
        function check(r: Yielded(unknown) | Finished(i32) | Error(string)): i32
            local f = function(): i32
                if r is Finished then
                    return r.value
                else
                    return 0
                end
            end
            return f()
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    verify(&module).expect("ir should verify");
}

// `break` jumps straight from the break site to the loop exit block. Code
// after the loop must observe the values mutated during the breaking
// iteration, so the exit block carries its own phis (normal exit edge +
// one edge per break site). Before that fix, post-loop reads resolved to the
// loop *header* phis, resurrecting start-of-iteration values on break paths
// (e.g. `local a = 1 for b = 1, 9 do a = a * 2 if a == 128 then break end end`
// left `a == 64`). Regression tests for the break phi bug surfaced by
// conformance/luau/basic.2.walu.
//
// The structural property checked: the value returned after the loop is
// defined in the block that returns it (the exit block phi), not in the loop
// header.
fn assert_returns_local_exit_phi(source: &str) {
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = &module.functions[0];
    if let Err(err) = verify(&module) {
        panic!("verify failed: {err}\n{}", function.dump());
    }

    let (return_block, return_value) = function
        .blocks
        .values()
        .find_map(|block| match block.terminator {
            Terminator::Return(value) => Some((block.id, value)),
            _ => None,
        })
        .expect("function should return a value");
    let defining_block = function
        .blocks
        .values()
        .find(|block| {
            block
                .instructions
                .iter()
                .any(|(value, _)| *value == return_value)
        })
        .expect("returned value should have a defining instruction");
    assert_eq!(
        defining_block.id,
        return_block,
        "returned value should be the loop exit phi, defined in the returning \
         block, not a header phi from an earlier block:\n{}",
        function.dump()
    );
    let (_, instruction) = defining_block
        .instructions
        .iter()
        .find(|(value, _)| *value == return_value)
        .expect("definition located above");
    let Instruction::Phi(incoming) = instruction else {
        panic!(
            "returned value should be an exit phi merging the normal exit \
             and break edges:\n{}",
            function.dump()
        );
    };
    assert_eq!(
        incoming.len(),
        2,
        "exit phi should merge the normal exit edge and the single break \
         edge:\n{}",
        function.dump()
    );
}

#[test]
fn numeric_for_break_reads_breaking_iteration_values() {
    assert_returns_local_exit_phi(
        r#"
        function entry(): i32
            local a: i32 = 1
            for b = 1, 9 do
                a = a * 2
                if a == 128 then
                    break
                end
            end
            return a
        end
    "#,
    );
}

#[test]
fn while_break_reads_breaking_iteration_values() {
    assert_returns_local_exit_phi(
        r#"
        function entry(): i32
            local x: i32 = 10
            local y: i32 = 1
            while true do
                y = y * 2
                x = x - 1
                if x == 1 then
                    break
                end
            end
            return y
        end
    "#,
    );
}

#[test]
fn repeat_mid_body_break_reads_breaking_iteration_values() {
    assert_returns_local_exit_phi(
        r#"
        function entry(): i32
            local i: i32 = 0
            repeat
                i = i + 1
                if i == 3 then
                    break
                end
                i = i + 10
            until i >= 100
            return i
        end
    "#,
    );
}

#[test]
fn array_for_in_break_reads_breaking_iteration_values() {
    assert_returns_local_exit_phi(
        r#"
        function entry(xs: {i32}): i32
            local sum: i32 = 0
            for x in xs do
                sum = sum + x
                if sum > 3 then
                    break
                end
            end
            return sum
        end
    "#,
    );
}

#[test]
fn lowers_length_of_record_field_array_in_numeric_context() {
    // Regression test for waluau-lhia: `#` on an array reached through a
    // record field failed with "cannot implicitly convert array to i32"
    // whenever the surrounding context expected a number, because the unary
    // lowering pushed the result's expected type into the operand.
    let source = r#"
        function entry(): i32
            local s: { items: {i32} } = { items = {1, 2, 3} }
            if #s.items == 3 then
                return #s.items + 1
            end
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let module = build(&program).expect("ir build should succeed");
    let function = &module.functions[0];
    assert!(function.blocks.values().any(|block| {
        block
            .instructions
            .iter()
            .any(|(_, instruction)| matches!(instruction, Instruction::ArrayLen { .. }))
    }));
}

/// Collects the string literals a lowered `entry` function emits.
fn entry_string_literals(source: &str) -> Vec<String> {
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    let function = module
        .functions
        .iter()
        .find(|function| function.name == "entry")
        .expect("entry function should exist");
    function
        .blocks
        .values()
        .flat_map(|block| block.instructions.iter())
        .filter_map(|(_, instruction)| match instruction {
            Instruction::String(literal) => Some(literal.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn statically_folded_type_builtins_evaluate_their_arguments_once() {
    let source = r#"
        function bump(box: { n: i32 }): i32
            box.n += 1
            return box.n
        end

        function entry(): string
            local box: { n: i32 } = { n = 0 }
            local first: string = type(bump(box))
            local second: string = typeof(bump(box))
            return second
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    let function = module
        .functions
        .iter()
        .find(|function| function.name == "entry")
        .expect("entry function should exist");
    let bump_calls = function
        .blocks
        .values()
        .flat_map(|block| block.instructions.iter())
        .filter(|(_, instruction)| {
            matches!(instruction, Instruction::Call { name, .. } if name == "bump")
        })
        .count();
    assert_eq!(
        bump_calls,
        2,
        "type() and typeof() must each evaluate their argument exactly once:\n{}",
        function.dump()
    );
    let folded_names = function
        .blocks
        .values()
        .flat_map(|block| block.instructions.iter())
        .filter(
            |(_, instruction)| matches!(instruction, Instruction::String(name) if name == "number"),
        )
        .count();
    assert_eq!(folded_names, 2, "known type names should remain folded");
    assert!(
        function.blocks.values().all(|block| {
            block
                .instructions
                .iter()
                .all(|(_, instruction)| !matches!(instruction, Instruction::TypeName { .. }))
        }),
        "known type names should not use runtime classification:\n{}",
        function.dump()
    );
}

#[test]
fn type_of_multi_value_call_reports_the_adjusted_first_value() {
    // Regression test for waluau-hlrk: `type()` on a multi-value call lowered
    // to the non-Lua string "unknown" because `Type::Multi` had no mapping.
    // Lua adjusts a call in single-value position to its first result.
    let literals = entry_string_literals(
        r#"
        function pair(): (number, string)
            return 1, "a"
        end

        function entry(): string
            return type(pair())
        end
    "#,
    );
    assert!(
        literals.iter().any(|literal| literal == "number"),
        "expected the first result's type name, got {literals:?}"
    );
    assert!(
        !literals.iter().any(|literal| literal == "unknown"),
        "expected no non-Lua type name, got {literals:?}"
    );
}

#[test]
fn type_of_empty_multi_value_call_reports_nil() {
    // A call returning nothing adjusts to `nil` in single-value position.
    let literals = entry_string_literals(
        r#"
        function nothing(): ()
        end

        function entry(): string
            return type(nothing())
        end
    "#,
    );
    assert!(
        literals.iter().any(|literal| literal == "nil"),
        "expected \"nil\" for an empty result list, got {literals:?}"
    );
}

#[test]
fn type_of_multi_value_call_with_unknown_first_result_dispatches_at_runtime() {
    // The adjusted value is `unknown`, so classification must go through the
    // dynamic `TypeName` instruction rather than any static string.
    let source = r#"
        function pair(): (unknown, number)
            return 1, 2
        end

        function entry(): string
            return type(pair())
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    let function = module
        .functions
        .iter()
        .find(|function| function.name == "entry")
        .expect("entry function should exist");
    assert!(
        function.blocks.values().any(|block| {
            block.instructions.iter().any(|(_, instruction)| {
                matches!(
                    instruction,
                    Instruction::TypeName {
                        from: Type::Unknown,
                        ..
                    }
                )
            })
        }),
        "expected a dynamic TypeName dispatch in:\n{}",
        function.dump()
    );
}

#[test]
fn type_of_nominal_alias_reports_the_underlying_representation() {
    // A nominal alias is transparent to `type()`: a record alias is a table,
    // and an extern alias is userdata. Neither may report "unknown".
    let literals = entry_string_literals(
        r#"
        type Point = { x: number, y: number }

        function entry(): string
            local point: Point = { x = 1, y = 2 }
            return type(point)
        end
    "#,
    );
    assert_eq!(literals, vec!["table".to_string()]);

    let literals = entry_string_literals(
        r#"
        type Handle = extern
        declare function make_handle(): Handle

        function entry(): string
            local handle: Handle = make_handle()
            return type(handle)
        end
    "#,
    );
    assert_eq!(literals, vec!["userdata".to_string()]);
}

#[test]
fn preserves_trailing_vararg_packs_in_function_returns() {
    let source = r#"
        function only(...)
            return ...
        end

        function prefixed(a, ...)
            return a, ...
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("type check should succeed");
    let module = build(&typed).expect("ir build should succeed");
    verify(&module).expect("ir should verify");

    assert_eq!(
        module.functions[0].return_type,
        Type::Variadic(Arc::new(Type::Unknown))
    );
    assert_eq!(
        module.functions[1].return_type,
        Type::Multi(vec![Type::Unknown, Type::Variadic(Arc::new(Type::Unknown)),])
    );
}
