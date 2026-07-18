//! Regression lint for active agent-facing WorksGood surfaces.
//!
//! Internal Rust identifiers remain out of scope for this compatibility pass.
//! The only allowed user-facing token is the explicitly labeled `.workgraph`
//! legacy directory fallback in CLI/install migration guidance.

use std::path::Path;

fn repo_file(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

#[test]
fn active_agent_facing_files_do_not_use_retired_branding() {
    let clean_files = [
        "AGENTS.md",
        "CLAUDE.md",
        "Cargo.toml",
        "README.md",
        "docs/README-windows.md",
        "docs/ops/runbook.md",
        "src/text/agent_guide.md",
        "src/commands/quickstart.rs",
        "src/profile/templates/claude.toml",
        "src/profile/templates/codex.toml",
        "src/profile/templates/nex.toml",
        "src/profile/templates/opencode.toml",
        "src/profile/templates/pi.toml",
    ];

    for relative in clean_files {
        let body = repo_file(relative);
        assert!(
            !body.to_ascii_lowercase().contains("workgraph"),
            "retired branding found in active agent-facing file {relative}"
        );
    }
}

#[test]
fn compatibility_mentions_are_explicit_and_cannot_expand() {
    // Exact counts make this an expansion guard, not a broad path exemption.
    let allowlist = [("src/cli.rs", 3usize), ("docs/guides/install.md", 2usize)];
    for (relative, expected) in allowlist {
        let body = repo_file(relative);
        let lines: Vec<_> = body
            .lines()
            .filter(|line| line.to_ascii_lowercase().contains("workgraph"))
            .collect();
        assert_eq!(
            lines.len(),
            expected,
            "compatibility allowlist changed for {relative}; inspect every new occurrence"
        );
        for line in lines {
            assert!(
                line.contains(".workgraph"),
                "only the legacy directory spelling is allowed in {relative}: {line}"
            );
        }
    }
}

#[test]
fn compatibility_source_tokens_are_pinned() {
    // Compatibility-bearing implementation files cannot be globally clean,
    // so pin each intentional retired spelling rather than exempting a whole
    // file. Any new occurrence requires a deliberate review here.
    let allowlist = [
        ("src/config.rs", "home.join(\".workgraph\")", 1usize),
        ("src/config.rs", "config_dir.join(\"workgraph\")", 1usize),
        (
            "src/notify/config.rs",
            "d.join(\"workgraph\").join(\"notify.toml\")",
            1usize,
        ),
        ("src/commands/setup.rs", "<!-- WG-managed -->", 2usize),
        ("src/commands/setup.rs", "<!-- wg-managed -->", 2usize),
        (
            "src/commands/setup.rs",
            "<!-- workgraph-managed -->",
            1usize,
        ),
    ];

    for (relative, needle, expected) in allowlist {
        let body = repo_file(relative);
        assert_eq!(
            body.matches(needle).count(),
            expected,
            "compatibility token count changed for {relative}: {needle}"
        );
    }
}

#[test]
fn generated_schema_and_cli_keys_use_graph_terminology() {
    let function_sources = [
        "src/function.rs",
        "src/commands/func_apply.rs",
        "src/commands/func_bootstrap.rs",
        "src/commands/func_cmd.rs",
        "src/commands/func_extract.rs",
        "src/commands/func_make_adaptive.rs",
    ];
    for relative in function_sources {
        let body = repo_file(relative);
        assert!(
            !body.contains("workgraph-yaml"),
            "retired generated format leaked from {relative}"
        );
    }

    let peer = repo_file("src/commands/peer.rs");
    assert!(!peer.contains("obj[\"workgraph_dir\"]"));
    assert!(peer.contains("obj[\"graph_dir\"]"));
}
