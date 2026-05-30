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

    pub fn action(&self) -> Option<&str> {
        self.action.as_deref()
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Preserve the original display behavior (message only) for backwards compatibility.
        formatter.write_str(&self.message)
    }
}

impl Error for Diagnostic {}
