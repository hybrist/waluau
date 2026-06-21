use waluau_ast::{BinaryOp, Expr, FunctionName, NumericType, Stmt, Type, UnaryOp};
use waluau_diagnostics::DiagnosticCategory;
use waluau_parser::parse;

#[test]
fn type_checks_valid_program() {
    let source = r#"
        function add(x: i32, y: i32): i32
            return x + y
        end

        function entry(flag: bool, x: i32, y: i32): i32
            local z: i32 = add(x, y)
            if flag then
                z = z + 1
            else
                z = z + 2
            end
            return z
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_non_bool_condition() {
    let source = r#"
        function entry(x: i32): i32
            if x then
                return x
            end
            return x
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "if condition must be bool");
}

#[test]
fn accepts_numeric_alias_and_scalar_types() {
    let source = r#"
        function widen(x: number, y: f32, z: u64, w: i64): f64
            local sum: f64 = x + 1
            if z > 0 then
                return sum
            end
            if w > 0 then
                return x + 2
            end
            return x + 3
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_exponentiation() {
    let source = r#"
        function fpow(base: f64, exp: f64): f64
            return base ^ exp
        end

        function ipow(base: i32, exp: i32): i32
            return base ^ exp
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_mixed_numeric_operands_in_exponentiation() {
    let source = r#"
        function entry(x: i64, y: f64): i64
            return x ^ y
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "operation requires compatible numeric operands"
    );
}

#[test]
fn rejects_mixed_numeric_operands() {
    let source = r#"
        function entry(x: i64, y: f64): i64
            return x + y
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "operation requires compatible numeric operands"
    );
}

#[test]
fn infers_local_types_from_literals() {
    let source = r#"
        function entry(flag: bool): f64
            local x = 41
            local y = x + 1
            if flag then
                y = y + 1
            end
            return y
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_incompatible_reassignment_of_inferred_local() {
    let source = r#"
        function entry(): i32
            local x = 1
            x = true
            return x
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "assignment to 'x' expects f64, got bool");
}

#[test]
fn accepts_full_range_i64_and_u64_literals() {
    let source = r#"
        function entry(x: i64, y: u64): i64
            local a: i64 = x + 1
            local b: u64 = 18446744073709551615
            if y > 0 then
                return a
            end
            if b > 0 then
                return 9223372036854775807
            end
            return x + 2
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_out_of_range_u64_literals() {
    let source = r#"
        function entry(): u64
            return 18446744073709551616
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "numeric literal is out of range for u64");
}

#[test]
fn accepts_implicit_numeric_widening() {
    let source = r#"
        function widen(x: i32, y: f32, z: u32): f64
            local a: i64 = x
            local b: f64 = x + 1
            local c: f64 = y
            local d: i64 = z + 1
            if a < d then
                return b
            end
            return c
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn requires_explicit_cast_for_narrowing() {
    let source = r#"
        function narrow(x: i64): i32
            return x
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "cannot implicitly convert i64 to i32");
}

#[test]
fn accepts_explicit_numeric_casts() {
    let source = r#"
        function narrow(x: i64, y: f64): i32
            local a: i32 = x :: i32
            local b: i32 = y :: i32
            return a + b
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn accepts_cast_style_initialization_of_named_record_types() {
    let source = r#"
        type MyType = { pos: number }

        function entry(): number
            local t1: MyType = { pos = 20 }
            local t2 = { pos = 20 }::MyType
            return t1.pos + t2.pos
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn opaque_types_require_explicit_casts_to_their_representation() {
    let source = r#"
        type Meters = number

        function entry(): f64
            local len = 10::Meters
            local len_explicit: number = len::number
            return len_explicit
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn opaque_types_reject_implicit_conversion_to_their_representation() {
    let source = r#"
        type Meters = number

        function entry(): f64
            local len = 10::Meters
            local len_implicit: number = len
            return len_implicit
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "cannot implicitly convert Meters to f64");
}

#[test]
fn extern_type_aliases_are_nominal_and_lower_to_extern() {
    let source = r#"
        type Element = extern

        function identity(value: Element): Element
            return value
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");

    assert_eq!(typed.type_declarations[0].ty, Type::Extern);
    assert!(matches!(
        &typed.functions[0].params[0].ty,
        Type::Opaque { name, ty } if name == "Element" && **ty == Type::Extern
    ));
}

#[test]
fn tfjs_model_extern_types_are_nominally_distinct() {
    let source = r#"
        type Promise<T> = extern
        type Tensor = extern
        type GraphModel = extern
        type LayersModel = extern

        declare function load_graph_model(url: string): Promise<GraphModel>
        declare function load_layers_model(url: string): Promise<LayersModel>
        declare function graph_model_predict(model: GraphModel, input: Tensor): Tensor
        declare function layers_model_predict(model: LayersModel, input: Tensor): Tensor

        function graph(url: string): Promise<GraphModel>
            return load_graph_model(url)
        end

        function layers(url: string): Promise<LayersModel>
            return load_layers_model(url)
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");

    assert!(matches!(
        typed.functions[0].return_type.as_ref(),
        Some(Type::Opaque { name, ty }) if name == "Promise<GraphModel>" && ty.as_ref() == &Type::Extern
    ));
    assert!(matches!(
        typed.functions[1].return_type.as_ref(),
        Some(Type::Opaque { name, ty }) if name == "Promise<LayersModel>" && ty.as_ref() == &Type::Extern
    ));
}

#[test]
fn tfjs_model_extern_types_reject_cross_assignment() {
    let source = r#"
        type GraphModel = extern
        type LayersModel = extern

        function entry(model: LayersModel): GraphModel
            return model
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "cannot implicitly convert LayersModel to GraphModel"
    );
}

#[test]
fn resolves_declared_extern_operator_overloads() {
    let source = r#"
        type Tensor = extern
        declare function make_tensor(): Tensor
        declare function Tensor:__add(rhs: Tensor): Tensor
        declare function Tensor:__neg(): Tensor

        function add(): Tensor
            return make_tensor() + make_tensor()
        end

        function neg(): Tensor
            return -make_tensor()
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");
    let add_fn = typed
        .functions
        .iter()
        .find(|function| function.name.to_string() == "add")
        .expect("add function should exist");
    let Stmt::Return(Expr::Binary {
        op: BinaryOp::Add,
        resolved_name: Some(add_name),
        ..
    }) = &add_fn.body[0]
    else {
        panic!("expected resolved binary overload");
    };
    assert_eq!(add_name, "Tensor.__add");

    let neg_fn = typed
        .functions
        .iter()
        .find(|function| function.name.to_string() == "neg")
        .expect("neg function should exist");
    let Stmt::Return(Expr::Unary {
        op: UnaryOp::Neg,
        resolved_name: Some(neg_name),
        ..
    }) = &neg_fn.body[0]
    else {
        panic!("expected resolved unary overload");
    };
    assert_eq!(neg_name, "Tensor.__neg");
}

#[test]
fn rejects_missing_extern_operator_overload_with_clear_diagnostic() {
    let source = r#"
        type Tensor = extern
        declare function make_tensor(): Tensor

        function add(): Tensor
            return make_tensor() + make_tensor()
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "operator '+' is not defined for Tensor and Tensor"
    );
}

