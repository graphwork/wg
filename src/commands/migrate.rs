//! Migration commands. Currently supports the chat-rename migration:
//! rewrites legacy `.coordinator-N` task ids to `.chat-N`, fixes up
//! after-edges, renames `coordinator-loop` tags to `chat-loop`, and
//! rewrites `Coordinator: <name>` / `Coordinator N` titles.

use anyhow::{Result, bail};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;

use worksgood::chat_id::{
    CHAT_LOOP_TAG, CHAT_PREFIX, LEGACY_COORDINATOR_LOOP_TAG, LEGACY_COORDINATOR_PREFIX,
};
use worksgood::graph::{LogEntry, Status, Task};
use worksgood::parser::{load_graph, modify_graph, modify_graph_with_exact_backup};

use super::graph_path;

// Kept nested until the synthesis task wires the public `wg completion ...`
// command surface. This still compiles and tests the owned adapter now.
#[path = "completion_repair.rs"]
pub mod completion_repair;

/// Classify and quarantine legacy active/archive `Done` records.
///
/// Exact pre-migration bytes and content-addressed classification records are
/// persisted before the active compatibility projection changes. The archive
/// itself is read-only. Re-running after a successful migration is a no-op.
pub fn run_review_identity_repair(
    dir: &Path,
    limit: usize,
    dry_run: bool,
    json: bool,
) -> Result<()> {
    if limit == 0 {
        anyhow::bail!("review identity repair limit must be at least 1");
    }
    let graph_file = graph_path(dir);
    let mut report = if dry_run {
        worksgood::parser::preview_review_projections(&graph_file, limit)?
    } else {
        worksgood::parser::repair_review_projections(&graph_file, limit)?
    };
    report.dry_run = dry_run;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let mode = if dry_run { "would repair" } else { "repaired" };
        println!(
            "Review identity migration: examined={} {mode}={} unchanged={} skipped={} invalid={} remaining={}",
            report.examined,
            report.repaired,
            report.unchanged,
            report.skipped,
            report.invalid,
            report.remaining
        );
        for row in &report.rows {
            println!(
                "  {}: {}{}{}",
                row.task_id,
                row.outcome,
                row.reason
                    .as_deref()
                    .map(|reason| format!(" ({reason})"))
                    .unwrap_or_default(),
                if row.activity_ids_restored.is_empty() {
                    String::new()
                } else {
                    format!(" receipts={}", row.activity_ids_restored.join(","))
                }
            );
        }
        println!(
            "Only current receipt-backed projections were considered; missing superseded history was not inferred."
        );
    }
    Ok(())
}

