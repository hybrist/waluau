use std::collections::BTreeMap;

use super::{
    BasicBlock, BlockId, Function, Instruction, Module, Terminator, ValueId, build, verify,
};
use waluau_ast::{BinaryOp, NumberLiteral, NumericType, Type};
use waluau_diagnostics::DiagnosticCategory;
use waluau_parser::parse;

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
fn rejects_non_bool_branch_condition() {
    let function = Function {
        name: "entry".into(),
        params: vec![],
        return_type: Type::Numeric(NumericType::I64),
        entry: BlockId(0),
        next_value: 2,
        capture_count: 0,
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
        functions: vec![function],
        start: None,
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
        functions: vec![function],
        start: None,
    })
    .expect_err("expected verifier to reject return type mismatch");
    assert!(err.to_string().contains("return in block"));
}

#[test]
fn lowers_array_literals_indexing_length_and_mutation() {
    let source = r#"
        function score_count(): i32
            local scores: {number} = {100, 250, 300}
            local first: number = scores[0]
            scores[1] = first + 1
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
        functions: vec![function],
        start: None,
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
fn verifies_loop_with_break_and_continue() {
    let source = r#"
        function entry(xs: {i32}, len: i32): i32
            local i: i32 = 0
            local acc: i32 = 0
            while i < len do
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
    let program = parse(source).expect("parse should succeed");

    let signatures: std::collections::HashMap<_, (Vec<waluau_ast::Type>, waluau_ast::Type)> =
        program
            .functions
            .iter()
            .map(|function| {
                let return_type = function.return_type.clone().ok_or_else(|| {
                    waluau_diagnostics::Diagnostic::new(format!(
                        "function '{}' must have a concrete return type before IR lowering",
                        function.name
                    ))
                })?;
                Ok((
                    function.name.to_string(),
                    (
                        function
                            .params
                            .iter()
                            .map(|param| param.ty.clone())
                            .collect(),
                        return_type,
                    ),
                ))
            })
            .collect::<Result<_, waluau_diagnostics::Diagnostic>>()
            .expect("signatures should build");

    let mut lowered = super::build_function(&program.functions[0], &signatures, &program.sources)
        .expect("ir lowering should succeed");
    let mut functions = Vec::new();
    functions.push(lowered.remove(0));
    functions.extend(lowered);
    let module = super::Module {
        functions,
        start: None,
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
            .any(|(_, instruction)| { matches!(instruction, Instruction::Print { .. }) })
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
fn lowers_coroutine_builtins_to_typed_instructions() {
    let source = r#"
        function run(): i32
            local co: thread = coroutine.create(function(): i32
                coroutine.yield(1)
                return 2
            end)
            local ok: bool, value: i32 = coroutine.resume(co)
            local closed: bool = coroutine.close(co)
            return value
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
