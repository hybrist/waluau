use std::collections::HashSet;
use std::sync::Arc;

use waluau_ast::{BinaryOp, Expr, FunctionName, NumericType, OpaquePayload, Stmt, Type, UnaryOp};
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
fn module_functions_remain_hoisted_for_forward_and_mutual_calls() {
    let source = r#"
        function is_even(n: i32): bool
            if n == 0 then
                return true
            end
            return is_odd(n - 1)
        end

        function is_odd(n: i32): bool
            if n == 0 then
                return false
            end
            return is_even(n - 1)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("hoisted mutual calls should type check");
}

#[test]
fn local_function_remains_a_capturing_rebindable_closure() {
    let source = r#"
        function evaluate(seed: i32): i32
            local function apply(value: i32): i32
                return seed + value
            end
            apply = function(value: i32): i32
                return seed * value
            end
            return apply(3)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("capturing local function should remain rebindable");
}

#[test]
fn local_function_body_uses_the_declarations_lexical_identity() {
    let source = r#"
        function evaluate(depth: i32): i32
            local function recurse(value: i32): i32
                if value == 0 then
                    return 0
                end
                return recurse(value - 1)
            end
            return recurse(depth)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");
    let evaluate = typed
        .functions
        .iter()
        .find(|function| function.name.to_string() == "evaluate")
        .expect("evaluate should exist");
    let Stmt::Let {
        symbol_id: declaration_id,
        value: Expr::Function(function),
        ..
    } = &evaluate.body[0]
    else {
        panic!("expected lexical function declaration");
    };
    let Stmt::Return(Expr::Call { callee, .. }) = &function.body[1] else {
        panic!("expected recursive call");
    };
    let Expr::Name(_, recursive_id, _) = callee.as_ref() else {
        panic!("expected recursive name");
    };
    assert_eq!(function.symbol_id, *declaration_id);
    assert_eq!(*recursive_id, *declaration_id);
}

#[test]
fn lexical_function_forward_reference_remains_out_of_scope() {
    let source = r#"
        function evaluate(value: i32): i32
            local function first(current: i32): i32
                return second(current)
            end
            local function second(current: i32): i32
                return current
            end
            return first(value)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("later lexical declaration is not visible");
    assert_eq!(error.to_string(), "unknown name 'second'");
}

#[test]
fn later_lexical_function_can_call_an_earlier_declaration() {
    let source = r#"
        function evaluate(value: i32): i32
            local function first(current: i32): i32
                return current + 1
            end
            local function second(current: i32): i32
                return first(current) + 1
            end
            return second(value)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("earlier lexical declaration should be visible");
}

#[test]
fn const_function_remains_a_non_rebindable_lexical_closure() {
    let source = r#"
        function evaluate(seed: i32): i32
            const function apply(value: i32): i32
                return seed + value
            end
            apply = function(value: i32): i32
                return value
            end
            return apply(3)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("const function must reject rebinding");
    assert_eq!(
        error.to_string(),
        "cannot rebind constant lexical binding 'apply'"
    );
}

#[test]
fn module_function_rebinding_reports_its_non_rebindable_semantics() {
    let program = parse(
        r#"
        function answer(): i32
            return 42
        end
        answer = function(): i32
            return 0
        end
        "#,
    )
    .expect("parse should succeed");
    let error = super::type_check(&program).expect_err("module functions cannot be rebound");
    assert_eq!(error.to_string(), "cannot rebind module function 'answer'");
}

#[test]
fn module_rebinding_classification_survives_an_earlier_unrelated_error() {
    let program = parse(
        r#"
        function answer(): i32
            return 42
        end
        function entry(): unit
            missing()
            answer = function(): i32
                return 0
            end
        end
        "#,
    )
    .expect("parse should succeed");
    let errors = super::type_check_and_infer_collect(&program).expect_err("program is invalid");
    assert!(
        errors
            .iter()
            .any(|error| error.to_string().contains("unknown name 'missing'")),
        "earlier error remains visible: {errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|error| { error.to_string() == "cannot rebind module function 'answer'" }),
        "later rebinding keeps its authored classification: {errors:?}"
    );
}

#[test]
fn non_module_function_rebinding_is_not_mislabeled_as_a_module_function() {
    for (source, expected) in [
        (
            "print = function(value: string): unit\nend\n",
            "unknown lexical binding 'print'",
        ),
        (
            "declare function host(value: string): unit\nhost = function(value: string): unit\nend\n",
            "cannot rebind declared or builtin function 'host'",
        ),
    ] {
        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("function binding cannot be rebound");
        assert_eq!(error.to_string(), expected);
        assert!(
            !error.to_string().contains("module function"),
            "non-module diagnostics must not claim authored module semantics"
        );
    }
}

#[test]
fn diagnostic_type_display_retains_nested_module_identity_until_decoration() {
    let ty = Type::Function {
        has_self: false,
        params: vec![Type::Opaque {
            name: "__waluau_m12_Graphics".to_string(),
            ty: OpaquePayload::new(Type::record(Default::default())),
            generic_extern: None,
        }],
        return_type: Arc::new(Type::Unit),
    };

    let display = super::module_type_display(&ty);
    assert_eq!(display, "(__waluau_m12_Graphics) -> unit");
}

#[test]
fn nominal_enums_reject_cross_enum_assignment() {
    let source = r#"
        enum Direction { north, south }
        enum Facing { north, south }

        local facing: Facing = Facing.north
        local direction: Direction = facing
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "cannot implicitly convert Facing to Direction"
    );
}

#[test]
fn any_enum_declared_param_accepts_every_nominal_enum() {
    let source = r#"
        enum Direction { north, south }
        enum Facing { north, south }

        declare function ordinal_of(value: enum): i32

        local first: i32 = ordinal_of(Direction.south)
        local facing: Facing = Facing.north
        local second: i32 = ordinal_of(facing)
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn any_enum_declared_param_rejects_plain_numbers() {
    let source = r#"
        declare function ordinal_of(value: enum): i32

        local n: i32 = 1
        local bad: i32 = ordinal_of(n)
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "cannot implicitly convert i32 to enum");
}

#[test]
fn any_enum_value_never_flows_back_into_a_specific_enum() {
    let source = r#"
        enum Direction { north, south }

        declare function some_enum(): enum

        local direction: Direction = some_enum()
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "cannot implicitly convert enum to Direction"
    );
}

#[test]
fn nullable_enum_coerces_into_nullable_any_enum_param() {
    let source = r#"
        enum Direction { north, south }

        declare function ordinal_of(value: enum?): i32

        local maybe: Direction? = Direction.south
        local first: i32 = ordinal_of(maybe)
        local second: i32 = ordinal_of(Direction.north)
        local third: i32 = ordinal_of(nil)
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn nullable_value_is_not_an_implicit_nullable_bool() {
    let source = r#"
        enum Direction { north, south }

        local maybe: Direction? = Direction.south
        local flag: bool? = maybe
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "cannot implicitly convert Direction? to bool?"
    );
}

#[test]
fn overload_selection_prefers_same_shape_over_nullable_wrapping() {
    let source = r#"
        enum Direction { north, south }

        declare function probe(value: f64): string
        declare function probe(value: f64?): string
        declare function probe(value: bool): string
        declare function probe(value: enum): string
        declare function probe(value: enum?): string

        local plain: string = probe(4)
        local from_enum: string = probe(Direction.north)
        local maybe: Direction? = Direction.south
        local from_nullable_enum: string = probe(maybe)
        local maybe_number: f64? = 1.5
        local from_nullable_number: string = probe(maybe_number)
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn not_property_chain_resolves_through_declared_extern_property() {
    let source = r#"
        type Expectation = extern

        declare function expect_value(value: f64): Expectation
        declare property Expectation:not: Expectation
        declare function Expectation:toBe(expected: f64): unit

        expect_value(4):not:toBe(5)
        expect_value(4):not:not:toBe(4)
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn enum_match_rejects_a_different_nominal_scrutinee() {
    let source = r#"
        enum Direction { north, south }
        enum Facing { north, south }

        function score(facing: Facing): i32
            match facing do
            case Direction.north then return 1
            case Direction.south then return 2
            end
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "cannot implicitly convert Facing to Direction"
    );
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
        Type::Opaque { name, ty, .. } if name == "Element" && **ty == Type::Extern
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
        Some(Type::Opaque { name, ty, .. }) if name == "Promise<GraphModel>" && ty.as_ref() == &Type::Extern
    ));
    assert!(matches!(
        typed.functions[1].return_type.as_ref(),
        Some(Type::Opaque { name, ty, .. }) if name == "Promise<LayersModel>" && ty.as_ref() == &Type::Extern
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
            if matches!(inner.as_ref(), Type::Opaque { name, ty, .. } if name == "Element" && ty.as_ref() == &Type::Extern)
    ));
}