pub fn run_completion_repair(dir: &Path, dry_run: bool, json: bool) -> Result<()> {
    let graph_file = graph_path(dir);
    let archive_file = dir.join("archive.jsonl");
    let graph_bytes = std::fs::read(&graph_file)
        .map_err(|error| anyhow::anyhow!("failed to read {}: {error}", graph_file.display()))?;
    let archive_bytes = match std::fs::read(&archive_file) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(anyhow::anyhow!(
                "failed to read {}: {error}",
                archive_file.display()
            ));
        }
    };
    let graph = worksgood::parser::load_graph(&graph_file)?;
    let archived = archive_bytes
        .as_deref()
        .map(completion_repair::parse_archived_tasks)
        .transpose()?
        .unwrap_or_default();
    let report = completion_repair::classify_legacy_completions(
        &graph,
        &archived,
        &graph_bytes,
        archive_bytes.as_deref(),
    )?;

    if !dry_run && !report.is_noop() {
        // The immutable snapshots/records must win the crash race. A crash
        // here leaves extra inert evidence; replay applies the same projection.
        completion_repair::persist_migration_evidence(
            dir,
            &graph_bytes,
            archive_bytes.as_deref(),
            &report,
        )?;
        let mut apply_error = None;
        modify_graph(
            &graph_file,
            |current| match completion_repair::apply_quarantine_plan(current, &report) {
                Ok(()) => true,
                Err(error) => {
                    apply_error = Some(error);
                    false
                }
            },
        )?;
        if let Some(error) = apply_error {
            return Err(error);
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if report.is_noop() {
        println!("No unverified legacy Done records found.");
    } else {
        let prefix = if dry_run {
            "Dry run: would quarantine"
        } else {
            "Quarantined"
        };
        println!(
            "{prefix} {} legacy Done record(s):",
            report.quarantined_count()
        );
        for record in report.records.iter().filter(|record| {
            record.classification == completion_repair::LegacyClassification::NeedsReconciliation
        }) {
            println!(
                "  {} ({:?}); blocks {} downstream task(s)",
                record.task_id,
                record.location,
                record.blocked_downstream.len()
            );
        }
        println!("No record was blessed, deleted, or removed from archive history.");
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct EvaluationCutoverSource {
    pub task_id: String,
    pub status: String,
    pub evidence: String,
    pub candidate: String,
    pub recovery_action: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvaluationCutoverEdgeRewrite {
    pub task_id: String,
    pub retired_dependency: String,
    pub source_dependency: String,
}

#[derive(Debug, Clone)]
struct PlannedEdgeNormalization {
    task_id: String,
    before: Vec<String>,
    after: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct EvaluationCutoverReport {
    pub operation_kind: String,
    pub cutover_version: u32,
    pub dry_run: bool,
    pub retired_rows: Vec<String>,
    pub newly_inert_rows: Vec<String>,
    pub edge_rewrites: Vec<EvaluationCutoverEdgeRewrite>,
    pub sources: Vec<EvaluationCutoverSource>,
    pub preserved_verdict_files: usize,
    pub backup_path: Option<String>,
    pub backup_digest: Option<String>,
    pub changed: bool,
}

fn count_preserved_verdict_files(dir: &Path) -> usize {
    [
        dir.join("agency/evaluations"),
        dir.join("agency/eval-verdicts"),
    ]
    .into_iter()
    .map(|root| {
        walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
            .count()
    })
    .sum()
}

fn completion_output_identity(output: &worksgood::completion_manifest::OutputRef) -> String {
    match output {
        worksgood::completion_manifest::OutputRef::Git(git) => git.commit_oid.clone(),
        worksgood::completion_manifest::OutputRef::Artifact(artifact) => {
            artifact.content_digest.to_string()
        }
        worksgood::completion_manifest::OutputRef::External(external) => {
            external.after_digest.to_string()
        }
    }
}

fn verify_receipt_publication(
    dir: &Path,
    task: &Task,
    manifest: &worksgood::completion_manifest::CompletionManifest,
    observed: &str,
) -> Result<(), String> {
    use worksgood::graph::CompletionContract;

    match task.completion_contract {
        CompletionContract::Land => {
            let mut commits = manifest.outputs.iter().filter_map(|output| match output {
                worksgood::completion_manifest::OutputRef::Git(git) => {
                    Some(git.commit_oid.as_str())
                }
                _ => None,
            });
            let commit = commits
                .next()
                .ok_or_else(|| "Land receipt has no candidate Git output".to_string())?;
            if commits.next().is_some() {
                return Err("Land receipt candidate has multiple Git outputs".to_string());
            }
            let encoded = observed
                .strip_prefix("git:")
                .ok_or_else(|| "Land receipt publication is not typed as git".to_string())?;
            let (target_ref, published_commit) = encoded
                .rsplit_once(':')
                .ok_or_else(|| "Land receipt publication omits target ref or commit".to_string())?;
            if target_ref.is_empty() || published_commit != commit {
                return Err("Land receipt publication does not bind the selected candidate".into());
            }
            let project = dir
                .parent()
                .ok_or_else(|| "workgraph directory has no project root".to_string())?;
            let status = Command::new("git")
                .args(["merge-base", "--is-ancestor", commit, target_ref])
                .current_dir(project)
                .status()
                .map_err(|error| format!("could not verify Land publication: {error}"))?;
            if !status.success() {
                return Err("Land receipt publication is no longer true".to_string());
            }
        }
        CompletionContract::Report | CompletionContract::Explore => {
            let prefix = if task.completion_contract == CompletionContract::Report {
                "artifacts:"
            } else {
                "exploration:"
            };
            let expected = format!(
                "{}{}",
                prefix,
                manifest
                    .outputs
                    .iter()
                    .map(completion_output_identity)
                    .collect::<Vec<_>>()
                    .join(",")
            );
            if observed != expected {
                return Err("receipt publication does not bind the selected outputs".to_string());
            }
        }
        CompletionContract::Deliver => {
            return Err("legacy Deliver cannot use publication-derived Done".to_string());
        }
    }
    Ok(())
}

/// Parse and verify the typed historical completion receipt without applying a
/// lifecycle transition. The migration reports this evidence, but the normal
/// completion/retry controller remains the only authority that may consume it.
fn verify_typed_completion_receipt(dir: &Path, task: &Task) -> Result<String, String> {
    let candidate = task
        .completion_candidate
        .as_ref()
        .ok_or_else(|| "no selected completion candidate".to_string())?;
    let receipt_id = task
        .completion_receipt
        .as_deref()
        .ok_or_else(|| "no completion receipt reference".to_string())?;
    let disposition = task
        .completion_disposition
        .ok_or_else(|| "no completion disposition".to_string())?;
    if !disposition.satisfies(task.completion_contract) {
        return Err("completion disposition does not satisfy the task contract".to_string());
    }

    let store_root = dir.join("completion/v3");
    let store = worksgood::completion_manifest::CompletionArtifactStore::open(&store_root)
        .map_err(|error| format!("completion store unavailable: {error}"))?;
    let (_, manifest, _, _) = worksgood::completion_task::load_submission_bytes(&store, task)
        .map_err(|error| format!("selected candidate does not verify: {error}"))?;
    let digest = worksgood::completion_manifest::ContentDigest::parse(receipt_id)
        .map_err(|error| format!("completion receipt id is invalid: {error}"))?;
    let object_name = digest
        .as_str()
        .strip_prefix("b3:")
        .expect("parsed b3 digest has prefix");
    let bytes = std::fs::read(store_root.join("objects").join(object_name))
        .map_err(|error| format!("completion receipt object is unavailable: {error}"))?;
    if worksgood::completion_manifest::ContentDigest::of_bytes(&bytes) != digest {
        return Err("completion receipt object digest mismatch".to_string());
    }
    let receipt: worksgood::terminal_observation::ReviewedCompletionReceipt =
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("completion receipt is not the typed receipt: {error}"))?;
    let manifest_digest = manifest.digest().map_err(|error| error.to_string())?;
    let expected_flip = candidate
        .flip_receipt
        .as_ref()
        .ok_or_else(|| "selected candidate has no FLIP receipt".to_string())?
        .content_digest
        .to_string();
    let expected_eval = candidate
        .eval_receipt
        .as_ref()
        .map(|reference| reference.content_digest.to_string());
    if receipt.receipt_version != 1
        || receipt.task_id != task.id
        || receipt.generation != task.lifecycle.generation
        || receipt.manifest_digest != manifest_digest.to_string()
        || receipt.requirements_digest != manifest.requirements_digest.to_string()
        || receipt.flip_receipt_digest != expected_flip
        || receipt.eval_receipt_digest != expected_eval
        || receipt.contract != task.completion_contract.to_string()
        || !matches!(receipt.review_policy.as_str(), "strict" | "advisory")
        || chrono::DateTime::parse_from_rfc3339(&receipt.completed_at).is_err()
    {
        return Err(
            "typed completion receipt is stale or disagrees on task, generation, candidate, contract, or review evidence"
                .to_string(),
        );
    }
    verify_receipt_publication(dir, task, &manifest, &receipt.publication)?;
    Ok(format!(
        "typed receipt {receipt_id} verifies candidate {}, contract {}, disposition {:?}, and publication; lifecycle remains unchanged",
        candidate.manifest.content_digest, task.completion_contract, disposition
    ))
}

fn source_plan(dir: &Path, task: &Task) -> EvaluationCutoverSource {
    let candidate = worksgood::evaluation_cutover::candidate_binding(task);
    let evidence = match verify_typed_completion_receipt(dir, task) {
        Ok(verified) => verified,
        Err(reason) => {
            format!("fail-closed: {reason}; no score or content-addressed bytes imply acceptance")
        }
    };
    EvaluationCutoverSource {
        task_id: task.id.clone(),
        status: task.status.to_string(),
        evidence,
        candidate,
        recovery_action: format!(
            "wg retry {} --reason 'recover retired evaluation state without inferring acceptance'",
            task.id
        ),
    }
}

fn print_evaluation_cutover_report(report: &EvaluationCutoverReport, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    println!("Operation kind: legacy_evaluation_cutover (migration-only; history preserved)");
    println!(
        "Evaluation cutover v{}{}: retired_rows={} newly_inert={} edge_rewrites={} sources={} verdict_files_preserved={} changed={}",
        report.cutover_version,
        if report.dry_run { " dry-run" } else { "" },
        report.retired_rows.len(),
        report.newly_inert_rows.len(),
        report.edge_rewrites.len(),
        report.sources.len(),
        report.preserved_verdict_files,
        report.changed
    );
    if let Some(path) = report.backup_path.as_deref() {
        println!(
            "  exact graph backup: {} ({})",
            path,
            report.backup_digest.as_deref().unwrap_or("unknown")
        );
    }
    for rewrite in &report.edge_rewrites {
        println!(
            "  edge {}: {} -> {}",
            rewrite.task_id, rewrite.retired_dependency, rewrite.source_dependency
        );
    }
    for source in &report.sources {
        println!(
            "  {}: {} -> UNCHANGED (fail-closed) [{}] candidate={}",
            source.task_id, source.status, source.evidence, source.candidate
        );
        println!("    recovery action: {}", source.recovery_action);
    }
    println!(
        "  legacy rows/logs remain in the graph and exact backup; evaluation/verdict files were not rewritten"
    );
    Ok(())
}

/// Mark retired rows as historical and normalize explicit dependencies to the
/// corresponding source. No source lifecycle is inferred or changed.
pub fn run_evaluation_cutover(dir: &Path, dry_run: bool, json: bool) -> Result<()> {
    let graph_file = graph_path(dir);
    let graph = load_graph(&graph_file)?;
    let mut report = EvaluationCutoverReport {
        operation_kind: "legacy_evaluation_cutover".to_string(),
        cutover_version: worksgood::evaluation_cutover::EVALUATION_CUTOVER_VERSION,
        dry_run,
        preserved_verdict_files: count_preserved_verdict_files(dir),
        ..EvaluationCutoverReport::default()
    };
    report.retired_rows = graph
        .tasks()
        .filter(|task| worksgood::evaluation_cutover::is_retired_agency_task_id(&task.id))
        .map(|task| task.id.clone())
        .collect();
    report.retired_rows.sort();
    report.newly_inert_rows = graph
        .tasks()
        .filter(|task| {
            worksgood::evaluation_cutover::is_retired_agency_task_id(&task.id)
                && !worksgood::evaluation_cutover::is_cutover_inert(task)
        })
        .map(|task| task.id.clone())
        .collect();
    report.newly_inert_rows.sort();
    report.sources = graph
        .tasks()
        .filter(|task| matches!(task.status, Status::PendingEval | Status::FailedPendingEval))
        .map(|task| source_plan(dir, task))
        .collect();
    report.sources.sort_by(|a, b| a.task_id.cmp(&b.task_id));

    let mut edge_plans = Vec::new();
    for task in graph.tasks() {
        let before = task.after.clone();
        let mut after = Vec::with_capacity(before.len());
        let mut seen = HashSet::new();
        for dependency in &before {
            let normalized = graph
                .get_task(dependency)
                .filter(|row| worksgood::evaluation_cutover::is_retired_agency_task_id(&row.id))
                .and_then(|row| worksgood::evaluation_cutover::source_id(&row.id))
                .filter(|source| !source.is_empty())
                .unwrap_or(dependency)
                .to_string();
            if normalized != *dependency {
                report.edge_rewrites.push(EvaluationCutoverEdgeRewrite {
                    task_id: task.id.clone(),
                    retired_dependency: dependency.clone(),
                    source_dependency: normalized.clone(),
                });
            }
            if seen.insert(normalized.clone()) {
                after.push(normalized);
            }
        }
        if after != before {
            edge_plans.push(PlannedEdgeNormalization {
                task_id: task.id.clone(),
                before,
                after,
            });
        }
    }
    report.edge_rewrites.sort_by(|a, b| {
        (&a.task_id, &a.retired_dependency).cmp(&(&b.task_id, &b.retired_dependency))
    });
    report.changed = !report.newly_inert_rows.is_empty() || !edge_plans.is_empty();
    if dry_run || !report.changed {
        return print_evaluation_cutover_report(&report, json);
    }

    let inert_ids = report.newly_inert_rows.clone();
    let mut refusal = None;
    let backup_dir = dir
        .join(worksgood::evaluation_cutover::EVALUATION_CUTOVER_DIR)
        .join("backups");
    let (_graph, backup) = modify_graph_with_exact_backup(&graph_file, &backup_dir, |current| {
        for id in &inert_ids {
            let Some(task) = current.get_task_mut(id) else {
                refusal = Some(format!("retired row '{id}' disappeared during migration"));
                return false;
            };
            if !task
                .tags
                .iter()
                .any(|tag| tag == worksgood::evaluation_cutover::EVALUATION_CUTOVER_TAG)
            {
                task.tags
                    .push(worksgood::evaluation_cutover::EVALUATION_CUTOVER_TAG.to_string());
            }
        }
        for plan in &edge_plans {
            let Some(task) = current.get_task_mut(&plan.task_id) else {
                refusal = Some(format!(
                    "dependent '{}' disappeared during migration",
                    plan.task_id
                ));
                return false;
            };
            if task.after != plan.before {
                refusal = Some(format!(
                    "dependencies for '{}' changed during migration; rerun the command",
                    plan.task_id
                ));
                return false;
            }
            task.after.clone_from(&plan.after);
        }
        true
    })?;
    if let Some(error) = refusal {
        bail!(error);
    }
    if let Some(backup) = backup {
        report.backup_path = Some(backup.path.display().to_string());
        report.backup_digest = Some(backup.content_digest);
    }
    print_evaluation_cutover_report(&report, json)
}

/// Result of a chat-rename migration.
#[derive(Debug, Default, Clone)]
pub struct ChatRenameMigrationResult {
    /// Old `.coordinator-N` ids that were rewritten to `.chat-N`.
    pub renamed_ids: Vec<(String, String)>,
    /// Number of `after`-edges that were rewritten on dependent tasks.
    pub rewritten_edges: usize,
    /// Number of tags renamed from `coordinator-loop` to `chat-loop`.
    pub renamed_tags: usize,
    /// Number of titles rewritten from `Coordinator: …` / `Coordinator N` to the new form.
    pub renamed_titles: usize,
}

impl ChatRenameMigrationResult {
    pub fn is_empty(&self) -> bool {
        self.renamed_ids.is_empty()
            && self.rewritten_edges == 0
            && self.renamed_tags == 0
            && self.renamed_titles == 0
    }
}

fn maybe_new_title(title: &str) -> Option<String> {
    if let Some(rest) = title.strip_prefix("Coordinator: ") {
        return Some(format!("Chat: {}", rest));
    }
    if let Some(rest) = title.strip_prefix("Coordinator ")
        && !rest.is_empty()
        && rest.chars().all(|c| c.is_ascii_digit())
    {
        return Some(format!("Chat {}", rest));
    }
    None
}

/// Rewrite legacy chat-agent task ids and tags to the new canonical form.
///
/// Runs in-place on `<dir>/graph.jsonl`. Idempotent — running twice on a
/// migrated graph is a no-op.
pub fn run_chat_rename(dir: &Path, dry_run: bool, json: bool) -> Result<()> {
    let graph_path = graph_path(dir);

    let mut result = ChatRenameMigrationResult::default();
    let now = chrono::Utc::now().to_rfc3339();

    if dry_run {
        let graph = worksgood::parser::load_graph(&graph_path)?;
        for task in graph.tasks() {
            if task.id.starts_with(LEGACY_COORDINATOR_PREFIX) {
                let suffix = &task.id[LEGACY_COORDINATOR_PREFIX.len()..];
                let new_id = format!("{}{}", CHAT_PREFIX, suffix);
                result.renamed_ids.push((task.id.clone(), new_id));
            }
            if task.tags.iter().any(|t| t == LEGACY_COORDINATOR_LOOP_TAG) {
                result.renamed_tags += 1;
            }
            if maybe_new_title(&task.title).is_some() {
                result.renamed_titles += 1;
            }
            for after in &task.after {
                if after.starts_with(LEGACY_COORDINATOR_PREFIX) {
                    result.rewritten_edges += 1;
                }
            }
        }
    } else {
        modify_graph(&graph_path, |graph| {
            // Phase 1: build the id remap.
            let id_remap: HashMap<String, String> = graph
                .tasks()
                .filter_map(|t| {
                    t.id.strip_prefix(LEGACY_COORDINATOR_PREFIX)
                        .map(|suffix| (t.id.clone(), format!("{}{}", CHAT_PREFIX, suffix)))
                })
                .collect();
            for (old, new) in &id_remap {
                result.renamed_ids.push((old.clone(), new.clone()));
            }

            // Phase 2: collect all current task ids (keys to iterate).
            let all_ids: Vec<String> = graph.tasks().map(|t| t.id.clone()).collect();

            // Phase 3: rewrite each task's fields in place — at this point
            // the HashMap key still equals the task.id (no re-keying yet),
            // so get_task_mut works with the OLD id.
            for old_key in &all_ids {
                if let Some(t) = graph.get_task_mut(old_key) {
                    // Rewrite after-edges for this task.
                    let mut local_edges = 0usize;
                    for after in t.after.iter_mut() {
                        if let Some(new_id) = id_remap.get(after) {
                            *after = new_id.clone();
                            local_edges += 1;
                        }
                    }
                    if local_edges > 0 {
                        result.rewritten_edges += local_edges;
                    }

                    // Rewrite legacy tags.
                    let mut renamed_tag_in_task = false;
                    for tag in t.tags.iter_mut() {
                        if tag == LEGACY_COORDINATOR_LOOP_TAG {
                            *tag = CHAT_LOOP_TAG.to_string();
                            renamed_tag_in_task = true;
                        }
                    }
                    if renamed_tag_in_task {
                        result.renamed_tags += 1;
                    }

                    // Rewrite legacy titles.
                    if let Some(new_title) = maybe_new_title(&t.title) {
                        t.title = new_title;
                        result.renamed_titles += 1;
                    }

                    // Rewrite this task's own id if it's a legacy coordinator id.
                    if let Some(new_id) = id_remap.get(&t.id) {
                        let old_id = t.id.clone();
                        t.id = new_id.clone();
                        t.log.push(LogEntry {
                            timestamp: now.clone(),
                            actor: Some("migration".to_string()),
                            user: Some(worksgood::current_user()),
                            message: format!(
                                "wg migrate chat-rename: renamed task id {} -> {}",
                                old_id, new_id
                            ),
                        });
                    }
                }
            }

            // Phase 4: re-key the HashMap so lookups by the NEW id work.
            // We pull each renamed task out by its old key and re-add it,
            // which inserts at the new key (add_node uses node.id()).
            for (old_id, _new_id) in &id_remap {
                if let Some(node) = graph.take_node(old_id) {
                    graph.add_node(node);
                }
            }

            true
        })?;
    }

    if json {
        let payload = serde_json::json!({
            "renamed_ids": result.renamed_ids,
            "rewritten_edges": result.rewritten_edges,
            "renamed_tags": result.renamed_tags,
            "renamed_titles": result.renamed_titles,
            "dry_run": dry_run,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if result.is_empty() {
        println!("No legacy coordinator data found — graph is already on the new schema.");
    } else {
        if dry_run {
            println!("Dry run — no changes written:");
        } else {
            println!("Migration complete:");
        }
        println!("  task ids renamed: {}", result.renamed_ids.len());
        for (old, new) in &result.renamed_ids {
            println!("    {} -> {}", old, new);
        }
        println!("  after-edges rewritten: {}", result.rewritten_edges);
        println!(
            "  tags renamed (coordinator-loop -> chat-loop): {}",
            result.renamed_tags
        );
        println!("  titles renamed: {}", result.renamed_titles);
    }
    Ok(())
}

/// Result of a retire-compact-archive migration.
#[derive(Debug, Default, Clone)]
pub struct RetireCompactArchiveResult {
    /// Task ids that were marked Abandoned.
    pub abandoned_ids: Vec<String>,
    /// Number of `after` edges that were stripped from other tasks because
    /// they pointed at retired `.compact-N` / `.archive-N` ids.
    pub stripped_edges: usize,
}

impl RetireCompactArchiveResult {
    pub fn is_empty(&self) -> bool {
        self.abandoned_ids.is_empty() && self.stripped_edges == 0
    }
}

/// Mark every `.compact-N` and `.archive-N` task as Abandoned and strip
/// after-edges referencing those ids from other tasks. Idempotent — running
/// twice on a migrated graph is a no-op.
pub fn run_retire_compact_archive(dir: &Path, dry_run: bool, json: bool) -> Result<()> {
    let graph_path = graph_path(dir);
    let now = chrono::Utc::now().to_rfc3339();
    let mut result = RetireCompactArchiveResult::default();

    if dry_run {
        let graph = worksgood::parser::load_graph(&graph_path)?;
        for task in graph.tasks() {
            if (task.id.starts_with(".compact-") || task.id.starts_with(".archive-"))
                && !task.status.is_terminal()
            {
                result.abandoned_ids.push(task.id.clone());
            }
        }
        for task in graph.tasks() {
            for dep in &task.after {
                if dep.starts_with(".compact-") || dep.starts_with(".archive-") {
                    result.stripped_edges += 1;
                }
            }
        }
    } else {
        worksgood::parser::modify_graph(&graph_path, |graph| {
            let all_ids: Vec<String> = graph.tasks().map(|t| t.id.clone()).collect();
            for tid in &all_ids {
                let is_target = tid.starts_with(".compact-") || tid.starts_with(".archive-");
                let can_retire = graph
                    .get_task(tid)
                    .is_some_and(|task| !task.status.is_terminal());
                if is_target
                    && can_retire
                    && let Some(t) = graph.get_task_mut(tid)
                {
                    let request = worksgood::lifecycle::TransitionRequest::new(
                        worksgood::lifecycle::TransitionKind::Abandoned,
                        worksgood::lifecycle::LifecycleActor::operator(worksgood::current_user()),
                        "legacy_compact_archive_retired",
                        format!("retire-compact-archive:{tid}:{}", t.lifecycle.generation),
                    )
                    .expecting(worksgood::lifecycle::FenceExpectation::current(t));
                    if worksgood::lifecycle::apply_transition(t, request).is_err() {
                        continue;
                    }
                    t.completed_at.get_or_insert_with(|| now.clone());
                    t.cycle_config = None;
                    t.log.push(LogEntry {
                        timestamp: now.clone(),
                        actor: Some("migration".to_string()),
                        user: Some(worksgood::current_user()),
                        message:
                            "wg migrate retire-compact-archive: retired .compact-N/.archive-N \
                             cycle scaffolding"
                                .to_string(),
                    });
                    result.abandoned_ids.push(tid.clone());
                }
            }
            // Strip after-edges pointing at retired ids.
            for tid in &all_ids {
                if let Some(t) = graph.get_task_mut(tid) {
                    let before = t.after.len();
                    t.after.retain(|dep| {
                        !(dep.starts_with(".compact-") || dep.starts_with(".archive-"))
                    });
                    let removed = before - t.after.len();
                    if removed > 0 {
                        result.stripped_edges += removed;
                    }
                }
            }
            true
        })?;
    }

    if json {
        let payload = serde_json::json!({
            "abandoned_ids": result.abandoned_ids,
            "stripped_edges": result.stripped_edges,
            "dry_run": dry_run,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if result.is_empty() {
        println!("No legacy .compact-N or .archive-N tasks found — graph is already migrated.");
    } else {
        if dry_run {
            println!("Dry run — no changes written:");
        } else {
            println!("Migration complete:");
        }
        println!("  tasks abandoned: {}", result.abandoned_ids.len());
        for id in &result.abandoned_ids {
            println!("    {}", id);
        }
        println!("  after-edges stripped: {}", result.stripped_edges);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use worksgood::graph::{Status, Task, WorkGraph};

    fn write_graph(dir: &Path, tasks: Vec<Task>) {
        let workgraph_dir = dir.join(".wg");
        std::fs::create_dir_all(&workgraph_dir).unwrap();
        let graph_path = workgraph_dir.join("graph.jsonl");
        let mut graph = WorkGraph::new();
        for t in tasks {
            graph.add_node(worksgood::graph::Node::Task(t));
        }
        worksgood::parser::save_graph(&graph, &graph_path).unwrap();
    }

    #[test]
    fn evaluation_cutover_dry_run_backup_edge_rewrite_and_preservation() {
        let tmp = TempDir::new().unwrap();
        let wg = tmp.path().join(".wg");
        let source = Task {
            id: "source".into(),
            title: "source".into(),
            status: Status::PendingEval,
            log: vec![LogEntry {
                timestamp: "2026-01-01T00:00:00Z".into(),
                actor: Some("legacy".into()),
                user: None,
                message: "original source log".into(),
            }],
            ..Task::default()
        };
        let evaluator = Task {
            id: ".evaluate-source".into(),
            title: "legacy evaluator".into(),
            status: Status::Open,
            after: vec!["source".into()],
            log: vec![LogEntry {
                timestamp: "2026-01-01T00:00:01Z".into(),
                actor: Some("legacy".into()),
                user: None,
                message: "original evaluator log".into(),
            }],
            ..Task::default()
        };
        let assigner = Task {
            id: ".assign-source".into(),
            title: "legacy assigner".into(),
            status: Status::Failed,
            ..Task::default()
        };
        let flipper = Task {
            id: ".flip-source".into(),
            title: "legacy flipper".into(),
            status: Status::Done,
            ..Task::default()
        };
        let downstream = Task {
            id: "downstream".into(),
            title: "downstream".into(),
            status: Status::Open,
            after: vec![
                ".assign-source".into(),
                ".flip-source".into(),
                ".evaluate-source".into(),
            ],
            ..Task::default()
        };
        write_graph(
            tmp.path(),
            vec![source, evaluator, assigner, flipper, downstream],
        );
        std::fs::create_dir_all(wg.join("agency/evaluations")).unwrap();
        let verdict_path = wg.join("agency/evaluations/original.json");
        let verdict_bytes = br#"{"task_id":"source","score":1.0,"foreign":"keep exact"}"#;
        std::fs::write(&verdict_path, verdict_bytes).unwrap();
        let graph_path = wg.join("graph.jsonl");
        let original = std::fs::read(&graph_path).unwrap();

        run_evaluation_cutover(&wg, true, true).unwrap();
        assert_eq!(std::fs::read(&graph_path).unwrap(), original);
        assert!(!wg.join("migrations/evaluation-cutover-v1").exists());

        run_evaluation_cutover(&wg, false, true).unwrap();
        let backups: Vec<_> =
            std::fs::read_dir(wg.join("migrations/evaluation-cutover-v1/backups"))
                .unwrap()
                .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(
            std::fs::read(backups[0].as_ref().unwrap().path()).unwrap(),
            original
        );
        assert_eq!(std::fs::read(&verdict_path).unwrap(), verdict_bytes);
        let migrated = load_graph(&graph_path).unwrap();
        for retired in [".assign-source", ".flip-source", ".evaluate-source"] {
            assert!(worksgood::evaluation_cutover::is_cutover_inert(
                migrated.get_task(retired).unwrap()
            ));
        }
        let eval = migrated.get_task(".evaluate-source").unwrap();
        assert_eq!(eval.status, Status::Open);
        assert_eq!(eval.log[0].message, "original evaluator log");
        let pending = migrated.get_task("source").unwrap();
        assert_eq!(pending.status, Status::PendingEval);
        assert_eq!(pending.log[0].message, "original source log");
        assert!(
            source_plan(&wg, pending)
                .recovery_action
                .contains("wg retry")
        );
        assert_eq!(
            migrated.get_task("downstream").unwrap().after,
            vec!["source".to_string()]
        );

        let once = std::fs::read(&graph_path).unwrap();
        run_evaluation_cutover(&wg, false, true).unwrap();
        assert_eq!(std::fs::read(&graph_path).unwrap(), once);
        assert_eq!(
            std::fs::read_dir(wg.join("migrations/evaluation-cutover-v1/backups"))
                .unwrap()
                .count(),
            1
        );
    }

    fn receipt_fixture(
        tmp: &TempDir,
    ) -> (
        Task,
        worksgood::terminal_observation::ReviewedCompletionReceipt,
    ) {
        use worksgood::completion_manifest::{
            COMPLETION_MANIFEST_VERSION, CompletionArtifactStore, CompletionManifest,
            CompletionManifestRef, EvidenceRef, OutputRef,
        };
        use worksgood::completion_task::CompletionCandidateRefs;
        use worksgood::graph::CompletionDisposition;

        let wg = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg).unwrap();
        let mut source = Task {
            id: "exact-source".into(),
            title: "exact source".into(),
            status: Status::PendingEval,
            completion_contract: worksgood::graph::CompletionContract::Report,
            ..Task::default()
        };
        let store = CompletionArtifactStore::open(wg.join("completion/v3")).unwrap();
        let requirements_bytes =
            worksgood::completion_task::task_requirements_bytes(&source).unwrap();
        let requirements = store
            .put_bytes(&requirements_bytes, "application/json")
            .unwrap();
        let summary = store.put_bytes(b"summary", "text/plain").unwrap();
        let output = store.put_bytes(b"report", "text/plain").unwrap();
        let validation = store.put_bytes(b"tests passed", "text/plain").unwrap();
        let flip = store.put_bytes(b"typed flip", "application/json").unwrap();
        let publication = format!("artifacts:{}", output.content_digest);
        let manifest = CompletionManifest {
            manifest_version: COMPLETION_MANIFEST_VERSION,
            task_id: source.id.clone(),
            generation: source.lifecycle.generation,
            completion_contract: worksgood::simple_land::CompletionContract::Report,
            requirements_digest: requirements.content_digest.clone(),
            source_revision: "legacy-revision".into(),
            outputs: vec![OutputRef::Artifact(output)],
            validation_evidence: vec![EvidenceRef {
                content_digest: validation.content_digest.clone(),
                immutable_locator: validation.immutable_locator.clone(),
                evidence_kind: "commands-run".into(),
                media_type: validation.media_type.clone(),
                size: validation.size,
                review_projection: None,
            }],
            worker_summary_digest: summary.content_digest.clone(),
        };
        let manifest_digest = manifest.digest().unwrap();
        let manifest_object = store
            .put_bytes(&manifest.canonical_bytes().unwrap(), "application/json")
            .unwrap();
        source.completion_candidate = Some(CompletionCandidateRefs {
            manifest: CompletionManifestRef {
                content_digest: manifest_object.content_digest,
                immutable_locator: manifest_object.immutable_locator,
                size: manifest_object.size,
            },
            requirements,
            worker_summary: summary,
            dependency_outputs: Vec::new(),
            review_binding: None,
            flip_receipt: Some(flip.clone()),
            eval_receipt: None,
        });
        source.completion_disposition = Some(CompletionDisposition::Reported);
        let receipt = worksgood::terminal_observation::ReviewedCompletionReceipt {
            receipt_version: 1,
            task_id: source.id.clone(),
            generation: source.lifecycle.generation,
            manifest_digest: manifest_digest.to_string(),
            requirements_digest: manifest.requirements_digest.to_string(),
            flip_receipt_digest: flip.content_digest.to_string(),
            eval_receipt_digest: None,
            review_policy: "strict".into(),
            contract: source.completion_contract.to_string(),
            publication,
            completed_at: "2026-01-01T00:00:00Z".into(),
        };
        (source, receipt)
    }

    fn attach_receipt(
        wg: &Path,
        task: &mut Task,
        receipt: &worksgood::terminal_observation::ReviewedCompletionReceipt,
    ) {
        let store =
            worksgood::completion_manifest::CompletionArtifactStore::open(wg.join("completion/v3"))
                .unwrap();
        let bytes = worksgood::identity::canonical_json(&serde_json::to_value(receipt).unwrap());
        task.completion_receipt = Some(
            store
                .put_bytes(&bytes, "application/vnd.worksgood.completion-receipt+json")
                .unwrap()
                .content_digest
                .to_string(),
        );
    }

    #[test]
    fn typed_receipt_binding_rejects_arbitrary_and_every_mismatched_field() {
        let tmp = TempDir::new().unwrap();
        let wg = tmp.path().join(".wg");
        let (mut source, receipt) = receipt_fixture(&tmp);
        attach_receipt(&wg, &mut source, &receipt);
        assert!(verify_typed_completion_receipt(&wg, &source).is_ok());

        let store =
            worksgood::completion_manifest::CompletionArtifactStore::open(wg.join("completion/v3"))
                .unwrap();
        let arbitrary = store
            .put_bytes(b"arbitrary CAS bytes", "application/json")
            .unwrap();
        let mut arbitrary_source = source.clone();
        arbitrary_source.completion_receipt = Some(arbitrary.content_digest.to_string());
        assert!(verify_typed_completion_receipt(&wg, &arbitrary_source).is_err());

        for (label, mutate) in [
            ("task", 0_u8),
            ("generation", 1),
            ("candidate", 2),
            ("contract", 3),
            ("publication", 4),
            ("stale", 5),
        ] {
            let mut bad = receipt.clone();
            match mutate {
                0 => bad.task_id = "other-task".into(),
                1 => bad.generation += 1,
                2 => bad.manifest_digest = format!("b3:{}", "0".repeat(64)),
                3 => bad.contract = "land".into(),
                4 => bad.publication = format!("artifacts:b3:{}", "1".repeat(64)),
                5 => {
                    bad.generation = source.lifecycle.generation + 2;
                    bad.completed_at = "2020-01-01T00:00:00Z".into();
                }
                _ => unreachable!(),
            }
            let mut candidate = source.clone();
            attach_receipt(&wg, &mut candidate, &bad);
            assert!(
                verify_typed_completion_receipt(&wg, &candidate).is_err(),
                "{label} mismatch was accepted"
            );
        }

        let mut wrong_disposition = source.clone();
        wrong_disposition.completion_disposition =
            Some(worksgood::graph::CompletionDisposition::Landed);
        assert!(verify_typed_completion_receipt(&wg, &wrong_disposition).is_err());

        // Even a fully verified dangling receipt is evidence only here: the
        // cutover never invents the lifecycle transition that consumes it.
        write_graph(tmp.path(), vec![source]);
        run_evaluation_cutover(&wg, false, true).unwrap();
        assert_eq!(
            load_graph(wg.join("graph.jsonl"))
                .unwrap()
                .get_task("exact-source")
                .unwrap()
                .status,
            Status::PendingEval
        );
    }

    #[test]
    fn evaluation_cutover_never_adjudicates_pending_or_failed_pending_sources() {
        let tmp = TempDir::new().unwrap();
        let wg = tmp.path().join(".wg");
        let failed = Task {
            id: "failed-source".into(),
            title: "failed".into(),
            status: Status::FailedPendingEval,
            failure_reason: Some("worker exited 9".into()),
            ..Task::default()
        };
        let pending = Task {
            id: "pending-source".into(),
            title: "pending".into(),
            status: Status::PendingEval,
            ..Task::default()
        };
        let evaluator = Task {
            id: ".evaluate-pending-source".into(),
            title: "retired".into(),
            status: Status::Open,
            ..Task::default()
        };
        write_graph(tmp.path(), vec![failed, pending, evaluator]);
        run_evaluation_cutover(&wg, false, true).unwrap();
        let graph = load_graph(wg.join("graph.jsonl")).unwrap();
        assert_eq!(
            graph.get_task("failed-source").unwrap().status,
            Status::FailedPendingEval
        );
        assert_eq!(
            graph.get_task("pending-source").unwrap().status,
            Status::PendingEval
        );
        assert_eq!(
            graph
                .get_task("failed-source")
                .unwrap()
                .failure_reason
                .as_deref(),
            Some("worker exited 9")
        );
        assert!(
            source_plan(&wg, graph.get_task("pending-source").unwrap())
                .recovery_action
                .contains("wg retry")
        );
    }

    #[test]
    fn migrates_legacy_coordinator_id_to_chat_prefix() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let coord = Task {
            id: ".coordinator-3".to_string(),
            title: "Coordinator: alice".to_string(),
            status: Status::InProgress,
            tags: vec!["coordinator-loop".to_string()],
            ..Default::default()
        };
        let dependent = Task {
            id: "feature-x".to_string(),
            title: "Feature X".to_string(),
            status: Status::Open,
            after: vec![".coordinator-3".to_string()],
            ..Default::default()
        };
        write_graph(dir, vec![coord, dependent]);

        run_chat_rename(&dir.join(".wg"), false, true).unwrap();

        let graph = worksgood::parser::load_graph(&dir.join(".wg").join("graph.jsonl")).unwrap();

        // .chat-3 exists with renamed title and tag
        let migrated = graph.get_task(".chat-3").expect("chat-3 should exist");
        assert_eq!(migrated.title, "Chat: alice");
        assert!(migrated.tags.iter().any(|t| t == "chat-loop"));
        assert!(!migrated.tags.iter().any(|t| t == "coordinator-loop"));

        // Old key is gone
        assert!(graph.get_task(".coordinator-3").is_none());

        // Dependent task's after-edge was rewritten
        let dep = graph.get_task("feature-x").expect("dependent must exist");
        assert!(dep.after.iter().any(|a| a == ".chat-3"));
        assert!(!dep.after.iter().any(|a| a == ".coordinator-3"));
    }

    #[test]
    fn migration_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let coord = Task {
            id: ".coordinator-0".to_string(),
            title: "Coordinator 0".to_string(),
            status: Status::InProgress,
            tags: vec!["coordinator-loop".to_string()],
            ..Default::default()
        };
        write_graph(dir, vec![coord]);

        run_chat_rename(&dir.join(".wg"), false, true).unwrap();
        run_chat_rename(&dir.join(".wg"), false, true).unwrap();

        let graph = worksgood::parser::load_graph(&dir.join(".wg").join("graph.jsonl")).unwrap();
        assert!(graph.get_task(".chat-0").is_some());
        assert!(graph.get_task(".coordinator-0").is_none());
    }

    #[test]
    fn retire_compact_archive_abandons_legacy_tasks() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let chat = Task {
            id: ".chat-0".to_string(),
            title: "Chat 0".to_string(),
            status: Status::InProgress,
            ..Default::default()
        };
        let compact = Task {
            id: ".compact-0".to_string(),
            title: "Compact 0".to_string(),
            status: Status::Open,
            ..Default::default()
        };
        let archive = Task {
            id: ".archive-0".to_string(),
            title: "Archive 0".to_string(),
            status: Status::Open,
            ..Default::default()
        };
        let blocked = Task {
            id: "real-task".to_string(),
            title: "Real task".to_string(),
            status: Status::Open,
            after: vec![".compact-0".to_string(), "real-prereq".to_string()],
            ..Default::default()
        };
        write_graph(dir, vec![chat, compact, archive, blocked]);

        run_retire_compact_archive(&dir.join(".wg"), false, true).unwrap();

        let graph = worksgood::parser::load_graph(&dir.join(".wg").join("graph.jsonl")).unwrap();
        assert_eq!(
            graph.get_task(".compact-0").unwrap().status,
            Status::Abandoned
        );
        assert_eq!(
            graph.get_task(".archive-0").unwrap().status,
            Status::Abandoned
        );
        assert_eq!(
            graph.get_task(".chat-0").unwrap().status,
            Status::InProgress
        );
        let real = graph.get_task("real-task").unwrap();
        assert_eq!(real.after, vec!["real-prereq".to_string()]);
    }

    #[test]
    fn retire_compact_archive_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let compact = Task {
            id: ".compact-0".to_string(),
            title: "Compact 0".to_string(),
            status: Status::Open,
            ..Default::default()
        };
        write_graph(dir, vec![compact]);

        run_retire_compact_archive(&dir.join(".wg"), false, true).unwrap();
        run_retire_compact_archive(&dir.join(".wg"), false, true).unwrap();

        let graph = worksgood::parser::load_graph(&dir.join(".wg").join("graph.jsonl")).unwrap();
        assert_eq!(
            graph.get_task(".compact-0").unwrap().status,
            Status::Abandoned
        );
    }

    #[test]
    fn completion_repair_preserves_archive_and_second_run_is_noop() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let active = Task {
            id: "legacy-active".to_string(),
            title: "Legacy active".to_string(),
            status: Status::Done,
            ..Default::default()
        };
        let dependent = Task {
            id: "dependent".to_string(),
            title: "Dependent".to_string(),
            status: Status::Open,
            after: vec!["legacy-active".to_string(), "legacy-archived".to_string()],
            ..Default::default()
        };
        write_graph(dir, vec![active, dependent]);
        let wg_dir = dir.join(".wg");
        let archived = Task {
            id: "legacy-archived".to_string(),
            title: "Legacy archived".to_string(),
            status: Status::Done,
            ..Default::default()
        };
        let archive_bytes = format!(
            "{}\n",
            serde_json::to_string(&worksgood::graph::Node::Task(archived)).unwrap()
        );
        std::fs::write(wg_dir.join("archive.jsonl"), &archive_bytes).unwrap();

        run_completion_repair(&wg_dir, false, true).unwrap();
        let migrated = worksgood::parser::load_graph(wg_dir.join("graph.jsonl")).unwrap();
        assert_eq!(
            migrated.get_task("legacy-active").unwrap().status,
            Status::Incomplete
        );
        assert_eq!(
            migrated
                .get_archived_boundary("legacy-archived")
                .unwrap()
                .status,
            Status::Incomplete
        );
        assert_eq!(
            std::fs::read_to_string(wg_dir.join("archive.jsonl")).unwrap(),
            archive_bytes,
            "archive history must remain byte-for-byte unchanged"
        );
        assert!(
            wg_dir
                .join("completion/v2/legacy/migration-report.json")
                .exists()
        );
        let ledger = std::fs::read_to_string(wg_dir.join("lifecycle/events.jsonl")).unwrap();
        assert!(ledger.contains("\"event_kind\":\"legacy-completion-quarantined\""));
        assert!(ledger.contains("\"new_state\":\"incomplete\""));

        let after_first_bytes = std::fs::read(wg_dir.join("graph.jsonl")).unwrap();
        run_completion_repair(&wg_dir, false, true).unwrap();
        let after_second_bytes = std::fs::read(wg_dir.join("graph.jsonl")).unwrap();
        assert_eq!(after_first_bytes, after_second_bytes);
    }

    #[test]
    fn dry_run_does_not_modify() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let coord = Task {
            id: ".coordinator-1".to_string(),
            title: "Coordinator 1".to_string(),
            status: Status::InProgress,
            tags: vec!["coordinator-loop".to_string()],
            ..Default::default()
        };
        write_graph(dir, vec![coord]);

        run_chat_rename(&dir.join(".wg"), true, true).unwrap();

        let graph = worksgood::parser::load_graph(&dir.join(".wg").join("graph.jsonl")).unwrap();
        // Legacy id still present, no chat- yet
        assert!(graph.get_task(".coordinator-1").is_some());
        assert!(graph.get_task(".chat-1").is_none());
    }
}

// ---------------------------------------------------------------------------
// `wg migrate config` — rewrite stale config.toml files to canonical form.
// ---------------------------------------------------------------------------

/// What scopes `wg migrate config` should rewrite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigMigrateTarget {
    Global,
    Local,
    All,
}

/// Per-file summary of what `wg migrate config` changed (or would change).
#[derive(Debug, Default, Clone)]
pub struct ConfigMigrateResult {
    /// Path of the file that was inspected.
    pub path: std::path::PathBuf,
    /// Whether the file existed at all.
    pub existed: bool,
    /// Top-level keys removed because they are deprecated/no-op.
    pub removed_keys: Vec<String>,
    /// Keys renamed (legacy → canonical).
    pub renamed_keys: Vec<(String, String)>,
    /// Keys whose values were rewritten (e.g. stale model strings).
    pub rewritten_values: Vec<(String, String, String)>, // (key, old, new)
    /// Path of the backup that was written (None on dry-run / no changes).
    pub backup_path: Option<std::path::PathBuf>,
    /// Whether the file was actually written (false on dry-run / no-op).
    pub wrote: bool,
}

impl ConfigMigrateResult {
    pub fn is_noop(&self) -> bool {
        self.removed_keys.is_empty()
            && self.renamed_keys.is_empty()
            && self.rewritten_values.is_empty()
    }
}

/// Top-level entry point: dispatch to global / local / both based on target.
pub fn run_config_migrate(
    workgraph_dir: &Path,
    target: ConfigMigrateTarget,
    dry_run: bool,
    json: bool,
) -> Result<()> {
    let global_path = worksgood::config::Config::global_config_path()?;
    let local_path = workgraph_dir.join("config.toml");

    let mut results = Vec::new();
    match target {
        ConfigMigrateTarget::Global => {
            results.push(migrate_one(&global_path, dry_run)?);
        }
        ConfigMigrateTarget::Local => {
            results.push(migrate_one(&local_path, dry_run)?);
        }
        ConfigMigrateTarget::All => {
            results.push(migrate_one(&global_path, dry_run)?);
            results.push(migrate_one(&local_path, dry_run)?);
        }
    }

    if json {
        let payload: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                serde_json::json!({
                    "path": r.path.display().to_string(),
                    "existed": r.existed,
                    "removed_keys": r.removed_keys,
                    "renamed_keys": r.renamed_keys,
                    "rewritten_values": r.rewritten_values,
                    "wrote": r.wrote,
                    "backup_path": r.backup_path.as_ref().map(|p| p.display().to_string()),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        for r in &results {
            print_one(r, dry_run);
        }
    }
    Ok(())
}

fn print_one(r: &ConfigMigrateResult, dry_run: bool) {
    if !r.existed {
        println!(
            "{}: file does not exist — nothing to migrate",
            r.path.display()
        );
        return;
    }
    if r.is_noop() {
        println!("{}: already canonical — no changes", r.path.display());
        return;
    }
    let prefix = if dry_run { "[dry-run] " } else { "" };
    println!("{}{}:", prefix, r.path.display());
    for k in &r.removed_keys {
        println!("  - removed deprecated key: {}", k);
    }
    for (old, new) in &r.renamed_keys {
        println!("  - renamed: {} → {}", old, new);
    }
    for (k, old, new) in &r.rewritten_values {
        println!("  - {}: {:?} → {:?}", k, old, new);
    }
    if r.wrote {
        if let Some(bk) = &r.backup_path {
            println!("  ✓ wrote (backup: {})", bk.display());
        } else {
            println!("  ✓ wrote");
        }
    } else if dry_run {
        println!("  (dry-run — file not modified; rerun without --dry-run to apply)");
    }
}

/// Canonicalization pipeline + report, re-exported from the worksgood lib so
/// the wg-binary migrate command and lib-side profile activation share one
/// transform. See `worksgood::config_migrate`.
pub(crate) use worksgood::config_migrate::{CanonicalizeReport, canonicalize_in_place};

/// Read one config file, compute the canonical form, and (unless dry-run)
/// write it back with a `.pre-migrate.<timestamp>` backup.
///
/// Exposed `pub(crate)` so `wg config lint` can reuse the predicates in
/// dry-run mode without touching the file. When `dry_run = true` the
/// returned `ConfigMigrateResult` describes what *would* change.
pub(crate) fn migrate_one(path: &Path, dry_run: bool) -> Result<ConfigMigrateResult> {
    let mut result = ConfigMigrateResult {
        path: path.to_path_buf(),
        ..Default::default()
    };
    if !path.exists() {
        return Ok(result);
    }
    result.existed = true;

    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {}", path.display(), e))?;

    let mut doc: toml::Value = match toml::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            anyhow::bail!(
                "{} is not valid TOML: {}\nFix syntax errors before migrating.",
                path.display(),
                e
            );
        }
    };

    // Run the full canonicalization pipeline (drop deprecated keys, rename
    // legacy fields, fix stale model strings, drop orphaned [openrouter]) and
    // capture what changed. This is the single shared entry point used by
    // both `wg migrate config` (which writes the file) and profile activation
    // (which writes canonical config without round-tripping through `Config`
    // serialization — see `profile::named::apply_profile_as_global_config`).
    let CanonicalizeReport {
        removed,
        renamed,
        rewritten,
    } = canonicalize_in_place(&mut doc);

    result.removed_keys = removed;
    result.renamed_keys = renamed;
    result.rewritten_values = rewritten;

    if result.is_noop() || dry_run {
        return Ok(result);
    }

    // Write backup + new file.
    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let backup = path.with_extension(format!("toml.pre-migrate.{}", now));
    std::fs::copy(path, &backup).map_err(|e| {
        anyhow::anyhow!(
            "failed to back up {} → {}: {}",
            path.display(),
            backup.display(),
            e
        )
    })?;
    result.backup_path = Some(backup);

    let new_body = toml::to_string_pretty(&doc)
        .map_err(|e| anyhow::anyhow!("failed to serialize migrated config: {}", e))?;
    std::fs::write(path, new_body)
        .map_err(|e| anyhow::anyhow!("failed to write {}: {}", path.display(), e))?;
    result.wrote = true;

    Ok(result)
}

#[cfg(test)]
mod config_migrate_tests {
    use super::*;
    use tempfile::TempDir;

    fn write_config(dir: &Path, body: &str) -> std::path::PathBuf {
        let path = dir.join("config.toml");
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn strips_deprecated_agent_executor() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            tmp.path(),
            r#"
[agent]
executor = "claude"
model = "claude:opus"
"#,
        );
        let r = migrate_one(&path, false).unwrap();
        assert!(
            r.removed_keys.iter().any(|k| k == "agent.executor"),
            "should remove agent.executor; got {:?}",
            r.removed_keys,
        );
        let migrated = std::fs::read_to_string(&path).unwrap();
        assert!(
            !migrated.contains("executor"),
            "migrated config should not contain executor; got:\n{}",
            migrated,
        );
        assert!(
            migrated.contains("model = \"claude:opus\""),
            "migrated config should keep model; got:\n{}",
            migrated,
        );
    }

    #[test]
    fn fixes_stale_openrouter_sonnet_model() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            tmp.path(),
            r#"
[agent]
model = "openrouter:anthropic/claude-sonnet-4"
"#,
        );
        let r = migrate_one(&path, false).unwrap();
        assert!(
            r.rewritten_values
                .iter()
                .any(|(_, _, new)| new == "openrouter:anthropic/claude-sonnet-4-6"),
            "should rewrite stale sonnet-4 to sonnet-4-6; got {:?}",
            r.rewritten_values,
        );
        let migrated = std::fs::read_to_string(&path).unwrap();
        assert!(migrated.contains("openrouter:anthropic/claude-sonnet-4-6"));
        assert!(!migrated.contains("\"openrouter:anthropic/claude-sonnet-4\""));
    }

    #[test]
    fn renames_chat_agent_to_coordinator_agent() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            tmp.path(),
            r#"
[dispatcher]
chat_agent = true
max_chats = 4
"#,
        );
        let r = migrate_one(&path, false).unwrap();
        assert!(
            r.renamed_keys
                .iter()
                .any(|(_, new)| new == "dispatcher.coordinator_agent"),
            "should rename chat_agent → coordinator_agent; got {:?}",
            r.renamed_keys,
        );
        let migrated = std::fs::read_to_string(&path).unwrap();
        assert!(migrated.contains("coordinator_agent"));
        assert!(migrated.contains("max_coordinators"));
        assert!(!migrated.contains("chat_agent"));
        assert!(!migrated.contains("max_chats"));
    }

    #[test]
    fn renames_poll_interval_to_safety_interval() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            tmp.path(),
            r#"
