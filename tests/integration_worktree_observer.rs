use std::fs;
use std::process::Command;

use tempfile::tempdir;
use worksgood::graph::{Status, Task};
use worksgood::worktree_observer::{
    CandidatePathPolicy, DeadlineInput, ObserverConfig, ObserverIdentity, ReconcileSource,
    WorktreeObserver, calculate_suspect_deadline,
};

fn git(root: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {:?} failed", args);
}

fn repo() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    git(
        dir.path(),
        &["config", "user.email", "observer@test.invalid"],
    );
    git(dir.path(), &["config", "user.name", "Observer Test"]);
    fs::write(dir.path().join("tracked.txt"), "base\n").unwrap();
    git(dir.path(), &["add", "tracked.txt"]);
    git(dir.path(), &["commit", "-qm", "base"]);
    dir
}

fn identity() -> ObserverIdentity {
    ObserverIdentity {
        task_id: "task-a".into(),
        generation: 2,
        attempt_id: "attempt-2-1".into(),
        attempt_fence: 7,
        worktree_id: "wt-a".into(),
        worktree_lease_epoch: 3,
        process_epoch: 0,
        observer_epoch: 11,
    }
}

#[test]
fn case_distinct_paths_retain_exact_manifest_identity_or_fail_materialization_explicitly() {
    let root = repo();
    let upper_name = "PROMPT_CONSTRUCTION_ANALYSIS.md";
    let lower_name = "prompt_construction_analysis.md";
    let upper = root.path().join(upper_name);
    let lower = root.path().join(lower_name);
    fs::write(&upper, "upper\n").unwrap();
    fs::write(&lower, "lower\n").unwrap();
    let materializes_both =
        fs::read_to_string(&upper).unwrap() != fs::read_to_string(&lower).unwrap();

    if !materializes_both {
        // Put both exact spellings in the index without asking checkout to
        // materialize them. On a case-insensitive filesystem the walk reports
        // one spelling while both exact paths resolve to that same entry; the
        // observer must reject that mismatch explicitly rather than silently
        // collapsing an index identity.
        let output = Command::new("git")
            .arg("-C")
            .arg(root.path())
            .args(["hash-object", "-w", upper_name])
            .output()
            .unwrap();
        assert!(output.status.success());
        let oid = String::from_utf8(output.stdout).unwrap();
        let oid = oid.trim();
        for name in [upper_name, lower_name] {
            git(
                root.path(),
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("100644,{oid},{name}"),
                ],
            );
        }
        let storage = tempdir().unwrap();
        let error = match WorktreeObserver::attach_at(
            root.path(),
            storage.path(),
            identity(),
            CandidatePathPolicy::new(vec![], vec![]).unwrap(),
            ObserverConfig::default(),
            1_000,
        ) {
            Ok(_) => panic!("case-insensitive materialization mismatch was accepted"),
            Err(error) => error,
        };
        assert!(
            format!("{error:#}").contains("tracked-path-materialization-is-not-exact"),
            "case-insensitive materialization mismatch was not explicit: {error:#}"
        );
        return;
    }

    git(root.path(), &["add", upper_name, lower_name]);
    git(root.path(), &["commit", "-qm", "case-distinct paths"]);

    let storage = tempdir().unwrap();
    let mut observer = WorktreeObserver::attach_at(
        root.path(),
        storage.path(),
        identity(),
        CandidatePathPolicy::new(vec![], vec![]).unwrap(),
        ObserverConfig::default(),
        1_000,
    )
    .expect("case-distinct paths in a valid checkout must not block observer attach");

    let baseline: serde_json::Value =
        serde_json::from_slice(&fs::read(storage.path().join("baseline.json")).unwrap()).unwrap();
    let entries = baseline["manifest"]["entries"].as_object().unwrap();
    assert!(entries.contains_key(upper_name));
    assert!(entries.contains_key(lower_name));
    assert_eq!(entries[upper_name]["path"], upper_name);
    assert_eq!(entries[lower_name]["path"], lower_name);

    fs::write(&upper, "upper changed\n").unwrap();
    observer
        .reconcile_at(ReconcileSource::Event, 1_010)
        .unwrap();
    assert_eq!(
        observer
            .projection()
            .last_activity
            .as_ref()
            .unwrap()
            .changed_paths[0]
            .path,
        upper_name
    );
    fs::write(&lower, "lower changed\n").unwrap();
    observer
        .reconcile_at(ReconcileSource::Event, 1_020)
        .unwrap();
    assert_eq!(
        observer
            .projection()
            .last_activity
            .as_ref()
            .unwrap()
            .changed_paths[0]
            .path,
        lower_name
    );
}

