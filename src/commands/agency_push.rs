use std::path::Path;

use anyhow::{Context, Result};

use workgraph::agency::{AgencyStore, DesiredOutcome, LocalStore, RoleComponent, TradeoffConfig};
use workgraph::federation::{self, EntityFilter, TransferOptions};

/// Options for the push command.
pub struct PushOptions<'a> {
    pub target: &'a str,
    pub dry_run: bool,
    pub no_performance: bool,
    pub no_evaluations: bool,
    pub force: bool,
    pub global: bool,
    pub entity_ids: &'a [String],
    pub entity_type: Option<&'a str>,
    pub json: bool,
}

/// Options for `wg agency export`.
pub struct ExportOptions<'a> {
    pub output: &'a str,
    pub format: &'a str,
    pub filter: Option<&'a str>,
    pub global: bool,
}

/// Get the local (source) store for push.
fn local_store(workgraph_dir: &Path, global: bool) -> Result<LocalStore> {
    let path = if global {
        let home = dirs::home_dir().context("Cannot determine home directory")?;
        home.join(".wg").join("agency")
    } else {
        workgraph_dir.join("agency")
    };
    if !LocalStore::new(&path).is_valid() {
        if global {
            anyhow::bail!(
                "No global agency store found at ~/.wg/agency/. Run 'wg agency init' first."
            );
        } else {
            anyhow::bail!("No local agency store found. Run 'wg agency init' first.");
        }
    }
    Ok(LocalStore::new(&path))
}

const AGENCY_CSV_HEADERS: [&str; 12] = [
    "type",
    "name",
    "description",
    "quality",
    "domain_specificity",
    "domain",
    "origin_instance_id",
    "parent_content_hash",
    "scope",
    "parent_ids",
    "generation",
    "created_by",
];

#[derive(Debug, Clone)]
struct AgencyCsvRow {
    type_rank: u8,
    type_name: &'static str,
    name: String,
    id: String,
    /// Original row index within the source CSV (set by `wg agency import`).
    /// Sorted to the front so re-export preserves input order; primitives
    /// without an index (e.g., locally-created via `wg agency init` or
    /// evolution) sort after, by (type_rank, name, id).
    csv_row_idx: Option<u64>,
    fields: [String; 12],
}

pub fn run_export(workgraph_dir: &Path, opts: &ExportOptions<'_>) -> Result<()> {
    if opts.format != "agency-csv" {
        anyhow::bail!(
            "Unsupported agency export format '{}'. Use: agency-csv",
            opts.format
        );
    }

    let source = local_store(workgraph_dir, opts.global)?;
    let origin_filter = parse_origin_filter(opts.filter)?;

    let mut rows = Vec::new();
    for component in source.load_components()? {
        if include_metadata(&component.metadata, origin_filter.as_deref()) {
            rows.push(row_from_component(component)?);
        }
    }
    for outcome in source.load_outcomes()? {
        if include_metadata(&outcome.metadata, origin_filter.as_deref()) {
            rows.push(row_from_outcome(outcome)?);
        }
    }
    for tradeoff in source.load_tradeoffs()? {
        if include_metadata(&tradeoff.metadata, origin_filter.as_deref()) {
            rows.push(row_from_tradeoff(tradeoff)?);
        }
    }

    rows.sort_by(|a, b| {
        // Imported rows (with `agency_csv_row_idx`) sort by their original
        // CSV position so re-export preserves input order — required for a
        // byte-equal roundtrip with sources like upstream starter.csv that
        // are not alphabetised. Locally-created primitives (no row index)
        // sort after, by (type_rank, name, id).
        match (a.csv_row_idx, b.csv_row_idx) {
            (Some(ai), Some(bi)) => ai.cmp(&bi),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => (a.type_rank, a.name.as_str(), a.id.as_str()).cmp(&(
                b.type_rank,
                b.name.as_str(),
                b.id.as_str(),
            )),
        }
    });

    let csv_bytes = write_agency_csv(&rows)?;
    if opts.output == "-" {
        print!(
            "{}",
            String::from_utf8(csv_bytes).context("CSV output was not UTF-8")?
        );
    } else {
        std::fs::write(opts.output, csv_bytes)
            .with_context(|| format!("Failed to write '{}'", opts.output))?;
    }

    Ok(())
}