#[test]
fn nullable_extern_aliases_narrow_after_nil_check() {
    let source = r#"
        type Element = extern

        function take(value: Element): i32
            return 20
        end

        function score(value: Element?): i32
            if value ~= nil then
                return take(value)
            end
            return 10
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");

    assert!(matches!(
        &typed.functions[1].params[0].ty,
        Type::Nullable(inner)
            if matches!(inner.as_ref(), Type::Opaque { name, ty } if name == "Element" && ty.as_ref() == &Type::Extern)
    ));
}

#[test]
fn nullable_strings_narrow_after_nil_check() {
    let source = r#"
        function take(value: string): i32
            return 20
        end

        function score(value: string?): i32
            if value ~= nil then
                return take(value)
            end
            return 10
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");

    assert!(matches!(
        &typed.functions[1].params[0].ty,
        Type::Nullable(inner) if inner.as_ref() == &Type::String
    ));
}

#[test]
fn nullable_modifier_rejects_non_host_ref_types() {
    let source = r#"
        function entry(value: i32?): i32
            return 0
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "nullable modifier '?' is only supported on host reference types, got i32"
    );
}

#[test]
fn nil_comparison_rejects_non_nullable_extern_values() {
    let source = r#"
        type Element = extern

        function entry(value: Element): bool
            return value == nil
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "nil comparison requires a nullable extern operand"
    );
}

#[test]
fn distinct_extern_type_aliases_do_not_implicitly_convert() {
    let source = r#"
        type Element = extern
        type Node = extern

        function take_node(value: Node): Node
            return value
        end

        function entry(value: Element): Node
            return take_node(value)
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "cannot implicitly convert Element to Node"
    );
}

#[test]
fn extern_inheritance_allows_upcast() {
    let source = r#"
        type Node = extern
        type Element = extern extends Node
        type HTMLElement = extern extends Element
        type HTMLHeadingElement = extern extends HTMLElement

        function take_node(value: Node): i32
            return 1
        end

        function entry(value: HTMLHeadingElement): i32
            local element: Element = value
            return take_node(element)
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check_and_infer(&program).expect("type check should succeed");
}

#[test]
fn extern_inheritance_rejects_unguarded_downcast() {
    let source = r#"
        type Node = extern
        type Element = extern extends Node
        type HTMLElement = extern extends Element
        type HTMLHeadingElement = extern extends HTMLElement

        function entry(value: Element): HTMLHeadingElement
            local heading: HTMLHeadingElement = value
            return heading
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "cannot implicitly convert Element to HTMLHeadingElement"
    );
}

#[test]
fn if_cast_narrows_extern_binding_only_in_success_branch() {
    let source = r#"
        type Node = extern
        type Element = extern extends Node
        type HTMLElement = extern extends Element
        type HTMLHeadingElement = extern extends HTMLElement

        function take_heading(value: HTMLHeadingElement): i32
            return 1
        end

        function entry(value: Element): i32
            if HTMLHeadingElement(heading) = value then
                return take_heading(heading)
            else
                return 0
            end
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check_and_infer(&program).expect("type check should succeed");
}

#[test]
fn if_cast_binding_does_not_escape_success_branch() {
    let source = r#"
        type Node = extern
        type Element = extern extends Node
        type HTMLHeadingElement = extern extends Element

        function entry(value: Element): HTMLHeadingElement
            if HTMLHeadingElement(heading) = value then
                return heading
            end
            return heading
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "unknown name 'heading'");
}

#[test]
fn generic_type_declarations_resolve_transparently() {
    let source = r#"
        type Pair<A, B> = {first: A, second: B}

        function entry(value: Pair<i32, bool>): Pair<i32, bool>
            return value
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");
    let expected = Type::Record(
        [
            ("first".to_string(), Type::Numeric(NumericType::I32)),
            ("second".to_string(), Type::Bool),
        ]
        .into_iter()
        .collect(),
    );
    assert_eq!(typed.functions[0].params[0].ty, expected.clone());
    assert_eq!(typed.functions[0].return_type, Some(expected));
}

