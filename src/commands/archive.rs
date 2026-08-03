use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use worksgood::graph::{ArchivedBoundary, Node, Status, Task, WorkGraph};
use worksgood::parser::{load_graph, modify_graph};

use super::graph_path;

fn archive_path(dir: &Path) -> std::path::PathBuf {
    dir.join("archive.jsonl")
}

fn last_batch_path(dir: &Path) -> std::path::PathBuf {
    dir.join("archive-last-batch.json")
}

const AUTO_ARCHIVE_STATE_VERSION: u32 = 1;
/// A normal daemon tick may archive at most this many newly-eligible tasks.
/// Anything larger becomes an attended operation with an exact persisted plan.
pub const MAX_AUTOMATIC_INCREMENTAL_BATCH: usize = 10;
/// A daemon that has not advanced its archival watermark for this long is no
/// longer a routine incremental run. It must be acknowledged by an operator.
pub const MAX_AUTOMATIC_WATERMARK_GAP_SECS: i64 = 48 * 60 * 60;

fn automatic_state_path(dir: &Path) -> std::path::PathBuf {
    dir.join("archive-auto-state.json")
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutomaticArchivePlanTask {
    pub id: String,
    /// Digest of the complete task record. Confirmation refuses if the task
    /// changed after the dry-run was persisted.
    pub digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutomaticArchivePending {
    pub created_at: String,
    pub evaluated_cutoff: String,
    pub retention_days: u64,
    pub build_id: String,
    pub reason: String,
    pub tasks: Vec<AutomaticArchivePlanTask>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct AutomaticArchiveReceipt {
    confirmed_at: String,
    cutoff: String,
    retention_days: u64,
    build_id: String,
    task_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct AutomaticArchiveState {
    version: u32,
    retention_days: u64,
    #[serde(default)]
    last_confirmed_cutoff: Option<String>,
    #[serde(default)]
    acknowledged_build_id: Option<String>,
    #[serde(default)]
    pending: Option<AutomaticArchivePending>,
    #[serde(default)]
    last_receipt: Option<AutomaticArchiveReceipt>,
}

impl AutomaticArchiveState {
    fn disabled() -> Self {
        Self {
            version: AUTO_ARCHIVE_STATE_VERSION,
            retention_days: 0,
            last_confirmed_cutoff: None,
            acknowledged_build_id: None,
            pending: None,
            last_receipt: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct AutomaticArchiveStatus {
    pub enabled: bool,
    pub retention_days: u64,
    pub pending: bool,
    pub pending_count: usize,
    pub reason: Option<String>,
    pub task_ids: Vec<String>,
    pub dry_run_command: Option<String>,
    pub confirm_command: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AutomaticArchiveOutcome {
    Disabled,
    Held { count: usize, reason: String },
    Archived { count: usize },
    Advanced,
}

fn load_automatic_state(dir: &Path) -> Result<Option<AutomaticArchiveState>> {
    let path = automatic_state_path(dir);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path).with_context(|| {
        format!(
            "Failed to read automatic archival state: {}",
            path.display()
        )
    })?;
    let state: AutomaticArchiveState = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "Failed to parse automatic archival state: {}",
            path.display()
        )
    })?;
    if state.version != AUTO_ARCHIVE_STATE_VERSION {
        anyhow::bail!(
            "Unsupported automatic archival state version {} (expected {})",
            state.version,
            AUTO_ARCHIVE_STATE_VERSION
        );
    }
    Ok(Some(state))
}

fn save_automatic_state(dir: &Path, state: &AutomaticArchiveState) -> Result<()> {
    let path = automatic_state_path(dir);
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(state)?;
    std::fs::write(&tmp, bytes).with_context(|| {
        format!(
            "Failed to write automatic archival state: {}",
            tmp.display()
        )
    })?;
    std::fs::rename(&tmp, &path).with_context(|| {
        format!(
            "Failed to install automatic archival state: {}",
            path.display()
        )
    })?;
    Ok(())
}

fn task_digest(task: &Task) -> Result<String> {
    let bytes = serde_json::to_vec(&Node::Task(task.clone()))?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(bytes))))
}

fn task_timestamp(task: &Task) -> Option<DateTime<Utc>> {
    task.completed_at
        .as_deref()
        .or(task.created_at.as_deref())
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

pub(crate) fn current_build_id() -> Result<String> {
    let executable = std::env::current_exe().context("Failed to locate current wg executable")?;
    let digest = worksgood::service_identity::executable_sha256(&executable)?;
    Ok(worksgood::service_identity::build_id(&digest))
}

/// Store batch metadata so we can undo the last archive operation
fn save_batch_metadata(dir: &Path, task_ids: &[String]) -> Result<()> {
    let metadata = serde_json::json!({
        "timestamp": Utc::now().to_rfc3339(),
        "task_ids": task_ids,
    });
    let path = last_batch_path(dir);
    let content = serde_json::to_string_pretty(&metadata)?;
    std::fs::write(&path, content)
        .with_context(|| format!("Failed to write batch metadata to {:?}", path))?;
    Ok(())
}

/// Load the last batch metadata for undo
fn load_batch_metadata(dir: &Path) -> Result<Vec<String>> {
    let path = last_batch_path(dir);
    if !path.exists() {
        anyhow::bail!("No archive batch to undo. No previous archive operation found.");
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read batch metadata from {:?}", path))?;
    let metadata: serde_json::Value =
        serde_json::from_str(&content).with_context(|| "Failed to parse batch metadata")?;
    let task_ids = metadata["task_ids"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Invalid batch metadata: missing task_ids"))?
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    Ok(task_ids)
}

/// Parse a duration string like "30d", "7d", "1w" into a chrono Duration
pub fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("Empty duration string");
    }

    let (num_str, unit) = if let Some(n) = s.strip_suffix('d') {
        (n, 'd')
    } else if let Some(n) = s.strip_suffix('w') {
        (n, 'w')
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 'h')
    } else {
        // Default to days if no unit specified
        (s, 'd')
    };

    let num: i64 = num_str
        .parse()
        .with_context(|| format!("Invalid number in duration: '{}'", num_str))?;

    match unit {
        'd' => Ok(Duration::days(num)),
        'w' => Ok(Duration::weeks(num)),
        'h' => Ok(Duration::hours(num)),
        _ => anyhow::bail!("Unknown duration unit: {}", unit),
    }
}

/// Check whether a task has authoritative terminal evidence for archival.
///
/// Abandonment is explicitly non-success terminal history.  A successful row
/// is archivable only when its compatibility `Done` projection is backed by a
/// v2 GraphSave lifecycle receipt; raw/legacy Done must stay active for the
/// reconciliation adapter and must never be hidden behind an archive boundary.
fn has_archivable_terminal_evidence(task: &Task) -> bool {
    task.status == Status::Abandoned
        || (task.status == Status::Done && task.graph_save_completion_disposition().is_some())
}

/// Check if a task should be archived based on the --older filter.
fn should_archive(task: &Task, older_than: Option<&Duration>) -> bool {
    if !has_archivable_terminal_evidence(task) {
        return false;
    }

    if let Some(min_age) = older_than {
        // Use completed_at for Done tasks, or created_at as fallback for Abandoned
        let timestamp = task.completed_at.as_deref().or(task.created_at.as_deref());
        if let Some(ts) = timestamp
            && let Ok(parsed) = DateTime::parse_from_rfc3339(ts)
        {
            let age = Utc::now().signed_duration_since(parsed);
            return age > *min_age;
        }
        // If no timestamp or can't parse, don't archive with --older filter
        return false;
    }

    true
}

/// Build the compact active-view marker for an archived task.
///
/// Successors are derived from both sides of the stored edge because old graph
/// files may have only canonical `after` edges or a stale/missing `before`
/// cache. The archived `Task` itself remains byte-for-byte complete in
/// `archive.jsonl`; this record exists only to preserve readiness and render an
/// honest cut edge in the induced active view.
fn archived_boundary_for(task: &Task, graph: &worksgood::graph::WorkGraph) -> ArchivedBoundary {
    let mut successors = task.before.clone();
    successors.extend(
        graph
            .tasks()
            .filter(|candidate| candidate.after.contains(&task.id))
            .map(|candidate| candidate.id.clone()),
    );
    successors.sort();
    successors.dedup();
    ArchivedBoundary {
        id: task.id.clone(),
        title: task.title.clone(),
        status: task.status,
        predecessors: task.after.clone(),
        successors,
        archived_at: Utc::now().to_rfc3339(),
    }
}

/// Append tasks to the archive file.
///
/// Existing identical records are accepted so a crash between the append and
/// graph replacement can be retried safely. A conflicting record with the same
/// ID fails closed rather than silently producing two histories.
fn append_to_archive(tasks: &[Task], archive_path: &Path) -> Result<()> {
    let existing = load_archive(archive_path)?;
    let mut to_append = Vec::new();
    for task in tasks {
        if let Some(prior) = existing.iter().find(|candidate| candidate.id == task.id) {
            if task_digest(prior)? != task_digest(task)? {
                anyhow::bail!(
                    "Archive already contains a different record for task '{}'; refusing to overwrite history",
                    task.id
                );
            }
        } else {
            to_append.push(task);
        }
    }
    if to_append.is_empty() {
        return Ok(());
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(archive_path)
        .with_context(|| format!("Failed to open archive file: {:?}", archive_path))?;

    for task in to_append {
        let node = Node::Task(task.clone());
        let json = serde_json::to_string(&node)
            .with_context(|| format!("Failed to serialize task: {}", task.id))?;
        writeln!(file, "{}", json)?;
    }

    Ok(())
}

/// Load archived tasks from the archive file
fn load_archive(archive_path: &Path) -> Result<Vec<Task>> {
    if !archive_path.exists() {
        return Ok(Vec::new());
    }

    let file = File::open(archive_path)
        .with_context(|| format!("Failed to open archive file: {:?}", archive_path))?;
    let reader = BufReader::new(file);
    let mut tasks = Vec::new();

    for (line_num, line) in reader.lines().enumerate() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let node: Node = serde_json::from_str(trimmed).with_context(|| {
            format!("Failed to parse archive line {}: {}", line_num + 1, trimmed)
        })?;
        if let Node::Task(task) = node {
            tasks.push(task);
        }
    }

    Ok(tasks)
}

/// Rewrite the archive file, excluding a specific task by ID.
fn remove_from_archive(archive_path: &Path, task_id: &str) -> Result<()> {
    let tasks = load_archive(archive_path)?;
    // Rewrite the file with all tasks except the one being restored
    let file = File::create(archive_path).with_context(|| {
        format!(
            "Failed to open archive file for writing: {:?}",
            archive_path
        )
    })?;
    let mut writer = std::io::BufWriter::new(file);
    for task in &tasks {
        if task.id != task_id {
            let node = Node::Task(task.clone());
            let json = serde_json::to_string(&node)
                .with_context(|| format!("Failed to serialize task: {}", task.id))?;
            writeln!(writer, "{}", json)?;
        }
    }
    Ok(())
}

/// Search archived tasks by title, description, and tags.
pub fn search(dir: &Path, query: &str, limit: usize, json: bool) -> Result<()> {
    let arch_path = archive_path(dir);
    let tasks = load_archive(&arch_path)?;
    let query_lower = query.to_lowercase();

    let matches: Vec<&Task> = tasks
        .iter()
        .filter(|t| {
            t.title.to_lowercase().contains(&query_lower)
                || t.description
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&query_lower)
                || t.tags
                    .iter()
                    .any(|tag| tag.to_lowercase().contains(&query_lower))
        })
        .take(limit)
        .collect();

    if json {
        let items: Vec<serde_json::Value> = matches
            .iter()
            .map(|t| {
                serde_json::json!({
                    "id": t.id,
                    "title": t.title,
                    "completed_at": t.completed_at,
                    "tags": t.tags,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else if matches.is_empty() {
        println!("No archived tasks matching '{}'.", query);
    } else {
        println!(
            "Archived tasks matching '{}' ({} result{}):",
            query,
            matches.len(),
            if matches.len() == 1 { "" } else { "s" }
        );
        for task in &matches {
            let completed = task.completed_at.as_deref().unwrap_or("unknown");
            let tags = if task.tags.is_empty() {
                String::new()
            } else {
                format!(" [{}]", task.tags.join(", "))
            };
            println!(
                "  {} - {} (completed: {}){}",
                task.id, task.title, completed, tags
            );
        }
    }

    Ok(())
}

/// Restore an archived task back into the active graph.
pub fn restore(dir: &Path, task_id: &str, reopen: bool) -> Result<()> {
    let path = graph_path(dir);
    let arch_path = archive_path(dir);

    if !path.exists() {
        anyhow::bail!("WG not initialized. Run 'wg init' first.");
    }

    let tasks = load_archive(&arch_path)?;
    let task = tasks
        .iter()
        .find(|t| t.id == task_id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Task '{}' not found in archive", task_id))?;

    let mut restored_task = task;
    if reopen {
        super::reopen::request(
            &mut restored_task,
            "archive-restore",
            false,
            true,
            "archive restore reopen",
            worksgood::lifecycle::LifecycleActor::operator(worksgood::current_user()),
            "archive_restore_reopen",
        )
        .map_err(anyhow::Error::new)?;
    }

    // Add back to graph atomically
    let restored_task_clone = restored_task.clone();
    let mut error: Option<anyhow::Error> = None;
    modify_graph(&path, |graph| {
        if graph.get_task(&restored_task_clone.id).is_some() {
            error = Some(anyhow::anyhow!(
                "Task '{}' already exists in the active graph",
                restored_task_clone.id
            ));
            return false;
        }
        graph.add_node(Node::Task(restored_task_clone.clone()));
        true
    })
    .context("Failed to modify graph")?;
    if let Some(e) = error {
        return Err(e);
    }

    // Remove from archive
    remove_from_archive(&arch_path, task_id)?;

    super::notify_graph_changed(dir);
    if reopen {
        let _ = super::reopen::reconcile_pending(dir)?;
    }

    let status = if reopen { "open" } else { "done" };
    println!(
        "Restored task '{}' ({}) to active graph with status '{}'",
        task_id, restored_task.title, status
    );

    Ok(())
}

/// Undo the last archive operation by restoring all tasks from the last batch.
pub fn undo(dir: &Path) -> Result<()> {
    let path = graph_path(dir);
    let arch_path = archive_path(dir);

    if !path.exists() {
        anyhow::bail!("WG not initialized. Run 'wg init' first.");
    }

    let task_ids = load_batch_metadata(dir)?;
    if task_ids.is_empty() {
        anyhow::bail!("No tasks in the last archive batch to restore.");
    }

    let archived_tasks = load_archive(&arch_path)?;

    let mut restored_count = 0;
    let mut skipped = Vec::new();

    // Pre-compute which tasks to restore (need archive removal outside closure)
    let mut to_restore: Vec<(String, Task)> = Vec::new();
    for task_id in &task_ids {
        if let Some(task) = archived_tasks.iter().find(|t| &t.id == task_id) {
            to_restore.push((task_id.clone(), task.clone()));
        } else {
            skipped.push(task_id.clone());
        }
    }

    modify_graph(&path, |graph| {
        let mut changed = false;
        for (task_id, task) in &to_restore {
            if graph.get_task(task_id).is_some() {
                skipped.push(task_id.clone());
                continue;
            }
            graph.add_node(Node::Task(task.clone()));
            restored_count += 1;
            changed = true;
        }
        changed
    })
    .context("Failed to modify graph")?;

    // Remove restored tasks from archive
    for (task_id, _) in &to_restore {
        if !skipped.contains(task_id) {
            remove_from_archive(&arch_path, task_id)?;
        }
    }
    super::notify_graph_changed(dir);

    // Remove the batch metadata file since undo is done
    let batch_path = last_batch_path(dir);
    if batch_path.exists() {
        std::fs::remove_file(&batch_path).ok();
    }

    println!("Restored {} tasks from last archive batch.", restored_count);
    if !skipped.is_empty() {
        println!(
            "Skipped {} tasks (not found in archive or already in graph): {}",
            skipped.len(),
            skipped.join(", ")
        );
    }

    Ok(())
}

pub fn run(
    dir: &Path,
    dry_run: bool,
    older: Option<&str>,
    list: bool,
    yes: bool,
    ids: &[String],
    json: bool,
) -> Result<()> {
    let path = graph_path(dir);
    let arch_path = archive_path(dir);

    if !path.exists() {
        anyhow::bail!("WG not initialized. Run 'wg init' first.");
    }

    // Handle --list: show archived tasks
    if list {
        let tasks = load_archive(&arch_path)?;
        if json {
            let items: Vec<serde_json::Value> = tasks
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "id": t.id,
                        "title": t.title,
                        "completed_at": t.completed_at,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&items)?);
        } else if tasks.is_empty() {
            println!("No archived tasks.");
        } else {
            println!("Archived tasks ({}):", tasks.len());
            for task in &tasks {
                let completed = task.completed_at.as_deref().unwrap_or("unknown");
                println!("  {} - {} (completed: {})", task.id, task.title, completed);
            }
        }
        return Ok(());
    }

    // Parse --older duration if provided
    let older_duration = if let Some(older_str) = older {
        Some(parse_duration(older_str)?)
    } else {
        None
    };

    let graph = load_graph(&path).context("Failed to load graph")?;

    // Find tasks to archive
    let tasks_to_archive: Vec<Task> = if !ids.is_empty() {
        // Archive specific tasks by ID
        let mut tasks = Vec::new();
        for id in ids {
            if let Some(task) = graph.get_task(id) {
                if !has_archivable_terminal_evidence(task) {
                    anyhow::bail!(
                        "Task '{}' is not archivable: status '{}' lacks a verified v2 GraphSave (or explicit abandonment); reconcile retained evidence first.",
                        id,
                        task.status,
                    );
                }
                tasks.push(task.clone());
            } else {
                anyhow::bail!("Task '{}' not found in the graph.", id);
            }
        }
        tasks
    } else {
        graph
            .tasks()
            .filter(|t| should_archive(t, older_duration.as_ref()))
            .cloned()
            .collect()
    };

    if tasks_to_archive.is_empty() {
        println!("No tasks to archive.");
        return Ok(());
    }

    if dry_run {
        println!("Would archive {} tasks:", tasks_to_archive.len());
        for task in &tasks_to_archive {
            let completed = task.completed_at.as_deref().unwrap_or("unknown");
            println!("  {} - {} (completed: {})", task.id, task.title, completed);
        }
        return Ok(());
    }

    // For bulk operations (no explicit IDs), require --yes confirmation
    let is_bulk = ids.is_empty();
    if is_bulk && !yes {
        println!("Would archive {} tasks:", tasks_to_archive.len());
        for task in &tasks_to_archive {
            let completed = task.completed_at.as_deref().unwrap_or("unknown");
            println!("  {} - {} (completed: {})", task.id, task.title, completed);
        }
        println!();
        anyhow::bail!(
            "Use --yes to confirm, or specify task IDs explicitly: wg archive <id1> <id2> ..."
        );
    }

    // Perform the archive operation
    // 1. Append tasks to archive file
    append_to_archive(&tasks_to_archive, &arch_path)?;

    // 2. Save batch metadata for undo
    let archived_ids: Vec<String> = tasks_to_archive.iter().map(|t| t.id.clone()).collect();
    save_batch_metadata(dir, &archived_ids)?;

    // 3. Replace archived tasks with compact boundary markers atomically.
    // Never call `remove_node` here: it rewrites adjacent `after`/`before`
    // lists, which would erase the historical cut we need to restore and show.
    let boundaries: Vec<ArchivedBoundary> = tasks_to_archive
        .iter()
        .map(|task| archived_boundary_for(task, &graph))
        .collect();
    modify_graph(&path, |graph| {
        for boundary in &boundaries {
            graph.take_node(&boundary.id);
            graph.add_node(Node::ArchivedBoundary(boundary.clone()));
        }
        true
    })
    .context("Failed to modify graph")?;
    super::notify_graph_changed(dir);

    // Record operation
    let config = worksgood::config::Config::load_or_default(dir);
    let task_ids: Vec<&str> = tasks_to_archive.iter().map(|t| t.id.as_str()).collect();
    let _ = worksgood::provenance::record(
        dir,
        "archive",
        None,
        None,
        serde_json::json!({ "task_ids": task_ids }),
        config.log.rotation_threshold,
    );

    println!(
        "Archived {} tasks. Use `wg archive --undo` to reverse.",
        tasks_to_archive.len(),
    );

    Ok(())
}

fn archive_automatic_batch(
    dir: &Path,
    graph: &WorkGraph,
    tasks_to_archive: &[Task],
    retention_days: u64,
) -> Result<usize> {
    if tasks_to_archive.is_empty() {
        return Ok(0);
    }
    let path = graph_path(dir);
    let arch_path = archive_path(dir);

    // The append is idempotent for an identical record, making a retry after a
    // crash safe. Batch metadata is installed before the graph replacement so
    // the exact records always have an undo receipt.
    append_to_archive(tasks_to_archive, &arch_path)?;
    let archived_ids: Vec<String> = tasks_to_archive
        .iter()
        .map(|task| task.id.clone())
        .collect();
    save_batch_metadata(dir, &archived_ids)?;

    let boundaries: Vec<ArchivedBoundary> = tasks_to_archive
        .iter()
        .map(|task| archived_boundary_for(task, graph))
        .collect();
    modify_graph(&path, |active| {
        for boundary in &boundaries {
            active.take_node(&boundary.id);
            active.add_node(Node::ArchivedBoundary(boundary.clone()));
        }
        true
    })
    .context("Failed to modify graph")?;
    super::notify_graph_changed(dir);

    let config = worksgood::config::Config::load_or_default(dir);
    let task_ids: Vec<&str> = tasks_to_archive
        .iter()
        .map(|task| task.id.as_str())
        .collect();
    let _ = worksgood::provenance::record(
        dir,
        "archive",
        None,
        None,
        serde_json::json!({
            "task_ids": task_ids,
            "automatic": true,
            "retention_days": retention_days,
        }),
        config.log.rotation_threshold,
    );

    Ok(tasks_to_archive.len())
}

fn eligible_automatic_tasks(
    graph: &WorkGraph,
    lower_exclusive: Option<DateTime<Utc>>,
    cutoff: DateTime<Utc>,
) -> Vec<Task> {
    let mut tasks: Vec<Task> = graph
        .tasks()
        .filter(|task| has_archivable_terminal_evidence(task))
        .filter(|task| !task.id.starts_with('.'))
        .filter(|task| {
            task_timestamp(task).is_some_and(|timestamp| {
                timestamp <= cutoff && lower_exclusive.is_none_or(|lower| timestamp > lower)
            })
        })
        .cloned()
        .collect();
    tasks.sort_by(|left, right| left.id.cmp(&right.id));
    tasks
}

fn pending_plan(
    tasks: &[Task],
    retention_days: u64,
    build_id: &str,
    cutoff: DateTime<Utc>,
    reason: impl Into<String>,
    now: DateTime<Utc>,
) -> Result<AutomaticArchivePending> {
    let tasks = tasks
        .iter()
        .map(|task| {
            Ok(AutomaticArchivePlanTask {
                id: task.id.clone(),
                digest: task_digest(task)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(AutomaticArchivePending {
        created_at: now.to_rfc3339(),
        evaluated_cutoff: cutoff.to_rfc3339(),
        retention_days,
        build_id: build_id.to_string(),
        reason: reason.into(),
        tasks,
    })
}

fn persist_hold(
    dir: &Path,
    mut state: AutomaticArchiveState,
    pending: AutomaticArchivePending,
) -> Result<AutomaticArchiveOutcome> {
    let count = pending.tasks.len();
    let reason = pending.reason.clone();
    state.retention_days = pending.retention_days;
    state.pending = Some(pending);
    save_automatic_state(dir, &state)?;
    Ok(AutomaticArchiveOutcome::Held { count, reason })
}

/// Recompute a held plan without performing any archival. This is used when a
/// new build inherits an older build's pending plan and when the operator asks
/// for a fresh dry-run after task bytes changed.
fn refresh_pending_plan_at(
    dir: &Path,
    retention_days: u64,
    build_id: &str,
    now: DateTime<Utc>,
    reason_override: Option<String>,
) -> Result<AutomaticArchiveOutcome> {
    let graph = load_graph(graph_path(dir)).context("Failed to load graph")?;
    let cutoff = now - Duration::days(retention_days as i64);
    let state = load_automatic_state(dir)?;
    let mut state = state.unwrap_or(AutomaticArchiveState {
        version: AUTO_ARCHIVE_STATE_VERSION,
        retention_days,
        last_confirmed_cutoff: None,
        acknowledged_build_id: None,
        pending: None,
        last_receipt: None,
    });
    let last_cutoff = state
        .last_confirmed_cutoff
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc));
    let tasks = match last_cutoff {
        Some(lower) if cutoff > lower => eligible_automatic_tasks(&graph, Some(lower), cutoff),
        Some(_) => Vec::new(),
        None => eligible_automatic_tasks(&graph, None, cutoff),
    };
    let reason = reason_override
        .or_else(|| state.pending.as_ref().map(|pending| pending.reason.clone()))
        .unwrap_or_else(|| {
            "no persisted automatic archival acknowledgment (legacy/unverified startup)".to_string()
        });
    let pending = pending_plan(&tasks, retention_days, build_id, cutoff, reason, now)?;
    // Do not let stale pending metadata supply dispatch authority after the
    // refresh. Only the last confirmed receipt remains authoritative.
    state.pending = None;
    persist_hold(dir, state, pending)
}

/// Read the operator-facing automatic archival state. A malformed receipt is
/// reported as a hold rather than being ignored; no caller may infer permission
/// to archive from unreadable state.
pub fn automatic_status(dir: &Path) -> AutomaticArchiveStatus {
    let retention_days = worksgood::config::Config::load_or_default(dir)
        .coordinator
        .archive_retention_days;
    if retention_days == 0 {
        return AutomaticArchiveStatus {
            enabled: false,
            retention_days,
            pending: false,
            pending_count: 0,
            reason: None,
            task_ids: Vec::new(),
            dry_run_command: None,
            confirm_command: None,
        };
    }
    match load_automatic_state(dir) {
        Ok(Some(state)) => match state.pending {
            Some(pending) => AutomaticArchiveStatus {
                enabled: true,
                retention_days,
                pending: true,
                pending_count: pending.tasks.len(),
                reason: Some(pending.reason),
                task_ids: pending.tasks.into_iter().map(|task| task.id).collect(),
                dry_run_command: Some("wg archive auto --dry-run".to_string()),
                confirm_command: Some("wg archive auto --confirm".to_string()),
            },
            None => AutomaticArchiveStatus {
                enabled: true,
                retention_days,
                pending: false,
                pending_count: 0,
                reason: None,
                task_ids: Vec::new(),
                dry_run_command: None,
                confirm_command: None,
            },
        },
        Ok(None) => AutomaticArchiveStatus {
            enabled: true,
            retention_days,
            pending: true,
            pending_count: 0,
            reason: Some(
                "no persisted automatic archival acknowledgment; awaiting first daemon maintenance pass"
                    .to_string(),
            ),
            task_ids: Vec::new(),
            dry_run_command: Some("wg archive auto --dry-run".to_string()),
            confirm_command: Some("wg archive auto --confirm".to_string()),
        },
        Err(error) => AutomaticArchiveStatus {
            enabled: true,
            retention_days,
            pending: true,
            pending_count: 0,
            reason: Some(format!("automatic archival state is unreadable: {error:#}")),
            task_ids: Vec::new(),
            dry_run_command: Some("wg archive auto --dry-run".to_string()),
            confirm_command: None,
        },
    }
}

/// Guarded daemon archival. A first run, build change, retention change, long
/// watermark gap, or oversized batch persists an exact dry-run and stops. Only
/// an acknowledged, small, recent interval may archive without attendance.
pub fn run_automatic(
    dir: &Path,
    retention_days: u64,
    build_id: &str,
) -> Result<AutomaticArchiveOutcome> {
    run_automatic_at(dir, retention_days, build_id, Utc::now())
}

fn run_automatic_at(
    dir: &Path,
    retention_days: u64,
    build_id: &str,
    now: DateTime<Utc>,
) -> Result<AutomaticArchiveOutcome> {
    if retention_days == 0 {
        let mut state = load_automatic_state(dir)?.unwrap_or_else(AutomaticArchiveState::disabled);
        if state.retention_days != 0 || state.pending.is_some() {
            state.retention_days = 0;
            state.pending = None;
            save_automatic_state(dir, &state)?;
        }
        return Ok(AutomaticArchiveOutcome::Disabled);
    }

    let path = graph_path(dir);
    if !path.exists() {
        return Ok(AutomaticArchiveOutcome::Advanced);
    }
    let graph = load_graph(&path).context("Failed to load graph")?;
    let cutoff = now - Duration::days(retention_days as i64);
    let state = load_automatic_state(dir)?;

    let Some(mut state) = state else {
        let tasks = eligible_automatic_tasks(&graph, None, cutoff);
        let pending = pending_plan(
            &tasks,
            retention_days,
            build_id,
            cutoff,
            "no persisted automatic archival acknowledgment (legacy/unverified startup)",
            now,
        )?;
        return persist_hold(
            dir,
            AutomaticArchiveState {
                version: AUTO_ARCHIVE_STATE_VERSION,
                retention_days,
                last_confirmed_cutoff: None,
                acknowledged_build_id: None,
                pending: None,
                last_receipt: None,
            },
            pending,
        );
    };

    if let Some(pending) = state.pending.as_ref() {
        if pending.build_id != build_id || pending.retention_days != retention_days {
            let reason = if pending.retention_days != retention_days {
                format!(
                    "retention changed from {}d to {}d while archival was held",
                    pending.retention_days, retention_days
                )
            } else {
                format!(
                    "daemon build changed from {} to {} while archival was held",
                    pending.build_id, build_id
                )
            };
            return refresh_pending_plan_at(dir, retention_days, build_id, now, Some(reason));
        }
        return Ok(AutomaticArchiveOutcome::Held {
            count: pending.tasks.len(),
            reason: pending.reason.clone(),
        });
    }

    let Some(last_cutoff) = state
        .last_confirmed_cutoff
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
    else {
        let tasks = eligible_automatic_tasks(&graph, None, cutoff);
        let pending = pending_plan(
            &tasks,
            retention_days,
            build_id,
            cutoff,
            "automatic archival receipt has no verified watermark",
            now,
        )?;
        return persist_hold(dir, state, pending);
    };

    let tasks = if cutoff > last_cutoff {
        eligible_automatic_tasks(&graph, Some(last_cutoff), cutoff)
    } else {
        Vec::new()
    };

    // Configuration/build identity changes require acknowledgment even when a
    // clock correction or longer retention moved the eligibility cutoff back.
    // The exact plan is empty in that case; confirmation only re-baselines the
    // trusted watermark and cannot resurrect pre-watermark history.
    let reason = if state.retention_days != retention_days {
        Some(format!(
            "retention changed from {}d to {}d",
            state.retention_days, retention_days
        ))
    } else if state.acknowledged_build_id.as_deref() != Some(build_id) {
        Some(format!(
            "daemon build changed from {} to {}",
            state
                .acknowledged_build_id
                .as_deref()
                .unwrap_or("unverified"),
            build_id
        ))
    } else {
        None
    };
    if let Some(reason) = reason {
        let pending = pending_plan(&tasks, retention_days, build_id, cutoff, reason, now)?;
        return persist_hold(dir, state, pending);
    }

    // Never move a watermark backwards after an ordinary clock correction.
    if cutoff <= last_cutoff {
        return Ok(AutomaticArchiveOutcome::Advanced);
    }

    let reason = if cutoff.signed_duration_since(last_cutoff).num_seconds()
        > MAX_AUTOMATIC_WATERMARK_GAP_SECS
    {
        Some(format!(
            "archival watermark is overdue by more than {} hours",
            MAX_AUTOMATIC_WATERMARK_GAP_SECS / 3600
        ))
    } else if tasks.len() > MAX_AUTOMATIC_INCREMENTAL_BATCH {
        Some(format!(
            "{} newly eligible tasks exceeds the unattended limit of {}",
            tasks.len(),
            MAX_AUTOMATIC_INCREMENTAL_BATCH
        ))
    } else {
        None
    };

    if let Some(reason) = reason {
        let pending = pending_plan(&tasks, retention_days, build_id, cutoff, reason, now)?;
        return persist_hold(dir, state, pending);
    }

    let count = archive_automatic_batch(dir, &graph, &tasks, retention_days)?;
    state.retention_days = retention_days;
    state.last_confirmed_cutoff = Some(cutoff.to_rfc3339());
    state.acknowledged_build_id = Some(build_id.to_string());
    state.last_receipt = Some(AutomaticArchiveReceipt {
        confirmed_at: now.to_rfc3339(),
        cutoff: cutoff.to_rfc3339(),
        retention_days,
        build_id: build_id.to_string(),
        task_ids: tasks.iter().map(|task| task.id.clone()).collect(),
    });
    save_automatic_state(dir, &state)?;
    if count == 0 {
        Ok(AutomaticArchiveOutcome::Advanced)
    } else {
        Ok(AutomaticArchiveOutcome::Archived { count })
    }
}

fn confirm_automatic_at(dir: &Path, build_id: &str, now: DateTime<Utc>) -> Result<Vec<String>> {
    let config = worksgood::config::Config::load_or_default(dir);
    let retention_days = config.coordinator.archive_retention_days;
    if retention_days == 0 {
        anyhow::bail!(
            "Automatic archival is disabled (coordinator.archive_retention_days=0); nothing to confirm"
        );
    }
    let mut state = load_automatic_state(dir)?
        .ok_or_else(|| anyhow::anyhow!("No persisted automatic archival plan. Start the daemon or run `wg archive auto --dry-run` first."))?;
    let pending = state.pending.clone().ok_or_else(|| {
        anyhow::anyhow!("No automatic archival is pending operator confirmation.")
    })?;
    if pending.retention_days != retention_days {
        anyhow::bail!(
            "Pending plan uses {}d retention but current config uses {}d; run `wg archive auto --dry-run` to create a new exact plan",
            pending.retention_days,
            retention_days
        );
    }
    if pending.build_id != build_id {
        anyhow::bail!(
            "Pending plan was created by build '{}' but this command is '{}'; restart the daemon and review a plan from the candidate binary",
            pending.build_id,
            build_id
        );
    }

    let graph = load_graph(graph_path(dir)).context("Failed to load graph")?;
    let archived = load_archive(&archive_path(dir))?;
    let mut active_tasks = Vec::new();
    for planned in &pending.tasks {
        if let Some(task) = graph.get_task(&planned.id) {
            let actual = task_digest(task)?;
            if actual != planned.digest {
                anyhow::bail!(
                    "Task '{}' changed after the dry-run (expected {}, found {}); refusing stale confirmation",
                    planned.id,
                    planned.digest,
                    actual
                );
            }
            active_tasks.push(task.clone());
        } else if let Some(task) = archived.iter().find(|task| task.id == planned.id) {
            if task_digest(task)? != planned.digest {
                anyhow::bail!(
                    "Archived task '{}' does not match the confirmed plan; refusing to finalize receipt",
                    planned.id
                );
            }
        } else {
            anyhow::bail!(
                "Task '{}' from the confirmed plan is neither active nor archived; refusing partial archival",
                planned.id
            );
        }
    }

    archive_automatic_batch(dir, &graph, &active_tasks, retention_days)?;
    let confirmed_ids: Vec<String> = pending.tasks.iter().map(|task| task.id.clone()).collect();
    // If a previous attempt crashed after archiving only part of the plan, make
    // the undo receipt cover the complete exact confirmation batch.
    if !confirmed_ids.is_empty() {
        save_batch_metadata(dir, &confirmed_ids)?;
    }
    state.retention_days = retention_days;
    state.last_confirmed_cutoff = Some(pending.evaluated_cutoff.clone());
    state.acknowledged_build_id = Some(build_id.to_string());
    state.pending = None;
    state.last_receipt = Some(AutomaticArchiveReceipt {
        confirmed_at: now.to_rfc3339(),
        cutoff: pending.evaluated_cutoff,
        retention_days,
        build_id: build_id.to_string(),
        task_ids: confirmed_ids.clone(),
    });
    save_automatic_state(dir, &state)?;
    Ok(confirmed_ids)
}

/// Operator surface for inspecting and confirming the exact held batch.
pub fn run_auto_control(dir: &Path, dry_run: bool, confirm: bool, json: bool) -> Result<()> {
    if confirm {
        let build_id = current_build_id()?;
        let ids = confirm_automatic_at(dir, &build_id, Utc::now())?;
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "confirmed": true,
                    "archived_count": ids.len(),
                    "task_ids": ids,
                    "undo_command": "wg archive --undo",
                }))?
            );
        } else {
            println!("Confirmed automatic archival of {} tasks.", ids.len());
            for id in &ids {
                println!("  {id}");
            }
            if !ids.is_empty() {
                println!("Undo this exact batch with: wg archive --undo");
            }
        }
        return Ok(());
    }

    // Make the dry-run independently actionable before the daemon's first
    // pass, after an upgrade, and after task bytes changed. Refreshing a held
    // plan is always non-destructive; acknowledged incremental mode is left
    // untouched when no hold exists.
    let retention_days = worksgood::config::Config::load_or_default(dir)
        .coordinator
        .archive_retention_days;
    if retention_days == 0 {
        // A dry-run after disabling is also an immediate, non-destructive
        // cancellation point for an older held plan. The daemon performs the
        // same transition on reload/tick, but the attended control surface
        // should not leave stale pending authority on disk while reporting
        // that archival is disabled.
        let _ = run_automatic(dir, 0, "disabled")?;
    } else if automatic_status(dir).pending {
        let build_id = current_build_id()?;
        let _ = refresh_pending_plan_at(dir, retention_days, &build_id, Utc::now(), None)?;
    }
    let status = automatic_status(dir);
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else if !status.enabled {
        println!("Automatic archival is disabled (coordinator.archive_retention_days=0).");
    } else if status.pending {
        println!(
            "Automatic archival pending operator confirmation: {} task(s)",
            status.pending_count
        );
        if let Some(reason) = status.reason.as_deref() {
            println!("Reason: {reason}");
        }
        for id in &status.task_ids {
            println!("  {id}");
        }
        println!("Confirm this exact batch once: wg archive auto --confirm");
    } else {
        println!("Automatic archival is acknowledged; no batch is pending.");
    }
    let _ = dry_run; // The no-confirm path is always non-destructive.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use worksgood::graph::{CompletionDisposition, WorkGraph};
    use worksgood::lifecycle::{ActorKind, LifecycleEvent, LifecycleEventProjection};
    use worksgood::parser::save_graph;

    fn mark_graph_saved(task: &mut Task) {
        let receipt = format!("wgcid:v2:blake3:{:064}", 1);
        task.completion_disposition = Some(CompletionDisposition::Landed);
        task.completion_receipt = Some(receipt.clone());
        task.lifecycle.revision = 1;
        task.lifecycle.ledger_head = Some(format!("graph-save:{}", task.id));
        task.lifecycle.audit.push(LifecycleEvent {
            schema_version: 2,
            event_id: format!("graph-save:{}", task.id),
            idempotency_key: format!("graph-save:{}", task.id),
            task_id: task.id.clone(),
            task_revision: 1,
            generation: 0,
            event_kind: "graph-save-committed".into(),
            old_state: Status::InProgress,
            new_state: Status::Done,
            actor_kind: ActorKind::Reconciler,
            actor_id: "test".into(),
            attempt_id: None,
            fence: 0,
            reason_code: "test-fixture".into(),
            evidence_refs: vec![receipt],
            occurred_at: "2024-01-01T00:00:00Z".into(),
            committed_at: "2024-01-01T00:00:00Z".into(),
            projection: LifecycleEventProjection {
                status: Status::Done,
                generation: 0,
                revision: 1,
                fence: 0,
                attempt_sequence: 0,
                current_attempt: None,
                pi_process_epoch: 0,
                pi_process_identity_digest: String::new(),
                pi_continuation_epoch: 0,
                pi_continuation: None,
                pi_terminal_reservation: None,
                reopen_intent: None,
            },
        });
    }

    fn make_task(id: &str, title: &str, status: Status, completed_at: Option<&str>) -> Task {
        let mut task = Task {
            id: id.to_string(),
            title: title.to_string(),
            status,
            completed_at: completed_at.map(String::from),
            ..Task::default()
        };
        if status == Status::Done {
            mark_graph_saved(&mut task);
        }
        task
    }

    #[test]
    fn test_parse_duration_days() {
        let d = parse_duration("30d").unwrap();
        assert_eq!(d, Duration::days(30));
    }

    #[test]
    fn test_parse_duration_weeks() {
        let d = parse_duration("2w").unwrap();
        assert_eq!(d, Duration::weeks(2));
    }

    #[test]
    fn test_parse_duration_hours() {
        let d = parse_duration("24h").unwrap();
        assert_eq!(d, Duration::hours(24));
    }

    #[test]
    fn test_parse_duration_no_unit() {
        let d = parse_duration("7").unwrap();
        assert_eq!(d, Duration::days(7));
    }

    #[test]
    fn test_should_archive_done_task() {
        let task = make_task("t1", "Test", Status::Done, None);
        assert!(should_archive(&task, None));
    }

    #[test]
    fn raw_done_without_graph_save_is_not_archivable() {
        let task = Task {
            id: "legacy".into(),
            title: "Legacy raw done".into(),
            status: Status::Done,
            ..Task::default()
        };
        assert!(!should_archive(&task, None));
    }

    #[test]
    fn test_should_not_archive_open_task() {
        let task = make_task("t1", "Test", Status::Open, None);
        assert!(!should_archive(&task, None));
    }

    #[test]
    fn test_should_archive_old_task() {
        // Task completed 40 days ago
        let completed_at = (Utc::now() - Duration::days(40)).to_rfc3339();
        let task = make_task("t1", "Test", Status::Done, Some(&completed_at));
        let min_age = Duration::days(30);
        assert!(should_archive(&task, Some(&min_age)));
    }

    #[test]
    fn test_should_not_archive_recent_task() {
        // Task completed 10 days ago
        let completed_at = (Utc::now() - Duration::days(10)).to_rfc3339();
        let task = make_task("t1", "Test", Status::Done, Some(&completed_at));
        let min_age = Duration::days(30);
        assert!(!should_archive(&task, Some(&min_age)));
    }

    #[test]
    fn test_archive_roundtrip() {
        let dir = tempdir().unwrap();
        let arch_path = dir.path().join("archive.jsonl");

        let tasks = vec![
            make_task("t1", "Task 1", Status::Done, Some("2024-01-01T00:00:00Z")),
            make_task("t2", "Task 2", Status::Done, Some("2024-01-02T00:00:00Z")),
        ];

        append_to_archive(&tasks, &arch_path).unwrap();

        let loaded = load_archive(&arch_path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "t1");
        assert_eq!(loaded[1].id, "t2");
    }

    #[test]
    fn test_run_dry_run() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();

        // Create .wg directory structure
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");

        // Create a graph with one done task
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task(
            "t1",
            "Done Task",
            Status::Done,
            Some("2024-01-01T00:00:00Z"),
        )));
        graph.add_node(Node::Task(make_task("t2", "Open Task", Status::Open, None)));
        save_graph(&graph, &graph_file).unwrap();

        // Run in dry-run mode
        run(wg_dir, true, None, false, false, &[], false).unwrap();

        // Verify graph is unchanged
        let loaded = load_graph(&graph_file).unwrap();
        assert_eq!(loaded.tasks().count(), 2);

        // Verify no archive file created
        let arch_path = wg_dir.join("archive.jsonl");
        assert!(!arch_path.exists());
    }

    #[test]
    fn test_run_archive() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();

        // Create .wg directory structure
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");

        // Create a graph with one done task and one open task
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task(
            "t1",
            "Done Task",
            Status::Done,
            Some("2024-01-01T00:00:00Z"),
        )));
        graph.add_node(Node::Task(make_task("t2", "Open Task", Status::Open, None)));
        save_graph(&graph, &graph_file).unwrap();

        // Run archive (with --yes to skip confirmation)
        run(wg_dir, false, None, false, true, &[], false).unwrap();

        // Verify done task removed from graph
        let loaded = load_graph(&graph_file).unwrap();
        assert_eq!(loaded.tasks().count(), 1);
        assert!(loaded.get_task("t1").is_none());
        assert!(loaded.get_task("t2").is_some());

        // Verify done task is in archive
        let arch_path = wg_dir.join("archive.jsonl");
        let archived = load_archive(&arch_path).unwrap();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].id, "t1");
    }

    #[test]
    fn archive_prefix_and_restore_preserve_exact_edges() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");

        let mut first = make_task("first", "First", Status::Done, Some("2024-01-01T00:00:00Z"));
        first.before = vec!["middle".to_string()];
        let mut middle = make_task(
            "middle",
            "Middle",
            Status::Done,
            Some("2024-01-02T00:00:00Z"),
        );
        middle.after = vec!["first".to_string()];
        middle.before = vec!["active".to_string()];
        let mut active = make_task("active", "Active", Status::Open, None);
        active.after = vec!["middle".to_string()];
        let original_first = first.clone();
        let original_middle = middle.clone();

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(first));
        graph.add_node(Node::Task(middle));
        graph.add_node(Node::Task(active));
        save_graph(&graph, &graph_file).unwrap();

        run(
            wg_dir,
            false,
            None,
            false,
            true,
            &["first".to_string(), "middle".to_string()],
            false,
        )
        .unwrap();
        let graph = load_graph(&graph_file).unwrap();
        assert!(graph.get_archived_boundary("first").is_some());
        assert!(graph.get_archived_boundary("middle").is_some());
        assert_eq!(graph.get_task("active").unwrap().after, vec!["middle"]);
        assert!(
            worksgood::query::ready_tasks(&graph).is_empty(),
            "the current ArchivedBoundary schema cannot preserve GraphSave proof, so the edge must fail closed"
        );

        restore(wg_dir, "middle", false).unwrap();
        restore(wg_dir, "first", false).unwrap();
        let graph = load_graph(&graph_file).unwrap();
        let restored_first = graph.get_task("first").unwrap();
        let restored_middle = graph.get_task("middle").unwrap();
        assert_eq!(restored_first.after, original_first.after);
        assert_eq!(restored_first.before, original_first.before);
        assert_eq!(restored_middle.after, original_middle.after);
        assert_eq!(restored_middle.before, original_middle.before);
        assert_eq!(graph.get_task("active").unwrap().after, vec!["middle"]);
    }

    #[test]
    fn test_run_list() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();

        // Create .wg directory structure
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");
        let arch_path = wg_dir.join("archive.jsonl");

        // Create empty graph
        let graph = WorkGraph::new();
        save_graph(&graph, &graph_file).unwrap();

        // Create archive with some tasks
        let tasks = vec![make_task(
            "t1",
            "Archived Task",
            Status::Done,
            Some("2024-01-01T00:00:00Z"),
        )];
        append_to_archive(&tasks, &arch_path).unwrap();

        // Run list - should not error
        run(wg_dir, false, None, true, false, &[], false).unwrap();
    }

    #[test]
    fn test_run_list_json() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();

        // Create .wg directory structure
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");
        let arch_path = wg_dir.join("archive.jsonl");

        // Create empty graph
        let graph = WorkGraph::new();
        save_graph(&graph, &graph_file).unwrap();

        // Create archive with some tasks
        let tasks = vec![
            make_task(
                "t1",
                "First Archived",
                Status::Done,
                Some("2024-01-01T00:00:00Z"),
            ),
            make_task(
                "t2",
                "Second Archived",
                Status::Done,
                Some("2024-02-15T12:00:00Z"),
            ),
        ];
        append_to_archive(&tasks, &arch_path).unwrap();

        // Run list with json=true (output goes to stdout, just verify no error)
        run(wg_dir, false, None, true, false, &[], true).unwrap();
    }

    fn make_task_with_tags(
        id: &str,
        title: &str,
        status: Status,
        completed_at: Option<&str>,
        description: Option<&str>,
        tags: Vec<&str>,
    ) -> Task {
        Task {
            id: id.to_string(),
            title: title.to_string(),
            status,
            completed_at: completed_at.map(String::from),
            description: description.map(String::from),
            tags: tags.into_iter().map(String::from).collect(),
            ..Task::default()
        }
    }

    #[test]
    fn test_search_by_title() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");
        save_graph(&WorkGraph::new(), &graph_file).unwrap();

        let arch_path = wg_dir.join("archive.jsonl");
        let tasks = vec![
            make_task(
                "t1",
                "Implement login feature",
                Status::Done,
                Some("2024-01-01T00:00:00Z"),
            ),
            make_task(
                "t2",
                "Fix database bug",
                Status::Done,
                Some("2024-01-02T00:00:00Z"),
            ),
            make_task(
                "t3",
                "Login page styling",
                Status::Done,
                Some("2024-01-03T00:00:00Z"),
            ),
        ];
        append_to_archive(&tasks, &arch_path).unwrap();

        // Search should find tasks matching by title
        search(wg_dir, "login", 20, false).unwrap();

        // Verify by loading and filtering manually
        let loaded = load_archive(&arch_path).unwrap();
        let matches: Vec<_> = loaded
            .iter()
            .filter(|t| t.title.to_lowercase().contains("login"))
            .collect();
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].id, "t1");
        assert_eq!(matches[1].id, "t3");
    }

    #[test]
    fn test_search_by_description() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");
        save_graph(&WorkGraph::new(), &graph_file).unwrap();

        let arch_path = wg_dir.join("archive.jsonl");
        let tasks = vec![
            make_task_with_tags(
                "t1",
                "Task A",
                Status::Done,
                Some("2024-01-01T00:00:00Z"),
                Some("Contains authentication logic"),
                vec![],
            ),
            make_task_with_tags(
                "t2",
                "Task B",
                Status::Done,
                Some("2024-01-02T00:00:00Z"),
                Some("Contains database logic"),
                vec![],
            ),
        ];
        append_to_archive(&tasks, &arch_path).unwrap();

        // Should find by description content
        search(wg_dir, "authentication", 20, false).unwrap();

        let loaded = load_archive(&arch_path).unwrap();
        let matches: Vec<_> = loaded
            .iter()
            .filter(|t| {
                t.description
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains("authentication")
            })
            .collect();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, "t1");
    }

    #[test]
    fn test_search_by_tags() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");
        save_graph(&WorkGraph::new(), &graph_file).unwrap();

        let arch_path = wg_dir.join("archive.jsonl");
        let tasks = vec![
            make_task_with_tags(
                "t1",
                "Task A",
                Status::Done,
                Some("2024-01-01T00:00:00Z"),
                None,
                vec!["frontend", "urgent"],
            ),
            make_task_with_tags(
                "t2",
                "Task B",
                Status::Done,
                Some("2024-01-02T00:00:00Z"),
                None,
                vec!["backend"],
            ),
        ];
        append_to_archive(&tasks, &arch_path).unwrap();

        // Search by tag
        search(wg_dir, "frontend", 20, false).unwrap();

        let loaded = load_archive(&arch_path).unwrap();
        let matches: Vec<_> = loaded
            .iter()
            .filter(|t| {
                t.tags
                    .iter()
                    .any(|tag| tag.to_lowercase().contains("frontend"))
            })
            .collect();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, "t1");
    }

    #[test]
    fn test_search_case_insensitive() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");
        save_graph(&WorkGraph::new(), &graph_file).unwrap();

        let arch_path = wg_dir.join("archive.jsonl");
        let tasks = vec![make_task(
            "t1",
            "IMPORTANT Feature",
            Status::Done,
            Some("2024-01-01T00:00:00Z"),
        )];
        append_to_archive(&tasks, &arch_path).unwrap();

        // Case-insensitive search
        search(wg_dir, "important", 20, false).unwrap();

        let loaded = load_archive(&arch_path).unwrap();
        let matches: Vec<_> = loaded
            .iter()
            .filter(|t| t.title.to_lowercase().contains("important"))
            .collect();
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_search_with_limit() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");
        save_graph(&WorkGraph::new(), &graph_file).unwrap();

        let arch_path = wg_dir.join("archive.jsonl");
        let tasks = vec![
            make_task(
                "t1",
                "Test task one",
                Status::Done,
                Some("2024-01-01T00:00:00Z"),
            ),
            make_task(
                "t2",
                "Test task two",
                Status::Done,
                Some("2024-01-02T00:00:00Z"),
            ),
            make_task(
                "t3",
                "Test task three",
                Status::Done,
                Some("2024-01-03T00:00:00Z"),
            ),
        ];
        append_to_archive(&tasks, &arch_path).unwrap();

        // Search with limit=1 should not error
        search(wg_dir, "test", 1, false).unwrap();
    }

    #[test]
    fn test_search_json_output() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");
        save_graph(&WorkGraph::new(), &graph_file).unwrap();

        let arch_path = wg_dir.join("archive.jsonl");
        let tasks = vec![make_task(
            "t1",
            "Test task",
            Status::Done,
            Some("2024-01-01T00:00:00Z"),
        )];
        append_to_archive(&tasks, &arch_path).unwrap();

        // JSON output should not error
        search(wg_dir, "test", 20, true).unwrap();
    }

    #[test]
    fn test_search_no_matches() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");
        save_graph(&WorkGraph::new(), &graph_file).unwrap();

        let arch_path = wg_dir.join("archive.jsonl");
        let tasks = vec![make_task(
            "t1",
            "Some task",
            Status::Done,
            Some("2024-01-01T00:00:00Z"),
        )];
        append_to_archive(&tasks, &arch_path).unwrap();

        // No matches
        search(wg_dir, "nonexistent", 20, false).unwrap();
    }

    #[test]
    fn test_restore_as_done() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");
        let arch_path = wg_dir.join("archive.jsonl");

        // Create an empty graph
        save_graph(&WorkGraph::new(), &graph_file).unwrap();

        // Create archive with a task
        let tasks = vec![
            make_task(
                "t1",
                "Archived Task",
                Status::Done,
                Some("2024-01-01T00:00:00Z"),
            ),
            make_task(
                "t2",
                "Other Archived",
                Status::Done,
                Some("2024-01-02T00:00:00Z"),
            ),
        ];
        append_to_archive(&tasks, &arch_path).unwrap();

        // Restore t1 without --reopen
        restore(wg_dir, "t1", false).unwrap();

        // Verify task is in graph with status Done
        let graph = load_graph(&graph_file).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);
        assert_eq!(task.title, "Archived Task");

        // Verify task is removed from archive
        let archived = load_archive(&arch_path).unwrap();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].id, "t2");
    }

    #[test]
    fn test_restore_with_reopen() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");
        let arch_path = wg_dir.join("archive.jsonl");

        save_graph(&WorkGraph::new(), &graph_file).unwrap();

        let tasks = vec![make_task(
            "t1",
            "Archived Task",
            Status::Done,
            Some("2024-01-01T00:00:00Z"),
        )];
        append_to_archive(&tasks, &arch_path).unwrap();

        // Restore with --reopen
        restore(wg_dir, "t1", true).unwrap();

        // Verify task is in graph with status Open
        let graph = load_graph(&graph_file).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Open);
        assert!(task.completed_at.is_none());

        // Verify archive is now empty
        let archived = load_archive(&arch_path).unwrap();
        assert!(archived.is_empty());
    }

    #[test]
    fn test_restore_nonexistent_task() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");
        let arch_path = wg_dir.join("archive.jsonl");

        save_graph(&WorkGraph::new(), &graph_file).unwrap();

        let tasks = vec![make_task(
            "t1",
            "Archived Task",
            Status::Done,
            Some("2024-01-01T00:00:00Z"),
        )];
        append_to_archive(&tasks, &arch_path).unwrap();

        // Restoring a nonexistent task should fail
        let result = restore(wg_dir, "nonexistent", false);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not found in archive")
        );
    }

    #[test]
    fn test_restore_duplicate_in_graph() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");
        let arch_path = wg_dir.join("archive.jsonl");

        // Create graph with existing task "t1"
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task(
            "t1",
            "Active Task",
            Status::Open,
            None,
        )));
        save_graph(&graph, &graph_file).unwrap();

        // Archive also has t1
        let tasks = vec![make_task(
            "t1",
            "Archived Task",
            Status::Done,
            Some("2024-01-01T00:00:00Z"),
        )];
        append_to_archive(&tasks, &arch_path).unwrap();

        // Should fail because t1 already exists in graph
        let result = restore(wg_dir, "t1", false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn test_remove_from_archive() {
        let dir = tempdir().unwrap();
        let arch_path = dir.path().join("archive.jsonl");

        let tasks = vec![
            make_task("t1", "Task 1", Status::Done, Some("2024-01-01T00:00:00Z")),
            make_task("t2", "Task 2", Status::Done, Some("2024-01-02T00:00:00Z")),
            make_task("t3", "Task 3", Status::Done, Some("2024-01-03T00:00:00Z")),
        ];
        append_to_archive(&tasks, &arch_path).unwrap();

        // Remove t2
        remove_from_archive(&arch_path, "t2").unwrap();

        let remaining = load_archive(&arch_path).unwrap();
        assert_eq!(remaining.len(), 2);
        assert_eq!(remaining[0].id, "t1");
        assert_eq!(remaining[1].id, "t3");
    }

    #[test]
    fn test_archive_specific_ids() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");

        // Create a graph with multiple done tasks
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task(
            "t1",
            "Done Task 1",
            Status::Done,
            Some("2024-01-01T00:00:00Z"),
        )));
        graph.add_node(Node::Task(make_task(
            "t2",
            "Done Task 2",
            Status::Done,
            Some("2024-01-02T00:00:00Z"),
        )));
        graph.add_node(Node::Task(make_task(
            "t3",
            "Done Task 3",
            Status::Done,
            Some("2024-01-03T00:00:00Z"),
        )));
        graph.add_node(Node::Task(make_task("t4", "Open Task", Status::Open, None)));
        save_graph(&graph, &graph_file).unwrap();

        // Archive only t1 and t3 by ID
        let ids = vec!["t1".to_string(), "t3".to_string()];
        run(wg_dir, false, None, false, false, &ids, false).unwrap();

        // Verify only t1 and t3 were archived
        let loaded = load_graph(&graph_file).unwrap();
        assert!(loaded.get_task("t1").is_none());
        assert!(loaded.get_task("t2").is_some());
        assert!(loaded.get_task("t3").is_none());
        assert!(loaded.get_task("t4").is_some());

        let arch_path = wg_dir.join("archive.jsonl");
        let archived = load_archive(&arch_path).unwrap();
        assert_eq!(archived.len(), 2);
        let archived_ids: Vec<&str> = archived.iter().map(|t| t.id.as_str()).collect();
        assert!(archived_ids.contains(&"t1"));
        assert!(archived_ids.contains(&"t3"));
    }

    #[test]
    fn test_archive_specific_ids_rejects_non_done() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("t1", "Open Task", Status::Open, None)));
        save_graph(&graph, &graph_file).unwrap();

        // Trying to archive a non-done task should fail
        let ids = vec!["t1".to_string()];
        let result = run(wg_dir, false, None, false, false, &ids, false);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("lacks a verified v2 GraphSave")
        );
    }

    #[test]
    fn test_archive_specific_ids_rejects_missing() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");
        save_graph(&WorkGraph::new(), &graph_file).unwrap();

        let ids = vec!["nonexistent".to_string()];
        let result = run(wg_dir, false, None, false, false, &ids, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_archive_yes_flag() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task(
            "t1",
            "Done Task",
            Status::Done,
            Some("2024-01-01T00:00:00Z"),
        )));
        save_graph(&graph, &graph_file).unwrap();

        // With --yes, bulk archive proceeds without error
        run(wg_dir, false, None, false, true, &[], false).unwrap();

        let loaded = load_graph(&graph_file).unwrap();
        assert!(loaded.get_task("t1").is_none());
    }

    #[test]
    fn test_archive_undo() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");

        // Create graph with two done tasks
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task(
            "t1",
            "Done Task 1",
            Status::Done,
            Some("2024-01-01T00:00:00Z"),
        )));
        graph.add_node(Node::Task(make_task(
            "t2",
            "Done Task 2",
            Status::Done,
            Some("2024-01-02T00:00:00Z"),
        )));
        graph.add_node(Node::Task(make_task("t3", "Open Task", Status::Open, None)));
        save_graph(&graph, &graph_file).unwrap();

        // Archive with --yes
        run(wg_dir, false, None, false, true, &[], false).unwrap();

        // Verify tasks are archived
        let loaded = load_graph(&graph_file).unwrap();
        assert!(loaded.get_task("t1").is_none());
        assert!(loaded.get_task("t2").is_none());
        assert!(loaded.get_task("t3").is_some());

        // Undo the archive
        undo(wg_dir).unwrap();

        // Verify tasks are restored
        let loaded = load_graph(&graph_file).unwrap();
        assert!(loaded.get_task("t1").is_some());
        assert!(loaded.get_task("t2").is_some());
        assert!(loaded.get_task("t3").is_some());

        // Verify archive is now empty for those tasks
        let arch_path = wg_dir.join("archive.jsonl");
        let archived = load_archive(&arch_path).unwrap();
        assert!(archived.is_empty());

        // Verify batch metadata file is removed
        let batch_path = wg_dir.join("archive-last-batch.json");
        assert!(!batch_path.exists());
    }

    #[test]
    fn test_archive_undo_no_batch() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir).unwrap();
        let graph_file = wg_dir.join("graph.jsonl");
        save_graph(&WorkGraph::new(), &graph_file).unwrap();

        // Undo without a previous archive should fail
        let result = undo(wg_dir);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No archive batch to undo")
        );
    }

    #[test]
    fn test_archive_batch_metadata_roundtrip() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();

        let ids = vec!["t1".to_string(), "t2".to_string(), "t3".to_string()];
        save_batch_metadata(wg_dir, &ids).unwrap();

        let loaded = load_batch_metadata(wg_dir).unwrap();
        assert_eq!(loaded, ids);
    }

    #[test]
    fn test_should_archive_abandoned_task() {
        let task = make_task("t1", "Test", Status::Abandoned, None);
        assert!(should_archive(&task, None));
    }

    #[test]
    fn test_should_archive_abandoned_with_older() {
        // Abandoned task with created_at 40 days ago
        let mut task = make_task("t1", "Test", Status::Abandoned, None);
        task.created_at = Some((Utc::now() - Duration::days(40)).to_rfc3339());
        let min_age = Duration::days(30);
        assert!(should_archive(&task, Some(&min_age)));
    }

    #[test]
    fn test_should_not_archive_recent_abandoned() {
        let mut task = make_task("t1", "Test", Status::Abandoned, None);
        task.created_at = Some((Utc::now() - Duration::days(5)).to_rfc3339());
        let min_age = Duration::days(30);
        assert!(!should_archive(&task, Some(&min_age)));
    }

    #[test]
    fn archived_boundary_records_active_successor_without_rewriting_edge() {
        let mut graph = WorkGraph::new();
        let mut archived = make_task(
            "t1",
            "Done Task",
            Status::Done,
            Some("2024-01-01T00:00:00Z"),
        );
        archived.before = vec!["t2".to_string()];
        let mut active = make_task("t2", "Open Task", Status::Open, None);
        active.after = vec!["t1".to_string()];
        graph.add_node(Node::Task(archived.clone()));
        graph.add_node(Node::Task(active));

        let boundary = archived_boundary_for(&archived, &graph);
        assert_eq!(boundary.successors, vec!["t2"]);
        assert_eq!(graph.get_task("t2").unwrap().after, vec!["t1"]);
    }

    fn set_retention(dir: &Path, days: u64) {
        std::fs::write(
            dir.join("config.toml"),
            format!("[coordinator]\narchive_retention_days = {days}\n"),
        )
        .unwrap();
    }

    fn fixed_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-01T07:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn automatic_first_legacy_restart_holds_month_old_backlog() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        let graph_file = wg_dir.join("graph.jsonl");
        set_retention(wg_dir, 7);

        let now = fixed_now();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task(
            "historical",
            "Month-old result",
            Status::Done,
            Some(&(now - Duration::days(38)).to_rfc3339()),
        )));
        save_graph(&graph, &graph_file).unwrap();

        let outcome = run_automatic_at(wg_dir, 7, "candidate-build", now).unwrap();
        assert!(matches!(
            outcome,
            AutomaticArchiveOutcome::Held { count: 1, .. }
        ));
        assert!(
            load_graph(&graph_file)
                .unwrap()
                .get_task("historical")
                .is_some()
        );
        assert!(load_archive(&archive_path(wg_dir)).unwrap().is_empty());

        let status = automatic_status(wg_dir);
        assert!(status.pending);
        assert_eq!(status.pending_count, 1);
        assert_eq!(status.task_ids, vec!["historical"]);
        assert_eq!(
            status.confirm_command.as_deref(),
            Some("wg archive auto --confirm")
        );
    }

    #[test]
    fn automatic_confirmation_archives_exact_batch_once_and_undo_is_lossless() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        let graph_file = wg_dir.join("graph.jsonl");
        set_retention(wg_dir, 7);
        let now = fixed_now();

        let mut historical = make_task(
            "historical",
            "Exact historical result",
            Status::Done,
            Some(&(now - Duration::days(38)).to_rfc3339()),
        );
        historical.created_at = Some((now - Duration::days(40)).to_rfc3339());
        historical.after = vec!["foundation".to_string()];
        historical.before = vec!["active".to_string()];
        historical.log = vec![worksgood::graph::LogEntry {
            timestamp: (now - Duration::days(20)).to_rfc3339(),
            actor: Some("operator".to_string()),
            user: Some("tester".to_string()),
            message: "kept log line".to_string(),
        }];
        let mut active = make_task("active", "Still active", Status::Open, None);
        active.after = vec!["historical".to_string()];
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(historical));
        graph.add_node(Node::Task(active));
        save_graph(&graph, &graph_file).unwrap();
        // Compare against the exact canonical task bytes as persisted (the
        // parser may populate derived interaction timestamps on first load).
        let original = load_graph(&graph_file)
            .unwrap()
            .get_task("historical")
            .unwrap()
            .clone();

        run_automatic_at(wg_dir, 7, "candidate-build", now).unwrap();
        let ids = confirm_automatic_at(wg_dir, "candidate-build", now).unwrap();
        assert_eq!(ids, vec!["historical"]);
        let archived_graph = load_graph(&graph_file).unwrap();
        assert!(archived_graph.get_task("historical").is_none());
        assert!(archived_graph.get_archived_boundary("historical").is_some());
        assert_eq!(
            archived_graph.get_task("active").unwrap().after,
            vec!["historical"]
        );
        assert_eq!(load_archive(&archive_path(wg_dir)).unwrap().len(), 1);

        // A second confirmation cannot reapply or duplicate the receipt.
        assert!(confirm_automatic_at(wg_dir, "candidate-build", now).is_err());
        assert_eq!(load_archive(&archive_path(wg_dir)).unwrap().len(), 1);

        undo(wg_dir).unwrap();
        let restored = load_graph(&graph_file).unwrap();
        assert_eq!(restored.get_task("historical"), Some(&original));
        assert_eq!(
            restored.get_task("active").unwrap().after,
            vec!["historical"]
        );
        assert!(load_archive(&archive_path(wg_dir)).unwrap().is_empty());
    }

    #[test]
    fn automatic_zero_retention_is_non_destructive() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        let graph_file = wg_dir.join("graph.jsonl");
        set_retention(wg_dir, 0);
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task(
            "historical",
            "Done Task",
            Status::Done,
            Some("2020-01-01T00:00:00Z"),
        )));
        save_graph(&graph, &graph_file).unwrap();

        assert_eq!(
            run_automatic_at(wg_dir, 0, "candidate-build", fixed_now()).unwrap(),
            AutomaticArchiveOutcome::Disabled
        );
        assert!(
            load_graph(&graph_file)
                .unwrap()
                .get_task("historical")
                .is_some()
        );
        assert!(!automatic_status(wg_dir).enabled);
    }

    #[test]
    fn disabling_retention_clears_held_batch_without_archiving_or_rewriting_graph() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        let graph_file = wg_dir.join("graph.jsonl");
        let now = fixed_now();
        set_retention(wg_dir, 7);

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task(
            "done-evidence",
            "Historical completion evidence",
            Status::Done,
            Some(&(now - Duration::days(40)).to_rfc3339()),
        )));
        let mut abandoned = make_task(
            "abandoned-evidence",
            "Historical incident evidence",
            Status::Abandoned,
            None,
        );
        abandoned.created_at = Some((now - Duration::days(40)).to_rfc3339());
        graph.add_node(Node::Task(abandoned));
        graph.add_node(Node::Task(make_task(
            "visible-open",
            "Visible active task",
            Status::Open,
            None,
        )));
        save_graph(&graph, &graph_file).unwrap();
        let before = std::fs::read(&graph_file).unwrap();

        let held = run_automatic_at(wg_dir, 7, "old-build", now).unwrap();
        assert!(matches!(
            held,
            AutomaticArchiveOutcome::Held { count: 2, .. }
        ));
        assert_eq!(
            load_automatic_state(wg_dir)
                .unwrap()
                .unwrap()
                .pending
                .as_ref()
                .unwrap()
                .tasks
                .len(),
            2
        );

        // Model a config reload followed by a candidate-daemon restart. Both
        // passes must see the hard disable, clear the stale dispatch plan, and
        // leave every visible task byte/status untouched.
        set_retention(wg_dir, 0);
        for (build, at) in [
            ("old-build", now + Duration::minutes(1)),
            ("candidate-build", now + Duration::minutes(2)),
        ] {
            assert_eq!(
                run_automatic_at(wg_dir, 0, build, at).unwrap(),
                AutomaticArchiveOutcome::Disabled
            );
        }

        let state = load_automatic_state(wg_dir).unwrap().unwrap();
        assert_eq!(state.retention_days, 0);
        assert!(state.pending.is_none());
        assert_eq!(std::fs::read(&graph_file).unwrap(), before);
        assert!(load_archive(&archive_path(wg_dir)).unwrap().is_empty());
        let active = load_graph(&graph_file).unwrap();
        assert_eq!(
            active.get_task("done-evidence").unwrap().status,
            Status::Done
        );
        assert_eq!(
            active.get_task("abandoned-evidence").unwrap().status,
            Status::Abandoned
        );
        assert_eq!(
            active.get_task("visible-open").unwrap().status,
            Status::Open
        );
    }

    #[test]
    fn acknowledged_restart_archives_only_newly_eligible_increment() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        let graph_file = wg_dir.join("graph.jsonl");
        set_retention(wg_dir, 7);
        let now = fixed_now();

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task(
            "backlog",
            "Initial backlog",
            Status::Done,
            Some(&(now - Duration::days(30)).to_rfc3339()),
        )));
        save_graph(&graph, &graph_file).unwrap();
        run_automatic_at(wg_dir, 7, "candidate-build", now).unwrap();
        confirm_automatic_at(wg_dir, "candidate-build", now).unwrap();

        // This task crosses the 7-day cutoff during the next routine 24h
        // interval. The already-confirmed historical cutoff is not rescanned.
        let mut graph = load_graph(&graph_file).unwrap();
        graph.add_node(Node::Task(make_task(
            "increment",
            "Newly eligible",
            Status::Done,
            Some(&(now - Duration::hours(156)).to_rfc3339()),
        )));
        // Simulate an imported ancient record after acknowledgment. It lies
        // before the watermark and must not resurrect the historical backlog.
        graph.add_node(Node::Task(make_task(
            "pre-watermark-import",
            "Old import",
            Status::Done,
            Some(&(now - Duration::days(60)).to_rfc3339()),
        )));
        save_graph(&graph, &graph_file).unwrap();

        assert_eq!(
            run_automatic_at(wg_dir, 7, "candidate-build", now + Duration::days(1)).unwrap(),
            AutomaticArchiveOutcome::Archived { count: 1 }
        );
        let graph = load_graph(&graph_file).unwrap();
        assert!(graph.get_task("increment").is_none());
        assert!(graph.get_task("pre-watermark-import").is_some());
        assert_eq!(load_archive(&archive_path(wg_dir)).unwrap().len(), 2);

        assert_eq!(
            run_automatic_at(
                wg_dir,
                7,
                "candidate-build",
                now + Duration::days(1) + Duration::hours(1)
            )
            .unwrap(),
            AutomaticArchiveOutcome::Advanced
        );
        assert_eq!(load_archive(&archive_path(wg_dir)).unwrap().len(), 2);
    }

    #[test]
    fn pending_plan_is_rebased_to_candidate_build_after_upgrade() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        let graph_file = wg_dir.join("graph.jsonl");
        set_retention(wg_dir, 7);
        let now = fixed_now();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task(
            "historical",
            "Historical",
            Status::Done,
            Some(&(now - Duration::days(30)).to_rfc3339()),
        )));
        save_graph(&graph, &graph_file).unwrap();

        run_automatic_at(wg_dir, 7, "build-a", now).unwrap();
        let outcome = run_automatic_at(wg_dir, 7, "build-b", now + Duration::minutes(1)).unwrap();
        assert!(matches!(
            outcome,
            AutomaticArchiveOutcome::Held { count: 1, reason }
                if reason.contains("build changed")
        ));
        let state = load_automatic_state(wg_dir).unwrap().unwrap();
        assert_eq!(state.pending.as_ref().unwrap().build_id, "build-b");
        assert_eq!(
            confirm_automatic_at(wg_dir, "build-b", now + Duration::minutes(1)).unwrap(),
            vec!["historical"]
        );
    }

    #[test]
    fn acknowledged_build_change_holds_even_small_increment() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        let graph_file = wg_dir.join("graph.jsonl");
        set_retention(wg_dir, 7);
        let now = fixed_now();
        save_graph(&WorkGraph::new(), &graph_file).unwrap();
        run_automatic_at(wg_dir, 7, "build-a", now).unwrap();
        confirm_automatic_at(wg_dir, "build-a", now).unwrap();

        let outcome = run_automatic_at(wg_dir, 7, "build-b", now + Duration::hours(1)).unwrap();
        assert!(matches!(
            outcome,
            AutomaticArchiveOutcome::Held { count: 0, reason }
                if reason.contains("build changed")
        ));
    }

    #[test]
    fn acknowledged_long_downtime_holds_even_empty_batch() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        let graph_file = wg_dir.join("graph.jsonl");
        set_retention(wg_dir, 7);
        let now = fixed_now();
        save_graph(&WorkGraph::new(), &graph_file).unwrap();
        run_automatic_at(wg_dir, 7, "candidate-build", now).unwrap();
        confirm_automatic_at(wg_dir, "candidate-build", now).unwrap();

        let outcome =
            run_automatic_at(wg_dir, 7, "candidate-build", now + Duration::days(3)).unwrap();
        assert!(matches!(
            outcome,
            AutomaticArchiveOutcome::Held { count: 0, reason }
                if reason.contains("overdue")
        ));
    }

    #[test]
    fn acknowledged_oversized_increment_holds_exact_ids() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        let graph_file = wg_dir.join("graph.jsonl");
        set_retention(wg_dir, 7);
        let now = fixed_now();
        save_graph(&WorkGraph::new(), &graph_file).unwrap();
        run_automatic_at(wg_dir, 7, "candidate-build", now).unwrap();
        confirm_automatic_at(wg_dir, "candidate-build", now).unwrap();

        let mut graph = WorkGraph::new();
        for index in 0..=MAX_AUTOMATIC_INCREMENTAL_BATCH {
            graph.add_node(Node::Task(make_task(
                &format!("increment-{index:02}"),
                "Increment",
                Status::Done,
                Some(&(now - Duration::days(7) + Duration::minutes(30)).to_rfc3339()),
            )));
        }
        save_graph(&graph, &graph_file).unwrap();
        let outcome =
            run_automatic_at(wg_dir, 7, "candidate-build", now + Duration::hours(1)).unwrap();
        assert!(matches!(
            outcome,
            AutomaticArchiveOutcome::Held { count, reason }
                if count == MAX_AUTOMATIC_INCREMENTAL_BATCH + 1
                    && reason.contains("unattended limit")
        ));
        let status = automatic_status(wg_dir);
        assert_eq!(status.pending_count, MAX_AUTOMATIC_INCREMENTAL_BATCH + 1);
        assert_eq!(status.task_ids.first().unwrap(), "increment-00");
        assert_eq!(status.task_ids.last().unwrap(), "increment-10");
        assert!(load_archive(&archive_path(wg_dir)).unwrap().is_empty());
    }

    #[test]
    fn acknowledged_retention_change_requires_new_confirmation() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        let graph_file = wg_dir.join("graph.jsonl");
        set_retention(wg_dir, 7);
        let now = fixed_now();
        save_graph(&WorkGraph::new(), &graph_file).unwrap();
        run_automatic_at(wg_dir, 7, "candidate-build", now).unwrap();
        confirm_automatic_at(wg_dir, "candidate-build", now).unwrap();

        set_retention(wg_dir, 8);
        let outcome =
            run_automatic_at(wg_dir, 8, "candidate-build", now + Duration::hours(1)).unwrap();
        assert!(matches!(
            outcome,
            AutomaticArchiveOutcome::Held { count: 0, reason }
                if reason.contains("retention changed")
        ));
    }

    #[test]
    fn automatic_skips_system_tasks_and_preserves_active_boundary() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        let graph_file = wg_dir.join("graph.jsonl");
        set_retention(wg_dir, 7);
        let now = fixed_now();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task(
            ".compact-0",
            "System Task",
            Status::Done,
            Some(&(now - Duration::days(40)).to_rfc3339()),
        )));
        let mut done = make_task(
            "done",
            "Done With Deps",
            Status::Done,
            Some(&(now - Duration::days(40)).to_rfc3339()),
        );
        done.before = vec!["active".to_string()];
        graph.add_node(Node::Task(done));
        let mut active = make_task("active", "In Progress", Status::InProgress, None);
        active.after = vec!["done".to_string()];
        graph.add_node(Node::Task(active));
        save_graph(&graph, &graph_file).unwrap();

        run_automatic_at(wg_dir, 7, "candidate-build", now).unwrap();
        confirm_automatic_at(wg_dir, "candidate-build", now).unwrap();
        let graph = load_graph(&graph_file).unwrap();
        assert!(graph.get_task(".compact-0").is_some());
        assert!(graph.get_archived_boundary("done").is_some());
        assert_eq!(graph.get_task("active").unwrap().after, vec!["done"]);
    }
}
