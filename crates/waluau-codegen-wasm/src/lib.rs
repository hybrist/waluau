use waluau_diagnostics::Diagnostic;
use waluau_ir::Module;

pub fn emit(_module: &Module) -> Result<Vec<u8>, Diagnostic> {
    Err(Diagnostic::new(
        "wasm code generation is not implemented yet",
    ))
}

#[cfg(test)]
mod tests {
    use wasmparser::Validator;

    #[test]
    fn placeholder_error_until_codegen_lands() {
        let source = r#"
            fn entry(x: i32) -> i32
                return x + 1
            end
        "#;
        let program = waluau_parser::parse(source).expect("parse should succeed");
        let ir = waluau_ir::build(&program).expect("ir should succeed");
        let err = super::emit(&ir).expect_err("emit should return placeholder error");
        assert_eq!(
            err.to_string(),
            "wasm code generation is not implemented yet"
        );
        let empty = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        Validator::new()
            .validate_all(&empty)
            .expect("validator still works");
    }
}
