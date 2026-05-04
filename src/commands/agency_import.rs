use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use workgraph::agency::{
    self, AccessControl, AccessPolicy, ComponentCategory, ContentRef, DesiredOutcome, Lineage,
    PerformanceRecord, RoleComponent, TradeoffConfig,
};
use workgraph::config::Config;

/// Counts of primitives imported from a CSV file.
#[derive(Debug, Clone, Default)]
pub struct ImportCounts {
    pub role_components: u32,
    pub desired_outcomes: u32,
    pub trade_off_configs: u32,
    /// Rows skipped (unknown type). Used for display in run_from_bytes.
    #[allow(dead_code)]
    pub skipped: u32,
}

/// Provenance manifest written after a successful CSV import.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImportManifest {
    pub source: String,
    pub version: String,
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub agency_compat_version: Option<String>,
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
        schema: Some("agency-12col-v1.2.4".to_string()),
        agency_compat_version: Some(agency::WG_AGENCY_COMPAT_VERSION.to_string()),
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

/// Options for the import command (covers local file, URL, and upstream modes).
pub struct ImportOptions {
    pub csv_path: Option<String>,
    pub url: Option<String>,
    pub upstream: bool,
    pub format: Option<String>,
    pub dry_run: bool,
    pub tag: Option<String>,
    pub force: bool,
    pub check: bool,
    /// Error on the first dedup collision rather than warn-and-skip.
    pub strict: bool,
}

/// One detected dedup collision during import. See docs/manual/03-agency.md
/// "Import Dedup Rule" for the rule rationale.
#[derive(Debug, Clone)]
pub struct ImportCollision {
    pub kind: &'static str,
    pub row: usize,
    pub hash: String,
    pub kept_name: String,
    pub kept_scope: Option<String>,
    pub dropped_name: String,
    pub dropped_scope: Option<String>,
    /// Where the collision happened: against another CSV row, or against an
    /// existing on-disk file.
    pub origin: CollisionOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollisionOrigin {
    /// Collided with a previous row in this same import.
    SameImport,
    /// Collided with a primitive file already present on disk (seed, prior
    /// import, or another remote).
    Existing,
}

#[derive(Debug, Clone)]
struct ParsedCsvColumns {
    quality_score: Option<f64>,
    domain_tags: Vec<String>,
    metadata: HashMap<String, String>,
    parent_ids: Vec<String>,
    generation: u32,
    created_by: Option<String>,
}

/// Fetch CSV content from a remote URL.
fn fetch_csv(url: &str) -> Result<Vec<u8>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to build HTTP client")?;

    let response = client
        .get(url)
        .send()
        .with_context(|| format!("Failed to fetch '{}'", url))?;

    if !response.status().is_success() {
        anyhow::bail!("HTTP {} fetching '{}'", response.status(), url);
    }

    let bytes = response
        .bytes()
        .with_context(|| format!("Failed to read response from '{}'", url))?;

    Ok(bytes.to_vec())
}

/// Read the existing import manifest, if any.
pub fn read_manifest(workgraph_dir: &Path) -> Result<Option<ImportManifest>> {
    let path = manifest_path(workgraph_dir);
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path).context("Failed to read import manifest")?;
    let manifest: ImportManifest =
        serde_yaml::from_str(&content).context("Failed to parse import manifest")?;
    Ok(Some(manifest))
}

/// Run import from raw CSV bytes (shared by local-file and URL-fetch paths).
pub fn run_from_bytes(
    workgraph_dir: &Path,
    source_label: &str,
    csv_bytes: &[u8],
    dry_run: bool,
    tag: Option<&str>,
) -> Result<ImportCounts> {
    run_from_bytes_with(workgraph_dir, source_label, csv_bytes, dry_run, tag, false).map(|(c, _)| c)
}

