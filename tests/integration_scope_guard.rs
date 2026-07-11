//! R8 — `--scope disposable` guard (docs/02 §3.2 in the family-team project).
//!
//! A task created with `wg add --scope disposable` runs its agent with
//! `WG_SCOPE=disposable`. A disposable-scoped agent must not be able to mint a
//! *persistent* persona: it may not run `wg agent create` or
//! `wg add --tag persistent`. These tests pin the policy at the library
//! boundary so the CLI handlers can rely on it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;
use worksgood::graph::{Node, WorkGraph};
use worksgood::parser::{load_graph, save_graph};
use worksgood::scope_guard::{
    PersistentSpawn, SCOPE_DISPOSABLE, check_scope, resolve_add_scope_for, scope_from_tags,
};

/// The load-bearing R8 policy: a disposable scope forbids every persistent
/// spawn, while unscoped / non-disposable scopes are unaffected.
#[test]
fn test_scoped_disposable_cannot_spawn_persistent() {
    // disposable is denied both privileged spawns
    assert!(
        check_scope(Some(SCOPE_DISPOSABLE), PersistentSpawn::Agent).is_err(),
        "disposable scope must forbid `wg agent create`"
    );
    assert!(
        check_scope(Some(SCOPE_DISPOSABLE), PersistentSpawn::Task).is_err(),
        "disposable scope must forbid `wg add --tag persistent`"
    );

    // unscoped and non-disposable scopes are allowed
    assert!(
        check_scope(None, PersistentSpawn::Agent).is_ok(),
        "unscoped agents may create persistent agents"
    );
    assert!(
        check_scope(Some("persistent"), PersistentSpawn::Task).is_ok(),
        "a persistent-scoped agent may create persistent tasks"
    );
    assert!(
        check_scope(Some("team"), PersistentSpawn::Agent).is_ok(),
        "only the reserved `disposable` scope is restricted"
    );
}

/// Scope is persisted on a task as a `scope:<value>` tag, which is how the
/// dispatcher recovers it to set `WG_SCOPE` on the spawned worker.
#[test]
fn test_scope_persisted_as_tag() {
    assert_eq!(
        scope_from_tags(&["scope:disposable".to_string(), "urgent".to_string()]).as_deref(),
        Some("disposable"),
    );
    assert_eq!(scope_from_tags(&["urgent".to_string()]), None);
}

/// Default-deny at the library boundary: a disposable caller may only ever mint
/// disposable-scoped children.
#[test]
fn test_disposable_caller_default_deny_resolution() {
    // Untagged durable add → inherits scope:disposable (no durable state minted).
    let resolved =
        resolve_add_scope_for(Some(SCOPE_DISPOSABLE), &["urgent".to_string()]).unwrap();
    assert_eq!(
        scope_from_tags(&resolved).as_deref(),
        Some(SCOPE_DISPOSABLE),
        "untagged add from disposable scope must inherit disposable scope"
    );

    // Explicit persistent tag / non-disposable scope → denied.
    assert!(
        resolve_add_scope_for(Some(SCOPE_DISPOSABLE), &["persistent".to_string()]).is_err(),
        "disposable caller must not create a persistent-tagged task"
    );
    assert!(
        resolve_add_scope_for(Some(SCOPE_DISPOSABLE), &["scope:team".to_string()]).is_err(),
        "disposable caller must not escalate a child to a non-disposable scope"
    );

    // Non-disposable callers are unaffected — tags pass through verbatim.
    assert_eq!(
        resolve_add_scope_for(None, &["urgent".to_string()]).unwrap(),
        vec!["urgent".to_string()],
        "unscoped callers must be unaffected"
    );
}

// ── CLI end-to-end regressions (Erik's ask on PR #56) ──────────────────────

fn wg_binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("could not get current exe path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("wg");
    assert!(
        path.exists(),
        "wg binary not found at {:?}. Run `cargo build` first.",
        path
    );
    path
}

fn setup_workgraph(dir: &Path) -> PathBuf {
    let wg_dir = dir.join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();
    let graph = WorkGraph::new();
    save_graph(&graph, &wg_dir.join("graph.jsonl")).unwrap();
    wg_dir
}

fn wg_add(wg_dir: &Path, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
    let mut cmd = Command::new(wg_binary());
    cmd.arg("--dir")
        .arg(wg_dir)
        .arg("add")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Drop ambient agent env so the test hits the temp graph via --dir, not the
    // live project graph, and controls scope explicitly.
    cmd.env_remove("WG_DIR")
        .env_remove("WG_SCOPE")
        .env_remove("WG_TASK_ID");
    // A real disposable worker runs in agent context; simulate it so the add is
    // placed immediately rather than parked as an interactive draft.
    cmd.env("WG_AGENT_ID", "agent-disposable");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("failed to run wg add")
}

fn task_tags(wg_dir: &Path, id: &str) -> Vec<String> {
    let graph = load_graph(&wg_dir.join("graph.jsonl")).unwrap();
    graph
        .get_task(id)
        .unwrap_or_else(|| panic!("task '{id}' not found in graph"))
        .tags
        .clone()
}

/// The Erik regression: `WG_SCOPE=disposable wg add "x"` with no `--tag
/// persistent` must NOT mint durable follow-up work — the child inherits
/// `scope:disposable`.
#[test]
fn cli_disposable_untagged_add_inherits_disposable_scope() {
    let dir = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(dir.path());

    let out = wg_add(
        &wg_dir,
        &["follow up work", "--id", "follow-up"],
        &[("WG_SCOPE", "disposable")],
    );
    assert!(
        out.status.success(),
        "untagged disposable add should succeed as a disposable child.\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        scope_from_tags(&task_tags(&wg_dir, "follow-up")).as_deref(),
        Some(SCOPE_DISPOSABLE),
        "untagged add from disposable scope must inherit scope:disposable, not mint durable work"
    );
}

/// A disposable caller trying to create a persistent-tagged task is refused.
#[test]
fn cli_disposable_persistent_tag_is_refused() {
    let dir = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(dir.path());

    let out = wg_add(
        &wg_dir,
        &["durable thing", "--id", "nope", "--tag", "persistent"],
        &[("WG_SCOPE", "disposable")],
    );
    assert!(
        !out.status.success(),
        "disposable + --tag persistent must be refused"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("disposable"),
        "error should explain the disposable boundary: {stderr}"
    );
    // And nothing was minted.
    let graph = load_graph(&wg_dir.join("graph.jsonl")).unwrap();
    assert!(
        graph.get_task("nope").is_none(),
        "refused add must not create a task"
    );
}

/// A disposable caller trying to escalate a child to a non-disposable scope is refused.
#[test]
fn cli_disposable_scope_escalation_is_refused() {
    let dir = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(dir.path());

    let out = wg_add(
        &wg_dir,
        &["escalate", "--id", "esc", "--scope", "team"],
        &[("WG_SCOPE", "disposable")],
    );
    assert!(
        !out.status.success(),
        "disposable + --scope team must be refused"
    );
}

/// A normal (unscoped) caller is unaffected: no scope tag is injected.
#[test]
fn cli_unscoped_add_is_unaffected() {
    let dir = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(dir.path());

    let out = wg_add(&wg_dir, &["ordinary", "--id", "ordinary"], &[]);
    assert!(
        out.status.success(),
        "unscoped add should succeed.\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        scope_from_tags(&task_tags(&wg_dir, "ordinary")),
        None,
        "unscoped add must not inherit any scope"
    );
}
