use super::parse;
use waluau_ast::{
    AssignOp, BinaryOp, FunctionName, NumberLiteral, NumericType, Rebindability, Stmt, Type,
    UnaryOp,
};

#[test]
fn parses_v0_function() {
    let source = r#"
        function choose(flag: bool, x: i32, y: number): f64
            local result: f64 = y
            if flag then
                result = x + 1
            else
                result = x + y
            end
            return result
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    assert_eq!(program.functions.len(), 1);
}

#[test]
fn parses_numeric_type_aliases() {
    let source = r#"
        function widen(x: number, y: f32, z: u64, w: i64): f64
            local result: f64 = x
            return result
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert_eq!(function.params[0].ty, Type::Numeric(NumericType::F64));
    assert_eq!(function.params[1].ty, Type::Numeric(NumericType::F32));
    assert_eq!(function.params[2].ty, Type::Numeric(NumericType::U64));
    assert_eq!(function.params[3].ty, Type::Numeric(NumericType::I64));
    assert_eq!(function.return_type, Some(Type::Numeric(NumericType::F64)));
}

#[test]
fn parses_unit_and_void_type_aliases() {
    let source = r#"
        function a(): unit
            return 1
        end
        function b(): void
            return 2
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    assert_eq!(program.functions[0].return_type, Some(Type::Unit));
    assert_eq!(program.functions[1].return_type, Some(Type::Unit));
}

#[test]
fn parses_type_declarations_and_named_type_references() {
    let source = r#"
        type Meters = number

        function scale(x: Meters): Meters
            return x
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    assert_eq!(program.type_declarations.len(), 1);
    assert_eq!(program.type_declarations[0].name, "Meters");
    assert!(program.type_declarations[0].type_params.is_empty());
    assert_eq!(program.type_declarations[0].ty, Type::number());
    assert_eq!(
        program.functions[0].params[0].ty,
        Type::Named {
            name: "Meters".into(),
            type_args: vec![],
        }
    );
    assert_eq!(
        program.functions[0].return_type,
        Some(Type::Named {
            name: "Meters".into(),
            type_args: vec![],
        })
    );
}

#[test]
fn parses_extern_type_declaration() {
    let source = r#"
        type Element = extern

        function identity(value: Element): Element
            return value
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    assert_eq!(program.type_declarations.len(), 1);
    assert_eq!(program.type_declarations[0].name, "Element");
    assert_eq!(program.type_declarations[0].ty, Type::Extern);
    assert_eq!(
        program.functions[0].params[0].ty,
        Type::Named {
            name: "Element".into(),
            type_args: vec![],
        }
    );
}

#[test]
fn parses_extern_inheritance_and_if_cast() {
    let source = r#"
        type Node = extern
        type Element = extern extends Node

        function entry(value: Node): i32
            if Element(element) = value then
                return 1
            else
                return 0
            end
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    assert_eq!(program.type_declarations.len(), 2);
    assert_eq!(program.type_declarations[1].name, "Element");
    assert_eq!(
        program.type_declarations[1].ty,
        Type::ExternSubtype(Box::new(Type::Named {
            name: "Node".into(),
            type_args: vec![],
        }))
    );
    assert!(matches!(
        &program.functions[0].body[0],
        Stmt::IfCast {
            target_name,
            binding,
            ..
        } if target_name == "Element" && binding == "element"
    ));
}

#[test]
fn parses_if_call_condition_without_confusing_it_for_if_cast() {
    let source = r#"
        function contains_text(haystack: string, needle: string): bool
            return haystack:find(needle) ~= -1
        end

        function entry(value: string): bool
            if contains_text(value, "card") then
                return true
            end
            return false
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    assert!(matches!(
        &program.functions[1].body[0],
        Stmt::If {
            condition: waluau_ast::Expr::Call { .. },
            ..
        }
    ));
}

#[test]
fn parses_declared_host_method_with_implicit_receiver_param() {
    let source = r#"
        type Element = extern
        declare function Element:addEventListener(event: string): i32
    "#;

    let program = parse(source).expect("parse should succeed");
    let declared = &program.declared_imports[0];
    assert_eq!(declared.name, "Element.addEventListener");
    assert_eq!(declared.params.len(), 2);
    assert_eq!(
        declared.params[0].ty,
        Type::Named {
            name: "Element".into(),
            type_args: vec![],
        }
    );
    assert_eq!(declared.params[1].ty, Type::String);
}