#[test]
fn exact_root_content_and_same_bytes_are_domain_separated() {
    let root = repo();
    let storage = tempdir().unwrap();
    let main = repo();
    let mut observer = WorktreeObserver::attach_at(
        root.path(),
        storage.path(),
        identity(),
        CandidatePathPolicy::new(vec![], vec![]).unwrap(),
        ObserverConfig::default(),
        1_000,
    )
    .unwrap();

    fs::write(main.path().join("tracked.txt"), "main is different\n").unwrap();
    observer
        .reconcile_at(ReconcileSource::Periodic, 1_010)
        .unwrap();
    assert_eq!(observer.projection().content_seq, 0, "main must be inert");

    fs::write(root.path().join("tracked.txt"), "candidate\n").unwrap();
    observer
        .reconcile_at(ReconcileSource::Event, 1_020)
        .unwrap();
    assert_eq!(observer.projection().content_seq, 1);
    assert_eq!(
        observer
            .projection()
            .last_activity
            .as_ref()
            .unwrap()
            .observed_at,
        1_020
    );

    fs::write(root.path().join("tracked.txt"), "candidate\n").unwrap();
    observer
        .reconcile_at(ReconcileSource::Event, 1_030)
        .unwrap();
    assert_eq!(
        observer.projection().content_seq,
        1,
        "same bytes cannot mint activity"
    );

    let moved = root.path().with_extension("old");
    fs::rename(root.path(), &moved).unwrap();
    fs::create_dir(root.path()).unwrap();
    fs::write(root.path().join("tracked.txt"), "replacement\n").unwrap();
    observer
        .reconcile_at(ReconcileSource::Periodic, 1_040)
        .unwrap();
    assert_eq!(observer.projection().content_seq, 1);
    assert!(observer.projection().classification_hold.is_some());
}

#[test]
fn classifier_tracks_candidate_semantics_and_ignores_volatile_churn() {
    let root = repo();
    let storage = tempdir().unwrap();
    fs::write(root.path().join(".gitignore"), "generated/\n").unwrap();
    fs::write(root.path().join("tracked.log"), "tracked baseline\n").unwrap();
    fs::create_dir(root.path().join("dist")).unwrap();
    fs::write(
        root.path().join("dist/generated.js"),
        "generated baseline\n",
    )
    .unwrap();
    git(root.path(), &["add", "tracked.log", "dist/generated.js"]);
    git(
        root.path(),
        &["commit", "-qm", "track log and generated output"],
    );
    fs::create_dir(root.path().join("target")).unwrap();
    fs::create_dir(root.path().join("generated")).unwrap();
    let policy =
        CandidatePathPolicy::new(vec!["generated/answer.log".into()], vec!["dist/**".into()])
            .unwrap();
    let mut observer = WorktreeObserver::attach_at(
        root.path(),
        storage.path(),
        identity(),
        policy,
        ObserverConfig::default(),
        2_000,
    )
    .unwrap();

    fs::write(root.path().join("target/churn.log"), "1").unwrap();
    observer
        .reconcile_at(ReconcileSource::Event, 2_010)
        .unwrap();
    assert_eq!(observer.projection().content_seq, 0);
    assert!(
        observer
            .projection()
            .ignored_churn
            .get("volatile-target")
            .copied()
            .unwrap_or(0)
            >= 1
    );
    fs::write(root.path().join("dist/generated.js"), "generated churn\n").unwrap();
    observer
        .reconcile_at(ReconcileSource::Event, 2_015)
        .unwrap();
    assert_eq!(
        observer.projection().content_seq,
        0,
        "snapshotted generated policy overrides tracked inclusion"
    );

    fs::write(root.path().join("generated/answer.log"), "deliverable\n").unwrap();
    observer
        .reconcile_at(ReconcileSource::Event, 2_020)
        .unwrap();
    assert_eq!(
        observer.projection().content_seq,
        1,
        "explicit deliverable overrides generated/ignored"
    );

    fs::write(root.path().join("tracked.txt"), "changed\n").unwrap();
    observer
        .reconcile_at(ReconcileSource::Event, 2_030)
        .unwrap();
    assert_eq!(observer.projection().content_seq, 2);
    assert!(
        observer
            .projection()
            .last_activity
            .as_ref()
            .unwrap()
            .changed_paths
            .iter()
            .any(|p| p.path == "tracked.txt")
    );

    // .gitignore is candidate content/effect input, not observer control state.
    fs::write(root.path().join(".gitignore"), "generated/\nother-cache/\n").unwrap();
    observer
        .reconcile_at(ReconcileSource::Event, 2_040)
        .unwrap();
    assert_eq!(observer.projection().content_seq, 3);

    fs::write(root.path().join("tracked.log"), "tracked log changed\n").unwrap();
    observer
        .reconcile_at(ReconcileSource::Event, 2_050)
        .unwrap();
    assert_eq!(
        observer.projection().content_seq,
        4,
        "tracked .log must not be globally excluded"
    );

    fs::write(root.path().join("stage-me.txt"), "same candidate bytes\n").unwrap();
    observer
        .reconcile_at(ReconcileSource::Event, 2_060)
        .unwrap();
    assert_eq!(observer.projection().content_seq, 5);
    git(root.path(), &["add", "stage-me.txt"]);
    observer
        .reconcile_at(ReconcileSource::Event, 2_070)
        .unwrap();
    assert_eq!(
        observer.projection().content_seq,
        5,
        "index-only class churn must not advance candidate semantics"
    );
}