/// Like `run_from_bytes` but accepts a `strict` flag and returns the detected
/// collision list alongside the counts. See docs/manual/03-agency.md
/// "Import Dedup Rule" for semantics.
pub fn run_from_bytes_with(
    workgraph_dir: &Path,
    source_label: &str,
    csv_bytes: &[u8],
    dry_run: bool,
    tag: Option<&str>,
    strict: bool,
) -> Result<(ImportCounts, Vec<ImportCollision>)> {
    let provenance_tag = tag.unwrap_or("agency-import");
    let agency_dir = workgraph_dir.join("agency");

    if !dry_run {
        agency::init(&agency_dir).context("Failed to initialize agency directory")?;
    }

    let csv_content = String::from_utf8_lossy(csv_bytes);
    let mut reader = csv::Reader::from_reader(csv_content.as_bytes());

    let format = detect_format(reader.headers().context("Failed to read CSV headers")?);

    // Per-type seen-hash trackers and pre-existing-name lookup. Keys are
    // content_hash; values are (name, scope) of the row that owns the file.
    let mut seen_components: HashMap<String, (String, Option<String>)> = HashMap::new();
    let mut seen_outcomes: HashMap<String, (String, Option<String>)> = HashMap::new();
    let mut seen_tradeoffs: HashMap<String, (String, Option<String>)> = HashMap::new();
    let mut collisions: Vec<ImportCollision> = Vec::new();

    let mut components_count = 0u32;
    let mut outcomes_count = 0u32;
    let mut tradeoffs_count = 0u32;
    let mut skipped = 0u32;

    for (row_idx, record) in reader.records().enumerate() {
        let record = record.with_context(|| format!("Failed to parse CSV row {}", row_idx + 1))?;

        let ptype = record.get(0).unwrap_or("").trim();
        let name = record.get(1).unwrap_or("").trim().to_string();
        let description = record.get(2).unwrap_or("").trim().to_string();

        let ParsedCsvColumns {
            quality_score,
            domain_tags,
            metadata,
            parent_ids,
            generation,
            created_by,
        } = match format {
            CsvFormat::Agency => parse_agency_columns(&record),
            CsvFormat::Legacy => parse_legacy_columns(&record),
        };
        let quality = quality_score
            .map(|score| (score * 100.0).round().clamp(0.0, 100.0) as u8)
            .unwrap_or(100);
        let domain_specificity = metadata
            .get("domain_specificity")
            .and_then(|value| value.parse::<u8>().ok())
            .unwrap_or(0);
        let domain = domain_tags.clone();
        let scope = metadata.get("scope").cloned();
        let origin_instance_id = metadata.get("origin_instance_id").cloned();
        let parent_content_hash = metadata.get("parent_content_hash").cloned();

        let lineage = Lineage {
            parent_ids,
            generation,
            created_by: created_by
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| format!("{}-v{}", provenance_tag, env!("CARGO_PKG_VERSION"))),
            created_at: chrono::Utc::now(),
            reframing_potential: None,
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
                let row_num = row_idx + 1;
                let scope_for_check = scope.clone();
                let collision = check_collision(
                    "component",
                    &id,
                    &name,
                    scope_for_check.as_deref(),
                    row_num,
                    &mut seen_components,
                    &agency_dir.join("primitives/components"),
                    dry_run,
                );
                if let Some(coll) = collision {
                    if strict {
                        anyhow::bail!(format_strict_error(&coll));
                    }
                    eprintln!("{}", format_collision_warning(&coll));
                    collisions.push(coll);
                    skipped += 1;
                    continue;
                }

                if dry_run {
                    println!("  [component] {} ({})", name, agency::short_hash(&id));
                } else {
                    let component = RoleComponent {
                        id: id.clone(),
                        name,
                        description,
                        quality,
                        domain_specificity,
                        domain,
                        scope,
                        origin_instance_id,
                        parent_content_hash,
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
                let row_num = row_idx + 1;
                let scope_for_check = scope.clone();
                let collision = check_collision(
                    "outcome",
                    &id,
                    &name,
                    scope_for_check.as_deref(),
                    row_num,
                    &mut seen_outcomes,
                    &agency_dir.join("primitives/outcomes"),
                    dry_run,
                );
                if let Some(coll) = collision {
                    if strict {
                        anyhow::bail!(format_strict_error(&coll));
                    }
                    eprintln!("{}", format_collision_warning(&coll));
                    collisions.push(coll);
                    skipped += 1;
                    continue;
                }

                if dry_run {
                    println!("  [outcome] {} ({})", name, agency::short_hash(&id));
                } else {
                    let outcome = DesiredOutcome {
                        id: id.clone(),
                        name,
                        description,
                        quality,
                        domain_specificity,
                        domain,
                        scope,
                        origin_instance_id,
                        parent_content_hash,
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
                    CsvFormat::Agency => (vec![], vec![]),
                };
                let id = agency::content_hash_tradeoff(&acceptable, &unacceptable, &description);
                let row_num = row_idx + 1;
                let scope_for_check = scope.clone();
                let collision = check_collision(
                    "tradeoff",
                    &id,
                    &name,
                    scope_for_check.as_deref(),
                    row_num,
                    &mut seen_tradeoffs,
                    &agency_dir.join("primitives/tradeoffs"),
                    dry_run,
                );
                if let Some(coll) = collision {
                    if strict {
                        anyhow::bail!(format_strict_error(&coll));
                    }
                    eprintln!("{}", format_collision_warning(&coll));
                    collisions.push(coll);
                    skipped += 1;
                    continue;
                }

                if dry_run {
                    println!("  [tradeoff] {} ({})", name, agency::short_hash(&id));
                } else {
                    let tradeoff = TradeoffConfig {
                        id: id.clone(),
                        name,
                        description,
                        quality,
                        domain_specificity,
                        domain,
                        scope,
                        origin_instance_id,
                        parent_content_hash,
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
    if !collisions.is_empty() {
        println!(
            "  Collisions: {} (description-hash dedup; rerun with --strict to fail)",
            collisions.len()
        );
    }

    if !dry_run {
        write_manifest(workgraph_dir, source_label, csv_bytes, &counts)?;
    }

    Ok((counts, collisions))
}

fn check_collision(
    kind: &'static str,
    id: &str,
    name: &str,
    scope: Option<&str>,
    row: usize,
    seen: &mut HashMap<String, (String, Option<String>)>,
    on_disk_dir: &Path,
    dry_run: bool,
) -> Option<ImportCollision> {
    if let Some((kept_name, kept_scope)) = seen.get(id) {
        let kept_name = kept_name.clone();
        let kept_scope = kept_scope.clone();
        if kept_name != name || kept_scope.as_deref() != scope {
            return Some(ImportCollision {
                kind,
                row,
                hash: id.to_string(),
                kept_name,
                kept_scope,
                dropped_name: name.to_string(),
                dropped_scope: scope.map(str::to_string),
                origin: CollisionOrigin::SameImport,
            });
        }
        return None;
    }
    if !dry_run {
        let on_disk = on_disk_dir.join(format!("{}.yaml", id));
        if on_disk.exists() {
            let (existing_name, existing_scope) = read_existing_name_scope(kind, &on_disk);
            if existing_name.as_deref() != Some(name)
                || existing_scope.as_deref() != scope
            {
                let collision = ImportCollision {
                    kind,
                    row,
                    hash: id.to_string(),
                    kept_name: existing_name.unwrap_or_else(|| "<unknown>".to_string()),
                    kept_scope: existing_scope,
                    dropped_name: name.to_string(),
                    dropped_scope: scope.map(str::to_string),
                    origin: CollisionOrigin::Existing,
                };
                seen.insert(
                    id.to_string(),
                    (collision.kept_name.clone(), collision.kept_scope.clone()),
                );
                return Some(collision);
            }
        }
    }
    seen.insert(id.to_string(), (name.to_string(), scope.map(str::to_string)));
    None
}

fn read_existing_name_scope(kind: &str, path: &Path) -> (Option<String>, Option<String>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (None, None);
    };
    match kind {
        "component" => {
            if let Ok(c) = serde_yaml::from_str::<workgraph::agency::RoleComponent>(&text) {
                return (Some(c.name), c.scope);
            }
        }
        "outcome" => {
            if let Ok(o) = serde_yaml::from_str::<workgraph::agency::DesiredOutcome>(&text) {
                return (Some(o.name), o.scope);
            }
        }
        "tradeoff" => {
            if let Ok(t) = serde_yaml::from_str::<workgraph::agency::TradeoffConfig>(&text) {
                return (Some(t.name), t.scope);
            }
        }
        _ => {}
    }
    (None, None)
}

fn format_collision_warning(c: &ImportCollision) -> String {
    let kept_scope = c.kept_scope.as_deref().unwrap_or("<none>");
    let dropped_scope = c.dropped_scope.as_deref().unwrap_or("<none>");
    let origin = match c.origin {
        CollisionOrigin::SameImport => "earlier row in this import",
        CollisionOrigin::Existing => "existing on-disk primitive",
    };
    format!(
        "Warning: agency import collision (row {}): {} '{}' (scope={}) shares description-hash {} with {} '{}' (scope={}); skipping",
        c.row,
        c.kind,
        c.dropped_name,
        dropped_scope,
        agency::short_hash(&c.hash),
        origin,
        c.kept_name,
        kept_scope,
    )
}

fn format_strict_error(c: &ImportCollision) -> String {
    let kept_scope = c.kept_scope.as_deref().unwrap_or("<none>");
    let dropped_scope = c.dropped_scope.as_deref().unwrap_or("<none>");
    format!(
        "agency import --strict: row {} {} '{}' (scope={}) collides on description-hash {} with '{}' (scope={})",
        c.row,
        c.kind,
        c.dropped_name,
        dropped_scope,
        agency::short_hash(&c.hash),
        c.kept_name,
        kept_scope,
    )
}

/// Unified entry point for `wg agency import` supporting local file, URL, and upstream modes.
pub fn run_import(workgraph_dir: &Path, opts: ImportOptions) -> Result<ImportCounts> {
    if let Some(format) = opts.format.as_deref()
        && !matches!(format, "agency-csv" | "auto")
    {
        anyhow::bail!(
            "Unsupported agency import format '{}'. Use: agency-csv",
            format
        );
    }

    // Determine the CSV source
    let source_count =
        opts.csv_path.is_some() as u8 + opts.url.is_some() as u8 + opts.upstream as u8;
    if source_count > 1 {
        anyhow::bail!("Specify only one of: CSV_PATH, --url, or --upstream");
    }

    if let Some(ref csv_path) = opts.csv_path {
        // Local file path — existing behavior
        let csv_bytes =
            std::fs::read(csv_path).with_context(|| format!("Failed to read '{}'", csv_path))?;
        return run_from_bytes_with(
            workgraph_dir,
            csv_path,
            &csv_bytes,
            opts.dry_run,
            opts.tag.as_deref(),
            opts.strict,
        )
        .map(|(c, _)| c);
    }

    // Resolve the URL (either explicit --url or --upstream from config)
    let url = if let Some(ref url) = opts.url {
        url.clone()
    } else if opts.upstream {
        let cfg = Config::load_merged(workgraph_dir)?;
        cfg.agency
            .upstream_url
            .ok_or_else(|| anyhow::anyhow!(
                "No upstream URL configured. Set agency.upstream_url in config:\n  wg config --set agency.upstream_url=<URL>"
            ))?
    } else {
        anyhow::bail!("Specify one of: CSV_PATH, --url <URL>, or --upstream");
    };

    // Change detection: compare hash of fetched CSV against manifest
    if (!opts.force || opts.check)
        && let Some(existing_manifest) = read_manifest(workgraph_dir)?
    {
        // Fetch and check
        let csv_bytes = match fetch_csv(&url) {
            Ok(bytes) => bytes,
            Err(e) => {
                if opts.check {
                    eprintln!("Warning: could not fetch upstream: {}", e);
                    std::process::exit(2);
                }
                return Err(e);
            }
        };
        let new_hash = sha256_hex(&csv_bytes);

        if opts.check {
            if new_hash == existing_manifest.content_hash {
                println!("Up to date (hash: {}…)", &new_hash[..12]);
                std::process::exit(1);
            } else {
                println!(
                    "Upstream has changed (local: {}… remote: {}…)",
                    &existing_manifest.content_hash[..12],
                    &new_hash[..12]
                );
                std::process::exit(0);
            }
        }

        if !opts.force && new_hash == existing_manifest.content_hash {
            println!("Already up to date (hash: {}…)", &new_hash[..12]);
            return Ok(ImportCounts::default());
        }

        // Hash differs — import
        return run_from_bytes_with(
            workgraph_dir,
            &url,
            &csv_bytes,
            opts.dry_run,
            opts.tag.as_deref(),
            opts.strict,
        )
        .map(|(c, _)| c);
    }

    // No existing manifest or --force: fetch and import
    let csv_bytes = match fetch_csv(&url) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("Warning: could not fetch upstream CSV: {}", e);
            if opts.check {
                std::process::exit(2);
            }
            return Err(e);
        }
    };

    if opts.check {
        // No manifest to compare against — treat as changed
        println!("No previous import found; upstream available");
        std::process::exit(0);
    }

    run_from_bytes_with(
        workgraph_dir,
        &url,
        &csv_bytes,
        opts.dry_run,
        opts.tag.as_deref(),
        opts.strict,
    )
    .map(|(c, _)| c)
}

/// Detected CSV format based on header or column count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CsvFormat {
    /// Old 7-column format: type,name,description,col4,col5,domain_tags,quality_score
    Legacy,
    /// Agency CSV format: the 9-column starter subset or the 12-column upstream
    /// starter schema ending in parent_ids,generation,created_by.
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

/// `wg agency import <csv-path>` -- import Agency's starter.csv primitives into workgraph.
///
/// Supports two CSV formats:
///
/// **Legacy (7 columns):** type,name,description,col4,col5,domain_tags,quality_score
///   - type: skill | outcome | tradeoff
///
/// **Agency CSV (9 or 12 columns):** type,name,description,quality,domain_specificity,domain,origin_instance_id,parent_content_hash,scope[,parent_ids,generation,created_by]
///   - type: role_component | desired_outcome | trade_off_config
///
/// Both formats are auto-detected. Legacy type names (skill/outcome/tradeoff) are also
/// accepted in the 9-column format and vice versa.
pub fn run(
    workgraph_dir: &Path,
    csv_path: &str,
    dry_run: bool,
    tag: Option<&str>,
) -> Result<ImportCounts> {
    let csv_bytes =
        std::fs::read(csv_path).with_context(|| format!("Failed to read '{}'", csv_path))?;
    run_from_bytes(workgraph_dir, csv_path, &csv_bytes, dry_run, tag)
}

/// Parse columns from Agency's 9- or 12-column CSV format.
///
/// Columns: type(0), name(1), description(2), quality(3), domain_specificity(4),
///          domain(5), origin_instance_id(6), parent_content_hash(7), scope(8),
///          parent_ids(9), generation(10), created_by(11)
fn parse_agency_columns(record: &csv::StringRecord) -> ParsedCsvColumns {
    // quality (col3): integer 0-100, map to avg_score as 0.0-1.0
    let quality_raw = record.get(3).map(|s| s.trim()).unwrap_or("");
    let quality_score: Option<f64> = quality_raw.parse::<f64>().ok().map(|v| v / 100.0);

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

    let parent_ids_raw = record.get(9).map(|s| s.trim()).unwrap_or("");
    let mut parent_ids = Vec::new();
    if let Some(ref pch) = parent_content_hash
        && !pch.is_empty()
    {
        parent_ids.push(pch.clone());
    }
    for parent_id in parse_parent_ids_column(parent_ids_raw) {
        if !parent_ids.contains(&parent_id) {
            parent_ids.push(parent_id);
        }
    }

    let generation = record
        .get(10)
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);
    let created_by = record
        .get(11)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let mut metadata = HashMap::new();
    if !quality_raw.is_empty() {
        metadata.insert("agency_quality".to_string(), quality_raw.to_string());
    }
    if !scope.is_empty() {
        metadata.insert("scope".to_string(), scope);
    }
    if !domain_specificity.is_empty() {
        metadata.insert("domain_specificity".to_string(), domain_specificity);
    }
    if !origin_instance_id.is_empty() {
        metadata.insert("origin_instance_id".to_string(), origin_instance_id);
    }
    if let Some(ref pch) = parent_content_hash
        && !pch.is_empty()
    {
        metadata.insert("parent_content_hash".to_string(), pch.clone());
    }
    if !parent_ids_raw.is_empty() {
        metadata.insert("parent_ids".to_string(), parent_ids_raw.to_string());
    }

    ParsedCsvColumns {
        quality_score,
        domain_tags,
        metadata,
        parent_ids,
        generation,
        created_by,
    }
}

fn parse_parent_ids_column(raw: &str) -> Vec<String> {
    if raw.is_empty() {
        return Vec::new();
    }
    if let Ok(ids) = serde_json::from_str::<Vec<String>>(raw) {
        return ids
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Parse columns from the legacy 7-column CSV format.
///
/// Columns: type(0), name(1), description(2), col4(3), col5(4), domain_tags(5), quality_score(6)
fn parse_legacy_columns(record: &csv::StringRecord) -> ParsedCsvColumns {
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

    ParsedCsvColumns {
        quality_score,
        domain_tags,
        metadata: HashMap::new(),
        parent_ids: Vec::new(),
        generation: 0,
        created_by: None,
    }
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
        let wg_dir = tmp.path().join(".wg");
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
        let wg_dir = tmp.path().join(".wg");
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
        let wg_dir = tmp.path().join(".wg");
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
        let wg_dir = tmp.path().join(".wg");
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
        let wg_dir2 = tmp2.path().join(".wg");
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
        let wg_dir = tmp.path().join(".wg");
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

        assert_eq!(
            comp_count, 2,
            "Expected 2 components (task + meta:assigner)"
        );
        assert_eq!(out_count, 1, "Expected 1 outcome");
        assert_eq!(trade_count, 1, "Expected 1 tradeoff");
    }

    #[test]
    fn test_agency_import_9col_metadata_preserved() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = write_agency_format_csv(tmp.path());
        run(&wg_dir, csv_path.to_str().unwrap(), false, None).unwrap();

        // Read all components and check metadata
        let components_dir = wg_dir.join("agency/primitives/components");
        let mut found_task_scope = false;
        let mut found_meta_scope = false;

        for entry in std::fs::read_dir(&components_dir).unwrap() {
            let entry = entry.unwrap();
            let component: RoleComponent = agency::load_component(&entry.path()).unwrap();

            // Check that domain_tags are populated
            assert!(
                !component.domain_tags.is_empty(),
                "domain_tags should be populated"
            );

            // Check scope metadata
            if let Some(scope) = component.metadata.get("scope") {
                if scope == "task" {
                    found_task_scope = true;
                    assert_eq!(
                        component
                            .metadata
                            .get("domain_specificity")
                            .map(|s| s.as_str()),
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
        assert!(
            found_meta_scope,
            "Should have a meta:assigner-scope component"
        );
    }

    #[test]
    fn test_agency_import_9col_quality_maps_to_score() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = write_agency_format_csv(tmp.path());
        run(&wg_dir, csv_path.to_str().unwrap(), false, None).unwrap();

        // Read the outcome and check that quality (90) mapped to avg_score (0.90)
        let outcomes_dir = wg_dir.join("agency/primitives/outcomes");
        for entry in std::fs::read_dir(&outcomes_dir).unwrap() {
            let entry = entry.unwrap();
            let outcome: DesiredOutcome = agency::load_outcome(&entry.path()).unwrap();
            assert_eq!(outcome.performance.avg_score, Some(0.90));
        }
    }

    #[test]
    fn test_agency_import_9col_domain_maps_to_tags() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = write_agency_format_csv(tmp.path());
        run(&wg_dir, csv_path.to_str().unwrap(), false, None).unwrap();

        // Read the tradeoff and check domain tags
        let tradeoffs_dir = wg_dir.join("agency/primitives/tradeoffs");
        for entry in std::fs::read_dir(&tradeoffs_dir).unwrap() {
            let entry = entry.unwrap();
            let tradeoff: TradeoffConfig = agency::load_tradeoff(&entry.path()).unwrap();
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
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = write_agency_format_csv(tmp.path());
        run(&wg_dir, csv_path.to_str().unwrap(), false, None).unwrap();

        // Tradeoff should use description as-is, with empty acceptable/unacceptable lists
        let tradeoffs_dir = wg_dir.join("agency/primitives/tradeoffs");
        for entry in std::fs::read_dir(&tradeoffs_dir).unwrap() {
            let entry = entry.unwrap();
            let tradeoff: TradeoffConfig = agency::load_tradeoff(&entry.path()).unwrap();
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
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = write_agency_format_csv(tmp.path());
        run(&wg_dir, csv_path.to_str().unwrap(), false, None).unwrap();

        // The "Identify Gaps" component has parent_content_hash=abc123
        let components_dir = wg_dir.join("agency/primitives/components");
        let mut found_with_parent = false;
        for entry in std::fs::read_dir(&components_dir).unwrap() {
            let entry = entry.unwrap();
            let component: RoleComponent = agency::load_component(&entry.path()).unwrap();
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
        let wg_dir = tmp.path().join(".wg");
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
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = write_agency_format_csv(tmp.path());
        run(&wg_dir, csv_path.to_str().unwrap(), true, None).unwrap();

        let mp = manifest_path(&wg_dir);
        assert!(!mp.exists(), "Manifest should NOT be written on dry run");
    }

    #[test]
    fn test_reimport_updates_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".wg");
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
        assert_eq!(
            manifest1.counts.role_components,
            manifest2.counts.role_components
        );
    }

    // --- Tests for the new URL/upstream import functionality ---

    #[test]
    fn test_agency_pull_run_from_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv = b"type,name,description,quality,domain_specificity,domain,origin_instance_id,parent_content_hash,scope\n\
                     role_component,Test Skill,Does testing,80,high,testing,inst-001,,task\n";

        let counts = run_from_bytes(&wg_dir, "test://fixture.csv", csv, false, None).unwrap();
        assert_eq!(counts.role_components, 1);
        assert_eq!(counts.desired_outcomes, 0);
        assert_eq!(counts.trade_off_configs, 0);

        // Verify manifest was written with source URL
        let manifest = read_manifest(&wg_dir).unwrap().unwrap();
        assert_eq!(manifest.source, "test://fixture.csv");
        assert_eq!(manifest.counts.role_components, 1);
    }

    #[test]
    fn test_agency_pull_read_manifest_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let result = read_manifest(&wg_dir).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_agency_pull_read_manifest_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = write_agency_format_csv(tmp.path());
        run(&wg_dir, csv_path.to_str().unwrap(), false, None).unwrap();

        let manifest = read_manifest(&wg_dir).unwrap().unwrap();
        assert!(!manifest.content_hash.is_empty());
        assert_eq!(manifest.counts.role_components, 2);
    }

    #[test]
    fn test_agency_pull_change_detection_same_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv = b"type,name,description,quality,domain_specificity,domain,origin_instance_id,parent_content_hash,scope\n\
                     role_component,Detect,Detection test,75,low,test,inst-001,,task\n";

        // First import writes manifest
        let counts1 = run_from_bytes(&wg_dir, "test://same.csv", csv, false, None).unwrap();
        assert_eq!(counts1.role_components, 1);

        let manifest = read_manifest(&wg_dir).unwrap().unwrap();
        let hash = sha256_hex(csv);
        assert_eq!(manifest.content_hash, hash);
    }

    #[test]
    fn test_agency_pull_change_detection_different_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv1 = b"type,name,description,quality,domain_specificity,domain,origin_instance_id,parent_content_hash,scope\n\
                      role_component,V1,Version one,75,low,test,inst-001,,task\n";
        let csv2 = b"type,name,description,quality,domain_specificity,domain,origin_instance_id,parent_content_hash,scope\n\
                      role_component,V1,Version one,75,low,test,inst-001,,task\n\
                      role_component,V2,Version two,80,medium,test,inst-002,,task\n";

        run_from_bytes(&wg_dir, "test://v1.csv", csv1, false, None).unwrap();
        let m1 = read_manifest(&wg_dir).unwrap().unwrap();

        run_from_bytes(&wg_dir, "test://v2.csv", csv2, false, None).unwrap();
        let m2 = read_manifest(&wg_dir).unwrap().unwrap();

        assert_ne!(m1.content_hash, m2.content_hash);
        assert_eq!(m2.counts.role_components, 2);
    }

    #[test]
    fn test_agency_pull_import_from_local_via_run_import() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv_path = write_agency_format_csv(tmp.path());

        let opts = ImportOptions {
            csv_path: Some(csv_path.to_str().unwrap().to_string()),
            url: None,
            upstream: false,
            format: None,
            dry_run: false,
            tag: None,
            force: false,
            check: false,
            strict: false,
        };
        let counts = run_import(&wg_dir, opts).unwrap();
        assert_eq!(counts.role_components, 2);
        assert_eq!(counts.desired_outcomes, 1);
        assert_eq!(counts.trade_off_configs, 1);
    }

    #[test]
    fn test_agency_pull_error_multiple_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let opts = ImportOptions {
            csv_path: Some("file.csv".to_string()),
            url: Some("http://example.com/file.csv".to_string()),
            upstream: false,
            format: None,
            dry_run: false,
            tag: None,
            force: false,
            check: false,
            strict: false,
        };
        let result = run_import(&wg_dir, opts);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Specify only one"));
    }

    #[test]
    fn test_agency_pull_error_no_source() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let opts = ImportOptions {
            csv_path: None,
            url: None,
            upstream: false,
            format: None,
            dry_run: false,
            tag: None,
            force: false,
            check: false,
            strict: false,
        };
        let result = run_import(&wg_dir, opts);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Specify one of"));
    }

    #[test]
    fn test_agency_pull_upstream_no_config() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let opts = ImportOptions {
            csv_path: None,
            url: None,
            upstream: true,
            format: None,
            dry_run: false,
            tag: None,
            force: false,
            check: false,
            strict: false,
        };
        let result = run_import(&wg_dir, opts);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No upstream URL configured")
        );
    }

    #[test]
    fn test_agency_pull_url_network_error_graceful() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        // Use a URL that will fail to connect (invalid host)
        let opts = ImportOptions {
            csv_path: None,
            url: Some("http://192.0.2.1:1/nonexistent.csv".to_string()),
            upstream: false,
            format: None,
            dry_run: false,
            tag: None,
            force: false,
            check: false,
            strict: false,
        };
        let result = run_import(&wg_dir, opts);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Failed to fetch") || err_msg.contains("error"),
            "Error should describe network failure, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_agency_pull_run_from_bytes_dry_run() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv = b"type,name,description,quality,domain_specificity,domain,origin_instance_id,parent_content_hash,scope\n\
                     role_component,Dry,Dry run test,80,high,testing,inst-001,,task\n";

        let counts = run_from_bytes(&wg_dir, "test://dry.csv", csv, true, None).unwrap();
        assert_eq!(counts.role_components, 1);

        // No manifest should be written
        assert!(read_manifest(&wg_dir).unwrap().is_none());
        // No agency directory should be created
        assert!(!wg_dir.join("agency/primitives/components").exists());
    }

    #[test]
    fn test_agency_pull_run_from_bytes_with_tag() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv = b"type,name,description,quality,domain_specificity,domain,origin_instance_id,parent_content_hash,scope\n\
                     role_component,Tagged,Tagged import,80,high,testing,inst-001,,task\n";

        run_from_bytes(&wg_dir, "test://tagged.csv", csv, false, Some("custom-tag")).unwrap();

        // Verify component was saved with agency-compatible import provenance.
        let components_dir = wg_dir.join("agency/primitives/components");
        let entries: Vec<_> = std::fs::read_dir(&components_dir).unwrap().collect();
        assert_eq!(entries.len(), 1);
        let component: RoleComponent =
            agency::load_component(&entries[0].as_ref().unwrap().path()).unwrap();
        assert_eq!(component.lineage.created_by, "import");
        assert_eq!(component.access_control.owner, "custom-tag");
    }

    #[test]
    fn test_agency_pull_additive_merge() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let csv1 = b"type,name,description,quality,domain_specificity,domain,origin_instance_id,parent_content_hash,scope\n\
                      role_component,First,First component,80,high,testing,inst-001,,task\n";
        let csv2 = b"type,name,description,quality,domain_specificity,domain,origin_instance_id,parent_content_hash,scope\n\
                      role_component,Second,Second component,85,high,testing,inst-002,,task\n";

        run_from_bytes(&wg_dir, "test://v1.csv", csv1, false, None).unwrap();
        let count1 = std::fs::read_dir(wg_dir.join("agency/primitives/components"))
            .unwrap()
            .count();
        assert_eq!(count1, 1);

        run_from_bytes(&wg_dir, "test://v2.csv", csv2, false, None).unwrap();
        let count2 = std::fs::read_dir(wg_dir.join("agency/primitives/components"))
            .unwrap()
            .count();
        // Second import should ADD the new component, not remove the first
        assert_eq!(count2, 2);
    }

    /// Per-scope variant: upstream uses the same description+name with two
    /// different scopes ("task" vs "meta:assigner"). Default behavior keeps
    /// the first row, warns, and records a same-import collision.
    #[test]
    fn test_agency_import_per_scope_variant_collision() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        // Two rows: same (type, name, description), different scope.
        let csv = b"type,name,description,quality,domain_specificity,domain,origin_instance_id,parent_content_hash,scope\n\
                     role_component,forward-compatible-deferral-spec,Deferral spec contents,80,high,management,inst-001,,task\n\
                     role_component,forward-compatible-deferral-spec,Deferral spec contents,80,high,management,inst-001,,meta:assigner\n";

        let (counts, collisions) =
            run_from_bytes_with(&wg_dir, "test://scope-variant.csv", csv, false, None, false)
                .unwrap();

        // Exactly one row saved; the duplicate-by-hash row is dropped + recorded.
        assert_eq!(counts.role_components, 1);
        assert_eq!(collisions.len(), 1);
        let coll = &collisions[0];
        assert_eq!(coll.kind, "component");
        assert_eq!(coll.kept_name, "forward-compatible-deferral-spec");
        assert_eq!(coll.dropped_name, "forward-compatible-deferral-spec");
        assert_eq!(coll.kept_scope.as_deref(), Some("task"));
        assert_eq!(coll.dropped_scope.as_deref(), Some("meta:assigner"));
        assert_eq!(coll.origin, CollisionOrigin::SameImport);

        // Strict mode errors on the same input.
        let tmp2 = tempfile::tempdir().unwrap();
        let wg_dir2 = tmp2.path().join(".wg");
        std::fs::create_dir_all(&wg_dir2).unwrap();
        let err = run_from_bytes_with(&wg_dir2, "test://scope-variant.csv", csv, false, None, true)
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("--strict"), "strict error should mention --strict, got: {}", msg);
        assert!(msg.contains("forward-compatible-deferral-spec"));
    }

    /// Same-description name collision: an upstream row whose description
    /// matches a primitive already on disk (a locally-seeded one) gets
    /// detected as an Existing-origin collision and skipped.
    #[test]
    fn test_agency_import_same_description_name_collision_with_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        // First import: seed a primitive with one name.
        let seed = b"type,name,description,quality,domain_specificity,domain,origin_instance_id,parent_content_hash,scope\n\
                      role_component,adapt-research-synthesis-for-non-domain-audience,Identical description text,90,high,research,inst-seed,,task\n";
        let (seed_counts, seed_coll) =
            run_from_bytes_with(&wg_dir, "test://seed.csv", seed, false, None, false).unwrap();
        assert_eq!(seed_counts.role_components, 1);
        assert!(seed_coll.is_empty());

        // Second import: an upstream row with a different name but identical
        // description. Default behavior should warn + skip (preserving the
        // locally-seeded row).
        let upstream = b"type,name,description,quality,domain_specificity,domain,origin_instance_id,parent_content_hash,scope\n\
                          role_component,identify-write-up-audience-and-adapt,Identical description text,80,medium,writing,inst-up,,task\n";
        let (upstream_counts, upstream_coll) =
            run_from_bytes_with(&wg_dir, "test://upstream.csv", upstream, false, None, false)
                .unwrap();
        assert_eq!(
            upstream_counts.role_components, 0,
            "upstream row should be skipped, not saved"
        );
        assert_eq!(upstream_coll.len(), 1);
        let coll = &upstream_coll[0];
        assert_eq!(coll.origin, CollisionOrigin::Existing);
        assert_eq!(
            coll.kept_name, "adapt-research-synthesis-for-non-domain-audience",
            "first-write-wins: locally-seeded primitive must not be silently overwritten"
        );
        assert_eq!(coll.dropped_name, "identify-write-up-audience-and-adapt");

        // Verify on-disk state preserves the seeded name.
        let comp_dir = wg_dir.join("agency/primitives/components");
        let entries: Vec<_> = std::fs::read_dir(&comp_dir).unwrap().collect();
        assert_eq!(entries.len(), 1);
        let comp: RoleComponent =
            agency::load_component(&entries[0].as_ref().unwrap().path()).unwrap();
        assert_eq!(comp.name, "adapt-research-synthesis-for-non-domain-audience");

        // Strict mode errors on the same upstream import against a seeded file.
        let tmp2 = tempfile::tempdir().unwrap();
        let wg_dir2 = tmp2.path().join(".wg");
        std::fs::create_dir_all(&wg_dir2).unwrap();
        run_from_bytes_with(&wg_dir2, "test://seed.csv", seed, false, None, false).unwrap();
        let err = run_from_bytes_with(
            &wg_dir2,
            "test://upstream.csv",
            upstream,
            false,
            None,
            true,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("--strict"),
            "strict error should mention --strict, got: {}",
            err
        );
    }
}
