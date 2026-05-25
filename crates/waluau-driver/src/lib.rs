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
    fn fixture_source(name: &str) -> &'static str {
        match name {
            "add" => include_str!("../../../fixtures/add.walu"),
            "mismatch" => include_str!("../../../fixtures/mismatch.walu"),
            other => panic!("unknown fixture: {other}"),
        }
    }

    #[test]
    fn compiles_valid_fixture_file() {
        let source = fixture_source("add");
        super::compile_source(source).expect("compile should succeed");
    }

    #[test]
    fn rejects_invalid_fixture_file() {
        let source = fixture_source("mismatch");
        let err = super::compile_source(source).expect_err("compile should fail");
        assert_eq!(err.to_string(), "return expects Number, got Bool");
    }
}