#[test]
fn observer_config_rejects_oversized_or_replenishing_windows() {
    let mut config = ObserverConfig::default();
    config.debounce_ms = 1;
    assert!(config.validate().is_err());
    config = ObserverConfig::default();
    config.reconcile_interval_secs = 0;
    assert!(config.validate().is_err());
    config = ObserverConfig::default();
    config.observed_activity_grace_secs = 601;
    assert!(config.validate().is_err());
    config = ObserverConfig::default();
    config.max_observed_only_extension_secs = 60;
    assert!(config.validate().is_err());
    config = ObserverConfig::default();
    config.generated_paths = vec!["../outside/**".into()];
    assert!(config.validate().is_err());

    let loaded: worksgood::config::Config = toml::from_str(
        "[worktree_observer]\ndebounce_ms=1\nreconcile_interval_secs=15\nobserved_activity_grace_secs=120\nmax_observed_only_extension_secs=600\n",
    )
    .unwrap();
    assert!(
        loaded
            .validate_config()
            .errors
            .iter()
            .any(|diagnostic| diagnostic.rule == "worktree-observer-policy")
    );
}

#[test]
fn deadline_keeps_proven_and_observed_clocks_independent_and_bounded() {
    let defaults = ObserverConfig::default();
    assert_eq!(defaults.reconcile_interval_secs, 15);
    assert_eq!(defaults.observed_activity_grace_secs, 120);
    assert_eq!(defaults.max_observed_only_extension_secs, 600);

    let none = calculate_suspect_deadline(DeadlineInput {
        last_proven_at: 10_000,
        last_proven_seq: 4,
        last_observed_at: None,
        observed_after_proven_seq: None,
        meaningful_silence_secs: 300,
        observed_activity_grace_secs: 120,
        max_observed_only_extension_secs: 600,
    });
    assert_eq!(none.proof_deadline, 10_300);
    assert_eq!(none.suspect_at, 10_300);

    let gradual = calculate_suspect_deadline(DeadlineInput {
        last_observed_at: Some(10_420),
        observed_after_proven_seq: Some(4),
        ..DeadlineInput {
            last_proven_at: 10_000,
            last_proven_seq: 4,
            last_observed_at: None,
            observed_after_proven_seq: None,
            meaningful_silence_secs: 300,
            observed_activity_grace_secs: 120,
            max_observed_only_extension_secs: 600,
        }
    });
    assert_eq!(gradual.observed_deadline, Some(10_540));
    assert_eq!(gradual.suspect_at, 10_540);

    let rewrite_loop = calculate_suspect_deadline(DeadlineInput {
        last_observed_at: Some(99_999),
        observed_after_proven_seq: Some(4),
        ..DeadlineInput {
            last_proven_at: 10_000,
            last_proven_seq: 4,
            last_observed_at: None,
            observed_after_proven_seq: None,
            meaningful_silence_secs: 300,
            observed_activity_grace_secs: 120,
            max_observed_only_extension_secs: 600,
        }
    });
    assert_eq!(
        rewrite_loop.suspect_at, 10_900,
        "observed-only rewriting must hit the hard cap"
    );
}

