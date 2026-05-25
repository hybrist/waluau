use waluau_diagnostics::Diagnostic;

pub fn compile_source(source: &str) -> Result<(), Diagnostic> {
    let program = waluau_parser::parse(source)?;
    waluau_hir::type_check(&program)
}

pub fn run() -> Result<(), Diagnostic> {
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn compiles_valid_program() {
        let source = r#"
            fn entry(x: number, y: number) -> number
                let z: number = x + y
                return z
            end
        "#;
        super::compile_source(source).expect("compile should succeed");
    }

    #[test]
    fn rejects_invalid_program() {
        let source = r#"
            fn entry(x: number) -> number
                return true
            end
        "#;
        let err = super::compile_source(source).expect_err("compile should fail");
        assert_eq!(err.to_string(), "return expects Number, got Bool");
    }
}
