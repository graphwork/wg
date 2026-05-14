//! Integration tests for `wg agency import` command.
//!
//! Exercises the full CLI import pipeline end-to-end, using a tempdir for
//! isolation. Covers: fixture import, idempotency, dry-run, invalid CSV,
//! YAML field validation, and provenance tagging.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn wg_binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("could not get current exe path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("wg");
    assert!(
        path.exists(),
        "wg binary not found at {:?}. Run `cargo build` first.",
        path
    );
    path
}

fn wg_cmd(wg_dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(wg_binary())
        .arg("--dir")
        .arg(wg_dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|e| panic!("Failed to run wg {:?}: {}", args, e))
}

fn wg_ok(wg_dir: &Path, args: &[&str]) -> String {
    let output = wg_cmd(wg_dir, args);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        output.status.success(),
        "wg {:?} failed.\nstdout: {}\nstderr: {}",
        args,
        stdout,
        stderr
    );
    stdout
}

/// Write a fixture CSV with: 2 skills, 1 outcome, 1 tradeoff, 1 unknown type.
fn write_fixture_csv(dir: &Path) -> PathBuf {
    let csv_path = dir.join("agency_fixture.csv");
    let mut f = fs::File::create(&csv_path).unwrap();
    writeln!(
        f,
        "type,name,description,col4,col5,domain_tags,quality_score"
    )
    .unwrap();
    // Skill 1
    writeln!(
        f,
        "skill,Code Review,Reviews code for correctness and style,Translated,Reviews code for correctness and style,programming,0.85"
    )
    .unwrap();
    // Skill 2
    writeln!(
        f,
        "skill,Test Writing,Writes comprehensive test suites,Translated,Writes comprehensive test suites,testing,0.80"
    )
    .unwrap();
    // Outcome 1
    write!(
        f,
        "outcome,Working Code,Code compiles and passes tests,,\"All tests pass\nNo compiler warnings\",programming,0.90\n"
    )
    .unwrap();
    // Tradeoff 1
    writeln!(
        f,
        "tradeoff,Speed vs Quality,Balances speed and quality,Fast execution,Incomplete analysis,general,0.75"
    )
    .unwrap();
    // Unknown type — should be skipped
    writeln!(f, "persona,Ghost Agent,A mysterious entity,x,y,misc,0.50").unwrap();
    csv_path
}

fn count_yaml_files(dir: &Path) -> usize {
    if !dir.exists() {
        return 0;
    }
    fs::read_dir(dir)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .ok()
                .map(|e| e.path().extension().map(|x| x == "yaml").unwrap_or(false))
                .unwrap_or(false)
        })
        .count()
}

// ---------------------------------------------------------------------------
// Test 1: Import fixture CSV → verify correct file count in primitives dirs
// ---------------------------------------------------------------------------
#[test]
fn test_import_creates_correct_file_counts() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();

    let csv_path = write_fixture_csv(tmp.path());

    let stdout = wg_ok(&wg_dir, &["agency", "import", csv_path.to_str().unwrap()]);

    // Verify output mentions counts
    assert!(
        stdout.contains("Components: 2"),
        "Expected 2 components in output, got:\n{}",
        stdout
    );
    assert!(
        stdout.contains("Outcomes:   1"),
        "Expected 1 outcome in output, got:\n{}",
        stdout
    );
    assert!(
        stdout.contains("Tradeoffs:  1"),
        "Expected 1 tradeoff in output, got:\n{}",
        stdout
    );
    assert!(
        stdout.contains("Skipped:    1"),
        "Expected 1 skipped in output, got:\n{}",
        stdout
    );

    // Verify file counts on disk
    let components_dir = wg_dir.join("agency/primitives/components");
    let outcomes_dir = wg_dir.join("agency/primitives/outcomes");
    let tradeoffs_dir = wg_dir.join("agency/primitives/tradeoffs");

    assert_eq!(
        count_yaml_files(&components_dir),
        2,
        "Expected 2 component YAML files"
    );
    assert_eq!(
        count_yaml_files(&outcomes_dir),
        1,
        "Expected 1 outcome YAML file"
    );
    assert_eq!(
        count_yaml_files(&tradeoffs_dir),
        1,
        "Expected 1 tradeoff YAML file"
    );
}

// ---------------------------------------------------------------------------
// Test 2: Re-import same CSV → verify no duplicates (file count unchanged)
// ---------------------------------------------------------------------------
#[test]
fn test_reimport_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();

    let csv_path = write_fixture_csv(tmp.path());

    // First import
    wg_ok(&wg_dir, &["agency", "import", csv_path.to_str().unwrap()]);

    let components_dir = wg_dir.join("agency/primitives/components");
    let outcomes_dir = wg_dir.join("agency/primitives/outcomes");
    let tradeoffs_dir = wg_dir.join("agency/primitives/tradeoffs");

    let comp_count_1 = count_yaml_files(&components_dir);
    let out_count_1 = count_yaml_files(&outcomes_dir);
    let trade_count_1 = count_yaml_files(&tradeoffs_dir);

    // Second import — same CSV
    wg_ok(&wg_dir, &["agency", "import", csv_path.to_str().unwrap()]);

    assert_eq!(
        count_yaml_files(&components_dir),
        comp_count_1,
        "Re-import should not create duplicate components"
    );
    assert_eq!(
        count_yaml_files(&outcomes_dir),
        out_count_1,
        "Re-import should not create duplicate outcomes"
    );
    assert_eq!(
        count_yaml_files(&tradeoffs_dir),
        trade_count_1,
        "Re-import should not create duplicate tradeoffs"
    );
}