#[test]
fn parses_generic_extern_promise_api_declarations() {
    let source = r#"
        type Response = extern
        type Promise<T> = extern

        declare function fetch(url: string): Promise<Response>
        declare function Response:text(): Promise<string>
    "#;

    let program = parse(source).expect("parse should succeed");
    assert_eq!(program.type_declarations.len(), 2);
    assert_eq!(program.type_declarations[1].name, "Promise");
    assert_eq!(program.type_declarations[1].type_params, vec!["T"]);
    assert_eq!(program.type_declarations[1].ty, Type::Extern);

    let fetch = &program.declared_imports[0];
    assert_eq!(fetch.name, "fetch");
    assert_eq!(fetch.params.len(), 1);
    assert_eq!(fetch.params[0].ty, Type::String);
    assert_eq!(
        fetch.return_type,
        Type::Named {
            name: "Promise".into(),
            type_args: vec![Type::Named {
                name: "Response".into(),
                type_args: vec![],
            }],
        }
    );

    let text = &program.declared_imports[1];
    assert_eq!(text.name, "Response.text");
    assert_eq!(text.params.len(), 1);
    assert_eq!(
        text.params[0].ty,
        Type::Named {
            name: "Response".into(),
            type_args: vec![],
        }
    );
    assert_eq!(
        text.return_type,
        Type::Named {
            name: "Promise".into(),
            type_args: vec![Type::String],
        }
    );
}

#[test]
fn parses_declared_property_as_getter_and_setter_imports() {
    let source = r#"
        type Element = extern
        declare property Element:inner_text: string
    "#;

    let program = parse(source).expect("parse should succeed");
    assert_eq!(program.declared_imports.len(), 2);
    let getter = &program.declared_imports[0];
    assert_eq!(getter.name, "Element.get/inner_text");
    assert_eq!(getter.params.len(), 1);
    assert_eq!(getter.return_type, Type::String);

    let setter = &program.declared_imports[1];
    assert_eq!(setter.name, "Element.set/inner_text");
    assert_eq!(setter.params.len(), 2);
    assert_eq!(setter.params[1].ty, Type::String);
    assert_eq!(setter.return_type, Type::Unit);
}

#[test]
fn parses_nullable_extern_annotations_and_nil_checks() {
    let source = r#"
        type Element = extern

        function present(value: Element?): bool
            return value ~= nil
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    assert_eq!(
        program.functions[0].params[0].ty,
        Type::Nullable(Box::new(Type::Named {
            name: "Element".into(),
            type_args: vec![],
        }))
    );
    assert!(matches!(
        &program.functions[0].body[0],
        waluau_ast::Stmt::Return(waluau_ast::Expr::Binary {
            op: waluau_ast::BinaryOp::NotEq,
            right,
            ..
        }) if matches!(right.as_ref(), waluau_ast::Expr::Nil(_))
    ));
}

#[test]
fn parses_generic_type_declarations_and_references() {
    let source = r#"
        type Pair<A, B> = {first: A, second: B}

        function entry(value: Pair<i32, bool>): Pair<i32, bool>
            return value
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    assert_eq!(program.type_declarations[0].type_params, vec!["A", "B"]);
    assert!(matches!(
        &program.functions[0].params[0].ty,
        Type::Named { name, type_args }
        if name == "Pair" && type_args.len() == 2
    ));
}

#[test]
fn parses_paren_unit_type_alias() {
    let source = r#"
        function f(): ()
            return 0
        end
        function g(cb: () -> i32): ()
            return cb()
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    assert_eq!(program.functions[0].return_type, Some(Type::Unit));
    assert_eq!(program.functions[1].return_type, Some(Type::Unit));
    assert_eq!(
        program.functions[1].params[0].ty,
        Type::Function {
            params: vec![],
            return_type: Box::new(Type::Numeric(NumericType::I32)),
        }
    );
}

#[test]
fn parses_multi_value_return_type_in_function_type() {
    // () -> (bool, i32)  — iterator-style function type
    let source = r#"
        function take_iter(iter: () -> (bool, i32)): i32
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    assert_eq!(
        program.functions[0].params[0].ty,
        Type::Function {
            params: vec![],
            return_type: Box::new(Type::Multi(vec![
                Type::Bool,
                Type::Numeric(NumericType::I32),
            ])),
        }
    );
}

#[test]
fn parses_multi_value_return_type_on_local() {
    // local with explicit function type annotation using multi-value return
    let source = r#"
        function entry(): i32
            local iter: () -> (bool, i32) = function(): bool, i32
                return false, 0
            end
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let body = &program.functions[0].body;
    assert!(matches!(body[0], waluau_ast::Stmt::Let { .. }));
}

#[test]
fn parses_paren_multi_value_return_in_function_decl() {
    // function two(): (f64, f64) — parenthesised form in function declaration return type
    let source = r#"
        function two(): (f64, f64)
            return 1, 2
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    assert_eq!(
        program.functions[0].return_type,
        Some(Type::Multi(vec![Type::number(), Type::number(),]))
    );
}

#[test]
fn parses_postfix_numeric_casts() {
    let source = r#"
        function cast(x: i64): i32
            return (x + 1) :: i32
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::Return(waluau_ast::Expr::Cast {
            ty: Type::Numeric(NumericType::I32),
            ..
        })
    ));
}

#[test]
fn preserves_large_integer_literal_text() {
    let source = r#"
        function entry(): u64
            return 18446744073709551615
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::Return(waluau_ast::Expr::Number(NumberLiteral { raw }, _))
            if raw == "18446744073709551615"
    ));
}

