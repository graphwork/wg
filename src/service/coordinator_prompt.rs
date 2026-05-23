use std::path::Path;

/// Coordinator prompt component file names (in composition order).
const COORDINATOR_PROMPT_FILES: &[&str] = &[
    "base-system-prompt.md",
    "behavioral-rules.md",
    "common-patterns.md",
    "evolved-amendments.md",
];

/// Build the system prompt for the coordinator agent by composing from files.
///
/// Reads from `.wg/agency/coordinator-prompt/` and concatenates the
/// component files in order. Falls back to the hardcoded prompt if the
/// directory doesn't exist or no files are found.
pub fn build_system_prompt(dir: &Path) -> String {
    let prompt_dir = dir.join("agency/coordinator-prompt");

    if prompt_dir.is_dir() {
        let mut parts = Vec::new();
        for filename in COORDINATOR_PROMPT_FILES {
            let path = prompt_dir.join(filename);
            if let Ok(content) = std::fs::read_to_string(&path) {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    parts.push(trimmed.to_string());
                }
            }
        }
        if !parts.is_empty() {
            return parts.join("\n\n");
        }
    }

    build_system_prompt_fallback()
}

/// Hardcoded fallback prompt used when coordinator-prompt files don't exist.
pub fn build_system_prompt_fallback() -> String {
    include_str!("../commands/service/coordinator_prompt_fallback.txt").to_string()
}
