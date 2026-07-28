use std::fs;
use std::process::Command;

use tempfile::tempdir;
use worksgood::finalization::{
    CandidateBinding, FinalizationContext, FinalizationPhase, FinalizationStore, QuiescenceProof,
    checkpoint_candidate, merge_candidate,
};

fn git(root: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    root: std::path::PathBuf,
    wg: std::path::PathBuf,
    wt: std::path::PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("repo");
    fs::create_dir_all(root.join("incident")).unwrap();
    git(&root, &["init", "-b", "main"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "user.name", "Test"]);
    fs::write(root.join("incident/payload.txt"), vec![b'm'; 6_144]).unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "base"]);
    let wg = root.join(".wg");
    fs::create_dir_all(&wg).unwrap();
    let wt = tmp.path().join("worker");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "wg/agent-1/incident",
            wt.to_str().unwrap(),
        ],
    );
    Fixture {
        _tmp: tmp,
        root,
        wg,
        wt,
    }
}

fn context(f: &Fixture) -> FinalizationContext {
    FinalizationContext {
        task_id: "incident".into(),
        generation: 0,
        attempt_id: "attempt-0-1".into(),
        attempt_fence: 1,
        process_epoch: 1,
        worktree_id: "agent-1".into(),
        worktree_lease_epoch: 1,
        worktree_path: f.wt.clone(),
        project_root: f.root.clone(),
        terminal_reservation_id: "terminal-1".into(),
        evaluation_policy: "required".into(),
        route_snapshot_cid: "route:test".into(),
        quiescence: QuiescenceProof {
            receipt_cid: "receipt:test".into(),
            process_identity_digest: "pid:42:start:7:nonce:n".into(),
            process_group_empty: true,
            nonce_pipe_eof: true,
            observed_manifest_digest: None,
        },
    }
}

#[test]
fn candidate_binds_28kb_bytes_not_6kb_main_and_is_immutable() {
    let f = fixture();
    fs::write(f.wt.join("incident/payload.txt"), vec![b'c'; 28_672]).unwrap();
    fs::write(f.wt.join("untracked.txt"), b"preserve me").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&f.wg, f.wt.join(".wg")).unwrap();
    let store = FinalizationStore::open(&f.wg).unwrap();
    let tx = checkpoint_candidate(&store, &context(&f)).unwrap();
    assert_eq!(tx.phase, FinalizationPhase::CandidateCheckpointed);
    let candidate = tx.candidate.as_ref().unwrap();
    let reloaded = store.read_candidate(&candidate.candidate_id).unwrap();
    assert_eq!(reloaded.binding, candidate.binding);
    assert_eq!(candidate.binding, tx.validation.as_ref().unwrap().binding);
    assert_eq!(
        candidate.binding,
        CandidateBinding {
            candidate_id: candidate.candidate_id.clone(),
            commit_oid: candidate.candidate_commit_oid.clone(),
            tree_oid: candidate.candidate_tree_oid.clone(),
            manifest_cid: candidate.content_manifest_cid.clone(),
            delta_manifest_cid: candidate.delta_manifest_cid.clone(),
        }
    );
    let blob = git(
        &f.root,
        &[
            "show",
            &format!("{}:incident/payload.txt", candidate.candidate_commit_oid),
        ],
    );
    assert_eq!(blob.len(), 28_672);
    #[cfg(unix)]
    assert!(
        !Command::new("git")
            .args([
                "cat-file",
                "-e",
                &format!("{}:.wg", candidate.candidate_commit_oid)
            ])
            .current_dir(&f.root)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        fs::metadata(f.root.join("incident/payload.txt"))
            .unwrap()
            .len(),
        6_144
    );

    fs::write(f.wt.join("incident/payload.txt"), vec![b'x'; 9_000]).unwrap();
    let immutable = git(
        &f.root,
        &[
            "show",
            &format!("{}:incident/payload.txt", candidate.candidate_commit_oid),
        ],
    );
    assert_eq!(immutable.len(), 28_672);
}

