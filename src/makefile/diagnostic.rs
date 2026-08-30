use std::fmt;

/// One thing the Makefile converter could not translate faithfully.
///
/// The converter is a heuristic, not a make implementation, and it always will
/// be: every construct it cannot represent used to be dropped or mangled in
/// silence. A `Diagnostic` is that silence made audible. Anything the converter
/// CAN detect but not represent becomes one of these; anything that would
/// produce a wrong ottofile is an `Err` instead, not a diagnostic.
///
/// `otto Convert` prints every diagnostic to stderr (stdout carries the YAML),
/// and `--strict` turns a non-empty list into a non-zero exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// 1-based physical line in the source Makefile, when it is known. The
    /// converter works from the AST and so knows the line of the variable or
    /// target it is converting, but not of the byte inside it.
    pub line: Option<usize>,
    pub message: String,
}

impl Diagnostic {
    pub fn at(line: usize, message: impl Into<String>) -> Self {
        Self {
            line: Some(line),
            message: message.into(),
        }
    }

    pub fn detached(message: impl Into<String>) -> Self {
        Self {
            line: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "Makefile:{}: warning: {}", line, self.message),
            None => write!(f, "Makefile: warning: {}", self.message),
        }
    }
}

#[path = "diagnostic_tests.rs"]
mod tests;
