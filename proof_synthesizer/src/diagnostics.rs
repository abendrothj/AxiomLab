//! Verus compiler diagnostic parser.
//!
//! Extracts structured error information from Verus output so the
//! agent loop can feed precise failure context back to the LLM.

use regex::Regex;
use std::sync::LazyLock;

/// A single diagnostic extracted from Verus output.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub span: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

// Pattern: `error[E0XXX]: message`  or  `error: message`
static ERROR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^(error|warning|note)(?:\[E\d+\])?: (.+)$").unwrap());

// Pattern: `  --> file.rs:42:10`
static LOCATION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-->\s+[^:]+:(\d+):(\d+)").unwrap());

// Verus-specific: `verification of <span> failed`
static VERUS_FAIL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"verification of (.+?) failed").unwrap());

/// Parse raw Verus/rustc output into structured diagnostics.
pub fn parse(output: &str) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let lines: Vec<&str> = output.lines().collect();

    for (i, line) in lines.iter().enumerate() {
        if let Some(cap) = ERROR_RE.captures(line) {
            let severity = match &cap[1] {
                "error" => Severity::Error,
                "warning" => Severity::Warning,
                _ => Severity::Note,
            };
            let message = cap[2].to_owned();

            // Look ahead for location.
            let (line_no, col_no) = lines
                .get(i + 1)
                .and_then(|next| LOCATION_RE.captures(next))
                .map(|loc| {
                    (
                        loc[1].parse::<u32>().ok(),
                        loc[2].parse::<u32>().ok(),
                    )
                })
                .unwrap_or((None, None));

            // Check for Verus span info.
            let span = VERUS_FAIL_RE
                .captures(&message)
                .map(|c| c[1].to_owned());

            diags.push(Diagnostic {
                severity,
                message,
                line: line_no,
                column: col_no,
                span,
            });
        }
    }

    diags
}

/// Maximum length (bytes) of a single diagnostic message included in an LLM prompt.
const MAX_MESSAGE_BYTES: usize = 500;
/// Maximum number of errors included in a single summary.
const MAX_ERRORS_IN_SUMMARY: usize = 10;

/// Sanitize a raw compiler message before including it in an LLM prompt.
///
/// Strips characters that could influence LLM behaviour out-of-band:
/// - Removes null bytes.
/// - Removes ANSI escape sequences.
/// - Truncates to `MAX_MESSAGE_BYTES`.
/// - Strips any line that looks like a system / role prompt injection attempt
///   (lines starting with "SYSTEM:", "USER:", "ASSISTANT:", "###", etc.).
fn sanitize_message(raw: &str) -> String {
    // Strip ANSI escape sequences (\x1b[...m).
    static ANSI_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\x1b\[[0-9;]*[mGKHF]").unwrap());

    // Patterns that look like prompt-injection attempts inside compiler output.
    static INJECTION_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| {
            regex::Regex::new(
                r"(?i)^\s*(system\s*:|user\s*:|assistant\s*:|<\|im_start\|>|<\|im_end\|>|###\s)",
            )
            .unwrap()
        });

    let stripped = ANSI_RE.replace_all(raw, "");
    let no_nulls = stripped.replace('\0', "");

    // Filter out potential injection lines.
    let filtered: Vec<&str> = no_nulls
        .lines()
        .filter(|line| !INJECTION_RE.is_match(line))
        .collect();
    let joined = filtered.join("\n");

    // Truncate to a safe length.
    if joined.len() <= MAX_MESSAGE_BYTES {
        joined
    } else {
        let mut end = MAX_MESSAGE_BYTES;
        while !joined.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…[truncated]", &joined[..end])
    }
}

/// Format diagnostics into a concise summary suitable for an LLM prompt.
///
/// Messages are sanitized to prevent prompt injection from crafted compiler
/// output.  At most `MAX_ERRORS_IN_SUMMARY` errors are included.
pub fn summarize(diags: &[Diagnostic]) -> String {
    if diags.is_empty() {
        return "No errors.".to_owned();
    }
    let errors: Vec<&Diagnostic> = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .take(MAX_ERRORS_IN_SUMMARY)
        .collect();

    if errors.is_empty() {
        return "No errors.".to_owned();
    }

    let total_errors = diags.iter().filter(|d| d.severity == Severity::Error).count();
    let mut lines: Vec<String> = errors
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let loc = match (d.line, d.column) {
                (Some(l), Some(c)) => format!(" (line {l}, col {c})"),
                (Some(l), None) => format!(" (line {l})"),
                _ => String::new(),
            };
            format!("{}. {}{}", i + 1, sanitize_message(&d.message), loc)
        })
        .collect();

    if total_errors > MAX_ERRORS_IN_SUMMARY {
        lines.push(format!(
            "[{} additional errors omitted]",
            total_errors - MAX_ERRORS_IN_SUMMARY
        ));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rustc_error() {
        let output = "\
error[E0308]: mismatched types
  --> candidate.rs:10:5
   |
10 |     42u32
   |     ^^^^^ expected bool, found u32";

        let diags = parse(output);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Error);
        assert_eq!(diags[0].line, Some(10));
        assert_eq!(diags[0].column, Some(5));
    }

    #[test]
    fn parse_verus_failure() {
        let output = "error: verification of function `move_arm` failed";
        let diags = parse(output);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].span.as_deref(), Some("function `move_arm`"));
    }

    #[test]
    fn summarize_empty() {
        assert_eq!(summarize(&[]), "No errors.");
    }

    #[test]
    fn summarize_formats_errors() {
        let diags = vec![Diagnostic {
            severity: Severity::Error,
            message: "integer overflow".into(),
            line: Some(42),
            column: Some(8),
            span: None,
        }];
        let s = summarize(&diags);
        assert!(s.contains("integer overflow"));
        assert!(s.contains("line 42"));
    }
}