#[test]
fn parses_unary_and_elseif_forms() {
    let source = r#"
        function entry(flag: bool, x: i32): i32
            if not flag then
                return -x
            elseif x > 0 then
                return x
            else
                return 0
            end
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::If {
            condition: waluau_ast::Expr::Unary {
                op: UnaryOp::Not,
                ..
            },
            else_body,
            ..
        } if matches!(
            else_body.as_slice(),
            [waluau_ast::Stmt::If {
                then_body,
                ..
            }] if matches!(
                then_body.as_slice(),
                [waluau_ast::Stmt::Return(waluau_ast::Expr::Name(name, _, _))] if name == "x"
            )
        )
    ));
}

#[test]
fn parses_floor_division_with_multiplicative_precedence() {
    let source = r#"
        function entry(x: number, y: number, z: number): number
            return -x // y * z % 2 / 3
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    let waluau_ast::Stmt::Return(waluau_ast::Expr::Binary {
        op: BinaryOp::Div,
        left: div_left,
        ..
    }) = &function.body[0]
    else {
        panic!("return should end with division");
    };
    let waluau_ast::Expr::Binary {
        op: BinaryOp::Mod,
        left: mod_left,
        ..
    } = div_left.as_ref()
    else {
        panic!("division left side should be modulo");
    };
    let waluau_ast::Expr::Binary {
        op: BinaryOp::Mul,
        left: mul_left,
        ..
    } = mod_left.as_ref()
    else {
        panic!("modulo left side should be multiplication");
    };
    let waluau_ast::Expr::Binary {
        op: BinaryOp::FloorDiv,
        left: floor_div_left,
        ..
    } = mul_left.as_ref()
    else {
        panic!("multiplication left side should be floor division");
    };
    assert!(matches!(
        floor_div_left.as_ref(),
        waluau_ast::Expr::Unary {
            op: UnaryOp::Neg,
            ..
        }
    ));
}

#[test]
fn parses_exponentiation_tighter_than_unary_minus() {
    // Lua: `-x ^ y` is `-(x ^ y)`.
    let source = r#"
        function entry(x: f64, y: f64): f64
            return -x ^ y
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    let waluau_ast::Stmt::Return(waluau_ast::Expr::Unary {
        op: UnaryOp::Neg,
        expr,
        ..
    }) = &function.body[0]
    else {
        panic!("return should be a unary negation");
    };
    assert!(
        matches!(
            expr.as_ref(),
            waluau_ast::Expr::Binary {
                op: BinaryOp::Pow,
                ..
            }
        ),
        "negation should apply to the exponentiation result"
    );
}

#[test]
fn parses_exponentiation_right_associatively() {
    // Lua: `x ^ y ^ z` is `x ^ (y ^ z)`, and `x ^ -y` is allowed.
    let source = r#"
        function entry(x: f64, y: f64, z: f64): f64
            return x ^ y ^ -z
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    let waluau_ast::Stmt::Return(waluau_ast::Expr::Binary {
        op: BinaryOp::Pow,
        right,
        ..
    }) = &function.body[0]
    else {
        panic!("return should be exponentiation");
    };
    let waluau_ast::Expr::Binary {
        op: BinaryOp::Pow,
        right: inner_right,
        ..
    } = right.as_ref()
    else {
        panic!("exponentiation should associate to the right");
    };
    assert!(
        matches!(
            inner_right.as_ref(),
            waluau_ast::Expr::Unary {
                op: UnaryOp::Neg,
                ..
            }
        ),
        "exponent may be a unary expression"
    );
}

#[test]
fn parses_concat_between_add_and_comparison_precedence() {
    let source = r#"
        function entry(a: string, b: string, x: i32): bool
            return a .. b == "x" .. "y"
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::Return(waluau_ast::Expr::Binary {
            op: BinaryOp::Eq,
            left,
            right,
            ..
        }) if matches!(left.as_ref(), waluau_ast::Expr::Binary { op: BinaryOp::Concat, .. })
            && matches!(right.as_ref(), waluau_ast::Expr::Binary { op: BinaryOp::Concat, .. })
    ));
}

#[test]
fn rejects_legacy_function_local_and_return_syntax() {
    let source = r#"
        fn entry(x: i32) -> i32
            let y: i32 = x
            return y
        end
    "#;

    let error = parse(source).expect_err("parse should fail");
    let message = error.to_string();
    assert!(message.contains("unsupported 'fn'") || message.contains("unsupported 'let'"));
}

#[test]
fn rejects_symbolic_logical_operators() {
    let source = r#"
        function entry(a: bool, b: bool): bool
            return a && b || a
        end
    "#;

    let error = parse(source).expect_err("parse should fail");
    let message = error.to_string();
    assert!(message.contains("unsupported '&&'") || message.contains("unsupported '||'"));
}

#[test]
fn allows_unresolved_named_type_references_for_later_resolution() {
    let source = r#"
        function add(x: f3, y: f4): f1
            local z: f2 = x + y
            return z
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert_eq!(
        function.params[0].ty,
        Type::Named {
            name: "f3".into(),
            type_args: vec![]
        }
    );
    assert_eq!(
        function.params[1].ty,
        Type::Named {
            name: "f4".into(),
            type_args: vec![]
        }
    );
    assert_eq!(
        function.return_type,
        Some(Type::Named {
            name: "f1".into(),
            type_args: vec![]
        })
    );
}

