use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
use worksgood::completion_manifest::{GitOutput, OutputRef, ReviewResolver};
use worksgood::completion_task::{
    load_exact_review_pair, load_review_evidence, load_submission_bytes,
};
use worksgood::config::Config;
use worksgood::graph::{
    CompletionBlocker, CompletionBlockerKind, CompletionContract, CompletionDisposition, LogEntry,
    Status,
};
use worksgood::identity::canonical_json;
use worksgood::parser::{load_graph, modify_graph};

use super::completion_submit::{collect_dependency_outputs, require_source_owner, store};

#[derive(Clone, Debug, Serialize)]
struct LandingReceipt {
    receipt_version: u32,
    task_id: String,
    generation: u64,
    manifest_digest: String,
    integration_ref: String,
    integrated_main_oid: String,
    accepted_commit_oid: String,
    observed_main_before: String,
    observed_main_after: String,
    already_published: bool,
    root_checkout_synchronized: bool,
    created_at: String,
}

pub fn run(dir: &Path, id: &str, integration_ref: &str) -> Result<()> {
    let cwd = std::env::current_dir().context("determine worker working directory")?;
    run_at(dir, id, integration_ref, Some(&cwd))
}

pub fn run_at(
    dir: &Path,
    id: &str,
    integration_ref: &str,
    worker_worktree: Option<&Path>,
) -> Result<()> {
    run_at_inner(dir, id, integration_ref, worker_worktree, None).map(|_| ())
}

/// Resume only the landing phase from an exact typed completion wait. No
/// source execution or model review is repeated.
pub(crate) fn pending_checkout_is_clean(dir: &Path, id: &str) -> Result<bool> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph
        .get_task(id)
        .with_context(|| format!("task '{id}' not found"))?;
    let blocker = task
        .completion_blocker
        .as_ref()
        .context("task has no pending completion finalization")?;
    if blocker.kind != CompletionBlockerKind::LandingPending || task.status != Status::Waiting {
        return Ok(false);
    }
    super::completion_wait::validate_current(task, blocker)?;
    let integration_ref = blocker
        .integration_ref
        .as_deref()
        .context("LandingPending has no integration ref")?;
    root_checkout_dirty_if_attached(
        dir.parent()
            .context("workgraph directory has no project root")?,
        integration_ref,
    )
    .map(|dirty| !dirty)
}

pub(crate) fn resume_pending(dir: &Path, id: &str) -> Result<bool> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph
        .get_task(id)
        .with_context(|| format!("task '{id}' not found"))?;
    let blocker = task
        .completion_blocker
        .clone()
        .context("task has no pending completion finalization")?;
    if blocker.kind != CompletionBlockerKind::LandingPending || task.status != Status::Waiting {
        bail!("task '{id}' is not Waiting/LandingPending");
    }
    super::completion_wait::validate_current(task, &blocker)?;
    let integration_ref = blocker
        .integration_ref
        .as_deref()
        .context("LandingPending has no integration ref")?;
    let worker = blocker
        .worker_worktree
        .as_deref()
        .map(Path::new)
        .context("LandingPending has no retained worker worktree")?;
    run_at_inner(dir, id, integration_ref, Some(worker), Some(&blocker))
}

