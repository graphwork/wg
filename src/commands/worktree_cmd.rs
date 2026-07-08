//! `wg worktree` subcommands — list, archive, gc, and inspect agent worktrees.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};

/// List all worktrees under `.wg-worktrees/`.
pub fn list(workgraph_dir: &Path) -> Result<()> {
    let project_root = workgraph_dir
        .parent()
        .context("Cannot determine project root from WG dir")?;
    let worktrees_dir = project_root.join(".wg-worktrees");

    if !worktrees_dir.exists() {
        println!("No worktrees directory found.");
        return Ok(());
    }

    let mut entries: Vec<_> = std::fs::read_dir(&worktrees_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    if entries.is_empty() {
        println!("No worktrees found.");
        return Ok(());
    }

    println!("Agent worktrees ({}):", entries.len());
    for entry in &entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let path = entry.path();
        let size = dir_size_human(&path);
        let mtime = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| {
                let elapsed = t.elapsed().ok()?;
                Some(humanize_duration(elapsed))
            })
            .unwrap_or_else(|| "unknown".to_string());

        // Check if there are uncommitted changes
        let has_changes = has_uncommitted_changes(&path);
        let status = if has_changes {
            " [uncommitted changes]"
        } else {
            ""
        };

        println!("  {} — {} — modified {}{}", name, size, mtime, status);
    }

    Ok(())
}

/// Archive a specific agent's worktree: commit uncommitted work,
/// then optionally remove the directory.
pub fn archive(workgraph_dir: &Path, agent_id: &str, remove: bool) -> Result<()> {
    let project_root = workgraph_dir
        .parent()
        .context("Cannot determine project root from WG dir")?;
    let worktrees_dir = project_root.join(".wg-worktrees");
    let wt_path = worktrees_dir.join(agent_id);

    if !wt_path.exists() {
        anyhow::bail!(
            "Worktree for '{}' not found at {}",
            agent_id,
            wt_path.display()
        );
    }

    // Check for uncommitted changes and auto-commit them
    if has_uncommitted_changes(&wt_path) {
        eprintln!(
            "[worktree] Committing uncommitted changes in {} ...",
            agent_id
        );

        // Stage all changes
        let add = Command::new("git")
            .args(["add", "-A"])
            .current_dir(&wt_path)
            .output()
            .context("Failed to run git add")?;

        if !add.status.success() {
            let stderr = String::from_utf8_lossy(&add.stderr);
            anyhow::bail!("git add failed: {}", stderr.trim());
        }

        // Commit with archive message
        let msg = format!(
            "archive: {} work snapshot\n\nAuto-committed by `wg worktree archive` to preserve\nuncommitted agent work before archival.",
            agent_id
        );
        let commit = Command::new("git")
            .args(["commit", "-m", &msg])
            .current_dir(&wt_path)
            .output()
            .context("Failed to run git commit")?;

        if !commit.status.success() {
            let stderr = String::from_utf8_lossy(&commit.stderr);
            // "nothing to commit" is OK
            if !stderr.contains("nothing to commit") {
                anyhow::bail!("git commit failed: {}", stderr.trim());
            }
        } else {
            eprintln!(
                "[worktree] Committed: {}",
                String::from_utf8_lossy(&commit.stdout).trim()
            );
        }
    } else {
        eprintln!("[worktree] No uncommitted changes in {}", agent_id);
    }

    if remove {
        eprintln!(
            "[worktree] Removing worktree directory {} ...",
            wt_path.display()
        );

        // First try git worktree remove (clean git integration)
        let wt_remove = Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&wt_path)
            .current_dir(project_root)
            .output();

        match wt_remove {
            Ok(output) if output.status.success() => {
                eprintln!("[worktree] Removed via git worktree remove");
            }
            _ => {
                // Fallback: manual removal (not a real git worktree,
                // just a directory)
                std::fs::remove_dir_all(&wt_path).context("Failed to remove worktree directory")?;
                eprintln!("[worktree] Removed directory manually");
            }
        }

        eprintln!("[worktree] Archived and removed: {}", agent_id);
    } else {
        eprintln!("[worktree] Archived (preserved on disk): {}", agent_id);
        eprintln!("  To remove: wg worktree archive {} --remove", agent_id);
    }

    Ok(())
}

