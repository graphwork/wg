use std::path::Path;

/// Authoritative attended-chat capability contract. It is appended to every
/// built-in or project-composed chat prompt so a legacy generated prompt
/// cannot silently reinstate the retired thin-task-creator restriction.
pub const ATTENDED_CHAT_OPERATOR_CONTRACT: &str = include_str!("../text/attended_chat_contract.md");

const RETIRED_CHAT_DENYLIST_MARKERS: &[&str] = &[
    "A chat agent NEVER reads source files",
    "You do NOT read source files",
    "The ONLY files you may read are WG state",
    "Never implement",
    "Never investigate",
    "thin task-creator",
];

fn with_attended_contract(body: String) -> String {
    // Old generated project prompts have the same system-message priority as
    // this contract. Merely appending a contradiction makes model behavior
    // nondeterministic, so omit a legacy body that carries a known retired
    // denylist marker. Neutral/custom graph guidance remains intact.
    let body = if RETIRED_CHAT_DENYLIST_MARKERS
        .iter()
        .any(|marker| body.contains(marker))
    {
        ""
    } else {
        body.trim()
    };
    if body.is_empty() {
        ATTENDED_CHAT_OPERATOR_CONTRACT.trim().to_string()
    } else {
        format!("{}\n\n{}", body, ATTENDED_CHAT_OPERATOR_CONTRACT.trim())
    }
}

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
            return with_attended_contract(parts.join("\n\n"));
        }
    }

    build_system_prompt_fallback()
}

/// Hardcoded fallback prompt used when coordinator-prompt files don't exist.
pub fn build_system_prompt_fallback() -> String {
    with_attended_contract(
        include_str!("../commands/service/coordinator_prompt_fallback.txt").to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_human_directed(prompt: &str) {
        assert!(prompt.contains("human's attended repository assistant"));
        assert!(prompt.contains("Follow the human's request"));
        assert!(prompt.contains("normal tool surface: read, search, write, edit, execute, test"));
        assert!(prompt.contains("no role-based operation denylist"));
        assert!(prompt.contains("Never say that the chat contract prohibits repository reads"));
        assert!(!prompt.contains("You do NOT read source files"));
        assert!(!prompt.contains("The ONLY files you may read are WG state"));
    }

    #[test]
    fn fallback_is_human_directed_operator_prompt() {
        assert_human_directed(&build_system_prompt_fallback());
    }

    #[test]
    fn project_prompt_cannot_remove_authoritative_attended_contract() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("agency/coordinator-prompt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("base-system-prompt.md"),
            "Neutral custom graph preface",
        )
        .unwrap();

        let prompt = build_system_prompt(tmp.path());
        assert!(prompt.starts_with("Neutral custom graph preface"));
        assert_human_directed(&prompt);
    }

    #[test]
    fn retired_project_prompt_is_removed_not_merely_contradicted() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("agency/coordinator-prompt");
        std::fs::create_dir_all(&dir).unwrap();
        let production_refusal = "A chat agent NEVER reads source files.";
        std::fs::write(
            dir.join("base-system-prompt.md"),
            format!("Legacy graph preface. {production_refusal}"),
        )
        .unwrap();

        let prompt = build_system_prompt(tmp.path());
        assert_human_directed(&prompt);
        assert!(!prompt.contains(production_refusal));
        assert!(!prompt.contains("Legacy graph preface"));
    }
}