[dispatcher]
poll_interval = 5

[coordinator]
poll_interval = 9
"#,
        );
        let r = migrate_one(&path, false).unwrap();
        assert!(
            r.renamed_keys
                .iter()
                .any(|(old, new)| old == "dispatcher.poll_interval"
                    && new == "dispatcher.safety_interval"),
            "should rename dispatcher.poll_interval; got {:?}",
            r.renamed_keys,
        );
        assert!(
            r.renamed_keys
                .iter()
                .any(|(old, new)| old == "coordinator.poll_interval"
                    && new == "coordinator.safety_interval"),
            "should rename coordinator.poll_interval; got {:?}",
            r.renamed_keys,
        );
        let migrated = std::fs::read_to_string(&path).unwrap();
        assert!(migrated.contains("safety_interval = 5"));
        assert!(migrated.contains("safety_interval = 9"));
        assert!(!migrated.contains("poll_interval"));
    }

    #[test]
    fn poll_interval_duplicate_is_removed_when_safety_interval_exists() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            tmp.path(),
            r#"
[dispatcher]
poll_interval = 5
safety_interval = 7
"#,
        );
        let r = migrate_one(&path, false).unwrap();
        assert!(
            r.renamed_keys
                .iter()
                .any(|(old, _)| old == "dispatcher.poll_interval"),
            "duplicate legacy key should still be reported; got {:?}",
            r.renamed_keys,
        );
        let migrated = std::fs::read_to_string(&path).unwrap();
        assert!(migrated.contains("safety_interval = 7"));
        assert!(!migrated.contains("poll_interval"));
        assert!(!migrated.contains("safety_interval = 5"));
    }

    #[test]
    fn dry_run_does_not_write() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            tmp.path(),
            r#"
