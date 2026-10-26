//! Agency federation: transferring agency entities between stores.
//!
//! Both `wg agency pull` and `wg agency push` are thin wrappers around `transfer()`,
//! differing only in which store is source vs. target.
//!
//! NOTE: this module was `crate::federation` until the WG-Fed cluster was gated
//! behind the non-default `federation` cargo feature. The agency-store transfer and
//! `federation.yaml` CONFIG plumbing stay unconditional (the `wg agency push/pull/
//! merge/remote` verbs and `wg show`/`wg why-blocked`'s cross-repo dependency
//! rendering still use them); the WG-Fed peer/wgid/endpoint machinery lives in the
//! gated `crate::federation`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::agency::{
    AccessPolicy, AgencyStore, Agent, DesiredOutcome, EvaluationRef, Lineage, LocalStore,
    PerformanceRecord, Role, RoleComponent, TradeoffConfig,
};
use crate::service::is_process_alive;

// ---------------------------------------------------------------------------
// Federation config: named remotes stored in .wg/federation.yaml
// ---------------------------------------------------------------------------

/// A named remote agency store reference.
///
/// Wave 4 (ADR-fed-002): a remote may now be **key-based** — carry a `wgid:` address
/// and one or more delivery `endpoints` (node/relay URIs) — *alongside* (or instead
/// of) the legacy filesystem `path`. Path-based remotes keep working unchanged; the
/// new fields default to empty so old `federation.yaml` files parse as before.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Remote {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<String>,
    /// The peer's self-certifying `wgid:` address (key-based federation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wgid: Option<String>,
    /// Delivery endpoints (node/relay base URIs, e.g. `http://host:port`) advertised
    /// for this peer. Used by the resolution cascade as the cached endpoint hint.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoints: Vec<String>,
}

/// A peer WG project (another repo with its own .wg/).
///
/// Wave 4 (ADR-fed-001 §D5, ADR-fed-002): a peer is now resolvable two ways that
/// **coexist** — by filesystem `path` (the legacy same-host federation) and/or by
/// self-certifying `wgid:` + delivery `endpoints` (key-based, cross-host messaging
/// over the node inbox). `path` is optional so a pure key-based peer needs no path;
/// old path-based `federation.yaml` peers still resolve untouched.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PeerConfig {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The peer's self-certifying `wgid:` address (key-based federation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wgid: Option<String>,
    /// Delivery endpoints (node/relay base URIs, e.g. `http://host:port`) advertised
    /// for this peer — the "cached signed endpoint record" rung of the cascade.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoints: Vec<String>,
    /// The authorizer's **local trust assertion** about this peer
    /// ([`crate::graph::TrustLevel`] — the one trust dial, never self-certified by the
    /// peer). This is the canonical author-trust source the inbound review gate reads
    /// (`wg msg poll --review`) so review depth is *derived*, not hand-passed; it is
    /// unified with the WG-Exec provider-pool trust through [`crate::trust`]. `None`
    /// means "enrolled but unvouched" and resolves to `Provisional` (TOFU) — a stranger
    /// who is not a peer at all is `Unknown` (fail-closed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust: Option<crate::graph::TrustLevel>,
}

/// Top-level federation.yaml structure.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct FederationConfig {
    #[serde(default)]
    pub remotes: BTreeMap<String, Remote>,
    /// Peer WG projects for cross-repo communication.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub peers: BTreeMap<String, PeerConfig>,
    /// This graph's own WG node inbox base URL (e.g. `http://host:port`), used as the
    /// default publish/poll target for key-based federation. Optional — the node is a
    /// convenience, never a mandatory root (ADR-fed-002 §D2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// An optional directory node base URL queried by the resolution cascade when a
    /// peer's endpoints are not already cached (ADR-fed-001 §D5 stage 2). A forged
    /// directory can only *hint*; it can never override the local signature check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
}

/// Load federation config from .wg/federation.yaml.
/// Returns default (empty) if the file doesn't exist.
pub fn load_federation_config(workgraph_dir: &Path) -> Result<FederationConfig, anyhow::Error> {
    let path = workgraph_dir.join("federation.yaml");
    if !path.exists() {
        return Ok(FederationConfig::default());
    }
    let content = std::fs::read_to_string(&path)?;
    let config: FederationConfig = serde_yaml::from_str(&content)?;
    Ok(config)
}

/// Save federation config to .wg/federation.yaml.
pub fn save_federation_config(
    workgraph_dir: &Path,
    config: &FederationConfig,
) -> Result<(), anyhow::Error> {
    let path = workgraph_dir.join("federation.yaml");
    let content = serde_yaml::to_string(config)?;
    std::fs::write(&path, content)?;
    Ok(())
}

/// Resolve a store reference, checking named remotes in federation.yaml first,
/// then falling back to filesystem path resolution.
pub fn resolve_store_with_remotes(
    reference: &str,
    workgraph_dir: &Path,
) -> Result<LocalStore, anyhow::Error> {
    let config = load_federation_config(workgraph_dir)?;
    if let Some(remote) = config.remotes.get(reference) {
        return resolve_store(&remote.path);
    }
    resolve_store(reference)
}

/// Update the last_sync timestamp for a named remote (if it exists).
pub fn touch_remote_sync(workgraph_dir: &Path, name: &str) -> Result<(), anyhow::Error> {
    let mut config = load_federation_config(workgraph_dir)?;
    if let Some(remote) = config.remotes.get_mut(name) {
        remote.last_sync = Some(chrono::Utc::now().to_rfc3339());
        save_federation_config(workgraph_dir, &config)?;
    }
    Ok(())
}

/// Which entity types to transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityFilter {
    All,
    Components,
    Outcomes,
    Roles,
    Tradeoffs,
    Agents,
}

/// Options controlling a transfer operation.
#[derive(Debug, Clone)]
pub struct TransferOptions {
    /// Only preview, don't write.
    pub dry_run: bool,
    /// Skip merging performance data.
    pub no_performance: bool,
    /// Skip copying evaluation JSON files.
    pub no_evaluations: bool,
    /// Overwrite target metadata instead of merging.
    pub force: bool,
    /// Only transfer specific entity IDs.
    pub entity_ids: Vec<String>,
    /// Filter by entity type.
    pub entity_filter: EntityFilter,
}

impl Default for TransferOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            no_performance: false,
            no_evaluations: false,
            force: false,
            entity_ids: Vec::new(),
            entity_filter: EntityFilter::All,
        }
    }
}

/// Summary of what was transferred.
#[derive(Debug, Clone, Default)]
pub struct TransferSummary {
    pub components_added: usize,
    pub components_updated: usize,
    pub components_skipped: usize,
    pub components_access_denied: usize,
    pub outcomes_added: usize,
    pub outcomes_updated: usize,
    pub outcomes_skipped: usize,
    pub outcomes_access_denied: usize,
    pub roles_added: usize,
    pub roles_updated: usize,
    pub roles_skipped: usize,
    pub tradeoffs_added: usize,
    pub tradeoffs_updated: usize,
    pub tradeoffs_skipped: usize,
    pub tradeoffs_access_denied: usize,
    pub agents_added: usize,
    pub agents_updated: usize,
    pub agents_skipped: usize,
    pub evaluations_added: usize,
    pub evaluations_skipped: usize,
}