#[test]
fn restart_reconciles_once_without_inventing_mtime_activity() {
    let root = repo();
    let storage = tempdir().unwrap();
    let policy = CandidatePathPolicy::new(vec![], vec![]).unwrap();
    let mut observer = WorktreeObserver::attach_at(
        root.path(),
        storage.path(),
        identity(),
        policy.clone(),
        ObserverConfig::default(),
        3_000,
    )
    .unwrap();
    let pre_advance_state = fs::read(storage.path().join("state.json")).unwrap();
    fs::write(root.path().join("tracked.txt"), "after baseline\n").unwrap();
    observer
        .reconcile_at(ReconcileSource::Startup, 3_010)
        .unwrap();
    assert_eq!(observer.projection().content_seq, 1);
    drop(observer);

    // Simulate crash after the fsynced journal append but before the derived
    // projection rename. Reconciliation must reuse the original record/time.
    fs::write(storage.path().join("state.json"), &pre_advance_state).unwrap();
    let mut restarted = WorktreeObserver::open_at(storage.path(), identity(), 9_000).unwrap();
    restarted
        .reconcile_at(ReconcileSource::Startup, 9_000)
        .unwrap();
    assert_eq!(restarted.projection().content_seq, 1);
    assert_eq!(
        restarted
            .projection()
            .last_activity
            .as_ref()
            .unwrap()
            .observed_at,
        3_010
    );
    assert_eq!(
        fs::read_to_string(storage.path().join("activity.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1,
        "projection crash replay must dedupe the prior/new manifest record"
    );

    fs::remove_file(storage.path().join("baseline.json")).unwrap();
    fs::remove_file(storage.path().join("state.json")).unwrap();
    let recovered = WorktreeObserver::recover_without_baseline_at(
        root.path(),
        storage.path(),
        identity(),
        policy,
        ObserverConfig::default(),
        10_000,
    )
    .unwrap();
    assert!(recovered.projection().baseline_time_unknown);
    assert_eq!(recovered.projection().content_seq, 0);
    assert!(recovered.projection().last_activity.is_none());
}

#[test]
fn watchdog_process_epoch_rebind_fences_old_callbacks_without_resetting_baseline() {
    let root = repo();
    let storage = tempdir().unwrap();
    let mut observer = WorktreeObserver::attach_at(
        root.path(),
        storage.path(),
        identity(),
        CandidatePathPolicy::new(vec![], vec![]).unwrap(),
        ObserverConfig::default(),
        3_500,
    )
    .unwrap();
    let old = observer.projection().source.identity.clone();
    let current = observer
        .rebind_process_epoch_from_watchdog_at(&old, 1, 3_510)
        .unwrap();
    assert_eq!(current.process_epoch, 1);
    assert!(current.observer_epoch > old.observer_epoch);
    fs::write(root.path().join("tracked.txt"), "new epoch bytes\n").unwrap();
    assert_eq!(
        observer
            .reconcile_callback_at(&old, ReconcileSource::Event, 3_520)
            .unwrap(),
        worksgood::worktree_observer::ReconcileOutcome::StaleCallback,
    );
    assert_eq!(observer.projection().content_seq, 0);
    observer
        .reconcile_callback_at(&current, ReconcileSource::Event, 3_530)
        .unwrap();
    assert_eq!(observer.projection().content_seq, 1);
    drop(observer);
    let reopened = WorktreeObserver::open_at(storage.path(), current, 9_000).unwrap();
    assert_eq!(reopened.projection().content_seq, 1);
}

#[test]
fn fenced_and_post_reap_writes_are_preserved_but_never_authoritative() {
    let root = repo();
    let storage = tempdir().unwrap();
    let mut observer = WorktreeObserver::attach_at(
        root.path(),
        storage.path(),
        identity(),
        CandidatePathPolicy::new(vec![], vec![]).unwrap(),
        ObserverConfig::default(),
        4_000,
    )
    .unwrap();
    observer.enter_preservation_at(false, 4_010).unwrap();
    fs::write(root.path().join("tracked.txt"), "late\n").unwrap();
    observer
        .reconcile_at(ReconcileSource::Event, 4_020)
        .unwrap();
    assert_eq!(
        observer.projection().content_seq,
        0,
        "late bytes cannot become current progress"
    );
    assert_eq!(observer.projection().late_mutations.len(), 1);
    assert!(observer.projection().quarantine_required);

    observer.enter_preservation_at(true, 4_030).unwrap();
    fs::write(root.path().join("tracked.txt"), "later still\n").unwrap();
    observer
        .reconcile_at(ReconcileSource::Periodic, 4_040)
        .unwrap();
    assert!(
        observer
            .projection()
            .late_mutations
            .iter()
            .any(|m| m.reason == "late-write-after-reap")
    );
}

#[cfg(unix)]
#[test]
fn atomic_replace_rename_mode_symlink_overflow_and_stale_callback_converge() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = repo();
    let storage = tempdir().unwrap();
    let mut observer = WorktreeObserver::attach_at(
        root.path(),
        storage.path(),
        identity(),
        CandidatePathPolicy::new(vec![], vec![]).unwrap(),
        ObserverConfig::default(),
        4_500,
    )
    .unwrap();

    fs::write(root.path().join("replacement"), "atomic\n").unwrap();
    fs::rename(
        root.path().join("replacement"),
        root.path().join("tracked.txt"),
    )
    .unwrap();
    observer
        .reconcile_at(ReconcileSource::Event, 4_510)
        .unwrap();
    assert_eq!(observer.projection().content_seq, 1);

    let mut permissions = fs::metadata(root.path().join("tracked.txt"))
        .unwrap()
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(root.path().join("tracked.txt"), permissions).unwrap();
    observer
        .reconcile_at(ReconcileSource::Event, 4_520)
        .unwrap();
    assert_eq!(observer.projection().content_seq, 2);
    assert!(
        observer
            .projection()
            .last_activity
            .as_ref()
            .unwrap()
            .changed_paths
            .iter()
            .any(|p| p.operation == "mode-change")
    );

    symlink("tracked.txt", root.path().join("link")).unwrap();
    observer
        .reconcile_at(ReconcileSource::Event, 4_530)
        .unwrap();
    assert_eq!(observer.projection().content_seq, 3);
    fs::remove_file(root.path().join("link")).unwrap();
    fs::write(root.path().join("renamed.txt"), "atomic\n").unwrap();
    fs::remove_file(root.path().join("tracked.txt")).unwrap();
    observer
        .reconcile_at(ReconcileSource::Periodic, 4_540)
        .unwrap();
    let paths = &observer
        .projection()
        .last_activity
        .as_ref()
        .unwrap()
        .changed_paths;
    assert!(paths.iter().any(|p| p.operation == "delete"));
    assert!(paths.iter().any(|p| p.operation == "add"));

    observer.mark_overflow_at("queue overflow", 4_550).unwrap();
    assert_eq!(observer.projection().watcher_overflows, 1);
    let mut stale = identity();
    stale.observer_epoch += 1;
    assert_eq!(
        observer
            .reconcile_callback_at(&stale, ReconcileSource::Event, 4_560)
            .unwrap(),
        worksgood::worktree_observer::ReconcileOutcome::StaleCallback
    );
    assert_eq!(observer.projection().content_seq, 4);
    observer
        .mark_watcher_unavailable_at("unsupported filesystem", 4_570)
        .unwrap();
    assert_eq!(
        observer.projection().health,
        worksgood::worktree_observer::ObserverHealth::PollOnly
    );
    assert!(
        observer
            .projection()
            .degraded_reason
            .as_deref()
            .unwrap()
            .starts_with("watcher-unavailable:")
    );
}

#[test]
fn gitlink_identity_change_is_candidate_activity_without_following_submodule() {
    let root = repo();
    fs::write(root.path().join("second.txt"), "second\n").unwrap();
    git(root.path(), &["add", "second.txt"]);
    git(root.path(), &["commit", "-qm", "second"]);
    let old_oid = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(root.path())
            .args(["rev-parse", "HEAD~1"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let new_oid = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(root.path())
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    git(
        root.path(),
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{},vendor/sub", old_oid.trim()),
        ],
    );
    let storage = tempdir().unwrap();
    let mut observer = WorktreeObserver::attach_at(
        root.path(),
        storage.path(),
        identity(),
        CandidatePathPolicy::new(vec![], vec![]).unwrap(),
        ObserverConfig::default(),
        4_600,
    )
    .unwrap();
    git(
        root.path(),
        &[
            "update-index",
            "--cacheinfo",
            &format!("160000,{},vendor/sub", new_oid.trim()),
        ],
    );
    observer
        .reconcile_at(ReconcileSource::Event, 4_610)
        .unwrap();
    assert_eq!(observer.projection().content_seq, 1);
    assert!(
        observer
            .projection()
            .last_activity
            .as_ref()
            .unwrap()
            .changed_paths
            .iter()
            .any(|p| p.path == "vendor/sub" && p.operation == "gitlink-change")
    );
}

#[cfg(unix)]
#[test]
fn special_and_escaping_symlink_entries_hold_instead_of_guessing() {
    use std::os::unix::fs::symlink;
    let root = repo();
    let storage = tempdir().unwrap();
    let mut observer = WorktreeObserver::attach_at(
        root.path(),
        storage.path(),
        identity(),
        CandidatePathPolicy::new(vec![], vec![]).unwrap(),
        ObserverConfig::default(),
        4_700,
    )
    .unwrap();
    symlink("../../outside", root.path().join("escape")).unwrap();
    let outcome = observer
        .reconcile_at(ReconcileSource::Event, 4_710)
        .unwrap();
    assert!(matches!(
        outcome,
        worksgood::worktree_observer::ReconcileOutcome::Held(_)
    ));
    assert_eq!(observer.projection().content_seq, 0);
    assert!(observer.projection().quarantine_required);
}

#[test]
fn gradual_writes_defer_only_inside_one_proof_window() {
    let mut last_observed = None;
    for elapsed in (30..=420).step_by(30) {
        last_observed = Some(20_000 + elapsed);
        let deadline = calculate_suspect_deadline(DeadlineInput {
            last_proven_at: 20_000,
            last_proven_seq: 9,
            last_observed_at: last_observed,
            observed_after_proven_seq: Some(9),
            meaningful_silence_secs: 300,
            observed_activity_grace_secs: 120,
            max_observed_only_extension_secs: 600,
        });
        if elapsed == 300 {
            assert!(
                deadline.suspect_at > 20_300,
                "gradual real source writing avoids premature 300s suspicion"
            );
        }
    }
    let new_proof = calculate_suspect_deadline(DeadlineInput {
        last_proven_at: 20_500,
        last_proven_seq: 10,
        last_observed_at: last_observed,
        observed_after_proven_seq: Some(9),
        meaningful_silence_secs: 300,
        observed_activity_grace_secs: 120,
        max_observed_only_extension_secs: 600,
    });
    assert_eq!(
        new_proof.suspect_at, 20_800,
        "old observed evidence cannot spill into a new proven-progress window"
    );
}

#[test]
fn observation_has_no_completion_or_interaction_authority() {
    let root = repo();
    let storage = tempdir().unwrap();
    let mut task = Task {
        id: "task-a".into(),
        status: Status::InProgress,
        ..Task::default()
    };
    task.last_interaction_at = Some("2026-01-01T00:00:00Z".into());
    let before = task.clone();
    let mut observer = WorktreeObserver::attach_at(
        root.path(),
        storage.path(),
        identity(),
        CandidatePathPolicy::new(vec![], vec![]).unwrap(),
        ObserverConfig::default(),
        5_000,
    )
    .unwrap();
    fs::write(root.path().join("complete-looking-output.txt"), "DONE\n").unwrap();
    observer
        .reconcile_at(ReconcileSource::Event, 5_010)
        .unwrap();
    assert_eq!(task.status, before.status);
    assert_eq!(task.last_interaction_at, before.last_interaction_at);
    assert_eq!(task.lifecycle, before.lifecycle);
}
