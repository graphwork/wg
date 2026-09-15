use serde_json::Value;
use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};
use worksgood::disk_sentinel::{CacheKind, load_ownership};
use worksgood::service::registry::AgentRegistry;

fn candidate_wg() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_wg"))
}

fn run_wg(home: &Path, wg_dir: &Path, args: &[&str]) -> Output {
    let binary = candidate_wg();
    let binary_dir = binary.parent().expect("candidate binary parent");
    let path = format!(
        "{}:{}",
        binary_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let real_home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    Command::new(&binary)
        .arg("--dir")
        .arg(wg_dir)
        .args(args)
        .current_dir(wg_dir.parent().expect("project root"))
        .env("HOME", home)
        .env("WG_GLOBAL_DIR", home.join(".wg"))
        .env("PATH", path)
        .env(
            "CARGO_HOME",
            std::env::var("CARGO_HOME").unwrap_or_else(|_| format!("{real_home}/.cargo")),
        )
        .env(
            "RUSTUP_HOME",
            std::env::var("RUSTUP_HOME").unwrap_or_else(|_| format!("{real_home}/.rustup")),
        )
        .env_remove("WG_DIR")
        .env_remove("WG_PROJECT_ROOT")
        .env_remove("WG_WORKTREE_PATH")
        .env_remove("WG_WORKTREE_ACTIVE")
        .env_remove("WG_BRANCH")
        .env_remove("WG_TASK_ID")
        .env_remove("WG_AGENT_ID")
        .env_remove("WG_GRAPH_ID")
        .env_remove("WG_WORKER_CAPABILITY")
        .env_remove("WG_WORKER_IPC")
        .env_remove("WG_WORKER_CONTROL_PROTOCOL")
        .env_remove("WG_WORKER_CONTROL_MODE")
        .env_remove("WG_WORKER_ATTEMPT_FENCE")
        .env_remove("WG_WORKER_ATTEMPT_ID")
        .env_remove("WG_WORKER_GENERATION")
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|error| panic!("run candidate wg {args:?}: {error}"))
}

fn wg_ok(home: &Path, wg_dir: &Path, args: &[&str]) -> String {
    let output = run_wg(home, wg_dir, args);
    assert!(
        output.status.success(),
        "candidate wg {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn git(project: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(project)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn available_bytes(path: &Path) -> u64 {
    let path = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::statvfs(path.as_ptr(), &mut stat) }, 0);
    stat.f_bavail.saturating_mul(stat.f_frsize)
}

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let started = Instant::now();
    while !predicate() {
        assert!(
            started.elapsed() < timeout,
            "condition not satisfied within {timeout:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn daemon_pid(wg_dir: &Path) -> u32 {
    serde_json::from_slice::<Value>(&fs::read(wg_dir.join("service/state.json")).unwrap()).unwrap()
        ["pid"]
        .as_u64()
        .unwrap() as u32
}

struct DaemonGuard {
    home: PathBuf,
    wg_dir: PathBuf,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = run_wg(
            &self.home,
            &self.wg_dir,
            &["service", "stop", "--force", "--kill-agents"],
        );
        if let Ok(bytes) = fs::read(self.wg_dir.join("service/state.json"))
            && let Ok(state) = serde_json::from_slice::<Value>(&bytes)
            && let Some(pid) = state["pid"].as_i64()
        {
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
    }
}

fn contains_build_artifact(target: &Path) -> bool {
    walkdir::WalkDir::new(target)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .any(|entry| {
            entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "rmeta")
        })
}

#[test]
fn test_dispatcher_config_roundtrip() {
    let mut cfg = worksgood::config::Config::default();
    cfg.coordinator.max_agents = 42;
    let toml_str = toml::to_string_pretty(&cfg).unwrap();
    eprintln!("=== TOML ===");
    eprintln!("{}", toml_str);

    let reload: worksgood::config::Config = toml::from_str(&toml_str).unwrap();
    eprintln!(
        "=== RELOADED max_agents = {} ===",
        reload.coordinator.max_agents
    );
    assert_eq!(reload.coordinator.max_agents, 42);
}