#[test]
fn parses_array_types_literals_indexing_and_length() {
    let source = r#"
        function score_count(): i32
            local scores: {number} = {100, 250, 300}
            scores[1] = 250
            return #scores
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert_eq!(function.return_type, Some(Type::Numeric(NumericType::I32)));
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::Let {
            ty: Some(Type::Array(element)),
            value: waluau_ast::Expr::ArrayLiteral { elements, .. },
            ..
        } if elements.len() == 3
            && **element == Type::number()
    ));
    assert!(matches!(
        &function.body[1],
        waluau_ast::Stmt::IndexAssign {
            op: AssignOp::Set,
            index,
            ..
        } if matches!(index.as_ref(), waluau_ast::Expr::Number(NumberLiteral { raw: _ }, _))
    ));
    assert!(matches!(
        &function.body[2],
        waluau_ast::Stmt::Return(waluau_ast::Expr::Unary {
            op: UnaryOp::Len,
            ..
        })
    ));
}

#[test]
fn parses_named_table_literals() {
    let source = r#"
        return {
            add = function (a: f64, b: f64): f64
                return a + b
            end,
        }
    "#;

    let program = parse(source).expect("parse should succeed");
    assert!(matches!(
        program.export,
        Some(waluau_ast::Expr::TableLiteral { fields, .. }) if fields.len() == 1
            && fields[0].name == "add"
            && matches!(fields[0].value, waluau_ast::Expr::Function(_))
    ));
}

#[test]
fn parses_table_literal_in_local_expression_context() {
    let source = r#"
        function entry(): i32
            local t = { x = 1 }
            return t.x
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let body = &program.functions[0].body;
    assert!(matches!(
        &body[0],
        Stmt::Let {
            value: waluau_ast::Expr::TableLiteral { fields, .. },
            ..
        } if fields.len() == 1
            && fields[0].name == "x"
    ));
}

#[test]
fn parses_namespace_member_access() {
    let source = r#"
        function main(): f64
            local m = require("./ops")
            return m.add(2.0, 3.0)
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let body = &program.functions[0].body;
    assert!(matches!(
        &body[1],
        Stmt::Return(waluau_ast::Expr::Call {
            callee,
            args,
            type_args: _,
            ..
        }) if args.len() == 2
            && matches!(
                callee.as_ref(),
                waluau_ast::Expr::Field { name, .. } if name == "add"
            )
    ));
}

#[test]
fn parses_method_call_syntax() {
    let source = r#"
        function main(obj: { value: i32 }): i32
            return obj:update(1)
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    assert!(matches!(
        &program.functions[0].body[0],
        Stmt::Return(waluau_ast::Expr::MethodCall {
            receiver,
            name,
            args,
            ..
        }) if name == "update"
            && args.len() == 1
            && matches!(receiver.as_ref(), waluau_ast::Expr::Name(base, _, _) if base == "obj")
    ));
}

#[test]
fn parses_generic_method_call_syntax() {
    let source = r#"
        function main(obj: { value: i32 }): i32
            return obj:identity<i32>(42)
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    assert!(matches!(
        &program.functions[0].body[0],
        Stmt::Return(waluau_ast::Expr::MethodCall {
            receiver,
            name,
            type_args,
            args,
            ..
        }) if name == "identity"
            && type_args.len() == 1
            && matches!(&type_args[0], waluau_ast::Type::Numeric(waluau_ast::NumericType::I32))
            && args.len() == 1
            && matches!(receiver.as_ref(), waluau_ast::Expr::Name(base, _, _) if base == "obj")
            && matches!(&args[0], waluau_ast::Expr::Number(_, _))
    ));
}

#[test]
fn parses_call_with_string_sugar() {
    let source = r#"
        function main(): i32
            return greet "world"
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    assert!(matches!(
        &program.functions[0].body[0],
        Stmt::Return(waluau_ast::Expr::Call { args, .. })
            if args.len() == 1
                && matches!(&args[0], waluau_ast::Expr::String(value, _) if value == "world")
    ));
}

#[test]
fn parses_call_with_table_sugar() {
    let source = r#"
        function main(): i32
            return make_thing { x = 0, y = 1 }
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    assert!(matches!(
        &program.functions[0].body[0],
        Stmt::Return(waluau_ast::Expr::Call { args, .. })
            if args.len() == 1
                && matches!(&args[0], waluau_ast::Expr::TableLiteral { .. })
    ));
}

#[test]
fn parses_method_call_with_string_sugar() {
    let source = r#"
        function main(obj: { value: i32 }): i32
            return obj:log "hello"
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    assert!(matches!(
        &program.functions[0].body[0],
        Stmt::Return(waluau_ast::Expr::MethodCall { name, args, .. })
            if name == "log"
                && args.len() == 1
                && matches!(&args[0], waluau_ast::Expr::String(value, _) if value == "hello")
    ));
}

