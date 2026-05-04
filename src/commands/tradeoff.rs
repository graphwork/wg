use anyhow::{Context, Result};
use std::path::Path;
use workgraph::agency::{self};

/// Get the agency base directory (creates it if needed).
fn agency_dir(workgraph_dir: &Path) -> Result<std::path::PathBuf> {
    let dir = workgraph_dir.join("agency");
    agency::init(&dir).context("Failed to initialise agency directory")?;
    Ok(dir)
}

/// Get the tradeoffs subdirectory.
fn tradeoffs_dir(workgraph_dir: &Path) -> Result<std::path::PathBuf> {
    Ok(agency_dir(workgraph_dir)?.join("primitives/tradeoffs"))
}

/// `wg tradeoff add <name> --accept ... --reject ... [--description ...]`
pub fn run_add(
    workgraph_dir: &Path,
    name: &str,
    accept: &[String],
    reject: &[String],
    description: Option<&str>,
) -> Result<()> {
    let dir = tradeoffs_dir(workgraph_dir)?;

    let tradeoff = agency::build_tradeoff(
        name,
        description.unwrap_or(""),
        accept.to_vec(),
        reject.to_vec(),
    );

    // Check for duplicates (same content = same hash)
    let tradeoff_path = dir.join(format!("{}.yaml", tradeoff.id));
    if tradeoff_path.exists() {
        anyhow::bail!(
            "Tradeoff with identical content already exists ({})",
            agency::short_hash(&tradeoff.id)
        );
    }

    let path = agency::save_tradeoff(&tradeoff, &dir)?;
    println!(
        "Created tradeoff: {} ({})",
        name,
        agency::short_hash(&tradeoff.id)
    );
    println!("  File: {}", path.display());
    Ok(())
}

