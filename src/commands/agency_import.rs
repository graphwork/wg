use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use workgraph::agency::{
    self, AccessControl, AccessPolicy, ComponentCategory, ContentRef, DesiredOutcome, Lineage,
    PerformanceRecord, RoleComponent, TradeoffConfig,
};

/// Counts of primitives imported from a CSV file.
#[derive(Debug, Clone, Default)]
pub struct ImportCounts {
    pub role_components: u32,
    pub desired_outcomes: u32,
    pub trade_off_configs: u32,
    pub skipped: u32,
}

/// Provenance manifest written after a successful CSV import.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImportManifest {
    pub source: String,
    pub version: String,
    pub imported_at: String,
    pub counts: ManifestCounts,
    pub content_hash: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ManifestCounts {
    pub role_components: u32,
    pub desired_outcomes: u32,
    pub trade_off_configs: u32,
}

/// Path to the import manifest within the workgraph agency directory.
pub fn manifest_path(workgraph_dir: &Path) -> std::path::PathBuf {
    workgraph_dir.join("agency/import-manifest.yaml")
}

/// Compute SHA-256 hex digest of file contents.
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// Write (or update) the import manifest after a successful import.
pub fn write_manifest(
    workgraph_dir: &Path,
    source: &str,
    csv_content: &[u8],
    counts: &ImportCounts,
) -> Result<()> {
    let manifest = ImportManifest {
        source: source.to_string(),
        version: format!("v{}", env!("CARGO_PKG_VERSION")),
        imported_at: chrono::Utc::now().to_rfc3339(),
        counts: ManifestCounts {
            role_components: counts.role_components,
            desired_outcomes: counts.desired_outcomes,
            trade_off_configs: counts.trade_off_configs,
        },
        content_hash: sha256_hex(csv_content),
    };
    let path = manifest_path(workgraph_dir);
    std::fs::write(&path, serde_yaml::to_string(&manifest)?)
        .context("Failed to write import manifest")?;
    Ok(())
}

/// Detected CSV format based on header or column count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CsvFormat {
    /// Old 7-column format: type,name,description,col4,col5,domain_tags,quality_score
    Legacy,
    /// Agency 9-column format: type,name,description,quality,domain_specificity,domain,origin_instance_id,parent_content_hash,scope
    Agency,
}

/// Detect the CSV format from the header row.
fn detect_format(headers: &csv::StringRecord) -> CsvFormat {
    // Check by column count first
    if headers.len() >= 9 {
        return CsvFormat::Agency;
    }
    // Also check by header names for explicit detection
    if let Some(col3) = headers.get(3) {
        let col3 = col3.trim().to_lowercase();
        if col3 == "quality" || col3 == "domain_specificity" {
            return CsvFormat::Agency;
        }
    }
    CsvFormat::Legacy
}

