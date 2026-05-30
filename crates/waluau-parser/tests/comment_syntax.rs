use waluau_parser::parse;

#[test]
fn reports_unsupported_line_comments() {
    let error =
        parse("-- comment\nfunction main(): i32\n  return 1\nend").expect_err("parse should fail");
    assert_eq!(
        error.to_string(),
        "unsupported line comment '--'; comments are not supported in V0"
    );
}

#[test]
fn reports_unsupported_block_comments() {
    let error = parse("--[[comment]]\nfunction main(): i32\n  return 1\nend")
        .expect_err("parse should fail");
    assert_eq!(
        error.to_string(),
        "unsupported block comment '--[[...]]'; comments are not supported in V0"
    );
}