// ---------------------------------------------------------------------------
// Test 3: Dry-run mode → verify no files written
// ---------------------------------------------------------------------------
#[test]
fn test_dry_run_writes_no_files() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();

    let csv_path = write_fixture_csv(tmp.path());

    let stdout = wg_ok(
        &wg_dir,
        &["agency", "import", csv_path.to_str().unwrap(), "--dry-run"],
    );

    // Output should mention "(dry run)"
    assert!(
        stdout.contains("(dry run)"),
        "Dry run output should mention '(dry run)', got:\n{}",
        stdout
    );

    // No primitives directories should be created
    let agency_dir = wg_dir.join("agency");
    let components_dir = agency_dir.join("primitives/components");
    let outcomes_dir = agency_dir.join("primitives/outcomes");
    let tradeoffs_dir = agency_dir.join("primitives/tradeoffs");

    assert_eq!(
        count_yaml_files(&components_dir),
        0,
        "Dry run should not create component files"
    );
    assert_eq!(
        count_yaml_files(&outcomes_dir),
        0,
        "Dry run should not create outcome files"
    );
    assert_eq!(
        count_yaml_files(&tradeoffs_dir),
        0,
        "Dry run should not create tradeoff files"
    );
}

// ---------------------------------------------------------------------------
// Test 4: Invalid CSV (missing columns) → verify error message
// ---------------------------------------------------------------------------
#[test]
fn test_invalid_csv_missing_columns() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();

    // Write a CSV with only 2 columns instead of the expected 7
    let bad_csv = tmp.path().join("bad.csv");
    fs::write(&bad_csv, "type,name\nskill,Only Name\n").unwrap();

    // The import should still succeed — missing columns get empty defaults
    // But the imported component will have empty description/content
    let output = wg_cmd(&wg_dir, &["agency", "import", bad_csv.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();

    // It should process without crashing (missing fields default to "")
    assert!(
        output.status.success(),
        "Import of sparse CSV should succeed (missing fields default to empty), stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("Components: 1"),
        "Should import the skill row, got:\n{}",
        stdout
    );
}

#[test]
fn test_invalid_csv_nonexistent_file() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();

    let output = wg_cmd(&wg_dir, &["agency", "import", "/nonexistent/path/data.csv"]);

    assert!(
        !output.status.success(),
        "Import of nonexistent file should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        stderr.contains("Failed to read") || stderr.contains("No such file"),
        "Error should mention file read failure, got:\n{}",
        stderr
    );
}