pub(crate) fn has_uncommitted_changes(wt_path: &Path) -> bool {
    Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(wt_path)
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false)
}

fn dir_size_human(path: &Path) -> String {
    let output = Command::new("du").args(["-sh"]).arg(path).output().ok();
    output
        .and_then(|o| {
            String::from_utf8(o.stdout)
                .ok()
                .map(|s| s.split_whitespace().next().unwrap_or("?").to_string())
        })
        .unwrap_or_else(|| "?".to_string())
}

fn humanize_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s ago", secs)
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

/// Parse a human-friendly duration ("7d", "24h", "90m", "3600s").
/// Used for the `--older` filter on `wg worktree gc`.
pub(crate) fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("empty duration");
    }
    let (num_part, unit_char): (&str, char) = {
        let last = s.chars().last().unwrap();
        if last.is_ascii_digit() {
            (s, 's')
        } else {
            (&s[..s.len() - last.len_utf8()], last)
        }
    };
    let n: u64 = num_part
        .parse()
        .with_context(|| format!("invalid duration number: '{}'", num_part))?;
    let secs = match unit_char {
        's' => n,
        'm' => n * 60,
        'h' => n * 60 * 60,
        'd' => n * 60 * 60 * 24,
        'w' => n * 60 * 60 * 24 * 7,
        other => anyhow::bail!("unknown duration unit '{}' — use s/m/h/d/w", other),
    };
    Ok(Duration::from_secs(secs))
}

