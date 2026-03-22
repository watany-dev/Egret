//! Rich validation diagnostics for task definitions.
//!
//! Provides structured diagnostic types that collect all validation issues
//! (rather than fail-fast) with field paths, suggestions, and severity levels.

use std::fmt;

/// Severity level for validation diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Must be fixed before running.
    Error,
    /// Likely a mistake but not fatal.
    Warning,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error => write!(f, "error"),
            Self::Warning => write!(f, "warning"),
        }
    }
}

/// A structured validation diagnostic with context for human-friendly output.
#[derive(Debug, Clone)]
pub struct ValidationDiagnostic {
    /// Severity of this diagnostic.
    pub severity: Severity,
    /// JSON-like field path (e.g., `containerDefinitions[0].image`).
    pub field_path: String,
    /// Human-readable error message.
    pub message: String,
    /// Optional suggestion for how to fix the issue.
    pub suggestion: Option<String>,
}

impl fmt::Display for ValidationDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} - {}", self.severity, self.field_path, self.message)?;
        if let Some(suggestion) = &self.suggestion {
            write!(f, " (hint: {suggestion})")?;
        }
        Ok(())
    }
}

/// Result of comprehensive validation containing all diagnostics.
#[derive(Debug)]
pub struct ValidationReport {
    /// All collected diagnostics.
    pub diagnostics: Vec<ValidationDiagnostic>,
}

impl ValidationReport {
    /// Returns `true` if the report contains any error-level diagnostics.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// Count of error-level diagnostics.
    pub fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count()
    }

    /// Count of warning-level diagnostics.
    pub fn warning_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count()
    }
}

impl fmt::Display for ValidationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for diagnostic in &self.diagnostics {
            writeln!(f, "{diagnostic}")?;
        }
        let errors = self.error_count();
        let warnings = self.warning_count();
        write!(
            f,
            "{errors} error(s), {warnings} warning(s)"
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn severity_display() {
        assert_eq!(Severity::Error.to_string(), "error");
        assert_eq!(Severity::Warning.to_string(), "warning");
    }

    #[test]
    fn diagnostic_display_without_suggestion() {
        let d = ValidationDiagnostic {
            severity: Severity::Error,
            field_path: "containerDefinitions[0].image".to_string(),
            message: "image name contains whitespace".to_string(),
            suggestion: None,
        };
        assert_eq!(
            d.to_string(),
            "error: containerDefinitions[0].image - image name contains whitespace"
        );
    }

    #[test]
    fn diagnostic_display_with_suggestion() {
        let d = ValidationDiagnostic {
            severity: Severity::Warning,
            field_path: "containerDefinitions[0].essential".to_string(),
            message: "all containers have essential=false".to_string(),
            suggestion: Some("set at least one container as essential".to_string()),
        };
        assert_eq!(
            d.to_string(),
            "warning: containerDefinitions[0].essential - all containers have essential=false (hint: set at least one container as essential)"
        );
    }

    #[test]
    fn report_counts_and_has_errors() {
        let report = ValidationReport {
            diagnostics: vec![
                ValidationDiagnostic {
                    severity: Severity::Error,
                    field_path: "family".to_string(),
                    message: "empty".to_string(),
                    suggestion: None,
                },
                ValidationDiagnostic {
                    severity: Severity::Warning,
                    field_path: "containerDefinitions".to_string(),
                    message: "no port mappings".to_string(),
                    suggestion: None,
                },
                ValidationDiagnostic {
                    severity: Severity::Error,
                    field_path: "containerDefinitions[0].image".to_string(),
                    message: "invalid".to_string(),
                    suggestion: None,
                },
            ],
        };
        assert!(report.has_errors());
        assert_eq!(report.error_count(), 2);
        assert_eq!(report.warning_count(), 1);
    }

    #[test]
    fn report_no_errors() {
        let report = ValidationReport {
            diagnostics: vec![ValidationDiagnostic {
                severity: Severity::Warning,
                field_path: "containerDefinitions".to_string(),
                message: "no port mappings".to_string(),
                suggestion: None,
            }],
        };
        assert!(!report.has_errors());
        assert_eq!(report.error_count(), 0);
        assert_eq!(report.warning_count(), 1);
    }

    #[test]
    fn report_display() {
        let report = ValidationReport {
            diagnostics: vec![
                ValidationDiagnostic {
                    severity: Severity::Error,
                    field_path: "family".to_string(),
                    message: "empty".to_string(),
                    suggestion: None,
                },
                ValidationDiagnostic {
                    severity: Severity::Warning,
                    field_path: "ports".to_string(),
                    message: "none".to_string(),
                    suggestion: Some("add port mappings".to_string()),
                },
            ],
        };
        let output = report.to_string();
        assert!(output.contains("error: family - empty"));
        assert!(output.contains("warning: ports - none (hint: add port mappings)"));
        assert!(output.contains("1 error(s), 1 warning(s)"));
    }

    #[test]
    fn empty_report() {
        let report = ValidationReport {
            diagnostics: vec![],
        };
        assert!(!report.has_errors());
        assert_eq!(report.error_count(), 0);
        assert_eq!(report.warning_count(), 0);
        assert!(report.to_string().contains("0 error(s), 0 warning(s)"));
    }
}