fn parse_origin_filter(filter: Option<&str>) -> Result<Option<String>> {
    match filter {
        None => Ok(None),
        Some(raw) if raw.trim().is_empty() => Ok(None),
        Some(raw) => {
            let Some(value) = raw.strip_prefix("origin_instance_id=") else {
                anyhow::bail!(
                    "Unsupported agency export filter '{}'. Use origin_instance_id=<value>",
                    raw
                );
            };
            Ok(Some(value.to_string()))
        }
    }
}

fn include_metadata(
    metadata: &std::collections::HashMap<String, String>,
    origin_filter: Option<&str>,
) -> bool {
    origin_filter
        .map(|origin| {
            metadata
                .get("origin_instance_id")
                .is_some_and(|value| value == origin)
        })
        .unwrap_or(true)
}

fn write_agency_csv(rows: &[AgencyCsvRow]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    {
        // CRLF terminator matches both upstream agentbureau/agency starter.csv
        // and RFC 4180. Required for byte-exact CSV roundtrip with upstream.
        let mut writer = csv::WriterBuilder::new()
            .terminator(csv::Terminator::CRLF)
            .quote_style(csv::QuoteStyle::Necessary)
            .from_writer(&mut output);
        writer.write_record(AGENCY_CSV_HEADERS)?;
        for row in rows {
            debug_assert_eq!(row.fields[0], row.type_name);
            writer.write_record(&row.fields)?;
        }
        writer.flush()?;
    }
    Ok(output)
}

fn csv_row_idx(metadata: &std::collections::HashMap<String, String>) -> Option<u64> {
    metadata.get("agency_csv_row_idx")?.parse::<u64>().ok()
}

fn row_from_component(component: RoleComponent) -> Result<AgencyCsvRow> {
    let csv_row_idx = csv_row_idx(&component.metadata);
    let fields = agency_csv_fields(
        "role_component",
        &component.name,
        &component.description,
        component.quality,
        component.domain_specificity,
        &component.domain,
        component.origin_instance_id.as_deref(),
        component.parent_content_hash.as_deref(),
        component.scope.as_deref(),
        &component.performance,
        &component.lineage,
        &component.domain_tags,
        &component.metadata,
    )?;
    Ok(AgencyCsvRow {
        type_rank: 0,
        type_name: "role_component",
        name: component.name,
        id: component.id,
        csv_row_idx,
        fields,
    })
}

fn row_from_outcome(outcome: DesiredOutcome) -> Result<AgencyCsvRow> {
    let csv_row_idx = csv_row_idx(&outcome.metadata);
    let fields = agency_csv_fields(
        "desired_outcome",
        &outcome.name,
        &outcome.description,
        outcome.quality,
        outcome.domain_specificity,
        &outcome.domain,
        outcome.origin_instance_id.as_deref(),
        outcome.parent_content_hash.as_deref(),
        outcome.scope.as_deref(),
        &outcome.performance,
        &outcome.lineage,
        &outcome.domain_tags,
        &outcome.metadata,
    )?;
    Ok(AgencyCsvRow {
        type_rank: 1,
        type_name: "desired_outcome",
        name: outcome.name,
        id: outcome.id,
        csv_row_idx,
        fields,
    })
}

fn row_from_tradeoff(tradeoff: TradeoffConfig) -> Result<AgencyCsvRow> {
    let csv_row_idx = csv_row_idx(&tradeoff.metadata);
    let fields = agency_csv_fields(
        "trade_off_config",
        &tradeoff.name,
        &tradeoff.description,
        tradeoff.quality,
        tradeoff.domain_specificity,
        &tradeoff.domain,
        tradeoff.origin_instance_id.as_deref(),
        tradeoff.parent_content_hash.as_deref(),
        tradeoff.scope.as_deref(),
        &tradeoff.performance,
        &tradeoff.lineage,
        &tradeoff.domain_tags,
        &tradeoff.metadata,
    )?;
    Ok(AgencyCsvRow {
        type_rank: 2,
        type_name: "trade_off_config",
        name: tradeoff.name,
        id: tradeoff.id,
        csv_row_idx,
        fields,
    })
}

