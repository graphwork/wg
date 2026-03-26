use anyhow::{Context, Result};
use std::path::Path;
use workgraph::agency::{
    self, AccessControl, AccessPolicy, ComponentCategory, ContentRef, DesiredOutcome, Lineage,
    PerformanceRecord, RoleComponent, TradeoffConfig,
};

/// `wg agency import <csv-path>` — import Agency's starter.csv primitives into WorkGraph.
///
/// CSV columns: type, name, description, col4 (category/acceptable), col5 (content/criteria/unacceptable), domain_tags, quality_score
///
/// Conversion:
/// - type=skill → RoleComponent
/// - type=outcome → DesiredOutcome
/// - type=tradeoff → TradeoffConfig
pub fn run(workgraph_dir: &Path, csv_path: &str, dry_run: bool, tag: Option<&str>) -> Result<()> {
    let provenance_tag = tag.unwrap_or("agency-import");
    let agency_dir = workgraph_dir.join("agency");

    if !dry_run {
        agency::init(&agency_dir).context("Failed to initialize agency directory")?;
    }

    let csv_content = std::fs::read_to_string(csv_path)
        .with_context(|| format!("Failed to read '{}'", csv_path))?;
    let mut reader = csv::Reader::from_reader(csv_content.as_bytes());

    let mut components_count = 0u32;
    let mut outcomes_count = 0u32;
    let mut tradeoffs_count = 0u32;
    let mut skipped = 0u32;

    for (row_idx, record) in reader.records().enumerate() {
        let record = record.with_context(|| format!("Failed to parse CSV row {}", row_idx + 1))?;

        let ptype = record.get(0).unwrap_or("").trim();
        let name = record.get(1).unwrap_or("").trim().to_string();
        let description = record.get(2).unwrap_or("").trim().to_string();
        let col4 = record.get(3).unwrap_or("").trim().to_string();
        let col5 = record.get(4).unwrap_or("").trim().to_string();
        // col6 = domain_tags (informational only)
        let quality_score: Option<f64> = record.get(6).and_then(|s| s.trim().parse().ok());

        let lineage = Lineage {
            parent_ids: vec![],
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

        match ptype {
            "skill" => {
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
                let success_criteria: Vec<String> = col5
                    .split('\n')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
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
                let acceptable: Vec<String> = col4
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                let unacceptable: Vec<String> = col5
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
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

    let mode = if dry_run { " (dry run)" } else { "" };
    println!("Agency import complete{}:", mode);
    println!("  Components: {}", components_count);
    println!("  Outcomes:   {}", outcomes_count);
    println!("  Tradeoffs:  {}", tradeoffs_count);
    if skipped > 0 {
        println!("  Skipped:    {}", skipped);
    }

    Ok(())
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

        // Same count — content hashing deduplicates
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
}
