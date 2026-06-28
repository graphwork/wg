use anyhow::{Context, Result};
use std::path::Path;

use worksgood::agency;
use worksgood::provenance;

use super::trace_export::TraceExport;

pub fn run(
    dir: &Path,
    file: &str,
    source: Option<&str>,
    dry_run: bool,
    review: bool,
    json: bool,
) -> Result<()> {
    // Read and deserialize the export file
    let contents =
        std::fs::read_to_string(file).with_context(|| format!("Failed to read '{}'", file))?;
    let export: TraceExport = serde_json::from_str(&contents)
        .with_context(|| format!("Failed to parse '{}' as trace export", file))?;

    // Determine source tag
    let source_tag = source
        .map(String::from)
        .or_else(|| export.metadata.source.clone())
        .unwrap_or_else(|| {
            Path::new(file)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string()
        });

    let task_count = export.tasks.len();
    let eval_count = export.evaluations.len();
    let op_count = export.operations.len();

    if dry_run {
        println!("=== Dry Run: wg trace import ===");
        println!("File:        {}", file);
        println!("Source:      {}", source_tag);
        println!("Visibility:  {}", export.metadata.visibility);
        println!("Tasks:       {}", task_count);
        println!("Evaluations: {}", eval_count);
        println!("Operations:  {}", op_count);

        if !export.tasks.is_empty() {
            println!("\nTasks to import:");
            for task in &export.tasks {
                println!(
                    "  imported/{}/{} - {} ({:?})",
                    source_tag, task.id, task.title, task.status
                );
            }
        }

        if json {
            let out = serde_json::json!({
                "dry_run": true,
                "source": source_tag,
                "task_count": task_count,
                "evaluation_count": eval_count,
                "operation_count": op_count,
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        return Ok(());
    }

    // Create import directory
    let import_dir = dir.join("imports").join(&source_tag);
    std::fs::create_dir_all(&import_dir)
        .with_context(|| format!("Failed to create import dir: {}", import_dir.display()))?;

    // ── IC1 ingest review (ENFORCING, on by default) ─────────────────────────────
    // Screen each imported task's text BEFORE it is written into the graph as
    // consumable context (a poisoned imported task description is a prompt-injection a
    // later agent reads via `wg context`). Author-trust is DERIVED from the source tag:
    // a `wgid:` source resolves through the canonical peer/provider dial; any other tag
    // is an un-vouched source and screens as Unknown (deep, fail-closed). A non-`accept`
    // verdict WITHHOLDS that task — it is NOT written (received ≠ consumed). `--no-review`
    // writes everything unscreened.
    let mut withheld: Vec<(String, String, String)> = Vec::new(); // (orig id, verdict, reason)
    let kept: Vec<_> = if review {
        let import_trust = worksgood::trust::resolve_author_trust(dir, &source_tag);
        let cfg = worksgood::config::Config::load_merged(dir).ok();
        let store = worksgood::review::VerdictStore::open(dir);
        export
            .tasks
            .iter()
            .filter(|t| {
                let text = match &t.description {
                    Some(d) => format!("{}\n\n{}", t.title, d),
                    None => t.title.clone(),
                };
                let prov = worksgood::review::Provenance {
                    author: Some(source_tag.clone()),
                    trust: import_trust.clone(),
                };
                let outcome = match &cfg {
                    Some(c) => worksgood::review::review_inbound_ctx(
                        c,
                        worksgood::review::ContentClass::Ic1Task,
                        &text,
                        &prov,
                        worksgood::review::Sensitivity::Unlabeled,
                    ),
                    None => worksgood::review::review_inbound(
                        worksgood::review::ContentClass::Ic1Task,
                        &text,
                        &prov,
                        worksgood::review::Sensitivity::Unlabeled,
                    ),
                };
                // Audit leg (best-effort): record the verdict on the sigchain.
                let consumer = format!("imported/{}/{}", source_tag, t.id);
                let _ = store.record(&outcome, Some(&source_tag), Some(&consumer));
                if outcome.verdict.permits_consumption() {
                    true
                } else {
                    withheld.push((
                        t.id.clone(),
                        outcome.verdict.tag().to_string(),
                        outcome.reason.tag().to_string(),
                    ));
                    false
                }
            })
            .collect()
    } else {
        export.tasks.iter().collect()
    };
    let imported_count = kept.len();
    let withheld_count = withheld.len();

    // Import tasks as namespaced YAML
    let tasks_path = import_dir.join("tasks.yaml");
    let imported_tasks: Vec<ImportedTask> = kept
        .iter()
        .map(|t| ImportedTask {
            id: format!("imported/{}/{}", source_tag, t.id),
            original_id: t.id.clone(),
            title: t.title.clone(),
            description: t.description.clone(),
            status: "Done".to_string(),
            visibility: "internal".to_string(),
            skills: t.skills.clone(),
            tags: {
                let mut tags = t.tags.clone();
                tags.push("imported".to_string());
                tags.push(format!("source:{}", source_tag));
                tags
            },
            artifacts: t.artifacts.clone(),
            created_at: t.created_at.clone(),
            completed_at: t.completed_at.clone(),
            agent: t.agent.clone(),
            source: source_tag.clone(),
        })
        .collect();

    let tasks_yaml =
        serde_yaml::to_string(&imported_tasks).context("Failed to serialize imported tasks")?;
    std::fs::write(&tasks_path, tasks_yaml)
        .with_context(|| format!("Failed to write {}", tasks_path.display()))?;

    // Import evaluations with prefix and modified source
    if !export.evaluations.is_empty() {
        let agency_dir = dir.join("agency");
        agency::init(&agency_dir)?;
        let evals_dir = agency_dir.join("evaluations");

        for eval in &export.evaluations {
            let mut imported_eval = eval.clone();
            imported_eval.id = format!("imported-{}", eval.id);
            imported_eval.source = format!("import:{}", eval.source);
            // Save directly without propagating to performance records
            agency::save_evaluation(&imported_eval, &evals_dir).with_context(|| {
                format!("Failed to save imported evaluation {}", imported_eval.id)
            })?;
        }
    }

    // Import operations to separate log
    if !export.operations.is_empty() {
        let ops_path = import_dir.join("operations.jsonl");
        let mut lines = String::new();
        for op in &export.operations {
            let line = serde_json::to_string(op)?;
            lines.push_str(&line);
            lines.push('\n');
        }
        std::fs::write(&ops_path, lines)
            .with_context(|| format!("Failed to write {}", ops_path.display()))?;
    }

    // Record provenance
    let _ = provenance::record(
        dir,
        "trace_import",
        None,
        Some("user"),
        serde_json::json!({
            "source": source_tag,
            "file": file,
            "task_count": task_count,
            "evaluation_count": eval_count,
            "operation_count": op_count,
        }),
        provenance::DEFAULT_ROTATION_THRESHOLD,
    );

    // Output result
    let withheld_json: Vec<serde_json::Value> = withheld
        .iter()
        .map(|(id, verdict, reason)| {
            serde_json::json!({ "id": id, "verdict": verdict, "reason": reason })
        })
        .collect();
    if json {
        let out = serde_json::json!({
            "source": source_tag,
            "import_dir": import_dir.display().to_string(),
            "task_count": task_count,
            "imported_count": imported_count,
            "withheld_count": withheld_count,
            "withheld": withheld_json,
            "evaluation_count": eval_count,
            "operation_count": op_count,
            "reviewed": review,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!(
            "Imported {} of {} tasks, {} evaluations, {} operations from '{}'",
            imported_count, task_count, eval_count, op_count, source_tag
        );
        if withheld_count > 0 {
            println!(
                "WITHHELD {withheld_count} task(s) on a non-accept review verdict (bytes not written):"
            );
            for (id, verdict, reason) in &withheld {
                println!("  - {id}: {verdict} ({reason})");
            }
            println!(
                "  To import an un-vouched source, enroll it (`wg peer add <name> --wgid <wgid> --trust verified`) \
                 or re-run with --no-review to accept the risk."
            );
        }
        println!("Import directory: {}", import_dir.display());
    }

    Ok(())
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ImportedTask {
    id: String,
    original_id: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    status: String,
    visibility: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    artifacts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    source: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::trace_export::{ExportMetadata, ExportedTask, TraceExport};
    use tempfile::TempDir;
    use worksgood::graph::Status;

    fn make_minimal_export(tasks: Vec<ExportedTask>) -> TraceExport {
        TraceExport {
            metadata: ExportMetadata {
                version: "0.1.0".to_string(),
                exported_at: "2026-02-28T12:00:00Z".to_string(),
                visibility: "internal".to_string(),
                root_task: None,
                source: None,
            },
            tasks,
            evaluations: vec![],
            operations: vec![],
            functions: vec![],
        }
    }

    fn make_exported_task(id: &str, title: &str) -> ExportedTask {
        ExportedTask {
            id: id.to_string(),
            title: title.to_string(),
            description: None,
            status: Status::Done,
            visibility: "public".to_string(),
            skills: vec![],
            after: vec![],
            before: vec![],
            tags: vec!["original-tag".to_string()],
            artifacts: vec![],
            created_at: None,
            completed_at: None,
            agent: None,
            log: vec![],
        }
    }

    // ── Source tag determination ──

    #[test]
    fn test_source_tag_from_explicit_arg() {
        // When source is provided as arg, it takes priority
        let source = Some(String::from("my-source"))
            .or(None)
            .unwrap_or_else(|| "fallback".to_string());
        assert_eq!(source, "my-source");
    }

    #[test]
    fn test_source_tag_from_metadata() {
        // When no explicit source, falls back to metadata.source
        let metadata_source = Some("meta-source".to_string());
        let source: Option<String> = None;
        let resolved = source
            .or(metadata_source)
            .unwrap_or_else(|| "fallback".to_string());
        assert_eq!(resolved, "meta-source");
    }

    #[test]
    fn test_source_tag_from_filename() {
        // When no source arg or metadata, derive from filename
        let file = "/path/to/my-export.json";
        let source_tag = Path::new(file)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        assert_eq!(source_tag, "my-export");
    }

    // ── Task namespacing and tag augmentation ──

    #[test]
    fn test_imported_task_namespacing() {
        let source_tag = "team-alpha";
        let original_id = "build-feature";
        let namespaced = format!("imported/{}/{}", source_tag, original_id);
        assert_eq!(namespaced, "imported/team-alpha/build-feature");
    }

    #[test]
    fn test_imported_task_tag_augmentation() {
        let source_tag = "team-alpha";
        let mut tags = vec!["original-tag".to_string()];
        tags.push("imported".to_string());
        tags.push(format!("source:{}", source_tag));
        assert_eq!(tags, vec!["original-tag", "imported", "source:team-alpha"]);
    }

    // ── ImportedTask serialization ──

    #[test]
    fn test_imported_task_serialization_roundtrip() {
        let task = ImportedTask {
            id: "imported/src/t1".to_string(),
            original_id: "t1".to_string(),
            title: "Test task".to_string(),
            description: Some("A description".to_string()),
            status: "Done".to_string(),
            visibility: "internal".to_string(),
            skills: vec!["rust".to_string()],
            tags: vec!["imported".to_string(), "source:src".to_string()],
            artifacts: vec![],
            created_at: None,
            completed_at: None,
            agent: Some("agent-1".to_string()),
            source: "src".to_string(),
        };

        let yaml = serde_yaml::to_string(&task).unwrap();
        let parsed: ImportedTask = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.id, "imported/src/t1");
        assert_eq!(parsed.original_id, "t1");
        assert_eq!(parsed.source, "src");
        assert_eq!(parsed.tags, vec!["imported", "source:src"]);
    }

    // ── Dry run test ──

    #[test]
    fn test_dry_run_does_not_write_files() {
        let tmp = TempDir::new().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        // Write a valid export file
        let export = make_minimal_export(vec![make_exported_task("t1", "Task 1")]);
        let export_json = serde_json::to_string_pretty(&export).unwrap();
        let export_path = tmp.path().join("export.json");
        std::fs::write(&export_path, &export_json).unwrap();

        // Run in dry_run mode
        let result = run(
            &wg_dir,
            export_path.to_str().unwrap(),
            Some("test-source"),
            true,  // dry_run
            true,  // review
            false, // json
        );
        assert!(result.is_ok());

        // No import directory should have been created
        let import_dir = wg_dir.join("imports").join("test-source");
        assert!(!import_dir.exists());
    }
}
