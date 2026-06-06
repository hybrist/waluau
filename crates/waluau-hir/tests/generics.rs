use waluau_parser::parse;

#[test]
fn type_checks_identity_generic() {
    let source = r#"
        function identity<T>(value: T): T
            return value
        end

        function main(): i32
            return identity<i32>(41)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    waluau_hir::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_choose_generic() {
    let source = r#"
        function choose<T>(condition: bool, a: T, b: T): T
            if condition then
                return a
            else
                return b
            end
        end

        function main(): i32
            return choose<i32>(true, 1, 2)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    waluau_hir::type_check(&program).expect("type check should succeed");
}

#[test]
fn rejects_missing_type_arguments() {
    let source = r#"
        function identity<T>(value: T): T
            return value
        end

        function main(): i32
            return identity(41)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = waluau_hir::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.code(), Some("generic/missing-type-args"));
}

#[test]
fn rejects_uninstantiated_generic_value() {
    let source = r#"
        function identity<T>(value: T): T
            return value
        end

        function main(): i32
            local f = identity
            return f<i32>(1)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = waluau_hir::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.code(), Some("generic/uninstantiated-value"));
}

#[test]
fn rejects_mismatched_specialized_argument_types() {
    let source = r#"
        function same<T>(a: T, b: T): T
            return a
        end

        function main(): i32
            return same<i32>(1, true)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = waluau_hir::type_check(&program).expect_err("type check should fail");
    let message = error.to_string();
    assert!(
        message.contains("call expected") || message.contains("cannot implicitly convert"),
        "message was: {}",
        message
    );
}

#[test]
fn rejects_wrong_type_argument_count() {
    let source = r#"
        function same<T>(a: T, b: T): T
            return a
        end

        function main(): i32
            return same<i32, bool>(1, 2)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = waluau_hir::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.code(), Some("generic/type-arg-count"));
}

#[test]
fn type_checks_nested_field_assignment_through_generic_record() {
    let source = r#"
        type Pair<A, B> = { first: A, second: B }
        type Box<T> = { value: T }

        function entry(seed: i32): i32
            local boxed: Box<Pair<i32, i32>> = {
                value = { first = seed, second = (seed + 1) :: i32 },
            }
            boxed.value.first = (boxed.value.first + 4) :: i32
            return boxed.value.first
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    waluau_hir::type_check(&program)
        .expect("nested field assignment through generic record should type-check");
}