#[test]
fn required_deep_flip_holds_candidate_across_reconcile_until_acceptance() {
    let f = fixture();
    let base = git(&f.root, &["rev-parse", "refs/heads/main"]);
    fs::write(f.wt.join("incident/payload.txt"), vec![b'c'; 28_672]).unwrap();
    let store = FinalizationStore::open(&f.wg).unwrap();
    let mut ctx = context(&f);
    ctx.evaluation_policy = "required-deep-readonly-flip-before-merge".into();
    let tx = checkpoint_candidate(&store, &ctx).unwrap();
    assert_eq!(tx.phase, FinalizationPhase::Evaluating);
    assert!(tx.replay_action.is_none());
    assert!(tx.merge_receipt.is_none());
    let replay = worksgood::finalization::reconcile(&store, "incident")
        .unwrap()
        .unwrap();
    assert_eq!(replay.phase, FinalizationPhase::Evaluating);
    assert!(replay.merge_receipt.is_none());
    let rejected = worksgood::finalization::retain_rejected_candidate(
        &store,
        &tx.candidate.as_ref().unwrap().candidate_id,
        "deep-report-semantic-reject",
    )
    .unwrap();
    assert_eq!(rejected.phase, FinalizationPhase::RepairNeeded);
    assert_eq!(
        rejected.retained_reason.as_deref(),
        Some("acceptance.rejected:deep-report-semantic-reject")
    );
    let replay = worksgood::finalization::retain_rejected_candidate(
        &store,
        &tx.candidate.as_ref().unwrap().candidate_id,
        "deep-report-semantic-reject",
    )
    .unwrap();
    assert_eq!(replay, rejected, "rejection projection is idempotent");
    assert_eq!(git(&f.root, &["rev-parse", "refs/heads/main"]), base);
}

#[test]
fn merge_is_content_bound_exactly_once_and_conflict_is_retained() {
    let f = fixture();
    fs::write(f.wt.join("incident/payload.txt"), vec![b'c'; 28_672]).unwrap();
    let store = FinalizationStore::open(&f.wg).unwrap();
    let tx = checkpoint_candidate(&store, &context(&f)).unwrap();
    let first = merge_candidate(&store, &tx.candidate.unwrap()).unwrap();
    let second = merge_candidate(&store, &first.candidate.clone().unwrap()).unwrap();
    assert_eq!(first.merge_receipt, second.merge_receipt);
    assert_eq!(first.phase, FinalizationPhase::Merged);
    assert_eq!(
        fs::metadata(f.root.join("incident/payload.txt"))
            .unwrap()
            .len(),
        28_672
    );

    let f = fixture();
    fs::write(f.wt.join("incident/payload.txt"), vec![b'c'; 28_672]).unwrap();
    let store = FinalizationStore::open(&f.wg).unwrap();
    let tx = checkpoint_candidate(&store, &context(&f)).unwrap();
    fs::write(f.root.join("incident/payload.txt"), vec![b'z'; 6_144]).unwrap();
    git(&f.root, &["add", "."]);
    git(&f.root, &["commit", "-m", "main moved"]);
    let held = merge_candidate(&store, &tx.candidate.unwrap()).unwrap();
    assert_eq!(held.phase, FinalizationPhase::RepairNeeded);
    assert!(held.merge_conflict.is_some());
    assert_eq!(
        git(&f.root, &["show", "HEAD:incident/payload.txt"]).len(),
        6_144
    );
}