#[test]
fn infers_method_call_local_after_early_return_nil_narrowing() {
    let source = r#"
        type RenderTarget = extern
        type Texture = extern
        type Graphics = extern

        declare function Graphics:texture_from_render_target(target: RenderTarget): Texture

        function render(
            graphics: Graphics,
            back: RenderTarget?,
            face: RenderTarget?,
            face_up: bool
        ): Texture?
            local target = back
            if face_up then target = face end
            if target == nil then return nil end
            local texture = graphics:texture_from_render_target(target)
            return texture
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check_and_infer(&program)
        .expect("the surviving branch should keep target narrowed for local inference");
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
fn nullable_function_values_support_locals_aliases_and_nil_narrowing() {
    let source = r#"
        type Callback = () -> unit

        function invoke(callback: () -> unit): unit
            callback()
        end

        function run(callback: (() -> unit)?): unit
            local unused: Callback? = nil
            if callback ~= nil then
                invoke(callback)
            end
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");
    assert!(matches!(
        &typed.functions[1].params[0].ty,
        Type::Nullable(inner) if matches!(inner.as_ref(), Type::Function { params, return_type, .. }
            if params.is_empty() && return_type.as_ref() == &Type::Unit)
    ));
}

#[test]
fn nullable_modifier_accepts_primitive_value_types() {
    let source = r#"
        function pick(a: i32?, b: u32?, c: i64?, d: u64?, e: f32?, f: f64?, g: bool?): i32
            return 0
        end

        type HTMLInputElement = extern

        declare property HTMLInputElement:selectionStart: u32?
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check_and_infer(&program)
        .expect("nullable primitive annotations should type check");
}

#[test]
fn nullable_primitive_nil_check_narrows_to_primitive() {
    let source = r#"
        function unwrap_or_zero(value: i32?): i32
            if value ~= nil then
                return value + 1
            end
            return 0
        end

        function unwrap_f64(value: f64?): f64
            if value == nil then
                return 0.0
            else
                return value * 2.0
            end
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check_and_infer(&program)
        .expect("nil checks should narrow nullable primitives to their inner type");
}

#[test]
fn nullable_primitive_does_not_implicitly_coerce_to_primitive() {
    let source = r#"
        function passthrough(value: i32?): i32
            return value
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "cannot implicitly convert nullable value to i32"
    );
}

#[test]
fn nullable_modifier_rejects_unsupported_inner_types() {
    let source = r#"
        function entry(value: unknown?): i32
            return 0
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "nullable modifier '?' is not supported on unknown"
    );
}