impl std::fmt::Display for TransferSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "  Components:   +{} new, {} updated, {} skipped{}",
            self.components_added,
            self.components_updated,
            self.components_skipped,
            if self.components_access_denied > 0 {
                format!(", {} denied (access policy)", self.components_access_denied)
            } else {
                String::new()
            }
        )?;
        writeln!(
            f,
            "  Outcomes:     +{} new, {} updated, {} skipped{}",
            self.outcomes_added,
            self.outcomes_updated,
            self.outcomes_skipped,
            if self.outcomes_access_denied > 0 {
                format!(", {} denied (access policy)", self.outcomes_access_denied)
            } else {
                String::new()
            }
        )?;
        writeln!(
            f,
            "  Tradeoffs:    +{} new, {} updated, {} skipped{}",
            self.tradeoffs_added,
            self.tradeoffs_updated,
            self.tradeoffs_skipped,
            if self.tradeoffs_access_denied > 0 {
                format!(", {} denied (access policy)", self.tradeoffs_access_denied)
            } else {
                String::new()
            }
        )?;
        writeln!(
            f,
            "  Roles:        +{} new, {} updated, {} skipped",
            self.roles_added, self.roles_updated, self.roles_skipped
        )?;
        writeln!(
            f,
            "  Agents:       +{} new, {} updated, {} skipped",
            self.agents_added, self.agents_updated, self.agents_skipped
        )?;
        write!(
            f,
            "  Evaluations:  +{} new, {} skipped",
            self.evaluations_added, self.evaluations_skipped
        )
    }
}

/// Resolve a store reference string to a `LocalStore`.
///
/// Resolution order (per §3.1 of the design doc):
/// 1. Absolute path or `~/` → filesystem path, look for `agency/` or `.wg/agency/`
/// 2. Relative path → resolve from CWD
///
/// Named remotes (from `.wg/federation.yaml`) are a future extension.
pub fn resolve_store(reference: &str) -> Result<LocalStore, anyhow::Error> {
    let expanded = if let Some(suffix) = reference.strip_prefix("~/") {
        let home =
            dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
        home.join(suffix)
    } else {
        PathBuf::from(reference)
    };

    let path = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()?.join(expanded)
    };

    // Canonicalize if it exists
    let path = path.canonicalize().unwrap_or(path);

    // Check for agency store in several locations:
    // 1. path itself is the agency dir (has roles/ or cache/roles/ or evaluations/)
    // 2. path/agency/ is the agency dir
    // 3. path/.wg/agency/ is the agency dir
    let is_agency_dir = |p: &PathBuf| {
        p.join("cache/roles").is_dir()
            || p.join("cache/roles").is_dir()
            || p.join("evaluations").is_dir()
    };
    if is_agency_dir(&path) {
        return Ok(LocalStore::new(path));
    }
    let agency_sub = path.join("agency");
    if is_agency_dir(&agency_sub) {
        return Ok(LocalStore::new(agency_sub));
    }
    let wg_agency = path.join(".wg").join("agency");
    if is_agency_dir(&wg_agency) {
        return Ok(LocalStore::new(wg_agency));
    }

    // Target doesn't exist yet — return the best-guess path.
    // For push, we create it. For pull, the caller can error.
    // Prefer .wg/agency if parent looks like a project dir.
    if path.join(".wg").is_dir() {
        Ok(LocalStore::new(wg_agency))
    } else if path.join("agency").is_dir()
        || path.file_name().map(|n| n != "agency").unwrap_or(true)
    {
        // If path/agency exists but has no roles, or if path is not named "agency",
        // assume we want path/agency/ (bare store convention).
        // But if the path itself ends in "agency", use it directly.
        if path.file_name().map(|n| n == "agency").unwrap_or(false) {
            Ok(LocalStore::new(path))
        } else {
            Ok(LocalStore::new(agency_sub))
        }
    } else {
        Ok(LocalStore::new(path))
    }
}

pub fn ensure_store_dirs(store: &LocalStore) -> Result<(), anyhow::Error> {
    crate::agency::init(store.store_path())?;
    Ok(())
}

/// Check whether a primitive's access policy allows federation transfer.
///
/// - `Private` primitives are never shared.
/// - `Shared` primitives are shared (the `shared_peers` list is advisory metadata;
///   enforcement of per-peer restrictions is a future extension).
/// - `Open` primitives are always shared.
fn access_policy_allows_transfer(policy: &AccessPolicy) -> bool {
    !matches!(policy, AccessPolicy::Private)
}

