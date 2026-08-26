use super::{parse, parse_with_path};
use waluau_ast::{
    AssignOp, BinaryOp, Expr, FunctionName, NumberLiteral, NumberLiteralUnion, NumberUnionMember,
    NumericType, Rebindability, Stmt, Type, UnaryOp,
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
fn rejects_reserved_linker_identifiers() {
    let error = parse("local value: __waluau_m0_Hidden = 1\n")
        .expect_err("source must not address linker-private declarations");
    assert!(
        error
            .to_string()
            .contains("identifier '__waluau_m0_Hidden' uses a reserved linker prefix"),
        "{error}"
    );
    parse("function __waluau_main(): unit end")
        .expect("the runtime entry name must not match a module prefix");
}

#[test]
fn parses_nominal_enum_values_and_exhaustive_match() {
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
    let program = parse(source).expect("parse should succeed");
    assert_eq!(program.type_declarations[0].name, "Direction");
    assert_eq!(
        program.type_declarations[0].ty,
        Type::Numeric(NumericType::I32)
    );
    let Stmt::Match { enum_ty, arms, .. } = &program.functions[0].body[0] else {
        panic!("expected match statement")
    };
    assert_eq!(
        enum_ty,
        &Type::Named {
            name: "Direction".into(),
            type_args: vec![],
        }
    );
    assert_eq!(
        arms.iter()
            .map(|arm| (arm.variant.as_str(), arm.ordinal))
            .collect::<Vec<_>>(),
        vec![("north", 0), ("east", 1), ("south", 2)]
    );
}

#[test]
fn rejects_non_exhaustive_and_duplicate_enum_matches() {
    let missing = parse(
        "enum Direction { north, south }\nmatch Direction.north do\ncase Direction.north then\nend\n",
    )
    .expect_err("match should be non-exhaustive");
    assert!(
        missing
            .to_string()
            .contains("non-exhaustive match for enum 'Direction'; missing: Direction.south")
    );

    let duplicate = parse(
        "enum Direction { north, south }\nmatch Direction.north do\ncase Direction.north then\ncase Direction.north then\ncase Direction.south then\nend\n",
    )
    .expect_err("duplicate case should fail");
    assert!(
        duplicate
            .to_string()
            .contains("duplicate/unreachable match case 'Direction.north'")
    );
}

#[test]
fn parses_string_literal_union_type_declaration() {
    let program = parse("type CardColor = \"red\" | \"black\"\n").expect("parse should succeed");
    assert_eq!(program.type_declarations.len(), 1);
    assert_eq!(
        program.type_declarations[0].ty,
        Type::StringLiteralUnion(vec!["red".to_string(), "black".to_string()])
    );
}

#[test]
fn parses_number_literal_union_type_declarations() {
    let program = parse("type Volume = 0 | 1 | 2\n").expect("parse should succeed");
    assert_eq!(
        program.type_declarations[0].ty,
        Type::NumberLiteralUnion(NumberLiteralUnion {
            numeric: NumericType::I32,
            members: vec![
                NumberUnionMember::Int(0),
                NumberUnionMember::Int(1),
                NumberUnionMember::Int(2),
            ],
        })
    );

    let program = parse("type Direction = -1 | 1\n").expect("parse should succeed");
    assert_eq!(
        program.type_declarations[0].ty,
        Type::NumberLiteralUnion(NumberLiteralUnion {
            numeric: NumericType::I32,
            members: vec![NumberUnionMember::Int(-1), NumberUnionMember::Int(1)],
        })
    );

    // A member outside i32 widens the whole union to i64.
    let program = parse("type BigId = 1 | 5000000000\n").expect("parse should succeed");
    assert_eq!(
        program.type_declarations[0].ty,
        Type::NumberLiteralUnion(NumberLiteralUnion {
            numeric: NumericType::I64,
            members: vec![
                NumberUnionMember::Int(1),
                NumberUnionMember::Int(5_000_000_000),
            ],
        })
    );

    let program = parse("type Speed = 0.5 | 2.0\n").expect("parse should succeed");
    assert_eq!(
        program.type_declarations[0].ty,
        Type::NumberLiteralUnion(NumberLiteralUnion {
            numeric: NumericType::F64,
            members: vec![NumberUnionMember::float(0.5), NumberUnionMember::float(2.0),],
        })
    );
}

#[test]
fn rejects_invalid_literal_union_declarations() {
    let mixed_kind = parse("type Bad = \"red\" | 1\n").expect_err("mixed union should fail");
    assert!(
        mixed_kind
            .to_string()
            .contains("string literal union member must be a string literal, got 1")
    );

    let mixed_numeric = parse("type Bad = 1 | 2.5\n").expect_err("mixed numerics should fail");
    assert!(
        mixed_numeric.to_string().contains(
            "number literal union members must all be integers or all be floats, not a mix"
        )
    );

    let duplicate = parse("type Bad = \"red\" | \"red\"\n").expect_err("duplicate should fail");
    assert!(
        duplicate
            .to_string()
            .contains("duplicate string literal union member \"red\"")
    );

    let duplicate_number = parse("type Bad = 1 | 0x1\n").expect_err("duplicate should fail");
    assert!(
        duplicate_number
            .to_string()
            .contains("duplicate number literal union member 1")
    );

    let tagged_mix =
        parse("type Bad = Blue(f64) | \"red\"\n").expect_err("tagged/literal mix should fail");
    assert!(
        tagged_mix
            .to_string()
            .contains("tagged union member must be a tagged variant")
    );
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
fn parses_module_opaque_type_declarations() {
    let program = parse_with_path("opaque type State = { value: i32 }", "/game.walu")
        .expect("opaque type declaration should parse");
    let declaration = &program.type_declarations[0];
    assert_eq!(declaration.name, "State");
    assert!(declaration.module_opaque);
    assert_eq!(declaration.file_path, "/game.walu");
}

#[test]
fn parses_exported_type_and_enum_declarations() {
    let program = parse_with_path(
        r#"
            export type Pair<T> = { first: T, second: T }
            export opaque type Token = i32
            export enum Direction { north, south }
            type Private = bool
        "#,
        "/types.walu",
    )
    .expect("exported declarations should parse");

    assert!(program.type_declarations[0].exported);
    assert_eq!(program.type_declarations[0].type_params, ["T"]);
    assert!(program.type_declarations[1].exported);
    assert!(program.type_declarations[1].module_opaque);
    assert_eq!(
        program.type_declarations[2].enum_variants.as_deref(),
        Some(["north".to_string(), "south".to_string()].as_slice())
    );
    assert!(!program.type_declarations[3].exported);
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
fn parses_declared_host_function_with_dotted_namespace_name() {
    let source = r#"
        declare function math.abs(x: f32): f32
        declare function math.abs(x: f64): f64
    "#;

    let program = parse(source).expect("parse should succeed");
    assert_eq!(program.declared_imports.len(), 2);
    for declared in &program.declared_imports {
        // Unlike `Iface:method` receiver sugar, a dotted name adds no
        // implicit self parameter.
        assert_eq!(declared.name, "math.abs");
        assert_eq!(declared.host_name, "math.abs");
        assert_eq!(declared.params.len(), 1);
    }
    assert_eq!(
        program.declared_imports[0].params[0].ty,
        Type::Numeric(waluau_ast::NumericType::F32)
    );
    assert_eq!(
        program.declared_imports[1].params[0].ty,
        Type::Numeric(waluau_ast::NumericType::F64)
    );
}

#[test]
fn parses_namespace_member_type_references() {
    let source = r#"
        local game = require("./game")

        type State = game.State

        function score(state: game.State): i32
            return 0
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    assert_eq!(program.type_declarations.len(), 1);
    assert_eq!(
        program.type_declarations[0].ty,
        Type::Named {
            name: "game.State".into(),
            type_args: vec![],
        }
    );
    assert_eq!(
        program.functions[0].params[0].ty,
        Type::Named {
            name: "game.State".into(),
            type_args: vec![],
        }
    );
}

#[test]
fn parses_declared_namespace_constant() {
    let source = r#"
        declare const math.pi: f64 = 3.141592653589793
    "#;

    let program = parse(source).expect("parse should succeed");
    assert_eq!(program.declared_constants.len(), 1);
    let constant = &program.declared_constants[0];
    assert_eq!(constant.name, "math.pi");
    assert_eq!(constant.ty, Type::Numeric(waluau_ast::NumericType::F64));
    assert_eq!(constant.value.raw, "3.141592653589793");
}

#[test]
fn rejects_declared_constant_with_non_numeric_type() {
    let source = r#"
        declare const math.name: string = 1
    "#;

    let error = parse(source).expect_err("parse should fail");
    assert!(
        error
            .to_string()
            .contains("declared constant 'math.name' must have a numeric type")
    );
}

#[test]
fn rejects_declared_constant_without_number_literal() {
    let source = r#"
        declare const math.pi: f64 = true
    "#;

    let error = parse(source).expect_err("parse should fail");
    assert!(
        error
            .to_string()
            .contains("declared constant 'math.pi' must be initialized with a number literal")
    );
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
fn parses_grouped_nullable_function_types() {
    let source = r#"
        type Event = extern

        function register(
            callback: ((Event) -> unit)?,
            higher_order: (((Event) -> unit) -> unit)?
        ): unit
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let event_callback = Type::Function {
        params: vec![Type::Named {
            name: "Event".into(),
            type_args: vec![],
        }],
        return_type: Box::new(Type::Unit),
        has_self: false,
    };
    assert_eq!(
        program.functions[0].params[0].ty,
        Type::Nullable(Box::new(event_callback.clone()))
    );
    assert_eq!(
        program.functions[0].params[1].ty,
        Type::Nullable(Box::new(Type::Function {
            params: vec![event_callback],
            return_type: Box::new(Type::Unit),
            has_self: false,
        }))
    );
    assert_eq!(
        program.functions[0].params[0].ty.to_string(),
        "((Event) -> unit)?"
    );
}

#[test]
fn rejects_grouped_multi_type_outside_return_position() {
    let error = parse("function invalid(value: (i32, string)?): unit end")
        .expect_err("grouping more than one type should fail");
    assert!(
        error
            .to_string()
            .starts_with("parenthesized type grouping must contain exactly one type"),
        "unexpected diagnostic: {error}"
    );
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
            has_self: false,
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
            has_self: false,
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
fn accepts_fn_and_let_as_identifiers() {
    // `fn` and `let` are not keywords in Luau; both are valid local names.
    let source = r#"
        function entry(x: i32): i32
            local fn = function(y: i32): i32 return y end
            local let: i32 = fn(x)
            return let
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    assert_eq!(program.functions.len(), 1);
}

#[test]
fn parses_local_function_as_named_function_expression_let() {
    let source = r#"
        function entry(x: i32): i32
            local function double(y: i32): i32
                return y * 2
            end
            return double(x)
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let Stmt::Let { name, value, .. } = &program.functions[0].body[0] else {
        panic!("expected local function to desugar to a let statement");
    };
    assert_eq!(name, "double");
    let waluau_ast::Expr::Function(function) = value else {
        panic!("expected let value to be a function expression");
    };
    // The function expression carries its own name so its body can recurse.
    assert_eq!(function.name.as_deref(), Some("double"));
}

#[test]
fn parses_standalone_do_block_as_scoped_body() {
    let source = r#"
        function entry(): i32
            local x: i32 = 1
            do
                local y: i32 = 2
                x = y
            end
            return x
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let Stmt::If {
        condition,
        then_body,
        else_body,
    } = &program.functions[0].body[1]
    else {
        panic!("expected do block to desugar to an always-true if");
    };
    assert!(matches!(condition, waluau_ast::Expr::Bool(true, _)));
    assert_eq!(then_body.len(), 2);
    assert!(else_body.is_empty());
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
fn parses_array_literal_with_trailing_comma() {
    let source = r#"
        function scores(): i32
            local values: {i32} = {100, 250, 300,}
            return values[0]
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    assert!(matches!(
        &program.functions[0].body[0],
        Stmt::Let {
            value: waluau_ast::Expr::ArrayLiteral { elements, .. },
            ..
        } if elements.len() == 3
    ));
}

#[test]
fn rejects_array_literal_with_only_a_comma() {
    let source = r#"
        function bad(): i32
            local values: {i32} = {,}
            return 0
        end
    "#;

    let error = parse(source).expect_err("parse should fail");
    assert!(error.to_string().contains("expected expression"));
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
fn parses_not_modifier_chain_as_field_access() {
    let source = r#"
        function main(obj: { value: i32 }): unit
            obj:check():not:verify(1)
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    assert!(matches!(
        &program.functions[0].body[0],
        Stmt::Expr(waluau_ast::Expr::MethodCall {
            receiver,
            name,
            args,
            ..
        }) if name == "verify"
            && args.len() == 1
            && matches!(
                receiver.as_ref(),
                waluau_ast::Expr::Field { base, name, .. }
                    if name == "not"
                        && matches!(base.as_ref(), waluau_ast::Expr::MethodCall { name, .. } if name == "check")
            )
    ));
}

#[test]
fn parses_declared_not_property() {
    let source = r#"
        type Expectation = extern
        declare property Expectation:not: Expectation
    "#;

    let program = parse(source).expect("parse should succeed");
    let names = program
        .declared_imports
        .iter()
        .map(|declared| declared.name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"Expectation.get/not"), "names: {names:?}");
    assert!(names.contains(&"Expectation.set/not"), "names: {names:?}");
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
            ty: Some(Type::Function { params, return_type, .. }),
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
fn parses_record_type_with_trailing_comma() {
    let source = r#"
        type Vec2d = {
            x: number,
            y: number,
        }

        function entry(): number
            local p: Vec2d = { x = 1, y = 2 }
            return p.x + p.y
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    assert!(matches!(
        &program.type_declarations[0].ty,
        Type::Record(fields) if fields.len() == 2
    ));
}

#[test]
fn parses_empty_record_type() {
    let source = r#"
        type Marker = {}
        type Spaced = { }

        function entry(): i32
            local m: Marker = {}
            local xs: {i32} = {}
            local p: { x: i32 } = { x = 1 }
            return #xs + p.x
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    assert!(matches!(
        &program.type_declarations[0].ty,
        Type::Record(fields) if fields.is_empty()
    ));
    assert!(matches!(
        &program.type_declarations[1].ty,
        Type::Record(fields) if fields.is_empty()
    ));
    // `{T}` stays an array type and `{ x: T }` stays a fielded record.
    assert!(matches!(
        &program.functions[0].body[1],
        Stmt::Let {
            ty: Some(Type::Array(_)),
            ..
        }
    ));
    assert!(matches!(
        &program.functions[0].body[2],
        Stmt::Let {
            ty: Some(Type::Record(fields)),
            ..
        } if fields.len() == 1
    ));
}

#[test]
fn rejects_record_type_with_double_trailing_comma() {
    let source = "type T = { x: number,, }\n";
    assert!(parse(source).is_err());
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
fn parses_untyped_const_declaration() {
    let source = r#"
        function entry(): i32
            const x = 5
            return x
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::Let {
            name,
            rebindability: Rebindability::Const,
            ty: None,
            ..
        } if name == "x"
    ));
}

#[test]
fn parses_multi_binding_const_declaration() {
    let source = r#"
        function entry(): i32
            const a, b = 1, 2
            const c: i32, d = 3, 4
            return a + b + c + d
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::LetMulti { bindings, values }
            if bindings.len() == 2
                && values.len() == 2
                && bindings.iter().all(|b| b.rebindability == Rebindability::Const)
                && bindings[0].name == "a"
                && bindings[1].name == "b"
    ));
    assert!(matches!(
        &function.body[1],
        waluau_ast::Stmt::LetMulti { bindings, .. }
            if bindings[0].ty == Some(Type::Numeric(NumericType::I32)) && bindings[1].ty.is_none()
    ));
}

#[test]
fn parses_const_function_declaration() {
    let source = r#"
        function entry(): i32
            const function double(x: i32): i32
                return x * 2
            end
            return double(21)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    assert!(matches!(
        &function.body[0],
        waluau_ast::Stmt::Let {
            name,
            rebindability: Rebindability::Const,
            ty: None,
            value: waluau_ast::Expr::Function(_),
            ..
        } if name == "double"
    ));
}

#[test]
fn rejects_const_declaration_without_initializer() {
    let source = r#"
        function entry(): i32
            const x
            return 0
        end
    "#;
    let error = parse(source).expect_err("parse should fail");
    assert!(
        error
            .to_string()
            .contains("const bindings must be initialized"),
        "unexpected message: {}",
        error
    );
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
fn rejects_non_string_require_argument_with_stable_source_diagnostic() {
    let source = "local add = require(module_name)";
    let start = source.find("module_name").expect("argument should exist");
    let error = parse_with_path(source, "main.walu").expect_err("non-string require should fail");

    assert_eq!(error.code(), Some("module/require-literal-path"));
    assert_eq!(
        error.to_string(),
        "require expects a string literal path, e.g. require(\"./module\")"
    );
    assert_eq!(
        error.span(),
        Some(waluau_ast::Span {
            start: start as u32,
            end: (start + "module_name".len()) as u32,
        })
    );
    assert_eq!(
        error.render(),
        format!(
            "main.walu:1:{}: require expects a string literal path, e.g. \
             require(\"./module\")",
            start + 1
        )
    );
}

#[test]
fn parse_with_path_populates_program_source_metadata() {
    let source = "function main(): unit\nend\n";
    let program = parse_with_path(source, "src/main.walu").expect("parse should succeed");

    assert_eq!(program.entry_file_path, "src/main.walu");
    assert_eq!(
        program.sources.get("src/main.walu").map(String::as_str),
        Some(source)
    );
    assert_eq!(program.sources.len(), 1);
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
fn parses_dot_named_function_declaration() {
    let source = r#"
        function State.new(cols: i32): i32
            return cols
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let function = &program.functions[0];
    // A dot-named function is a plain function under the dotted name; unlike
    // `:` method sugar there is no implicit self parameter.
    assert_eq!(function.name, FunctionName::Simple("State.new".to_string()));
    assert_eq!(function.params.len(), 1);
}

#[test]
fn enum_member_access_falls_back_to_declared_statics() {
    // `SpellKind.from` names no variant but a dot-named function later in
    // the file declares it, so the access stays a field access for later
    // stages instead of failing as a variant typo.
    let source = r#"
        enum SpellKind { Firebolt, FreezeRay }

        function lookup(value: i32): SpellKind?
            return SpellKind.from(value)
        end

        function SpellKind.from(value: i32): SpellKind?
            if value == 1 then return SpellKind.Firebolt end
            if value == 2 then return SpellKind.FreezeRay end
            return nil
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let Stmt::Return(Expr::Call { callee, .. }) = &program.functions[0].body[0] else {
        panic!(
            "lookup should return a call: {:?}",
            program.functions[0].body
        );
    };
    assert!(
        matches!(
            &**callee,
            Expr::Field { base, name, .. }
                if name == "from"
                    && matches!(&**base, Expr::Name(name, _, _) if name == "SpellKind")
        ),
        "the static access should stay a field access: {callee:?}"
    );
}

#[test]
fn rejects_unknown_enum_member_without_matching_static() {
    let source = r#"
        enum SpellKind { Firebolt, FreezeRay }

        function broken(): SpellKind
            return SpellKind.Firebot
        end

        function SpellKind.from(value: i32): SpellKind?
            return nil
        end
    "#;

    let error = parse(source).expect_err("parse should fail");
    assert!(
        error
            .to_string()
            .contains("unknown enum variant 'SpellKind.Firebot'")
    );
}

#[test]
fn rejects_method_on_dot_named_function() {
    let source = r#"
        function State.new:clone(): i32
            return 0
        end
    "#;

    let error = parse(source).expect_err("parse should fail");
    assert!(
        error
            .to_string()
            .contains("cannot declare a method on dot-named function 'State.new'")
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

#[test]
fn recovery_reports_every_error_with_structural_spans() {
    // Three independent syntax errors in three separate statements: recovery
    // must surface all of them, each with a byte span and resolved line.
    let source = r#"
        function first(): i32
            return 1 +
        end
        function second(): i32
            local value: = 2
            return value
        end
        function third(): i32
            return 3
        end
    "#;

    let outcome = crate::parse_with_recovery(source, "example.walu");
    assert!(
        outcome.diagnostics.len() >= 2,
        "expected multiple diagnostics, got {:?}",
        outcome.diagnostics
    );
    for diagnostic in &outcome.diagnostics {
        assert!(
            diagnostic.span().is_some(),
            "diagnostic missing span: {diagnostic}"
        );
        assert!(
            diagnostic.source_location().is_some(),
            "diagnostic missing line/column: {diagnostic}"
        );
        assert_eq!(diagnostic.file_path(), Some("example.walu"));
    }
    // The healthy function after the errors still parses.
    assert!(
        outcome
            .program
            .functions
            .iter()
            .any(|function| matches!(&function.name, waluau_ast::FunctionName::Simple(name) if name == "third")),
        "recovery should keep parsing later functions"
    );
}

#[test]
fn recovery_returns_program_and_no_diagnostics_for_valid_source() {
    let outcome = crate::parse_with_recovery("local x: i32 = 1\n", "ok.walu");
    assert!(outcome.diagnostics.is_empty());
    assert_eq!(outcome.program.top_level.len(), 1);
}

mod definitions {
    use crate::{DefinitionKind, parse_with_recovery};

    fn defs(source: &str) -> Vec<crate::DefinitionSite> {
        let outcome = parse_with_recovery(source, "defs.walu");
        assert!(
            outcome.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            outcome.diagnostics
        );
        outcome.definitions
    }

    fn find<'a>(definitions: &'a [crate::DefinitionSite], name: &str) -> &'a crate::DefinitionSite {
        definitions
            .iter()
            .find(|definition| definition.name == name)
            .unwrap_or_else(|| panic!("definition '{name}' should be recorded"))
    }

    #[test]
    fn local_definition_records_exact_name_span_and_statement_end_visibility() {
        let source = "local answer: i32 = 40 + 2\nprint(tostring(answer))\n";
        let definitions = defs(source);
        let answer = find(&definitions, "answer");
        assert_eq!(answer.kind, DefinitionKind::Local);
        let span = answer.name_span;
        assert_eq!(&source[span.start as usize..span.end as usize], "answer");
        // Visible only after the whole statement, not inside the initializer.
        assert_eq!(answer.visible_from, source.find('\n').unwrap() as u32);
        assert_eq!(answer.scope_end, u32::MAX);
        assert_eq!(
            answer.ty,
            Some(waluau_ast::Type::Numeric(waluau_ast::NumericType::I32))
        );
    }

    #[test]
    fn initializer_shadow_does_not_see_new_binding() {
        // `local x = x` — the initializer reference must resolve to an outer
        // x, i.e. the new binding's visibility starts after the initializer.
        let source = "local x = 1\nlocal x = x + 1\n";
        let definitions = defs(source);
        let both: Vec<_> = definitions.iter().filter(|d| d.name == "x").collect();
        assert_eq!(both.len(), 2);
        let initializer_ref = source.rfind("x + 1").unwrap() as u32;
        assert!(both[0].visible_from <= initializer_ref);
        assert!(both[1].visible_from > initializer_ref);
    }

    #[test]
    fn function_definitions_are_file_visible_with_signature_detail() {
        let source = "function add(a: i32, b: i32): i32\n    return a + b\nend\n";
        let definitions = defs(source);
        let add = find(&definitions, "add");
        assert_eq!(add.kind, DefinitionKind::Function);
        assert_eq!(add.visible_from, 0);
        assert_eq!(add.scope_end, u32::MAX);
        assert_eq!(
            add.detail.as_deref(),
            Some("function add(a: i32, b: i32): i32")
        );

        let a = find(&definitions, "a");
        assert_eq!(a.kind, DefinitionKind::Param);
        // Parameters go out of scope at the function's `end`.
        assert!(a.scope_end < source.len() as u32 + 1);
        assert!(a.scope_end >= source.rfind("end").unwrap() as u32);
    }

    #[test]
    fn loop_variables_are_scoped_to_the_body_and_hidden_from_the_range() {
        let source =
            "local i = 100\nfor i = 1, 10 do\n    print(tostring(i))\nend\nprint(tostring(i))\n";
        let definitions = defs(source);
        let loop_var = definitions
            .iter()
            .find(|d| d.name == "i" && d.kind == DefinitionKind::LoopVar)
            .expect("loop var should be recorded");
        let range_pos = source.find("1, 10").unwrap() as u32;
        assert!(loop_var.visible_from > range_pos);
        let trailing_print = source.rfind("print").unwrap() as u32;
        assert!(loop_var.scope_end < trailing_print);
    }

    #[test]
    fn local_function_is_visible_inside_its_own_body() {
        let source = "local function fact(n: i32): i32\n    if n <= 1 then\n        return 1\n    end\n    return n * fact(n - 1)\nend\n";
        let definitions = defs(source);
        let fact = find(&definitions, "fact");
        assert_eq!(fact.kind, DefinitionKind::Function);
        let recursive_call = source.rfind("fact(n - 1)").unwrap() as u32;
        assert!(fact.visible_from < recursive_call);
    }

    #[test]
    fn require_locals_carry_the_module_path() {
        let source = "local m = require(\"./lib\")\n";
        let definitions = defs(source);
        let module = find(&definitions, "m");
        assert_eq!(module.require_path.as_deref(), Some("./lib"));
    }

    #[test]
    fn declares_and_type_declarations_are_recorded() {
        let source = "declare function math.abs(x: f64): f64\ndeclare const math.pi: f64 = 3.141592653589793\ntype Pair = {i32}\n";
        let definitions = defs(source);
        let abs = find(&definitions, "math.abs");
        assert_eq!(abs.kind, DefinitionKind::DeclaredFunction);
        assert_eq!(
            abs.detail.as_deref(),
            Some("function math.abs(x: f64): f64")
        );
        let pi = find(&definitions, "math.pi");
        assert_eq!(pi.kind, DefinitionKind::DeclaredConstant);
        let pair = find(&definitions, "Pair");
        assert_eq!(pair.kind, DefinitionKind::TypeName);
    }

    #[test]
    fn sibling_scopes_do_not_leak() {
        let source = "do\n    local inner = 1\nend\nlocal outer = 2\n";
        let definitions = defs(source);
        let inner = find(&definitions, "inner");
        let outer_pos = source.find("local outer").unwrap() as u32;
        assert!(inner.scope_end < outer_pos);
    }
}

#[test]
fn definitions_record_initializer_hints_and_function_signature_types() {
    use crate::{DefinitionKind, InitializerHint, parse_with_recovery};
    let source = "function new(): i32\n    return 1\nend\nlocal a = new()\nlocal b = game.new()\nlocal c = state.middle[0]\nlocal d = deck[0]\nlocal e = s:upper()\n";
    let outcome = parse_with_recovery(source, "hints.walu");
    let find = |name: &str| {
        outcome
            .definitions
            .iter()
            .find(|definition| definition.name == name)
            .unwrap_or_else(|| panic!("definition '{name}' should be recorded"))
    };
    let new_def = find("new");
    assert_eq!(new_def.kind, DefinitionKind::Function);
    assert!(
        matches!(&new_def.ty, Some(waluau_ast::Type::Function { .. })),
        "{:?}",
        new_def.ty
    );
    assert_eq!(
        find("a").initializer,
        Some(InitializerHint::Call {
            callee: "new".to_string()
        })
    );
    assert_eq!(
        find("b").initializer,
        Some(InitializerHint::Call {
            callee: "game.new".to_string()
        })
    );
    assert_eq!(
        find("c").initializer,
        Some(InitializerHint::Field {
            base: "state".to_string(),
            field: "middle".to_string(),
            indexed: true
        })
    );
    assert_eq!(
        find("d").initializer,
        Some(InitializerHint::Index {
            base: "deck".to_string()
        })
    );
    assert_eq!(
        find("e").initializer,
        Some(InitializerHint::MethodCall {
            receiver: "s".to_string(),
            method: "upper".to_string()
        })
    );
}

#[test]
fn parses_named_parameters_in_function_types() {
    // Parameter names are documentation only and are dropped at parse time:
    // named and unnamed spellings produce identical types.
    let named = parse("function apply(op: (a: i32, b: i32) -> i32): unit end")
        .expect("named parameters should parse");
    let unnamed = parse("function apply(op: (i32, i32) -> i32): unit end")
        .expect("unnamed parameters should parse");
    assert_eq!(
        named.functions[0].params[0].ty,
        unnamed.functions[0].params[0].ty
    );
    assert_eq!(
        named.functions[0].params[0].ty,
        Type::Function {
            params: vec![
                Type::Numeric(NumericType::I32),
                Type::Numeric(NumericType::I32),
            ],
            return_type: Box::new(Type::Numeric(NumericType::I32)),
            has_self: false,
        }
    );

    // Names may cover only some parameters.
    let mixed = parse("function apply(op: (base: string, i32) -> string): unit end")
        .expect("mixed named/positional parameters should parse");
    assert_eq!(
        mixed.functions[0].params[0].ty,
        Type::Function {
            params: vec![Type::String, Type::Numeric(NumericType::I32)],
            return_type: Box::new(Type::String),
            has_self: false,
        }
    );
}

#[test]
fn parses_self_receiver_in_record_field_function_type() {
    let program = parse("type Op = { exec: (self, a: i32, b: i32) -> i32 }")
        .expect("a self receiver in a record field's function type should parse");
    let expected_field = Type::Function {
        params: vec![
            Type::Numeric(NumericType::I32),
            Type::Numeric(NumericType::I32),
        ],
        return_type: Box::new(Type::Numeric(NumericType::I32)),
        has_self: true,
    };
    assert_eq!(
        program.type_declarations[0].ty,
        Type::Record([("exec".to_string(), expected_field)].into_iter().collect())
    );

    // `self` alone is a zero-parameter method type.
    let program =
        parse("type Ticker = { tick: (self) -> unit }").expect("a lone self receiver should parse");
    assert_eq!(
        program.type_declarations[0].ty,
        Type::Record(
            [(
                "tick".to_string(),
                Type::Function {
                    params: Vec::new(),
                    return_type: Box::new(Type::Unit),
                    has_self: true,
                }
            )]
            .into_iter()
            .collect()
        )
    );
}

#[test]
fn self_receiver_participates_in_type_identity() {
    let with_self = parse("type Op = { exec: (self, a: i32) -> i32 }").expect("parses");
    let without_self = parse("type Op = { exec: (a: i32) -> i32 }").expect("parses");
    assert_ne!(
        with_self.type_declarations[0].ty,
        without_self.type_declarations[0].ty
    );
}

#[test]
fn rejects_self_outside_record_field_function_types() {
    let error = parse("type F = (self) -> i32")
        .expect_err("a self receiver in a plain type alias must fail");
    assert!(
        error.to_string().contains(
            "'self' is only allowed in a function type used directly as a record field type"
        ),
        "unexpected diagnostic: {error}"
    );

    let error = parse("function f(callback: (self) -> i32): unit end")
        .expect_err("a self receiver in a parameter annotation must fail");
    assert!(
        error.to_string().contains("'self' is only allowed"),
        "unexpected diagnostic: {error}"
    );

    // Nested function types inside a record field are not the field's own
    // method type, so the receiver permission does not reach them.
    let error = parse("type Op = { exec: ((self) -> i32) -> i32 }")
        .expect_err("a self receiver in a nested function type must fail");
    assert!(
        error.to_string().contains("'self' is only allowed"),
        "unexpected diagnostic: {error}"
    );
}

#[test]
fn rejects_self_after_the_first_parameter() {
    let error = parse("type Op = { exec: (a: i32, self) -> i32 }")
        .expect_err("self after the first parameter must fail");
    assert!(
        error
            .to_string()
            .contains("'self' must be the first parameter in a function type"),
        "unexpected diagnostic: {error}"
    );
}

#[test]
fn parses_conformance_declarations() {
    let program = parse("type Add = Op & {}").expect("an empty conformance record should parse");
    let declaration = &program.type_declarations[0];
    assert_eq!(declaration.conforms, vec!["Op".to_string()]);
    assert_eq!(
        declaration.ty,
        Type::Record(std::collections::BTreeMap::new())
    );

    let program =
        parse("type Add = Op & { count: i32 }").expect("a conformance record with fields parses");
    let declaration = &program.type_declarations[0];
    assert_eq!(declaration.conforms, vec!["Op".to_string()]);
    assert_eq!(
        declaration.ty,
        Type::Record(
            [("count".to_string(), Type::Numeric(NumericType::I32))]
                .into_iter()
                .collect()
        )
    );

    // A dotted interface name references an imported module's type alias.
    let program = parse("type Add = ops.Op & {}").expect("a dotted interface name parses");
    assert_eq!(
        program.type_declarations[0].conforms,
        vec!["ops.Op".to_string()]
    );

    // Plain declarations conform to nothing.
    let program = parse("type Add = { count: i32 }").expect("parses");
    assert!(program.type_declarations[0].conforms.is_empty());
}

#[test]
fn rejects_ampersand_outside_type_declarations() {
    for source in [
        "function f(x: A & B): unit end",
        "function f(): unit local x: A & B = y end",
        "type Add = { op: Op & {} }",
        "function f(x: (A & B) -> i32): unit end",
    ] {
        let error = parse(source).expect_err("'&' outside a type declaration RHS must fail");
        assert!(
            error
                .to_string()
                .contains("intersection types are not supported"),
            "unexpected diagnostic for {source}: {error}"
        );
    }
}

#[test]
fn rejects_malformed_conformance_declarations() {
    let error = parse("type Add = {} & Op").expect_err("a record interface position must fail");
    assert!(
        error
            .to_string()
            .contains("left-hand side of '&' in a type declaration must be a named interface"),
        "unexpected diagnostic: {error}"
    );

    let error =
        parse("type Add = Op & Sub & {}").expect_err("multiple interfaces must fail for now");
    assert!(
        error
            .to_string()
            .contains("intersection types are not supported"),
        "unexpected diagnostic: {error}"
    );

    let error = parse("type Add = Op & i32").expect_err("a non-record shape must fail");
    assert!(
        error
            .to_string()
            .contains("right-hand side of '&' in a type declaration must be a record type"),
        "unexpected diagnostic: {error}"
    );

    let error = parse("type Add = Op<i32> & {}").expect_err("interface type arguments must fail");
    assert!(
        error
            .to_string()
            .contains("cannot take type arguments in a conformance declaration"),
        "unexpected diagnostic: {error}"
    );

    let error =
        parse("type Add<T> = Op & {}").expect_err("generic conformance declarations must fail");
    assert!(
        error
            .to_string()
            .contains("generic type 'Add' cannot declare interface conformance"),
        "unexpected diagnostic: {error}"
    );
}

#[test]
fn rejects_self_and_parameter_names_in_type_grouping() {
    let error =
        parse("type Op = { exec: (self) }").expect_err("a self receiver without '->' must fail");
    assert!(
        error.to_string().contains("expected '->'"),
        "unexpected diagnostic: {error}"
    );

    let error = parse("function f(x: (a: i32)): unit end")
        .expect_err("parameter names in a parenthesized grouping must fail");
    assert!(
        error
            .to_string()
            .contains("parameter names are only allowed in function types"),
        "unexpected diagnostic: {error}"
    );
}

#[test]
fn desugars_enum_pairs_loop_into_variant_name_array() {
    let source = r#"
        enum SpellKind { Firebolt, FreezeRay }

        function catalog(): unit
            for name, kind in pairs(SpellKind) do
                local x = name
            end
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let Stmt::ForIn {
        names,
        iterators,
        body,
        ..
    } = &program.functions[0].body[0]
    else {
        panic!("pairs over an enum should parse as a for-in loop");
    };
    let [iterator] = iterators.as_slice() else {
        panic!("the enum pairs desugar produces a single iterator");
    };
    assert_eq!(
        names,
        &vec![
            waluau_ast::ENUM_PAIRS_ORDINAL.to_string(),
            "name".to_string()
        ]
    );
    let Expr::ArrayLiteral { elements, .. } = iterator else {
        panic!("the iterator should be the variant-name array, got {iterator:?}");
    };
    let variants: Vec<_> = elements
        .iter()
        .map(|element| match element {
            Expr::String(value, _) => value.as_str(),
            other => panic!("variant array should hold strings, got {other:?}"),
        })
        .collect();
    assert_eq!(variants, ["Firebolt", "FreezeRay"]);
    let Some(Stmt::Let {
        name,
        rebindability,
        value,
        ..
    }) = body.first()
    else {
        panic!("the loop body should open with the enum-value binding");
    };
    assert_eq!(name, "kind");
    assert_eq!(*rebindability, Rebindability::Const);
    let Expr::Cast { expr, ty, .. } = value else {
        panic!("the enum value should be a cast of the loop ordinal, got {value:?}");
    };
    assert!(
        matches!(&**expr, Expr::Name(ordinal, _, _) if ordinal == waluau_ast::ENUM_PAIRS_ORDINAL)
    );
    assert!(matches!(ty, Type::Named { name, .. } if name == "SpellKind"));
}

#[test]
fn desugars_name_only_enum_pairs_loop() {
    let source = r#"
        enum SpellKind { Firebolt, FreezeRay }

        function catalog(): unit
            for name in pairs(SpellKind) do
                local x = name
            end
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let Stmt::ForIn {
        names, iterators, ..
    } = &program.functions[0].body[0]
    else {
        panic!("pairs over an enum should parse as a for-in loop");
    };
    assert_eq!(names, &vec!["name".to_string()]);
    assert!(matches!(iterators.as_slice(), [Expr::ArrayLiteral { .. }]));
}

#[test]
fn rejects_enum_pairs_loop_with_three_variables() {
    let source = r#"
        enum SpellKind { Firebolt, FreezeRay }

        function catalog(): unit
            for a, b, c in pairs(SpellKind) do
            end
        end
    "#;

    let error = parse(source).expect_err("three loop variables should be rejected");
    assert_eq!(
        error.to_string(),
        "pairs over enum 'SpellKind' yields a variant name and value; expected 1 or 2 loop variables, got 3"
    );
}

#[test]
fn pairs_over_non_enum_name_stays_a_plain_call() {
    let source = r#"
        function scan(): unit
            local scores = { alice = 3::i32 }
            for name, score in pairs(scores) do
                local x = name
            end
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let Stmt::ForIn { iterators, .. } = &program.functions[0].body[1] else {
        panic!("the loop should parse as a for-in");
    };
    assert!(
        matches!(iterators.as_slice(), [Expr::Call { .. }]),
        "a non-enum pairs iterator is left for the type checker, got {iterators:?}"
    );
}

#[test]
fn parses_iterator_expression_list() {
    let source = r#"
        function scan(): unit
            for k, v in iter, state, -1 do
            end
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let Stmt::ForIn {
        names, iterators, ..
    } = &program.functions[0].body[0]
    else {
        panic!("expected a for-in loop");
    };
    assert_eq!(names, &vec!["k".to_string(), "v".to_string()]);
    assert_eq!(iterators.len(), 3);
    assert!(matches!(&iterators[0], Expr::Name(name, _, _) if name == "iter"));
    assert!(matches!(&iterators[1], Expr::Name(name, _, _) if name == "state"));
}

#[test]
fn rejects_more_than_three_iterator_expressions() {
    let source = r#"
        function scan(): unit
            for k in a, b, c, d do
            end
        end
    "#;

    let error = parse(source).expect_err("four iterator expressions should be rejected");
    assert!(
        error
            .to_string()
            .contains("for-in takes at most 3 iterator expressions"),
        "{error}"
    );
}