/// `wg agency import <csv-path>` -- import Agency's starter.csv primitives into WorkGraph.
///
/// Supports two CSV formats:
///
/// **Legacy (7 columns):** type,name,description,col4,col5,domain_tags,quality_score
///   - type: skill | outcome | tradeoff
///
/// **Agency (9 columns):** type,name,description,quality,domain_specificity,domain,origin_instance_id,parent_content_hash,scope
///   - type: role_component | desired_outcome | trade_off_config
///
/// Both formats are auto-detected. Legacy type names (skill/outcome/tradeoff) are also
/// accepted in the 9-column format and vice versa.
pub fn run(workgraph_dir: &Path, csv_path: &str, dry_run: bool, tag: Option<&str>) -> Result<ImportCounts> {
    let provenance_tag = tag.unwrap_or("agency-import");
    let agency_dir = workgraph_dir.join("agency");

    if !dry_run {
        agency::init(&agency_dir).context("Failed to initialize agency directory")?;
    }

    let csv_bytes = std::fs::read(csv_path)
        .with_context(|| format!("Failed to read '{}'", csv_path))?;
    let csv_content = String::from_utf8_lossy(&csv_bytes);
    let mut reader = csv::Reader::from_reader(csv_content.as_bytes());

    let format = detect_format(reader.headers().context("Failed to read CSV headers")?);

    let mut components_count = 0u32;
    let mut outcomes_count = 0u32;
    let mut tradeoffs_count = 0u32;
    let mut skipped = 0u32;

    for (row_idx, record) in reader.records().enumerate() {
        let record = record.with_context(|| format!("Failed to parse CSV row {}", row_idx + 1))?;

        let ptype = record.get(0).unwrap_or("").trim();
        let name = record.get(1).unwrap_or("").trim().to_string();
        let description = record.get(2).unwrap_or("").trim().to_string();

        // Parse format-specific columns
        let (quality_score, domain_tags, metadata, parent_content_hash) = match format {
            CsvFormat::Agency => parse_agency_columns(&record),
            CsvFormat::Legacy => parse_legacy_columns(&record),
        };

        let mut parent_ids = vec![];
        if let Some(ref pch) = parent_content_hash {
            if !pch.is_empty() {
                parent_ids.push(pch.clone());
            }
        }

        let lineage = Lineage {
            parent_ids,
            generation: 0,
            created_by: format!("{}-v{}", provenance_tag, env!("CARGO_PKG_VERSION")),
            created_at: chrono::Utc::now(),
        };

        let access_control = AccessControl {
            owner: provenance_tag.to_string(),
            policy: AccessPolicy::Open,
        };

        let performance = PerformanceRecord {
            task_count: 0,
            avg_score: quality_score,
            evaluations: vec![],
        };

        // Normalize type names: accept both old and new format names
        let normalized_type = match ptype {
            "skill" | "role_component" => "component",
            "outcome" | "desired_outcome" => "outcome",
            "tradeoff" | "trade_off_config" => "tradeoff",
            other => other,
        };

        match normalized_type {
            "component" => {
                let content = ContentRef::Inline(description.clone());
                let category = ComponentCategory::Translated;
                let id = agency::content_hash_component(&description, &category, &content);

                if dry_run {
                    println!("  [component] {} ({})", name, agency::short_hash(&id));
                } else {
                    let component = RoleComponent {
                        id: id.clone(),
                        name,
                        description,
                        category,
                        content,
                        performance,
                        lineage,
                        access_control,
                        domain_tags,
                        metadata,
                        former_agents: vec![],
                        former_deployments: vec![],
                    };
                    let dir = agency_dir.join("primitives/components");
                    agency::save_component(&component, &dir).with_context(|| {
                        format!("Failed to save component {}", agency::short_hash(&id))
                    })?;
                }
                components_count += 1;
            }
            "outcome" => {
                // In legacy format, col5 contains newline-separated success criteria.
                // In Agency format, there's no separate criteria column; use description as-is.
                let success_criteria = match format {
                    CsvFormat::Legacy => {
                        let col5 = record.get(4).unwrap_or("").trim().to_string();
                        col5.split('\n')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect()
                    }
                    CsvFormat::Agency => vec![],
                };
                let id = agency::content_hash_outcome(&description, &success_criteria);

                if dry_run {
                    println!("  [outcome] {} ({})", name, agency::short_hash(&id));
                } else {
                    let outcome = DesiredOutcome {
                        id: id.clone(),
                        name,
                        description,
                        success_criteria,
                        performance,
                        lineage,
                        access_control,
                        requires_human_oversight: true,
                        domain_tags,
                        metadata,
                        former_agents: vec![],
                        former_deployments: vec![],
                    };
                    let dir = agency_dir.join("primitives/outcomes");
                    agency::save_outcome(&outcome, &dir).with_context(|| {
                        format!("Failed to save outcome {}", agency::short_hash(&id))
                    })?;
                }
                outcomes_count += 1;
            }
            "tradeoff" => {
                // In legacy format, col4=acceptable tradeoffs, col5=unacceptable tradeoffs (comma-separated).
                // In Agency format, the description is a single coherent trade-off statement;
                // we store it as the description and use the description as a single acceptable entry.
                let (acceptable, unacceptable) = match format {
                    CsvFormat::Legacy => {
                        let col4 = record.get(3).unwrap_or("").trim().to_string();
                        let col5 = record.get(4).unwrap_or("").trim().to_string();
                        let acc: Vec<String> = col4
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                        let unacc: Vec<String> = col5
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                        (acc, unacc)
                    }
                    CsvFormat::Agency => {
                        // Description is the complete trade-off statement.
                        // Don't split into acceptable/unacceptable lists.
                        (vec![], vec![])
                    }
                };
                let id = agency::content_hash_tradeoff(&acceptable, &unacceptable, &description);

                if dry_run {
                    println!("  [tradeoff] {} ({})", name, agency::short_hash(&id));
                } else {
                    let tradeoff = TradeoffConfig {
                        id: id.clone(),
                        name,
                        description,
                        acceptable_tradeoffs: acceptable,
                        unacceptable_tradeoffs: unacceptable,
                        performance,
                        lineage,
                        access_control,
                        domain_tags,
                        metadata,
                        former_agents: vec![],
                        former_deployments: vec![],
                    };
                    let dir = agency_dir.join("primitives/tradeoffs");
                    agency::save_tradeoff(&tradeoff, &dir).with_context(|| {
                        format!("Failed to save tradeoff {}", agency::short_hash(&id))
                    })?;
                }
                tradeoffs_count += 1;
            }
            _ => {
                skipped += 1;
                if !ptype.is_empty() {
                    eprintln!(
                        "Warning: skipping unknown type '{}' for '{}' (row {})",
                        ptype,
                        name,
                        row_idx + 1
                    );
                }
            }
        }
    }

    let counts = ImportCounts {
        role_components: components_count,
        desired_outcomes: outcomes_count,
        trade_off_configs: tradeoffs_count,
        skipped,
    };

    let mode = if dry_run { " (dry run)" } else { "" };
    println!("Agency import complete{}:", mode);
    println!("  Components: {}", components_count);
    println!("  Outcomes:   {}", outcomes_count);
    println!("  Tradeoffs:  {}", tradeoffs_count);
    if skipped > 0 {
        println!("  Skipped:    {}", skipped);
    }

    // Write/update the provenance manifest on non-dry-run imports
    if !dry_run {
        write_manifest(workgraph_dir, csv_path, &csv_bytes, &counts)?;
    }

    Ok(counts)
}