#[test]
fn nullable_records_and_arrays_support_options_objects() {
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
    super::type_check_and_infer(&program).expect("options-object example should type check");
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
        "nil comparison requires a nullable operand"
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
fn type_checks_extern_reference_equality() {
    let source = r#"
        type Node = extern
        type Element = extern extends Node

        function same(a: Element, b: Element): bool
            return a == b
        end

        function different(a: Element, b: Element): bool
            return a ~= b
        end

        function across_subtypes(a: Node, b: Element): bool
            return a == b
        end

        function nullable_side(a: Element?, b: Element): bool
            return a == b
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check_and_infer(&program).expect("type check should succeed");
}

#[test]
fn rejects_equality_between_unrelated_extern_types() {
    let source = r#"
        type Element = extern
        type AudioContext = extern

        function entry(a: Element, b: AudioContext): bool
            return a == b
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "== requires compatible extern operand types"
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
fn infers_method_call_local_inside_if_cast_branch() {
    let source = r#"
        type Node = extern
        type Element = extern extends Node

        declare function Element:value(): i32

        function read(node: Node): i32
            if Element(element) = node then
                local result = element:value()
                return result
            end
            return 0
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check_and_infer(&program)
        .expect("the if-cast binding should be available during local inference");
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
    let expected = Type::record(
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
        Type::Opaque { name, ty, .. } if name == "Promise<Response>" && ty.as_ref() == &Type::Extern
    ));
    assert!(matches!(
        &typed.functions[1].params[0].ty,
        Type::Opaque { name, ty, .. } if name == "Promise<string>" && ty.as_ref() == &Type::Extern
    ));
    assert_ne!(
        typed.functions[0].params[0].ty,
        typed.functions[1].params[0].ty
    );

    let response_promise = &typed.functions[0].params[0].ty;
    let metadata = response_promise
        .generic_extern()
        .expect("generic extern metadata should be retained");
    assert_eq!(metadata.constructor, "Promise");
    assert_eq!(metadata.source_name, "Promise");
    assert_eq!(metadata.type_args.len(), 1);
    assert!(matches!(
        &metadata.type_args[0],
        Type::Opaque { name, ty, .. } if name == "Response" && ty.as_ref() == &Type::Extern
    ));
    assert_eq!(response_promise.to_string(), "Promise<Response>");
}

#[test]
fn typed_promise_await_rejects_unrelated_generic_externs() {
    let source = r#"
        type Future<T> = extern
        declare function make(): Future<string>

        function read(): string
            return promise.await(make())
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("Future<T> is not Promise<T>");
    assert_eq!(
        error.to_string(),
        "promise.await expects a Promise<T> extern value"
    );
}

#[test]
fn generic_functions_substitute_generic_extern_metadata() {
    let source = r#"
        type Promise<T> = extern
        declare function make_text(): Promise<string>

        function unwrap<T>(pending: Promise<T>): T
            return promise.await(pending)
        end

        function inferred(): string
            return unwrap(make_text())
        end

        function explicit(): string
            return unwrap<string>(make_text())
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("generic Promise<T> should substitute structurally");
}

#[test]
fn generic_extern_equality_rejects_distinct_specializations() {
    let source = r#"
        type Promise<T> = extern

        function compare(left: Promise<string>, right: Promise<i32>): bool
            return left == right
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("distinct specializations stay nominal");
    assert_eq!(
        error.to_string(),
        "== requires compatible extern operand types"
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
        Some(Type::Opaque { name, ty, .. }) if name == "Promise<Response>" && ty.as_ref() == &Type::Extern
    ));
    assert!(matches!(
        typed.functions[1].return_type.as_ref(),
        Some(Type::Opaque { name, ty, .. }) if name == "Promise<string>" && ty.as_ref() == &Type::Extern
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
fn recursive_record_arrays_are_supported() {
    let source = r#"
        type Tree = {value: i32, children: {Tree}}

        function leaf(value: i32): Tree
            return {value = value, children = {}}
        end

        function sum(tree: Tree): i32
            local total: i32 = tree.value
            for child in tree.children do
                total += sum(child)
            end
            return total
        end

        function entry(): i32
            local tree: Tree = {
                value = 1,
                children = {leaf(2)},
            }
            return sum(tree)
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check_and_infer(&program).expect("recursive tree should type check");
}

fn retained_record_graph_source(helper_count: usize) -> String {
    let mut source = String::from("type Leaf = {value: i32}\n");
    source.push_str("type Layer0 = {a: Leaf, b: Leaf, c: Leaf, d: Leaf}\n");
    for depth in 1..8 {
        source.push_str(&format!(
            "type Layer{depth} = {{a: Layer{}, b: Layer{}, c: Layer{}, d: Layer{}}}\n",
            depth - 1,
            depth - 1,
            depth - 1,
            depth - 1,
        ));
    }
    source.push_str(
        "type Node = {content: Layer7, children: {Node}, callback: (Node) -> Layer7}\n\
         type Board = {a: Node, b: Node, c: Node, d: Node}\n",
    );
    for helper in 0..helper_count {
        source.push_str(&format!(
            "function helper{helper}(board: Board, node: Node): Board\n\
                 return board\n\
             end\n"
        ));
    }
    source.push_str(&format!(
        "function entry(board: Board, node: Node): Board\n\
             return helper{}(board, node)\n\
         end\n",
        helper_count - 1,
    ));
    source
}

fn collect_unique_opaque_payloads(ty: &Type, payloads: &mut HashSet<*const Type>) {
    match ty {
        Type::Opaque { ty, .. } => {
            if payloads.insert(ty.as_ptr()) {
                collect_unique_opaque_payloads(ty, payloads);
            }
        }
        Type::ExternSubtype(ty) | Type::Nullable(ty) | Type::Array(ty) | Type::Variadic(ty) => {
            collect_unique_opaque_payloads(ty, payloads)
        }
        Type::Multi(types) => {
            for ty in types {
                collect_unique_opaque_payloads(ty, payloads);
            }
        }
        Type::Function {
            params,
            return_type,
            ..
        } => {
            for ty in params {
                collect_unique_opaque_payloads(ty, payloads);
            }
            collect_unique_opaque_payloads(return_type, payloads);
        }
        Type::Record(fields) => {
            for ty in fields.values() {
                collect_unique_opaque_payloads(ty, payloads);
            }
        }
        Type::TaggedVariant(variant) => collect_unique_opaque_payloads(&variant.payload, payloads),
        Type::TaggedUnion(variants) => {
            for variant in variants {
                collect_unique_opaque_payloads(&variant.payload, payloads);
            }
        }
        Type::Named { type_args, .. } => {
            for ty in type_args {
                collect_unique_opaque_payloads(ty, payloads);
            }
        }
        _ => {}
    }
}

fn retained_record_graph_payloads(helper_count: usize) -> (usize, Vec<OpaquePayload>) {
    let source = retained_record_graph_source(helper_count);
    let program = parse(&source).expect("retained record graph should parse");
    let typed = super::type_check_and_infer(&program).expect("retained record graph should check");
    let mut payloads = HashSet::new();
    let mut board_payloads = Vec::new();
    for function in &typed.functions {
        for param in &function.params {
            collect_unique_opaque_payloads(&param.ty, &mut payloads);
            if let Type::Opaque { name, ty, .. } = &param.ty
                && name == "Board"
            {
                board_payloads.push(ty.clone());
            }
        }
        if let Some(return_type) = &function.return_type {
            collect_unique_opaque_payloads(return_type, &mut payloads);
        }
    }
    (payloads.len(), board_payloads)
}

#[test]
fn repeated_retained_record_signatures_share_resolved_payloads() {
    let (single_count, _) = retained_record_graph_payloads(1);
    let (repeated_count, board_payloads) = retained_record_graph_payloads(32);

    assert_eq!(
        repeated_count, single_count,
        "adding receiver/helper signatures must not duplicate the resolved alias graph"
    );
    let first = board_payloads
        .first()
        .expect("helpers should have Board parameters");
    assert!(
        board_payloads.iter().all(|payload| payload.ptr_eq(first)),
        "every Board parameter should share the cached resolved payload"
    );
}

#[test]
fn recursive_alias_callbacks_accept_the_same_nominal_identity() {
    let source = r#"
        type Node = {children: {Node}, update: (Node) -> Node}

        function identity(node: Node): Node
            return node
        end

        function new(): Node
            return {children = {}, update = identity}
        end
    "#;
    let program = parse(source).expect("recursive callback program should parse");
    super::type_check_and_infer(&program)
        .expect("the same recursive alias should match inside a callback signature");
}

#[test]
fn recursive_alias_callbacks_reject_distinct_nominal_identities() {
    let source = r#"
        type Left = {children: {Left}}
        type Right = {children: {Right}}
        type Holder = {update: (Left) -> Left}

        function update(right: Right): Right
            return right
        end

        function new(): Holder
            return {update = update}
        end
    "#;
    let program = parse(source).expect("distinct recursive callback program should parse");
    let error = super::type_check_and_infer(&program)
        .expect_err("distinct recursive aliases must remain distinct in callback signatures");
    assert_eq!(
        error.to_string(),
        "cannot implicitly convert (Right) -> Right to (Left) -> Left"
    );
}

#[test]
fn rejects_unguarded_mutually_recursive_aliases() {
    let source = r#"
        type Left = Right
        type Right = Left

        function entry(value: Left): unit
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    let message = error.to_string();
    assert!(
        message.contains("cyclic type declaration detected")
            && message.contains("Left -> Right -> Left"),
        "expected a stable alias-cycle diagnostic, got: {message}"
    );
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
fn annotated_nullable_primitive_arrays_infer_literal_elements_from_context() {
    let source = r#"
        function entry(): i32
            local flags: {bool?} = {true, nil, false}
            local labels: {string?} = {"first", nil, "last"}
            local chunks: {bytes?} = {b"first", nil, b"last"}
            return #flags + #labels + #chunks
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check_and_infer(&program)
        .expect("the declared nullable element type should drive literal inference");
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
fn infers_annotated_empty_array_literals() {
    let source = r#"
        function entry(): i32
            local xs: {i32} = {}
            return #xs
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("annotated empty array literal should type check");
}

#[test]
fn infers_empty_array_literals_assigned_to_record_fields() {
    let source = r#"
        function entry(): i32
            local state: { items: {i32} } = { items = {1, 2, 3} }
            state.items = {}
            return #state.items
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("record-field empty array assignment should type check");
}

#[test]
fn rejects_empty_array_literals_without_context() {
    let source = r#"
        function entry(): i32
            return #{}
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
fn checks_empty_record_type_alias() {
    let source = r#"
        type Marker = {}

        function tag(m: Marker): i32
            return 7
        end

        function entry(): i32
            local m: Marker = {}
            return tag(m)
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("empty record alias should type check");
}

#[test]
fn rejects_empty_braces_against_nonempty_record_type() {
    let source = r#"
        function entry(): i32
            local p: { x: i32 } = {}
            return p.x
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert!(
        error.to_string().contains("missing record field 'x'"),
        "unexpected diagnostic: {error}"
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
    assert_eq!(
        error.to_string(),
        "# requires a string, array, or bytes operand"
    );
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
fn rejects_concat_compound_assignment_on_numeric_target() {
    let source = r#"
        function entry(n: i32): i32
            n ..= "x"
            return n
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "compound assignment to 'n' requires a string target"
    );
}

#[test]
fn accepts_compound_assignment_operators() {
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
    super::type_check(&program).expect("type check should succeed");
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
    assert_eq!(
        error.to_string(),
        "cannot rebind constant lexical binding 'y'"
    );
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
fn infers_unannotated_function_expression_return_type() {
    let source = r#"
        function entry(): i32
            local add1 = function(x: i32)
                return x + 1
            end
            return add1(1)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program)
        .expect("unannotated function expression return type should be inferred");
    let entry = typed
        .functions
        .iter()
        .find(|function| function.name.to_string() == "entry")
        .expect("entry function should exist");
    let add1_return = entry.body.iter().find_map(|stmt| match stmt {
        Stmt::Let {
            value: Expr::Function(function),
            ..
        } => Some(function.return_type.clone()),
        _ => None,
    });
    // The function expression's return type is backfilled onto the AST so the IR
    // can lower it without an explicit annotation.
    assert_eq!(add1_return, Some(Some(Type::Numeric(NumericType::I32))));
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
fn type_checks_implicit_top_level_assignment_declaration() {
    let source = r#"
        x = 41::i32
        assert(x == 41::i32)
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");
    let init = typed
        .functions
        .iter()
        .find(|function| function.name.to_string() == "__waluau_top_level_init")
        .expect("expected synthesized top-level init");
    assert!(matches!(&init.body[0], Stmt::Let { name, .. } if name == "x"));
}

#[test]
fn type_checks_implicit_top_level_multi_assignment_declaration() {
    let source = r#"
        function pair(x: i32, y: bool): i32, bool
            return x, y
        end

        a, b = pair(1::i32, true)
        assert(a == 1::i32)
        assert(b)
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");
    let init = typed
        .functions
        .iter()
        .find(|function| function.name.to_string() == "__waluau_top_level_init")
        .expect("expected synthesized top-level init");
    assert!(matches!(
        &init.body[0],
        Stmt::LetMulti { bindings, .. }
            if bindings.iter().map(|binding| binding.name.as_str()).collect::<Vec<_>>()
                == vec!["a", "b"]
    ));
}

#[test]
fn type_checks_nested_function_mutating_implicit_top_level_declaration() {
    let source = r#"
        t = { value = 0::i32 }
        local callback: () -> unit = function(): unit
            t.value = 41::i32
        end
        callback()
        assert(t.value == 41::i32)
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_top_level_read_before_implicit_assignment_declaration() {
    let source = r#"
        assert(x == 1::i32)
        x = 1::i32
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert!(error.to_string().contains("unknown"));
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
        Type::Opaque { name, ty, .. } if name == "Point" && matches!(ty.as_ref(), Type::Record(_))
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
        Type::record(std::iter::once(("x".to_string(), Type::Numeric(NumericType::I32))).collect())
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
        Type::record(std::iter::once(("x".to_string(), Type::Numeric(NumericType::I32))).collect())
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
fn readonly_is_an_unknown_generic_type() {
    let source = r#"
        type Model = { count: i32 }

        function read(view: readonly<Model>): i32
            return view.count
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("readonly should not be a built-in type");
    assert_eq!(error.to_string(), "unknown type 'readonly'");
}

#[test]
fn ordinary_alias_parameters_remain_mutable() {
    let source = r#"
        type Model = { count: i32, values: {i32} }

        function mutate(model: Model): unit
            model.count += 1
            model.values[0] = 2
            table.insert(model.values, 3)
        end

        local model: Model = { count = 0::i32, values = { 1::i32 } }
        mutate(model)
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("aliases and parameters should remain mutable");
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
fn type_checks_mixed_annotation_multi_binding() {
    let source = r#"
        function entry(): i32
            local a: i32, b = 10, 20.5
            const c: i32, d = 3, 4.0
            local e, f: i32 = 1.5, 2
            return a + c + f
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_mixed_annotation_multi_binding_value_mismatch() {
    let source = r#"
        function entry(): i32
            local a: i32, b = "nope", 2.0
            return a
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert!(
        error
            .to_string()
            .contains("cannot implicitly convert string to i32"),
        "unexpected message: {error}"
    );
}

#[test]
fn rejects_mixed_annotation_multi_binding_arity_mismatch() {
    let source = r#"
        function entry(x: i32): i32
            local a: i32, b = x
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
fn allows_omitting_only_trailing_nullable_arguments() {
    let source = r#"
        function valid(required: string, optional: string?): string
            return required
        end

        function invalid(optional: string?, required: string): string
            return required
        end

        function entry(): string
            local value = valid("value")
            return invalid()
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("non-trailing omission should fail");
    assert_eq!(error.to_string(), "function expects 2 arguments, got 0");
}

#[test]
fn multi_value_in_scalar_context_takes_first_value() {
    // Lua adjustment rules: a multi-value call in a single-value context
    // keeps only its first value.
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
    super::type_check(&program).expect("multi-value adjustment should type check");
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
fn type_checks_tagged_union_payload_with_literal_union_field() {
    let source = r#"
        type Control = "leave" | "cancel"
        type Line = Exit({ control: Control }) | Sale({ slot: i32 })

        function test(): i32
            local direct: Line = Exit({ control = "cancel" })
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_tagged_union_payload_bound_to_a_local_first() {
    let source = r#"
        type Control = "leave" | "cancel"
        type Line = Exit({ control: Control }) | Sale({ slot: i32 })

        function test(): i32
            local payload: { control: Control } = { control = "cancel" }
            local bound: Line = Exit(payload)
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_tagged_union_payload_with_nullable_field() {
    let source = r#"
        type Control = "leave" | "cancel"
        type Line = Exit({ note: Control? }) | Sale({ slot: i32 })

        function test(): i32
            local blank: Line = Exit({ note = nil })
            local noted: Line = Exit({ note = "leave" })
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn resolves_named_types_inside_a_variant_payload() {
    let source = r#"
        type Slot = { control: i32 }
        type Line = Exit(Slot) | Sale({ slot: i32 })

        function test(): i32
            local slot: Slot = { control = 3 }
            local line: Line = Exit(slot)
            if Exit(p) = line then
                return p.control
            end
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_non_member_string_in_a_variant_payload_field() {
    let source = r#"
        type Control = "leave" | "cancel"
        type Line = Exit({ control: Control }) | Sale({ slot: i32 })

        function test(): i32
            local bad: Line = Exit({ control = "nope" })
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "\"nope\" is not a member of \"leave\" | \"cancel\""
    );
}

#[test]
fn type_checks_tagged_union_constructor_through_nullable_return_type() {
    let source = r#"
        type Goods = Upgrade({ kind: i32 }) | Spell({ kind: i32 })

        function find(want: bool): Goods?
            if want then
                return Upgrade({ kind = 1 })
            end
            return nil
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_tagged_union_constructor_through_nullable_local_and_argument() {
    let source = r#"
        type Goods = Upgrade({ kind: i32 }) | Spell({ kind: i32 })

        function accept(goods: Goods?): i32
            if goods ~= nil then
                return 1
            end
            return 0
        end

        function test(): i32
            local slot: Goods? = Upgrade({ kind = 1 })
            slot = nil
            slot = Spell({ kind = 2 })
            return accept(slot) + accept(Upgrade({ kind = 3 })) + accept(nil)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_unknown_tag_against_nullable_tagged_union() {
    let source = r#"
        type Goods = Upgrade({ kind: i32 }) | Spell({ kind: i32 })

        function find(): Goods?
            return Trinket({ kind = 1 })
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "unknown name 'Trinket'");
}

#[test]
fn rejects_variant_test_on_unnarrowed_nullable_tagged_union() {
    let source = r#"
        type Goods = Upgrade({ kind: i32 }) | Spell({ kind: i32 })

        function test(goods: Goods?): bool
            return goods is Upgrade
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "type Goods? has no tagged variant 'Upgrade'"
    );
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
fn allows_coroutine_resume_unknown_payload_with_implicit_unbox() {
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
    // `unknown` implicitly unboxes to concrete types with a runtime-checked
    // cast, so binding the resume payload to i32 type-checks.
    super::type_check(&program).expect("implicit unbox should type check");
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
fn type_checks_print_with_mixed_arguments_and_multi_spread() {
    let source = r#"
        function multi(): (i32, string)
            return 7, "mid"
        end

        function check(): unit
            print()
            print(42)
            print("a", 1, true, nil)
            print(multi())
            print("start", multi(), 2.5)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn adjusts_non_final_multi_value_to_first_value_in_expression_lists() {
    let source = r#"
        function multi(): (i32, i32)
            return 2, 5
        end

        function add(a: i32, b: i32): i32
            return a + b
        end

        function pair(): (i32, i32)
            return multi(), 100
        end

        function check(): unit
            local sum = add(multi(), 100)
            local a, b = multi(), 100
            local p, q, r = 100, multi()
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_return_list_relying_on_non_final_multi_expansion() {
    let source = r#"
        function multi(): (i32, i32)
            return 2, 5
        end

        function triple(): (i32, i32, i32)
            return multi(), 100
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "return expects 3 values, got 2");
}

#[test]
fn rejects_print_of_unit_value() {
    let source = r#"
        function noop(): unit
        end

        function check(): unit
            print(noop())
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "print cannot convert a unit value");
}

#[test]
fn type_checks_bare_return_in_unit_function() {
    let source = r#"
        function check(x: i32): unit
            if x > 0 then
                return
            end
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
fn type_checks_scalar_numeric_equality_with_single_value_multi_result() {
    let source = r#"
        function clipped_negative_end(): bool
            return string.byte("\n\n", 2, -1) == 10
        end

        function exact_range(): bool
            return string.byte("\n\n", 2, 2) == 10
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
fn type_checks_tostring_after_untyped_multi_binding() {
    let source = r#"
        function pair(): (i32, i32)
            return 1, 2
        end

        function entry(): string
            local a, b = pair()
            return tostring(a) .. tostring(b)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_tostring_after_nullable_field_narrowing() {
    let source = r#"
        type Options = { children: {string}? }

        function entry(opts: Options?): string
            if opts == nil then
                return "none"
            end
            if opts.children ~= nil then
                return tostring(#opts.children)
            end
            return "empty"
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_concat_with_string_and_numeric_operands() {
    let source = r#"
        function entry(a: i32, b: f64): string
            local left: string = a .. " apples"
            local right: string = "value=" .. b
            local compound: string = "count="
            compound ..= a
            return left .. right .. compound
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rewrites_tostring_metamethod_to_resolved_method_call() {
    let source = r#"
        enum SpellKind { Firebolt, FreezeRay }

        function SpellKind:__tostring(): string
            if self == SpellKind.Firebolt then return "Firebolt" end
            return "Freeze Ray"
        end

        function entry(kind: SpellKind): string
            return tostring(kind)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");
    let entry = typed
        .functions
        .iter()
        .find(|function| function.name.to_string() == "entry")
        .expect("entry function");
    assert!(matches!(
        &entry.body[0],
        Stmt::Return(Expr::MethodCall {
            name,
            resolved_name: Some(resolved_name),
            args,
            ..
        }) if name == "__tostring" && resolved_name == "SpellKind.__tostring" && args.is_empty()
    ));
}

#[test]
fn resolves_concat_metamethod() {
    let source = r#"
        type Label = { value: string }

        function Label:__concat(suffix: string): string
            return self.value .. suffix
        end

        function entry(label: Label): string
            return label .. "!"
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");
    let entry = typed
        .functions
        .iter()
        .find(|function| function.name.to_string() == "entry")
        .expect("entry function");
    assert!(matches!(
        &entry.body[0],
        Stmt::Return(Expr::Binary {
            op: BinaryOp::Concat,
            resolved_name: Some(resolved_name),
            ..
        }) if resolved_name == "Label.__concat"
    ));
}

#[test]
fn type_checks_dynamic_type_and_number_builtins() {
    let source = r#"
        function entry(a: i32): string
            local boxed: unknown = a
            local tx: string = type(a)
            local tu: string = typeof(boxed)
            local n: f64 = tonumber("ff", 16)
            return tx .. tu .. tostring(n)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_tostring_for_reference_inputs() {
    let source = r#"
        function id(x: i32): i32
            return x
        end

        function entry(xs: {i32}): string
            return tostring(xs) .. tostring(nil) .. tostring(id)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_tostring_for_unit_inputs() {
    let source = r#"
        function nothing(): unit
        end

        function entry(): string
            return tostring(nothing())
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "tostring cannot convert a unit value");
}

#[test]
fn type_checks_table_concat_with_separator_and_bounds() {
    let source = r#"
        function entry(words: {string}, first: i32, last: i32): string
            local a: string = table.concat(words, ", ")
            local b: string = table.concat(words)
            local c: string = table.concat(words, ", ", first, last)
            return a .. b .. c
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
fn type_checks_select_with_typed_array() {
    let source = r#"
        function entry(): i32
            return select('#', {1::i32, 2::i32, 3::i32})
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_select_with_different_typed_array() {
    let source = r#"
        function entry(): i32
            local nums: {f64} = {1.0, 2.0, 3.0}
            return select('#', nums)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_select_with_assert_like_user() {
    let source = r#"
        function entry(): bool
            return select('#', {1, 2, 3}) == 3
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_select_with_non_array() {
    let source = r#"
        function entry(): i32
            return select('#', "not an array")
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "select expects an array, got string");
}

#[test]
fn type_checks_select_integration_test() {
    let source = r#"
        function test_select_with_numbers(): bool
            return select('#', {1, 2, 3}) == 3
        end

        function test_select_with_integers(): bool
            return select('#', {1::i32, 2::i32, 3::i32}) == 3::i32
        end

        function test_select_with_floats(): bool
            return select('#', {1.0, 2.0, 3.0}) == 3::i32
        end

        function entry(): bool
            local test1: bool = test_select_with_numbers()
            local test2: bool = test_select_with_integers() 
            local test3: bool = test_select_with_floats()
            return test1 and test2 and test3
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
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

#[test]
fn narrows_pcall_payload_in_if_branches() {
    let source = r#"
        function entry(): f64
            local ok, value = pcall(function(): f64
                return 42.0
            end)
            if ok then
                return value + 1.0
            else
                assert(false, value)
            end
            return 0.0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("pcall payload should narrow in both branches");
}

#[test]
fn preserves_recursive_local_function_scope_during_multi_binding_annotation() {
    let source = r#"
        function entry(): f64
            local function recurse(depth: f64): f64
                if depth == 0 then
                    return 1
                end
                local ok, value = pcall(recurse, depth - 1)
                assert(ok)
                return value
            end
            return recurse(2)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("recursive local scope should survive annotation");
}

#[test]
fn narrows_pcall_payload_after_assert() {
    let source = r#"
        function entry(): f64
            local ok, value = pcall(function(): f64
                return 42.0
            end)
            assert(ok)
            return value + 1.0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("pcall payload should narrow after assert(ok)");
}

#[test]
fn narrows_pcall_error_after_negated_assert() {
    let source = r#"
        function entry(): string
            local ok, err = pcall(function(): f64
                error("boom")
                return 1.0
            end)
            assert(not ok)
            return err
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("pcall error payload should narrow after assert(not ok)");
}

#[test]
fn narrows_pcall_payload_after_diverging_branch() {
    let source = r#"
        function entry(): f64
            local ok, value = pcall(function(): f64
                return 42.0
            end)
            if not ok then
                return -1.0
            end
            return value
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("pcall payload should stay narrowed after early return");
}

#[test]
fn pcall_narrowing_severed_by_reassignment() {
    let source = r#"
        function entry(): f64
            local ok, value = pcall(function(): f64
                return 1.0
            end)
            ok = true
            if ok then
                return value + 1.0
            end
            return 0.0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program)
        .expect_err("reassigning the discriminant must sever pcall narrowing");
}

#[test]
fn pcall_narrowing_severed_by_shadowing() {
    let source = r#"
        function entry(): f64
            local ok, value = pcall(function(): f64
                return 1.0
            end)
            local value = "hello"
            if ok then
                return value + 1.0
            end
            return 0.0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect_err("shadowing the payload must sever pcall narrowing");
}

#[test]
fn assert_narrows_variant_for_rest_of_scope() {
    let source = r#"
        type Either = Left(i32) | Right(f64)

        function entry(either: Either): i32
            assert(either is Left)
            return either.value
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("assert(x is Variant) should narrow for the rest of scope");
}

#[test]
fn rejects_variant_test_ruled_out_by_assert() {
    let source = r#"
        type Either = Left(i32) | Right(f64)

        function entry(either: Either): bool
            assert(either is Left)
            return either is Right
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program)
        .expect_err("testing a variant the assert ruled out must be a type error");
    assert_eq!(
        error.to_string(),
        "type Left(i32) has no tagged variant 'Right'"
    );
}

#[test]
fn assert_narrows_nullable_for_rest_of_scope() {
    let source = r#"
        function take(value: string): i32
            return 20
        end

        function entry(value: string?): i32
            assert(value ~= nil)
            return take(value)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("assert(x ~= nil) should narrow for the rest of scope");
}

#[test]
fn resolves_declared_import_overloads_by_parameter_type() {
    let source = r#"
        declare function pick(x: f32): f32
        declare function pick(x: f64): f64

        function narrow(x: f32): f32
            return pick(x)
        end

        function wide(x: f64): f64
            return pick(x)
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");

    // The two declarations were renamed to unique internal names while the
    // host-facing name stays `pick` for both.
    let pick_imports: Vec<_> = typed
        .declared_imports
        .iter()
        .filter(|declared| declared.host_name == "pick")
        .collect();
    assert_eq!(pick_imports.len(), 2);
    assert_eq!(pick_imports[0].name, "pick$overload0");
    assert_eq!(pick_imports[1].name, "pick$overload1");

    // Each call site was rewritten to the overload selected from its
    // argument type.
    let callee_name = |function_name: &str| {
        let function = typed
            .functions
            .iter()
            .find(|function| function.name.to_string() == function_name)
            .expect("function should exist");
        let Stmt::Return(Expr::Call { callee, .. }) = &function.body[0] else {
            panic!("expected a returned call in {function_name}");
        };
        let Expr::Name(name, _, _) = callee.as_ref() else {
            panic!("expected a name callee in {function_name}");
        };
        name.clone()
    };
    assert_eq!(callee_name("narrow"), "pick$overload0");
    assert_eq!(callee_name("wide"), "pick$overload1");
}

#[test]
fn resolves_declared_method_overloads_by_arity() {
    let source = r#"
        type Ctx = extern
        declare function get_ctx(): Ctx
        declare function Ctx:fill(): unit
        declare function Ctx:fill(rule: string): unit

        function paint(): unit
            local c: Ctx = get_ctx()
            c:fill()
            c:fill("evenodd")
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");

    let paint = typed
        .functions
        .iter()
        .find(|function| function.name.to_string() == "paint")
        .expect("paint should exist");
    let resolved = paint.body[1..]
        .iter()
        .map(|stmt| {
            let Stmt::Expr(Expr::MethodCall { resolved_name, .. }) = stmt else {
                panic!("expected method call statements");
            };
            resolved_name.clone().expect("method call should resolve")
        })
        .collect::<Vec<_>>();
    assert_eq!(resolved, ["Ctx.fill$overload0", "Ctx.fill$overload1"]);
}

#[test]
fn overloaded_call_literal_prefers_exact_match_over_coercion() {
    let source = r#"
        declare function pick(x: f32): f32
        declare function pick(x: f64): f64

        function literal(): f64
            return pick(1.5)
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");
    let literal = typed
        .functions
        .iter()
        .find(|function| function.name.to_string() == "literal")
        .expect("literal should exist");
    let Stmt::Return(Expr::Call { callee, .. }) = &literal.body[0] else {
        panic!("expected returned call");
    };
    let Expr::Name(name, _, _) = callee.as_ref() else {
        panic!("expected name callee");
    };
    // An unsuffixed literal defaults to f64, which matches the f64 overload
    // exactly; the f32 overload would need a literal coercion.
    assert_eq!(name, "pick$overload1");
}

#[test]
fn rejects_ambiguous_overloaded_call() {
    let source = r#"
        declare function pick(x: i64): i64
        declare function pick(x: f64): f64

        function ambiguous(x: i32): f64
            return pick(x)::f64
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "ambiguous call to overloaded function 'pick': candidates (i64) and (f64) match equally well"
    );
}

#[test]
fn rejects_overloaded_call_without_matching_types() {
    let source = r#"
        declare function pick(x: f32): f32
        declare function pick(x: f64): f64

        function bad(): f64
            return pick("nope")
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "no overload of 'pick' matches argument types (string); available overloads: (f32), (f64)"
    );
}

#[test]
fn attaches_argument_span_to_call_coercion_diagnostic() {
    let source = r#"
        declare function accept(value: i32): unit

        function bad(): unit
            accept("wrong")
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    let start = source
        .find("\"wrong\"")
        .expect("argument should be present") as u32;

    assert_eq!(error.to_string(), "cannot implicitly convert string to i32");
    assert_eq!(
        error.span(),
        Some(waluau_ast::Span {
            start,
            end: start + "\"wrong\"".len() as u32,
        })
    );
    assert_eq!(error.file_path(), Some("source"));
}

#[test]
fn rejects_overloaded_call_without_matching_arity() {
    let source = r#"
        type Ctx = extern
        declare function get_ctx(): Ctx
        declare function Ctx:fill(): unit
        declare function Ctx:fill(rule: string): unit

        function bad(): unit
            get_ctx():fill("evenodd", "extra")
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "no overload of 'Ctx.fill' accepts 3 arguments; available overloads: (Ctx), (Ctx, string)"
    );
}

#[test]
fn rejects_overloaded_function_used_as_value() {
    let source = r#"
        declare function pick(x: f32): f32
        declare function pick(x: f64): f64

        function bad(): f64
            local alias = pick
            return alias(1.0)
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "overloaded host function 'pick' cannot be used as a value; call it directly"
    );
}

#[test]
fn deduplicates_identical_declared_import_redeclarations() {
    let source = r#"
        declare function ping(x: i32): i32
        declare function ping(x: i32): i32

        function go(x: i32): i32
            return ping(x)
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");
    let pings: Vec<_> = typed
        .declared_imports
        .iter()
        .filter(|declared| declared.host_name == "ping")
        .collect();
    assert_eq!(pings.len(), 1, "identical re-declarations should collapse");
    assert_eq!(pings[0].name, "ping");
}

#[test]
fn rejects_overloads_with_identical_parameters_and_conflicting_returns() {
    let source = r#"
        declare function ping(x: i32): i32
        declare function ping(x: i32): f64
    "#;

    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "conflicting declarations of host function 'ping': overloads must differ in parameter \
         types, but two declarations share the parameter list and disagree on the return type \
         or host name"
    );
}

#[test]
fn numeric_for_untyped_literal_bounds_adopt_the_typed_bound_type() {
    // `0` carries no numeric type of its own, so the loop variable adopts the
    // i32 type of `#a - 1` (mirroring untyped literals in binary expressions)
    // instead of defaulting to f64.
    let source = r#"
        local a: {i32} = {1, 2, 3}
        local sum: i32 = 0
        for i = 0, #a - 1 do
            local index: i32 = i
            sum += a[index]
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check_and_infer(&program).expect("type check should succeed");
}

#[test]
fn countdown_numeric_for_adopts_typed_bound_type_for_negative_literal_step() {
    // The `-1` step is an untyped literal behind unary minus; it adopts the
    // i32 loop type instead of dragging the loop to f64.
    let source = r#"
        local a: {i32} = {1, 2, 3}
        local sum: i32 = 0
        for i = #a - 1, 0, -1 do
            local index: i32 = i
            sum += a[index]
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check_and_infer(&program).expect("type check should succeed");
}

#[test]
fn rejects_fractional_literal_bound_in_an_integer_loop() {
    let source = r#"
        local a: {i32} = {1, 2, 3}
        local sum: i32 = 0
        for i = 0.5, #a - 1 do
            sum += 1
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check_and_infer(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "numeric literal must be an integer for i32"
    );
}

#[test]
fn infers_trailing_vararg_returns_as_variadic_packs() {
    let source = r#"
        function only(...)
            return ...
        end

        function prefixed(a, ...)
            return a, ...
        end
    "#;

    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");
    assert_eq!(
        typed.functions[0].return_type,
        Some(Type::Variadic(Arc::new(Type::Unknown)))
    );
    assert_eq!(
        typed.functions[1].return_type,
        Some(Type::Multi(vec![
            Type::Unknown,
            Type::Variadic(Arc::new(Type::Unknown)),
        ]))
    );
}

#[test]
fn collects_type_errors_across_independent_functions() {
    let source = r#"
        function first(x: i32): i32
            if x then
                return x
            end
            return x
        end

        function second(x: i32): bool
            return x + 1
        end

        function healthy(x: i32): i32
            return x + 1
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let errors = super::type_check_and_infer_collect(&program).expect_err("type check should fail");
    assert_eq!(
        errors.len(),
        2,
        "expected one error per broken function: {errors:?}"
    );
    assert_eq!(errors[0].to_string(), "if condition must be bool");
    assert!(
        errors[1].to_string().contains("bool"),
        "second error should be the return mismatch: {}",
        errors[1]
    );
}

#[test]
fn collects_multiple_statement_errors_within_one_function() {
    let source = r#"
        function broken(x: i32): i32
            local a: bool = x
            local b: bool = x
            return x
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let errors = super::type_check_and_infer_collect(&program).expect_err("type check should fail");
    assert_eq!(
        errors.len(),
        2,
        "expected one error per failing statement: {errors:?}"
    );
}

#[test]
fn failed_binding_does_not_cascade_into_later_statements() {
    let source = r#"
        function broken(x: i32): i32
            local flag: bool = x
            if flag then
                return 1
            end
            return x
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let errors = super::type_check_and_infer_collect(&program).expect_err("type check should fail");
    // Only the bad initializer errors; `flag` falls back to unknown so the
    // `if flag` use does not produce a second unknown-variable error.
    assert_eq!(errors.len(), 1, "unexpected cascade: {errors:?}");
}

#[test]
fn single_error_wrapper_reports_first_collected_error() {
    let source = r#"
        function broken(x: i32): bool
            return x
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let collected = super::type_check_and_infer_collect(&program).expect_err("collect should fail");
    let single = super::type_check_and_infer(&program).expect_err("wrapper should fail");
    assert_eq!(collected[0], single);
}

#[test]
fn statement_level_errors_carry_spans() {
    let source = concat!(
        "function f(x: i32): i32\n",
        "    if x then\n",
        "        return x\n",
        "    end\n",
        "    while x do\n",
        "        return x\n",
        "    end\n",
        "    return x\n",
        "end\n",
    );
    let program = parse(source).expect("parse should succeed");
    let errors = super::type_check_and_infer_collect(&program).expect_err("type check should fail");
    assert_eq!(errors.len(), 2, "{errors:?}");
    for error in &errors {
        assert!(
            error.span().is_some(),
            "condition error should carry the condition's span: {error:?}"
        );
    }
    // Distinct conditions -> distinct spans.
    assert_ne!(errors[0].span(), errors[1].span());
}

#[test]
fn missing_return_points_at_the_last_statement() {
    let source = "function f(x: i32): i32\n    local y: i32 = x + 1\nend\n";
    let program = parse(source).expect("parse should succeed");
    let errors = super::type_check_and_infer_collect(&program).expect_err("type check should fail");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].to_string().contains("missing a return"));
    assert!(errors[0].span().is_some(), "{:?}", errors[0]);
}

#[test]
fn string_literal_union_accepts_member_literals() {
    let source = r#"
        type CardColor = "red" | "black"

        function flip(color: CardColor): CardColor
            if color == "red" then
                return "black"
            end
            return "red"
        end

        function entry(): bool
            local color: CardColor = "red"
            return flip(color) == "black"
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn string_literal_union_rejects_non_member_literals() {
    let source = r#"
        type CardColor = "red" | "black"

        function entry(): CardColor
            return "banana"
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert!(
        error
            .to_string()
            .contains("\"banana\" is not a member of \"red\" | \"black\""),
        "{error}"
    );
}

#[test]
fn string_literal_union_never_converts_to_or_from_string() {
    let assignment = r#"
        type CardColor = "red" | "black"

        function entry(name: string): CardColor
            return name
        end
    "#;
    let program = parse(assignment).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert!(
        error
            .to_string()
            .contains("cannot implicitly convert string to CardColor"),
        "{error}"
    );

    let cast_in = r#"
        type CardColor = "red" | "black"

        function entry(name: string): CardColor
            return (name :: CardColor)
        end
    "#;
    let program = parse(cast_in).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert!(
        error
            .to_string()
            .contains("cannot implicitly convert string to CardColor"),
        "{error}"
    );

    let cast_out = r#"
        type CardColor = "red" | "black"

        function entry(color: CardColor): string
            return (color :: string)
        end
    "#;
    let program = parse(cast_out).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert!(
        error
            .to_string()
            .contains("cannot implicitly convert CardColor to string"),
        "{error}"
    );
}

#[test]
fn string_literal_union_equality_rejects_plain_strings_and_non_members() {
    let plain = r#"
        type CardColor = "red" | "black"

        function entry(color: CardColor, name: string): bool
            return color == name
        end
    "#;
    let program = parse(plain).expect("parse should succeed");
    super::type_check(&program).expect_err("comparing a union to a plain string should fail");

    let non_member = r#"
        type CardColor = "red" | "black"

        function entry(color: CardColor): bool
            return color == "banana"
        end
    "#;
    let program = parse(non_member).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert!(
        error
            .to_string()
            .contains("\"banana\" is not a member of \"red\" | \"black\""),
        "{error}"
    );
}

#[test]
fn literal_union_aliases_are_nominal() {
    let source = r#"
        type A = "x" | "y"
        type B = "x" | "y"

        function entry(a: A): B
            return a
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert!(
        error
            .to_string()
            .contains("cannot implicitly convert A to B"),
        "{error}"
    );
}

#[test]
fn nullable_literal_union_takes_member_literals() {
    let source = r#"
        type CardColor = "red" | "black"

        function entry(): bool
            local maybe: CardColor? = nil
            maybe = "red"
            return maybe ~= nil
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_declarations_resolve_independently_of_declaration_order() {
    // A record may reference an alias declared after it; the reference must
    // resolve to the real type, not a stale placeholder anchor.
    let union_after_use = r#"
        type Beat = { index: BeatIndex, part: f64 }
        type BeatIndex = "flight" | "land"

        function entry(clock: f64): bool
            local beat: Beat = { index = "flight", part = clock }
            return beat.index == "flight"
        end
    "#;
    let program = parse(union_after_use).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");

    let enum_after_use = r#"
        type Wrap = { color: CardColor }
        enum CardColor { red, black }

        function entry(): bool
            local wrapped: Wrap = { color = CardColor.red }
            return wrapped.color == CardColor.red
        end
    "#;
    let program = parse(enum_after_use).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");

    // A chain of forward references resolves all the way down.
    let chain = r#"
        type Outer = { middle: Middle }
        type Middle = { kind: Kind }
        type Kind = "on" | "off"

        function entry(): bool
            local outer: Outer = { middle = { kind = "on" } }
            return outer.middle.kind == "on"
        end
    "#;
    let program = parse(chain).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn mutually_recursive_records_resolve_in_either_declaration_order() {
    for source in [
        r#"
        type Parent = { children: {Child} }
        type Child = { parent: Parent? }

        function entry(): i32
            local parent: Parent = { children = {} }
            return #parent.children
        end
        "#,
        r#"
        type Child = { parent: Parent? }
        type Parent = { children: {Child} }

        function entry(): i32
            local parent: Parent = { children = {} }
            return #parent.children
        end
        "#,
    ] {
        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }
}

#[test]
fn unguarded_forward_reference_cycle_is_rejected() {
    // A cycle with no record/array boundary has no finite anchor; resolving
    // on demand must report it rather than silently accepting a placeholder.
    let source = r#"
        type First = Second?
        type Second = First?
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert!(
        error.to_string().contains("cyclic type declaration"),
        "{error}"
    );
}

#[test]
fn resolves_record_types_with_self_method_fields() {
    // A record type whose field carries a `self` receiver parses and
    // resolves.
    let source = r#"
        type Op = { exec: (self, a: i32, b: i32) -> i32 }

        function describe(op: Op?): bool
            return op == nil
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn self_method_fields_accept_bound_closures() {
    // A method slot stores a *bound* method: a closure taking the self-less
    // parameters. A manual vtable therefore constructs directly, and both
    // call syntaxes provide the self-less arguments.
    let source = r#"
        type Op = { exec: (self, a: i32, b: i32) -> i32 }

        local op: Op = {
            exec = function(a: i32, b: i32): i32
                return a + b
            end,
        }

        function run_dot(op: Op): i32
            return op.exec(1, 2)
        end

        function run_colon(op: Op): i32
            return op:exec(1, 2)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("bound-method construction and calls should check");
}

#[test]
fn self_method_fields_reject_mismatched_values() {
    // A value that is not a function of the self-less shape cannot fill a
    // method slot; the diagnostic points at conformance declarations.
    let source = r#"
        type Op = { exec: (self, a: i32, b: i32) -> i32 }

        local op: Op = {
            exec = function(a: i32): i32
                return a
            end,
        }
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("construction must fail");
    assert!(
        error.to_string().contains("declares conformance"),
        "unexpected diagnostic: {error}"
    );
}

#[test]
fn conformance_satisfied_by_method_declaration() {
    // The full accepted end-to-end declaration, with the method textually
    // after the conformance declaration.
    let source = r#"
        type Op = { exec: (self, a: i32, b: i32) -> i32 }
        type Add = Op & {}

        function Add:exec(a: i32, b: i32): i32
            return a + b
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("conformance via a method declaration should check");
}

#[test]
fn conformance_satisfied_with_inferred_return_type() {
    // Conformance is checked after whole-program return-type inference, so
    // the implementing method may leave its return type to inference.
    let source = r#"
        type Op = { exec: (self, a: i32, b: i32) -> i32 }
        type Add = Op & {}

        function Add:exec(a: i32, b: i32)
            return a + b
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("an inferred method return type should satisfy conformance");
}

#[test]
fn conformance_satisfied_by_record_fields() {
    // A plain function field and a receiver-typed field both satisfy their
    // interface slots as record fields; the receiver-typed field references
    // the conforming type from inside its own declaration (recursion
    // anchor), which nominal matching identifies by name.
    let source = r#"
        type Op = { exec: (self, a: i32) -> i32, helper: (i32) -> i32 }
        type Add = Op & { exec: (Add, i32) -> i32, helper: (i32) -> i32 }
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("conformance via record fields should check");
}

#[test]
fn conformance_requires_declared_non_function_fields() {
    let source = r#"
        type Op = { exec: (self) -> i32, count: i32 }
        type Add = Op & { count: i32 }

        function Add:exec(): i32
            return self.count
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("a declared data field should satisfy conformance");

    let source = r#"
        type Op = { count: i32 }
        type Add = Op & {}
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("a missing data field must fail");
    assert!(
        error
            .to_string()
            .contains("does not conform to interface 'Op': missing field 'count' with type i32"),
        "unexpected diagnostic: {error}"
    );
}

#[test]
fn conformance_missing_method_is_reported() {
    let source = r#"
        type Op = { exec: (self, a: i32, b: i32) -> i32 }
        type Add = Op & {}
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("a missing method must fail");
    let rendered = error.to_string();
    assert!(
        rendered.contains("type 'Add' does not conform to interface 'Op': missing method 'exec'")
            && rendered.contains("(Add, i32, i32) -> i32"),
        "unexpected diagnostic: {error}"
    );
}

#[test]
fn conformance_mismatched_method_signature_is_reported() {
    let source = r#"
        type Op = { exec: (self, a: i32, b: i32) -> i32 }
        type Add = Op & {}

        function Add:exec(a: i32): i32
            return a
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("a mismatched method must fail");
    let rendered = error.to_string();
    assert!(
        rendered.contains("method 'exec' has type (Add, i32) -> i32")
            && rendered.contains("requires (Add, i32, i32) -> i32"),
        "unexpected diagnostic: {error}"
    );
}

#[test]
fn conformance_self_substitutes_to_the_conforming_type() {
    // The receiver slot must be the conforming type itself: a field typed
    // with the interface as its receiver does not satisfy the method.
    let source = r#"
        type Op = { exec: (self, a: i32) -> i32 }
        type Add = Op & { exec: (Op, i32) -> i32 }
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("a wrong receiver type must fail");
    let rendered = error.to_string();
    assert!(
        rendered.contains("field 'exec' has type (Op, i32) -> i32")
            && rendered.contains("requires (Add, i32) -> i32"),
        "unexpected diagnostic: {error}"
    );

    // A method declared on a different type does not satisfy the obligation.
    let source = r#"
        type Op = { exec: (self, a: i32) -> i32 }
        type Add = Op & {}
        type Sub = {}

        function Sub:exec(a: i32): i32
            return a
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("another type's method must not satisfy");
    assert!(
        error.to_string().contains("missing method 'exec'"),
        "unexpected diagnostic: {error}"
    );
}

#[test]
fn conformance_to_unknown_or_non_record_interfaces_is_reported() {
    let source = "type Add = Op & {}";
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("an unknown interface must fail");
    assert!(
        error
            .to_string()
            .contains("unknown interface 'Op' in the conformance declaration of type 'Add'"),
        "unexpected diagnostic: {error}"
    );

    let source = r#"
        type Num = i32
        type Add = Num & {}
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("a non-record interface must fail");
    assert!(
        error
            .to_string()
            .contains("an interface must be a record type, got i32"),
        "unexpected diagnostic: {error}"
    );
}

#[test]
fn conformance_coercion_rewrites_annotated_local() {
    // `local op: Op = add` desugars to a call to the generated constructor
    // `__conform$Add$Op`, and both call syntaxes dispatch through the
    // interface record.
    let source = r#"
        type Op = { exec: (self, a: i32, b: i32) -> i32 }
        type Add = Op & {}

        function Add:exec(a: i32, b: i32): i32
            return a + b
        end

        local add: Add = {}
        local op: Op = add
        assert(op.exec(2, 3) == 5)
        assert(op:exec(2, 3) == 5)
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("coercion should check");
    let init = typed
        .functions
        .iter()
        .find(|function| function.name.to_string() == "__waluau_top_level_init")
        .expect("top-level init function");
    let rendered = format!("{:?}", init.body);
    assert!(
        rendered.contains("__conform$Add$Op"),
        "expected a constructor call in the rewritten body: {rendered}"
    );
    assert!(
        typed
            .functions
            .iter()
            .any(|function| function.name.to_string() == "__conform$Add$Op"),
        "expected the generated constructor function"
    );
}

#[test]
fn conformance_coercion_covers_arguments_returns_and_casts() {
    let source = r#"
        type Op = { exec: (self, a: i32, b: i32) -> i32 }
        type Add = Op & {}

        function Add:exec(a: i32, b: i32): i32
            return a + b
        end

        function run(op: Op): i32
            return op:exec(2, 3)
        end

        function as_op(add: Add): Op
            return add
        end

        local add: Add = {}
        assert(run(add) == 5)
        assert(run(add :: Op) == 5)
        assert(as_op(add):exec(2, 3) == 5)
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("argument, return, and cast coercions should check");
}

#[test]
fn conformance_coercion_dispatches_polymorphically() {
    // Two conforming types flow through the same interface-typed function.
    let source = r#"
        type Op = { exec: (self, a: i32, b: i32) -> i32 }
        type Add = Op & {}
        type Mul = Op & {}

        function Add:exec(a: i32, b: i32): i32
            return a + b
        end

        function Mul:exec(a: i32, b: i32): i32
            return a * b
        end

        function run(op: Op): i32
            return op:exec(2, 3)
        end

        local add: Add = {}
        local mul: Mul = {}
        assert(run(add) == 5)
        assert(run(mul) == 6)
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("polymorphic dispatch should check");
}

#[test]
fn conformance_coercion_infers_unannotated_returns() {
    // A function without a return annotation may still return a coerced
    // value: the rewrite runs before return-type inference.
    let source = r#"
        type Op = { exec: (self, a: i32, b: i32) -> i32 }
        type Add = Op & {}

        function Add:exec(a: i32, b: i32): i32
            return a + b
        end

        function make_op(add: Add)
            return add :: Op
        end

        local add: Add = {}
        assert(make_op(add):exec(2, 3) == 5)
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("inferred interface return should check");
}

#[test]
fn coercing_a_non_conforming_type_is_an_error() {
    let source = r#"
        type Op = { exec: (self, a: i32, b: i32) -> i32 }
        type Plain = {}

        local plain: Plain = {}
        local op: Op = plain
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("non-conforming coercion must fail");
    assert!(
        error.to_string().contains("missing record field 'exec'"),
        "unexpected diagnostic: {error}"
    );
}

#[test]
fn function_type_parameter_names_do_not_affect_checking() {
    let source = r#"
        local add: (first: i32, second: i32) -> i32 = function(x: i32, y: i32): i32
            return x + y
        end
        local unnamed: (i32, i32) -> i32 = add
        assert(unnamed(1, 2) == 3)
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("names must not affect checking");
}

// ---------------------------------------------------------------------------
// Interface brand checks and downcasts (wrapper identity + surface forms)
// ---------------------------------------------------------------------------

/// Source shared by the brand tests: two conforming types with identical
/// layouts behind one interface.
const SIBLING_CONFORMANCE: &str = r#"
    type Op = { exec: (self, a: i32, b: i32) -> i32 }
    type Add = Op & { bias: i32 }
    type Mul = Op & { bias: i32 }

    function Add:exec(a: i32, b: i32): i32
        return a + b + self.bias
    end

    function Mul:exec(a: i32, b: i32): i32
        return a * b + self.bias
    end
"#;

#[test]
fn conformance_brands_are_distinct_and_order_independent() {
    // Layout-identical siblings get distinct brands, and the assignment
    // depends only on the set of canonical names, not on declaration order
    // (stand-in for module link order).
    let forward = parse(SIBLING_CONFORMANCE).expect("parse should succeed");
    let reversed = parse(
        r#"
        type Mul = Op & { bias: i32 }
        type Add = Op & { bias: i32 }
        type Op = { exec: (self, a: i32, b: i32) -> i32 }

        function Add:exec(a: i32, b: i32): i32
            return a + b + self.bias
        end

        function Mul:exec(a: i32, b: i32): i32
            return a * b + self.bias
        end
    "#,
    )
    .expect("parse should succeed");
    let brands = super::conformance::conformance_brands(&forward);
    assert_eq!(brands.len(), 2);
    assert_ne!(brands["Add"], brands["Mul"]);
    assert_eq!(brands, super::conformance::conformance_brands(&reversed));
}

#[test]
fn conformance_wrappers_carry_brand_and_receiver_identity() {
    // The generated wrapper's interface record includes the hidden identity
    // field holding the concrete type's brand and the receiver reference,
    // and the interface declaration itself gains the (nullable, omittable)
    // field.
    let mut program = parse(SIBLING_CONFORMANCE).expect("parse should succeed");
    super::conformance::generate_conformance_wrappers(&mut program);
    let brands = super::conformance::conformance_brands(&program);

    let op = program
        .type_declarations
        .iter()
        .find(|decl| decl.name == "Op")
        .expect("Op declaration");
    let Type::Record(op_fields) = &op.ty else {
        panic!("Op must stay a record");
    };
    assert!(
        matches!(
            op_fields.get(super::conformance::META_FIELD),
            Some(Type::Nullable(_))
        ),
        "interface record must gain the hidden nullable identity field"
    );

    for concrete in ["Add", "Mul"] {
        let wrapper_name = super::conformance::conformance_wrapper_name(concrete, "Op");
        let wrapper = program
            .functions
            .iter()
            .find(|function| function.name.to_string() == wrapper_name)
            .expect("wrapper function");
        let [Stmt::Return(Expr::TableLiteral { fields, .. })] = wrapper.body.as_slice() else {
            panic!("wrapper body must return a table literal");
        };
        let meta = fields
            .iter()
            .find(|field| field.name == super::conformance::META_FIELD)
            .expect("wrapper record must fill the identity field");
        let Expr::TableLiteral {
            fields: meta_fields,
            ..
        } = &meta.value
        else {
            panic!("identity field must be a record literal");
        };
        let brand = meta_fields
            .iter()
            .find(|field| field.name == "brand")
            .expect("identity record must carry the brand");
        let Expr::Number(literal, _) = &brand.value else {
            panic!("brand must be a compile-time constant");
        };
        assert_eq!(literal.raw, brands[concrete].to_string());
        let receiver = meta_fields
            .iter()
            .find(|field| field.name == "receiver")
            .expect("identity record must carry the receiver");
        assert!(
            matches!(&receiver.value, Expr::Name(name, ..) if name == "__conform_receiver"),
            "receiver must be the original receiver reference, not a copy"
        );
        // The consuming forms exist alongside the wrapper.
        for helper in [
            super::conformance::conformance_check_name(concrete, "Op"),
            super::conformance::conformance_cast_name(concrete, "Op"),
        ] {
            assert!(
                program
                    .functions
                    .iter()
                    .any(|function| function.name.to_string() == helper),
                "missing generated helper {helper}"
            );
        }
    }
}

#[test]
fn interface_narrowing_and_hard_cast_type_check() {
    let source = format!(
        "{SIBLING_CONFORMANCE}
        function pick(op: Op): i32
            if Add(a) = op then
                return a.bias
            end
            local forced = op :: Mul
            return forced.bias
        end
    "
    );
    let program = parse(&source).expect("parse should succeed");
    super::type_check(&program).expect("interface narrowing and hard cast should check");
}

#[test]
fn interface_narrowing_rejects_non_conforming_target() {
    let source = format!(
        "{SIBLING_CONFORMANCE}
        type Other = {{ tag: i32 }}
        function pick(op: Op): i32
            if Other(o) = op then
                return o.tag
            end
            return 0
        end
    "
    );
    let program = parse(&source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("non-conforming target must fail");
    assert!(
        error.to_string().contains("neither a tagged-union variant"),
        "unexpected diagnostic: {error}"
    );
}

#[test]
fn interface_hard_cast_rejects_non_conforming_target() {
    let source = format!(
        "{SIBLING_CONFORMANCE}
        type Other = {{ tag: i32 }}
        function pick(op: Op): Other
            return op :: Other
        end
    "
    );
    let program = parse(&source).expect("parse should succeed");
    super::type_check(&program).expect_err("non-conforming hard cast must fail");
}

#[test]
fn plain_interface_literals_omit_the_identity_field() {
    // A plain interface literal never mentions the hidden field; the
    // nullable field is omittable, so construction still checks.
    let source = r#"
        type Op = { exec: (self, a: i32, b: i32) -> i32 }
        type Add = Op & {}

        function Add:exec(a: i32, b: i32): i32
            return a + b
        end

        local plain: Op = {
            exec = function(a: i32, b: i32): i32
                return a - b
            end,
        }
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("plain interface literals should still check");
}

#[test]
fn enum_pairs_loop_type_checks() {
    let source = r#"
        enum SpellKind { Firebolt, FreezeRay }

        function catalog(): string
            local out = ""
            for name, kind in pairs(SpellKind) do
                out = out .. name
                assert(kind == SpellKind.Firebolt or kind == SpellKind.FreezeRay)
            end
            return out
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("enum pairs loop should check");
}

#[test]
fn record_pairs_loop_type_checks() {
    let source = r#"
        function total(): i32
            local scores = { alice = 3::i32, bob = 5::i32 }
            local sum: i32 = 0
            for name, score in pairs(scores) do
                sum = sum + score
            end
            return sum
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("record pairs loop should check");
}

#[test]
fn record_pairs_name_only_loop_allows_mixed_field_types() {
    let source = r#"
        function keys(): string
            local mixed = { id = 7::i32, label = "seven" }
            local out = ""
            for key in pairs(mixed) do
                out = out .. key
            end
            return out
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("name-only record pairs loop should check");
}

#[test]
fn rejects_record_pairs_value_over_mixed_field_types() {
    let source = r#"
        function broken(): unit
            local mixed = { id = 7::i32, label = "seven" }
            for key, value in pairs(mixed) do
            end
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "pairs over a record requires every field to have the same type; 'id' is i32 but 'label' is string"
    );
}

#[test]
fn rejects_pairs_over_non_record_value() {
    let source = r#"
        function broken(): unit
            for key, value in pairs(42) do
            end
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "pairs(...) requires an enum type or a record value, got f64"
    );
}

#[test]
fn rejects_pairs_over_array_with_iteration_hint() {
    let source = r#"
        function broken(): unit
            local arr = {1, 2, 3}
            for key, value in pairs(arr) do
            end
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "pairs(...) requires an enum type or a record value; arrays iterate directly: `for i, v in arr`"
    );
}

#[test]
fn rejects_record_pairs_with_three_loop_variables() {
    let source = r#"
        function broken(): unit
            local scores = { alice = 3::i32 }
            for a, b, c in pairs(scores) do
            end
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "pairs for-in loop expects 1 or 2 loop variables, got 3"
    );
}

#[test]
fn iterator_protocol_loop_type_checks() {
    let source = r#"
        function iter(a: {i32}, i: i32): (i32?, i32)
            local n = i + 1
            if n < #a then
                return n, a[n]
            end
            return nil, 0
        end

        function total(nums: {i32}): i32
            local sum: i32 = 0
            for i, v in iter, nums, -1 do
                sum = sum + v
            end
            return sum
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("protocol loop should check");
}

#[test]
fn iterator_protocol_factory_loop_type_checks() {
    let source = r#"
        function iter(a: {i32}, i: i32): (i32?, i32)
            return nil, 0
        end

        function my_ipairs(a: {i32}): (({i32}, i32) -> (i32?, i32), {i32}, i32)
            return iter, a, -1
        end

        function total(nums: {i32}): i32
            local sum: i32 = 0
            for i, v in my_ipairs(nums) do
                sum = sum + v
            end
            return sum
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("factory protocol loop should check");
}

#[test]
fn next_over_record_and_array_type_checks() {
    let source = r#"
        function scan(): i32
            local scores = { alice = 3::i32, bob = 5::i32 }
            local total: i32 = 0
            for name, score in next, scores do
                total = total + score
            end
            local words = {"a", "b"}
            for i, v in next, words do
                total = total + i
            end
            return total
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("next loops should check");
}

#[test]
fn rejects_iterator_with_non_nullable_first_return() {
    let source = r#"
        function bad(a: {i32}, i: i32): (i32, i32)
            return i, 0
        end

        function scan(nums: {i32}): unit
            for i, v in bad, nums, 0 do
            end
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "the iterator function's first return value must be nullable — returning nil ends the loop"
    );
}

#[test]
fn rejects_omitted_control_start_for_non_nullable_control() {
    let source = r#"
        function iter(a: {i32}, i: i32): (i32?, i32)
            return nil, 0
        end

        function scan(nums: {i32}): unit
            for i, v in iter, nums do
            end
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "the iterator's control parameter i32 does not accept nil; pass an explicit control start: `for ... in f, state, start`"
    );
}

#[test]
fn rejects_binding_more_variables_than_the_iterator_produces() {
    let source = r#"
        function iter(a: {i32}, i: i32): (i32?, i32)
            return nil, 0
        end

        function scan(nums: {i32}): unit
            for a, b, c in iter, nums, 0 do
            end
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "for-in loop binds 3 variables, but the iterator produces 2 values"
    );
}

#[test]
fn rejects_next_over_non_iterable_value() {
    let source = r#"
        function scan(): unit
            for k, v in next, 42 do
            end
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "next iterates a record or an array, got f64"
    );
}

#[test]
fn rejects_non_nil_control_start_for_builtin_next() {
    let source = r#"
        function scan(): unit
            local scores = { a = 1::i32 }
            for k, v in next, scores, 5 do
            end
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "the control start for a `next` iterator must be nil"
    );
}

#[test]
fn shadowed_next_iterates_as_an_ordinary_protocol_function() {
    let source = r#"
        function step(a: {i32}, i: i32): (i32?, i32)
            return nil, 0
        end

        function scan(nums: {i32}): i32
            local next = step
            local total: i32 = 0
            for i, v in next, nums, -1 do
                total = total + v
            end
            return total
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("a shadowed next should check as a protocol iterator");
}

#[test]
fn type_checks_typed_json_pack_and_unpack() {
    let source = r#"
        type Payload = { name: string, values: {i32} }
        local payload: Payload = { name = "test", values = {1, 2} }
        local packed: string = json.pack(payload)
        local decoded, err = json.unpack<Payload>(packed)
        local message: string = err
        if decoded ~= nil then
            local name: string = decoded.name
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("typed JSON calls should type check");
}

#[test]
fn type_checks_unannotated_local_from_json_unpack() {
    let source = r#"
        type Payload = { name: string, values: {i32} }
        local payload: Payload = { name = "test", values = {1, 2} }
        local decoded = json.unpack<Payload>(json.pack(payload))
        if decoded ~= nil then
            local name: string = decoded.name
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program)
        .expect("an unannotated local should adjust json.unpack to its decoded value");
}

#[test]
fn type_checks_unannotated_local_from_multi_value_call() {
    let source = r#"
        local function pair(): (i32, string)
            return 1, "a"
        end
        local first = pair()
        local incremented: i32 = first + 1
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program)
        .expect("an unannotated local should adjust a multi-value call to its first value");
}

#[test]
fn json_unpack_requires_a_type_hint() {
    let program = parse("local value, err = json.unpack(\"{}\")").expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(
        error.to_string(),
        "json.unpack expects exactly 1 type argument, got 0"
    );
}

#[test]
fn json_rejects_function_values() {
    let program = parse(
        r#"
        local packed = json.pack(function(): unit end)
    "#,
    )
    .expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert!(
        error
            .to_string()
            .contains("json.pack does not support values of type")
    );
}

#[test]
fn checks_typed_vararg_call_arguments() {
    let source = r#"
        function sum(...: number): f64
            return select('#', ...) + 0
        end

        function entry(): f64
            return sum(1, "x")
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "cannot implicitly convert string to f64");
}

#[test]
fn typed_vararg_gives_select_the_element_type() {
    let source = r#"
        function first(...: string): string
            return select(1, ...)
        end

        function tail(...: number): f64
            return select(-1, ...) * 2
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_typed_vararg_element_mismatch_in_body() {
    let source = r#"
        function first(...: number): string
            return select(1, ...)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect_err("type check should fail");
}

#[test]
fn rejects_mismatched_typed_vararg_forwarding() {
    let source = r#"
        function sum(...: number): f64
            return 0
        end

        function join(...: string): f64
            return sum(...)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = super::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.to_string(), "call expected f64..., got string...");
}

#[test]
fn untyped_vararg_still_forwards_into_typed_vararg() {
    let source = r#"
        function sum(...: number): f64
            return 0
        end

        function forward(...): f64
            return sum(...)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    super::type_check(&program).expect("type check should succeed");
}

#[test]
fn typed_vararg_returns_still_widen_to_unknown_packs() {
    let source = r#"
        function only(...: number)
            return ...
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = super::type_check_and_infer(&program).expect("type check should succeed");
    assert_eq!(
        typed.functions[0].return_type,
        Some(Type::Variadic(Arc::new(Type::Unknown)))
    );
}