/// `wg tradeoff list [--json]`
pub fn run_list(workgraph_dir: &Path, json: bool) -> Result<()> {
    let dir = tradeoffs_dir(workgraph_dir)?;
    let tradeoffs = agency::load_all_tradeoffs(&dir)?;

    if json {
        let output: Vec<serde_json::Value> = tradeoffs
            .iter()
            .map(|m| {
                serde_json::json!({
                    "id": m.id,
                    "name": m.name,
                    "description": m.description,
                    "acceptable_tradeoffs": m.acceptable_tradeoffs.len(),
                    "unacceptable_tradeoffs": m.unacceptable_tradeoffs.len(),
                    "avg_score": m.performance.avg_score,
                    "task_count": m.performance.task_count,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if tradeoffs.is_empty() {
        println!("No tradeoffs defined. Use 'wg tradeoff add' to create one.");
    } else {
        println!("Tradeoffs:\n");
        for m in &tradeoffs {
            let score_str = m
                .performance
                .avg_score
                .map(|s| format!("{:.2}", s))
                .unwrap_or_else(|| "n/a".to_string());
            println!(
                "  {}  {:20} accept:{} reject:{} score:{} tasks:{}",
                agency::short_hash(&m.id),
                m.name,
                m.acceptable_tradeoffs.len(),
                m.unacceptable_tradeoffs.len(),
                score_str,
                m.performance.task_count,
            );
        }
    }

    Ok(())
}

/// `wg tradeoff show <id> [--json]`
pub fn run_show(workgraph_dir: &Path, id: &str, json: bool) -> Result<()> {
    let dir = tradeoffs_dir(workgraph_dir)?;
    let tradeoff = agency::find_tradeoff_by_prefix(&dir, id)
        .with_context(|| format!("Failed to find tradeoff '{}'", id))?;

    if json {
        let yaml_str = serde_yaml::to_string(&tradeoff)?;
        // Convert YAML to JSON for --json output
        let value: serde_json::Value = serde_yaml::from_str(&yaml_str)?;
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!(
            "Tradeoff: {} ({})",
            tradeoff.name,
            agency::short_hash(&tradeoff.id)
        );
        println!("ID: {}", tradeoff.id);
        if !tradeoff.description.is_empty() {
            println!("Description: {}", tradeoff.description);
        }
        println!();

        if !tradeoff.acceptable_tradeoffs.is_empty() {
            println!("Acceptable tradeoffs:");
            for t in &tradeoff.acceptable_tradeoffs {
                println!("  + {}", t);
            }
        }

        if !tradeoff.unacceptable_tradeoffs.is_empty() {
            println!("Unacceptable tradeoffs:");
            for t in &tradeoff.unacceptable_tradeoffs {
                println!("  - {}", t);
            }
        }

        println!();
        println!(
            "Performance: {} tasks, avg score: {}",
            tradeoff.performance.task_count,
            tradeoff
                .performance
                .avg_score
                .map(|s| format!("{:.2}", s))
                .unwrap_or_else(|| "n/a".to_string()),
        );
    }

    Ok(())
}

/// `wg tradeoff lineage <id> [--json]`
pub fn run_lineage(workgraph_dir: &Path, id: &str, json: bool) -> Result<()> {
    let dir = tradeoffs_dir(workgraph_dir)?;

    // Resolve prefix to full ID first
    let tradeoff = agency::find_tradeoff_by_prefix(&dir, id)
        .with_context(|| format!("Failed to find tradeoff '{}'", id))?;

    let ancestry = agency::tradeoff_ancestry(&tradeoff.id, &dir)?;

    if ancestry.is_empty() {
        anyhow::bail!("Tradeoff '{}' not found", id);
    }

    if json {
        let json_nodes: Vec<serde_json::Value> = ancestry
            .iter()
            .map(|n| {
                serde_json::json!({
                    "id": n.id,
                    "name": n.name,
                    "generation": n.generation,
                    "created_by": n.created_by,
                    "created_at": n.created_at.to_rfc3339(),
                    "parent_ids": n.parent_ids,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json_nodes)?);
        return Ok(());
    }

    let target = &ancestry[0];
    println!(
        "Lineage for tradeoff: {} ({})",
        agency::short_hash(&target.id),
        target.name
    );
    println!();

    for node in &ancestry {
        let indent = "  ".repeat(node.generation as usize);
        let gen_label = if node.generation == 0 {
            "gen 0 (root)".to_string()
        } else {
            format!("gen {}", node.generation)
        };

        let parents = if node.parent_ids.is_empty() {
            String::new()
        } else {
            let short_parents: Vec<&str> = node
                .parent_ids
                .iter()
                .map(|p| agency::short_hash(p))
                .collect();
            format!(" <- [{}]", short_parents.join(", "))
        };

        println!(
            "{}{} ({}) [{}] created by: {}{}",
            indent,
            agency::short_hash(&node.id),
            node.name,
            gen_label,
            node.created_by,
            parents
        );
    }

    if ancestry.len() == 1 && ancestry[0].parent_ids.is_empty() {
        println!();
        println!("This tradeoff has no evolutionary history (manually created).");
    }

    Ok(())
}

/// `wg tradeoff edit <id>` - opens in $EDITOR
///
/// After editing, the tradeoff is re-hashed. If the content changed, the file is
/// renamed to the new hash and the old file is removed.
pub fn run_edit(workgraph_dir: &Path, id: &str) -> Result<()> {
    let dir = tradeoffs_dir(workgraph_dir)?;
    let tradeoff = agency::find_tradeoff_by_prefix(&dir, id)
        .with_context(|| format!("Failed to find tradeoff '{}'", id))?;

    let tradeoff_path = dir.join(format!("{}.yaml", tradeoff.id));

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());

    let status = std::process::Command::new(&editor)
        .arg(&tradeoff_path)
        .status()
        .with_context(|| format!("Failed to launch editor '{}'", editor))?;

    if !status.success() {
        anyhow::bail!("Editor exited with non-zero status");
    }

    // Validate and re-hash
    let mut edited = agency::load_tradeoff(&tradeoff_path)
        .context("Edited file is not valid tradeoff YAML - changes may be malformed")?;

    let new_id = agency::content_hash_tradeoff(
        &edited.acceptable_tradeoffs,
        &edited.unacceptable_tradeoffs,
        &edited.description,
    );
    if new_id != edited.id {
        // Content changed — rename to new hash
        let old_path = tradeoff_path;
        edited.id = new_id;
        agency::save_tradeoff(&edited, &dir)?;
        std::fs::remove_file(&old_path).ok();
        println!(
            "Tradeoff content changed, new ID: {}",
            agency::short_hash(&edited.id)
        );
    } else {
        // Mutable fields (name, etc.) may have changed; re-save in place
        agency::save_tradeoff(&edited, &dir)?;
        println!("Tradeoff '{}' updated", agency::short_hash(&edited.id));
    }

    Ok(())
}

/// `wg tradeoff rm <id>`
pub fn run_rm(workgraph_dir: &Path, id: &str) -> Result<()> {
    let dir = tradeoffs_dir(workgraph_dir)?;
    let tradeoff = agency::find_tradeoff_by_prefix(&dir, id)
        .with_context(|| format!("Failed to find tradeoff '{}'", id))?;

    let path = dir.join(format!("{}.yaml", tradeoff.id));
    std::fs::remove_file(&path).context("Failed to remove tradeoff file")?;
    println!(
        "Removed tradeoff: {} ({})",
        tradeoff.name,
        agency::short_hash(&tradeoff.id)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> TempDir {
        let tmp = TempDir::new().unwrap();
        // Create the workgraph dir structure
        std::fs::create_dir_all(tmp.path().join("agency").join("primitives/tradeoffs")).unwrap();
        tmp
    }

    #[test]
    fn test_content_hash_deterministic() {
        let h1 = agency::content_hash_tradeoff(&["Slow".into()], &["Broken".into()], "desc");
        let h2 = agency::content_hash_tradeoff(&["Slow".into()], &["Broken".into()], "desc");
        assert_eq!(h1, h2);
        // Agency-compatible primitive hashes ignore local tradeoff extension fields.
        let h3 = agency::content_hash_tradeoff(&["Fast".into()], &["Broken".into()], "desc");
        assert_eq!(h1, h3);
    }

    #[test]
    fn test_add_and_list() {
        let tmp = setup();
        run_add(
            tmp.path(),
            "Quality First",
            &["Slower delivery".to_string()],
            &["Skipping tests".to_string()],
            Some("Prioritise correctness"),
        )
        .unwrap();

        let dir = tradeoffs_dir(tmp.path()).unwrap();
        let all = agency::load_all_tradeoffs(&dir).unwrap();
        assert_eq!(all.len(), 1);
        // ID is now a content hash, not a slug
        assert_eq!(all[0].id.len(), 64); // SHA-256 hex = 64 chars
        assert_eq!(all[0].name, "Quality First");
        assert_eq!(all[0].acceptable_tradeoffs, vec!["Slower delivery"]);
        assert_eq!(all[0].unacceptable_tradeoffs, vec!["Skipping tests"]);
    }

    #[test]
    fn test_add_duplicate_fails() {
        let tmp = setup();
        run_add(tmp.path(), "Quality First", &[], &[], None).unwrap();
        let result = run_add(tmp.path(), "Quality First", &[], &[], None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn test_show_not_found() {
        let tmp = setup();
        let result = run_show(tmp.path(), "nonexistent", false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not found")
                || err.contains("Failed to find")
                || err.contains("No tradeoff matching"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_show_existing_by_prefix() {
        let tmp = setup();
        run_add(
            tmp.path(),
            "Speed Demon",
            &["Lower quality".to_string()],
            &["Data loss".to_string()],
            Some("Ship fast"),
        )
        .unwrap();

        // Look up by full hash
        let dir = tradeoffs_dir(tmp.path()).unwrap();
        let all = agency::load_all_tradeoffs(&dir).unwrap();
        let full_id = &all[0].id;
        let result = run_show(tmp.path(), full_id, false);
        assert!(result.is_ok());

        // Look up by short prefix
        let prefix = &full_id[..8];
        let result = run_show(tmp.path(), prefix, false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_rm() {
        let tmp = setup();
        run_add(tmp.path(), "Temp Tradeoff", &[], &[], None).unwrap();

        let dir = tradeoffs_dir(tmp.path()).unwrap();
        let all = agency::load_all_tradeoffs(&dir).unwrap();
        assert_eq!(all.len(), 1);
        let full_id = all[0].id.clone();

        run_rm(tmp.path(), &full_id).unwrap();
        assert_eq!(agency::load_all_tradeoffs(&dir).unwrap().len(), 0);
    }

    #[test]
    fn test_rm_not_found() {
        let tmp = setup();
        let result = run_rm(tmp.path(), "nonexistent");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not found")
                || err.contains("Failed to find")
                || err.contains("No tradeoff matching"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_list_empty() {
        let tmp = setup();
        let result = run_list(tmp.path(), false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_list_json() {
        let tmp = setup();
        run_add(
            tmp.path(),
            "Test Mot",
            &["a".to_string()],
            &["b".to_string()],
            None,
        )
        .unwrap();
        let result = run_list(tmp.path(), true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_show_json() {
        let tmp = setup();
        run_add(tmp.path(), "Test Mot", &[], &[], Some("desc")).unwrap();
        let dir = tradeoffs_dir(tmp.path()).unwrap();
        let all = agency::load_all_tradeoffs(&dir).unwrap();
        let result = run_show(tmp.path(), &all[0].id, true);
        assert!(result.is_ok());
    }
}