/// Transfer entities from `source` to `target`.
///
/// This is the core operation used by both pull (remote→local) and push (local→remote).
/// Transfers primitives (components, outcomes, tradeoffs), cache entries (roles, agents),
/// evaluations. AccessPolicy is enforced on primitives: Private
/// primitives are never transferred.
pub fn transfer(
    source: &LocalStore,
    target: &LocalStore,
    opts: &TransferOptions,
) -> Result<TransferSummary, anyhow::Error> {
    let mut summary = TransferSummary::default();

    if !opts.dry_run {
        ensure_store_dirs(target)?;
    }

    let has_filter = !opts.entity_ids.is_empty();
    let matches_filter = |id: &str| -> bool {
        opts.entity_ids
            .iter()
            .any(|prefix| id.starts_with(prefix.as_str()))
    };

    // --- Load source primitives ---
    let source_components = if matches!(
        opts.entity_filter,
        EntityFilter::All | EntityFilter::Components
    ) {
        source.load_components().unwrap_or_default()
    } else {
        Vec::new()
    };
    let source_outcomes = if matches!(
        opts.entity_filter,
        EntityFilter::All | EntityFilter::Outcomes
    ) {
        source.load_outcomes().unwrap_or_default()
    } else {
        Vec::new()
    };

    // --- Load source cache + composition entities ---
    let source_roles = if matches!(
        opts.entity_filter,
        EntityFilter::All | EntityFilter::Roles | EntityFilter::Agents
    ) {
        source.load_roles().unwrap_or_default()
    } else {
        Vec::new()
    };
    let source_tradeoffs = if matches!(
        opts.entity_filter,
        EntityFilter::All | EntityFilter::Tradeoffs | EntityFilter::Agents
    ) {
        source.load_tradeoffs().unwrap_or_default()
    } else {
        Vec::new()
    };
    let source_agents = if matches!(opts.entity_filter, EntityFilter::All | EntityFilter::Agents) {
        source.load_agents().unwrap_or_default()
    } else {
        Vec::new()
    };

    // Build lookup maps
    let role_map: HashMap<String, &Role> = source_roles.iter().map(|r| (r.id.clone(), r)).collect();
    let motivation_map: HashMap<String, &TradeoffConfig> =
        source_tradeoffs.iter().map(|m| (m.id.clone(), m)).collect();

    // --- Collect primitives to transfer (with AccessPolicy enforcement) ---
    let mut components_to_transfer: Vec<&RoleComponent> = Vec::new();
    if matches!(
        opts.entity_filter,
        EntityFilter::All | EntityFilter::Components
    ) {
        for comp in &source_components {
            if has_filter && !matches_filter(&comp.id) {
                continue;
            }
            if !access_policy_allows_transfer(&comp.access_control.policy) {
                summary.components_access_denied += 1;
                continue;
            }
            components_to_transfer.push(comp);
        }
    }

    let mut outcomes_to_transfer: Vec<&DesiredOutcome> = Vec::new();
    if matches!(
        opts.entity_filter,
        EntityFilter::All | EntityFilter::Outcomes
    ) {
        for outcome in &source_outcomes {
            if has_filter && !matches_filter(&outcome.id) {
                continue;
            }
            if !access_policy_allows_transfer(&outcome.access_control.policy) {
                summary.outcomes_access_denied += 1;
                continue;
            }
            outcomes_to_transfer.push(outcome);
        }
    }

    // Determine which cache/composition entities to transfer
    let mut roles_to_transfer: Vec<&Role> = Vec::new();
    let mut tradeoffs_to_transfer: Vec<&TradeoffConfig> = Vec::new();
    let mut agents_to_transfer: Vec<&Agent> = Vec::new();

    // Collect agents
    if matches!(opts.entity_filter, EntityFilter::All | EntityFilter::Agents) {
        for agent in &source_agents {
            if has_filter && !matches_filter(&agent.id) {
                continue;
            }
            agents_to_transfer.push(agent);
        }
    }

    // Collect directly-requested roles and motivations
    if matches!(opts.entity_filter, EntityFilter::All | EntityFilter::Roles) {
        for role in &source_roles {
            if has_filter && !matches_filter(&role.id) {
                continue;
            }
            roles_to_transfer.push(role);
        }
    }
    if matches!(
        opts.entity_filter,
        EntityFilter::All | EntityFilter::Tradeoffs
    ) {
        for motivation in &source_tradeoffs {
            if has_filter && !matches_filter(&motivation.id) {
                continue;
            }
            // AccessPolicy enforcement on tradeoff primitives
            if !access_policy_allows_transfer(&motivation.access_control.policy) {
                summary.tradeoffs_access_denied += 1;
                continue;
            }
            tradeoffs_to_transfer.push(motivation);
        }
    }

    // Load target entities ONCE for O(1) lookups during both referential
    // integrity checks and the transfer phase (avoids repeated filesystem reads).
    // Errors are propagated — if target YAML is corrupt, we must not silently
    // treat it as empty (which would cause overwrites instead of merges).
    let target_component_map: HashMap<String, RoleComponent> = target
        .load_components()
        .unwrap_or_default()
        .into_iter()
        .map(|c| (c.id.clone(), c))
        .collect();
    let target_outcome_map: HashMap<String, DesiredOutcome> = target
        .load_outcomes()
        .unwrap_or_default()
        .into_iter()
        .map(|o| (o.id.clone(), o))
        .collect();
    let target_role_map: HashMap<String, Role> = target
        .load_roles()?
        .into_iter()
        .map(|r| (r.id.clone(), r))
        .collect();
    let target_tradeoff_map: HashMap<String, TradeoffConfig> = target
        .load_tradeoffs()?
        .into_iter()
        .map(|m| (m.id.clone(), m))
        .collect();
    let target_agent_map: HashMap<String, Agent> = target
        .load_agents()?
        .into_iter()
        .map(|a| (a.id.clone(), a))
        .collect();

    // Referential integrity: when transferring agents, also transfer their
    // referenced roles and motivations if not already in the target.
    let mut dep_role_ids: HashSet<String> = HashSet::new();
    let mut dep_tradeoff_ids: HashSet<String> = HashSet::new();
    for agent in &agents_to_transfer {
        if !target_role_map.contains_key(&agent.role_id) {
            dep_role_ids.insert(agent.role_id.clone());
        }
        if !target_tradeoff_map.contains_key(&agent.tradeoff_id) {
            dep_tradeoff_ids.insert(agent.tradeoff_id.clone());
        }
    }
    // Add dependency roles not already in the transfer set.
    // Per §7.1: if a dependency doesn't exist in source, that's a broken agent — error out.
    let existing_role_ids: HashSet<String> =
        roles_to_transfer.iter().map(|r| r.id.clone()).collect();
    for dep_id in &dep_role_ids {
        if !existing_role_ids.contains(dep_id) {
            if let Some(role) = role_map.get(dep_id) {
                roles_to_transfer.push(role);
            } else {
                return Err(anyhow::anyhow!(
                    "Agent references role '{}' which does not exist in source store \
                     (broken referential integrity, see design doc §7.1)",
                    dep_id
                ));
            }
        }
    }
    let existing_tradeoff_ids: HashSet<String> =
        tradeoffs_to_transfer.iter().map(|m| m.id.clone()).collect();
    for dep_id in &dep_tradeoff_ids {
        if !existing_tradeoff_ids.contains(dep_id) {
            if let Some(motivation) = motivation_map.get(dep_id) {
                tradeoffs_to_transfer.push(motivation);
            } else {
                return Err(anyhow::anyhow!(
                    "Agent references motivation '{}' which does not exist in source store \
                     (broken referential integrity, see design doc §7.1)",
                    dep_id
                ));
            }
        }
    }

    // --- Transfer components (primitives) ---
    for comp in &components_to_transfer {
        if let Some(existing) = target_component_map.get(&comp.id) {
            if opts.force || opts.no_performance {
                let mut merged = (*comp).clone();
                if opts.no_performance {
                    merged.performance = existing.performance.clone();
                }
                if !opts.dry_run {
                    target.save_component(&merged)?;
                }
                summary.components_updated += 1;
            } else {
                let merged = merge_component(existing, comp);
                if merged_component_differs(existing, &merged) {
                    if !opts.dry_run {
                        target.save_component(&merged)?;
                    }
                    summary.components_updated += 1;
                } else {
                    summary.components_skipped += 1;
                }
            }
        } else {
            let mut to_save = (*comp).clone();
            if opts.no_performance {
                to_save.performance = PerformanceRecord::default();
            }
            if !opts.dry_run {
                target.save_component(&to_save)?;
            }
            summary.components_added += 1;
        }
    }

    // --- Transfer outcomes (primitives) ---
    for outcome in &outcomes_to_transfer {
        if let Some(existing) = target_outcome_map.get(&outcome.id) {
            if opts.force || opts.no_performance {
                let mut merged = (*outcome).clone();
                if opts.no_performance {
                    merged.performance = existing.performance.clone();
                }
                if !opts.dry_run {
                    target.save_outcome(&merged)?;
                }
                summary.outcomes_updated += 1;
            } else {
                let merged = merge_outcome(existing, outcome);
                if merged_outcome_differs(existing, &merged) {
                    if !opts.dry_run {
                        target.save_outcome(&merged)?;
                    }
                    summary.outcomes_updated += 1;
                } else {
                    summary.outcomes_skipped += 1;
                }
            }
        } else {
            let mut to_save = (*outcome).clone();
            if opts.no_performance {
                to_save.performance = PerformanceRecord::default();
            }
            if !opts.dry_run {
                target.save_outcome(&to_save)?;
            }
            summary.outcomes_added += 1;
        }
    }

    // Transfer roles
    for role in &roles_to_transfer {
        if let Some(existing) = target_role_map.get(&role.id) {
            // Entity exists — check if metadata differs and merge
            if opts.force || opts.no_performance {
                let mut merged = (*role).clone();
                if opts.no_performance {
                    merged.performance = existing.performance.clone();
                }
                if !opts.dry_run {
                    target.save_role(&merged)?;
                }
                summary.roles_updated += 1;
            } else {
                // Merge metadata
                let merged = merge_role(existing, role);
                if merged_role_differs(existing, &merged) {
                    if !opts.dry_run {
                        target.save_role(&merged)?;
                    }
                    summary.roles_updated += 1;
                } else {
                    summary.roles_skipped += 1;
                }
            }
        } else {
            let mut to_save = (*role).clone();
            if opts.no_performance {
                to_save.performance = PerformanceRecord::default();
            }
            if !opts.dry_run {
                target.save_role(&to_save)?;
            }
            summary.roles_added += 1;
        }
    }

    // Transfer motivations
    for motivation in &tradeoffs_to_transfer {
        if let Some(existing) = target_tradeoff_map.get(&motivation.id) {
            if opts.force || opts.no_performance {
                let mut merged = (*motivation).clone();
                if opts.no_performance {
                    merged.performance = existing.performance.clone();
                }
                if !opts.dry_run {
                    target.save_tradeoff(&merged)?;
                }
                summary.tradeoffs_updated += 1;
            } else {
                let merged = merge_tradeoff(existing, motivation);
                if merged_tradeoff_differs(existing, &merged) {
                    if !opts.dry_run {
                        target.save_tradeoff(&merged)?;
                    }
                    summary.tradeoffs_updated += 1;
                } else {
                    summary.tradeoffs_skipped += 1;
                }
            }
        } else {
            let mut to_save = (*motivation).clone();
            if opts.no_performance {
                to_save.performance = PerformanceRecord::default();
            }
            if !opts.dry_run {
                target.save_tradeoff(&to_save)?;
            }
            summary.tradeoffs_added += 1;
        }
    }

    // Transfer agents
    for agent in &agents_to_transfer {
        if let Some(existing) = target_agent_map.get(&agent.id) {
            if opts.force || opts.no_performance {
                let mut merged = (*agent).clone();
                if opts.no_performance {
                    merged.performance = existing.performance.clone();
                }
                if !opts.dry_run {
                    target.save_agent(&merged)?;
                }
                summary.agents_updated += 1;
            } else {
                let merged = merge_agent(existing, agent);
                if merged_agent_differs(existing, &merged) {
                    if !opts.dry_run {
                        target.save_agent(&merged)?;
                    }
                    summary.agents_updated += 1;
                } else {
                    summary.agents_skipped += 1;
                }
            }
        } else {
            let mut to_save = (*agent).clone();
            if opts.no_performance {
                to_save.performance = PerformanceRecord::default();
            }
            if !opts.dry_run {
                target.save_agent(&to_save)?;
            }
            summary.agents_added += 1;
        }
    }

    // Transfer evaluations
    if !opts.no_evaluations
        && matches!(opts.entity_filter, EntityFilter::All | EntityFilter::Agents)
    {
        let source_evals = source.load_evaluations().unwrap_or_default();
        let target_evals: HashSet<String> = target
            .load_evaluations()
            .unwrap_or_default()
            .iter()
            .map(|e| e.id.clone())
            .collect();

        // Build filter sets ONCE outside the eval loop
        let eval_agent_ids: HashSet<&String> = agents_to_transfer.iter().map(|a| &a.id).collect();
        let eval_role_ids: HashSet<&String> = roles_to_transfer.iter().map(|r| &r.id).collect();
        let eval_tradeoff_ids: HashSet<&String> =
            tradeoffs_to_transfer.iter().map(|m| &m.id).collect();

        for eval in &source_evals {
            // If filtering by entity, only transfer evals for transferred agents/roles/motivations
            if has_filter {
                let relevant = eval_agent_ids.contains(&eval.agent_id)
                    || eval_role_ids.contains(&eval.role_id)
                    || eval_tradeoff_ids.contains(&eval.tradeoff_id);
                if !relevant {
                    continue;
                }
            }

            if target_evals.contains(&eval.id) {
                summary.evaluations_skipped += 1;
            } else {
                if !opts.dry_run {
                    target.save_evaluation(eval)?;
                }
                summary.evaluations_added += 1;
            }
        }
    }

    Ok(summary)
}