fn run_at_inner(
    dir: &Path,
    id: &str,
    integration_ref: &str,
    worker_worktree: Option<&Path>,
    pending: Option<&CompletionBlocker>,
) -> Result<bool> {
    validate_integration_ref(integration_ref)?;
    let graph_path = dir.join("graph.jsonl");
    let graph = load_graph(&graph_path)?;
    let task = graph
        .get_task(id)
        .with_context(|| format!("task '{id}' not found"))?;
    if let Some(blocker) = pending {
        super::completion_wait::validate_current(task, blocker)?;
    } else {
        require_source_owner(task, id)?;
    }
    if task.completion_contract != CompletionContract::Land {
        bail!(
            "wg land applies only to Land tasks; '{}' is {}",
            id,
            task.completion_contract
        );
    }
    let completion_store = store(dir)?;
    let (submission, manifest, requirements, summary) =
        load_submission_bytes(&completion_store, task)?;
    let current_dependencies = collect_dependency_outputs(&completion_store, &graph, task)?;
    let selected_dependencies = task
        .completion_candidate
        .as_ref()
        .context("missing completion candidate")?
        .dependency_outputs
        .clone();
    if current_dependencies != selected_dependencies {
        bail!("dependency outputs changed after review; submit a new manifest");
    }
    let project_root = dir
        .parent()
        .context("workgraph directory has no project root")?;
    let resolved = ReviewResolver::new(&completion_store)
        .repository(project_root)
        .resolve_submission(
            &submission.manifest_ref,
            &requirements,
            &summary,
            &current_dependencies,
        )
        .map_err(|error| anyhow::anyhow!("completion evidence no longer resolves: {error}"))?;
    let config = Config::load_merged(dir)?;
    if config.agency.completion_review_strict {
        load_exact_review_pair(&completion_store, &submission, &manifest, &resolved)?;
    } else {
        let evidence = load_review_evidence(&completion_store, &submission, &manifest, &resolved)?;
        if evidence.flip.verdict != worksgood::simple_land::ReviewVerdict::Pass
            || evidence.eval.as_ref().is_some_and(|receipt| {
                receipt.verdict != worksgood::simple_land::ReviewVerdict::Pass
            })
        {
            eprintln!(
                "Advisory model review did not pass; deterministic publication continues. Inspect `wg show {id}` for findings."
            );
        }
    }
    let git_output = exact_git_output(&manifest.outputs)?;
    worksgood::control_plane::assert_tree_has_no_control_plane(
        project_root,
        &git_output.commit_oid,
    )?;

    let _lock = LandingLock::acquire(project_root)?;
    let observed_before = git(project_root, &["rev-parse", integration_ref])?;
    if let Some(blocker) = pending {
        let expected = blocker
            .target_ref_oid
            .as_deref()
            .context("LandingPending has no target-ref CAS binding")?;
        if observed_before != expected && observed_before != git_output.commit_oid {
            bail!(
                "pending landing target ref moved: expected {} (or already-published candidate {}), found {}",
                expected,
                git_output.commit_oid,
                observed_before
            );
        }
    }
    let already_published = is_ancestor(project_root, &git_output.commit_oid, &observed_before)?;
    if !already_published && pending.is_none() {
        let worker = worker_worktree.context(
            "initial landing requires the retained worker worktree; crash recovery may omit it after publication",
        )?;
        verify_worker_worktree(worker, &git_output.commit_oid)?;
    }

    if root_checkout_dirty_if_attached(project_root, integration_ref)? {
        if pending.is_none() {
            let worker =
                worker_worktree.context("landing wait requires the retained worker worktree")?;
            super::completion_wait::park_landing_pending(
                dir,
                id,
                "attached integration checkout has tracked, index, or untracked changes; publication deferred without modifying user bytes",
                super::completion_wait::LandingWait {
                    integration_ref,
                    target_ref_oid: &observed_before,
                    worker_worktree: worker,
                },
            )?;
        }
        eprintln!(
            "Landing pending: attached integration checkout is dirty; user bytes were not modified"
        );
        return Ok(false);
    }

    let root_checkout_synchronized = if already_published {
        // If the integration ref already contains the candidate, a clean
        // attached checkout is already synchronized. A stale index/worktree
        // appears dirty above and is deferred rather than overwritten.
        symbolic_head(project_root).as_deref() == Some(integration_ref)
    } else {
        if observed_before != git_output.integrated_main_oid {
            bail!(
                "NeedsRebase: current integration ref is {}, reviewed candidate integrated {}; merge current main in the same worker, revalidate, and submit a new manifest",
                observed_before,
                git_output.integrated_main_oid
            );
        }
        if !is_ancestor(
            project_root,
            &git_output.integrated_main_oid,
            &git_output.commit_oid,
        )? {
            bail!("reviewed commit is not a fast-forward of integrated_main_oid");
        }
        if symbolic_head(project_root).as_deref() == Some(integration_ref) {
            // `merge --ff-only` is Git's checked worktree/index update: it
            // protects local tracked, staged, and obstructing untracked bytes.
            // Unlike reset --hard it refuses rather than overwriting a user
            // race. Its ref transaction is locked against the observed HEAD.
            if let Err(error) = git(
                project_root,
                &["merge", "--ff-only", "--no-edit", &git_output.commit_oid],
            ) {
                if root_checkout_dirty_if_attached(project_root, integration_ref)? {
                    if pending.is_none() {
                        let worker = worker_worktree
                            .context("landing wait requires the retained worker worktree")?;
                        super::completion_wait::park_landing_pending(
                            dir,
                            id,
                            "attached integration checkout changed during publication; Git refused the update and preserved user bytes",
                            super::completion_wait::LandingWait {
                                integration_ref,
                                target_ref_oid: &observed_before,
                                worker_worktree: worker,
                            },
                        )?;
                    }
                    eprintln!(
                        "Landing pending: attached integration checkout changed; user bytes were not modified"
                    );
                    return Ok(false);
                }
                return Err(error).context(
                    "atomic checked fast-forward failed; no destructive fallback was attempted",
                );
            }
            true
        } else {
            git(
                project_root,
                &[
                    "update-ref",
                    integration_ref,
                    &git_output.commit_oid,
                    &observed_before,
                ],
            )
            .context("atomic compare-and-fast-forward failed; no fallback was attempted")?;
            false
        }
    };

    let observed_after = git(project_root, &["rev-parse", integration_ref])?;
    if !is_ancestor(project_root, &git_output.commit_oid, &observed_after)? {
        bail!(
            "landing postcondition failed: accepted commit is not reachable from integration ref"
        );
    }
    let manifest_digest = manifest.digest().map_err(anyhow::Error::msg)?;
    let receipt = LandingReceipt {
        receipt_version: 1,
        task_id: id.to_string(),
        generation: task.lifecycle.generation,
        manifest_digest: manifest_digest.to_string(),
        integration_ref: integration_ref.to_string(),
        integrated_main_oid: git_output.integrated_main_oid.clone(),
        accepted_commit_oid: git_output.commit_oid.clone(),
        observed_main_before: observed_before,
        observed_main_after: observed_after,
        already_published,
        root_checkout_synchronized,
        created_at: Utc::now().to_rfc3339(),
    };
    let receipt_bytes = canonical_json(&serde_json::to_value(&receipt)?);
    let receipt_ref = completion_store.put_bytes(
        &receipt_bytes,
        "application/vnd.worksgood.landing-receipt+json",
    )?;
    record_landing(
        &graph_path,
        id,
        task.lifecycle.generation,
        task.lifecycle
            .current_attempt
            .as_ref()
            .map(|attempt| attempt.id.as_str()),
        task.lifecycle.fence,
        &manifest_digest,
        &receipt_ref.content_digest.to_string(),
    )?;

    if !root_checkout_synchronized {
        eprintln!(
            "WARNING: integration ref contains the reviewed commit, but this invocation did not synchronize a root checkout"
        );
    }
    println!(
        "Landed '{}' at {}{}",
        id,
        git_output.commit_oid,
        if already_published {
            " (already published)"
        } else {
            ""
        }
    );
    Ok(true)
}

