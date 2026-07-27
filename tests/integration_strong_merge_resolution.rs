use std::fs;
use std::process::Command;
use tempfile::tempdir;
use worksgood::finalization::{
    FinalizationContext, FinalizationStore, QuiescenceProof, checkpoint_candidate,
};
use worksgood::merge_resolution::{MergeClassification, ResolutionState, RunOptions, run_task};

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
    String::from_utf8_lossy(&out.stdout).trim().into()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    root: std::path::PathBuf,
    wg: std::path::PathBuf,
    wt: std::path::PathBuf,
    adapter: std::path::PathBuf,
    counter: std::path::PathBuf,
}
fn fixture() -> Fixture {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-b", "main"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    fs::write(root.join("value.txt"), "base\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "base"]);
    let wg = root.join(".wg");
    fs::create_dir_all(&wg).unwrap();
    fs::write(wg.join("config.toml"),"[models.merger]\nmodel = \"pi:fake:strong-coder\"\ntier = \"premium\"\nreasoning = \"high\"\n").unwrap();
    let wt = tmp.path().join("worker");
    git(
        &root,
        &["worktree", "add", "-b", "candidate", wt.to_str().unwrap()],
    );
    let adapter = tmp.path().join("fake-strong-merger");
    let counter = tmp.path().join("calls");
    fs::write(&adapter,format!(r#"#!/bin/sh
set -eu
workspace= outcome= route= reasoning= bundle=
while [ "$#" -gt 0 ]; do
 case "$1" in
 --workspace) workspace=$2;; --outcome) outcome=$2;; --route) route=$2;; --reasoning) reasoning=$2;; --bundle-cid) bundle=$2;;
 esac
 shift 2
done
[ "$route" = "pi:fake:strong-coder" ]
[ "$reasoning" = "high" ]
case "$bundle" in wgcid:v1:blake3:*) ;; *) exit 31;; esac
[ ! -e "$workspace/.wg/graph.jsonl" ]
printf x >> '{}'
printf 'resolved\n' > "$workspace/value.txt"
rm -f "$workspace/candidate.flag" "$workspace/target.flag"
printf '{{"outcome":"resolved","explanation":"combined both changes","generator_commands":[]}}\n' > "$outcome"
"#,counter.display())).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&adapter, fs::Permissions::from_mode(0o755)).unwrap();
    }
    Fixture {
        _tmp: tmp,
        root,
        wg,
        wt,
        adapter,
        counter,
    }
}
fn checkpoint(f: &Fixture) {
    fs::write(f.wt.join("value.txt"), "candidate\n").unwrap();
    let ctx = FinalizationContext {
        task_id: "merge-me".into(),
        generation: 0,
        attempt_id: "attempt-1".into(),
        attempt_fence: 1,
        process_epoch: 1,
        worktree_id: "agent-1".into(),
        worktree_lease_epoch: 1,
        worktree_path: f.wt.clone(),
        project_root: f.root.clone(),
        terminal_reservation_id: "terminal-1".into(),
        evaluation_policy: "required".into(),
        route_snapshot_cid: "route:source".into(),
        quiescence: QuiescenceProof {
            receipt_cid: "q".into(),
            process_identity_digest: "pid:1:start:1".into(),
            process_group_empty: true,
            nonce_pipe_eof: true,
            observed_manifest_digest: None,
        },
    };
    checkpoint_candidate(&FinalizationStore::open(&f.wg).unwrap(), &ctx).unwrap();
}

#[test]
fn textual_conflict_invokes_exact_strong_route_once_and_merges_bound_tree() {
    let f = fixture();
    checkpoint(&f);
    fs::write(f.root.join("value.txt"), "target\n").unwrap();
    git(&f.root, &["add", "."]);
    git(&f.root, &["commit", "-m", "target moved"]);
    let source_ref_name = FinalizationStore::open(&f.wg)
        .unwrap()
        .load_task("merge-me")
        .unwrap()
        .unwrap()
        .candidate
        .unwrap()
        .immutable_ref;
    let source_ref = git(&f.root, &["rev-parse", &source_ref_name]);
    let first = run_task(
        &f.wg,
        "merge-me",
        RunOptions {
            adapter: &f.adapter,
            integration_check: Some("test $(cat value.txt) = resolved"),
            generated: false,
            generated_owned: false,
            ambiguous_intent: false,
        },
    )
    .unwrap();
    assert_eq!(first.state, ResolutionState::Merged);
    assert_eq!(first.runner_invocations, 1);
    assert_eq!(fs::read_to_string(&f.counter).unwrap(), "x");
    assert_eq!(git(&f.root, &["show", "main:value.txt"]), "resolved");
    assert_eq!(
        first.descriptor.as_ref().unwrap().resolution_tree_oid,
        first.merge_receipt.as_ref().unwrap().result_tree_oid
    );
    assert_eq!(git(&f.root, &["rev-parse", &source_ref_name]), source_ref);
    let second = run_task(
        &f.wg,
        "merge-me",
        RunOptions {
            adapter: &f.adapter,
            integration_check: Some("false"),
            generated: false,
            generated_owned: false,
            ambiguous_intent: false,
        },
    )
    .unwrap();
    assert_eq!(second.merge_receipt, first.merge_receipt);
    assert_eq!(fs::read_to_string(&f.counter).unwrap(), "x");
}

#[test]
fn clean_semantic_failure_invokes_one_strong_resolution() {
    let f = fixture();
    fs::write(f.wt.join("candidate.flag"), "candidate\n").unwrap();
    checkpoint(&f);
    fs::write(f.root.join("target.flag"), "target\n").unwrap();
    git(&f.root, &["add", "."]);
    git(&f.root, &["commit", "-m", "independent target"]);
    let record = run_task(
        &f.wg,
        "merge-me",
        RunOptions {
            adapter: &f.adapter,
            integration_check: Some("test ! -f candidate.flag -o ! -f target.flag"),
            generated: false,
            generated_owned: false,
            ambiguous_intent: false,
        },
    )
    .unwrap();
    assert_eq!(
        record.classification.classification,
        MergeClassification::MergeResolutionRequired(
            worksgood::merge_resolution::ConflictKind::SemanticIntegration
        )
    );
    assert_eq!(record.state, ResolutionState::Merged);
    assert_eq!(record.runner_invocations, 1);
    assert_eq!(fs::read_to_string(&f.counter).unwrap(), "x");
}

#[test]
fn clean_and_human_rows_never_invoke_adapter() {
    let f = fixture();
    checkpoint(&f);
    let clean = run_task(
        &f.wg,
        "merge-me",
        RunOptions {
            adapter: &f.adapter,
            integration_check: None,
            generated: false,
            generated_owned: false,
            ambiguous_intent: false,
        },
    )
    .unwrap();
    assert_eq!(
        clean.classification.classification,
        MergeClassification::MechanicalMerge
    );
    assert_eq!(clean.runner_invocations, 0);
    assert!(!f.counter.exists());

    let f = fixture();
    checkpoint(&f);
    fs::write(f.root.join("value.txt"), "target\n").unwrap();
    git(&f.root, &["add", "."]);
    git(&f.root, &["commit", "-m", "target"]);
    let human = run_task(
        &f.wg,
        "merge-me",
        RunOptions {
            adapter: &f.adapter,
            integration_check: None,
            generated: true,
            generated_owned: false,
            ambiguous_intent: false,
        },
    )
    .unwrap();
    assert_eq!(human.state, ResolutionState::HumanDecisionRequired);
    assert_eq!(human.runner_invocations, 0);
    assert!(!f.counter.exists());
}
