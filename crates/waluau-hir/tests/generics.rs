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
fn type_checks_swap_pair_generic_alias() {
    let source = r#"
        type Pair<A, B> = { first: A, second: B }

        function swap_pair(pair: Pair<i32, bool>): Pair<bool, i32>
            return { first = pair.second, second = pair.first }
        end

        function main(): i32
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    waluau_hir::type_check(&program).expect("type check should succeed");
}

#[test]
fn type_checks_record_literal_construction_against_generic_alias() {
    let source = r#"
        type Pair<A, B> = { first: A, second: B }

        function make_pair(): Pair<bool, i32>
            local result: Pair<bool, i32> = { first = true, second = 42::i32 }
            return result
        end

        function main(): i32
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    waluau_hir::type_check(&program).expect("type check should succeed");
}
