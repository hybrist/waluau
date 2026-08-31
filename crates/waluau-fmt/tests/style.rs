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
fn formats_empty_record_types() {
    assert_eq!(fmt("type Marker={}\n"), "type Marker = {}\n");
    assert_eq!(fmt("type Marker = {  }\n"), "type Marker = {}\n");
    assert_eq!(fmt("local m:{}={}\n"), "local m: {} = {}\n");
}

#[test]
fn formats_conformance_declarations() {
    assert_eq!(fmt("type Add=Op&{}\n"), "type Add = Op & {}\n");
    assert_eq!(
        fmt("type Add = Op  &  { count: i32 }\n"),
        "type Add = Op & { count: i32 }\n"
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
fn preserves_escaped_spaces_verbatim() {
    // `\ ` escapes a space. The formatter reprints string lexemes from the
    // source, so the escape must survive rather than collapse to a bare space.
    let src = "local s = \"two\\ \\ spaces\"\nlocal t = `Backslash \\ here`\n";
    assert_eq!(fmt(src), src);
    assert_eq!(fmt(&fmt(src)), src);
}

#[test]
fn preserves_binary_literals_and_luau_separators() {
    let src = "local bits=0B_0101__1010_\nlocal exponent=1e+___2\n";
    assert_eq!(
        fmt(src),
        "local bits = 0B_0101__1010_\nlocal exponent = 1e+___2\n"
    );
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
fn formats_keyword_named_properties_and_method_chains() {
    assert_eq!(
        fmt("declare property Expectation:not:Expectation\nexpect(value):not:toBe(other)\n"),
        "declare property Expectation: not: Expectation\nexpect(value):not:toBe(other)\n"
    );
}

#[test]
fn formats_nominal_enums_and_matches() {
    let src = "enum Direction{north,east,south}\nfunction score(d:Direction):i32\nmatch d do\ncase Direction.north then\nreturn 1\ncase Direction.east then\nreturn 2\ncase Direction.south then\nreturn 3\nend\nend\n";
    assert_eq!(
        fmt(src),
        "enum Direction { north, east, south }\nfunction score(d: Direction): i32\n    match d do\n    case Direction.north then\n        return 1\n    case Direction.east then\n        return 2\n    case Direction.south then\n        return 3\n    end\nend\n"
    );
}

#[test]
fn formats_exported_declarations_and_qualified_enum_matches() {
    let src = "export type Pair<T>={first:T,second:T}\nexport opaque type Token=i32\nexport enum Direction{north,south}\nexport function direction_name(d:Direction):string\nreturn \"north\"\nend\nmatch d do\ncase directions.Direction.north then\ncase directions.Direction.south then\nend\n";
    assert_eq!(
        fmt(src),
        "export type Pair<T> = { first: T, second: T }\nexport opaque type Token = i32\nexport enum Direction { north, south }\nexport function direction_name(d: Direction): string\n    return \"north\"\nend\nmatch d do\ncase directions.Direction.north then\ncase directions.Direction.south then\nend\n"
    );
}

#[test]
fn formats_literal_union_type_declarations() {
    assert_eq!(
        fmt("type CardColor=\"red\"|\"black\"\n"),
        "type CardColor = \"red\" | \"black\"\n"
    );
}

#[test]
fn formats_self_receivers_and_named_parameters_in_function_types() {
    assert_eq!(
        fmt("type Op={exec:(self,a:i32,b:i32)->i32}\n"),
        "type Op = { exec: (self, a: i32, b: i32) -> i32 }\n"
    );
    assert_eq!(
        fmt("type Calc={apply:(a:i32,b:i32)->i32,offset:i32}\n"),
        "type Calc = { apply: (a: i32, b: i32) -> i32, offset: i32 }\n"
    );
    assert_eq!(
        fmt("function use(op: (base:string,i32)->string): unit end\n"),
        "function use(op: (base: string, i32) -> string): unit\nend\n"
    );
}

#[test]
fn formats_if_cast_targets_including_module_qualified_names() {
    assert_eq!(
        fmt("if Add(a)=op then\nuse(a)\nend\n"),
        "if Add(a) = op then\n    use(a)\nend\n"
    );
    assert_eq!(
        fmt("if ops . Add(a)=op then\nuse(a)\nelse\nother()\nend\n"),
        "if ops.Add(a) = op then\n    use(a)\nelse\n    other()\nend\n"
    );
}

#[test]
fn formats_chained_nested_and_operator_position_if_expressions() {
    assert_eq!(
        fmt("local x=7+if first then 10 elseif second then 20 elseif third then 30 else 40\n"),
        "local x = 7 + if first then 10 elseif second then 20 elseif third then 30 else 40\n"
    );
    assert_eq!(
        fmt("local x=if if first then false else second then 1 else 2\n"),
        "local x = if if first then false else second then 1 else 2\n"
    );
}

#[test]
fn preserves_vararg_type_annotations() {
    let src = "function sum(base:f64,...:number):f64\nreturn base\nend\n";
    assert_eq!(
        fmt(src),
        "function sum(base: f64, ...: number): f64\n    return base\nend\n"
    );
}