#[test]
fn parses_single_quoted_string_literal() {
    let source = r#"
        function main(): string
            return 'it\'s'
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    assert!(matches!(
        &program.functions[0].body[0],
        Stmt::Return(waluau_ast::Expr::String(value, _)) if value == "it's"
    ));
}

#[test]
fn parses_method_call_with_table_sugar() {
    let source = r#"
        function main(obj: { value: i32 }): i32
            return obj:configure { enabled = true }
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    assert!(matches!(
        &program.functions[0].body[0],
        Stmt::Return(waluau_ast::Expr::MethodCall { name, args, .. })
            if name == "configure"
                && args.len() == 1
                && matches!(&args[0], waluau_ast::Expr::TableLiteral { .. })
    ));
}

#[test]
fn records_call_span() {
    let source = r#"
        function main(): i32
            return add(1, 2)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let Stmt::Return(waluau_ast::Expr::Call { span, .. }) = &program.functions[0].body[0] else {
        panic!("expected return call");
    };
    let span = span.expect("call span should be present");
    assert!(span.end > span.start);
}

#[test]
fn parses_repeat_until_loop() {
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
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[1],
        waluau_ast::Stmt::Repeat { body, condition } if body.len() == 1
            && matches!(condition, waluau_ast::Expr::Binary { .. })
    ));
}

#[test]
fn parses_numeric_for_loop_with_optional_step() {
    let source = r#"
        function entry(limit: i32): i32
            local acc: i32 = 0
            for i = 0, limit do
                acc += i
            end
            for j = limit, 0, -2 do
                acc += j
            end
            return acc
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[1],
        Stmt::NumericFor {
            name,
            step: None,
            body,
            ..
        } if name == "i" && body.len() == 1
    ));
    assert!(matches!(
        &function.body[2],
        Stmt::NumericFor {
            name,
            step: Some(_),
            body,
            ..
        } if name == "j" && body.len() == 1
    ));
}

#[test]
fn parses_for_in_loop_with_multiple_bindings() {
    let source = r#"
        function entry(): i32
            local acc: i32 = 0
            local iter = function(): bool, i32, i32
                return true, 1, 2
            end
            for i, v in iter do
                acc += i + v
            end
            return acc
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[2],
        Stmt::ForIn { names, body, .. } if names == &vec!["i".to_string(), "v".to_string()] && body.len() == 1
    ));
}

#[test]
fn parses_const_declarations_in_both_forms() {
    let source = r#"
        function entry(v: i32): i32
            local a <const>: i32 = v
            const b: i32 = a
            return b
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::Let {
            name,
            rebindability: Rebindability::Const,
            ..
        } if name == "a"
    ));
    assert!(matches!(
        &function.body[1],
        waluau_ast::Stmt::Let {
            name,
            rebindability: Rebindability::Const,
            ..
        } if name == "b"
    ));
}

#[test]
fn parses_function_type_and_literal_assignment() {
    let source = r#"
        function entry(): i32
            local add1: (i32) -> i32 = function(x: i32): i32
                return x + 1
            end
            return add1(41)
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::Let {
            ty: Some(Type::Function { params, return_type }),
            value: waluau_ast::Expr::Function(_),
            ..
        } if params == &vec![Type::Numeric(NumericType::I32)]
            && **return_type == Type::Numeric(NumericType::I32)
    ));
    assert!(matches!(
        &function.body[1],
        waluau_ast::Stmt::Return(waluau_ast::Expr::Call { .. })
    ));
}

#[test]
fn parses_record_type_annotations_and_function_signature_types() {
    let source = r#"
        function make_point(x: i32, y: i32): { x: i32, y: i32 }
            return { x = x, y = y }
        end

        function entry(): i32
            local p: { x: i32, y: i32 } = mk_point(1, 2)
            local mk: (i32, i32) -> { x: i32, y: i32 } = mk_point
            return p.x + mk(3, 4).y
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    assert!(matches!(
        program.functions[0].return_type,
        Some(Type::Record(_))
    ));
    assert!(matches!(
        &program.functions[1].body[0],
        Stmt::Let {
            ty: Some(Type::Record(_)),
            ..
        }
    ));
    assert!(matches!(
        &program.functions[1].body[1],
        Stmt::Let {
            ty: Some(Type::Function { return_type, .. }),
            ..
        } if matches!(return_type.as_ref(), Type::Record(_))
    ));
}

#[test]
fn parses_tagged_union_type_annotations_and_is_checks() {
    let source = r#"
        type Resume<R> = Yielded(unknown) | Finished(R) | Error(string)

        function read(result: Resume<i32>): i32
            if result is Yielded then
                return 0
            else
                return result.value
            end
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    assert!(matches!(
        &program.type_declarations[0].ty,
        Type::TaggedUnion(variants)
            if variants.len() == 3
                && variants.iter().any(|variant| variant.tag == "Yielded")
                && variants.iter().any(|variant| variant.tag == "Finished")
                && variants.iter().any(|variant| variant.tag == "Error")
    ));
    assert!(matches!(
        &program.functions[0].body[0],
        Stmt::If {
            condition: waluau_ast::Expr::IsVariant { tag, .. },
            ..
        } if tag == "Yielded"
    ));
}

