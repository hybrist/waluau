//! Illustrative golden tests documenting the formatter's style decisions.
//! Broad correctness (semantics, comments, idempotency) is covered by the
//! corpus tests; these lock in specific formatting choices.

use waluau_fmt::{FormatConfig, format_source};

fn fmt(src: &str) -> String {
    format_source(src, &FormatConfig::default()).expect("formats")
}

#[test]
fn normalises_indentation_and_spacing() {
    let src = "function add(a:i32,b:i32):i32\nreturn a+b\nend\n";
    assert_eq!(
        fmt(src),
        "function add(a: i32, b: i32): i32\n    return a + b\nend\n"
    );
}

#[test]
fn reflows_long_call_arguments() {
    let src =
        "local x = call(aaaaaaaaaa, bbbbbbbbbb, cccccccccc, dddddddddd, eeeeeeeeee, ffffffffff)\n";
    let out = format_source(
        src,
        &FormatConfig {
            line_width: 40,
            indent_width: 4,
        },
    )
    .unwrap();
    assert_eq!(
        out,
        "local x = call(\n    aaaaaaaaaa,\n    bbbbbbbbbb,\n    cccccccccc,\n    dddddddddd,\n    eeeeeeeeee,\n    ffffffffff\n)\n"
    );
}

#[test]
fn keeps_short_constructs_on_one_line() {
    assert_eq!(fmt("local xs = {1,2,3}\n"), "local xs = {1, 2, 3}\n");
    assert_eq!(
        fmt("local r = { value = 3 }\n"),
        "local r = { value = 3 }\n"
    );
}

#[test]
fn formats_module_opaque_type_declarations() {
    assert_eq!(
        fmt("opaque   type State={value:i32}\n"),
        "opaque type State = { value: i32 }\n"
    );
}

#[test]
fn formats_non_final_function_typed_record_fields() {
    let src = "type Node={measure:(i32)->i32,split:(string)->(i32,string),grow:f64}\n";
    assert_eq!(
        fmt(src),
        "type Node = { measure: (i32) -> i32, split: (string) -> (i32, string), grow: f64 }\n"
    );
}

#[test]
fn preserves_comments_and_blank_lines() {
    let src = "-- header\nlocal a = 1 -- trailing\n\nlocal b = 2\n";
    assert_eq!(
        fmt(src),
        "-- header\nlocal a = 1 -- trailing\n\nlocal b = 2\n"
    );
}

#[test]
fn preserves_string_interpolation_verbatim() {
    let src = "local s = `hi {name}!`\n";
    assert_eq!(fmt(src), "local s = `hi {name}!`\n");
}

#[test]
fn indents_nested_blocks() {
    let src = "if a == b then\nreturn 1\nelse\nreturn 2\nend\n";
    assert_eq!(
        fmt(src),
        "if a == b then\n    return 1\nelse\n    return 2\nend\n"
    );
}

#[test]
fn is_idempotent_on_messy_input() {
    let messy = "function   f( )\n\n\n   local   x=1\n   return    x\nend\n";
    let once = fmt(messy);
    assert_eq!(fmt(&once), once);
}

#[test]
fn formats_nominal_enums_and_matches() {
    let src = "enum Direction{north,east,south}\nfunction score(d:Direction):i32\nmatch d do\ncase Direction.north then\nreturn 1\ncase Direction.east then\nreturn 2\ncase Direction.south then\nreturn 3\nend\nend\n";
    assert_eq!(
        fmt(src),
        "enum Direction { north, east, south }\nfunction score(d: Direction): i32\n    match d do\n    case Direction.north then\n        return 1\n    case Direction.east then\n        return 2\n    case Direction.south then\n        return 3\n    end\nend\n"
    );
}