/// Garbage-collect stale agent worktrees. Dry-run by default.
///
/// Worktrees are sacred — this is the only bulk-removal path in WG,
/// and it refuses to act without explicit filters to prevent accidental
/// nuke-all. Dirty matches are blocked by default and diagnostics point to
/// `archive --remove`, which commits a preservation snapshot before removal.
pub fn gc(
    workgraph_dir: &Path,
    execute: bool,
    older: Option<&str>,
    dead_only: bool,
    discard_uncommitted: bool,
) -> Result<()> {
    let project_root = workgraph_dir
        .parent()
        .context("Cannot determine project root from WG dir")?;
    let worktrees_dir = project_root.join(".wg-worktrees");

    if !worktrees_dir.exists() {
        println!("No worktrees directory found.");
        return Ok(());
    }

    // Require at least one filter — refuse to nuke-all by default.
    if older.is_none() && !dead_only {
        anyhow::bail!(
            "wg worktree gc requires at least one filter (--older <dur> and/or --dead-only). \
             Worktrees are sacred — use explicit criteria to choose which ones to collect."
        );
    }

    let older_than = match older {
        Some(s) => Some(parse_duration(s).context("--older parse failed")?),
        None => None,
    };

    // Build the live-agent set if we'll need it.
    let alive_agents: std::collections::HashSet<String> = if dead_only {
        use worksgood::service::AgentRegistry;
        match AgentRegistry::load_locked(workgraph_dir) {
            Ok(reg) => reg
                .list_alive_agents()
                .into_iter()
                .map(|a| a.id.clone())
                .collect(),
            Err(_) => std::collections::HashSet::new(),
        }
    } else {
        std::collections::HashSet::new()
    };

    let mut clean_candidates: Vec<GcCandidate> = Vec::new();
    let mut dirty_candidates: Vec<GcCandidate> = Vec::new();
    let now = SystemTime::now();

    for entry in std::fs::read_dir(&worktrees_dir)? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("agent-") {
            continue;
        }
        let path = entry.path();
        let mtime = match entry.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let age = now.duration_since(mtime).unwrap_or(Duration::ZERO);

        if let Some(threshold) = older_than
            && age < threshold
        {
            continue;
        }
        if dead_only && alive_agents.contains(&name) {
            continue;
        }

        let candidate = GcCandidate {
            agent_id: name.clone(),
            path: path.clone(),
            age: humanize_duration(age),
            size: dir_size_human(&path),
        };
        if has_uncommitted_changes(&path) {
            dirty_candidates.push(candidate);
        } else {
            clean_candidates.push(candidate);
        }
    }

    if clean_candidates.is_empty() && dirty_candidates.is_empty() {
        println!("No worktrees match the filters.");
        return Ok(());
    }

    clean_candidates.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
    dirty_candidates.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));

    let total = clean_candidates.len() + dirty_candidates.len();

    if !execute {
        println!("Matched {} worktree(s) (dry-run):", total);
        println!("  {} clean removable", clean_candidates.len());
        println!("  {} dirty blocked", dirty_candidates.len());
        if !clean_candidates.is_empty() {
            println!();
            println!("Clean removable worktrees:");
            for c in &clean_candidates {
                println!("  [remove] {} — {} — {}", c.agent_id, c.size, c.age);
            }
        }
        if !dirty_candidates.is_empty() {
            println!();
            println!("Dirty blocked worktrees (uncommitted work is preserved):");
            for c in &dirty_candidates {
                println!("  [blocked] {} — {} — {}", c.agent_id, c.size, c.age);
            }
            println!();
            println!(
                "Preserve-first cleanup: `wg worktree archive <agent-id> --remove` commits a snapshot before removal."
            );
            println!(
                "Destructive cleanup: re-run with --execute --discard-uncommitted to permanently discard dirty work."
            );
        }
        println!();
        println!("Re-run with --execute to remove clean worktrees.");
        return Ok(());
    }

    let mut ok = 0;
    let mut failed = 0;
    let removal_candidates: Vec<&GcCandidate> = if discard_uncommitted {
        clean_candidates
            .iter()
            .chain(dirty_candidates.iter())
            .collect()
    } else {
        clean_candidates.iter().collect()
    };

    if !dirty_candidates.is_empty() && !discard_uncommitted {
        eprintln!(
            "[worktree-gc] Refusing to remove {} dirty worktree(s) with uncommitted changes.",
            dirty_candidates.len()
        );
        for c in &dirty_candidates {
            eprintln!(
                "[worktree-gc] skipped dirty {} — preserve with `wg worktree archive {} --remove`, or intentionally discard with --discard-uncommitted",
                c.agent_id, c.agent_id
            );
        }
    } else if !dirty_candidates.is_empty() {
        eprintln!(
            "[worktree-gc] DANGEROUS: --discard-uncommitted active; {} dirty worktree(s) will be destroyed.",
            dirty_candidates.len()
        );
    }

    for c in &removal_candidates {
        let branch = find_branch_for_agent(project_root, &c.agent_id)
            .unwrap_or_else(|| format!("wg/{}/unknown", c.agent_id));
        match crate::commands::spawn::worktree::remove_worktree(project_root, &c.path, &branch) {
            Ok(()) => {
                ok += 1;
                println!("[removed] {}", c.agent_id);
            }
            Err(e) => {
                eprintln!("[worktree-gc] {}: {}", c.agent_id, e);
                failed += 1;
            }
        }
    }
    println!();
    println!("Removed {} worktree(s); {} failed.", ok, failed);
    if !dirty_candidates.is_empty() && !discard_uncommitted {
        anyhow::bail!(
            "cleanup incomplete: {} dirty worktree(s) skipped. Preserve with `wg worktree archive <agent-id> --remove`, or re-run with --discard-uncommitted to intentionally destroy uncommitted work.",
            dirty_candidates.len()
        );
    }
    if failed > 0 {
        anyhow::bail!("cleanup incomplete: {} worktree removal(s) failed", failed);
    }
    Ok(())
}

struct GcCandidate {
    agent_id: String,
    path: PathBuf,
    age: String,
    size: String,
}