fn exact_git_output(outputs: &[OutputRef]) -> Result<&GitOutput> {
    let mut git_outputs = outputs.iter().filter_map(|output| match output {
        OutputRef::Git(git) => Some(git),
        _ => None,
    });
    let output = git_outputs
        .next()
        .context("Land manifest has no Git output")?;
    if git_outputs.next().is_some() {
        bail!("Land manifest has more than one Git output");
    }
    Ok(output)
}

fn validate_integration_ref(reference: &str) -> Result<()> {
    if !reference.starts_with("refs/heads/") || reference.contains("..") {
        bail!("integration ref must be an explicit refs/heads/* reference");
    }
    Ok(())
}

fn verify_worker_worktree(worktree: &Path, accepted_commit: &str) -> Result<()> {
    let head = git(worktree, &["rev-parse", "HEAD"])?;
    if head != accepted_commit {
        bail!(
            "worker worktree HEAD {} is not the exact reviewed commit {}",
            head,
            accepted_commit
        );
    }
    let status = git(
        worktree,
        &["status", "--porcelain", "--untracked-files=all"],
    )?;
    if !status.is_empty() {
        bail!("worker worktree is not clean at the reviewed commit");
    }
    Ok(())
}

fn root_checkout_dirty_if_attached(project: &Path, integration_ref: &str) -> Result<bool> {
    if symbolic_head(project).as_deref() != Some(integration_ref) {
        return Ok(false);
    }
    Ok(!git(project, &["status", "--porcelain", "--untracked-files=all"])?.is_empty())
}

