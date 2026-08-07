//! Fail-closed classification and repair planning for pre-v2 completion rows.
//!
//! A legacy `Done` is historical evidence, not completion authority.  This
//! adapter preserves the original graph/archive bytes, emits immutable
//! quarantine records, and changes only the active compatibility projection.
//! It never manufactures a GraphSave and never deletes retained work.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use worksgood::completion_evidence::{COMPLETION_PROTOCOL_MAJOR, COMPLETION_SCHEMA_VERSION};
use worksgood::graph::{ArchivedBoundary, Node, Status, Task, WorkGraph};
use worksgood::lifecycle::{
    ActorKind, LifecycleActor, TransitionKind, TransitionRequest, apply_transition,
};

pub const LEGACY_QUARANTINE_TAG: &str = "completion:legacy-quarantined";
const LEGACY_STORE_DIR: &str = "completion/v2/legacy";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LegacyRecordLocation {
    Active,
    Archived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LegacyClassification {
    VerifiedV2,
    NeedsReconciliation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyCompletionRecord {
    pub schema_version: u32,
    pub protocol_major: u32,
    pub task_id: String,
    pub location: LegacyRecordLocation,
    pub classification: LegacyClassification,
    pub original_node_cid: String,
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_downstream: Vec<String>,
}

impl LegacyCompletionRecord {
    pub fn cid(&self) -> Result<String> {
        worksgood::completion_evidence::content_cid(self)
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyMigrationReport {
    pub schema_version: u32,
    pub protocol_major: u32,
    pub graph_snapshot_cid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive_snapshot_cid: Option<String>,
    pub records: Vec<LegacyCompletionRecord>,
}

impl LegacyMigrationReport {
    pub fn quarantined_count(&self) -> usize {
        self.records
            .iter()
            .filter(|record| record.classification == LegacyClassification::NeedsReconciliation)
            .count()
    }

    pub fn is_noop(&self) -> bool {
        self.quarantined_count() == 0
    }
}

fn bytes_cid(bytes: &[u8]) -> String {
    format!("wgcid:v2:blake3:{}", blake3::hash(bytes).to_hex())
}

fn node_cid(task: &Task) -> Result<String> {
    worksgood::completion_evidence::content_cid(&Node::Task(task.clone()))
        .map_err(|error| anyhow::anyhow!(error.to_string()))
}

/// Parse archived task rows without changing the archive. Comments and blank
/// lines remain part of the separately persisted byte snapshot.
pub fn parse_archived_tasks(bytes: &[u8]) -> Result<Vec<Task>> {
    let text = std::str::from_utf8(bytes).context("archive.jsonl is not UTF-8")?;
    let mut tasks = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let node: Node = serde_json::from_str(trimmed)
            .with_context(|| format!("invalid archive.jsonl row {}", index + 1))?;
        if let Node::Task(task) = node {
            tasks.push(task);
        }
    }
    Ok(tasks)
}

fn downstream_index(graph: &WorkGraph, archived: &[Task]) -> BTreeMap<String, Vec<String>> {
    let mut reverse: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for task in graph.tasks().chain(archived.iter()) {
        for predecessor in &task.after {
            reverse
                .entry(predecessor.clone())
                .or_default()
                .push(task.id.clone());
        }
    }
    for successors in reverse.values_mut() {
        successors.sort();
        successors.dedup();
    }
    reverse
}

fn transitive_downstream(task_id: &str, reverse: &BTreeMap<String, Vec<String>>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([task_id.to_string()]);
    while let Some(current) = queue.pop_front() {
        for successor in reverse.get(&current).into_iter().flatten() {
            if seen.insert(successor.clone()) {
                queue.push_back(successor.clone());
            }
        }
    }
    seen.remove(task_id);
    seen.into_iter().collect()
}

/// Classify every active and archived successful-looking record. A v2 row is
/// accepted only when it is already a lifecycle-verified GraphSave projection;
/// all other `Done` rows are quarantined. This deliberately has no heuristic
/// "probably landed" branch.
pub fn classify_legacy_completions(
    graph: &WorkGraph,
    archived: &[Task],
    graph_bytes: &[u8],
    archive_bytes: Option<&[u8]>,
) -> Result<LegacyMigrationReport> {
    let reverse = downstream_index(graph, archived);
    let mut records = Vec::new();

    for (location, task) in graph
        .tasks()
        .map(|task| (LegacyRecordLocation::Active, task))
        .chain(
            archived
                .iter()
                .map(|task| (LegacyRecordLocation::Archived, task)),
        )
        .filter(|(_, task)| task.status == Status::Done)
    {
        if location == LegacyRecordLocation::Archived
            && (graph.get_task(&task.id).is_some()
                || graph
                    .get_archived_boundary(&task.id)
                    .is_some_and(|boundary| boundary.status != Status::Done))
        {
            // A restored active generation supersedes this archive projection;
            // an existing non-Done boundary proves the archived row was already
            // quarantined on a prior idempotent pass.
            continue;
        }
        let verified = task.graph_save_completion_disposition().is_some();
        records.push(LegacyCompletionRecord {
            schema_version: COMPLETION_SCHEMA_VERSION,
            protocol_major: COMPLETION_PROTOCOL_MAJOR,
            task_id: task.id.clone(),
            location,
            classification: if verified {
                LegacyClassification::VerifiedV2
            } else {
                LegacyClassification::NeedsReconciliation
            },
            original_node_cid: node_cid(task)?,
            reason_code: if verified {
                "existing-graph-save-projection".to_string()
            } else {
                "legacy-done-without-verified-graph-save".to_string()
            },
            blocked_downstream: if verified {
                Vec::new()
            } else {
                transitive_downstream(&task.id, &reverse)
            },
        });
    }
    records.sort_by(|a, b| {
        a.task_id
            .cmp(&b.task_id)
            .then_with(|| format!("{:?}", a.location).cmp(&format!("{:?}", b.location)))
    });

    Ok(LegacyMigrationReport {
        schema_version: COMPLETION_SCHEMA_VERSION,
        protocol_major: COMPLETION_PROTOCOL_MAJOR,
        graph_snapshot_cid: bytes_cid(graph_bytes),
        archive_snapshot_cid: archive_bytes.map(bytes_cid),
        records,
    })
}

fn quarantine_active_task(task: &mut Task, record: &LegacyCompletionRecord) -> Result<()> {
    let record_cid = record.cid()?;
    let request = TransitionRequest::new(
        TransitionKind::LegacyCompletionQuarantined {
            record_ref: record_cid.clone(),
        },
        LifecycleActor {
            kind: ActorKind::Reconciler,
            id: "completion-v2-legacy-migrator".to_string(),
        },
        "legacy_done_quarantined",
        format!("legacy-done-quarantined:{}", record.original_node_cid),
    )
    .with_evidence(record_cid.clone());
    apply_transition(task, request)
        .map_err(|error| anyhow::anyhow!("failed to append quarantine event: {error}"))?;

    // `Incomplete` is the compatibility spelling for a terminal hold; the
    // lifecycle transition owns that projection and ledger replay reproduces it.
    if !task.tags.iter().any(|tag| tag == LEGACY_QUARANTINE_TAG) {
        task.tags.push(LEGACY_QUARANTINE_TAG.to_string());
    }
    task.log.push(worksgood::graph::LogEntry {
        timestamp: chrono::Utc::now().to_rfc3339(),
        actor: Some("completion-v2-legacy-migrator".to_string()),
        user: Some(worksgood::current_user()),
        message: format!(
            "Legacy Done quarantined as NeedsReconciliation; original={} record={}. Deterministic reconstruction, retry, or abandon is required.",
            record.original_node_cid, record_cid
        ),
    });
    Ok(())
}

/// Apply only the compatibility projection described by a prior immutable
/// plan. Archived task rows are never rewritten; only their active boundary is
/// replaced with a non-satisfying marker.
pub fn apply_quarantine_plan(graph: &mut WorkGraph, report: &LegacyMigrationReport) -> Result<()> {
    for record in report
        .records
        .iter()
        .filter(|record| record.classification == LegacyClassification::NeedsReconciliation)
    {
        match record.location {
            LegacyRecordLocation::Active => {
                let task = graph.get_task_mut(&record.task_id).ok_or_else(|| {
                    anyhow::anyhow!(
                        "active task '{}' changed after classification",
                        record.task_id
                    )
                })?;
                if task.status == Status::Done {
                    quarantine_active_task(task, record)?;
                }
            }
            LegacyRecordLocation::Archived => {
                if graph.get_task(&record.task_id).is_some() {
                    continue;
                }
                let existing = graph.take_node(&record.task_id);
                let boundary = match existing {
                    Some(Node::ArchivedBoundary(boundary)) => ArchivedBoundary {
                        status: Status::Incomplete,
                        ..boundary
                    },
                    Some(other) => {
                        graph.add_node(other);
                        continue;
                    }
                    None => ArchivedBoundary {
                        id: record.task_id.clone(),
                        title: format!("{} (legacy completion quarantined)", record.task_id),
                        status: Status::Incomplete,
                        predecessors: Vec::new(),
                        successors: record.blocked_downstream.clone(),
                        archived_at: chrono::Utc::now().to_rfc3339(),
                    },
                };
                graph.add_node(Node::ArchivedBoundary(boundary));
            }
        }
    }
    Ok(())
}

pub fn legacy_store_dir(workgraph_dir: &Path) -> PathBuf {
    workgraph_dir.join(LEGACY_STORE_DIR)
}

/// Persist exact pre-migration byte snapshots plus content-addressed
/// classification records. Replaying the same plan writes the same objects.
pub fn persist_migration_evidence(
    workgraph_dir: &Path,
    graph_bytes: &[u8],
    archive_bytes: Option<&[u8]>,
    report: &LegacyMigrationReport,
) -> Result<()> {
    let store = legacy_store_dir(workgraph_dir);
    let snapshots = store.join("snapshots");
    let objects = store.join("objects");
    let reports = store.join("reports");
    std::fs::create_dir_all(&snapshots)?;
    std::fs::create_dir_all(&objects)?;
    std::fs::create_dir_all(&reports)?;

    write_create_or_verify(
        &snapshots.join(format!("{}.graph.jsonl", report.graph_snapshot_cid)),
        graph_bytes,
    )?;
    if let (Some(cid), Some(bytes)) = (&report.archive_snapshot_cid, archive_bytes) {
        write_create_or_verify(&snapshots.join(format!("{cid}.archive.jsonl")), bytes)?;
    }
    for record in &report.records {
        let cid = record.cid()?;
        let bytes = serde_json::to_vec_pretty(record)?;
        write_create_or_verify(&objects.join(format!("{cid}.json")), &bytes)?;
    }
    let report_bytes = serde_json::to_vec_pretty(report)?;
    let report_cid = worksgood::completion_evidence::content_cid(report)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    write_create_or_verify(&reports.join(format!("{report_cid}.json")), &report_bytes)?;
    worksgood::atomic_file::write_atomic(&store.join("migration-report.json"), &report_bytes)?;
    Ok(())
}

fn write_create_or_verify(path: &Path, bytes: &[u8]) -> Result<()> {
    match std::fs::read(path) {
        Ok(existing) if existing == bytes => Ok(()),
        Ok(_) => anyhow::bail!("immutable migration object conflicts at {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            worksgood::atomic_file::write_atomic_create_new(path, bytes)
                .with_context(|| format!("failed to persist {}", path.display()))
        }
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, status: Status, after: &[&str]) -> Task {
        Task {
            id: id.to_string(),
            title: id.to_string(),
            status,
            after: after.iter().map(|value| value.to_string()).collect(),
            ..Task::default()
        }
    }

    #[test]
    fn completion_repair_quarantines_legacy_done_and_lists_transitive_impact() {
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task("legacy", Status::Done, &[])));
        graph.add_node(Node::Task(task("child", Status::Open, &["legacy"])));
        graph.add_node(Node::Task(task("grandchild", Status::Open, &["child"])));
        let graph_bytes = b"original graph bytes\n";
        let report =
            classify_legacy_completions(&graph, &[], graph_bytes, None).expect("classification");
        assert_eq!(report.quarantined_count(), 1);
        assert_eq!(
            report.records[0].blocked_downstream,
            vec!["child".to_string(), "grandchild".to_string()]
        );

        apply_quarantine_plan(&mut graph, &report).expect("quarantine");
        let legacy = graph.get_task("legacy").unwrap();
        assert_eq!(legacy.status, Status::Incomplete);
        assert!(legacy.tags.iter().any(|tag| tag == LEGACY_QUARANTINE_TAG));
        assert!(
            legacy
                .lifecycle
                .audit
                .iter()
                .any(|event| event.event_kind == "legacy-completion-quarantined")
        );
    }

    #[test]
    fn completion_repair_archive_row_remains_unchanged_and_boundary_blocks() {
        let archived = task("old", Status::Done, &[]);
        let archive_bytes = format!(
            "{}\n",
            serde_json::to_string(&Node::Task(archived.clone())).unwrap()
        );
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task("dependent", Status::Open, &["old"])));
        let report = classify_legacy_completions(
            &graph,
            std::slice::from_ref(&archived),
            b"graph\n",
            Some(archive_bytes.as_bytes()),
        )
        .unwrap();
        apply_quarantine_plan(&mut graph, &report).unwrap();
        assert_eq!(
            graph.get_archived_boundary("old").unwrap().status,
            Status::Incomplete
        );
        assert_eq!(
            parse_archived_tasks(archive_bytes.as_bytes()).unwrap(),
            vec![archived]
        );
    }

    #[test]
    fn completion_repair_second_classification_is_noop() {
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task("legacy", Status::Done, &[])));
        let first = classify_legacy_completions(&graph, &[], b"before", None).unwrap();
        apply_quarantine_plan(&mut graph, &first).unwrap();
        let after = serde_json::to_vec(&graph.nodes().cloned().collect::<Vec<_>>()).unwrap();
        let second = classify_legacy_completions(&graph, &[], &after, None).unwrap();
        assert!(second.is_noop());
    }
}
