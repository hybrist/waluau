use waluau_parser::parse;

fn main() {
    let source = r#"
        local point = { x = 41::i32 }

        function point:identity<T>(value: T): T
            return value
        end

        assert(point:identity<i32>(42::i32) == 42)
    "#;
    let program = parse(source).expect("parse should succeed");
    waluau_hir::type_check(&program).expect("type check should succeed");
    println!("Success!");
}