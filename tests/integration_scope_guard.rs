//! R8 — `--scope disposable` guard (docs/02 §3.2 in the family-team project).
//!
//! A task created with `wg add --scope disposable` runs its agent with
//! `WG_SCOPE=disposable`. A disposable-scoped agent must not be able to mint
//! durable/persistent graph state: it may not run `wg agent create`,
//! `wg add --tag persistent`, or an ordinary durable `wg add`. From disposable
//! scope the `wg add` boundary is default-deny — the *only* allowed add is an
//! explicit, scope-carrying `--scope disposable` (which persists a
//! `scope:disposable` tag the dispatcher propagates as `WG_SCOPE`). A bare
//! `--tag disposable` is refused: it carries no `scope:` prefix, so the child
//! would spawn unscoped and could mint durable grandchildren (Erik, PR #56
//! rd3). These tests pin the policy at both the library boundary and the real
//! CLI/dispatch wiring.

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

/// Default-deny at the library boundary: from disposable scope only *explicitly*
/// disposable child work is allowed; every other add is refused.
#[test]
fn test_disposable_caller_default_deny_resolution() {
    // Untagged durable add → REFUSED (not silently downgraded). This is Erik's
    // blocking case: a disposable agent may not mint durable follow-up work.
    assert!(
        resolve_add_scope_for(Some(SCOPE_DISPOSABLE), &["urgent".to_string()]).is_err(),
        "untagged durable add from disposable scope must be refused"
    );
    assert!(
        resolve_add_scope_for(Some(SCOPE_DISPOSABLE), &[]).is_err(),
        "a fully untagged add from disposable scope must be refused"
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

    // Bare `disposable` tag → denied (Erik PR #56 rd3): no `scope:` prefix, so
    // plan_spawn would leave the child unscoped. Only `--scope disposable` is ok.
    assert!(
        resolve_add_scope_for(Some(SCOPE_DISPOSABLE), &["disposable".to_string()]).is_err(),
        "a bare `--tag disposable` must be refused — it does not carry scope for propagation"
    );

    // The allowed case: an explicit --scope disposable child passes through, and
    // the returned tag set carries scope:disposable for WG_SCOPE propagation.
    let resolved =
        resolve_add_scope_for(Some(SCOPE_DISPOSABLE), &["scope:disposable".to_string()]).unwrap();
    assert_eq!(
        scope_from_tags(&resolved).as_deref(),
        Some(SCOPE_DISPOSABLE),
        "an explicit --scope disposable child must be allowed from disposable scope"
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
/// persistent` must be REFUSED — a disposable agent may not mint durable
/// follow-up work by omitting the tag, and nothing is minted.
#[test]
fn cli_disposable_untagged_add_is_refused() {
    let dir = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(dir.path());

    let out = wg_add(
        &wg_dir,
        &["follow up work", "--id", "follow-up"],
        &[("WG_SCOPE", "disposable")],
    );
    assert!(
        !out.status.success(),
        "untagged durable add from disposable scope must be refused.\nstdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("disposable"),
        "error should explain the disposable boundary: {stderr}"
    );
    // And nothing durable was minted.
    let graph = load_graph(&wg_dir.join("graph.jsonl")).unwrap();
    assert!(
        graph.get_task("follow-up").is_none(),
        "refused durable add must not create a task"
    );
}

/// The allowed disposable case Erik asked for: `WG_SCOPE=disposable wg add "x"
/// --scope disposable` succeeds and the child carries `scope:disposable` — a
/// disposable agent may spawn an explicitly disposable child.
#[test]
fn cli_disposable_explicit_disposable_add_is_allowed() {
    let dir = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(dir.path());

    let out = wg_add(
        &wg_dir,
        &["scrape child", "--id", "child", "--scope", "disposable"],
        &[("WG_SCOPE", "disposable")],
    );
    assert!(
        out.status.success(),
        "explicit --scope disposable child should be allowed from disposable scope.\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        scope_from_tags(&task_tags(&wg_dir, "child")).as_deref(),
        Some(SCOPE_DISPOSABLE),
        "explicit disposable child must carry scope:disposable"
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

/// Erik's rd3 blocking hole: `WG_SCOPE=disposable wg add child --tag disposable`
/// must be REFUSED. A bare `disposable` tag carries no `scope:` prefix, so the
/// dispatcher's `plan_spawn` would leave the child unscoped — free to mint
/// durable grandchildren. The allowed route must be scope-carrying
/// (`--scope disposable`); the bare tag mints nothing.
#[test]
fn cli_disposable_bare_disposable_tag_add_is_refused() {
    let dir = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(dir.path());

    let out = wg_add(
        &wg_dir,
        &["scrape child", "--id", "bare-child", "--tag", "disposable"],
        &[("WG_SCOPE", "disposable")],
    );
    assert!(
        !out.status.success(),
        "a bare `--tag disposable` from disposable scope must be refused.\nstdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("disposable"),
        "error should explain the disposable boundary: {stderr}"
    );
    // Nothing was minted — no unscoped child slipped through.
    let graph = load_graph(&wg_dir.join("graph.jsonl")).unwrap();
    assert!(
        graph.get_task("bare-child").is_none(),
        "refused bare-tag add must not create an unscoped child"
    );
}

/// Erik's rd3 required regression #1: a real CLI run of `WG_SCOPE=disposable
/// wg agent create ...` is refused AND no persona file is written. The prior
/// `test_scoped_disposable_cannot_spawn_persistent` only exercised `check_scope`
/// directly; this pins the actual `agent_crud` wiring end-to-end.
#[test]
fn cli_disposable_agent_create_is_refused_and_mints_nothing() {
    let dir = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(dir.path());

    let mut cmd = Command::new(wg_binary());
    cmd.arg("--dir")
        .arg(&wg_dir)
        .args([
            "agent",
            "create",
            "scratch-bot",
            "--role",
            "deadbeef",
            "--tradeoff",
            "deadbeef",
            "--executor",
            "claude",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.env_remove("WG_DIR").env_remove("WG_TASK_ID");
    cmd.env("WG_SCOPE", "disposable");
    let out = cmd.output().expect("failed to run wg agent create");

    assert!(
        !out.status.success(),
        "wg agent create from disposable scope must be refused.\nstdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("disposable"),
        "refusal must come from the scope guard (mention `disposable`), not the \
         role/tradeoff check: {stderr}"
    );

    // No persona file was written anywhere under the agency cache.
    let agents_dir = wg_dir.join("agency").join("cache/agents");
    let minted: Vec<_> = fs::read_dir(&agents_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("yaml"))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        minted.is_empty(),
        "refused agent create must not mint a persona file, found: {minted:?}"
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