#[test]
fn generic_extern_type_declarations_instantiate_nominally() {
    let source = r#"
        type Response = extern
        type Promise<T> = extern

        function take_response(value: Promise<Response>): Promise<Response>
            return value
        end

        function take_string(value: Promise<string>): Promise<string>
            return value
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");

    assert!(matches!(
        &typed.functions[0].params[0].ty,
        Type::Opaque { name, ty } if name == "Promise<Response>" && ty.as_ref() == &Type::Extern
    ));
    assert!(matches!(
        &typed.functions[1].params[0].ty,
        Type::Opaque { name, ty } if name == "Promise<string>" && ty.as_ref() == &Type::Extern
    ));
    assert_ne!(
        typed.functions[0].params[0].ty,
        typed.functions[1].params[0].ty
    );
}

#[test]
fn generic_extern_type_declarations_reject_cross_specialization_assignment() {
    let source = r#"
        type Response = extern
        type Promise<T> = extern

        function take_response(value: Promise<Response>): Promise<Response>
            return value
        end

        function entry(value: Promise<string>): Promise<Response>
            return take_response(value)
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "cannot implicitly convert Promise<string> to Promise<Response>"
    );
}

#[test]
fn generic_extern_type_declarations_validate_arity() {
    let source = r#"
        type Promise<T> = extern

        function entry(value: Promise<i32, bool>): unit
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "type declaration 'Promise' expects 1 type argument, got 2"
    );
}

#[test]
fn promise_extern_api_declarations_type_check_with_nominal_specializations() {
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
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");

    assert!(matches!(
        typed.functions[0].return_type.as_ref(),
        Some(Type::Opaque { name, ty }) if name == "Promise<Response>" && ty.as_ref() == &Type::Extern
    ));
    assert!(matches!(
        typed.functions[1].return_type.as_ref(),
        Some(Type::Opaque { name, ty }) if name == "Promise<string>" && ty.as_ref() == &Type::Extern
    ));
}

#[test]
fn typed_promise_await_function_returns_resolved_type() {
    let source = r#"
        type Response = extern
        type Promise<T> = extern

        declare function fetch(url: string): Promise<Response>
        declare function make_text(): Promise<string>

        function request(): Response
            local res = promise.await(fetch("/test.json"))
            return res
        end

        function text(): string
            local body = promise.await(make_text())
            return body
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn typed_promise_await_method_returns_resolved_type() {
    let source = r#"
        type Response = extern
        type Promise<T> = extern

        declare function fetch(url: string): Promise<Response>
        declare function make_text(): Promise<string>

        function request(): Response
            local res = fetch("/test.json"):await()
            return res
        end

        function text(): string
            local body = make_text():await()
            return body
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn generic_type_declarations_reject_recursive_cycles() {
    let source = r#"
        type Loop<T> = Loop<T>

        function entry(value: Loop<i32>): i32
            return 0
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    assert!(
        error
            .to_string()
            .contains("cyclic type declaration detected")
    );
}

#[test]
fn mutually_recursive_type_aliases_are_supported() {
    let source = r#"
        type A = {b: B}
        type B = {a: A}

        function entry(a: A): B
            return a.b
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");

    // Verify that both types were resolved
    assert_eq!(typed.type_declarations.len(), 2);

    // Find the type declarations
    let type_a = typed
        .type_declarations
        .iter()
        .find(|d| d.name == "A")
        .expect("Type A should exist");
    let type_b = typed
        .type_declarations
        .iter()
        .find(|d| d.name == "B")
        .expect("Type B should exist");

    // Verify they are record types with the expected structure
    match &type_a.ty {
        Type::Record(fields) => {
            assert!(fields.contains_key("b"), "Type A should have field 'b'");
            match fields.get("b").unwrap() {
                Type::Opaque { name, .. } => {
                    assert_eq!(name, "B", "Field 'b' should reference type B");
                }
                other => panic!("Expected opaque reference to B, got {:?}", other),
            }
        }
        other => panic!("Expected record type for A, got {:?}", other),
    }

    match &type_b.ty {
        Type::Record(fields) => {
            assert!(fields.contains_key("a"), "Type B should have field 'a'");
            match fields.get("a").unwrap() {
                Type::Opaque { name, .. } => {
                    assert_eq!(name, "A", "Field 'a' should reference type A");
                }
                other => panic!("Expected opaque reference to A, got {:?}", other),
            }
        }
        other => panic!("Expected record type for B, got {:?}", other),
    }
}

#[test]
fn rejects_direct_self_referencing_type_aliases() {
    let source = r#"
        type A = A
        
        function entry(): i32
            return 0
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    assert!(
        error
            .to_string()
            .contains("cyclic type declaration detected"),
        "Expected cycle detection error, got: {}",
        error
    );
}

#[test]
fn accepts_unary_negation_not_and_elseif() {
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
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_non_call_expression_statements() {
    let source = r#"
        function entry(x: i32): i32
            x + 1
            return x
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "expression statements must be calls");
}

#[test]
fn method_call_expression_statements_use_call_type_checking() {
    let source = r#"
        function ping(self: { x: f64, y: f64 }): i32
            return 1
        end

        function entry(): i32
            local obj = { x = 1 }
            obj.ping = ping
            obj:ping()
            return 0
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "call expected {x: f64, y: f64}, got {ping: ({x: f64, y: f64}) -> i32, x: f64}"
    );
}

#[test]
fn type_checks_array_literals_indexing_and_length() {
    let source = r#"
        function score_count(): i32
            local scores: {number} = {100, 250, 300}
            local first: number = scores[0]
            scores[1] = first + 1
            return #scores
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_heterogeneous_array_literals() {
    let source = r#"
        function entry(): i32
            local xs: {i32} = {1, true}
            return #xs
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "array literal elements must share a common type"
    );
}

#[test]
fn rejects_empty_array_literals() {
    let source = r#"
        function entry(): i32
            local xs: {i32} = {}
            return #xs
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "empty array literal requires explicit element type"
    );
}

