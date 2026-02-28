use anyhow::{Context, Result};

use super::strategy::EvolverOutput;

pub(crate) fn parse_evolver_output(raw: &str) -> Result<EvolverOutput> {
    // Try to extract JSON from potentially noisy LLM output
    let json_str = extract_json(raw)
        .ok_or_else(|| anyhow::anyhow!("No valid JSON found in evolver output"))?;

    let output: EvolverOutput = serde_json::from_str(&json_str)
        .with_context(|| format!("Failed to parse evolver JSON:\n{}", json_str))?;

    Ok(output)
}

/// Extract a JSON object from potentially noisy LLM output.
pub(super) fn extract_json(raw: &str) -> Option<String> {
    let trimmed = raw.trim();

    // Try the whole string first
    if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
        return Some(trimmed.to_string());
    }

    // Strip markdown code fences
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
        && start <= end
    {
        let candidate = &stripped[start..=end];
        if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
            return Some(candidate.to_string());
        }
    }

    None
}
