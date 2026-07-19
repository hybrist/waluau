use std::error::Error;
use std::fmt;

use waluau_span::Span;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticCategory {
    Ambiguous,
    Conflict,
    Unsupported,
    MissingContext,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    message: String,
    code: Option<&'static str>,
    category: Option<DiagnosticCategory>,
    span: Option<Span>,
    file_path: Option<String>,
    source_location: Option<(u32, u32)>,
    action: Option<String>,
}

impl Diagnostic {
    /// Create a diagnostic with only a human message (backwards compatible).
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
            category: None,
            span: None,
            file_path: None,
            source_location: None,
            action: None,
        }
    }

    /// Create a diagnostic with a machine-stable code and a human message.
    /// Code must be a `'static` string literal.
    pub fn new_with_code(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: Some(code),
            category: None,
            span: None,
            file_path: None,
            source_location: None,
            action: None,
        }
    }

    /// Builder-style setter for code.
    pub fn with_code(mut self, code: &'static str) -> Self {
        self.code = Some(code);
        self
    }

    pub fn with_category(mut self, category: DiagnosticCategory) -> Self {
        self.category = Some(category);
        self
    }

    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    pub fn with_file_path(mut self, file_path: impl Into<String>) -> Self {
        self.file_path = Some(file_path.into());
        self
    }

    pub fn with_file_path_if_missing(mut self, file_path: impl Into<String>) -> Self {
        if self.file_path.is_none() {
            self.file_path = Some(file_path.into());
        }
        self
    }

    /// Resolve the diagnostic's byte span to a one-based line and column.
    pub fn with_source(mut self, file_path: impl Into<String>, source: &str) -> Self {
        let Some(span) = self.span else {
            return self;
        };
        let mut offset = (span.start as usize).min(source.len());
        while !source.is_char_boundary(offset) {
            offset -= 1;
        }
        let prefix = &source[..offset];
        let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
        let column = prefix
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .chars()
            .count() as u32
            + 1;
        self.file_path = Some(file_path.into());
        self.source_location = Some((line, column));
        self
    }

    pub fn with_action(mut self, action: impl Into<String>) -> Self {
        self.action = Some(action.into());
        self
    }

    /// Read-only accessor for the optional diagnostic code.
    pub fn code(&self) -> Option<&'static str> {
        self.code
    }

    pub fn category(&self) -> Option<DiagnosticCategory> {
        self.category
    }

    pub fn span(&self) -> Option<Span> {
        self.span
    }

    pub fn file_path(&self) -> Option<&str> {
        self.file_path.as_deref()
    }

    pub fn source_location(&self) -> Option<(u32, u32)> {
        self.source_location
    }

    pub fn action(&self) -> Option<&str> {
        self.action.as_deref()
    }

    /// Render a diagnostic for a user-facing compiler surface.
    ///
    /// `Display` intentionally remains message-only for callers that compare or
    /// otherwise process the human message. Compiler frontends can opt into the
    /// source byte range when one is available.
    pub fn render(&self) -> String {
        if let (Some(file_path), Some((line, column))) =
            (self.file_path.as_deref(), self.source_location)
        {
            return format!("{file_path}:{line}:{column}: {}", self.message);
        }
        match self.span {
            Some(span) => format!("{} at {}..{}", self.message, span.start, span.end),
            None => self.message.clone(),
        }
    }

    /// Render the byte range in the module-aware form consumed by the
    /// playground's Monaco marker integration.
    pub fn render_for_playground(&self) -> String {
        match (self.file_path.as_deref(), self.span) {
            (Some(file_path), Some(span)) if file_path != "source" => format!(
                "in module \"{file_path}\": {} at {}..{}",
                self.message, span.start, span.end
            ),
            _ => match self.span {
                Some(span) => format!("{} at {}..{}", self.message, span.start, span.end),
                None => self.message.clone(),
            },
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Preserve the original display behavior (message only) for backwards compatibility.
        formatter.write_str(&self.message)
    }
}

impl Error for Diagnostic {}

#[cfg(test)]
mod tests {
    use super::{Diagnostic, Span};

    #[test]
    fn renders_source_span_without_changing_display() {
        let diagnostic =
            Diagnostic::new("cannot convert string to i32").with_span(Span { start: 8, end: 13 });

        assert_eq!(diagnostic.to_string(), "cannot convert string to i32");
        assert_eq!(diagnostic.render(), "cannot convert string to i32 at 8..13");
    }

    #[test]
    fn renders_file_line_and_column_when_source_is_available() {
        let diagnostic = Diagnostic::new("cannot convert string to i32")
            .with_span(Span { start: 8, end: 13 })
            .with_source("example.walu", "first\n  value");

        assert_eq!(
            diagnostic.render(),
            "example.walu:2:3: cannot convert string to i32"
        );
        assert_eq!(diagnostic.file_path(), Some("example.walu"));
        assert_eq!(diagnostic.source_location(), Some((2, 3)));
    }

    #[test]
    fn renders_module_range_for_playground() {
        let diagnostic = Diagnostic::new("cannot convert string to i32")
            .with_span(Span { start: 8, end: 13 })
            .with_file_path("/lib.walu");

        assert_eq!(
            diagnostic.render_for_playground(),
            "in module \"/lib.walu\": cannot convert string to i32 at 8..13"
        );
    }
}