#[test]
fn failure_rescue_preserves_dirty_tracked_untracked_and_deleted() {
    let f = fixture();
    fs::write(f.wt.join("useful.txt"), b"wip").unwrap();
    fs::remove_file(f.wt.join("incident/payload.txt")).unwrap();
    let store = FinalizationStore::open(&f.wg).unwrap();
    let mut ctx = context(&f);
    ctx.terminal_reservation_id = "failure-1".into();
    let tx = worksgood::finalization::checkpoint_rescue(&store, &ctx, false).unwrap();
    assert_eq!(tx.phase, FinalizationPhase::FailedPreserved);
    let rescue = tx.rescue.unwrap();
    assert_eq!(
        git(
            &f.root,
            &["show", &format!("{}:useful.txt", rescue.rescue_commit_oid)]
        ),
        "wip"
    );
    let missing = Command::new("git")
        .args([
            "cat-file",
            "-e",
            &format!("{}:incident/payload.txt", rescue.rescue_commit_oid),
        ])
        .current_dir(&f.root)
        .status()
        .unwrap();
    assert!(!missing.success());
}

#[test]
fn restart_after_target_cas_reconstructs_one_receipt_and_repair_is_v2() {
    let f = fixture();
    fs::write(f.wt.join("incident/payload.txt"), vec![b'c'; 28_672]).unwrap();
    let store = FinalizationStore::open(&f.wg).unwrap();
    let tx = checkpoint_candidate(&store, &context(&f)).unwrap();
    assert_eq!(
        tx.evaluation_request.as_ref().unwrap().binding,
        tx.candidate.as_ref().unwrap().binding
    );
    let merged = merge_candidate(&store, tx.candidate.as_ref().unwrap()).unwrap();
    let first_receipt = merged.merge_receipt.as_ref().unwrap().receipt_id.clone();
    let integration = merged
        .merge_receipt
        .as_ref()
        .unwrap()
        .integration_commit_oid
        .clone();

    // Crash projection: target CAS + immutable result ref happened, but receipt
    // was not linked into the mutable read projection. Startup replay must
    // reconstruct the same receipt rather than merge/charge again.
    let mut crashed = merged.clone();
    crashed.phase = FinalizationPhase::MergePending;
    crashed.merge_receipt = None;
    crashed.replay_action = Some("merge-replay".into());
    fs::write(
        store.root().join("transactions/incident.json"),
        serde_json::to_vec_pretty(&crashed).unwrap(),
    )
    .unwrap();
    let replayed = worksgood::finalization::reconcile(&store, "incident")
        .unwrap()
        .unwrap();
    assert_eq!(replayed.phase, FinalizationPhase::Merged);
    assert_eq!(
        replayed.merge_receipt.as_ref().unwrap().receipt_id,
        first_receipt
    );
    assert_eq!(git(&f.root, &["rev-parse", "main"]), integration);

    // A lifecycle-authorized repair source gets a new immutable version; v1
    // remains readable and can never be retagged by later mutable bytes.
    fs::write(f.wt.join("incident/payload.txt"), vec![b'r'; 30_000]).unwrap();
    let mut repair = context(&f);
    repair.generation = 1;
    repair.attempt_id = "attempt-1-2".into();
    repair.attempt_fence = 2;
    repair.worktree_lease_epoch = 2;
    repair.terminal_reservation_id = "terminal-repair".into();
    repair.quiescence.receipt_cid = "receipt:repair".into();
    let v2 = checkpoint_candidate(&store, &repair).unwrap();
    assert_eq!(v2.candidate.as_ref().unwrap().candidate_version, 2);
    assert_ne!(
        v2.candidate.as_ref().unwrap().candidate_id,
        merged.candidate.as_ref().unwrap().candidate_id
    );
    assert_eq!(
        store
            .read_candidate(&merged.candidate.as_ref().unwrap().candidate_id)
            .unwrap()
            .candidate_version,
        1
    );
}

#[test]
fn quiescence_and_lease_are_fail_closed() {
    let f = fixture();
    fs::write(f.wt.join("late.txt"), b"late").unwrap();
    let store = FinalizationStore::open(&f.wg).unwrap();
    let mut ctx = context(&f);
    ctx.quiescence.process_group_empty = false;
    let error = checkpoint_candidate(&store, &ctx).unwrap_err().to_string();
    assert!(error.contains("finalize.quiescence_unproven"));
    assert!(store.load_task("incident").unwrap().is_none());
}
