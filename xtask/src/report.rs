//! What a check found: either a one-line statement of the fact it verified, or the failures.

use std::process::ExitCode;

/// The outcome of one check. A check with no failures prints one line and exits 0; a check with
/// failures prints them to stderr and exits 1, exactly as its Python predecessor did.
#[derive(Debug)]
pub struct Report {
    failures: Vec<String>,
    summary: String,
}

impl Report {
    /// The check held; `summary` is the single line it prints.
    pub fn passed(summary: impl Into<String>) -> Self {
        Self {
            failures: Vec::new(),
            summary: summary.into(),
        }
    }

    /// The check found something; `failures` are printed one per line to stderr.
    pub fn failed(failures: Vec<String>) -> Self {
        Self {
            failures,
            summary: String::new(),
        }
    }

    #[cfg(test)]
    pub fn failures(&self) -> &[String] {
        &self.failures
    }

    #[cfg(test)]
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// Every failure, one per line — the shape the tests assert against.
    #[cfg(test)]
    pub fn failure_text(&self) -> String {
        self.failures.join("\n")
    }

    pub fn emit(&self) -> ExitCode {
        if self.failures.is_empty() {
            println!("{}", self.summary);
            return ExitCode::SUCCESS;
        }
        eprintln!("{}", self.failures.join("\n"));
        ExitCode::FAILURE
    }
}