// ---------------------------------------------------------------------------
// Metadata merge helpers (§6 of design doc)
// ---------------------------------------------------------------------------

/// Merge performance records: union evaluation refs by (task_id, timestamp), recalculate stats.
fn merge_performance(target: &PerformanceRecord, source: &PerformanceRecord) -> PerformanceRecord {
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut merged_evals: Vec<EvaluationRef> = Vec::new();

    for eval in target.evaluations.iter().chain(source.evaluations.iter()) {
        let key = (eval.task_id.clone(), eval.timestamp.clone());
        if seen.insert(key) {
            merged_evals.push(eval.clone());
        }
    }

    let task_count = merged_evals.len() as u32;
    let avg_score = if merged_evals.is_empty() {
        None
    } else {
        let sum: f64 = merged_evals.iter().map(|e| e.score).sum();
        Some(sum / merged_evals.len() as f64)
    };

    PerformanceRecord {
        task_count,
        avg_score,
        evaluations: merged_evals,
    }
}

/// Merge lineage: prefer richer lineage (more parent_ids, higher generation).
fn merge_lineage(target: &Lineage, source: &Lineage) -> Lineage {
    if source.parent_ids.len() > target.parent_ids.len() {
        source.clone()
    } else if target.parent_ids.len() > source.parent_ids.len() {
        target.clone()
    } else if source.generation > target.generation {
        source.clone()
    } else {
        // Equal or target wins (keep local/target)
        target.clone()
    }
}

/// Merge a component: target name wins, performance is unioned, lineage prefers richer.
fn merge_component(target: &RoleComponent, source: &RoleComponent) -> RoleComponent {
    RoleComponent {
        id: target.id.clone(),
        name: target.name.clone(),
        description: target.description.clone(),
        quality: target.quality,
        domain_specificity: target.domain_specificity,
        domain: target.domain.clone(),
        scope: target.scope.clone(),
        origin_instance_id: target.origin_instance_id.clone(),
        parent_content_hash: target.parent_content_hash.clone(),
        category: target.category.clone(),
        content: target.content.clone(),
        performance: merge_performance(&target.performance, &source.performance),
        lineage: merge_lineage(&target.lineage, &source.lineage),
        access_control: target.access_control.clone(),
        domain_tags: target.domain_tags.clone(),
        metadata: target.metadata.clone(),
        former_agents: target.former_agents.clone(),
        former_deployments: target.former_deployments.clone(),
    }
}

