use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
use worksgood::completion_manifest::{GitOutput, OutputRef, ReviewResolver};
use worksgood::completion_task::{load_exact_review_pair, load_submission_bytes};
use worksgood::graph::{CompletionContract, CompletionDisposition, LogEntry, Status};
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
    validate_integration_ref(integration_ref)?;
    let graph_path = dir.join("graph.jsonl");
    let graph = load_graph(&graph_path)?;
    let task = graph
        .get_task(id)
        .with_context(|| format!("task '{id}' not found"))?;
    require_source_owner(task, id)?;
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
    load_exact_review_pair(&completion_store, &submission, &manifest, &resolved)?;
    let git_output = exact_git_output(&manifest.outputs)?;
    worksgood::control_plane::assert_tree_has_no_control_plane(
        project_root,
        &git_output.commit_oid,
    )?;

    let _lock = LandingLock::acquire(project_root)?;
    let observed_before = git(project_root, &["rev-parse", integration_ref])?;
    let already_published = is_ancestor(project_root, &git_output.commit_oid, &observed_before)?;

    let root_checkout_synchronized = if already_published {
        synchronize_root_checkout(project_root, integration_ref, &observed_before, false)?
    } else {
        if observed_before != git_output.integrated_main_oid {
            bail!(
                "NeedsRebase: current integration ref is {}, reviewed candidate integrated {}; merge current main in the same worker, revalidate, and submit a new manifest",
                observed_before,
                git_output.integrated_main_oid
            );
        }
        let worker_worktree = worker_worktree.context(
            "initial landing requires the retained worker worktree; crash recovery may omit it after publication",
        )?;
        verify_worker_worktree(worker_worktree, &git_output.commit_oid)?;
        ensure_root_checkout_clean_if_attached(project_root, integration_ref)?;
        if !is_ancestor(
            project_root,
            &git_output.integrated_main_oid,
            &git_output.commit_oid,
        )? {
            bail!("reviewed commit is not a fast-forward of integrated_main_oid");
        }
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
        synchronize_root_checkout(project_root, integration_ref, &git_output.commit_oid, true)?
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
    Ok(())
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

fn ensure_root_checkout_clean_if_attached(project: &Path, integration_ref: &str) -> Result<()> {
    if symbolic_head(project).as_deref() == Some(integration_ref) {
        let status = git(project, &["status", "--porcelain", "--untracked-files=no"])?;
        if !status.is_empty() {
            bail!("integration root has tracked or index changes; refusing publication");
        }
    }
    Ok(())
}

fn synchronize_root_checkout(
    project: &Path,
    integration_ref: &str,
    target: &str,
    clean_prechecked: bool,
) -> Result<bool> {
    if symbolic_head(project).as_deref() != Some(integration_ref) {
        return Ok(false);
    }
    if !clean_prechecked {
        ensure_root_checkout_clean_if_attached(project, integration_ref)?;
    }
    git(project, &["reset", "--hard", target])?;
    Ok(true)
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
            || task
                .completion_candidate
                .as_ref()
                .map(|candidate| &candidate.manifest.content_digest)
                != Some(manifest_digest)
        {
            refusal = Some(
                "task candidate changed after Git publication; accepted commit remains recoverable by ancestry"
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
    use worksgood::graph::{Node, Task, WorkGraph};
    use worksgood::parser::save_graph;

    struct PassReviewer {
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

    struct Fixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        dir: PathBuf,
        worker: PathBuf,
        candidate: String,
        integrated: String,
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
        let mut task = Task {
            id: "land-task".to_string(),
            title: "Land exact candidate".to_string(),
            description: Some("Land result.\n\n## Validation\nInspect diff.".to_string()),
            status: Status::InProgress,
            completion_contract: CompletionContract::Land,
            ..Task::default()
        };
        task.lifecycle.generation = 2;
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
        let manifest_path = root.join("manifest.json");
        fs::write(&manifest_path, manifest.canonical_bytes().unwrap()).unwrap();
        let summary_path = root.join("summary.txt");
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
            calls,
        };
        super::super::completion_submit::run_with_reviewers(
            &dir,
            "land-task",
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
        }
    }

    #[test]
    fn land_compare_and_fast_forwards_reviewed_commit() {
        let fixture = fixture();
        run_at(
            &fixture.dir,
            "land-task",
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
        let task = graph.get_task("land-task").unwrap();
        assert_eq!(
            task.completion_disposition,
            Some(CompletionDisposition::Landed)
        );
        assert!(task.completion_receipt.is_some());
        assert_eq!(task.status, Status::InProgress);
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
            "land-task",
            "refs/heads/main",
            Some(&fixture.worker),
        )
        .unwrap_err();
        assert!(error.to_string().contains("NeedsRebase"));
        assert_eq!(command(&fixture.root, &["rev-parse", "main"]), moved);
        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task("land-task").unwrap();
        assert_eq!(task.status, Status::InProgress);
        assert_eq!(task.completion_disposition, None);
    }
}
