//! Integration tests for worktree isolation and CARGO_TARGET_DIR per-worktree.
//!
//! This verifies that agents running in isolated worktrees don't contend
//! over cargo file locks, which was the #1 source of task failures.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;
use tempfile::TempDir;

// Test helper to initialize a git repo with basic Rust project structure
fn init_test_repo(path: &Path) {
    // Initialize git
    Command::new("git")
        .args(["init"])
        .arg(path)
        .output()
        .expect("Failed to init git repo");

    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(path)
        .output()
        .expect("Failed to set git email");

    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(path)
        .output()
        .expect("Failed to set git name");

    // Create a basic Rust project
    std::fs::write(
        path.join("Cargo.toml"),
        r#"
[package]
name = "testproject"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "testbin"
path = "src/main.rs"
"#,
    )
    .expect("Failed to write Cargo.toml");

    std::fs::create_dir_all(path.join("src")).expect("Failed to create src dir");

    std::fs::write(
        path.join("src/main.rs"),
        r#"
fn main() {
    println!("Hello from test project");
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_basic() {
        assert_eq!(2 + 2, 4);
    }
}
"#,
    )
    .expect("Failed to write src/main.rs");

    // Initial commit
    Command::new("git")
        .args(["add", "."])
        .current_dir(path)
        .output()
        .expect("Failed to git add");

    Command::new("git")
        .args(["commit", "-m", "initial commit"])
        .current_dir(path)
        .output()
        .expect("Failed to git commit");
}