/// Check if merged component has different metadata from original.
fn merged_component_differs(original: &RoleComponent, merged: &RoleComponent) -> bool {
    original.performance.task_count != merged.performance.task_count
        || original.performance.evaluations.len() != merged.performance.evaluations.len()
        || original.lineage.generation != merged.lineage.generation
        || original.lineage.parent_ids.len() != merged.lineage.parent_ids.len()
}

/// Merge an outcome: target name wins, performance is unioned, lineage prefers richer.
fn merge_outcome(target: &DesiredOutcome, source: &DesiredOutcome) -> DesiredOutcome {
    DesiredOutcome {
        id: target.id.clone(),
        name: target.name.clone(),
        description: target.description.clone(),
        quality: target.quality,
        domain_specificity: target.domain_specificity,
        domain: target.domain.clone(),
        scope: target.scope.clone(),
        origin_instance_id: target.origin_instance_id.clone(),
        parent_content_hash: target.parent_content_hash.clone(),
        success_criteria: target.success_criteria.clone(),
        performance: merge_performance(&target.performance, &source.performance),
        lineage: merge_lineage(&target.lineage, &source.lineage),
        access_control: target.access_control.clone(),
        requires_human_oversight: target.requires_human_oversight,
        domain_tags: target.domain_tags.clone(),
        metadata: target.metadata.clone(),
        former_agents: target.former_agents.clone(),
        former_deployments: target.former_deployments.clone(),
    }
}

/// Check if merged outcome has different metadata from original.
fn merged_outcome_differs(original: &DesiredOutcome, merged: &DesiredOutcome) -> bool {
    original.performance.task_count != merged.performance.task_count
        || original.performance.evaluations.len() != merged.performance.evaluations.len()
        || original.lineage.generation != merged.lineage.generation
        || original.lineage.parent_ids.len() != merged.lineage.parent_ids.len()
}

/// Merge a role: target name wins, performance is unioned, lineage prefers richer.
fn merge_role(target: &Role, source: &Role) -> Role {
    Role {
        id: target.id.clone(),
        name: target.name.clone(), // keep target name
        description: target.description.clone(),
        component_ids: target.component_ids.clone(),
        outcome_id: target.outcome_id.clone(),
        performance: merge_performance(&target.performance, &source.performance),
        lineage: merge_lineage(&target.lineage, &source.lineage),
        default_context_scope: target.default_context_scope.clone(),
        default_exec_mode: target.default_exec_mode.clone(),
    }
}

/// Merge a tradeoff: target name wins, performance is unioned, lineage prefers richer.
fn merge_tradeoff(target: &TradeoffConfig, source: &TradeoffConfig) -> TradeoffConfig {
    TradeoffConfig {
        id: target.id.clone(),
        name: target.name.clone(),
        description: target.description.clone(),
        quality: target.quality,
        domain_specificity: target.domain_specificity,
        domain: target.domain.clone(),
        scope: target.scope.clone(),
        origin_instance_id: target.origin_instance_id.clone(),
        parent_content_hash: target.parent_content_hash.clone(),
        acceptable_tradeoffs: target.acceptable_tradeoffs.clone(),
        unacceptable_tradeoffs: target.unacceptable_tradeoffs.clone(),
        performance: merge_performance(&target.performance, &source.performance),
        lineage: merge_lineage(&target.lineage, &source.lineage),
        access_control: target.access_control.clone(),
        domain_tags: target.domain_tags.clone(),
        metadata: target.metadata.clone(),
        former_agents: target.former_agents.clone(),
        former_deployments: target.former_deployments.clone(),
    }
}

/// Merge an agent: target name wins, performance is unioned, lineage prefers richer.
fn merge_agent(target: &Agent, source: &Agent) -> Agent {
    Agent {
        id: target.id.clone(),
        role_id: target.role_id.clone(),
        tradeoff_id: target.tradeoff_id.clone(),
        name: target.name.clone(),
        performance: merge_performance(&target.performance, &source.performance),
        lineage: merge_lineage(&target.lineage, &source.lineage),
        capabilities: target.capabilities.clone(),
        rate: target.rate,
        capacity: target.capacity,
        trust_level: target.trust_level.clone(),
        contact: target.contact.clone(),
        executor: target.executor.clone(),
        deployment_history: target.deployment_history.clone(),
        attractor_weight: target.attractor_weight,
        staleness_flags: target.staleness_flags.clone(),
        preferred_model: target
            .preferred_model
            .clone()
            .or_else(|| source.preferred_model.clone()),
        preferred_provider: target
            .preferred_provider
            .clone()
            .or_else(|| source.preferred_provider.clone()),
    }
}

/// Check if merged role has different metadata from original.
fn merged_role_differs(original: &Role, merged: &Role) -> bool {
    original.performance.task_count != merged.performance.task_count
        || original.performance.evaluations.len() != merged.performance.evaluations.len()
        || original.lineage.generation != merged.lineage.generation
        || original.lineage.parent_ids.len() != merged.lineage.parent_ids.len()
}

/// Check if merged tradeoff has different metadata from original.
fn merged_tradeoff_differs(original: &TradeoffConfig, merged: &TradeoffConfig) -> bool {
    original.performance.task_count != merged.performance.task_count
        || original.performance.evaluations.len() != merged.performance.evaluations.len()
        || original.lineage.generation != merged.lineage.generation
        || original.lineage.parent_ids.len() != merged.lineage.parent_ids.len()
}

/// Check if merged agent has different metadata from original.
fn merged_agent_differs(original: &Agent, merged: &Agent) -> bool {
    original.performance.task_count != merged.performance.task_count
        || original.performance.evaluations.len() != merged.performance.evaluations.len()
        || original.lineage.generation != merged.lineage.generation
        || original.lineage.parent_ids.len() != merged.lineage.parent_ids.len()
}

/// Parse a remote reference string like "peer:task-id" into (peer_name, task_id).
///
/// Returns `None` if the string doesn't contain a colon or looks like
/// a local reference. Local task IDs are slug-based (lowercase alphanumeric +
/// dashes, no colons), so the colon delimiter is unambiguous.
///
/// This lives here (unconditional) because dep-id validation and rendering in
/// unconditionally-built commands call it; resolving the *status* behind the
/// reference is the gated half ([`crate::federation::resolve_remote_task_status`]).
pub fn parse_remote_ref(dep: &str) -> Option<(&str, &str)> {
    // Split on the first colon only
    let (peer, task_id) = dep.split_once(':')?;
    // Both parts must be non-empty
    if peer.is_empty() || task_id.is_empty() {
        return None;
    }
    Some((peer, task_id))
}

// ---------------------------------------------------------------------------
// Peer liveness probe (a plain read of `<peer>/.wg/service/state.json`)
// ---------------------------------------------------------------------------

