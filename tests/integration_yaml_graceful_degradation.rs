//! Tests for graceful degradation when loading corrupt YAML cache files.
//!
//! Verifies that `load_all_*` functions skip individual corrupt files
//! with warnings instead of failing the entire operation.

use std::fs;
use tempfile::TempDir;

use worksgood::agency;

/// Helper: create a valid role YAML file in the given directory.
/// Uses `name` as a fake outcome_id to produce distinct content hashes.
fn write_valid_role(dir: &std::path::Path, name: &str) -> String {
    let role = agency::build_role(name, "A test role", vec![], name);
    agency::save_role(&role, dir).unwrap();
    role.id
}

/// Helper: write a corrupt YAML file in the given directory.
fn write_corrupt_yaml(dir: &std::path::Path, filename: &str, content: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join(format!("{}.yaml", filename)), content).unwrap();
}

#[test]
fn test_evolve_graceful_skip_one_corrupt_role() {
    let tmp = TempDir::new().unwrap();
    let roles_dir = tmp.path().join("cache/roles");

    // Create two valid roles
    let id1 = write_valid_role(&roles_dir, "ValidRole1");
    let id2 = write_valid_role(&roles_dir, "ValidRole2");

    // Inject a corrupt YAML file
    write_corrupt_yaml(&roles_dir, "corrupt-role", "name: Bad\ninvalid yaml: [\n");

    // load_all_roles should succeed, returning only the two valid roles
    let roles = agency::load_all_roles(&roles_dir).unwrap();
    assert_eq!(roles.len(), 2);
    let ids: Vec<&str> = roles.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&id1.as_str()));
    assert!(ids.contains(&id2.as_str()));
}

#[test]
fn test_evolve_graceful_all_corrupt_fails() {
    let tmp = TempDir::new().unwrap();
    let roles_dir = tmp.path().join("cache/roles");

    // Only corrupt files, no valid ones
    write_corrupt_yaml(&roles_dir, "bad1", "invalid: [\n");
    write_corrupt_yaml(&roles_dir, "bad2", "also broken: {{{\n");

    // load_all_roles should fail when ALL files are corrupt
    let result = agency::load_all_roles(&roles_dir);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("corrupt"),
        "Error should mention corruption: {}",
        err_msg
    );
}

#[test]
fn test_evolve_graceful_empty_dir_still_ok() {
    let tmp = TempDir::new().unwrap();
    let roles_dir = tmp.path().join("cache/roles");
    fs::create_dir_all(&roles_dir).unwrap();

    // Empty directory should return Ok with empty vec (no change from before)
    let roles = agency::load_all_roles(&roles_dir).unwrap();
    assert!(roles.is_empty());
}

#[test]
fn test_evolve_graceful_nonexistent_dir_still_ok() {
    let tmp = TempDir::new().unwrap();
    let roles_dir = tmp.path().join("cache/roles");

    // Non-existent directory should return Ok with empty vec
    let roles = agency::load_all_roles(&roles_dir).unwrap();
    assert!(roles.is_empty());
}

#[test]
fn test_evolve_graceful_corrupt_tradeoff_skipped() {
    let tmp = TempDir::new().unwrap();
    let tradeoffs_dir = tmp.path().join("primitives/tradeoffs");

    // Create a valid tradeoff
    let tradeoff = agency::build_tradeoff("GoodTradeoff", "A valid one", vec![], vec![]);
    agency::save_tradeoff(&tradeoff, &tradeoffs_dir).unwrap();

    // Inject corrupt file
    write_corrupt_yaml(
        &tradeoffs_dir,
        "corrupt-tradeoff",
        "broken timestamp: 2026-04-01T15:30:\n45.123Z\n",
    );

    let tradeoffs = agency::load_all_tradeoffs(&tradeoffs_dir).unwrap();
    assert_eq!(tradeoffs.len(), 1);
    assert_eq!(tradeoffs[0].id, tradeoff.id);
}

#[test]
fn test_evolve_graceful_truncated_yaml_skipped() {
    let tmp = TempDir::new().unwrap();
    let roles_dir = tmp.path().join("cache/roles");

    // Create a valid role
    let id = write_valid_role(&roles_dir, "SurvivorRole");

    // Simulate a truncated write (partial YAML)
    write_corrupt_yaml(
        &roles_dir,
        "truncated-role",
        "id: some-id\nname: Truncated\ndescription: This file is trun",
    );

    let roles = agency::load_all_roles(&roles_dir).unwrap();
    assert_eq!(roles.len(), 1);
    assert_eq!(roles[0].id, id);
}