#[test]
fn parses_function_literal_without_return_annotation() {
    let source = r#"
        function entry(): i32
            local add1 = function(x: i32)
                return x + 1
            end
            return add1(41)
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::Let {
            ty: None,
            value: waluau_ast::Expr::Function(waluau_ast::FunctionExpr {
                return_type: None,
                ..
            }),
            ..
        }
    ));
}

#[test]
fn const_is_contextual_not_reserved() {
    let source = r#"
        function entry(): i32
            local const: i32 = 20
            return const
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::Let {
            name,
            rebindability: Rebindability::Rebindable,
            ..
        } if name == "const"
    ));
}

#[test]
fn parses_local_without_annotation() {
    let source = r#"
        function entry(): i32
            local x = 20
            return x
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::Let { name, ty: None, .. } if name == "x"
    ));
}

#[test]
fn parses_top_level_function_without_return_annotation() {
    let source = r#"
        function entry(x: i32)
            return x + 1
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert_eq!(function.return_type, None);
}

#[test]
fn parses_compound_assignments() {
    let source = r#"
        function entry(xs: {i32}, i: i32, x: i32): i32
            x += 1
            xs[i] += x
            return x
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::Assign {
            op: AssignOp::Compound(BinaryOp::Add),
            ..
        }
    ));
    assert!(matches!(
        &function.body[1],
        waluau_ast::Stmt::IndexAssign {
            op: AssignOp::Compound(BinaryOp::Add),
            ..
        }
    ));
}

#[test]
fn parses_all_compound_assignment_operators() {
    let source = r#"
        function entry(a: f64, s: string): f64
            a -= 1
            a *= 2
            a /= 3
            a //= 4
            a %= 5
            a ^= 6
            s ..= "x"
            return a
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    let ops: Vec<AssignOp> = function.body[..7]
        .iter()
        .map(|stmt| match stmt {
            waluau_ast::Stmt::Assign { op, .. } => *op,
            other => panic!("expected assignment, got {other:?}"),
        })
        .collect();
    assert_eq!(
        ops,
        vec![
            AssignOp::Compound(BinaryOp::Sub),
            AssignOp::Compound(BinaryOp::Mul),
            AssignOp::Compound(BinaryOp::Div),
            AssignOp::Compound(BinaryOp::FloorDiv),
            AssignOp::Compound(BinaryOp::Mod),
            AssignOp::Compound(BinaryOp::Pow),
            AssignOp::Compound(BinaryOp::Concat),
        ]
    );
}

#[test]
fn parses_field_assignment_statement() {
    let source = r#"
        function entry(): i32
            local t = {}
            t.x = 10
            return t.x
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[1],
        waluau_ast::Stmt::FieldAssign {
            op: AssignOp::Set,
            name,
            value: waluau_ast::Expr::Number(NumberLiteral { raw: _ }, _),
            ..
        } if name == "x"
    ));
}

#[test]
fn parses_if_expression_in_return() {
    let source = r#"
        function entry(flag: bool, x: i32, y: i32): i32
            return if flag then x else y
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::Return(waluau_ast::Expr::If { .. })
    ));
}

#[test]
fn parses_multi_return_signature_and_statement() {
    let source = r#"
        function pair(x: i32, y: bool): i32, bool
            return x, y
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        function.return_type,
        Some(Type::Multi(ref tys)) if tys == &vec![Type::Numeric(NumericType::I32), Type::Bool]
    ));
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::ReturnMulti(values) if values.len() == 2
    ));
}

#[test]
fn parses_bare_return_as_nil() {
    let source = r#"
        function entry(): unit
            return
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::Return(waluau_ast::Expr::Nil(_))
    ));
}

#[test]
fn parses_bare_return_before_until_as_nil() {
    // A bare `return` may be the final statement of a `repeat ... until`
    // body, so `until` must be treated as a statement terminator.
    let source = r#"
        function entry(): unit
            repeat
                return
            until true
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::Repeat { body, .. }
            if matches!(body[0], waluau_ast::Stmt::Return(waluau_ast::Expr::Nil(_)))
    ));
}

#[test]
fn parses_uninitialized_local_as_nil() {
    let source = r#"
        function entry(): unit
            local a
            return a
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::Let {
            name,
            value: waluau_ast::Expr::Nil(_),
            ..
        } if name == "a"
    ));
}

#[test]
fn parses_uninitialized_multi_local_as_nil_values() {
    let source = r#"
        function entry(): unit
            local a, b
            return a
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::LetMulti { bindings, values }
            if bindings.len() == 2
                && values.len() == 2
                && values.iter().all(|value| matches!(value, waluau_ast::Expr::Nil(_)))
    ));
}