/// Peer service status information.
#[derive(Debug, Clone)]
pub struct PeerServiceStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub socket_path: Option<String>,
    pub started_at: Option<String>,
}

/// Check whether a peer's WG service is running.
///
/// Reads `<workgraph_dir>/service/state.json` and checks if the PID is alive.
pub fn check_peer_service(workgraph_dir: &Path) -> PeerServiceStatus {
    let state_path = workgraph_dir.join("service").join("state.json");
    if !state_path.exists() {
        return PeerServiceStatus {
            running: false,
            pid: None,
            socket_path: None,
            started_at: None,
        };
    }

    let content = match std::fs::read_to_string(&state_path) {
        Ok(c) => c,
        Err(_) => {
            return PeerServiceStatus {
                running: false,
                pid: None,
                socket_path: None,
                started_at: None,
            };
        }
    };

    #[derive(serde::Deserialize)]
    struct StateJson {
        pid: u32,
        socket_path: String,
        #[serde(default)]
        started_at: Option<String>,
    }

    let state: StateJson = match serde_json::from_str(&content) {
        Ok(s) => s,
        Err(_) => {
            return PeerServiceStatus {
                running: false,
                pid: None,
                socket_path: None,
                started_at: None,
            };
        }
    };

    let alive = is_process_alive(state.pid);

    PeerServiceStatus {
        running: alive,
        pid: Some(state.pid),
        socket_path: Some(state.socket_path),
        started_at: state.started_at,
    }
}

// ---------------------------------------------------------------------------
// Peer path resolution (WG-Fed — behind the non-default `federation` feature)
// ---------------------------------------------------------------------------

/// Resolve a peer reference (`federation.yaml` peer name, absolute path, or path
/// relative to CWD) to its project / `.wg` directories.
///
/// Peer resolution belongs to the WG-Fed cluster, so with the `federation` feature
/// off there is no peer plane at all: the call fails with an actionable error
/// naming the feature rather than silently resolving to something else.
#[cfg(feature = "federation")]
pub use crate::federation::{ResolvedPeer, resolve_peer};

/// See the feature-on re-export above.
#[cfg(not(feature = "federation"))]
#[derive(Debug, Clone)]
pub struct ResolvedPeer {
    /// The project root (parent of .wg/).
    pub project_path: PathBuf,
    /// The .wg directory.
    pub workgraph_dir: PathBuf,
}

