use waluau_ast::Program;
use waluau_diagnostics::Diagnostic;

mod parser;

/// Result of parsing with error recovery: a best-effort (possibly partial)
/// program plus every diagnostic encountered. The program is complete exactly
/// when `diagnostics` contains no errors.
#[derive(Debug)]
pub struct ParseOutcome {
    pub program: Program,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn parse(source: &str) -> Result<Program, Diagnostic> {
    parse_with_path(source, "source")
}

pub fn parse_with_path(source: &str, file_path: &str) -> Result<Program, Diagnostic> {
    let ParseOutcome {
        program,
        mut diagnostics,
    } = parse_with_recovery(source, file_path);
    match diagnostics.len() {
        0 => Ok(program),
        1 => Err(diagnostics.remove(0)),
        // Join with the legacy `message at start..end` rendering so existing
        // string-based consumers keep seeing the format they parse today.
        _ => Err(Diagnostic::new(
            diagnostics
                .iter()
                .map(Diagnostic::render_for_playground)
                .collect::<Vec<_>>()
                .join("\n"),
        )),
    }
}

/// Parse with statement-boundary error recovery. Always produces a program;
/// diagnostics carry structural spans resolved to line/column against
/// `source` so they can drive editor markers directly.
pub fn parse_with_recovery(source: &str, file_path: &str) -> ParseOutcome {
    let tokens = match waluau_lexer::lex(source) {
        Ok(tokens) => tokens,
        Err(error) => {
            return ParseOutcome {
                program: empty_program(source, file_path),
                diagnostics: vec![error.with_source(file_path, source)],
            };
        }
    };
    let (program, diagnostics) =
        parser::Parser::new(tokens, file_path.to_string()).parse_program(source);
    ParseOutcome {
        program,
        diagnostics: diagnostics
            .into_iter()
            .map(|diagnostic| diagnostic.with_source(file_path, source))
            .collect(),
    }
}

fn empty_program(source: &str, file_path: &str) -> Program {
    Program {
        functions: Vec::new(),
        declared_imports: Vec::new(),
        declared_constants: Vec::new(),
        type_declarations: Vec::new(),
        top_level: Vec::new(),
        export: None,
        sources: std::collections::BTreeMap::from([(file_path.to_string(), source.to_string())]),
        entry_file_path: file_path.to_string(),
    }
}

#[cfg(test)]
mod tests;
