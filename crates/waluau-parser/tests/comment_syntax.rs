use waluau_parser::parse;

#[test]
fn parses_line_comments() {
    let program =
        parse("-- comment\nfunction main(): i32\n  return 1\nend").expect("parse should succeed");
    assert_eq!(program.functions.len(), 1);
}

#[test]
fn parses_block_comments() {
    let program = parse("--[[comment]]\nfunction main(): i32\n  return 1\nend")
        .expect("parse should succeed");
    assert_eq!(program.functions.len(), 1);
}

#[test]
fn reports_unterminated_block_comments() {
    let error =
        parse("--[[comment\nfunction main(): i32\n  return 1\nend").expect_err("parse should fail");
    assert_eq!(error.to_string(), "unterminated block comment '--[[...]]'");
}
