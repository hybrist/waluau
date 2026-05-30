use std::error::Error;
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    message: String,
    code: Option<String>,
}

impl Diagnostic {
    /// Create a diagnostic with only a human message (backwards compatible).
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
        }
    }

    /// Create a diagnostic with a machine-stable code and a human message.
    pub fn new_with_code(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: Some(code.into()),
        }
    }

    /// Read-only accessor for the optional diagnostic code.
    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Preserve the original display behavior (message only) for backwards compatibility.
        formatter.write_str(&self.message)
    }
}

impl Error for Diagnostic {}
