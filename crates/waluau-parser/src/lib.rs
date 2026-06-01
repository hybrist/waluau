use waluau_ast::Program;
use waluau_diagnostics::Diagnostic;

mod parser;

pub fn parse(source: &str) -> Result<Program, Diagnostic> {
    parse_with_path(source, "source")
}

pub fn parse_with_path(source: &str, file_path: &str) -> Result<Program, Diagnostic> {
    let tokens = waluau_lexer::lex(source)?;
    let mut program = parser::Parser::new(tokens, file_path.to_string()).parse_program()?;
    program
        .sources
        .insert(file_path.to_string(), source.to_string());
    program.entry_file_path = file_path.to_string();
    Ok(program)
}

#[cfg(test)]
mod tests;