/// Real public service flow plus the candidate CLI's production cleanup
/// publication event. The first process builds a cold layer and exits; cleanup
/// publishes exact READY bytes, and the next tick must admit the warm follower
/// without restarting the daemon while preserving the disk-capacity bound.
#[test]
fn live_daemon_refreshes_exact_baseline_publication_without_restart() {
    let fixture = tempfile::Builder::new()
        .prefix("wg-baseline-admission-")
        .tempdir_in(std::env::temp_dir())
        .unwrap();
    let project = fixture.path().join("project");
    let home = fixture.path().join("home");
    let cache = fixture.path().join("target-cache");
    let wg_dir = project.join(".wg");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&cache).unwrap();

    git(&project, &["init", "-q", "-b", "main"]);
    git(&project, &["config", "user.name", "WG Baseline Test"]);
    git(&project, &["config", "user.email", "wg@example.invalid"]);
    fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"wg_baseline_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(project.join("src/main.rs"), "fn main() {}\n").unwrap();
    let cargo = Command::new("cargo")
        .arg("generate-lockfile")
        .arg("--quiet")
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(
        cargo.status.success(),
        "cargo generate-lockfile failed: {}",
        String::from_utf8_lossy(&cargo.stderr)
    );
    git(
        &project,
        &["add", "Cargo.toml", "Cargo.lock", "src/main.rs"],
    );
    git(&project, &["commit", "-qm", "cargo fixture"]);

    wg_ok(&home, &wg_dir, &["init", "--no-agency"]);
    let free = available_bytes(&cache);
    let warm_delta = (free / 16).max(16 * 1024 * 1024);
    let cold_baseline = warm_delta.saturating_mul(4);
    assert!(
        free > cold_baseline.saturating_add(warm_delta),
        "fixture filesystem lacks bounded admission headroom"
    );
    let warning_floor = free.saturating_sub(cold_baseline + warm_delta / 2);
    fs::write(
        wg_dir.join("config.toml"),
        format!(
            r#"[agency]
auto_assign = false
auto_evaluate = false

[dispatcher]
max_agents = 2
poll_interval = 3
graph_watch_enabled = false
settling_delay_ms = 0
worktree_isolation = false

[dispatcher.resource_management]
disk_sentinel_enabled = true
cargo_target_root = "{}"
disk_warning_bytes = {}
disk_pause_build_bytes = 0
disk_hard_refuse_bytes = 0
disk_warning_percent = 0.0
disk_pause_build_percent = 0.0
disk_hard_refuse_percent = 0.0
estimated_build_bytes = {}
estimated_build_heavy_bytes = {}
estimated_cargo_baseline_bytes = {}
build_link_test_safety_bytes = 0
max_build_agents = 2
owned_cache_lease_seconds = 1
disk_agent_heartbeat_seconds = 60
"#,
            cache.display(),
            warning_floor,
            warm_delta,
            cold_baseline.saturating_mul(2),
            cold_baseline,
        ),
    )
    .unwrap();
    wg_ok(
        &home,
        &wg_dir,
        &[
            "config",
            "--local",
            "--model",
            "codex:gpt-5.5",
            "--no-reload",
        ],
    );
    git(
        &project,
        &[
            "add",
            ".gitignore",
            "AGENTS.md",
            "CLAUDE.md",
            "worksgood.toml",
        ],
    );
    git(&project, &["commit", "-qm", "wg fixture"]);
    assert!(
        Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&project)
            .output()
            .unwrap()
            .stdout
            .is_empty(),
        "baseline source must be clean"
    );

    let exact_command = "cargo check && sleep 10";
    for (id, title, priority) in [
        ("a-cold-builder", "exact Cargo cold builder", "100"),
        ("b-warm-follower", "exact Cargo warm follower", "20"),
        (
            "c-capacity-follower",
            "build-heavy exact Cargo capacity follower",
            "10",
        ),
    ] {
        wg_ok(
            &home,
            &wg_dir,
            &[
                "add",
                title,
                "--id",
                id,
                "--priority",
                priority,
                "--exec",
                exact_command,
                "--exec-mode",
                "shell",
            ],
        );
        wg_ok(&home, &wg_dir, &["publish", id, "--only"]);
    }

    wg_ok(
        &home,
        &wg_dir,
        &[
            "service",
            "start",
            "--max-agents",
            "2",
            "--no-chat-agent",
            "--interval",
            "3",
        ],
    );
    let _guard = DaemonGuard {
        home: home.clone(),
        wg_dir: wg_dir.clone(),
    };
    let original_pid = daemon_pid(&wg_dir);

    let mut cold_status = String::new();
    wait_until(Duration::from_secs(15), || {
        cold_status = wg_ok(&home, &wg_dir, &["service", "status"]);
        cold_status.contains("active baseline builder task 'a-cold-builder'")
            && cold_status.contains("next action")
    });
    assert!(!cold_status.contains("WG-EXEC-ROUTE-MISSING"));
    let registry = AgentRegistry::load(&wg_dir).unwrap();
    assert_eq!(
        registry
            .all()
            .filter(|agent| agent.task_id == "a-cold-builder")
            .count(),
        1
    );
    assert_eq!(
        registry
            .all()
            .filter(|agent| agent.task_id == "b-warm-follower")
            .count(),
        0
    );
    let graph = worksgood::parser::load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let waiting = graph.get_task("b-warm-follower").unwrap();
    assert_eq!(waiting.status, worksgood::graph::Status::Open);
    assert_eq!(waiting.lifecycle.attempt_sequence, 0);

    let mut builder_target = None;
    wait_until(Duration::from_secs(15), || {
        builder_target = load_ownership(&wg_dir)
            .unwrap()
            .caches
            .into_iter()
            .find(|cache| cache.task_id == "a-cold-builder" && cache.kind == CacheKind::CargoTarget)
            .map(|cache| PathBuf::from(cache.path));
        builder_target
            .as_deref()
            .is_some_and(contains_build_artifact)
    });

    // Let the real shell process finish, then exercise the production cleanup
    // publication event through the candidate CLI. The daemon stays alive and
    // the graph watcher is disabled, so the follower cannot race a pre-publish
    // coordinator tick into becoming another cold builder.
    wait_until(Duration::from_secs(15), || {
        let graph = worksgood::parser::load_graph(wg_dir.join("graph.jsonl")).unwrap();
        let registry = AgentRegistry::load(&wg_dir).unwrap();
        let owner_retired = registry
            .all()
            .find(|agent| agent.task_id == "a-cold-builder")
            .is_none_or(|agent| !worksgood::service::is_process_alive(agent.pid));
        graph
            .get_task("a-cold-builder")
            .is_some_and(|task| task.status.is_terminal())
            && owner_retired
    });
    let publication = wg_ok(&home, &wg_dir, &["disk", "cleanup", "--execute"]);
    assert!(
        publication.contains("promoted clean completed layer to immutable shared baseline"),
        "production cleanup did not publish the baseline: {publication}"
    );
    assert!(
        worksgood::target_cache::has_ready_baseline(&cache, &project, Some(exact_command)),
        "publication must match the candidate's exact source/toolchain/command key"
    );
    let replay = wg_ok(&home, &wg_dir, &["disk", "cleanup", "--execute"]);
    assert!(
        !replay.contains("promoted clean completed layer to immutable shared baseline"),
        "replayed publication was not idempotent: {replay}"
    );

    wait_until(Duration::from_secs(20), || {
        AgentRegistry::load(&wg_dir).unwrap().all().any(|agent| {
            agent.task_id == "b-warm-follower"
                && agent.is_live(60)
                && worksgood::service::is_process_alive(agent.pid)
        })
    });
    assert_eq!(daemon_pid(&wg_dir), original_pid, "daemon restarted");

    // Let repeated safety ticks observe the same READY publication. The disk
    // projection must remain authoritative: no duplicate follower and no
    // build-heavy third admission while that follower remains live.
    std::thread::sleep(Duration::from_secs(2));
    let registry = AgentRegistry::load(&wg_dir).unwrap();
    assert_eq!(
        registry
            .all()
            .filter(|agent| agent.task_id == "b-warm-follower")
            .count(),
        1
    );
    assert_eq!(
        registry
            .all()
            .filter(|agent| agent.task_id == "c-capacity-follower")
            .count(),
        0
    );
    let graph = worksgood::parser::load_graph(wg_dir.join("graph.jsonl")).unwrap();
    assert_eq!(
        graph
            .get_task("b-warm-follower")
            .unwrap()
            .lifecycle
            .attempt_sequence,
        1
    );
    let capacity_waiter = graph.get_task("c-capacity-follower").unwrap();
    assert_eq!(capacity_waiter.status, worksgood::graph::Status::Open);
    assert_eq!(capacity_waiter.lifecycle.attempt_sequence, 0);
}

#[test]
fn owned_build_baseline_smoke_scenario_passes_candidate_harness() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest_path = repo.join("tests/smoke/manifest.toml");
    let manifest = worksgood::smoke::Manifest::load_from(&manifest_path).unwrap();
    let scenarios = manifest.scenarios_for_task("refresh-build-baseline-admission");
    assert_eq!(scenarios.len(), 1, "owned smoke registration drifted");
    let report = worksgood::smoke::run_scenarios(
        &scenarios,
        manifest_path.parent().expect("smoke manifest parent"),
    );
    assert!(!report.blocks_done(), "{}", report.render());
}
