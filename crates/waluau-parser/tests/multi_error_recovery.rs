use waluau_parser::parse;

#[test]
fn reports_multiple_errors_for_missing_local_type_fixture() {
    let source = include_str!("fixtures/multi_error_missing_local_type.walu");
    let error = parse(source).expect_err("parse should fail");
    let message = error.to_string();
    let lines: Vec<_> = message.lines().collect();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("expected expression"));
}

#[test]
fn reports_multiple_errors_for_if_while_fixture() {
    let source = include_str!("fixtures/multi_error_if_while.walu");
    let error = parse(source).expect_err("parse should fail");
    let message = error.to_string();
    let lines: Vec<_> = message.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("expected expression"));
    assert!(lines[1].contains("expected expression"));
}

#[test]
fn reports_multiple_errors_for_calls_and_assign_fixture() {
    let source = include_str!("fixtures/multi_error_calls_and_assign.walu");
    let error = parse(source).expect_err("parse should fail");
    let message = error.to_string();
    let lines: Vec<_> = message.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("expected expression"));
    assert!(lines[1].contains("expected expression"));
}