[agent]
executor = "claude"
"#,
        );
        let original = std::fs::read_to_string(&path).unwrap();
        let r = migrate_one(&path, true).unwrap();
        assert!(!r.removed_keys.is_empty());
        assert!(!r.wrote);
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(original, after, "dry-run must not touch the file");
    }

    #[test]
    fn idempotent_on_canonical_config() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            tmp.path(),
            r#"
[agent]
model = "claude:opus"

[tiers]
fast = "claude:haiku"
standard = "claude:opus"
premium = "claude:opus"
"#,
        );
        let r = migrate_one(&path, false).unwrap();
        assert!(
            r.is_noop(),
            "canonical config should be a no-op; got {:?}",
            r
        );
    }

    #[test]
    fn fixes_stale_codex_default_pins_to_gpt55() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            tmp.path(),
            r#"
[agent]
model = "codex:o1-pro"

[tiers]
fast = "codex:gpt-5-mini"
standard = "codex:gpt-5"
premium = "codex:o1-pro"
"#,
        );
        let r = migrate_one(&path, false).unwrap();
        assert!(
            r.rewritten_values
                .iter()
                .any(|(path, old, new)| path == "agent.model"
                    && old == "codex:o1-pro"
                    && new == "codex:gpt-5.5"),
            "should rewrite default agent codex:o1-pro to codex:gpt-5.5; got {:?}",
            r.rewritten_values,
        );
        let migrated = std::fs::read_to_string(&path).unwrap();
        assert!(
            migrated.contains("codex:gpt-5.5"),
            "migrated should contain codex:gpt-5.5"
        );
        assert!(
            !migrated.contains("\"codex:o1-pro\""),
            "migrated should not contain codex:o1-pro"
        );
        assert!(
            !migrated.contains("\"codex:gpt-5-mini\""),
            "migrated should not contain codex:gpt-5-mini"
        );
        assert!(
            !migrated.contains("\"codex:gpt-5\""),
            "migrated should not contain bare codex:gpt-5"
        );
    }

    #[test]
    fn fixes_stale_codex_tier_defaults_to_gpt55() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            tmp.path(),
            r#"
