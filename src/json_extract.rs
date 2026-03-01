//! Utility for extracting JSON from noisy LLM output.
//!
//! LLMs are instructed to return only JSON, but they may wrap it in
//! markdown fences or include leading/trailing commentary. This module
//! provides a robust extraction function.

/// Extract a JSON object from potentially noisy LLM output.
///
/// Tries these strategies in order:
/// 1. Parse the whole trimmed string as JSON.
/// 2. Strip markdown code fences and parse.
/// 3. Find the first `{` and last `}` and parse the substring.
///
/// Returns `None` if no valid JSON object can be found.
pub fn extract_json(raw: &str) -> Option<String> {
    // Try the whole string first (ideal case)
    let trimmed = raw.trim();
    if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
        return Some(trimmed.to_string());
    }

    // Strip markdown code fences if present
    let stripped = if trimmed.starts_with("```") {
        let inner = trimmed
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();
        if serde_json::from_str::<serde_json::Value>(inner).is_ok() {
            return Some(inner.to_string());
        }
        inner
    } else {
        trimmed
    };

    // Find the first { and last } and try to parse
    if let Some(start) = stripped.find('{')
        && let Some(end) = stripped.rfind('}')
    {
        let candidate = &stripped[start..=end];
        if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
            return Some(candidate.to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_plain_json() {
        let input = r#"{"score": 0.85, "notes": "Good"}"#;
        let result = extract_json(input).unwrap();
        assert!(result.contains("0.85"));
    }

    #[test]
    fn extract_with_fences() {
        let input = "```json\n{\"score\": 0.7}\n```";
        let result = extract_json(input).unwrap();
        assert!(result.contains("0.7"));
    }

    #[test]
    fn extract_with_surrounding_text() {
        let input = "Here is my evaluation:\n{\"score\": 0.9}\nEnd.";
        let result = extract_json(input).unwrap();
        assert!(result.contains("0.9"));
    }

    #[test]
    fn returns_none_for_garbage() {
        assert!(extract_json("no json here at all").is_none());
    }
}