#[test]
fn parses_uninitialized_local_followed_by_call() {
    // An uninitialized local can be followed by any statement, including one
    // that begins with an identifier (a call or assignment), not just block
    // terminators or statement keywords.
    let source = r#"
        function entry(value: i32): unit
            local a
            host(a)
            return
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::Let {
            name,
            value: waluau_ast::Expr::Nil(_),
            ..
        } if name == "a"
    ));
    assert!(matches!(&function.body[1], waluau_ast::Stmt::Expr(_)));
}

#[test]
fn parses_uninitialized_local_before_until() {
    let source = r#"
        function entry(): unit
            repeat
                local a
            until true
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::Repeat { body, .. }
            if matches!(
                &body[0],
                waluau_ast::Stmt::Let { value: waluau_ast::Expr::Nil(_), .. }
            )
    ));
}

#[test]
fn parses_uninitialized_multi_local_followed_by_assignment() {
    let source = r#"
        function entry(): unit
            local a, b
            a = b
            return
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::LetMulti { bindings, values }
            if bindings.len() == 2
                && values.len() == 2
                && values.iter().all(|value| matches!(value, waluau_ast::Expr::Nil(_)))
    ));
}

#[test]
fn parses_multi_local_and_multi_assignment() {
    let source = r#"
        function entry(x: i32, y: i32): i32
            local a: i32, b: i32 = x, y
            a, b = b, a
            return a
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::LetMulti { bindings, values } if bindings.len() == 2 && values.len() == 2
    ));
    assert!(matches!(
        &function.body[1],
        waluau_ast::Stmt::AssignMulti { targets, values, .. } if targets.len() == 2 && values.len() == 2
    ));
}

#[test]
fn parses_untyped_multi_local() {
    let source = r#"
        function entry(x: i32, y: i32): i32
            local a, b = x, y
            return a
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::LetMulti { bindings, values }
            if bindings.len() == 2
                && values.len() == 2
                && bindings.iter().all(|binding| binding.ty.is_none())
    ));
}

#[test]
fn rejects_if_expression_without_else() {
    let source = r#"
        function entry(flag: bool, x: i32): i32
            return if flag then x
        end
    "#;
    let error = parse(source).expect_err("parse should fail");
    assert!(
        error
            .to_string()
            .contains("expected 'else' in if expression")
    );
}

#[test]
fn rejects_incomplete_call_expression_without_hanging() {
    let error = parse("a(").expect_err("parse should fail");
    let message = error.to_string();
    assert!(
        message.contains("unexpected end of input")
            || message.contains("expected ')' after call arguments")
    );
}

#[test]
fn parses_top_level_statements_with_functions() {
    let source = r#"
        local x: i32 = 41
        function add1(v: i32): i32
            return v + 1
        end
        x += 1
    "#;
    let program = parse(source).expect("parse should succeed");
    assert_eq!(program.functions.len(), 1);
    assert_eq!(program.top_level.len(), 2);
}

#[test]
fn captures_trailing_top_level_return_as_module_export() {
    let source = r#"
        function helper(): i32
            return 1
        end
        return helper
    "#;
    let program = parse(source).expect("parse should succeed");
    assert!(matches!(program.export, Some(waluau_ast::Expr::Name(name, _, _)) if name == "helper"));
}

#[test]
fn rejects_top_level_return_that_is_not_last() {
    let source = r#"
        return 1
        local x: i32 = 2
    "#;
    let error = parse(source).expect_err("parse should fail");
    assert!(
        error
            .to_string()
            .contains("top-level return must be the final statement")
    );
}

#[test]
fn parses_require_as_a_dedicated_node() {
    let source = r#"
        local add: (i32, i32) -> i32 = require("./add")
    "#;
    let program = parse(source).expect("parse should succeed");
    let waluau_ast::Stmt::Let { value, .. } = &program.top_level[0] else {
        panic!("expected a let binding");
    };
    assert!(matches!(value, waluau_ast::Expr::Require(path, _) if path == "./add"));
}

#[test]
fn parses_require_with_string_sugar() {
    let source = r#"
        local add: (i32, i32) -> i32 = require "./add"
    "#;
    let program = parse(source).expect("parse should succeed");
    let waluau_ast::Stmt::Let { value, .. } = &program.top_level[0] else {
        panic!("expected a let binding");
    };
    assert!(matches!(value, waluau_ast::Expr::Require(path, _) if path == "./add"));
}

#[test]
fn parses_string_literals_as_values() {
    let source = r#"local x: string = "ok""#;
    let program = parse(source).expect("parse should succeed");
    let waluau_ast::Stmt::Let { value, .. } = &program.top_level[0] else {
        panic!("expected a let binding");
    };
    assert!(matches!(value, waluau_ast::Expr::String(value, _) if value == "ok"));
}

#[test]
fn parses_bytes_literals_as_values() {
    let source = r#"local x: bytes = b"OK\x00""#;
    let program = parse(source).expect("parse should succeed");
    let waluau_ast::Stmt::Let { value, .. } = &program.top_level[0] else {
        panic!("expected a let binding");
    };
    assert!(matches!(value, waluau_ast::Expr::Bytes(value, _) if value == &vec![79, 75, 0]));
}

