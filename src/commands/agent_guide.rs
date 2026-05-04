use anyhow::Result;

/// Universal agent / chat-agent role contract, bundled into the wg binary.
///
/// This text is project-independent: it describes how agents behave in ANY
/// workgraph project. Project-specific rules live in that project's
/// `CLAUDE.md` / `AGENTS.md`. workgraph contributor docs live in
/// `docs/designs/` and `docs/research/` of the workgraph source repo.
pub const AGENT_GUIDE_TEXT: &str = include_str!("../text/agent_guide.md");

pub fn run() -> Result<()> {
    print!("{}", AGENT_GUIDE_TEXT);
    if !AGENT_GUIDE_TEXT.ends_with('\n') {
        println!();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guide_text_is_non_empty() {
        assert!(!AGENT_GUIDE_TEXT.trim().is_empty());
    }

    #[test]
    fn guide_text_covers_three_roles() {
        assert!(AGENT_GUIDE_TEXT.contains("dispatcher"));
        assert!(AGENT_GUIDE_TEXT.contains("chat agent"));
        assert!(AGENT_GUIDE_TEXT.contains("worker agent"));
    }

    #[test]
    fn guide_text_covers_chat_agent_contract() {
        assert!(AGENT_GUIDE_TEXT.contains("Chat Agent Contract"));
        assert!(AGENT_GUIDE_TEXT.contains("thin task-creator"));
        assert!(AGENT_GUIDE_TEXT.contains("NEVER"));
    }

    #[test]
    fn guide_text_warns_off_builtin_task_tools() {
        assert!(AGENT_GUIDE_TEXT.contains("TaskCreate"));
        assert!(AGENT_GUIDE_TEXT.contains("Task tool"));
    }

    #[test]
    fn guide_text_documents_validation_section() {
        assert!(AGENT_GUIDE_TEXT.contains("## Validation"));
    }

    #[test]
    fn guide_text_documents_smoke_gate() {
        assert!(AGENT_GUIDE_TEXT.contains("Smoke Gate"));
        assert!(AGENT_GUIDE_TEXT.contains("manifest.toml"));
    }

    #[test]
    fn guide_text_documents_quality_pass() {
        assert!(
            AGENT_GUIDE_TEXT.contains("quality pass") || AGENT_GUIDE_TEXT.contains("Quality pass")
        );
    }

    #[test]
    fn guide_text_documents_paused_task_convention() {
        assert!(AGENT_GUIDE_TEXT.contains("Paused-task") || AGENT_GUIDE_TEXT.contains("paused"));
    }

    /// Regression lock for fix-agents-md: the guide must lead with a loud
    /// chat-agent role banner so that codex chat agents (whose
    /// "be helpful, do the work" baseline is stronger than their
    /// instruction-following) see the role contract before anything else.
    #[test]
    fn guide_text_leads_with_chat_agent_stop_banner() {
        // The "STOP" banner must appear in the first ~1000 bytes — i.e.
        // before "Three Roles" or any other heading. This guards against
        // a future refactor that buries it.
        let head = &AGENT_GUIDE_TEXT[..AGENT_GUIDE_TEXT.len().min(1500)];
        assert!(
            head.contains("STOP"),
            "agent guide must lead with a STOP banner for chat agents; head was:\n{}",
            head
        );
        assert!(
            head.contains("chat agent"),
            "agent guide head must name the chat-agent role explicitly"
        );
    }

    #[test]
    fn guide_text_lists_chat_agent_anti_patterns() {
        // Concrete forbidden actions — the things codex chat agents have
        // been observed doing instead of dispatching via wg add.
        assert!(
            AGENT_GUIDE_TEXT.contains("DO NOT write code")
                || AGENT_GUIDE_TEXT.contains("DO NOT edit files")
                || AGENT_GUIDE_TEXT.contains("CANNOT do"),
            "agent guide must explicitly forbid chat-agent code-touching actions"
        );
        assert!(
            AGENT_GUIDE_TEXT.contains("cargo build") || AGENT_GUIDE_TEXT.contains("cargo test"),
            "agent guide must call out cargo as a forbidden chat-agent action"
        );
    }

    #[test]
    fn guide_text_lists_chat_agent_allow_list() {
        // Allowed wg commands — chat agents need to know what they CAN do.
        for cmd in ["wg add", "wg show", "wg list", "wg edit"] {
            assert!(
                AGENT_GUIDE_TEXT.contains(cmd),
                "agent guide must list allowed command `{}` in the chat-agent surface",
                cmd
            );
        }
    }

    /// Regression lock: AGENTS.md and CLAUDE.md must stay in lock-step.
    /// Pre-fix, AGENTS.md had inline universal-role-contract content while
    /// CLAUDE.md was layer-2-only — codex chat agents (which read AGENTS.md)
    /// saw a softer / older contract than claude chat agents.
    #[test]
    fn agents_md_and_claude_md_are_layer2_parity() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let agents_md = std::fs::read_to_string(manifest_dir.join("AGENTS.md"))
            .expect("AGENTS.md must exist at repo root");
        let claude_md = std::fs::read_to_string(manifest_dir.join("CLAUDE.md"))
            .expect("CLAUDE.md must exist at repo root");

        // Both must point at the bundled agent-guide as the source of truth.
        for (name, body) in [("AGENTS.md", &agents_md), ("CLAUDE.md", &claude_md)] {
            assert!(
                body.contains("wg agent-guide"),
                "{} must point at `wg agent-guide` for the universal role contract",
                name
            );
            assert!(
                body.contains("layer-2"),
                "{} must declare itself as the layer-2 (project-specific) guide",
                name
            );
            // No inline universal-role-contract content. The pre-fix
            // AGENTS.md had a "Chat agent role" section with the contract
            // inlined — that's exactly what we removed.
            assert!(
                !body.contains("### Chat agent role"),
                "{} must not duplicate the chat-agent contract inline (use `wg agent-guide`)",
                name
            );
            assert!(
                !body.contains("### Smoke gate"),
                "{} must not duplicate the smoke-gate contract inline (use `wg agent-guide`)",
                name
            );
        }

        // Both must reference each other so future edits stay in lockstep.
        assert!(
            agents_md.contains("CLAUDE.md"),
            "AGENTS.md must cross-reference CLAUDE.md (lockstep invariant)"
        );
        assert!(
            claude_md.contains("AGENTS.md"),
            "CLAUDE.md must cross-reference AGENTS.md (lockstep invariant)"
        );
    }
}