fn agency_csv_fields(
    type_name: &str,
    name: &str,
    description: &str,
    quality_value: u8,
    domain_specificity_value: u8,
    domain: &[String],
    origin_instance_id: Option<&str>,
    parent_content_hash: Option<&str>,
    scope: Option<&str>,
    performance: &workgraph::agency::PerformanceRecord,
    lineage: &workgraph::agency::Lineage,
    domain_tags: &[String],
    metadata: &std::collections::HashMap<String, String>,
) -> Result<[String; 12]> {
    let quality = metadata.get("agency_quality").cloned().unwrap_or_else(|| {
        if quality_value > 0 {
            quality_value.to_string()
        } else {
            quality_from_score(performance.avg_score)
        }
    });
    // For roundtrip with imported CSV: prefer the raw text captured at import
    // time. Fall back to deriving from lineage for primitives that originated
    // inside wg (no metadata.parent_ids key). Note that the importer copies
    // parent_content_hash into lineage.parent_ids; we filter it out here so
    // upstream rows whose only "parent" is parent_content_hash re-export with
    // an empty parent_ids field rather than a synthesised JSON array.
    let parent_ids = metadata.get("parent_ids").cloned().unwrap_or_else(|| {
        let derived: Vec<String> = lineage
            .parent_ids
            .iter()
            .filter(|id| Some(id.as_str()) != parent_content_hash)
            .cloned()
            .collect();
        parent_ids_json(&derived)
    });
    let domain_str = metadata.get("domain_raw").cloned().unwrap_or_else(|| {
        let domain_values = if domain.is_empty() {
            domain_tags
        } else {
            domain
        };
        domain_values.join(",")
    });

    Ok([
        type_name.to_string(),
        name.to_string(),
        description.to_string(),
        quality,
        metadata
            .get("domain_specificity")
            .cloned()
            .unwrap_or_else(|| domain_specificity_value.to_string()),
        domain_str,
        metadata
            .get("origin_instance_id")
            .cloned()
            .or_else(|| origin_instance_id.map(str::to_string))
            .unwrap_or_default(),
        metadata
            .get("parent_content_hash")
            .cloned()
            .or_else(|| parent_content_hash.map(str::to_string))
            .unwrap_or_default(),
        metadata
            .get("scope")
            .cloned()
            .or_else(|| scope.map(str::to_string))
            .unwrap_or_default(),
        parent_ids,
        lineage.generation.to_string(),
        lineage.created_by.clone(),
    ])
}

fn quality_from_score(score: Option<f64>) -> String {
    ((score.unwrap_or(1.0) * 100.0).round() as u32).to_string()
}

fn parent_ids_json(parent_ids: &[String]) -> String {
    if parent_ids.is_empty() {
        String::new()
    } else {
        serde_json::to_string(parent_ids).unwrap_or_default()
    }
}