[tiers]
standard = "codex:gpt-5-codex"
premium = "codex:gpt-5.4-pro"
"#,
        );
        let r = migrate_one(&path, false).unwrap();
        assert!(
            r.rewritten_values
                .iter()
                .any(|(path, old, new)| path == "tiers.standard"
                    && old == "codex:gpt-5-codex"
                    && new == "codex:gpt-5.5"),
            "should rewrite standard codex:gpt-5-codex to codex:gpt-5.5; got {:?}",
            r.rewritten_values,
        );
        assert!(
            r.rewritten_values
                .iter()
                .any(|(_, old, new)| old == "codex:gpt-5.4-pro" && new == "codex:gpt-5.5"),
            "should rewrite codex:gpt-5.4-pro to codex:gpt-5.5; got {:?}",
            r.rewritten_values,
        );
    }

    #[test]
    fn fixes_stale_claude_default_pins_to_opus() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            tmp.path(),
            r#"
[agent]
model = "sonnet"

[dispatcher]
model = "claude:sonnet"

[tiers]
fast = "claude:haiku"
standard = "claude:sonnet"
premium = "claude:opus"

[models.task_agent]
model = "sonnet"
"#,
        );
        let r = migrate_one(&path, false).unwrap();
        assert!(
            r.rewritten_values
                .iter()
                .any(|(path, old, new)| path == "agent.model"
                    && old == "sonnet"
                    && new == "claude:opus"),
            "should rewrite bare sonnet default pin to claude:opus; got {:?}",
            r.rewritten_values,
        );
        let migrated = std::fs::read_to_string(&path).unwrap();
        assert!(migrated.contains("model = \"claude:opus\""));
        assert!(!migrated.contains("model = \"sonnet\""));
        assert!(!migrated.contains("standard = \"claude:sonnet\""));
    }

    #[test]
    fn rewrites_deprecated_local_prefix_to_nex() {
        // `local:` is the deprecated alias for `nex:` (canonical, matches
        // the `wg nex` subcommand). `wg migrate config` rewrites it.
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            tmp.path(),
            r#"
[agent]
model = "local:qwen3-coder-30b"

[tiers]
fast = "local:qwen3-coder-30b"
"#,
        );
        let r = migrate_one(&path, false).unwrap();
        assert!(
            r.rewritten_values
                .iter()
                .any(|(_, old, new)| old == "local:qwen3-coder-30b"
                    && new == "nex:qwen3-coder-30b"),
            "should rewrite local:qwen3-coder-30b to nex:qwen3-coder-30b; got {:?}",
            r.rewritten_values,
        );
        let migrated = std::fs::read_to_string(&path).unwrap();
        assert!(migrated.contains("\"nex:qwen3-coder-30b\""));
        assert!(!migrated.contains("\"local:qwen3-coder-30b\""));
    }

    #[test]
    fn rewrites_deprecated_oai_compat_prefix_to_nex() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            tmp.path(),
            r#"
