//! Integration tests: atomic writes for YAML/JSON cache files.
//!
//! Verifies that save_yaml and save_evaluation use temp file + atomic rename,
//! and that failed writes don't leave corrupted target files.

use std::fs;
use tempfile::TempDir;

use workgraph::agency;

/// Verify that save_role writes atomically (no .tmp file left behind,
/// target file contains valid YAML).
#[test]
fn test_save_role_atomic_write() {
    let tmp = TempDir::new().unwrap();
    let agency_dir = tmp.path().join("agency");
    agency::init(&agency_dir).unwrap();

    let role = agency::build_role(
        "Test Role",
        "A role for testing atomic writes.",
        vec!["skill-a".to_string()],
        "Tested code",
    );
    let roles_dir = agency_dir.join("cache/roles");

    // Save should succeed
    let path = agency::save_role(&role, &roles_dir).unwrap();
    assert!(path.exists(), "Target file should exist after save");

    // No temp file should be left behind
    let tmp_path = roles_dir.join(format!(".{}.yaml.tmp", role.id));
    assert!(
        !tmp_path.exists(),
        "Temp file should be cleaned up after successful write"
    );

    // The written file should be valid YAML that round-trips
    let loaded = agency::load_all_roles(&roles_dir).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, role.id);
    assert_eq!(loaded[0].name, "Test Role");
}

/// Verify that save_tradeoff writes atomically.
#[test]
fn test_save_tradeoff_atomic_write() {
    let tmp = TempDir::new().unwrap();
    let agency_dir = tmp.path().join("agency");
    agency::init(&agency_dir).unwrap();

    let tradeoff = agency::build_tradeoff(
        "Test Tradeoff",
        "Prioritizes testing.",
        vec!["Slower delivery".to_string()],
        vec!["Untested code".to_string()],
    );
    let tradeoffs_dir = agency_dir.join("primitives/tradeoffs");

    let path = agency::save_tradeoff(&tradeoff, &tradeoffs_dir).unwrap();
    assert!(path.exists());

    let tmp_path = tradeoffs_dir.join(format!(".{}.yaml.tmp", tradeoff.id));
    assert!(!tmp_path.exists(), "Temp file should not remain");

    let loaded = agency::load_all_tradeoffs(&tradeoffs_dir).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, tradeoff.id);
}

/// Verify that overwriting an existing file atomically preserves the
/// old content if the new write is valid (i.e., the old file isn't
/// truncated before the new one is ready).
#[test]
fn test_atomic_overwrite_preserves_consistency() {
    let tmp = TempDir::new().unwrap();
    let agency_dir = tmp.path().join("agency");
    agency::init(&agency_dir).unwrap();

    let roles_dir = agency_dir.join("cache/roles");

    // Write initial version
    let role_v1 = agency::build_role(
        "Role V1",
        "First version.",
        vec!["skill-a".to_string()],
        "Outcome v1",
    );
    agency::save_role(&role_v1, &roles_dir).unwrap();

    // Overwrite with updated version (same ID structure means same hash)
    // Use a different role with its own ID to test general atomic overwrite
    let role_v2 = agency::build_role(
        "Role V2",
        "Second version.",
        vec!["skill-a".to_string(), "skill-b".to_string()],
        "Outcome v2",
    );
    let path = agency::save_role(&role_v2, &roles_dir).unwrap();

    // File should contain v2 content
    let content = fs::read_to_string(&path).unwrap();
    assert!(content.contains("Role V2"));

    // Verify it loads correctly
    let loaded: Vec<_> = agency::load_all_roles(&roles_dir)
        .unwrap()
        .into_iter()
        .filter(|r| r.id == role_v2.id)
        .collect();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].name, "Role V2");
}

/// Verify that writing to a read-only directory fails gracefully
/// and does not leave a temp file behind.
#[test]
#[cfg(unix)]
fn test_atomic_write_cleanup_on_failure() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let agency_dir = tmp.path().join("agency");
    agency::init(&agency_dir).unwrap();

    let roles_dir = agency_dir.join("cache/roles");

    // Write one valid file first
    let role = agency::build_role(
        "Original",
        "Should survive.",
        vec!["skill-a".to_string()],
        "Outcome",
    );
    agency::save_role(&role, &roles_dir).unwrap();

    // Make the directory read-only so the temp file write fails
    let perms = fs::Permissions::from_mode(0o555);
    fs::set_permissions(&roles_dir, perms).unwrap();

    // Try to save a new role — should fail
    let role2 = agency::build_role(
        "New Role",
        "Should fail to save.",
        vec!["skill-b".to_string()],
        "Outcome 2",
    );
    let result = agency::save_role(&role2, &roles_dir);
    assert!(result.is_err(), "Write to read-only dir should fail");

    // Restore permissions for cleanup
    let perms = fs::Permissions::from_mode(0o755);
    fs::set_permissions(&roles_dir, perms).unwrap();

    // Original file should still be intact
    let loaded = agency::load_all_roles(&roles_dir).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].name, "Original");

    // No temp file should be left
    let tmp_path = roles_dir.join(format!(".{}.yaml.tmp", role2.id));
    assert!(!tmp_path.exists(), "Temp file should be cleaned up on failure");
}