fn symbolic_head(project: &Path) -> Option<String> {
    git(project, &["symbolic-ref", "-q", "HEAD"]).ok()
}

fn is_ancestor(project: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let status = Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(project)
        .status()?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!("git merge-base --is-ancestor failed"),
    }
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").args(args).current_dir(root).output()?;
    if !output.status.success() {
        bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn record_landing(
    graph_path: &Path,
    id: &str,
    generation: u64,
    attempt_id: Option<&str>,
    fence: u64,
    manifest_digest: &worksgood::completion_manifest::ContentDigest,
    receipt_digest: &str,
) -> Result<()> {
    let mut refusal = None;
    modify_graph(graph_path, |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            refusal = Some("task disappeared after Git publication".to_string());
            return false;
        };
        if task.lifecycle.generation != generation
            || task.lifecycle.fence != fence
            || task
                .lifecycle
                .current_attempt
                .as_ref()
                .map(|attempt| attempt.id.as_str())
                != attempt_id
            || task
                .completion_candidate
                .as_ref()
                .map(|candidate| &candidate.manifest.content_digest)
                != Some(manifest_digest)
        {
            refusal = Some(
                "task generation, attempt, fence, or candidate changed after Git publication; accepted commit remains recoverable by ancestry"
                    .to_string(),
            );
            return false;
        }
        task.completion_disposition = Some(CompletionDisposition::Landed);
        task.completion_receipt = Some(receipt_digest.to_string());
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some("land".to_string()),
            user: None,
            message: format!("Reviewed manifest {manifest_digest} published to integration ref"),
        });
        true
    })?;
    if let Some(refusal) = refusal {
        bail!(refusal);
    }
    Ok(())
}

struct LandingLock {
    file: File,
}

impl LandingLock {
    fn acquire(project: &Path) -> Result<Self> {
        let common = git(project, &["rev-parse", "--git-common-dir"])?;
        let common = PathBuf::from(common);
        let common = if common.is_absolute() {
            common
        } else {
            project.join(common)
        };
        fs::create_dir_all(&common)?;
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(common.join("wg-land.lock"))?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let fd = file.as_raw_fd();
            worksgood::lock::retry_acquire(
                &worksgood::lock::RetryPolicy::default(),
                worksgood::lock::is_transient_blocking,
                || {
                    let result = unsafe { libc::flock(fd, libc::LOCK_EX) };
                    if result == 0 {
                        Ok(())
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                },
            )?;
        }
        Ok(Self { file })
    }
}