[agent]
model = "oai-compat:gpt-5"
"#,
        );
        let r = migrate_one(&path, false).unwrap();
        assert!(
            r.rewritten_values
                .iter()
                .any(|(_, old, new)| old == "oai-compat:gpt-5" && new == "nex:gpt-5"),
            "should rewrite oai-compat:gpt-5 to nex:gpt-5; got {:?}",
            r.rewritten_values,
        );
        let migrated = std::fs::read_to_string(&path).unwrap();
        assert!(migrated.contains("\"nex:gpt-5\""));
        assert!(!migrated.contains("\"oai-compat:gpt-5\""));
    }

    #[test]
    fn rewrites_bare_openrouter_prefix_to_handler_first() {
        // Handler-first enforcement: a bare `openrouter:` leading token is a
        // provider namespace, not a handler. `wg migrate config` PREPENDS the
        // canonical `nex:` handler, keeping `openrouter` as the inner dialect
        // (the wire is distinct and the native handler still needs it). This
        // is the exact spec behind the 14h-401 incident.
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            tmp.path(),
            r#"
[dispatcher]
model = "openrouter:z-ai/glm-5.2"

[tiers]
fast = "ollama:llama3"
"#,
        );
        let r = migrate_one(&path, false).unwrap();
        assert!(
            r.rewritten_values
                .iter()
                .any(|(_, old, new)| old == "openrouter:z-ai/glm-5.2"
                    && new == "nex:openrouter:z-ai/glm-5.2"),
            "should prepend nex: to openrouter; got {:?}",
            r.rewritten_values,
        );
        assert!(
            r.rewritten_values
                .iter()
                .any(|(_, old, new)| old == "ollama:llama3" && new == "nex:ollama:llama3"),
            "should prepend nex: to ollama; got {:?}",
            r.rewritten_values,
        );
        let migrated = std::fs::read_to_string(&path).unwrap();
        assert!(migrated.contains("\"nex:openrouter:z-ai/glm-5.2\""));
        assert!(migrated.contains("\"nex:ollama:llama3\""));
        // No bare provider prefix survives the migration.
        assert!(!migrated.contains("\"openrouter:z-ai/glm-5.2\""));
        assert!(!migrated.contains("\"ollama:llama3\""));
    }

    #[test]
    fn lint_flags_bare_openrouter_via_dry_run() {
        // `wg config lint` reuses `migrate_one(path, dry_run=true)`, so the
        // dry run must REPORT the bare-provider rewrite without writing the
        // file. This is the exact predicate the lint surface prints.
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            tmp.path(),
            r#"
[dispatcher]
model = "openrouter:z-ai/glm-5.2"
"#,
        );
        let r = migrate_one(&path, true).unwrap();
        assert!(!r.wrote, "dry run must not write the file");
        assert!(
            r.rewritten_values
                .iter()
                .any(|(_, old, new)| old == "openrouter:z-ai/glm-5.2"
                    && new == "nex:openrouter:z-ai/glm-5.2"),
            "lint dry run must flag the bare openrouter prefix; got {:?}",
            r.rewritten_values,
        );
        // File is untouched on disk.
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(on_disk.contains("\"openrouter:z-ai/glm-5.2\""));
    }

    #[test]
    fn migrate_writes_pre_migrate_backup() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            tmp.path(),
            r#"
