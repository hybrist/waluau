use waluau_diagnostics::DiagnosticCategory;
use waluau_parser::parse;

#[test]
fn generic_function_unsupported_code() {
    let source = r#"
        function identity<T>(x: T): T
            return x
        end
    "#;
    let error = parse(source).expect_err("parse should fail");
    assert_eq!(error.code(), Some("generic/unsupported-function"));
    assert_eq!(error.category(), Some(DiagnosticCategory::Unsupported));
    assert!(error.span().is_some(), "diagnostic should have a span");
    assert_eq!(
        error.action(),
        Some("remove type parameters and use concrete types")
    );
    assert!(
        error
            .to_string()
            .contains("generic type parameters are not supported"),
        "message was: {}",
        error
    );
}

#[test]
fn generic_type_unsupported_code() {
    let source = r#"
        function entry(): i32
            local xs: Array<i32> = {}
            return 0
        end
    "#;
    let error = parse(source).expect_err("parse should fail");
    assert_eq!(error.code(), Some("generic/unsupported-type"));
    assert_eq!(error.category(), Some(DiagnosticCategory::Unsupported));
    assert!(error.span().is_some(), "diagnostic should have a span");
    assert_eq!(
        error.action(),
        Some("use a concrete type like {i32} for arrays")
    );
    assert!(
        error
            .to_string()
            .contains("generic types are not supported"),
        "message was: {}",
        error
    );
}

#[test]
fn generic_call_unsupported_code() {
    let source = r#"
        function entry(): i32
            return identity<i32>(42)
        end
    "#;
    let error = parse(source).expect_err("parse should fail");
    assert_eq!(error.code(), Some("generic/unsupported-call"));
    assert_eq!(error.category(), Some(DiagnosticCategory::Unsupported));
    assert!(error.span().is_some(), "diagnostic should have a span");
    assert_eq!(
        error.action(),
        Some("remove type arguments; the compiler infers types from arguments")
    );
    assert!(
        error
            .to_string()
            .contains("explicit type arguments in function calls are not supported"),
        "message was: {}",
        error
    );
}

#[test]
fn inference_empty_array_missing_context_code() {
    let source = r#"
        function entry(): i32
            local xs = {}
            return #xs
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = waluau_hir::type_check_and_infer(&program).expect_err("inference should fail");
    assert_eq!(error.code(), Some("inference/missing-context"));
    assert!(
        error
            .to_string()
            .contains("empty array literal requires explicit element type"),
        "message was: {}",
        error
    );
}

#[test]
fn inference_recursive_return_unsupported_code() {
    let source = r#"
        function fact(n: i32)
            if n == 0 then
                return 1
            end
            return n * fact(n - 1)
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = waluau_hir::type_check_and_infer(&program).expect_err("inference should fail");
    assert_eq!(error.code(), Some("inference/unsupported"));
    assert!(
        error
            .to_string()
            .contains("cannot infer return type for recursive or cyclic function"),
        "message was: {}",
        error
    );
}

#[test]
fn inference_return_conflict_code() {
    let source = r#"
        function bad(flag: bool)
            if flag then
                return 1
            end
            return true
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = waluau_hir::type_check_and_infer(&program).expect_err("inference should fail");
    assert_eq!(error.code(), Some("inference/conflict"));
    assert!(
        error
            .to_string()
            .contains("function return branches must resolve to the same type"),
        "message was: {}",
        error
    );
}

#[test]
fn inference_array_element_conflict_code() {
    let source = r#"
        function entry(): i32
            local xs: {i32} = {1, true}
            return #xs
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = waluau_hir::type_check(&program).expect_err("type check should fail");
    assert_eq!(error.code(), Some("inference/conflict"));
    assert!(
        error
            .to_string()
            .contains("array literal elements must share a common type"),
        "message was: {}",
        error
    );
}