#[test]
fn treats_untyped_empty_braces_as_record_locals() {
    let source = r#"
        function entry(): i32
            local t = {}
            t.x = 1::i32
            return t.x
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check_and_infer(&program).expect("inference should succeed");
}

#[test]
fn rejects_length_on_non_array() {
    let source = r#"
        function entry(x: i32): i32
            return #x
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "# requires an array or bytes operand");
}

#[test]
fn rejects_incompatible_array_assignment() {
    let source = r#"
        function entry(): i32
            local xs: {i32} = {1, 2, 3}
            local ys: {i64} = {1, 2, 3}
            xs = ys
            return #xs
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "cannot implicitly convert {i64} to {i32}"
    );
}

#[test]
fn rejects_repeat_until_non_bool_condition() {
    let source = r#"
        function entry(x: i32): i32
            repeat
                x = x + 1
            until x
            return x
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "repeat-until condition must be bool");
}

#[test]
fn rejects_break_and_continue_outside_loops() {
    let source = r#"
        function entry(x: i32): i32
            break
            continue
            return x
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    let message = error.to_string();
    assert!(
        message.contains("break is only allowed inside loops")
            || message.contains("continue is only allowed inside loops")
    );
}

#[test]
fn type_checks_break_and_continue_in_loops() {
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
                break
            end
            repeat
                i += 1
                if i > len then
                    break
                end
            until false
            return acc
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_repeat_until_loop() {
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
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_numeric_for_loop() {
    let source = r#"
        function entry(limit: i32): i32
            local acc: i32 = 0
            for i = 0::i32, limit do
                acc += i
            end
            for j = limit, 0::i32, -2::i32 do
                acc += j
            end
            return acc
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_closure_capture() {
    let source = r#"
        function entry(x: i32): i32
            local make: (i32) -> (i32) -> i32 = function(offset: i32): (i32) -> i32
                return function(value: i32): i32
                    return x + offset + value
                end
            end
            local add5: (i32) -> i32 = make(5)
            return add5(7)
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_named_function_expression_recursion() {
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
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_compound_assignment_on_non_numeric_targets() {
    let source = r#"
        function entry(flag: bool, xs: {bool}): i32
            flag += true
            xs[0] += false
            return 0
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "compound assignment to 'flag' requires a numeric target"
    );
}

#[test]
fn rejects_rebinding_const_local() {
    let source = r#"
        function entry(x: i32): i32
            const y: i32 = x
            y = x + 1
            return y
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "cannot rebind const local 'y'");
}

#[test]
fn allows_rebinding_plain_local_named_const() {
    let source = r#"
        function entry(): i32
            local const: i32 = 1
            const = const + 1
            return const
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn allows_shadowing_const_with_inner_local() {
    let source = r#"
        function entry(flag: bool): i32
            const x: i32 = 1
            if flag then
                local x: i32 = 2
                return x
            end
            return x
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn allows_mutation_through_const_array_binding() {
    let source = r#"
        function entry(): i32
            const xs: {i32} = {1, 2, 3}
            xs[0] = 9
            return xs[0]
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_if_expression() {
    let source = r#"
        function entry(flag: bool, x: i32, y: i32): i32
            return if flag then x else y
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_if_expression_type_mismatch() {
    let source = r#"
        function entry(flag: bool, x: i32): i32
            return if flag then x else true
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "if expression branches must resolve to the same type"
    );
}

#[test]
fn rejects_unannotated_function_expression_returns_for_now() {
    let source = r#"
        function entry(): i32
            local add1 = function(x: i32)
                return x + 1
            end
            return add1(1)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "function return inference is only supported for named functions in this MVP"
    );
}

#[test]
fn rejects_typed_local_function_value_with_mismatched_annotation() {
    let source = r#"
        function entry(): i32
            local job: (i32) -> i32 = function(x: i32): bool
                return x > 0
            end
            return job(1)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "cannot implicitly convert (i32) -> bool to (i32) -> i32"
    );
}

#[test]
fn infers_top_level_function_return_type_from_single_return() {
    let source = r#"
        function inc(x: i32)
            return x + 1
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("inference should succeed");
    assert_eq!(
        typed.functions[0].return_type,
        Some(Type::Numeric(NumericType::I32))
    );
}

#[test]
fn infers_top_level_function_return_type_from_branches() {
    let source = r#"
        function choose(flag: bool)
            if flag then
                return 1 :: i32
            else
                return 2 :: i64
            end
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("inference should succeed");
    assert_eq!(
        typed.functions[0].return_type,
        Some(Type::Numeric(NumericType::I64))
    );
}

#[test]
fn rejects_incompatible_inferred_return_branches() {
    let source = r#"
        function bad(flag: bool)
            if flag then
                return 1
            end
            return true
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("inference should fail");
    assert_eq!(
        error.to_string(),
        "function return branches must resolve to the same type"
    );
}

#[test]
fn rejects_recursive_return_inference() {
    let source = r#"
        function fact(n: i32)
            if n == 0 then
                return 1
            end
            return n * fact(n - 1)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("inference should fail");
    assert_eq!(
        error.to_string(),
        "cannot infer return type for recursive or cyclic function 'fact'"
    );
}

#[test]
fn inference_is_deterministic_for_identical_ast_input() {
    let source = r#"
        function pick(flag: bool)
            local value = if flag then 1 else 2
            return value + 1
        end
    "#;
    let program = parse(source).expect("parse should succeed");

    let inferred_once = super::type_check_and_infer(&program).expect("first inference succeeds");
    let inferred_twice = super::type_check_and_infer(&program).expect("second inference succeeds");

    assert_eq!(inferred_once, inferred_twice);
}

#[test]
fn type_checks_multi_return_and_multi_assignment() {
    let source = r#"
        function pair(x: i32, y: bool): i32, bool
            return x, y
        end

        function entry(x: i32, y: bool): i32
            local a: i32, b: bool = pair(x, y)
            a, b = pair(a + 1, b)
            return a
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_multi_value_call_argument_expansion() {
    let source = r#"
        function pair(x: i32, y: i32): i32, i32
            return x, y
        end

        function sum2(a: i32, b: i32): i32
            return a + b
        end

        function entry(x: i32, y: i32): i32
            return sum2(pair(x, y))
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_table_literal_and_field_access() {
    let source = r#"
        function entry(): i32
            local t = { x = 41::i32 }
            return t.x
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_incremental_field_assignment_on_record_local() {
    let source = r#"
        function entry(): i32
            local t = { x = 10::i32 }
            t.y = 2::i32
            return t.x + t.y
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_method_declaration_with_implicit_self() {
    let source = r#"
        local point = { x = 41::i32 }

        function point:get_x(): i32
            return self.x
        end

        function entry(): i32
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_declared_host_method_on_extern_type() {
    let source = r#"
        type Element = extern
        declare function getElement(): Element
        declare function Element:value(delta: i32): i32

        assert(getElement():value(7::i32) == 49)
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_duplicate_declared_host_members_across_extern_types() {
    let source = r#"
        type Alpha = extern
        type Beta = extern

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
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_duplicate_declared_host_members_inside_capturing_closure() {
    let source = r#"
        type Event = extern
        type Alpha = extern
        type Beta = extern

        declare property Alpha:size: u32
        declare property Beta:size: u32
        declare function Alpha:value(delta: i32): i32
        declare function Beta:value(delta: i32): i32

        declare function make_alpha(): Alpha
        declare function listen(callback: (Event) -> unit): unit

        function install(): unit
            local alpha: Alpha = make_alpha()
            listen(function(event: Event): unit
                alpha.size = 3::u32
                assert(alpha:value(4::i32) == 4)
            end)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_method_call_via_method_declaration() {
    let source = r#"
        local point = { x = 41::i32 }

        function point:get_x(): i32
            return self.x
        end

        assert(point:get_x() == 41)
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_method_declaration_on_named_record_type() {
    let source = r#"
        type Point = { x: i32, y: i32 }

        function Point:sum_with(delta: i32): i32
            return self.x + self.y + delta
        end

        function Point:add(other: Point): Point
            return { x = (self.x + other.x)::i32, y = (self.y + other.y)::i32 }
        end

        local a: Point = { x = 2::i32, y = 4::i32 }
        local b: Point = { x = 10::i32, y = 1::i32 }
        local c: Point = a:add(b)

        assert(a:sum_with(3::i32) == 9::i32)
        assert(b:sum_with(3::i32) == 14::i32)
        assert(c.x == 12::i32)
        assert(c.y == 5::i32)
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn preserves_type_method_declarations_as_direct_functions() {
    let source = r#"
        type Point = { x: i32, y: i32 }

        function Point:sum_with(delta: i32): i32
            return self.x + self.y + delta
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");

    let function = typed
        .functions
        .iter()
        .find(|function| function.name == FunctionName::Simple("Point.sum_with".to_string()))
        .expect("expected type method to lower to direct function");
    assert_eq!(function.params[0].name, "self");
    assert!(matches!(
        &function.params[0].ty,
        Type::Opaque { name, ty } if name == "Point" && matches!(ty.as_ref(), Type::Record(_))
    ));
    assert!(
        typed
            .top_level
            .iter()
            .all(|stmt| !matches!(stmt, Stmt::FieldAssign { .. })),
        "type method declarations should not become top-level field assignments"
    );
}

#[test]
fn desugars_method_declaration_into_field_assignment_with_resolved_self_type() {
    let source = r#"
        local point = { x = 41::i32 }

        function point:get_x()
            return self.x
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");

    assert!(
        typed
            .functions
            .iter()
            .all(|function| !matches!(function.name, FunctionName::Method { .. })),
        "method declarations should be desugared before the typed program is returned"
    );

    let init = typed
        .functions
        .iter()
        .find(|function| function.name.to_string() == "__waluau_top_level_init")
        .expect("expected synthesized top-level init");
    let field_assign = init
        .body
        .iter()
        .find_map(|stmt| match stmt {
            Stmt::FieldAssign {
                base, name, value, ..
            } if matches!(base.as_ref(), Expr::Name(base_name, _, _) if base_name == "point")
                && name == "get_x" =>
            {
                Some(value)
            }
            _ => None,
        })
        .expect("expected method declaration to lower to point.get_x assignment");
    let Expr::Function(function) = field_assign else {
        panic!("expected method assignment to store a function value");
    };
    assert_eq!(function.return_type, Some(Type::Numeric(NumericType::I32)));
    assert_eq!(function.params[0].name, "self");
    assert_eq!(
        function.params[0].ty,
        Type::Record(std::iter::once(("x".to_string(), Type::Numeric(NumericType::I32))).collect())
    );
}

#[test]
fn type_checks_generic_method_declaration() {
    let source = r#"
        local point = { x = 41::i32 }

        function point:identity<T>(value: T): T
            local _x: i32 = self.x
            return value
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");

    let typed = super::type_check_and_infer(&program).expect("type check should succeed");
    let init = typed
        .functions
        .iter()
        .find(|function| function.name.to_string() == "__waluau_top_level_init")
        .expect("expected synthesized top-level init");
    let field_assign = init
        .body
        .iter()
        .find_map(|stmt| match stmt {
            Stmt::FieldAssign {
                base, name, value, ..
            } if matches!(base.as_ref(), Expr::Name(base_name, _, _) if base_name == "point")
                && name == "identity" =>
            {
                Some(value)
            }
            _ => None,
        })
        .expect("expected method declaration to lower to point.identity assignment");
    let Expr::Function(function) = field_assign else {
        panic!("expected method assignment to store a function value");
    };
    assert_eq!(function.type_params, vec!["T".to_string()]);
    assert_eq!(function.params[0].name, "self");
    assert_eq!(
        function.params[0].ty,
        Type::Record(std::iter::once(("x".to_string(), Type::Numeric(NumericType::I32))).collect())
    );
}

#[test]
fn rejects_generic_method_used_as_value_without_type_arguments() {
    let source = r#"
        local point = { x = 41::i32 }

        function point:identity<T>(value: T): T
            return value
        end

        function entry(): i32
            local f = point.identity
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.code(), Some("generic/uninstantiated-value"));
}

#[test]
fn type_checks_generic_method_call_with_type_arguments() {
    let source = r#"
        local point = { x = 41::i32 }

        function point:identity<T>(value: T): T
            return value
        end

        assert(point:identity<i32>(42::i32) == 42)
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_generic_method_call_without_type_arguments() {
    let source = r#"
        local point = { x = 41::i32 }

        function point:identity<T>(value: T): T
            return value
        end

        assert(point:identity(42::i32) == 42)
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_generic_method_call_without_type_arguments_when_uninferable() {
    let source = r#"
        local point = { x = 41::i32 }

        function point:identity<T, U>(value: T): T
            return value
        end

        assert(point:identity(42::i32) == 42)
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.code(), Some("generic/missing-type-args"));
}

#[test]
fn rejects_non_generic_method_call_with_type_arguments() {
    let source = r#"
        local point = { x = 41::i32 }

        function point:get_x(): i32
            return self.x
        end

        assert(point:get_x<i32>() == 41)
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.code(), Some("generic/extra-type-args"));
}

#[test]
fn rejects_new_field_after_record_read() {
    let source = r#"
        function entry(): i32
            local t = {}
            local x = t
            t.y = 1::i32
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "cannot add new field 't.y' after record was sealed"
    );
}

#[test]
fn rejects_new_field_after_record_field_read() {
    let source = r#"
        function entry(): i32
            local t = { x = 1::i32 }
            local x = t.x
            t.y = 1::i32
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "cannot add new field 't.y' after record was sealed"
    );
}

#[test]
fn rejects_new_field_after_record_return() {
    let source = r#"
        function entry()
            local t = {}
            return t
            t.y = 1::i32
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "cannot add new field 't.y' after record was sealed"
    );
}

#[test]
fn rejects_unknown_record_field_access() {
    let source = r#"
        function entry(): i32
            local t = { x = 1 }
            return t.y
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "unknown record field 'y'");
}

#[test]
fn type_checks_record_param_return_and_function_type_annotation() {
    let source = r#"
        function make_point(x: i32, y: i32): { x: i32, y: i32 }
            return { x = x, y = y }
        end

        function consume_point(p: { x: i32, y: i32 }): i32
            return p.x + p.y
        end

        function entry(): i32
            local make: (i32, i32) -> { x: i32, y: i32 } = make_point
            return consume_point(make(1, 2))
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_record_missing_field_on_annotation() {
    let source = r#"
        function entry(): i32
            local p: { x: i32, y: i32 } = { x = 1::i32 }
            return p.x
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "missing record field 'y'");
}

#[test]
fn rejects_record_extra_field_on_annotation() {
    let source = r#"
        function entry(): i32
            local p: { x: i32 } = { x = 1::i32, y = 2::i32 }
            return p.x
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "unexpected record field 'y'");
}

#[test]
fn rejects_record_field_type_mismatch_on_annotation() {
    let source = r#"
        function entry(): i32
            local p: { x: i32 } = { x = 1.5 }
            return p.x
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "numeric literal must be an integer for i32"
    );
}

#[test]
fn rejects_multi_assignment_arity_mismatch() {
    let source = r#"
        function pair(x: i32): i32, i32
            return x, x + 1
        end

        function entry(x: i32): i32
            local a: i32, b: i32 = pair(x)
            a, b = x
            return a
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "multi-assignment expects 2 values, got 1"
    );
}

#[test]
fn rejects_multi_return_type_mismatch() {
    let source = r#"
        function pair(x: i32): i32, bool
            return x, x + 1
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "cannot implicitly convert i32 to bool");
}

#[test]
fn rejects_multi_binding_arity_mismatch() {
    let source = r#"
        function entry(x: i32): i32
            local a: i32, b: i32 = x
            return a
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "multi-binding declaration expects 2 values, got 1"
    );
}

#[test]
fn rejects_multi_value_call_arity_mismatch() {
    let source = r#"
        function pair(x: i32): i32, i32
            return x, x + 1
        end

        function sum3(a: i32, b: i32, c: i32): i32
            return a + b + c
        end

        function entry(x: i32): i32
            return sum3(pair(x))
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "function expects 3 arguments, got 2");
}

#[test]
fn rejects_multi_value_call_type_mismatch() {
    let source = r#"
        function pair(x: i32): i32, bool
            return x, x > 0
        end

        function sum2(a: i32, b: i32): i32
            return a + b
        end

        function entry(x: i32): i32
            return sum2(pair(x))
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "cannot implicitly convert bool to i32");
}

#[test]
fn rejects_multi_value_in_scalar_context() {
    let source = r#"
        function pair(x: i32, y: i32): i32, i32
            return x, y
        end

        function entry(x: i32, y: i32): number
            local t: number = pair(x, y)
            return t
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "cannot implicitly convert multiple values to f64"
    );
}

#[test]
fn type_checks_coroutine_create_and_resume_for_zero_arg_functions() {
    let source = r#"
        function run_job(): i32
            local job: () -> i32 = function(): i32
                return 7
            end
            local co: thread = coroutine.create(job)
            local result: Yielded(unknown) | Finished(i32) | Error(string) = coroutine.resume(co)
            if result is Finished then
                return result.value
            end
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_coroutine_create_for_non_zero_arg_functions() {
    let source = r#"
        function run_job(): i32
            local job: (i32) -> i32 = function(x: i32): i32
                return x
            end
            local co: thread = coroutine.create(job)
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "coroutine.create expects a zero-argument i32-returning function"
    );
}

#[test]
fn type_checks_coroutine_close_for_threads() {
    let source = r#"
        function run_job(): bool
            local job: () -> i32 = function(): i32
                return 7
            end
            local co: thread = coroutine.create(job)
            return coroutine.close(co)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_coroutine_resume_for_non_thread() {
    let source = r#"
        function run_job(): i32
            local co: () -> i32 = function(): i32
                return 7
            end
            local result: Yielded(unknown) | Finished(i32) | Error(string) = coroutine.resume(co)
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "coroutine.resume expects a thread");
}

#[test]
fn type_checks_tagged_union_constructor_inline_union() {
    let source = r#"
        function test(): i32
            local a: Num(i32) | Flag(i32) = Num(42)
            local b: Num(i32) | Flag(i32) = Flag(7)
            if a is Num then
                return a.value
            end
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_tagged_union_constructor_named_alias() {
    let source = r#"
        type Value = Num(i32) | Flag(i32)

        function process(v: Value): i32
            if v is Num then
                return v.value
            end
            return 0
        end

        function test(): i32
            local a: Value = Num(42)
            return process(a)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_tagged_union_constructor_mixed_payloads() {
    let source = r#"
        type Either = Left(i32) | Right(f64) | Text(string)

        function int_value(): Either
            return Left(42)
        end

        function float_value(): Either
            return Right(3.5)
        end

        function text_value(): Either
            return Text("ok")
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_tagged_union_constructor_for_unknown_variant() {
    let source = r#"
        function test(): i32
            local a: Num(i32) | Flag(i32) = Other(42)
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect_err("type check should fail");
}

#[test]
fn type_checks_tagged_union_narrowing_and_value_access() {
    let source = r#"
        type Resume<R> = Yielded(unknown) | Finished(R) | Error(string)

        function unwrap(result: Resume<i32>): i32
            if result is Yielded then
                return 0
            end
            if result is Error then
                return 0
            end
            return result.value
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_tagged_union_pattern_match_binding() {
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
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_tagged_union_pattern_match_binding_outside_then_branch() {
    let source = r#"
        type Either = Left(i32) | Right(f64)

        function left(either: Either): i32
            if Left(value) = either then
                return 0
            end
            return value
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("binding should not escape the then branch");
    assert!(
        error.to_string().contains("value"),
        "error should mention 'value', got: {error}"
    );
}

#[test]
fn rejects_tagged_union_pattern_match_for_unknown_variant() {
    let source = r#"
        type Either = Left(i32) | Right(f64)

        function left(either: Either): i32
            if Up(value) = either then
                return value
            end
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("Up is not a variant of Either");
    assert!(
        error.to_string().contains("Up"),
        "error should mention the unknown variant 'Up', got: {error}"
    );
}

#[test]
fn coroutine_resume_returns_tagged_union() {
    let source = r#"
        function run_job(): i32
            local job: () -> i32 = function(): i32
                return 7
            end
            local co: thread = coroutine.create(job)
            local result: Yielded(unknown) | Finished(i32) | Error(string) = coroutine.resume(co)
            if result is Finished then
                return result.value
            end
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn coroutine_resume_assigns_to_named_tagged_union_alias() {
    let source = r#"
        type Result = Yielded(unknown) | Finished(i32) | Error(string)

        function run_job(): i32
            local job: () -> i32 = function(): i32
                return 7
            end
            local co: thread = coroutine.create(job)
            local r: Result = coroutine.resume(co)
            if r is Finished then
                return r.value
            end
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_tagged_union_value_access_without_narrowing() {
    let source = r#"
        type Resume<R> = Yielded(unknown) | Finished(R) | Error(string)

        function unwrap(result: Resume<i32>): i32
            return result.value
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "field access on tagged union requires narrowing before reading 'value'"
    );
}

#[test]
fn type_checks_coroutine_yield_calls() {
    let source = r#"
        function run_job(): i32
            coroutine.yield("tick")
            return 7
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_coroutine_await_promise_calls() {
    let source = r#"
        declare function makePromise(): extern

        function run_job(): string
            local value: unknown = coroutine.await_promise(makePromise())
            return value::string
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_coroutine_await_promise_for_non_extern_value() {
    let source = r#"
        function run_job(): i32
            local value: unknown = coroutine.await_promise(42)
            return value::i32
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "coroutine.await_promise expects an extern Promise-like value"
    );
}

#[test]
fn type_checks_coroutine_resume_unknown_payloads() {
    let source = r#"
        function run_job(): i32
            local co: thread = coroutine.create(function(): i32
                coroutine.yield(1)
                return 7
            end)
            local ok: bool, value: unknown = coroutine.resume(co)
            if ok then
                return value::i32
            end
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_coroutine_resume_unknown_payload_without_explicit_cast() {
    let source = r#"
        function run_job(): i32
            local co: thread = coroutine.create(function(): i32
                coroutine.yield(1)
                return 7
            end)
            local ok: bool, value: i32 = coroutine.resume(co)
            if ok then
                return value
            end
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "cannot implicitly convert unknown to i32; use an explicit cast"
    );
}

#[test]
fn type_checks_assert_statement() {
    let source = r#"
        function check(x: i32): i32
            assert(x > 0)
            return x
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_assert_with_non_bool_argument() {
    let source = r#"
        function check(x: i32): i32
            assert(x)
            return x
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "cannot implicitly convert i32 to bool");
}

#[test]
fn type_checks_print_with_string_argument() {
    let source = r#"
        declare function print(message: string): unit

        function check(): i32
            print("hello")
            return 1
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_unit_return_with_print() {
    let source = r#"
        declare function print(message: string): unit

        function check(): unit
            print("hello")
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_print_in_non_unit_expression_context() {
    let source = r#"
        declare function print(message: string): unit

        function check(): string
            return print("hello")
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "cannot implicitly convert unit to string"
    );
}

#[test]
fn type_checks_string_literals_and_annotations() {
    let source = r#"
        function entry(): string
            local x: string = "hello"
            return x
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_string_equality_in_control_flow() {
    let source = r#"
        function entry(a: string, b: string): i32
            if a == b then
                return 1
            end
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_bytes_literals_and_operations() {
    let source = r#"
        function entry(a: bytes, b: bytes): i32
            local prefix: bytes = b"OK"
            local merged: bytes = prefix .. a
            if merged == b then
                return merged[0]
            end
            if merged < b"ZZ" then
                return #merged
            end
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_string_equality_with_non_string() {
    let source = r#"
        function entry(a: string): bool
            return a == 1
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "numeric literal is not assignable to string"
    );
}

#[test]
fn type_checks_tostring_for_primitive_inputs() {
    let source = r#"
        function entry(a: i32, b: bool, c: string): string
            local x: string = tostring(a)
            local y: string = tostring(b)
            local z: string = tostring(c)
            return x .. y .. z
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_tostring_for_non_primitive_inputs() {
    let source = r#"
        function entry(xs: {i32}): string
            return tostring(xs)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "tostring expects a primitive argument (numeric, bool, or string), got {i32}"
    );
}

#[test]
fn type_checks_table_concat_with_and_without_separator() {
    let source = r#"
        function entry(words: {string}): string
            local a: string = table.concat(words, ", ")
            local b: string = table.concat(words)
            return a .. b
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_table_concat_for_non_string_array() {
    let source = r#"
        function entry(nums: {i32}): string
            return table.concat(nums, ", ")
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "table.concat expects an array of strings, got {i32}"
    );
}

#[test]
fn rejects_table_concat_with_non_string_separator() {
    let source = r#"
        function entry(words: {string}, separator: i32): string
            return table.concat(words, separator)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "table.concat expects a string separator, got i32"
    );
}

#[test]
fn rejects_array_equality_in_mvp() {
    let source = r#"
        function entry(a: {i32}, b: {i32}): bool
            return a == b
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "== supports only numeric, bool, string, and bytes operands in MVP"
    );
}

#[test]
fn rejects_function_equality_in_mvp() {
    let source = r#"
        function entry(): bool
            local a: () -> i32 = function(): i32
                return 1
            end
            local b: () -> i32 = function(): i32
                return 2
            end
            return a == b
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "== supports only numeric, bool, string, and bytes operands in MVP"
    );
}

#[test]
fn empty_braces_record_can_be_unused() {
    let source = r#"
        function entry(): i32
            local t = {}
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn tags_recursive_return_inference_failure_as_unsupported() {
    let source = r#"
        function fact(n: i32)
            if n == 0 then
                return 1
            end
            return n * fact(n - 1)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.code(), Some("inference/unsupported"));
    assert_eq!(error.category(), Some(DiagnosticCategory::Unsupported));
    assert_eq!(
        error.action(),
        Some("add an explicit return type annotation to break the cycle")
    );
}

#[test]
fn accepts_for_in_with_bool_termination_and_two_bindings() {
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
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_for_in_arity_mismatch() {
    let source = r#"
        function entry(): i32
            local iter = function(): bool, i32
                return true, 1
            end
            for a, b in iter do
                return a + b
            end
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "for-in iterator expects 3 return values (bool + 2 loop values), got 2"
    );
}

#[test]
fn rejects_for_in_without_bool_first_value() {
    let source = r#"
        function entry(): i32
            local iter = function(): i32
                return 1
            end
            for a in iter do
                return a
            end
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "for-in iterator expects 2 return values (bool + 1 loop values), got 1"
    );
}