// Test helper to create a worktree
fn create_test_worktree(project_root: &Path, agent_id: &str) -> std::path::PathBuf {
    let worktree_dir = project_root.join(".wg-worktrees").join(agent_id);
    let branch = format!("wg/{}/test-task", agent_id);

    std::fs::create_dir_all(&worktree_dir.parent().unwrap())
        .expect("Failed to create worktrees dir");

    let output = Command::new("git")
        .args(["worktree", "add"])
        .arg(&worktree_dir)
        .args(["-b", &branch, "HEAD"])
        .current_dir(project_root)
        .output()
        .expect("Failed to create worktree");

    if !output.status.success() {
        panic!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    worktree_dir
}

#[test]
fn test_worktree_cargo_isolation() {
    let temp = TempDir::new().expect("Failed to create temp dir");
    let project_root = temp.path().join("project");
    std::fs::create_dir_all(&project_root).expect("Failed to create project dir");

    // Initialize the test repo
    init_test_repo(&project_root);

    // Create two worktrees
    let wt1 = create_test_worktree(&project_root, "agent-1");
    let wt2 = create_test_worktree(&project_root, "agent-2");

    // Verify worktrees exist
    assert!(wt1.exists(), "Worktree 1 should exist");
    assert!(wt2.exists(), "Worktree 2 should exist");

    // Test concurrent cargo operations with different target dirs
    let start = Instant::now();

    let handle1 = std::thread::spawn({
        let wt1 = wt1.clone();
        move || {
            let mut cmd = Command::new("cargo");
            cmd.arg("test")
                .current_dir(&wt1)
                .env("CARGO_TARGET_DIR", wt1.join("target"))
                .stdout(Stdio::null())
                .stderr(Stdio::null());

            let output = cmd.output().expect("Failed to run cargo test in wt1");
            output.status.success()
        }
    });

    let handle2 = std::thread::spawn({
        let wt2 = wt2.clone();
        move || {
            let mut cmd = Command::new("cargo");
            cmd.arg("test")
                .current_dir(&wt2)
                .env("CARGO_TARGET_DIR", wt2.join("target"))
                .stdout(Stdio::null())
                .stderr(Stdio::null());

            let output = cmd.output().expect("Failed to run cargo test in wt2");
            output.status.success()
        }
    });

    // Wait for both to complete
    let result1 = handle1.join().expect("Thread 1 panicked");
    let result2 = handle2.join().expect("Thread 2 panicked");
    let elapsed = start.elapsed();

    // Both should succeed
    assert!(result1, "Cargo test in worktree 1 should succeed");
    assert!(result2, "Cargo test in worktree 2 should succeed");

    // If they were properly isolated, they should complete relatively quickly
    // (not serialized waiting for locks). This is a rough heuristic.
    assert!(
        elapsed.as_secs() < 30,
        "Concurrent tests should complete in reasonable time if properly isolated"
    );

    println!(
        "✓ Worktree isolation test passed - concurrent cargo operations completed in {:?}",
        elapsed
    );
}

#[test]
fn test_worktree_isolation_default_config() {
    use workgraph::config::CoordinatorConfig;

    // Verify that worktree isolation is enabled by default
    let config = CoordinatorConfig::default();
    assert!(
        config.worktree_isolation,
        "Worktree isolation should be enabled by default to prevent cargo lock contention"
    );
}

#[test]
fn test_worktree_creates_separate_target_dirs() {
    let temp = TempDir::new().expect("Failed to create temp dir");
    let project_root = temp.path().join("project");
    std::fs::create_dir_all(&project_root).expect("Failed to create project dir");

    // Initialize the test repo
    init_test_repo(&project_root);

    // Create two worktrees
    let wt1 = create_test_worktree(&project_root, "agent-1");
    let wt2 = create_test_worktree(&project_root, "agent-2");

    // Run a simple cargo check to create target directories
    let output1 = Command::new("cargo")
        .arg("check")
        .current_dir(&wt1)
        .env("CARGO_TARGET_DIR", wt1.join("target"))
        .output()
        .expect("Failed to run cargo check in wt1");
    assert!(
        output1.status.success(),
        "Cargo check should succeed in wt1"
    );

    let output2 = Command::new("cargo")
        .arg("check")
        .current_dir(&wt2)
        .env("CARGO_TARGET_DIR", wt2.join("target"))
        .output()
        .expect("Failed to run cargo check in wt2");
    assert!(
        output2.status.success(),
        "Cargo check should succeed in wt2"
    );

    // Verify separate target directories were created
    assert!(
        wt1.join("target").exists(),
        "Worktree 1 should have its own target directory"
    );
    assert!(
        wt2.join("target").exists(),
        "Worktree 2 should have its own target directory"
    );

    // Verify they are different directories
    let target1_path = wt1
        .join("target")
        .canonicalize()
        .expect("Failed to canonicalize target1");
    let target2_path = wt2
        .join("target")
        .canonicalize()
        .expect("Failed to canonicalize target2");
    assert_ne!(
        target1_path, target2_path,
        "Each worktree should have a separate target directory"
    );

    println!("✓ Separate target directories test passed");
}

#[test]
fn test_worktree_isolation_serde_default() {
    // Verify that deserializing a CoordinatorConfig WITHOUT worktree_isolation
    // field defaults to true (matching the programmatic Default impl).
    // This is critical: both serde and Default must agree.
    let toml_str = r#"
max_agents = 2
"#;
    let config: workgraph::config::CoordinatorConfig = toml::from_str(toml_str).unwrap();
    assert!(
        config.worktree_isolation,
        "Serde default for worktree_isolation should be true"
    );
}

#[test]
fn test_worktree_full_lifecycle() {
    // Full lifecycle test: create worktree → modify files → commit in worktree
    // → verify worktree state → cleanup → verify main branch unaffected
    let temp = TempDir::new().expect("Failed to create temp dir");
    let project_root = temp.path().join("project");
    std::fs::create_dir_all(&project_root).expect("Failed to create project dir");

    init_test_repo(&project_root);

    // Create .workgraph dir for symlink testing
    let wg_dir = project_root.join(".workgraph");
    std::fs::create_dir_all(&wg_dir).expect("Failed to create .workgraph");
    std::fs::write(wg_dir.join("graph.jsonl"), "").expect("Failed to write graph");

    // Step 1: Create worktree using the library function
    let wt_dir = create_test_worktree(&project_root, "agent-lifecycle");

    // Verify it shows in git worktree list
    let output = Command::new("git")
        .args(["worktree", "list"])
        .current_dir(&project_root)
        .output()
        .expect("Failed to list worktrees");
    let worktree_list = String::from_utf8_lossy(&output.stdout);
    assert!(
        worktree_list.contains(".wg-worktrees/agent-lifecycle"),
        "Worktree should appear in git worktree list: {}",
        worktree_list
    );

    // Step 2: Modify files in the worktree
    std::fs::write(wt_dir.join("agent_output.txt"), "work done by agent").unwrap();

    // Step 3: Commit in the worktree
    let output = Command::new("git")
        .args(["add", "agent_output.txt"])
        .current_dir(&wt_dir)
        .output()
        .expect("Failed to git add");
    assert!(output.status.success());

    let output = Command::new("git")
        .args(["commit", "-m", "agent work"])
        .current_dir(&wt_dir)
        .env("GIT_AUTHOR_NAME", "Test Agent")
        .env("GIT_AUTHOR_EMAIL", "agent@test.com")
        .env("GIT_COMMITTER_NAME", "Test Agent")
        .env("GIT_COMMITTER_EMAIL", "agent@test.com")
        .output()
        .expect("Failed to git commit");
    assert!(
        output.status.success(),
        "Commit in worktree should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify the file does NOT exist in main worktree
    assert!(
        !project_root.join("agent_output.txt").exists(),
        "Agent's file should not appear in main worktree before merge"
    );

    // Step 4: Simulate merge-back (squash merge from worktree branch to main)
    let branch = "wg/agent-lifecycle/test-task";
    let output = Command::new("git")
        .args(["merge", "--squash", branch])
        .current_dir(&project_root)
        .output()
        .expect("Failed to squash merge");
    assert!(
        output.status.success(),
        "Squash merge should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = Command::new("git")
        .args(["commit", "-m", "feat: lifecycle-test (agent-lifecycle)\n\nSquash-merged from worktree branch"])
        .current_dir(&project_root)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .output()
        .expect("Failed to commit merge");
    assert!(
        output.status.success(),
        "Merge commit should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify the file NOW exists in main worktree
    assert!(
        project_root.join("agent_output.txt").exists(),
        "Agent's file should appear in main worktree after merge"
    );

    // Step 5: Cleanup worktree
    let output = Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(&wt_dir)
        .current_dir(&project_root)
        .output()
        .expect("Failed to remove worktree");
    assert!(output.status.success());

    let output = Command::new("git")
        .args(["branch", "-D", branch])
        .current_dir(&project_root)
        .output()
        .expect("Failed to delete branch");
    assert!(output.status.success());

    // Verify worktree is gone from list
    let output = Command::new("git")
        .args(["worktree", "list"])
        .current_dir(&project_root)
        .output()
        .expect("Failed to list worktrees");
    let worktree_list = String::from_utf8_lossy(&output.stdout);
    assert!(
        !worktree_list.contains("agent-lifecycle"),
        "Worktree should be removed from git worktree list"
    );

    // Verify the merged file persists
    assert!(
        project_root.join("agent_output.txt").exists(),
        "Merged file should persist after worktree cleanup"
    );
}

#[test]
fn test_worktree_cleanup_on_failed_agent() {
    // Verify worktree cleanup works even when agent didn't commit anything
    // (simulating a failed/crashed agent)
    let temp = TempDir::new().expect("Failed to create temp dir");
    let project_root = temp.path().join("project");
    std::fs::create_dir_all(&project_root).expect("Failed to create project dir");

    init_test_repo(&project_root);

    let wt_dir = create_test_worktree(&project_root, "agent-failed");
    assert!(wt_dir.exists());

    // Agent modifies files but doesn't commit (simulating crash)
    std::fs::write(wt_dir.join("uncommitted.txt"), "work in progress").unwrap();

    // Cleanup should still work (force remove discards uncommitted changes)
    let branch = "wg/agent-failed/test-task";
    let output = Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(&wt_dir)
        .current_dir(&project_root)
        .output()
        .expect("Failed to remove worktree");
    assert!(
        output.status.success(),
        "Force-remove should work even with uncommitted changes: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = Command::new("git")
        .args(["branch", "-D", branch])
        .current_dir(&project_root)
        .output()
        .expect("Failed to delete branch");
    assert!(output.status.success());

    assert!(
        !wt_dir.exists(),
        "Worktree directory should be removed after cleanup"
    );
}

#[test]
fn test_worktree_workgraph_symlink_lifecycle() {
    // Verify that .workgraph is accessible from the worktree via symlink
    // and survives the full lifecycle
    let temp = TempDir::new().expect("Failed to create temp dir");
    let project_root = temp.path().join("project");
    std::fs::create_dir_all(&project_root).expect("Failed to create project dir");

    init_test_repo(&project_root);

    // Create .workgraph with test content
    let wg_dir = project_root.join(".workgraph");
    std::fs::create_dir_all(&wg_dir).expect("Failed to create .workgraph");
    std::fs::write(wg_dir.join("graph.jsonl"), r#"{"id":"test"}"#).unwrap();

    let wt_dir = create_test_worktree(&project_root, "agent-symlink");

    // Manually create the .workgraph symlink (as create_worktree in spawn/worktree.rs does)
    let symlink_path = wt_dir.join(".workgraph");
    let wg_canonical = wg_dir.canonicalize().expect("Failed to canonicalize");
    std::os::unix::fs::symlink(&wg_canonical, &symlink_path)
        .expect("Failed to create symlink");

    // Verify symlink works — agent can read graph.jsonl through it
    let content =
        std::fs::read_to_string(symlink_path.join("graph.jsonl")).expect("Failed to read through symlink");
    assert!(content.contains("test"), "Should read graph through symlink");

    // Agent writes to .workgraph through symlink (e.g., logging)
    std::fs::write(
        symlink_path.join("test_log.txt"),
        "agent log entry",
    )
    .expect("Failed to write through symlink");

    // Verify the write went to the real .workgraph
    assert!(
        wg_dir.join("test_log.txt").exists(),
        "Write through symlink should appear in real .workgraph"
    );

    // Cleanup: remove symlink first (like the real cleanup does)
    std::fs::remove_file(&symlink_path).expect("Failed to remove symlink");
    assert!(!symlink_path.exists(), "Symlink should be removed");
    assert!(
        wg_dir.join("test_log.txt").exists(),
        "Real .workgraph contents should survive symlink removal"
    );

    // Remove worktree
    let branch = "wg/agent-symlink/test-task";
    Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(&wt_dir)
        .current_dir(&project_root)
        .output()
        .unwrap();
    Command::new("git")
        .args(["branch", "-D", branch])
        .current_dir(&project_root)
        .output()
        .unwrap();
}

#[test]
fn test_worktree_concurrent_merge_safety() {
    // Verify that two worktrees modifying different files can both
    // be merged back without conflicts
    let temp = TempDir::new().expect("Failed to create temp dir");
    let project_root = temp.path().join("project");
    std::fs::create_dir_all(&project_root).expect("Failed to create project dir");

    init_test_repo(&project_root);

    // Create two worktrees
    let wt1 = create_test_worktree(&project_root, "agent-a");
    let wt2 = create_test_worktree(&project_root, "agent-b");

    // Agent A modifies one file
    std::fs::write(wt1.join("file_a.txt"), "agent A output").unwrap();
    Command::new("git")
        .args(["add", "file_a.txt"])
        .current_dir(&wt1)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "agent A work"])
        .current_dir(&wt1)
        .env("GIT_AUTHOR_NAME", "A")
        .env("GIT_AUTHOR_EMAIL", "a@test.com")
        .env("GIT_COMMITTER_NAME", "A")
        .env("GIT_COMMITTER_EMAIL", "a@test.com")
        .output()
        .unwrap();

    // Agent B modifies a different file
    std::fs::write(wt2.join("file_b.txt"), "agent B output").unwrap();
    Command::new("git")
        .args(["add", "file_b.txt"])
        .current_dir(&wt2)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "agent B work"])
        .current_dir(&wt2)
        .env("GIT_AUTHOR_NAME", "B")
        .env("GIT_AUTHOR_EMAIL", "b@test.com")
        .env("GIT_COMMITTER_NAME", "B")
        .env("GIT_COMMITTER_EMAIL", "b@test.com")
        .output()
        .unwrap();

    // Merge A first
    let output = Command::new("git")
        .args(["merge", "--squash", "wg/agent-a/test-task"])
        .current_dir(&project_root)
        .output()
        .unwrap();
    assert!(output.status.success(), "Merge A should succeed");
    Command::new("git")
        .args(["commit", "-m", "merge A"])
        .current_dir(&project_root)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .output()
        .unwrap();

    // Merge B second — should succeed since different files
    let output = Command::new("git")
        .args(["merge", "--squash", "wg/agent-b/test-task"])
        .current_dir(&project_root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Merge B should succeed (non-conflicting): {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Command::new("git")
        .args(["commit", "-m", "merge B"])
        .current_dir(&project_root)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .output()
        .unwrap();

    // Both files should exist in main
    assert!(project_root.join("file_a.txt").exists());
    assert!(project_root.join("file_b.txt").exists());

    // Cleanup
    for agent in &["agent-a", "agent-b"] {
        let wt = project_root.join(".wg-worktrees").join(agent);
        let branch = format!("wg/{}/test-task", agent);
        Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&wt)
            .current_dir(&project_root)
            .output()
            .unwrap();
        Command::new("git")
            .args(["branch", "-D", &branch])
            .current_dir(&project_root)
            .output()
            .unwrap();
    }
}
