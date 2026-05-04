//! Classify an agent failure from the raw JSONL stream written by the claude/codex executors.
//!
//! This is a pure function: no side-effects, no graph I/O. The wrapper invokes
//! `wg classify-failure` which shells out to this logic.

use std::path::Path;
use workgraph::graph::FailureClass;

/// Maximum bytes to read from the tail of raw_stream.jsonl when scanning for
/// api_error_status. The relevant event is always near the end of the stream.
const TAIL_BYTES: u64 = 4096;

/// Classify an agent failure from the raw JSONL stream and exit code.
///
/// # Arguments
/// - `raw_stream`: path to the `raw_stream.jsonl` produced by the executor wrapper.
///   May not exist if the agent was killed before producing any output.
/// - `exit_code`: the shell exit code of the agent process (124 = hard timeout).
pub fn classify_from_raw_stream(raw_stream: &Path, exit_code: i32) -> FailureClass {
    // Hard timeout: exit 124 is set by the `timeout` command in the wrapper.
    if exit_code == 124 {
        return FailureClass::AgentHardTimeout;
    }

    // Read the tail of raw_stream.jsonl for api_error_status.
    let tail = match read_tail(raw_stream) {
        Some(t) => t,
        None => {
            // File missing or unreadable — could be a wrapper-internal problem
            // (exit_code != 0 with no stream) or the agent never ran.
            if exit_code != 0 {
                return FailureClass::WrapperInternal;
            }
            return FailureClass::AgentExitNonzero;
        }
    };

    // Scan for api_error_status numeric value.
    if let Some(status_code) = extract_api_error_status(&tail) {
        match status_code {
            400 => {
                // Confirm it's a document-processing error, not an unrelated 400.
                if tail.contains("Could not process PDF")
                    || tail.contains("Could not process document")
                    || tail.contains("Could not process image")
                {
                    return FailureClass::ApiError400Document;
                }
                // Generic 400 — treat as document error conservatively.
                return FailureClass::ApiError400Document;
            }
            429 => return FailureClass::ApiError429RateLimit,
            500..=599 => return FailureClass::ApiError5xxTransient,
            _ => {}
        }
    }

    FailureClass::AgentExitNonzero
}

/// Read up to TAIL_BYTES from the end of `path`, returning the string content.
/// Returns None if the file doesn't exist, can't be read, or is empty.
fn read_tail(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len == 0 {
        return None;
    }
    let offset = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;
    if buf.is_empty() { None } else { Some(buf) }
}

/// Extract the integer value of the first `api_error_status` key found in `text`.
/// Handles both `"api_error_status":400` and `"api_error_status": 400` (with space).
fn extract_api_error_status(text: &str) -> Option<u32> {
    let key = "api_error_status";
    let pos = text.find(key)?;
    let after = &text[pos + key.len()..];
    let mut chars = after.chars().peekable();
    // Skip closing quote (if present), then colon, then optional whitespace.
    // Input is typically: `"api_error_status":400` or `api_error_status: 400`.
    // After skipping past `api_error_status`, `after` starts with `":400` or `:400`.
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            break;
        }
        chars.next();
    }
    // read digits
    let digits: String = chars.take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_stream(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    fn test_classifier_pdf_400_from_real_jsonl() {
        let f = write_stream(
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"api_error_status":400,"message":"Could not process PDF"}"#,
        );
        assert_eq!(
            classify_from_raw_stream(f.path(), 1),
            FailureClass::ApiError400Document
        );
    }

    #[test]
    fn test_classifier_pdf_400_could_not_process_document() {
        let f = write_stream(
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"api_error_status":400,"message":"Could not process document"}"#,
        );
        assert_eq!(
            classify_from_raw_stream(f.path(), 1),
            FailureClass::ApiError400Document
        );
    }

    #[test]
    fn test_classifier_429_rate_limit() {
        let f = write_stream(
            r#"{"type":"result","is_error":true,"api_error_status":429,"message":"Rate limit exceeded"}"#,
        );
        assert_eq!(
            classify_from_raw_stream(f.path(), 1),
            FailureClass::ApiError429RateLimit
        );
    }

    #[test]
    fn test_classifier_500_transient() {
        let f = write_stream(
            r#"{"type":"result","is_error":true,"api_error_status":500,"message":"Internal server error"}"#,
        );
        assert_eq!(
            classify_from_raw_stream(f.path(), 1),
            FailureClass::ApiError5xxTransient
        );
    }

    #[test]
    fn test_classifier_503_transient() {
        let f = write_stream(
            r#"{"type":"result","is_error":true,"api_error_status":503,"message":"Service unavailable"}"#,
        );
        assert_eq!(
            classify_from_raw_stream(f.path(), 1),
            FailureClass::ApiError5xxTransient
        );
    }

    #[test]
    fn test_classifier_hard_timeout() {
        // File doesn't matter for exit 124
        let f = write_stream("doesn't matter");
        assert_eq!(
            classify_from_raw_stream(f.path(), 124),
            FailureClass::AgentHardTimeout
        );
    }

    #[test]
    fn test_classifier_generic_exit() {
        let f = write_stream(r#"{"type":"result","subtype":"success","result":"done"}"#);
        assert_eq!(
            classify_from_raw_stream(f.path(), 1),
            FailureClass::AgentExitNonzero
        );
    }

    #[test]
    fn test_classifier_missing_raw_stream() {
        let path = std::path::PathBuf::from("/nonexistent/path/raw_stream.jsonl");
        assert_eq!(
            classify_from_raw_stream(&path, 1),
            FailureClass::WrapperInternal
        );
    }

    #[test]
    fn test_classifier_truncated_jsonl() {
        // Last line is partial JSON — should fall back, not panic
        let f = write_stream(r#"{"type":"result","api_error_status":400,"mes"#);
        // Still extracts the status code from partial JSON
        assert_eq!(
            classify_from_raw_stream(f.path(), 1),
            FailureClass::ApiError400Document
        );
    }

    #[test]
    fn test_classifier_empty_stream_nonzero_exit() {
        let f = write_stream("");
        // Empty stream + non-zero exit → WrapperInternal (no stream data)
        assert_eq!(
            classify_from_raw_stream(f.path(), 1),
            FailureClass::WrapperInternal
        );
    }

    #[test]
    fn test_extract_api_error_status_with_space() {
        assert_eq!(
            extract_api_error_status(r#""api_error_status": 400"#),
            Some(400)
        );
    }

    #[test]
    fn test_extract_api_error_status_no_space() {
        assert_eq!(
            extract_api_error_status(r#""api_error_status":429"#),
            Some(429)
        );
    }

    #[test]
    fn test_extract_api_error_status_not_found() {
        assert_eq!(extract_api_error_status(r#"{"type":"result"}"#), None);
    }
}
