use waluau_ast::FunctionName;
use waluau_parser::parse;

#[test]
fn infers_local_and_return_types_for_happy_path() {
    let source = r#"
        function entry(flag: bool)
            local xs = {1, 2, 3}
            local acc = if flag then xs[0] else xs[1]
            return acc + 1
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let typed = waluau_hir::type_check_and_infer(&program).expect("inference should succeed");
    assert_eq!(typed.functions.len(), 1);
    assert_eq!(
        typed.functions[0].name,
        FunctionName::Name("entry".to_string())
    );
    assert!(typed.functions[0].return_type.is_some());
}

#[test]
fn rejects_conflicting_inferred_local_reassignment() {
    let source = r#"
        function entry(flag: bool): i32
            local value = 1
            if flag then
                value = 2
            else
                value = true
            end
            return 0
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = waluau_hir::type_check_and_infer(&program).expect_err("inference should fail");
    assert!(
        error.to_string().contains("assignment to 'value' expects"),
        "message was: {}",
        error
    );
    assert!(
        error.to_string().contains("got bool"),
        "message was: {}",
        error
    );
}

#[test]
fn infers_empty_braces_as_record_for_local_bindings() {
    let source = r#"
        function entry(): i32
            local xs = {}
            xs.value = 1::i32
            return xs.value
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    waluau_hir::type_check_and_infer(&program).expect("inference should succeed");
}

#[test]
fn rejects_ambiguous_numeric_operations_without_cast() {
    let source = r#"
        function entry(x: i64, y: f64)
            return x + y
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let error = waluau_hir::type_check_and_infer(&program).expect_err("inference should fail");
    assert_eq!(error.code(), Some("inference/ambiguous"));
}

#[test]
fn inference_is_stable_across_repeated_runs() {
    let source = r#"
        function entry(flag: bool)
            local value = if flag then 1 else 2
            return value + 1
        end
    "#;
    let program = parse(source).expect("parse should succeed");
    let inferred_once = waluau_hir::type_check_and_infer(&program).expect("first run succeeds");
    let inferred_twice = waluau_hir::type_check_and_infer(&program).expect("second run succeeds");
    assert_eq!(inferred_once, inferred_twice);
}