#[test]
fn parses_break_and_continue_in_loops() {
    let source = r#"
        function entry(): i32
            while true do
                break
            end
            repeat
                continue
            until true
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    let mut saw_break = false;
    let mut saw_continue = false;
    for stmt in &function.body {
        match stmt {
            Stmt::While { body, .. } if matches!(body.first(), Some(Stmt::Break)) => {
                saw_break = true;
            }
            Stmt::Repeat { body, .. } if matches!(body.first(), Some(Stmt::Continue)) => {
                saw_continue = true;
            }
            _ => {}
        }
    }
    assert!(saw_break, "expected a while loop containing a break");
    assert!(
        saw_continue,
        "expected a repeat-until loop containing a continue"
    );
}

#[test]
fn parses_generic_function_declaration() {
    let source = r#"
        function identity<T>(x: T): T
            return x
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert_eq!(
        function.name,
        waluau_ast::FunctionName::Simple("identity".to_string())
    );
    assert_eq!(function.type_params, vec!["T".to_string()]);
    assert_eq!(function.params[0].ty, Type::TypeParam("T".into()));
    assert_eq!(function.return_type, Some(Type::TypeParam("T".into())));
}

#[test]
fn parses_method_function_declaration_name() {
    let source = r#"
        function point:length(): f64
            return 0
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert_eq!(
        function.name,
        FunctionName::Method {
            table: "point".to_string(),
            method: "length".to_string(),
        }
    );
}

#[test]
fn parses_generic_method_function_declaration() {
    let source = r#"
        function point:identity<T>(value: T): T
            return value
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert_eq!(
        function.name,
        FunctionName::Method {
            table: "point".to_string(),
            method: "identity".to_string(),
        }
    );
    assert_eq!(function.type_params, vec!["T".to_string()]);
    assert_eq!(function.params[0].ty, Type::TypeParam("T".into()));
    assert_eq!(function.return_type, Some(Type::TypeParam("T".into())));
}

#[test]
fn parses_generic_call_with_type_arguments() {
    let source = r#"
        function entry(): i32
            return identity<i32>(42)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let waluau_ast::Stmt::Return(waluau_ast::Expr::Call {
        type_args, args, ..
    }) = &program.functions[0].body[0]
    else {
        panic!("expected return of generic call");
    };
    assert_eq!(type_args, &vec![Type::Numeric(NumericType::I32)]);
    assert_eq!(args.len(), 1);
}

#[test]
fn parses_generic_type_annotation() {
    let source = r#"
        type Array<T> = {T}

        function entry(): i32
            local xs: Array<i32> = {}
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    assert!(matches!(
        &program.functions[0].body[0],
        waluau_ast::Stmt::Let {
            ty: Some(Type::Named { name, type_args }),
            ..
        } if name == "Array" && type_args == &vec![Type::Numeric(NumericType::I32)]
    ));
}

#[test]
fn allows_less_than_comparison_not_confused_with_generics() {
    let source = r#"
        function entry(x: i32, y: i32): bool
            return x < y
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    assert_eq!(program.functions.len(), 1);
}

#[test]
fn allows_chained_comparisons_not_confused_with_generics() {
    let source = r#"
        function entry(x: i32, y: i32, z: i32): bool
            local a: bool = x < y
            local b: bool = y > z
            return a and b
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    assert_eq!(program.functions.len(), 1);
}

#[test]
fn parses_less_equal_and_greater_equal_comparisons() {
    let source = r#"
        function cmp(a: i32, b: i32): bool
            local le: bool = a <= b
            local ge: bool = a >= b
            return le and ge
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    let Stmt::Let { value, .. } = &function.body[0] else {
        panic!("expected local declaration");
    };
    let waluau_ast::Expr::Binary { op, .. } = value else {
        panic!("expected binary expression");
    };
    assert_eq!(*op, BinaryOp::LessEq);
    let Stmt::Let { value, .. } = &function.body[1] else {
        panic!("expected local declaration");
    };
    let waluau_ast::Expr::Binary { op, .. } = value else {
        panic!("expected binary expression");
    };
    assert_eq!(*op, BinaryOp::GreaterEq);
}

// `local x: Foo<T>=v` greedy-lexes `>=`; the parser must split the token back
// into the `>` closing the type arguments and the `=` starting the initializer.
#[test]
fn splits_greater_equal_closing_type_arguments() {
    let source = r#"
        type Box<T> = { value: T }
        function make(): i32
            local b: Box<i32>= { value = 3 }
            return b.value
        end
    "#;

    parse(source).expect("parse should succeed");
}

#[test]
fn skips_semicolon_statement_separators() {
    let source = r#"
        function sum(): i32
            local a: i32 = 1; local b: i32 = 2;
            ;
            return a + b
        end
        local total: i32 = sum();
    "#;

    let program = parse(source).expect("parse should succeed");
    assert_eq!(program.functions[0].body.len(), 3);
    assert_eq!(program.top_level.len(), 1);
}