/// See the feature-on re-export above.
#[cfg(not(feature = "federation"))]
pub fn resolve_peer(
    _reference: &str,
    _workgraph_dir: &Path,
) -> Result<ResolvedPeer, anyhow::Error> {
    anyhow::bail!(
        "peer resolution is part of the federation cluster, which this binary was built \
         without; rebuild with `cargo build --features federation`"
    )
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use crate::agency::{self, EvaluationRef, Lineage, PerformanceRecord};
    use tempfile::TempDir;

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
            access_control: agency::AccessControl::default(),
            domain_tags: vec![],
            metadata: HashMap::new(),
            former_agents: Vec::new(),
            former_deployments: Vec::new(),
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
            trust_level: crate::graph::TrustLevel::Provisional,
            contact: None,
            executor: "claude".to_string(),
            preferred_model: None,
            preferred_provider: None,
            deployment_history: Vec::new(),
            attractor_weight: 0.5,
            staleness_flags: Vec::new(),
        }
    }

    #[test]
    fn transfer_new_roles() {
        let tmp = TempDir::new().unwrap();
        let source = setup_store(&tmp, "source");
        let target = setup_store(&tmp, "target");

        source.save_role(&make_role("r1", "role1")).unwrap();
        source.save_role(&make_role("r2", "role2")).unwrap();

        let summary = transfer(&source, &target, &TransferOptions::default()).unwrap();
        assert_eq!(summary.roles_added, 2);
        assert_eq!(summary.roles_skipped, 0);
        assert!(target.exists_role("r1"));
        assert!(target.exists_role("r2"));
    }

    #[test]
    fn transfer_skips_identical() {
        let tmp = TempDir::new().unwrap();
        let source = setup_store(&tmp, "source");
        let target = setup_store(&tmp, "target");

        let role = make_role("r1", "role1");
        source.save_role(&role).unwrap();
        target.save_role(&role).unwrap();

        let summary = transfer(&source, &target, &TransferOptions::default()).unwrap();
        assert_eq!(summary.roles_added, 0);
        assert_eq!(summary.roles_skipped, 1);
    }

    #[test]
    fn transfer_merges_performance() {
        let tmp = TempDir::new().unwrap();
        let source = setup_store(&tmp, "source");
        let target = setup_store(&tmp, "target");

        let mut role_source = make_role("r1", "role1");
        role_source.performance.evaluations.push(EvaluationRef {
            score: 0.9,
            task_id: "task-a".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            context_id: String::new(),
        });
        role_source.performance.task_count = 1;
        role_source.performance.avg_score = Some(0.9);

        let mut role_target = make_role("r1", "role1-local");
        role_target.performance.evaluations.push(EvaluationRef {
            score: 0.8,
            task_id: "task-b".to_string(),
            timestamp: "2026-01-02T00:00:00Z".to_string(),
            context_id: String::new(),
        });
        role_target.performance.task_count = 1;
        role_target.performance.avg_score = Some(0.8);

        source.save_role(&role_source).unwrap();
        target.save_role(&role_target).unwrap();

        let summary = transfer(&source, &target, &TransferOptions::default()).unwrap();
        assert_eq!(summary.roles_updated, 1);

        // Verify merged performance
        let roles = target.load_roles().unwrap();
        let merged = roles.iter().find(|r| r.id == "r1").unwrap();
        assert_eq!(merged.performance.task_count, 2);
        assert_eq!(merged.name, "role1-local"); // target name preserved
    }

    #[test]
    fn transfer_agent_pulls_dependencies() {
        let tmp = TempDir::new().unwrap();
        let source = setup_store(&tmp, "source");
        let target = setup_store(&tmp, "target");

        let role = make_role("r1", "role1");
        let motivation = make_motivation("m1", "motivation1");
        let agent = make_agent("a1", "agent1", "r1", "m1");

        source.save_role(&role).unwrap();
        source.save_tradeoff(&motivation).unwrap();
        source.save_agent(&agent).unwrap();

        // Transfer only agents — should auto-include role and motivation
        let opts = TransferOptions {
            entity_filter: EntityFilter::Agents,
            ..Default::default()
        };
        let summary = transfer(&source, &target, &opts).unwrap();

        assert_eq!(summary.agents_added, 1);
        assert_eq!(summary.roles_added, 1);
        assert_eq!(summary.tradeoffs_added, 1);
        assert!(target.exists_role("r1"));
        assert!(target.exists_tradeoff("m1"));
    }

    #[test]
    fn transfer_agent_skips_existing_deps() {
        let tmp = TempDir::new().unwrap();
        let source = setup_store(&tmp, "source");
        let target = setup_store(&tmp, "target");

        let role = make_role("r1", "role1");
        let motivation = make_motivation("m1", "motivation1");
        let agent = make_agent("a1", "agent1", "r1", "m1");

        source.save_role(&role).unwrap();
        source.save_tradeoff(&motivation).unwrap();
        source.save_agent(&agent).unwrap();

        // Pre-populate target with deps
        target.save_role(&role).unwrap();
        target.save_tradeoff(&motivation).unwrap();

        let opts = TransferOptions {
            entity_filter: EntityFilter::Agents,
            ..Default::default()
        };
        let summary = transfer(&source, &target, &opts).unwrap();

        assert_eq!(summary.agents_added, 1);
        // Deps already exist in target — referential integrity check skips them
        // (they were never added to the transfer set, so no count increment)
        assert_eq!(summary.roles_added, 0);
        assert_eq!(summary.tradeoffs_added, 0);
    }

    #[test]
    fn transfer_dry_run_does_not_write() {
        let tmp = TempDir::new().unwrap();
        let source = setup_store(&tmp, "source");
        let target = setup_store(&tmp, "target");

        source.save_role(&make_role("r1", "role1")).unwrap();

        let opts = TransferOptions {
            dry_run: true,
            ..Default::default()
        };
        let summary = transfer(&source, &target, &opts).unwrap();
        assert_eq!(summary.roles_added, 1);
        assert!(!target.exists_role("r1")); // not actually written
    }

    #[test]
    fn transfer_no_performance_strips_perf() {
        let tmp = TempDir::new().unwrap();
        let source = setup_store(&tmp, "source");
        let target = setup_store(&tmp, "target");

        let mut role = make_role("r1", "role1");
        role.performance.task_count = 5;
        role.performance.avg_score = Some(0.85);
        source.save_role(&role).unwrap();

        let opts = TransferOptions {
            no_performance: true,
            ..Default::default()
        };
        transfer(&source, &target, &opts).unwrap();

        let roles = target.load_roles().unwrap();
        let saved = roles.iter().find(|r| r.id == "r1").unwrap();
        assert_eq!(saved.performance.task_count, 0);
        assert!(saved.performance.avg_score.is_none());
    }

    #[test]
    fn transfer_entity_filter_by_id() {
        let tmp = TempDir::new().unwrap();
        let source = setup_store(&tmp, "source");
        let target = setup_store(&tmp, "target");

        source.save_role(&make_role("r1", "role1")).unwrap();
        source.save_role(&make_role("r2", "role2")).unwrap();

        let opts = TransferOptions {
            entity_ids: vec!["r1".to_string()],
            ..Default::default()
        };
        let summary = transfer(&source, &target, &opts).unwrap();
        assert_eq!(summary.roles_added, 1);
        assert!(target.exists_role("r1"));
        assert!(!target.exists_role("r2"));
    }

    #[test]
    fn merge_performance_deduplicates() {
        let a = PerformanceRecord {
            task_count: 1,
            avg_score: Some(0.9),
            evaluations: vec![EvaluationRef {
                score: 0.9,
                task_id: "t1".to_string(),
                timestamp: "2026-01-01".to_string(),
                context_id: String::new(),
            }],
        };
        let b = PerformanceRecord {
            task_count: 2,
            avg_score: Some(0.85),
            evaluations: vec![
                EvaluationRef {
                    score: 0.9,
                    task_id: "t1".to_string(),
                    timestamp: "2026-01-01".to_string(),
                    context_id: String::new(),
                },
                EvaluationRef {
                    score: 0.8,
                    task_id: "t2".to_string(),
                    timestamp: "2026-01-02".to_string(),
                    context_id: String::new(),
                },
            ],
        };
        let merged = merge_performance(&a, &b);
        assert_eq!(merged.task_count, 2); // deduped
        assert_eq!(merged.evaluations.len(), 2);
    }

    #[test]
    fn merge_lineage_prefers_richer() {
        let sparse = Lineage {
            parent_ids: Vec::new(),
            generation: 0,
            created_by: "human".to_string(),
            created_at: chrono::Utc::now(),
            reframing_potential: None,
        };
        let rich = Lineage {
            parent_ids: vec!["p1".to_string()],
            generation: 1,
            created_by: "evolver-1".to_string(),
            created_at: chrono::Utc::now(),
            reframing_potential: None,
        };
        let merged = merge_lineage(&sparse, &rich);
        assert_eq!(merged.parent_ids.len(), 1);
        assert_eq!(merged.generation, 1);
    }

    #[test]
    fn resolve_store_finds_project_store() {
        let tmp = TempDir::new().unwrap();
        let wg = tmp.path().join(".wg").join("agency");
        agency::init(&wg).unwrap();

        let store = resolve_store(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(store.store_path(), wg);
    }

    #[test]
    fn resolve_store_finds_bare_store() {
        let tmp = TempDir::new().unwrap();
        let bare = tmp.path().join("agency");
        agency::init(&bare).unwrap();

        let store = resolve_store(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(store.store_path(), bare);
    }

    #[test]
    fn resolve_store_finds_direct_agency_dir() {
        let tmp = TempDir::new().unwrap();
        let direct = tmp.path().join("myagency");
        agency::init(&direct).unwrap();

        let store = resolve_store(direct.to_str().unwrap()).unwrap();
        assert_eq!(store.store_path(), direct);
    }

    #[test]
    fn transfer_skips_corrupt_target_yaml_gracefully() {
        let tmp = TempDir::new().unwrap();
        let source = setup_store(&tmp, "source");
        let target = setup_store(&tmp, "target");

        // Put a valid role in source
        source.save_role(&make_role("r1", "role1")).unwrap();
        // Also put it in target so the merge path is exercised
        target.save_role(&make_role("r1", "role1")).unwrap();

        // Corrupt a role YAML in the target store
        let corrupt_path = target.roles_dir().join("corrupt.yaml");
        std::fs::write(&corrupt_path, "{{{{not valid yaml!!!!").unwrap();

        // Transfer should succeed — corrupt files are skipped with warnings
        let result = transfer(&source, &target, &TransferOptions::default());
        assert!(
            result.is_ok(),
            "transfer should skip corrupt target YAML gracefully, got: {:?}",
            result.unwrap_err()
        );

        // The valid role should still be present after transfer
        let roles = target.load_roles().unwrap();
        assert!(
            roles.iter().any(|r| r.id == "r1"),
            "valid role r1 should survive transfer alongside corrupt file"
        );
    }

    #[test]
    fn transfer_errors_on_missing_agent_role_dependency() {
        let tmp = TempDir::new().unwrap();
        let source = setup_store(&tmp, "source");
        let target = setup_store(&tmp, "target");

        // Create an agent that references a role not in source
        let motivation = make_motivation("m1", "motivation1");
        source.save_tradeoff(&motivation).unwrap();

        let agent = make_agent("a1", "agent1", "nonexistent-role", "m1");
        source.save_agent(&agent).unwrap();

        let opts = TransferOptions {
            entity_filter: EntityFilter::Agents,
            ..Default::default()
        };
        let result = transfer(&source, &target, &opts);
        assert!(
            result.is_err(),
            "transfer should fail on missing role dependency"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("nonexistent-role") && err_msg.contains("referential integrity"),
            "error should mention the missing role and referential integrity: {}",
            err_msg
        );
    }

    #[test]
    fn transfer_errors_on_missing_agent_motivation_dependency() {
        let tmp = TempDir::new().unwrap();
        let source = setup_store(&tmp, "source");
        let target = setup_store(&tmp, "target");

        // Create an agent that references a motivation not in source
        let role = make_role("r1", "role1");
        source.save_role(&role).unwrap();

        let agent = make_agent("a1", "agent1", "r1", "nonexistent-motivation");
        source.save_agent(&agent).unwrap();

        let opts = TransferOptions {
            entity_filter: EntityFilter::Agents,
            ..Default::default()
        };
        let result = transfer(&source, &target, &opts);
        assert!(
            result.is_err(),
            "transfer should fail on missing motivation dependency"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("nonexistent-motivation") && err_msg.contains("referential integrity"),
            "error should mention the missing motivation and referential integrity: {}",
            err_msg
        );
    }

    // Primitive type federation tests
    // -----------------------------------------------------------------------

    fn make_component(id: &str, name: &str) -> RoleComponent {
        RoleComponent {
            id: id.to_string(),
            name: name.to_string(),
            description: format!("test component {}", name),
            quality: 100,
            domain_specificity: 0,
            domain: vec![],
            scope: None,
            origin_instance_id: None,
            parent_content_hash: None,
            category: agency::ComponentCategory::Translated,
            content: agency::ContentRef::Inline("test content".into()),
            performance: PerformanceRecord::default(),
            lineage: Lineage::default(),
            access_control: agency::AccessControl::default(),
            domain_tags: vec![],
            metadata: HashMap::new(),
            former_agents: Vec::new(),
            former_deployments: Vec::new(),
        }
    }

    fn make_outcome(id: &str, name: &str) -> DesiredOutcome {
        DesiredOutcome {
            id: id.to_string(),
            name: name.to_string(),
            description: format!("test outcome {}", name),
            quality: 100,
            domain_specificity: 0,
            domain: vec![],
            scope: None,
            origin_instance_id: None,
            parent_content_hash: None,
            success_criteria: vec!["criterion 1".into()],
            performance: PerformanceRecord::default(),
            lineage: Lineage::default(),
            access_control: agency::AccessControl::default(),
            requires_human_oversight: true,
            domain_tags: vec![],
            metadata: HashMap::new(),
            former_agents: Vec::new(),
            former_deployments: Vec::new(),
        }
    }

    #[test]
    fn transfer_components_between_stores() {
        let tmp = TempDir::new().unwrap();
        let source = setup_store(&tmp, "source");
        let target = setup_store(&tmp, "target");

        source
            .save_component(&make_component("c1", "coding"))
            .unwrap();
        source
            .save_component(&make_component("c2", "testing"))
            .unwrap();

        let summary = transfer(&source, &target, &TransferOptions::default()).unwrap();
        assert_eq!(summary.components_added, 2);
        assert!(target.exists_component("c1"));
        assert!(target.exists_component("c2"));
    }

    #[test]
    fn transfer_outcomes_between_stores() {
        let tmp = TempDir::new().unwrap();
        let source = setup_store(&tmp, "source");
        let target = setup_store(&tmp, "target");

        source
            .save_outcome(&make_outcome("o1", "Working code"))
            .unwrap();

        let summary = transfer(&source, &target, &TransferOptions::default()).unwrap();
        assert_eq!(summary.outcomes_added, 1);
        assert!(target.exists_outcome("o1"));
    }

    #[test]
    fn transfer_tradeoffs_between_stores() {
        let tmp = TempDir::new().unwrap();
        let source = setup_store(&tmp, "source");
        let target = setup_store(&tmp, "target");

        source
            .save_tradeoff(&make_motivation("t1", "Careful"))
            .unwrap();
        source
            .save_tradeoff(&make_motivation("t2", "Fast"))
            .unwrap();

        let opts = TransferOptions {
            entity_filter: EntityFilter::Tradeoffs,
            ..Default::default()
        };
        let summary = transfer(&source, &target, &opts).unwrap();
        assert_eq!(summary.tradeoffs_added, 2);
        assert!(target.exists_tradeoff("t1"));
        assert!(target.exists_tradeoff("t2"));
    }

    #[test]
    fn transfer_component_with_private_access_denied() {
        let tmp = TempDir::new().unwrap();
        let source = setup_store(&tmp, "source");
        let target = setup_store(&tmp, "target");

        let mut comp = make_component("c1", "private-comp");
        comp.access_control.policy = AccessPolicy::Private;
        source.save_component(&comp).unwrap();

        let summary = transfer(&source, &target, &TransferOptions::default()).unwrap();
        assert_eq!(summary.components_added, 0);
        assert_eq!(summary.components_access_denied, 1);
        assert!(!target.exists_component("c1"));
    }

    #[test]
    fn transfer_component_skips_identical() {
        let tmp = TempDir::new().unwrap();
        let source = setup_store(&tmp, "source");
        let target = setup_store(&tmp, "target");

        let comp = make_component("c1", "coding");
        source.save_component(&comp).unwrap();
        target.save_component(&comp).unwrap();

        let summary = transfer(&source, &target, &TransferOptions::default()).unwrap();
        assert_eq!(summary.components_added, 0);
        assert_eq!(summary.components_skipped, 1);
    }

    #[test]
    fn transfer_component_merges_performance() {
        let tmp = TempDir::new().unwrap();
        let source = setup_store(&tmp, "source");
        let target = setup_store(&tmp, "target");

        let mut comp_src = make_component("c1", "coding");
        comp_src.performance.task_count = 3;
        comp_src.performance.avg_score = Some(0.8);
        comp_src.performance.evaluations.push(EvaluationRef {
            score: 0.8,
            task_id: "t1".into(),
            timestamp: "2025-01-01T00:00:00Z".into(),
            context_id: "ctx1".into(),
        });
        source.save_component(&comp_src).unwrap();

        let mut comp_tgt = make_component("c1", "coding");
        comp_tgt.performance.task_count = 1;
        comp_tgt.performance.avg_score = Some(0.6);
        comp_tgt.performance.evaluations.push(EvaluationRef {
            score: 0.6,
            task_id: "t0".into(),
            timestamp: "2024-12-01T00:00:00Z".into(),
            context_id: "ctx0".into(),
        });
        target.save_component(&comp_tgt).unwrap();

        let summary = transfer(&source, &target, &TransferOptions::default()).unwrap();
        assert_eq!(summary.components_updated, 1);

        // Merged component should have evaluations from both
        let merged = target.load_components().unwrap();
        let merged_comp = merged.iter().find(|c| c.id == "c1").unwrap();
        assert_eq!(merged_comp.performance.evaluations.len(), 2);
    }
}