// ---------------------------------------------------------------------------
// Test 5: Verify component YAML has correct fields
// ---------------------------------------------------------------------------
#[test]
fn test_imported_yaml_has_correct_fields() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();

    let csv_path = write_fixture_csv(tmp.path());

    wg_ok(&wg_dir, &["agency", "import", csv_path.to_str().unwrap()]);

    // Check a component YAML
    let components_dir = wg_dir.join("agency/primitives/components");
    let component_files: Vec<_> = fs::read_dir(&components_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "yaml").unwrap_or(false))
        .collect();
    assert!(!component_files.is_empty(), "Should have component files");

    for entry in &component_files {
        let content = fs::read_to_string(entry.path()).unwrap();
        let yaml: serde_yaml::Value = serde_yaml::from_str(&content).unwrap();
        let map = yaml.as_mapping().expect("YAML should be a mapping");

        // Required fields: name, description, category, lineage
        assert!(
            map.contains_key(serde_yaml::Value::String("name".to_string())),
            "Component YAML should have 'name' field. File: {:?}\nContent:\n{}",
            entry.path(),
            content
        );
        assert!(
            map.contains_key(serde_yaml::Value::String("description".to_string())),
            "Component YAML should have 'description' field. File: {:?}",
            entry.path()
        );
        assert!(
            map.contains_key(serde_yaml::Value::String("category".to_string())),
            "Component YAML should have 'category' field. File: {:?}",
            entry.path()
        );
        assert!(
            map.contains_key(serde_yaml::Value::String("lineage".to_string())),
            "Component YAML should have 'lineage' field. File: {:?}",
            entry.path()
        );

        // Verify category is "translated" (all fixture skills are)
        let category = map
            .get(serde_yaml::Value::String("category".to_string()))
            .unwrap();
        assert_eq!(
            category.as_str().unwrap(),
            "translated",
            "Imported skill should have category 'translated'"
        );
    }

    // Check an outcome YAML
    let outcomes_dir = wg_dir.join("agency/primitives/outcomes");
    let outcome_files: Vec<_> = fs::read_dir(&outcomes_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "yaml").unwrap_or(false))
        .collect();
    assert!(!outcome_files.is_empty(), "Should have outcome files");

    for entry in &outcome_files {
        let content = fs::read_to_string(entry.path()).unwrap();
        let yaml: serde_yaml::Value = serde_yaml::from_str(&content).unwrap();
        let map = yaml.as_mapping().expect("YAML should be a mapping");

        assert!(
            map.contains_key(serde_yaml::Value::String("name".to_string())),
            "Outcome YAML should have 'name' field"
        );
        assert!(
            map.contains_key(serde_yaml::Value::String("description".to_string())),
            "Outcome YAML should have 'description' field"
        );
        assert!(
            map.contains_key(serde_yaml::Value::String("lineage".to_string())),
            "Outcome YAML should have 'lineage' field"
        );

        // Verify lineage has created_by field
        let lineage = map
            .get(serde_yaml::Value::String("lineage".to_string()))
            .unwrap();
        let lineage_map = lineage.as_mapping().expect("lineage should be a mapping");
        assert!(
            lineage_map.contains_key(serde_yaml::Value::String("created_by".to_string())),
            "Lineage should have 'created_by' field"
        );
    }

    // Check a tradeoff YAML
    let tradeoffs_dir = wg_dir.join("agency/primitives/tradeoffs");
    let tradeoff_files: Vec<_> = fs::read_dir(&tradeoffs_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "yaml").unwrap_or(false))
        .collect();
    assert!(!tradeoff_files.is_empty(), "Should have tradeoff files");

    for entry in &tradeoff_files {
        let content = fs::read_to_string(entry.path()).unwrap();
        let yaml: serde_yaml::Value = serde_yaml::from_str(&content).unwrap();
        let map = yaml.as_mapping().expect("YAML should be a mapping");

        assert!(
            map.contains_key(serde_yaml::Value::String("name".to_string())),
            "Tradeoff YAML should have 'name' field"
        );
        assert!(
            map.contains_key(serde_yaml::Value::String("description".to_string())),
            "Tradeoff YAML should have 'description' field"
        );
        assert!(
            map.contains_key(serde_yaml::Value::String("lineage".to_string())),
            "Tradeoff YAML should have 'lineage' field"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 6: Provenance tag → verify access_control.owner matches
// ---------------------------------------------------------------------------
#[test]
fn test_custom_provenance_tag() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();

    let csv_path = write_fixture_csv(tmp.path());

    wg_ok(
        &wg_dir,
        &[
            "agency",
            "import",
            csv_path.to_str().unwrap(),
            "--tag",
            "custom-source",
        ],
    );

    // Check that all primitives have access_control.owner == "custom-source"
    let dirs = [
        wg_dir.join("agency/primitives/components"),
        wg_dir.join("agency/primitives/outcomes"),
        wg_dir.join("agency/primitives/tradeoffs"),
    ];

    let mut checked = 0;
    for dir in &dirs {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry
                .path()
                .extension()
                .map(|x| x == "yaml")
                .unwrap_or(false)
            {
                let content = fs::read_to_string(entry.path()).unwrap();
                let yaml: serde_yaml::Value = serde_yaml::from_str(&content).unwrap();
                let map = yaml.as_mapping().unwrap();

                let access = map
                    .get(serde_yaml::Value::String("access_control".to_string()))
                    .unwrap_or_else(|| {
                        panic!(
                            "Missing access_control in {:?}\nContent:\n{}",
                            entry.path(),
                            content
                        )
                    });
                let owner = access
                    .as_mapping()
                    .unwrap()
                    .get(serde_yaml::Value::String("owner".to_string()))
                    .unwrap_or_else(|| {
                        panic!(
                            "Missing access_control.owner in {:?}\nContent:\n{}",
                            entry.path(),
                            content
                        )
                    });

                assert_eq!(
                    owner.as_str().unwrap(),
                    "custom-source",
                    "access_control.owner should be 'custom-source' in {:?}",
                    entry.path()
                );

                // Custom provenance remains on access_control.owner; lineage.created_by
                // is constrained to the agency v1.2.4 enum domain.
                let lineage = map
                    .get(serde_yaml::Value::String("lineage".to_string()))
                    .unwrap();
                let created_by = lineage
                    .as_mapping()
                    .unwrap()
                    .get(serde_yaml::Value::String("created_by".to_string()))
                    .unwrap();
                assert_eq!(created_by.as_str().unwrap(), "import");

                checked += 1;
            }
        }
    }

    assert!(
        checked >= 4,
        "Should have checked at least 4 YAML files (2 components + 1 outcome + 1 tradeoff), checked {}",
        checked
    );
}