/// Look up the `wg/<agent>/<task>` branch for an agent-id, if any.
fn find_branch_for_agent(project_root: &Path, agent_id: &str) -> Option<String> {
    let out = Command::new("git")
        .args(["branch", "--list", &format!("wg/{}/*", agent_id)])
        .current_dir(project_root)
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    for line in s.lines() {
        let trimmed = line.trim_start_matches(['*', '+', ' ']).trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn run_git(project_root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(project_root)
            .output()
            .unwrap_or_else(|e| panic!("failed to run git {:?}: {}", args, e));
        assert!(
            output.status.success(),
            "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn fixture_repo(tmp: &TempDir) -> (PathBuf, PathBuf) {
        let project_root = tmp.path().join("project");
        std::fs::create_dir_all(&project_root).unwrap();
        run_git(
            tmp.path(),
            &["init", "-b", "main", project_root.to_str().unwrap()],
        );
        run_git(&project_root, &["config", "user.email", "test@example.com"]);
        run_git(&project_root, &["config", "user.name", "WG Test"]);
        std::fs::write(project_root.join("README.md"), "initial\n").unwrap();
        run_git(&project_root, &["add", "README.md"]);
        run_git(&project_root, &["commit", "-m", "initial"]);

        let wg_dir = project_root.join(".wg");
        std::fs::create_dir_all(wg_dir.join("service")).unwrap();
        (project_root, wg_dir)
    }

    fn add_agent_worktree(project_root: &Path, agent_id: &str, task_id: &str) -> PathBuf {
        let wt_path = project_root.join(".wg-worktrees").join(agent_id);
        std::fs::create_dir_all(wt_path.parent().unwrap()).unwrap();
        let branch = format!("wg/{}/{}", agent_id, task_id);
        run_git(
            project_root,
            &[
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "-b",
                &branch,
                "HEAD",
            ],
        );
        wt_path
    }

    #[test]
    fn parse_duration_supports_common_units() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_duration("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_duration("7d").unwrap(), Duration::from_secs(604800));
        assert_eq!(parse_duration("1w").unwrap(), Duration::from_secs(604800));
    }

    #[test]
    fn parse_duration_bare_number_is_seconds() {
        assert_eq!(parse_duration("60").unwrap(), Duration::from_secs(60));
    }

    #[test]
    fn parse_duration_rejects_unknown_unit() {
        let err = parse_duration("7y").unwrap_err();
        assert!(err.to_string().contains("unknown duration unit"));
    }

    #[test]
    fn parse_duration_rejects_empty() {
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn has_uncommitted_changes_counts_untracked_agent_work() {
        let tmp = TempDir::new().unwrap();
        let (project_root, _wg_dir) = fixture_repo(&tmp);
        let wt_path = add_agent_worktree(&project_root, "agent-dirty", "task-dirty");

        std::fs::write(wt_path.join("new-agent-file.txt"), "important WIP\n").unwrap();

        assert!(
            has_uncommitted_changes(&wt_path),
            "untracked files are uncommitted agent work and must block GC"
        );
    }

    #[test]
    fn worktree_gc_execute_skips_dirty_and_removes_clean_by_default() {
        let tmp = TempDir::new().unwrap();
        let (project_root, wg_dir) = fixture_repo(&tmp);
        let clean = add_agent_worktree(&project_root, "agent-clean", "task-clean");
        let dirty = add_agent_worktree(&project_root, "agent-dirty", "task-dirty");
        std::fs::write(dirty.join("uncommitted.txt"), "preserve me\n").unwrap();

        gc(&wg_dir, false, None, true, false).expect("dry-run should not fail");
        assert!(clean.exists(), "dry-run must not remove clean worktree");
        assert!(dirty.exists(), "dry-run must not remove dirty worktree");

        let err = gc(&wg_dir, true, None, true, false).unwrap_err();
        assert!(
            err.to_string().contains("dirty worktree"),
            "execute should report dirty blocked worktrees, got: {}",
            err
        );
        assert!(!clean.exists(), "clean dead worktree should be removed");
        assert!(
            dirty.exists(),
            "dirty dead worktree must survive by default"
        );
    }

    #[test]
    fn worktree_gc_discard_uncommitted_removes_dirty_opt_in() {
        let tmp = TempDir::new().unwrap();
        let (project_root, wg_dir) = fixture_repo(&tmp);
        let dirty = add_agent_worktree(&project_root, "agent-discard", "task-discard");
        std::fs::write(dirty.join("uncommitted.txt"), "discard me\n").unwrap();

        gc(&wg_dir, true, None, true, true).expect("discard opt-in should remove dirty worktree");

        assert!(
            !dirty.exists(),
            "--discard-uncommitted should intentionally remove dirty worktree"
        );
    }
}
