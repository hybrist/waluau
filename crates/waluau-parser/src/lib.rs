use waluau_ast::Program;
use waluau_diagnostics::Diagnostic;

mod parser;

pub fn parse(source: &str) -> Result<Program, Diagnostic> {
    parse_with_path(source, "source")
}

pub fn parse_with_path(source: &str, file_path: &str) -> Result<Program, Diagnostic> {
    let tokens = waluau_lexer::lex(source).map_err(|error| error.with_source(file_path, source))?;
    parser::Parser::new(tokens, file_path.to_string())
        .parse_program(source)
        .map_err(|error| error.with_source(file_path, source))
}

#[cfg(test)]
mod tests;