[agent]
model = "local:qwen3-coder"
"#,
        );
        let r = migrate_one(&path, false).unwrap();
        assert!(r.wrote);
        let backup = r.backup_path.expect("backup path must be set on a write");
        assert!(backup.exists(), "backup file must exist on disk");
        let backup_body = std::fs::read_to_string(&backup).unwrap();
        // Backup is the pre-migration content — still the deprecated prefix.
        assert!(backup_body.contains("local:qwen3-coder"));
    }

    #[test]
    fn drops_orphaned_openrouter_section_on_claude_cli_project() {
        // A claude-cli project should never carry a default [openrouter]
        // section. The registry-refresh job would otherwise probe
        // OpenRouter every poll and fill the daemon log with auth errors.
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            tmp.path(),
            r#"
[agent]
model = "claude:opus"

[tiers]
fast = "claude:haiku"
standard = "claude:sonnet"
premium = "claude:opus"

[openrouter]
cap_behavior = "escalate"
key_status_check_interval_minutes = 5
warn_at_usage_percent = 80
cost_estimation_buffer = 1.2
enable_cache_tracking = true
track_session_costs = true
persist_cost_history = false
"#,
        );
        let r = migrate_one(&path, false).unwrap();
        assert!(
            r.removed_keys.iter().any(|k| k.starts_with("openrouter")),
            "should remove orphaned [openrouter] section; got {:?}",
            r.removed_keys,
        );
        let migrated = std::fs::read_to_string(&path).unwrap();
        assert!(
            !migrated.lines().any(|l| l.trim() == "[openrouter]"),
            "migrated config must not contain [openrouter]; got:\n{}",
            migrated,
        );
        // claude config remains intact
        assert!(migrated.contains("claude:opus"));
    }

    #[test]
    fn keeps_openrouter_section_when_used() {
        // If the project has an openrouter:* model anywhere, the
        // [openrouter] section is load-bearing — leave it alone.
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            tmp.path(),
            r#"
[agent]
model = "openrouter:anthropic/claude-opus-4-7"

[tiers]
premium = "openrouter:anthropic/claude-opus-4-7"

[openrouter]
cost_cap_global_usd = 5.0
"#,
        );
        let r = migrate_one(&path, false).unwrap();
        assert!(
            !r.removed_keys.iter().any(|k| k.starts_with("openrouter")),
            "must not remove [openrouter] when a model spec uses it; got {:?}",
            r.removed_keys,
        );
        let migrated = std::fs::read_to_string(&path).unwrap();
        assert!(
            migrated.lines().any(|l| l.trim() == "[openrouter]"),
            "migrated config must keep [openrouter]; got:\n{}",
            migrated,
        );
        assert!(migrated.contains("cost_cap_global_usd"));
    }

    #[test]
    fn drop_orphaned_openrouter_is_idempotent() {
        // Running migrate twice on a config that's already had its
        // orphan section removed must be a no-op for the openrouter check.
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            tmp.path(),
            r#"
[agent]
model = "claude:opus"

[tiers]
fast = "claude:haiku"
standard = "claude:sonnet"
premium = "claude:opus"
"#,
        );
        let r = migrate_one(&path, false).unwrap();
        assert!(
            !r.removed_keys.iter().any(|k| k.starts_with("openrouter")),
            "first pass on a config without [openrouter] should not report removing it; got {:?}",
            r.removed_keys,
        );
        // Second pass is also a no-op
        let r2 = migrate_one(&path, false).unwrap();
        assert!(r2.is_noop(), "second pass should be a no-op; got {:?}", r2);
    }

    #[test]
    fn renames_legacy_coordinator_section_to_dispatcher() {
        let tmp = TempDir::new().unwrap();
        let path = write_config(
            tmp.path(),
            r#"
[coordinator]
max_agents = 4
"#,
        );
        let r = migrate_one(&path, false).unwrap();
        assert!(
            r.renamed_keys
                .iter()
                .any(|(old, new)| old == "[coordinator]" && new == "[dispatcher]"),
            "should rename [coordinator] → [dispatcher]; got {:?}",
            r.renamed_keys,
        );
        let migrated = std::fs::read_to_string(&path).unwrap();
        assert!(migrated.contains("[dispatcher]"));
        assert!(!migrated.contains("[coordinator]"));
    }
}