pub fn run(workgraph_dir: &Path, opts: &PushOptions<'_>) -> Result<()> {
    // Local store is the source (we're pushing FROM local)
    let source = local_store(workgraph_dir, opts.global)?;

    // Resolve target store (check named remotes first, then path)
    let target_store = federation::resolve_store_with_remotes(opts.target, workgraph_dir)?;

    let entity_filter = match opts.entity_type {
        Some("component" | "components") => EntityFilter::Components,
        Some("outcome" | "outcomes") => EntityFilter::Outcomes,
        Some("role" | "roles") => EntityFilter::Roles,
        Some("motivation" | "motivations" | "tradeoff" | "tradeoffs") => EntityFilter::Tradeoffs,
        Some("agent" | "agents") => EntityFilter::Agents,
        Some(other) => anyhow::bail!(
            "Unknown entity type '{}'. Use: component, outcome, role, tradeoff, motivation, or agent",
            other
        ),
        None => EntityFilter::All,
    };

    let transfer_opts = TransferOptions {
        dry_run: opts.dry_run,
        no_performance: opts.no_performance,
        no_evaluations: opts.no_evaluations,
        force: opts.force,
        entity_ids: opts.entity_ids.to_vec(),
        entity_filter,
    };

    let summary = federation::transfer(&source, &target_store, &transfer_opts)?;

    // Update last_sync if the target was a named remote
    if !opts.dry_run {
        let _ = federation::touch_remote_sync(workgraph_dir, opts.target);
    }

    if opts.json {
        let output = serde_json::json!({
            "action": if opts.dry_run { "dry_run" } else { "push" },
            "target": target_store.store_path().display().to_string(),
            "roles": {
                "added": summary.roles_added,
                "updated": summary.roles_updated,
                "skipped": summary.roles_skipped,
            },
            "motivations": {
                "added": summary.tradeoffs_added,
                "updated": summary.tradeoffs_updated,
                "skipped": summary.tradeoffs_skipped,
            },
            "agents": {
                "added": summary.agents_added,
                "updated": summary.agents_updated,
                "skipped": summary.agents_skipped,
            },
            "evaluations": {
                "added": summary.evaluations_added,
                "skipped": summary.evaluations_skipped,
            },
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if opts.dry_run {
        println!(
            "Dry run — would push to {}:",
            target_store.store_path().display()
        );
        println!("{}", summary);
    } else {
        println!("Pushed to {}:", target_store.store_path().display());
        println!("{}", summary);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use workgraph::agency::{
        self, AgencyStore, Agent, Lineage, PerformanceRecord, Role, TradeoffConfig,
    };
    use workgraph::graph::TrustLevel;

    /// Byte-equal CSV roundtrip on a synthetic fixture covering the three
    /// classes of drift that previously broke roundtripping the upstream
    /// agentbureau/agency starter.csv:
    ///   1. CRLF line terminators
    ///   2. Multi-domain space-after-comma ("software, management")
    ///   3. parent_content_hash set with empty parent_ids
    ///
    /// Repro for the upstream byte-equal test (out-of-process):
    ///   curl -sL .../agentbureau/agency/main/primitives/starter.csv > u.csv
    ///   mkdir -p .wg/agency
    ///   wg --dir .wg agency import --format agency-csv u.csv
    ///   wg --dir .wg agency export --format agency-csv reexport.csv
    ///   cmp u.csv reexport.csv  # matching rows are byte-equal
    /// Note: full byte-equal with upstream additionally requires the dedup
    /// fixes tracked in `investigate-agency-import` (some primitives in
    /// upstream share descriptions and only differ by `scope`, which the
    /// content-hash filename collapses).
    #[test]
    fn agency_csv_roundtrip_byte_equal_synthetic() {
        use std::io::Write;
        // Synthetic fixture covering all three drift classes.
        // Use CRLF terminators throughout; csv::QuoteStyle::Necessary on the
        // export side means fields are only quoted when they contain commas
        // or quotes, matching the input.
        let csv = b"\
type,name,description,quality,domain_specificity,domain,origin_instance_id,parent_content_hash,scope,parent_ids,generation,created_by\r\n\
role_component,verify-distribution-channel-currency,After each release verify that every distribution channel referenced in documentation points to the current release version,100,45,software,upstream-1,5028f239d7f7080f,task,,0,human\r\n\
role_component,write-session-handoff-context,Write a timestamped handoff file at session end,100,50,\"software, management\",upstream-1,,task,,0,human\r\n\
role_component,classify-claims-by-type,\"Extract and classify claims from source material by type: fact, opinion, or theory\",100,40,\"research,analysis\",upstream-1,,task,,0,human\r\n\
trade_off_config,prefer-depth,When depth and breadth conflict prefer deeper analysis,80,low,research,upstream-1,pch-depth,task,\"[\"\"pch-depth\"\"]\",1,human\r\n\
desired_outcome,verification-summary,Return a summary with total claim count,90,50,analysis,upstream-1,,task,,0,human\r\n\
";

        let tmp = TempDir::new().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = wg_dir.parent().unwrap().join("upstream.csv");
        let mut f = std::fs::File::create(&csv_path).unwrap();
        f.write_all(csv).unwrap();
        drop(f);

        super::super::agency_import::run(&wg_dir, csv_path.to_str().unwrap(), false, None).unwrap();

        let out_path = wg_dir.parent().unwrap().join("reexport.csv");
        run_export(
            &wg_dir,
            &ExportOptions {
                output: out_path.to_str().unwrap(),
                format: "agency-csv",
                filter: None,
                global: false,
            },
        )
        .unwrap();

        let exported = std::fs::read(&out_path).unwrap();
        if exported != csv {
            // Pretty-print the byte-level diff so test failures are debuggable.
            let exp_str = String::from_utf8_lossy(csv).replace("\r\n", "\\r\\n\n");
            let got_str = String::from_utf8_lossy(&exported).replace("\r\n", "\\r\\n\n");
            panic!(
                "CSV roundtrip drifted ({} vs {} bytes).\n--- expected ---\n{}\n--- got ---\n{}",
                csv.len(),
                exported.len(),
                exp_str,
                got_str,
            );
        }
    }

    /// Repro for the third drift class in isolation: when the upstream row has
    /// `parent_content_hash` set but `parent_ids` empty, the importer used to
    /// copy `parent_content_hash` into `lineage.parent_ids`, and the exporter
    /// would then synthesise `parent_ids = ["<hash>"]` — diverging from the
    /// empty cell in the source.
    #[test]
    fn agency_csv_export_does_not_synthesise_parent_ids_from_parent_content_hash() {
        let tmp = TempDir::new().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv = b"\
type,name,description,quality,domain_specificity,domain,origin_instance_id,parent_content_hash,scope,parent_ids,generation,created_by\r\n\
role_component,verify-install-path,Run install command from documentation in clean environment,100,50,software,upstream-1,5028f239d7f7080f,task,,0,human\r\n\
";
        let csv_path = tmp.path().join("input.csv");
        std::fs::write(&csv_path, csv).unwrap();
        super::super::agency_import::run(&wg_dir, csv_path.to_str().unwrap(), false, None).unwrap();

        let out_path = tmp.path().join("export.csv");
        run_export(
            &wg_dir,
            &ExportOptions {
                output: out_path.to_str().unwrap(),
                format: "agency-csv",
                filter: None,
                global: false,
            },
        )
        .unwrap();

        let exported = std::fs::read_to_string(&out_path).unwrap();
        assert!(
            !exported.contains("[\"5028f239d7f7080f\"]"),
            "export must not synthesise parent_ids from parent_content_hash:\n{}",
            exported
        );
        // The exact roundtripped row should match (CRLF + empty parent_ids cell).
        assert_eq!(exported.as_bytes(), csv, "byte-equal roundtrip");
    }

    fn setup_store(tmp: &TempDir, name: &str) -> LocalStore {
        let path = tmp.path().join(name).join("agency");
        agency::init(&path).unwrap();
        LocalStore::new(path)
    }

    fn make_role(id: &str, name: &str) -> Role {
        Role {
            id: id.to_string(),
            name: name.to_string(),
            description: "test role".to_string(),
            component_ids: Vec::new(),
            outcome_id: "test outcome".to_string(),
            performance: PerformanceRecord::default(),
            lineage: Lineage::default(),
            default_context_scope: None,
            default_exec_mode: None,
        }
    }

    fn make_motivation(id: &str, name: &str) -> TradeoffConfig {
        TradeoffConfig {
            id: id.to_string(),
            name: name.to_string(),
            description: "test motivation".to_string(),
            quality: 100,
            domain_specificity: 0,
            domain: vec![],
            scope: None,
            origin_instance_id: None,
            parent_content_hash: None,
            acceptable_tradeoffs: Vec::new(),
            unacceptable_tradeoffs: Vec::new(),
            performance: PerformanceRecord::default(),
            lineage: Lineage::default(),
            access_control: workgraph::agency::AccessControl::default(),
            domain_tags: vec![],
            metadata: std::collections::HashMap::new(),
            former_agents: vec![],
            former_deployments: vec![],
        }
    }

    fn make_agent(id: &str, name: &str, role_id: &str, tradeoff_id: &str) -> Agent {
        Agent {
            id: id.to_string(),
            role_id: role_id.to_string(),
            tradeoff_id: tradeoff_id.to_string(),
            name: name.to_string(),
            performance: PerformanceRecord::default(),
            lineage: Lineage::default(),
            capabilities: Vec::new(),
            rate: None,
            capacity: None,
            trust_level: TrustLevel::Provisional,
            contact: None,
            executor: "claude".to_string(),
            preferred_model: None,
            preferred_provider: None,
            deployment_history: vec![],
            attractor_weight: 0.5,
            staleness_flags: vec![],
        }
    }

    fn default_opts(target: &str) -> PushOptions<'_> {
        PushOptions {
            target,
            dry_run: false,
            no_performance: false,
            no_evaluations: false,
            force: false,
            global: false,
            entity_ids: &[],
            entity_type: None,
            json: false,
        }
    }

    #[test]
    fn push_via_run_function() {
        let tmp = TempDir::new().unwrap();

        // Set up a WG dir as source
        let wg_dir = tmp.path().join("project").join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();
        let agency_dir = wg_dir.join("agency");
        agency::init(&agency_dir).unwrap();

        let source = LocalStore::new(&agency_dir);
        source.save_role(&make_role("r1", "tester")).unwrap();
        source
            .save_tradeoff(&make_motivation("m1", "quality"))
            .unwrap();

        // Target doesn't exist yet — push should create it
        let target_path = tmp.path().join("target");
        std::fs::create_dir_all(&target_path).unwrap();

        run(&wg_dir, &default_opts(target_path.to_str().unwrap())).unwrap();

        let target = LocalStore::new(target_path.join("agency"));
        assert!(target.exists_role("r1"));
        assert!(target.exists_tradeoff("m1"));
    }

    #[test]
    fn push_with_named_remote() {
        let tmp = TempDir::new().unwrap();

        // Set up WG dir as source
        let wg_dir = tmp.path().join("project").join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();
        let agency_dir = wg_dir.join("agency");
        agency::init(&agency_dir).unwrap();

        let source = LocalStore::new(&agency_dir);
        source.save_role(&make_role("r1", "pushed-role")).unwrap();

        // Set up target store
        let target = setup_store(&tmp, "target");

        // Write federation.yaml with a named remote pointing to target
        let federation_yaml = format!(
            "remotes:\n  downstream:\n    path: \"{}\"\n    description: \"test remote\"\n",
            target.store_path().display()
        );
        std::fs::write(wg_dir.join("federation.yaml"), federation_yaml).unwrap();

        run(
            &wg_dir,
            &PushOptions {
                target: "downstream",
                no_evaluations: true,
                ..default_opts("")
            },
        )
        .unwrap();

        assert!(target.exists_role("r1"));
    }

    #[test]
    fn push_invalid_type_errors() {
        let tmp = TempDir::new().unwrap();
        let wg_dir = tmp.path().join("project").join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();
        let agency_dir = wg_dir.join("agency");
        agency::init(&agency_dir).unwrap();

        let target_path = tmp.path().join("target");
        std::fs::create_dir_all(&target_path).unwrap();

        let result = run(
            &wg_dir,
            &PushOptions {
                target: target_path.to_str().unwrap(),
                entity_type: Some("invalid_type"),
                ..default_opts("")
            },
        );
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Unknown entity type")
        );
    }

    #[test]
    fn push_no_local_store_errors() {
        let tmp = TempDir::new().unwrap();
        let wg_dir = tmp.path().join("empty").join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();
        // Don't init agency — no roles/ dir

        let target_path = tmp.path().join("target");
        std::fs::create_dir_all(&target_path).unwrap();

        let result = run(&wg_dir, &default_opts(target_path.to_str().unwrap()));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No local agency store")
        );
    }

    #[test]
    fn push_dry_run_does_not_write() {
        let tmp = TempDir::new().unwrap();
        let wg_dir = tmp.path().join("project").join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();
        let agency_dir = wg_dir.join("agency");
        agency::init(&agency_dir).unwrap();

        let source = LocalStore::new(&agency_dir);
        source.save_role(&make_role("r1", "dry-role")).unwrap();

        let target = setup_store(&tmp, "target");

        run(
            &wg_dir,
            &PushOptions {
                target: target.store_path().to_str().unwrap(),
                dry_run: true,
                ..default_opts("")
            },
        )
        .unwrap();

        assert!(!target.exists_role("r1"));
    }

    #[test]
    fn push_type_filter_roles_only() {
        let tmp = TempDir::new().unwrap();
        let wg_dir = tmp.path().join("project").join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();
        let agency_dir = wg_dir.join("agency");
        agency::init(&agency_dir).unwrap();

        let source = LocalStore::new(&agency_dir);
        source.save_role(&make_role("r1", "role")).unwrap();
        source.save_tradeoff(&make_motivation("m1", "mot")).unwrap();

        let target = setup_store(&tmp, "target");

        run(
            &wg_dir,
            &PushOptions {
                target: target.store_path().to_str().unwrap(),
                entity_type: Some("role"),
                ..default_opts("")
            },
        )
        .unwrap();

        assert!(target.exists_role("r1"));
        assert!(!target.exists_tradeoff("m1"));
    }

    #[test]
    fn push_agent_auto_pushes_dependencies() {
        let tmp = TempDir::new().unwrap();
        let wg_dir = tmp.path().join("project").join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();
        let agency_dir = wg_dir.join("agency");
        agency::init(&agency_dir).unwrap();

        let source = LocalStore::new(&agency_dir);
        source.save_role(&make_role("r1", "builder")).unwrap();
        source
            .save_tradeoff(&make_motivation("m1", "speed"))
            .unwrap();
        source
            .save_agent(&make_agent("a1", "fast-builder", "r1", "m1"))
            .unwrap();

        let target = setup_store(&tmp, "target");

        run(
            &wg_dir,
            &PushOptions {
                target: target.store_path().to_str().unwrap(),
                entity_type: Some("agent"),
                ..default_opts("")
            },
        )
        .unwrap();

        assert!(target.exists_agent("a1"));
        assert!(target.exists_role("r1"));
        assert!(target.exists_tradeoff("m1"));
    }
}