/// Parse columns from Agency's 9-column CSV format.
///
/// Columns: type(0), name(1), description(2), quality(3), domain_specificity(4),
///          domain(5), origin_instance_id(6), parent_content_hash(7), scope(8)
fn parse_agency_columns(
    record: &csv::StringRecord,
) -> (Option<f64>, Vec<String>, HashMap<String, String>, Option<String>) {
    // quality (col3): integer 0-100, map to avg_score as 0.0-1.0
    let quality_score: Option<f64> = record.get(3).and_then(|s| {
        let s = s.trim();
        s.parse::<f64>().ok().map(|v| v / 100.0)
    });

    // domain_specificity (col4): store as metadata
    let domain_specificity = record
        .get(4)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    // domain (col5): comma-separated tags
    let domain_tags: Vec<String> = record
        .get(5)
        .map(|s| {
            s.split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();

    // origin_instance_id (col6): store as metadata
    let origin_instance_id = record
        .get(6)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    // parent_content_hash (col7): store in lineage.parent_ids
    let parent_content_hash = record.get(7).map(|s| s.trim().to_string());

    // scope (col8): store as metadata
    let scope = record
        .get(8)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let mut metadata = HashMap::new();
    if !scope.is_empty() {
        metadata.insert("scope".to_string(), scope);
    }
    if !domain_specificity.is_empty() {
        metadata.insert("domain_specificity".to_string(), domain_specificity);
    }
    if !origin_instance_id.is_empty() {
        metadata.insert("origin_instance_id".to_string(), origin_instance_id);
    }
    if let Some(ref pch) = parent_content_hash {
        if !pch.is_empty() {
            metadata.insert("parent_content_hash".to_string(), pch.clone());
        }
    }

    (quality_score, domain_tags, metadata, parent_content_hash)
}

/// Parse columns from the legacy 7-column CSV format.
///
/// Columns: type(0), name(1), description(2), col4(3), col5(4), domain_tags(5), quality_score(6)
fn parse_legacy_columns(
    record: &csv::StringRecord,
) -> (Option<f64>, Vec<String>, HashMap<String, String>, Option<String>) {
    let quality_score: Option<f64> = record.get(6).and_then(|s| s.trim().parse().ok());

    let domain_tags: Vec<String> = record
        .get(5)
        .map(|s| {
            s.split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();

    (quality_score, domain_tags, HashMap::new(), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_fixture_csv(dir: &Path) -> std::path::PathBuf {
        let csv_path = dir.join("test_agency.csv");
        let mut f = std::fs::File::create(&csv_path).unwrap();
        writeln!(
            f,
            "type,name,description,col4,col5,domain_tags,quality_score"
        )
        .unwrap();
        writeln!(
            f,
            "skill,Code Review,Reviews code for correctness and style,Translated,Reviews code for correctness and style,programming,0.85"
        )
        .unwrap();
        // Use quoted field with literal newline for success criteria
        write!(
            f,
            "outcome,Working Code,Code compiles and passes tests,,\"All tests pass\nNo compiler warnings\",programming,0.90\n"
        )
        .unwrap();
        writeln!(
            f,
            "tradeoff,Speed vs Quality,Balances speed and quality,Fast execution,Incomplete analysis,general,0.75"
        )
        .unwrap();
        csv_path
    }

    fn write_agency_format_csv(dir: &Path) -> std::path::PathBuf {
        let csv_path = dir.join("test_agency_9col.csv");
        let mut f = std::fs::File::create(&csv_path).unwrap();
        writeln!(
            f,
            "type,name,description,quality,domain_specificity,domain,origin_instance_id,parent_content_hash,scope"
        )
        .unwrap();
        writeln!(
            f,
            "role_component,Identify Gaps,Identify gaps and errors in provided content,85,high,\"analysis,review\",inst-001,abc123,task"
        )
        .unwrap();
        writeln!(
            f,
            "desired_outcome,Accurate Analysis,Analysis is thorough and identifies all issues,90,medium,analysis,inst-002,,task"
        )
        .unwrap();
        writeln!(
            f,
            "trade_off_config,Prefer Depth,When depth and breadth conflict: prefer deeper analysis of fewer items over shallow coverage of many,70,low,\"analysis,research\",inst-003,def456,task"
        )
        .unwrap();
        // A meta-scope primitive
        writeln!(
            f,
            "role_component,Assign by Expertise,Match agent skills to task requirements using domain tags,80,high,meta,inst-004,,meta:assigner"
        )
        .unwrap();
        csv_path
    }

    #[test]
    fn test_agency_import_parses_csv() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".workgraph");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = write_fixture_csv(tmp.path());
        run(&wg_dir, csv_path.to_str().unwrap(), false, None).unwrap();

        // Verify files were created
        let components_dir = wg_dir.join("agency/primitives/components");
        let outcomes_dir = wg_dir.join("agency/primitives/outcomes");
        let tradeoffs_dir = wg_dir.join("agency/primitives/tradeoffs");

        let comp_count = std::fs::read_dir(&components_dir).unwrap().count();
        let out_count = std::fs::read_dir(&outcomes_dir).unwrap().count();
        let trade_count = std::fs::read_dir(&tradeoffs_dir).unwrap().count();

        assert_eq!(comp_count, 1, "Expected 1 component");
        assert_eq!(out_count, 1, "Expected 1 outcome");
        assert_eq!(trade_count, 1, "Expected 1 tradeoff");
    }

    #[test]
    fn test_agency_import_dry_run_no_files() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".workgraph");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = write_fixture_csv(tmp.path());
        run(&wg_dir, csv_path.to_str().unwrap(), true, None).unwrap();

        // Agency dir should not have been created (or should be empty)
        let agency_dir = wg_dir.join("agency");
        assert!(
            !agency_dir.exists() || !agency_dir.join("primitives/components").exists(),
            "Dry run should not create files"
        );
    }

    #[test]
    fn test_agency_import_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".workgraph");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = write_fixture_csv(tmp.path());

        // Import twice
        run(&wg_dir, csv_path.to_str().unwrap(), false, None).unwrap();
        run(&wg_dir, csv_path.to_str().unwrap(), false, None).unwrap();

        // Same count -- content hashing deduplicates
        let comp_count = std::fs::read_dir(wg_dir.join("agency/primitives/components"))
            .unwrap()
            .count();
        assert_eq!(comp_count, 1, "Re-import should not create duplicates");
    }

    #[test]
    fn test_agency_import_content_hash_stability() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".workgraph");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = write_fixture_csv(tmp.path());
        run(&wg_dir, csv_path.to_str().unwrap(), false, None).unwrap();

        // Record file names (which are content hashes)
        let components_dir = wg_dir.join("agency/primitives/components");
        let names1: Vec<String> = std::fs::read_dir(&components_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();

        // Import again into a fresh dir
        let tmp2 = tempfile::tempdir().unwrap();
        let wg_dir2 = tmp2.path().join(".workgraph");
        std::fs::create_dir_all(&wg_dir2).unwrap();
        run(&wg_dir2, csv_path.to_str().unwrap(), false, None).unwrap();

        let components_dir2 = wg_dir2.join("agency/primitives/components");
        let names2: Vec<String> = std::fs::read_dir(&components_dir2)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();

        assert_eq!(
            names1, names2,
            "Content hashes should be stable across imports"
        );
    }

    #[test]
    fn test_agency_import_9col_format() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".workgraph");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = write_agency_format_csv(tmp.path());
        run(&wg_dir, csv_path.to_str().unwrap(), false, None).unwrap();

        // Verify files were created: 2 components, 1 outcome, 1 tradeoff
        let components_dir = wg_dir.join("agency/primitives/components");
        let outcomes_dir = wg_dir.join("agency/primitives/outcomes");
        let tradeoffs_dir = wg_dir.join("agency/primitives/tradeoffs");

        let comp_count = std::fs::read_dir(&components_dir).unwrap().count();
        let out_count = std::fs::read_dir(&outcomes_dir).unwrap().count();
        let trade_count = std::fs::read_dir(&tradeoffs_dir).unwrap().count();

        assert_eq!(comp_count, 2, "Expected 2 components (task + meta:assigner)");
        assert_eq!(out_count, 1, "Expected 1 outcome");
        assert_eq!(trade_count, 1, "Expected 1 tradeoff");
    }

    #[test]
    fn test_agency_import_9col_metadata_preserved() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".workgraph");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = write_agency_format_csv(tmp.path());
        run(&wg_dir, csv_path.to_str().unwrap(), false, None).unwrap();

        // Read all components and check metadata
        let components_dir = wg_dir.join("agency/primitives/components");
        let mut found_task_scope = false;
        let mut found_meta_scope = false;

        for entry in std::fs::read_dir(&components_dir).unwrap() {
            let entry = entry.unwrap();
            let component: RoleComponent =
                agency::load_component(&entry.path()).unwrap();

            // Check that domain_tags are populated
            assert!(!component.domain_tags.is_empty(), "domain_tags should be populated");

            // Check scope metadata
            if let Some(scope) = component.metadata.get("scope") {
                if scope == "task" {
                    found_task_scope = true;
                    assert_eq!(
                        component.metadata.get("domain_specificity").map(|s| s.as_str()),
                        Some("high")
                    );
                    assert!(component.metadata.contains_key("origin_instance_id"));
                }
                if scope == "meta:assigner" {
                    found_meta_scope = true;
                }
            }
        }

        assert!(found_task_scope, "Should have a task-scope component");
        assert!(found_meta_scope, "Should have a meta:assigner-scope component");
    }

    #[test]
    fn test_agency_import_9col_quality_maps_to_score() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".workgraph");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = write_agency_format_csv(tmp.path());
        run(&wg_dir, csv_path.to_str().unwrap(), false, None).unwrap();

        // Read the outcome and check that quality (90) mapped to avg_score (0.90)
        let outcomes_dir = wg_dir.join("agency/primitives/outcomes");
        for entry in std::fs::read_dir(&outcomes_dir).unwrap() {
            let entry = entry.unwrap();
            let outcome: DesiredOutcome =
                agency::load_outcome(&entry.path()).unwrap();
            assert_eq!(outcome.performance.avg_score, Some(0.90));
        }
    }

    #[test]
    fn test_agency_import_9col_domain_maps_to_tags() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".workgraph");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = write_agency_format_csv(tmp.path());
        run(&wg_dir, csv_path.to_str().unwrap(), false, None).unwrap();

        // Read the tradeoff and check domain tags
        let tradeoffs_dir = wg_dir.join("agency/primitives/tradeoffs");
        for entry in std::fs::read_dir(&tradeoffs_dir).unwrap() {
            let entry = entry.unwrap();
            let tradeoff: TradeoffConfig =
                agency::load_tradeoff(&entry.path()).unwrap();
            assert!(
                tradeoff.domain_tags.contains(&"analysis".to_string()),
                "Expected 'analysis' in domain_tags, got: {:?}",
                tradeoff.domain_tags
            );
            assert!(
                tradeoff.domain_tags.contains(&"research".to_string()),
                "Expected 'research' in domain_tags, got: {:?}",
                tradeoff.domain_tags
            );
        }
    }

    #[test]
    fn test_agency_import_9col_tradeoff_uses_description() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".workgraph");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = write_agency_format_csv(tmp.path());
        run(&wg_dir, csv_path.to_str().unwrap(), false, None).unwrap();

        // Tradeoff should use description as-is, with empty acceptable/unacceptable lists
        let tradeoffs_dir = wg_dir.join("agency/primitives/tradeoffs");
        for entry in std::fs::read_dir(&tradeoffs_dir).unwrap() {
            let entry = entry.unwrap();
            let tradeoff: TradeoffConfig =
                agency::load_tradeoff(&entry.path()).unwrap();
            assert!(
                tradeoff.acceptable_tradeoffs.is_empty(),
                "Agency format should not split description into acceptable list"
            );
            assert!(
                tradeoff.unacceptable_tradeoffs.is_empty(),
                "Agency format should not split description into unacceptable list"
            );
            assert!(
                tradeoff.description.contains("depth and breadth conflict"),
                "Description should be preserved as-is"
            );
        }
    }

    #[test]
    fn test_agency_import_9col_parent_content_hash_in_lineage() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".workgraph");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = write_agency_format_csv(tmp.path());
        run(&wg_dir, csv_path.to_str().unwrap(), false, None).unwrap();

        // The "Identify Gaps" component has parent_content_hash=abc123
        let components_dir = wg_dir.join("agency/primitives/components");
        let mut found_with_parent = false;
        for entry in std::fs::read_dir(&components_dir).unwrap() {
            let entry = entry.unwrap();
            let component: RoleComponent =
                agency::load_component(&entry.path()).unwrap();
            if component.name == "Identify Gaps" {
                assert!(
                    component.lineage.parent_ids.contains(&"abc123".to_string()),
                    "parent_content_hash should be in lineage.parent_ids"
                );
                found_with_parent = true;
            }
        }
        assert!(found_with_parent, "Should find the Identify Gaps component");
    }

    #[test]
    fn test_detect_format_agency() {
        let header = csv::StringRecord::from(vec![
            "type",
            "name",
            "description",
            "quality",
            "domain_specificity",
            "domain",
            "origin_instance_id",
            "parent_content_hash",
            "scope",
        ]);
        assert_eq!(detect_format(&header), CsvFormat::Agency);
    }

    #[test]
    fn test_detect_format_legacy() {
        let header = csv::StringRecord::from(vec![
            "type",
            "name",
            "description",
            "col4",
            "col5",
            "domain_tags",
            "quality_score",
        ]);
        assert_eq!(detect_format(&header), CsvFormat::Legacy);
    }

    #[test]
    fn test_import_writes_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".workgraph");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = write_agency_format_csv(tmp.path());
        let counts = run(&wg_dir, csv_path.to_str().unwrap(), false, None).unwrap();

        assert_eq!(counts.role_components, 2);
        assert_eq!(counts.desired_outcomes, 1);
        assert_eq!(counts.trade_off_configs, 1);

        // Manifest should exist
        let mp = manifest_path(&wg_dir);
        assert!(mp.exists(), "Manifest should be written after import");

        let manifest: ImportManifest =
            serde_yaml::from_str(&std::fs::read_to_string(&mp).unwrap()).unwrap();
        assert_eq!(manifest.counts.role_components, 2);
        assert_eq!(manifest.counts.desired_outcomes, 1);
        assert_eq!(manifest.counts.trade_off_configs, 1);
        assert!(!manifest.content_hash.is_empty());
    }

    #[test]
    fn test_import_dry_run_no_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".workgraph");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = write_agency_format_csv(tmp.path());
        run(&wg_dir, csv_path.to_str().unwrap(), true, None).unwrap();

        let mp = manifest_path(&wg_dir);
        assert!(
            !mp.exists(),
            "Manifest should NOT be written on dry run"
        );
    }

    #[test]
    fn test_reimport_updates_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".workgraph");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = write_agency_format_csv(tmp.path());

        // First import
        run(&wg_dir, csv_path.to_str().unwrap(), false, None).unwrap();
        let mp = manifest_path(&wg_dir);
        let manifest1: ImportManifest =
            serde_yaml::from_str(&std::fs::read_to_string(&mp).unwrap()).unwrap();

        // Re-import (idempotent — same content hash)
        run(&wg_dir, csv_path.to_str().unwrap(), false, None).unwrap();
        let manifest2: ImportManifest =
            serde_yaml::from_str(&std::fs::read_to_string(&mp).unwrap()).unwrap();

        assert_eq!(manifest1.content_hash, manifest2.content_hash);
        assert_eq!(manifest1.counts.role_components, manifest2.counts.role_components);
    }
}
