use std::process::Command;

const FIXTURE: &str = "tests/fixtures/agency-starter-sample.csv";

#[test]
fn test_agency_csv_byte_equal_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let wg_dir = tmp.path().join(".wg");
    std::fs::create_dir_all(&wg_dir).unwrap();

    let import = Command::new(env!("CARGO_BIN_EXE_wg"))
        .args([
            "--dir",
            wg_dir.to_str().unwrap(),
            "agency",
            "import",
            "--format",
            "agency-csv",
            FIXTURE,
        ])
        .output()
        .unwrap();
    assert!(
        import.status.success(),
        "agency import failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&import.stdout),
        String::from_utf8_lossy(&import.stderr)
    );

    let export = Command::new(env!("CARGO_BIN_EXE_wg"))
        .args([
            "--dir",
            wg_dir.to_str().unwrap(),
            "agency",
            "export",
            "--format",
            "agency-csv",
            "-",
        ])
        .output()
        .unwrap();
    assert!(
        export.status.success(),
        "agency export failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&export.stdout),
        String::from_utf8_lossy(&export.stderr)
    );

    let expected = std::fs::read(FIXTURE).unwrap();
    assert_eq!(export.stdout, expected);
}

#[test]
fn test_agency_csv_import_reads_lineage_columns() {
    let tmp = tempfile::tempdir().unwrap();
    let wg_dir = tmp.path().join(".wg");
    std::fs::create_dir_all(&wg_dir).unwrap();

    let import = Command::new(env!("CARGO_BIN_EXE_wg"))
        .args([
            "--dir",
            wg_dir.to_str().unwrap(),
            "agency",
            "import",
            "--format",
            "agency-csv",
            FIXTURE,
        ])
        .output()
        .unwrap();
    assert!(import.status.success());

    let components_dir = wg_dir.join("agency/primitives/components");
    let mut found = false;
    for entry in std::fs::read_dir(components_dir).unwrap() {
        let path = entry.unwrap().path();
        let value: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        if value["name"].as_str() == Some("Identify Gaps") {
            found = true;
            assert_eq!(value["lineage"]["generation"].as_u64(), Some(1));
            assert_eq!(value["lineage"]["created_by"].as_str(), Some("human"));
            let parent_ids = value["lineage"]["parent_ids"].as_sequence().unwrap();
            assert_eq!(parent_ids[0].as_str(), Some("pch-gaps"));
            assert_eq!(parent_ids[1].as_str(), Some("root-beta"));
            assert_eq!(value["domain_tags"][0].as_str(), Some("analysis"));
            assert_eq!(value["domain_tags"][1].as_str(), Some("review"));
        }
    }
    assert!(found, "Identify Gaps component was not imported");
}