impl Drop for LandingLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;
    use worksgood::completion_manifest::{
        COMPLETION_MANIFEST_VERSION, CompletionManifest, ContentDigest, GitOutput, OutputRef,
    };
    use worksgood::completion_review::{
        ManifestReviewer, ReviewFinding, ReviewerKind, ReviewerUnavailable, SemanticReview,
        SemanticVerdict,
    };
    use worksgood::graph::{Node, Status, Task, WorkGraph};
    use worksgood::parser::save_graph;

    struct PassReviewer {
        route: &'static str,
        calls: Arc<Mutex<Vec<ReviewerKind>>>,
    }

    struct RejectReviewer {
        route: &'static str,
        calls: Arc<Mutex<Vec<ReviewerKind>>>,
    }

    impl ManifestReviewer for PassReviewer {
        fn route(&self) -> &str {
            self.route
        }
        fn review(
            &mut self,
            kind: ReviewerKind,
            _bundle: &worksgood::completion_manifest::ResolvedReviewBundle,
        ) -> std::result::Result<SemanticReview, ReviewerUnavailable> {
            self.calls.lock().unwrap().push(kind);
            Ok(SemanticReview {
                verdict: SemanticVerdict::Pass,
                findings: Vec::<ReviewFinding>::new(),
            })
        }
    }

    impl ManifestReviewer for RejectReviewer {
        fn route(&self) -> &str {
            self.route
        }
        fn review(
            &mut self,
            kind: ReviewerKind,
            _bundle: &worksgood::completion_manifest::ResolvedReviewBundle,
        ) -> std::result::Result<SemanticReview, ReviewerUnavailable> {
            self.calls.lock().unwrap().push(kind);
            Ok(SemanticReview {
                verdict: SemanticVerdict::Reject,
                findings: vec![ReviewFinding::new(
                    "advisory.fixture",
                    "bounded actionable finding",
                )],
            })
        }
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        dir: PathBuf,
        worker: PathBuf,
        candidate: String,
        integrated: String,
        manifest_path: PathBuf,
        summary_path: PathBuf,
        review_calls: Arc<Mutex<Vec<ReviewerKind>>>,
        task_id: String,
    }

    fn command(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn fixture() -> Fixture {
        let temp = tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        command(&root, &["init", "-b", "main"]);
        command(&root, &["config", "user.email", "test@example.com"]);
        command(&root, &["config", "user.name", "Test"]);
        fs::write(root.join(".gitignore"), ".wg\n").unwrap();
        fs::write(root.join("base.txt"), "base\n").unwrap();
        command(&root, &["add", ".gitignore", "base.txt"]);
        command(&root, &["commit", "-m", "base"]);
        let integrated = command(&root, &["rev-parse", "HEAD"]);
        let worker = temp.path().join("worker");
        command(
            &root,
            &[
                "worktree",
                "add",
                "-b",
                "wg/test-land",
                worker.to_str().unwrap(),
                "main",
            ],
        );
        fs::write(worker.join("result.txt"), "accepted\n").unwrap();
        command(&worker, &["add", "result.txt"]);
        command(&worker, &["commit", "-m", "candidate"]);
        let candidate = command(&worker, &["rev-parse", "HEAD"]);
        let tree = command(&worker, &["rev-parse", "HEAD^{tree}"]);
        let diff = Command::new("git")
            .args([
                "diff",
                "--binary",
                "--full-index",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                &integrated,
                &candidate,
                "--",
            ])
            .current_dir(&root)
            .output()
            .unwrap()
            .stdout;

        let dir = root.join(".wg");
        fs::create_dir_all(&dir).unwrap();
        let task_id = std::env::var("WG_TASK_ID")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "land-task".to_string());
        let mut task = Task {
            id: task_id.clone(),
            title: "Land exact candidate".to_string(),
            description: Some("Land result.\n\n## Validation\nInspect diff.".to_string()),
            status: Status::InProgress,
            completion_contract: CompletionContract::Land,
            ..Task::default()
        };
        let source_agent = std::env::var("WG_AGENT_ID")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "agent-land".to_string());
        task.assigned = Some(source_agent.clone());
        task.lifecycle.generation = 2;
        task.lifecycle.fence = 9;
        task.lifecycle.current_attempt = Some(worksgood::lifecycle::AttemptRef {
            id: "attempt-2-1".to_string(),
            generation: 2,
            fence: 9,
            actor_id: source_agent,
            disposition: None,
        });
        let requirements = worksgood::completion_task::requirements_digest(&task).unwrap();
        let summary = b"candidate complete\n";
        let evidence = store(&dir)
            .unwrap()
            .evidence_from_bytes(b"tests pass\n", "validation", "text/plain")
            .unwrap();
        let manifest = CompletionManifest {
            manifest_version: COMPLETION_MANIFEST_VERSION,
            task_id: task.id.clone(),
            generation: task.lifecycle.generation,
            completion_contract: worksgood::simple_land::CompletionContract::Land,
            requirements_digest: requirements,
            source_revision: "worker:test".to_string(),
            outputs: vec![OutputRef::Git(GitOutput {
                commit_oid: candidate.clone(),
                integrated_main_oid: integrated.clone(),
                tree_oid: tree,
                diff_bundle_digest: ContentDigest::of_bytes(&diff),
            })],
            validation_evidence: vec![evidence],
            worker_summary_digest: ContentDigest::of_bytes(summary),
        };
        let manifest_path = temp.path().join("manifest.json");
        fs::write(&manifest_path, manifest.canonical_bytes().unwrap()).unwrap();
        let summary_path = temp.path().join("summary.txt");
        fs::write(&summary_path, summary).unwrap();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task));
        save_graph(&graph, dir.join("graph.jsonl")).unwrap();

        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut flip = PassReviewer {
            route: "pi:test-flip",
            calls: calls.clone(),
        };
        let mut eval = PassReviewer {
            route: "codex:test-eval",
            calls: calls.clone(),
        };
        super::super::completion_submit::run_with_reviewers(
            &dir,
            &task_id,
            &manifest_path,
            &summary_path,
            &mut flip,
            &mut eval,
        )
        .unwrap();

        Fixture {
            _temp: temp,
            root,
            dir,
            worker,
            candidate,
            integrated,
            manifest_path,
            summary_path,
            review_calls: calls,
            task_id,
        }
    }

    #[test]
    fn land_compare_and_fast_forwards_reviewed_commit() {
        let fixture = fixture();
        run_at(
            &fixture.dir,
            &fixture.task_id,
            "refs/heads/main",
            Some(&fixture.worker),
        )
        .unwrap();
        assert_eq!(
            command(&fixture.root, &["rev-parse", "main"]),
            fixture.candidate
        );
        assert_eq!(
            command(&fixture.root, &["rev-parse", "HEAD"]),
            fixture.candidate
        );
        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task(&fixture.task_id).unwrap();
        assert_eq!(
            task.completion_disposition,
            Some(CompletionDisposition::Landed)
        );
        assert!(task.completion_receipt.is_some());
        assert_eq!(task.status, Status::InProgress);

        // Simulate a crash after the Git CAS but before a durable landing
        // projection. Done is recovered from ancestry plus exact reviews.
        modify_graph(fixture.dir.join("graph.jsonl"), |graph| {
            let task = graph.get_task_mut(&fixture.task_id).unwrap();
            task.completion_disposition = None;
            task.completion_receipt = None;
            true
        })
        .unwrap();
        super::super::completion_done::run(&fixture.dir, &fixture.task_id, "refs/heads/main")
            .unwrap();
        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        assert_eq!(
            graph.get_task(&fixture.task_id).unwrap().status,
            Status::Done
        );
    }

    #[test]
    fn advisory_flip_rejection_survives_landing_and_done() {
        let fixture = fixture();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut flip = RejectReviewer {
            route: "pi:test-advisory-flip",
            calls: calls.clone(),
        };
        let mut eval = PassReviewer {
            route: "pi:test-eval",
            calls: calls.clone(),
        };
        let outcome = super::super::completion_submit::run_with_reviewers(
            &fixture.dir,
            &fixture.task_id,
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut flip,
            &mut eval,
        )
        .unwrap();
        assert_eq!(
            outcome.status,
            worksgood::completion_review::ReviewValveStatus::FlipRejected
        );
        assert_eq!(*calls.lock().unwrap(), vec![ReviewerKind::Flip]);

        run_at(
            &fixture.dir,
            &fixture.task_id,
            "refs/heads/main",
            Some(&fixture.worker),
        )
        .unwrap();
        super::super::completion_done::run(&fixture.dir, &fixture.task_id, "refs/heads/main")
            .unwrap();

        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task(&fixture.task_id).unwrap();
        assert_eq!(task.status, Status::Done);
        assert_eq!(task.completion_review_activity.len(), 3);
        let verified = worksgood::completion_review::verified_review_activities(&fixture.dir, task);
        assert_eq!(verified.invalid_count, 0);
        assert_eq!(
            verified.activities.last().unwrap().candidate_state,
            worksgood::completion_review::ReviewCandidateState::Current
        );
        assert_eq!(
            verified.activities.last().unwrap().failure_class,
            Some(worksgood::completion_review::ReviewFailureClass::SemanticRejection)
        );
        assert_eq!(
            verified.activities.last().unwrap().findings[0].code,
            "advisory.fixture"
        );
    }

    #[test]
    fn completion_blockers_dirty_attached_main_preserves_bytes_and_resumes_once() {
        let fixture = fixture();
        fs::write(fixture.root.join("base.txt"), "user staged bytes\n").unwrap();
        command(&fixture.root, &["add", "base.txt"]);
        fs::write(fixture.root.join("base.txt"), "user worktree bytes\n").unwrap();
        fs::write(
            fixture.root.join("user-untracked.txt"),
            "user untracked bytes\n",
        )
        .unwrap();
        let index_before = command(&fixture.root, &["show", ":base.txt"]);
        let worktree_before = fs::read(fixture.root.join("base.txt")).unwrap();
        let untracked_before = fs::read(fixture.root.join("user-untracked.txt")).unwrap();
        let status_before = command(
            &fixture.root,
            &["status", "--porcelain", "--untracked-files=all"],
        );
        let main_before = command(&fixture.root, &["rev-parse", "main"]);
        let candidate_before = load_graph(fixture.dir.join("graph.jsonl"))
            .unwrap()
            .get_task(&fixture.task_id)
            .unwrap()
            .completion_candidate
            .clone()
            .unwrap();

        run_at(
            &fixture.dir,
            &fixture.task_id,
            "refs/heads/main",
            Some(&fixture.worker),
        )
        .unwrap();

        assert_eq!(command(&fixture.root, &["rev-parse", "main"]), main_before);
        assert_eq!(command(&fixture.root, &["show", ":base.txt"]), index_before);
        assert_eq!(
            fs::read(fixture.root.join("base.txt")).unwrap(),
            worktree_before
        );
        assert_eq!(
            fs::read(fixture.root.join("user-untracked.txt")).unwrap(),
            untracked_before
        );
        assert_eq!(
            command(
                &fixture.root,
                &["status", "--porcelain", "--untracked-files=all"],
            ),
            status_before
        );
        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task(&fixture.task_id).unwrap();
        assert_eq!(task.status, Status::Waiting);
        assert!(task.assigned.is_none());
        assert!(task.failure_reason.is_none());
        assert_eq!(task.completion_candidate.as_ref(), Some(&candidate_before));
        assert_eq!(
            task.lifecycle.current_attempt.as_ref().unwrap().disposition,
            Some(worksgood::lifecycle::AttemptDisposition::Parked)
        );
        let blocker = task.completion_blocker.clone().unwrap();
        assert_eq!(blocker.kind, CompletionBlockerKind::LandingPending);
        assert_eq!(blocker.generation, 2);
        assert_eq!(blocker.attempt_id.as_deref(), Some("attempt-2-1"));
        assert_eq!(blocker.fence, 9);
        assert_eq!(
            blocker.target_ref_oid.as_deref(),
            Some(main_before.as_str())
        );
        assert_eq!(blocker.candidate, candidate_before);
        let blocker_bytes = serde_json::to_vec(&blocker).unwrap();
        drop(graph);

        // A restart is a pure reload: exact candidate/fence/review receipts are
        // unchanged and no live source/session process is needed.
        let reopened = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        assert_eq!(
            serde_json::to_vec(
                reopened
                    .get_task(&fixture.task_id)
                    .unwrap()
                    .completion_blocker
                    .as_ref()
                    .unwrap()
            )
            .unwrap(),
            blocker_bytes
        );
        drop(reopened);

        command(&fixture.root, &["restore", "--staged", "base.txt"]);
        command(&fixture.root, &["restore", "base.txt"]);
        fs::remove_file(fixture.root.join("user-untracked.txt")).unwrap();
        assert!(pending_checkout_is_clean(&fixture.dir, &fixture.task_id).unwrap());
        assert!(
            super::super::resume::resume_landing_finalization(&fixture.dir, &fixture.task_id)
                .unwrap()
        );
        assert_eq!(
            command(&fixture.root, &["rev-parse", "main"]),
            fixture.candidate
        );
        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task(&fixture.task_id).unwrap();
        assert_eq!(task.status, Status::Done);
        assert!(task.completion_blocker.is_none());
        assert_eq!(
            task.completion_disposition,
            Some(CompletionDisposition::Landed)
        );
        assert_eq!(
            *fixture.review_calls.lock().unwrap(),
            vec![ReviewerKind::Flip, ReviewerKind::Eval]
        );
        drop(graph);

        // Explicit replay is idempotent: target/ref, candidate, and review
        // call count stay fixed and source work is never dispatched.
        assert!(
            super::super::resume::resume_landing_finalization(&fixture.dir, &fixture.task_id)
                .unwrap()
        );
        assert_eq!(
            command(&fixture.root, &["rev-parse", "main"]),
            fixture.candidate
        );
        assert_eq!(
            *fixture.review_calls.lock().unwrap(),
            vec![ReviewerKind::Flip, ReviewerKind::Eval]
        );
    }

    #[test]
    fn completion_blockers_stale_candidate_fence_and_moved_target_fail_closed() {
        let park = |fixture: &Fixture| {
            fs::write(fixture.root.join("base.txt"), "dirty\n").unwrap();
            run_at(
                &fixture.dir,
                &fixture.task_id,
                "refs/heads/main",
                Some(&fixture.worker),
            )
            .unwrap();
            command(&fixture.root, &["restore", "base.txt"]);
        };

        let stale_fence = fixture();
        park(&stale_fence);
        modify_graph(stale_fence.dir.join("graph.jsonl"), |graph| {
            graph
                .get_task_mut(&stale_fence.task_id)
                .unwrap()
                .lifecycle
                .fence += 1;
            true
        })
        .unwrap();
        let before = command(&stale_fence.root, &["rev-parse", "main"]);
        let error = resume_pending(&stale_fence.dir, &stale_fence.task_id).unwrap_err();
        assert!(error.to_string().contains("binding is stale"));
        assert_eq!(command(&stale_fence.root, &["rev-parse", "main"]), before);

        let stale_candidate = fixture();
        park(&stale_candidate);
        modify_graph(stale_candidate.dir.join("graph.jsonl"), |graph| {
            graph
                .get_task_mut(&stale_candidate.task_id)
                .unwrap()
                .completion_candidate
                .as_mut()
                .unwrap()
                .eval_receipt = None;
            true
        })
        .unwrap();
        let before = command(&stale_candidate.root, &["rev-parse", "main"]);
        let error = resume_pending(&stale_candidate.dir, &stale_candidate.task_id).unwrap_err();
        assert!(error.to_string().contains("binding is stale"));
        assert_eq!(
            command(&stale_candidate.root, &["rev-parse", "main"]),
            before
        );

        let moved_target = fixture();
        park(&moved_target);
        fs::write(moved_target.root.join("other.txt"), "target moved\n").unwrap();
        command(&moved_target.root, &["add", "other.txt"]);
        command(&moved_target.root, &["commit", "-m", "target moved"]);
        let moved = command(&moved_target.root, &["rev-parse", "main"]);
        let error = resume_pending(&moved_target.dir, &moved_target.task_id).unwrap_err();
        assert!(error.to_string().contains("target ref moved"));
        assert_eq!(command(&moved_target.root, &["rev-parse", "main"]), moved);
        let graph = load_graph(moved_target.dir.join("graph.jsonl")).unwrap();
        assert_eq!(
            graph.get_task(&moved_target.task_id).unwrap().status,
            Status::Waiting
        );
    }

    #[test]
    fn moved_main_refuses_without_rewriting_candidate_or_source_owner() {
        let fixture = fixture();
        fs::write(fixture.root.join("other.txt"), "other\n").unwrap();
        command(&fixture.root, &["add", "other.txt"]);
        command(&fixture.root, &["commit", "-m", "main moved"]);
        let moved = command(&fixture.root, &["rev-parse", "main"]);
        assert_ne!(moved, fixture.integrated);

        let error = run_at(
            &fixture.dir,
            &fixture.task_id,
            "refs/heads/main",
            Some(&fixture.worker),
        )
        .unwrap_err();
        assert!(error.to_string().contains("NeedsRebase"));
        assert_eq!(command(&fixture.root, &["rev-parse", "main"]), moved);
        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task(&fixture.task_id).unwrap();
        assert_eq!(task.status, Status::InProgress);
        assert_eq!(task.completion_disposition, None);
    }
}
