use waluau_diagnostics::DiagnosticCategory;
use waluau_parser::parse;

#[test]
fn unknown_generic_type_alias_reports_code() {
    let source = r#"
        function entry(): i32
            local xs: Array<i32> = {}
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = waluau_hir::type_check_and_infer(&program).expect_err("type check should fail");
    assert_eq!(error.code(), Some("alias/unknown"));
    assert_eq!(error.category(), Some(DiagnosticCategory::Unsupported));
    assert!(
        error.to_string().contains("unknown type alias 'Array'"),
        "message was: {}",
        error
    );
}

#[test]
fn inference_empty_braces_defaults_to_record_for_locals() {
    let source = r#"
        function entry(): i32
            local xs = {}
            return xs.x
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = waluau_hir::type_check_and_infer(&program).expect_err("inference should fail");
    assert_eq!(error.code(), None);
    assert!(error.to_string().contains("unknown record field 'x'"));
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
    assert_eq!(error.category(), Some(DiagnosticCategory::Unsupported));
    assert_eq!(
        error.action(),
        Some("add an explicit return type annotation to break the cycle")
    );
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
    assert_eq!(error.category(), Some(DiagnosticCategory::Conflict));
    assert_eq!(
        error.action(),
        Some(
            "ensure all return branches produce the same type or add an explicit return annotation"
        ),
    );
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
    assert_eq!(error.category(), Some(DiagnosticCategory::Conflict));
    assert_eq!(
        error.action(),
        Some("cast elements to a common type or split values into separate arrays"),
    );
    assert!(
        error
            .to_string()
            .contains("array literal elements must share a common type"),
        "message was: {}",
        error
    );
}

#[test]
fn inference_numeric_ambiguity_code() {
    let source = r#"
        function entry(x: i64, y: f64)
            return x + y
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = waluau_hir::type_check_and_infer(&program).expect_err("inference should fail");
    assert_eq!(error.code(), Some("inference/ambiguous"));
    assert_eq!(error.category(), Some(DiagnosticCategory::Ambiguous));
    assert_eq!(
        error.action(),
        Some("add an explicit cast to pick the intended numeric type"),
    );
    assert!(
        error
            .to_string()
            .contains("operation requires compatible numeric operands"),
        "message was: {}",
        error
    );
}
