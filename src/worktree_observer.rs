//! Fail-closed candidate-content activity observer for isolated worktrees.
//!
//! This module is deliberately an evidence producer.  It fingerprints the exact
//! leased worktree bound in [`ObserverSource`], persists a hash-linked journal,
//! and exposes a read-only projection.  It has no dependency on the lifecycle
//! mutator and cannot complete, fail, reopen, lease, checkpoint, or merge a task.
//! Native filesystem notifications are hints only; every accepted advance comes
//! from [`WorktreeObserver::reconcile_at`].

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

pub const CANDIDATE_PATH_POLICY_VERSION: u32 = 1;
pub const OBSERVER_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_RECONCILE_INTERVAL_SECS: u64 = 15;
pub const DEFAULT_OBSERVED_ACTIVITY_GRACE_SECS: u64 = 120;
pub const DEFAULT_MAX_OBSERVED_ONLY_EXTENSION_SECS: u64 = 600;
pub const DEFAULT_MEANINGFUL_SILENCE_SECS: u64 = 300;
const MAX_CHANGED_PATHS: usize = 64;
const MAX_PATH_RENDER_BYTES: usize = 512;
/// Native notifications are wake-up hints, not evidence. Keeping a single
/// coalesced hint prevents a build tree from retaining one `notify::Event` (and
/// all of its paths) per kernel event while a reconciliation is in progress.
pub const WATCHER_WAKE_QUEUE_CAPACITY: usize = 1;
const MAX_WATCHED_SOURCE_DIRECTORIES: usize = 16_384;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObserverConfig {
    /// Ambient generated-output rules snapshotted into CandidatePathPolicyV1
    /// at attempt reservation. Task deliverables override these rules.
    #[serde(default)]
    pub generated_paths: Vec<String>,
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u64,
    #[serde(default = "default_reconcile_interval")]
    pub reconcile_interval_secs: u64,
    #[serde(default = "default_observed_grace")]
    pub observed_activity_grace_secs: u64,
    #[serde(default = "default_observed_cap")]
    pub max_observed_only_extension_secs: u64,
}

const fn default_debounce_ms() -> u64 {
    100
}
const fn default_reconcile_interval() -> u64 {
    DEFAULT_RECONCILE_INTERVAL_SECS
}
const fn default_observed_grace() -> u64 {
    DEFAULT_OBSERVED_ACTIVITY_GRACE_SECS
}
const fn default_observed_cap() -> u64 {
    DEFAULT_MAX_OBSERVED_ONLY_EXTENSION_SECS
}

impl Default for ObserverConfig {
    fn default() -> Self {
        Self {
            generated_paths: Vec::new(),
            debounce_ms: default_debounce_ms(),
            reconcile_interval_secs: default_reconcile_interval(),
            observed_activity_grace_secs: default_observed_grace(),
            max_observed_only_extension_secs: default_observed_cap(),
        }
    }
}

impl ObserverConfig {
    pub fn validate(&self) -> Result<()> {
        if self.generated_paths.len() > 256 {
            bail!("worktree observer generated_paths may contain at most 256 entries");
        }
        for path in &self.generated_paths {
            validate_policy_pattern(path)?;
        }
        if !(10..=5_000).contains(&self.debounce_ms) {
            bail!("worktree observer debounce_ms must be in 10..=5000");
        }
        if !(1..=300).contains(&self.reconcile_interval_secs) {
            bail!("worktree observer reconcile_interval_secs must be in 1..=300");
        }
        if self.observed_activity_grace_secs > 600 {
            bail!("worktree observer observed_activity_grace_secs must be <= 600");
        }
        if self.max_observed_only_extension_secs > 3_600 {
            bail!("worktree observer max_observed_only_extension_secs must be <= 3600");
        }
        if self.observed_activity_grace_secs > self.max_observed_only_extension_secs {
            bail!("worktree observer grace may not exceed its hard extension cap");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObserverIdentity {
    pub task_id: String,
    pub generation: u64,
    pub attempt_id: String,
    pub attempt_fence: u64,
    pub worktree_id: String,
    pub worktree_lease_epoch: u64,
    pub process_epoch: u32,
    pub observer_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootIdentity {
    pub canonical_path: String,
    pub device: u64,
    pub file_identity: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObserverSource {
    #[serde(flatten)]
    pub identity: ObserverIdentity,
    pub canonical_worktree_root: String,
    pub root_identity: RootIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidatePathPolicy {
    pub version: u32,
    pub explicit_deliverables: Vec<String>,
    pub generated_paths: Vec<String>,
    pub digest: String,
}

impl CandidatePathPolicy {
    pub fn new(
        mut explicit_deliverables: Vec<String>,
        mut generated_paths: Vec<String>,
    ) -> Result<Self> {
        explicit_deliverables.sort();
        explicit_deliverables.dedup();
        generated_paths.sort();
        generated_paths.dedup();
        for path in explicit_deliverables.iter().chain(generated_paths.iter()) {
            validate_policy_pattern(path)?;
        }
        let mut policy = Self {
            version: CANDIDATE_PATH_POLICY_VERSION,
            explicit_deliverables,
            generated_paths,
            digest: String::new(),
        };
        policy.digest = policy.compute_digest()?;
        Ok(policy)
    }

    fn compute_digest(&self) -> Result<String> {
        #[derive(Serialize)]
        struct Body<'a> {
            version: u32,
            explicit_deliverables: &'a [String],
            generated_paths: &'a [String],
        }
        let bytes = serde_json::to_vec(&Body {
            version: self.version,
            explicit_deliverables: &self.explicit_deliverables,
            generated_paths: &self.generated_paths,
        })?;
        Ok(format!("b3:{}", blake3::hash(&bytes).to_hex()))
    }

    pub fn verify(&self) -> Result<()> {
        if self.version != CANDIDATE_PATH_POLICY_VERSION {
            bail!("unsupported candidate path policy version {}", self.version);
        }
        if self.compute_digest()? != self.digest {
            bail!("candidate path policy digest mismatch");
        }
        Ok(())
    }

    fn matches(patterns: &[String], path: &str) -> bool {
        patterns.iter().any(|pattern| {
            pattern == path
                || pattern.strip_suffix("/**").is_some_and(|prefix| {
                    path == prefix
                        || path
                            .strip_prefix(prefix)
                            .is_some_and(|rest| rest.starts_with('/'))
                })
                || path
                    .strip_prefix(pattern.trim_end_matches('/'))
                    .is_some_and(|rest| rest.starts_with('/'))
                || glob::Pattern::new(pattern).is_ok_and(|p| p.matches(path))
        })
    }

    fn explicit_may_be_beneath(&self, directory: &str) -> bool {
        let prefix = format!("{}/", directory.trim_end_matches('/'));
        self.explicit_deliverables.iter().any(|pattern| {
            pattern == directory
                || pattern.starts_with(&prefix)
                || pattern
                    .find(['*', '?', '['])
                    .map(|index| pattern[..index].starts_with(&prefix))
                    .unwrap_or(false)
        })
    }

    fn generated_directory(&self, directory: &str) -> bool {
        Self::matches(&self.generated_paths, directory)
            || self.generated_paths.iter().any(|pattern| {
                pattern
                    .strip_suffix("/**")
                    .is_some_and(|prefix| prefix == directory)
            })
    }

    fn classify(&self, path: &str, tracked: bool, ignored: bool) -> PathClass {
        if is_internal_control(path) {
            return PathClass::Excluded("internal-control");
        }
        if Self::matches(&self.explicit_deliverables, path) {
            return PathClass::Candidate("explicit-deliverable");
        }
        if Self::matches(&self.generated_paths, path) || self.generated_directory(path) {
            return PathClass::Excluded("configured-generated");
        }
        if tracked {
            return PathClass::Candidate("tracked");
        }
        if let Some(reason) = volatile_reason(path) {
            return PathClass::Excluded(reason);
        }
        if ignored {
            return PathClass::Excluded("git-ignored");
        }
        PathClass::Candidate("plausible-untracked")
    }

    /// Cheap event-ingress classification. This intentionally uses only the
    /// immutable attempt policy and structural rules: no callback may read,
    /// hash, or retain bytes from a volatile tree. Unknown paths remain wake-up
    /// hints and are classified exactly by the subsequent stable scan.
    fn ingress_exclusion(&self, path: &str) -> Option<&'static str> {
        if Self::matches(&self.explicit_deliverables, path) || self.explicit_may_be_beneath(path) {
            return None;
        }
        if is_internal_control(path) {
            return Some("internal-control");
        }
        if let Some(reason) = volatile_reason(path) {
            return Some(reason);
        }
        if Self::matches(&self.generated_paths, path) || self.generated_directory(path) {
            return Some("configured-generated");
        }
        None
    }
}

fn validate_policy_pattern(path: &str) -> Result<()> {
    if path.is_empty() || Path::new(path).is_absolute() || path.contains('\0') {
        bail!("candidate path policy entries must be non-empty relative paths");
    }
    if Path::new(path).components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        bail!("candidate path policy entries may not escape the worktree: {path}");
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathClass {
    Candidate(&'static str),
    Excluded(&'static str),
}

fn is_internal_control(path: &str) -> bool {
    path == ".git"
        || path.starts_with(".git/")
        || path == ".wg"
        || path.starts_with(".wg/attempts/")
        || path.starts_with(".wg/agents/")
        || path.starts_with(".wg/service/")
        || path.starts_with(".wg/sessions/")
        || path == ".wg-cleanup-pending"
        || path.ends_with("/.wg-cleanup-pending")
}

fn volatile_reason(path: &str) -> Option<&'static str> {
    let components: Vec<&str> = path.split('/').collect();
    if components.contains(&"target") {
        return Some("volatile-target");
    }
    if components
        .iter()
        .any(|c| matches!(*c, "node_modules" | ".venv" | "__pypackages__" | "deps"))
    {
        return Some("dependency-tree");
    }
    if components.iter().any(|c| {
        matches!(
            *c,
            ".cache" | "__pycache__" | ".pytest_cache" | ".mypy_cache" | ".gradle"
        )
    }) {
        return Some("cache-tree");
    }
    let leaf = components.last().copied().unwrap_or("");
    if leaf.ends_with('~')
        || leaf.ends_with(".swp")
        || leaf.ends_with(".tmp")
        || leaf.starts_with(".#")
    {
        return Some("temporary-file");
    }
    if path.starts_with(".wg/agents/")
        || matches!(
            leaf,
            "stream.jsonl" | "raw_stream.jsonl" | "heartbeat" | "output.log"
        )
    {
        return Some("wg-runtime");
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CandidateKind {
    Regular,
    Symlink,
    Gitlink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub path: String,
    pub kind: CandidateKind,
    pub git_mode: u32,
    pub content_digest: String,
    pub size: u64,
    pub class: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ExcludedSignature {
    reason: String,
    signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManifestSnapshot {
    digest: String,
    entries: BTreeMap<String, ManifestEntry>,
    excluded: BTreeMap<String, ExcludedSignature>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BaselineFile {
    schema_version: u32,
    source: ObserverSource,
    policy_digest: String,
    established_at: Option<i64>,
    baseline_time_unknown: bool,
    manifest: ManifestSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedPath {
    pub path: String,
    pub class: String,
    pub operation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_digest: Option<String>,
    pub byte_delta: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivitySummary {
    pub observed_at: i64,
    pub content_seq: u64,
    pub source: String,
    pub changed_paths: Vec<ChangedPath>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LateMutation {
    pub observed_at: i64,
    pub prior_manifest_digest: String,
    pub new_manifest_digest: String,
    pub reason: String,
    pub changed_paths: Vec<ChangedPath>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ObserverHealth {
    EventAndReconcile,
    PollOnly,
    RescanRequired,
    ClassificationHold,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObserverProjection {
    pub schema_version: u32,
    pub source: ObserverSource,
    pub policy_digest: String,
    /// Attempt-snapshotted timing policy. Ambient configuration changes do not
    /// reclassify the attempt or replenish its proof window.
    pub timing_policy: ObserverConfig,
    pub manifest_digest: String,
    pub content_seq: u64,
    pub baseline_time_unknown: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<ActivitySummary>,
    pub health: ObserverHealth,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
    pub last_reconciliation_at: i64,
    #[serde(default)]
    pub ignored_churn: BTreeMap<String, u64>,
    #[serde(default)]
    pub unstable_scans: u64,
    #[serde(default)]
    pub watcher_overflows: u64,
    #[serde(default)]
    pub stale_callbacks: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classification_hold: Option<String>,
    #[serde(default)]
    pub preservation_mode: bool,
    #[serde(default)]
    pub purported_reap: bool,
    #[serde(default)]
    pub quarantine_required: bool,
    #[serde(default)]
    pub late_mutations: Vec<LateMutation>,
    pub next_safe_action: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ObserverState {
    projection: ObserverProjection,
    current_manifest: ManifestSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    journal_head: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReconcileSource {
    Event,
    Periodic,
    Startup,
    BeforeWatchdogDecision,
    Overflow,
    Manual,
}

impl ReconcileSource {
    fn label(self) -> &'static str {
        match self {
            Self::Event => "event",
            Self::Periodic => "periodic",
            Self::Startup => "startup",
            Self::BeforeWatchdogDecision => "before-watchdog-decision",
            Self::Overflow => "overflow",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileOutcome {
    Unchanged,
    Advanced(u64),
    LateMutation,
    Held(String),
    StaleCallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ActivityRecord {
    schema_version: u32,
    source_tuple: ObserverSource,
    content_seq: u64,
    prior_manifest_digest: String,
    new_manifest_digest: String,
    changed_paths: Vec<ChangedPath>,
    operation_kind: String,
    wall_timestamp: i64,
    reconciliation_source: String,
    prior_record_hash: Option<String>,
    record_hash: String,
}

#[derive(Serialize)]
struct ActivityRecordBody<'a> {
    schema_version: u32,
    source_tuple: &'a ObserverSource,
    content_seq: u64,
    prior_manifest_digest: &'a str,
    new_manifest_digest: &'a str,
    changed_paths: &'a [ChangedPath],
    operation_kind: &'a str,
    wall_timestamp: i64,
    reconciliation_source: &'a str,
    prior_record_hash: &'a Option<String>,
}

fn activity_record_hash(record: &ActivityRecord) -> Result<String> {
    let body = ActivityRecordBody {
        schema_version: record.schema_version,
        source_tuple: &record.source_tuple,
        content_seq: record.content_seq,
        prior_manifest_digest: &record.prior_manifest_digest,
        new_manifest_digest: &record.new_manifest_digest,
        changed_paths: &record.changed_paths,
        operation_kind: &record.operation_kind,
        wall_timestamp: record.wall_timestamp,
        reconciliation_source: &record.reconciliation_source,
        prior_record_hash: &record.prior_record_hash,
    };
    Ok(format!(
        "b3:{}",
        blake3::hash(&serde_json::to_vec(&body)?).to_hex()
    ))
}

fn read_activity_records(path: &Path) -> Result<Vec<ActivityRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    let mut prior = None;
    for line in fs::read_to_string(path)?
        .lines()
        .filter(|line| !line.trim().is_empty())
    {
        let record: ActivityRecord = serde_json::from_str(line)
            .context("observer activity journal contains invalid JSON")?;
        if record.prior_record_hash != prior || activity_record_hash(&record)? != record.record_hash
        {
            bail!("observer activity journal hash-chain mismatch");
        }
        prior = Some(record.record_hash.clone());
        records.push(record);
    }
    Ok(records)
}

pub struct WorktreeObserver {
    storage: PathBuf,
    policy: CandidatePathPolicy,
    config: ObserverConfig,
    state: ObserverState,
}

impl WorktreeObserver {
    /// Establish a baseline or idempotently reopen the exact preparation left
    /// by an earlier transaction pass. A state directory is never adopted by
    /// path alone: the complete source tuple, root identity, policy snapshot,
    /// and runtime policy must all match before reconciliation.
    pub fn prepare_at(
        root: &Path,
        storage: &Path,
        identity: ObserverIdentity,
        policy: CandidatePathPolicy,
        config: ObserverConfig,
        now: i64,
    ) -> Result<Self> {
        let state_exists = storage.join("state.json").exists();
        let baseline_exists = storage.join("baseline.json").exists();
        if !state_exists && !baseline_exists {
            return Self::attach_at(root, storage, identity, policy, config, now);
        }
        if state_exists != baseline_exists {
            bail!("observer preparation is partial; preserve state and recover explicitly");
        }
        config.validate()?;
        policy.verify()?;
        let expected_source = source_for_root(root, identity.clone())?;
        let mut observer = Self::open_at(storage, identity, now)?;
        if observer.state.projection.source != expected_source {
            bail!("observer root identity mismatch during restart reconciliation");
        }
        if observer.policy != policy {
            bail!("observer policy snapshot mismatch during restart reconciliation");
        }
        if observer.config != config || observer.state.projection.timing_policy != config {
            bail!("observer runtime policy mismatch during restart reconciliation");
        }
        let callback = observer.state.projection.source.identity.clone();
        let _ = observer.reconcile_callback_at(&callback, ReconcileSource::Startup, now)?;
        Ok(observer)
    }

    pub fn attach_at(
        root: &Path,
        storage: &Path,
        identity: ObserverIdentity,
        policy: CandidatePathPolicy,
        config: ObserverConfig,
        now: i64,
    ) -> Result<Self> {
        config.validate()?;
        policy.verify()?;
        if storage.join("state.json").exists() || storage.join("baseline.json").exists() {
            bail!("observer state already exists; use open_at for restart reconciliation");
        }
        fs::create_dir_all(storage)?;
        let source = source_for_root(root, identity)?;
        let manifest = scan_manifest(root, &policy)?;
        let baseline = BaselineFile {
            schema_version: OBSERVER_SCHEMA_VERSION,
            source: source.clone(),
            policy_digest: policy.digest.clone(),
            established_at: Some(now),
            baseline_time_unknown: false,
            manifest: manifest.clone(),
        };
        let projection = ObserverProjection {
            schema_version: OBSERVER_SCHEMA_VERSION,
            source,
            policy_digest: policy.digest.clone(),
            timing_policy: config.clone(),
            manifest_digest: manifest.digest.clone(),
            content_seq: 0,
            baseline_time_unknown: false,
            last_activity: None,
            health: ObserverHealth::EventAndReconcile,
            degraded_reason: None,
            last_reconciliation_at: now,
            ignored_churn: BTreeMap::new(),
            unstable_scans: 0,
            watcher_overflows: 0,
            stale_callbacks: 0,
            classification_hold: None,
            preservation_mode: false,
            purported_reap: false,
            quarantine_required: false,
            late_mutations: Vec::new(),
            next_safe_action: "observer active; lifecycle/watchdog remain authoritative".into(),
        };
        let observer = Self {
            storage: storage.to_path_buf(),
            policy,
            config,
            state: ObserverState {
                projection,
                current_manifest: manifest,
                journal_head: None,
            },
        };
        atomic_json(&observer.storage.join("policy.json"), &observer.policy)?;
        atomic_json(&observer.storage.join("baseline.json"), &baseline)?;
        observer.persist()?;
        Ok(observer)
    }

    pub fn recover_without_baseline_at(
        root: &Path,
        storage: &Path,
        identity: ObserverIdentity,
        policy: CandidatePathPolicy,
        config: ObserverConfig,
        now: i64,
    ) -> Result<Self> {
        config.validate()?;
        policy.verify()?;
        fs::create_dir_all(storage)?;
        let source = source_for_root(root, identity)?;
        let manifest = scan_manifest(root, &policy)?;
        let baseline = BaselineFile {
            schema_version: OBSERVER_SCHEMA_VERSION,
            source: source.clone(),
            policy_digest: policy.digest.clone(),
            established_at: None,
            baseline_time_unknown: true,
            manifest: manifest.clone(),
        };
        let projection = ObserverProjection {
            schema_version: OBSERVER_SCHEMA_VERSION,
            source,
            policy_digest: policy.digest.clone(),
            timing_policy: config.clone(),
            manifest_digest: manifest.digest.clone(),
            content_seq: 0,
            baseline_time_unknown: true,
            last_activity: None,
            health: ObserverHealth::PollOnly,
            degraded_reason: Some("baseline-time-unknown".into()),
            last_reconciliation_at: now,
            ignored_churn: BTreeMap::new(),
            unstable_scans: 0,
            watcher_overflows: 0,
            stale_callbacks: 0,
            classification_hold: None,
            preservation_mode: false,
            purported_reap: false,
            quarantine_required: false,
            late_mutations: Vec::new(),
            next_safe_action:
                "preserve bytes; do not infer an activity timestamp from filesystem metadata".into(),
        };
        let observer = Self {
            storage: storage.to_path_buf(),
            policy,
            config,
            state: ObserverState {
                projection,
                current_manifest: manifest,
                journal_head: None,
            },
        };
        atomic_json(&observer.storage.join("policy.json"), &observer.policy)?;
        atomic_json(&observer.storage.join("baseline.json"), &baseline)?;
        observer.persist()?;
        Ok(observer)
    }

    pub fn open_at(storage: &Path, expected: ObserverIdentity, _now: i64) -> Result<Self> {
        let policy: CandidatePathPolicy = read_json(&storage.join("policy.json"))?;
        policy.verify()?;
        let baseline: BaselineFile = read_json(&storage.join("baseline.json"))?;
        let state: ObserverState = read_json(&storage.join("state.json"))?;
        if !same_attempt_lease(&baseline.source.identity, &expected)
            || state.projection.source.identity != expected
        {
            bail!("observer source tuple mismatch");
        }
        if baseline.policy_digest != policy.digest
            || state.projection.policy_digest != policy.digest
        {
            bail!("observer policy snapshot mismatch");
        }
        let config = read_json::<ObserverRuntimeFile>(&storage.join("runtime.json"))
            .map(|r| r.config)
            .unwrap_or_default();
        config.validate()?;
        Ok(Self {
            storage: storage.to_path_buf(),
            policy,
            config,
            state,
        })
    }

    pub fn open(storage: &Path) -> Result<Self> {
        let state: ObserverState = read_json(&storage.join("state.json"))?;
        Self::open_at(
            storage,
            state.projection.source.identity.clone(),
            chrono::Utc::now().timestamp(),
        )
    }

    pub fn projection(&self) -> &ObserverProjection {
        &self.state.projection
    }
    pub fn config(&self) -> &ObserverConfig {
        &self.config
    }
    pub fn storage(&self) -> &Path {
        &self.storage
    }

    pub fn rebind_process_epoch_from_watchdog_at(
        &mut self,
        expected: &ObserverIdentity,
        new_process_epoch: u32,
        now: i64,
    ) -> Result<ObserverIdentity> {
        if expected != &self.state.projection.source.identity {
            bail!("stale observer/process epoch rebind");
        }
        if new_process_epoch <= expected.process_epoch {
            bail!("process epoch must advance monotonically");
        }
        self.state.projection.source.identity.process_epoch = new_process_epoch;
        self.state.projection.source.identity.observer_epoch = self
            .state
            .projection
            .source
            .identity
            .observer_epoch
            .saturating_add(1);
        self.state.projection.last_reconciliation_at = now;
        self.persist()?;
        Ok(self.state.projection.source.identity.clone())
    }

    pub fn claim_observer_epoch_at(&mut self, now: i64) -> Result<u64> {
        self.state.projection.source.identity.observer_epoch = self
            .state
            .projection
            .source
            .identity
            .observer_epoch
            .saturating_add(1);
        self.state.projection.last_reconciliation_at = now;
        self.persist()?;
        Ok(self.state.projection.source.identity.observer_epoch)
    }

    pub fn reconcile_callback_at(
        &mut self,
        callback: &ObserverIdentity,
        source: ReconcileSource,
        now: i64,
    ) -> Result<ReconcileOutcome> {
        if callback != &self.state.projection.source.identity {
            self.state.projection.stale_callbacks =
                self.state.projection.stale_callbacks.saturating_add(1);
            self.persist()?;
            return Ok(ReconcileOutcome::StaleCallback);
        }
        self.reconcile_at(source, now)
    }

    pub fn reconcile_at(&mut self, source: ReconcileSource, now: i64) -> Result<ReconcileOutcome> {
        if let Err(error) = validate_root(&self.state.projection.source) {
            let reason = bounded(&error.to_string(), 240);
            self.state.projection.health = ObserverHealth::ClassificationHold;
            self.state.projection.classification_hold = Some(reason.clone());
            self.state.projection.quarantine_required = true;
            self.state.projection.next_safe_action = "quarantine: canonical worktree root identity changed; inspect lease before any seal or cleanup".into();
            self.persist()?;
            return Ok(ReconcileOutcome::Held(reason));
        }
        let root = Path::new(&self.state.projection.source.canonical_worktree_root);
        let next = match scan_manifest(root, &self.policy) {
            Ok(manifest) => manifest,
            Err(error) => {
                let reason = bounded(&error.to_string(), 240);
                if reason.starts_with("scan-unstable:") {
                    self.state.projection.unstable_scans =
                        self.state.projection.unstable_scans.saturating_add(1);
                    self.state.projection.health = ObserverHealth::RescanRequired;
                } else {
                    self.state.projection.health = ObserverHealth::ClassificationHold;
                }
                self.state.projection.classification_hold = Some(reason.clone());
                self.state.projection.quarantine_required = true;
                self.state.projection.next_safe_action =
                    "hold candidate classification and retry a stable full reconciliation".into();
                self.persist()?;
                return Ok(ReconcileOutcome::Held(reason));
            }
        };
        self.state.projection.last_reconciliation_at = now;
        self.state.projection.classification_hold = None;
        if self.state.projection.late_mutations.is_empty() {
            self.state.projection.quarantine_required = false;
        }
        count_excluded_churn(
            &self.state.current_manifest.excluded,
            &next.excluded,
            &mut self.state.projection.ignored_churn,
        );
        if next.digest == self.state.current_manifest.digest {
            self.state.projection.manifest_digest = next.digest.clone();
            self.state.current_manifest = next;
            if self.state.projection.health == ObserverHealth::RescanRequired {
                self.state.projection.health = ObserverHealth::EventAndReconcile;
                self.state.projection.degraded_reason = None;
            }
            self.persist()?;
            return Ok(ReconcileOutcome::Unchanged);
        }
        let changed = manifest_delta(&self.state.current_manifest, &next);
        let prior = self.state.current_manifest.digest.clone();
        if self.state.projection.preservation_mode {
            let reason = if self.state.projection.purported_reap {
                "late-write-after-reap"
            } else {
                "late-worktree-mutation-observed"
            };
            let record = self.append_record(
                &prior,
                &next.digest,
                changed,
                reason,
                source,
                now,
                self.state.projection.content_seq,
            )?;
            let late = LateMutation {
                observed_at: record.wall_timestamp,
                prior_manifest_digest: prior.clone(),
                new_manifest_digest: next.digest.clone(),
                reason: reason.into(),
                changed_paths: record.changed_paths,
            };
            if !self.state.projection.late_mutations.iter().any(|m| {
                m.prior_manifest_digest == late.prior_manifest_digest
                    && m.new_manifest_digest == late.new_manifest_digest
                    && m.reason == late.reason
            }) {
                self.state.projection.late_mutations.push(late);
                if self.state.projection.late_mutations.len() > 32 {
                    self.state.projection.late_mutations.remove(0);
                }
            }
            self.state.projection.quarantine_required = true;
            self.state.projection.next_safe_action = "preserve/quarantine and request finalizer reconciliation; never wake or terminalize from late bytes".into();
            self.state.projection.manifest_digest = next.digest.clone();
            self.state.current_manifest = next;
            self.persist()?;
            return Ok(ReconcileOutcome::LateMutation);
        }
        self.state.projection.content_seq = self.state.projection.content_seq.saturating_add(1);
        let seq = self.state.projection.content_seq;
        let record = self.append_record(
            &prior,
            &next.digest,
            changed,
            "candidate-manifest-advance",
            source,
            now,
            seq,
        )?;
        let recorded_seq = record.content_seq;
        self.state.projection.content_seq = recorded_seq;
        self.state.projection.last_activity = Some(ActivitySummary {
            observed_at: record.wall_timestamp,
            content_seq: record.content_seq,
            source: record.reconciliation_source,
            changed_paths: record.changed_paths,
        });
        self.state.projection.manifest_digest = next.digest.clone();
        if self.state.projection.health != ObserverHealth::PollOnly {
            self.state.projection.health = ObserverHealth::EventAndReconcile;
            self.state.projection.degraded_reason = None;
        }
        self.state.current_manifest = next;
        self.persist()?;
        Ok(ReconcileOutcome::Advanced(recorded_seq))
    }

    pub fn enter_preservation_at(&mut self, purported_reap: bool, now: i64) -> Result<()> {
        self.state.projection.preservation_mode = true;
        self.state.projection.purported_reap |= purported_reap;
        self.state.projection.last_reconciliation_at = now;
        self.state.projection.next_safe_action = "preserve exact worktree until finalizer seals or retains it; late bytes cannot restore authority".into();
        self.persist()
    }

    pub fn mark_watcher_unavailable_at(&mut self, reason: &str, now: i64) -> Result<()> {
        self.state.projection.health = ObserverHealth::PollOnly;
        self.state.projection.degraded_reason =
            Some(format!("watcher-unavailable:{}", bounded(reason, 120)));
        self.state.projection.last_reconciliation_at = now;
        self.persist()
    }

    pub fn mark_overflow_at(&mut self, reason: &str, now: i64) -> Result<()> {
        self.state.projection.health = ObserverHealth::RescanRequired;
        self.state.projection.degraded_reason =
            Some(format!("rescan-required:{}", bounded(reason, 120)));
        self.state.projection.watcher_overflows =
            self.state.projection.watcher_overflows.saturating_add(1);
        self.persist()?;
        let _ = self.reconcile_at(ReconcileSource::Overflow, now)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn append_record(
        &mut self,
        prior: &str,
        next: &str,
        changed: Vec<ChangedPath>,
        operation: &str,
        source: ReconcileSource,
        now: i64,
        seq: u64,
    ) -> Result<ActivityRecord> {
        let journal_path = self.storage.join("activity.jsonl");
        let records = read_activity_records(&journal_path)?;
        if let Some(existing) = records.last().filter(|record| {
            record.prior_manifest_digest == prior
                && record.new_manifest_digest == next
                && record.operation_kind == operation
        }) {
            self.state.journal_head = Some(existing.record_hash.clone());
            return Ok(existing.clone());
        }
        let mut effective_prior = prior.to_owned();
        let mut effective_seq = seq;
        if let Some(tail) = records.last() {
            let projection_head_matches =
                self.state.journal_head.as_deref() == Some(tail.record_hash.as_str());
            self.state.journal_head = Some(tail.record_hash.clone());
            if !projection_head_matches || tail.new_manifest_digest != prior {
                effective_prior = tail.new_manifest_digest.clone();
                effective_seq = effective_seq.max(tail.content_seq.saturating_add(1));
            }
        }
        let body = ActivityRecordBody {
            schema_version: OBSERVER_SCHEMA_VERSION,
            source_tuple: &self.state.projection.source,
            content_seq: effective_seq,
            prior_manifest_digest: &effective_prior,
            new_manifest_digest: next,
            changed_paths: &changed,
            operation_kind: operation,
            wall_timestamp: now,
            reconciliation_source: source.label(),
            prior_record_hash: &self.state.journal_head,
        };
        let hash = format!("b3:{}", blake3::hash(&serde_json::to_vec(&body)?).to_hex());
        let record = ActivityRecord {
            schema_version: OBSERVER_SCHEMA_VERSION,
            source_tuple: self.state.projection.source.clone(),
            content_seq: effective_seq,
            prior_manifest_digest: effective_prior,
            new_manifest_digest: next.into(),
            changed_paths: changed,
            operation_kind: operation.into(),
            wall_timestamp: now,
            reconciliation_source: source.label().into(),
            prior_record_hash: self.state.journal_head.clone(),
            record_hash: hash.clone(),
        };
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(journal_path)?;
        serde_json::to_writer(&mut file, &record)?;
        file.write_all(b"\n")?;
        file.flush()?;
        file.sync_all()?;
        self.state.journal_head = Some(hash);
        Ok(record)
    }

    fn persist(&self) -> Result<()> {
        atomic_json(&self.storage.join("state.json"), &self.state)?;
        atomic_json(
            &self.storage.join("runtime.json"),
            &ObserverRuntimeFile {
                config: self.config.clone(),
            },
        )
    }
}

#[derive(Serialize, Deserialize)]
struct ObserverRuntimeFile {
    config: ObserverConfig,
}

pub fn read_projection(storage: &Path) -> Result<ObserverProjection> {
    let mut projection = read_json::<ObserverState>(&storage.join("state.json"))?.projection;
    // Health is partly a live-process fact. Never let a durable last-known
    // `event-and-reconcile` value make operator surfaces claim that a dead
    // exact PID is active. This is a read-model overlay only: all state and
    // evidence remain untouched for restart reconciliation.
    if matches!(
        projection.health,
        ObserverHealth::EventAndReconcile | ObserverHealth::RescanRequired
    ) {
        let owner = watcher_lock_owner(storage);
        if !owner.is_some_and(|pid| watcher_owner_is_live(pid, storage)) {
            projection.health = ObserverHealth::PollOnly;
            projection.degraded_reason = Some(match owner {
                Some(pid) => format!("observer-process-exited:pid={pid}"),
                None => "observer-process-not-started-or-exited".into(),
            });
            projection.next_safe_action = format!(
                "state preserved; recover with argv: wg worktree-observer-run --state-dir {}",
                storage.display()
            );
        }
    }
    Ok(projection)
}

/// Load bounded observer projections for operator/service read models. Corrupt
/// entries are omitted here (their task-specific `wg show` path reports them as
/// unavailable); this function never repairs or mutates observer state.
pub fn list_projections(wg_dir: &Path) -> Vec<ObserverProjection> {
    let mut projections =
        crate::attempt_runtime::list_component_dirs(wg_dir, "worktree-observer", 256)
            .into_iter()
            .filter_map(|path| read_projection(&path).ok())
            .collect::<Vec<_>>();
    projections.sort_by(|a, b| {
        a.source
            .identity
            .task_id
            .cmp(&b.source.identity.task_id)
            .then(
                a.source
                    .identity
                    .attempt_id
                    .cmp(&b.source.identity.attempt_id),
            )
    });
    projections
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenProgressReadModel {
    pub seq: u64,
    pub observed_at: i64,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityClocksReadModel {
    pub schema_version: u32,
    pub worktree_authority: String,
    pub worktree_content_seq: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_observed_worktree_activity: Option<ActivitySummary>,
    pub last_proven_progress: Option<ProvenProgressReadModel>,
    pub meaningful_silence_secs: u64,
    pub observed_activity_grace_secs: u64,
    pub max_observed_only_extension_secs: u64,
    pub deadline: Option<DeadlineProjection>,
    pub observer: ObserverProjection,
}

/// Build the stable cross-surface read model. `proven` can only come from the
/// Pi receipt/watchdog channel; callers must never synthesize it from the
/// observer journal.
pub fn activity_clocks_read_model(
    observer: ObserverProjection,
    proven: Option<ProvenProgressReadModel>,
) -> ActivityClocksReadModel {
    let deadline = proven.as_ref().map(|progress| {
        calculate_suspect_deadline(DeadlineInput {
            last_proven_at: progress.observed_at,
            last_proven_seq: progress.seq,
            last_observed_at: observer.last_activity.as_ref().map(|a| a.observed_at),
            observed_after_proven_seq: None,
            meaningful_silence_secs: DEFAULT_MEANINGFUL_SILENCE_SECS,
            observed_activity_grace_secs: observer.timing_policy.observed_activity_grace_secs,
            max_observed_only_extension_secs: observer
                .timing_policy
                .max_observed_only_extension_secs,
        })
    });
    ActivityClocksReadModel {
        schema_version: OBSERVER_SCHEMA_VERSION,
        worktree_authority: "observed-unproven".into(),
        worktree_content_seq: observer.content_seq,
        last_observed_worktree_activity: observer.last_activity.clone(),
        last_proven_progress: proven,
        meaningful_silence_secs: DEFAULT_MEANINGFUL_SILENCE_SECS,
        observed_activity_grace_secs: observer.timing_policy.observed_activity_grace_secs,
        max_observed_only_extension_secs: observer.timing_policy.max_observed_only_extension_secs,
        deadline,
        observer,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadlineInput {
    pub last_proven_at: i64,
    pub last_proven_seq: u64,
    pub last_observed_at: Option<i64>,
    /// Proven-progress sequence current when the candidate sequence advanced.
    pub observed_after_proven_seq: Option<u64>,
    pub meaningful_silence_secs: u64,
    pub observed_activity_grace_secs: u64,
    pub max_observed_only_extension_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeadlineProjection {
    pub proof_deadline: i64,
    pub observed_deadline: Option<i64>,
    pub suspect_at: i64,
    pub observed_only_extension_secs: u64,
    pub hard_cap_secs: u64,
}

pub fn calculate_suspect_deadline(input: DeadlineInput) -> DeadlineProjection {
    let proof_deadline = input
        .last_proven_at
        .saturating_add(input.meaningful_silence_secs.min(i64::MAX as u64) as i64);
    let applicable = input
        .last_observed_at
        .zip(input.observed_after_proven_seq)
        .filter(|(_, proof_seq_at_observation)| *proof_seq_at_observation == input.last_proven_seq);
    let observed_deadline = applicable.map(|(observed_at, _)| {
        let grace_deadline = observed_at
            .saturating_add(input.observed_activity_grace_secs.min(i64::MAX as u64) as i64);
        let cap = proof_deadline
            .saturating_add(input.max_observed_only_extension_secs.min(i64::MAX as u64) as i64);
        grace_deadline.min(cap)
    });
    let suspect_at =
        observed_deadline.map_or(proof_deadline, |deadline| proof_deadline.max(deadline));
    DeadlineProjection {
        proof_deadline,
        observed_deadline,
        suspect_at,
        observed_only_extension_secs: suspect_at.saturating_sub(proof_deadline) as u64,
        hard_cap_secs: input.max_observed_only_extension_secs,
    }
}

#[derive(Debug, Clone, Copy)]
enum WatchWake {
    Candidate,
    IgnoredAccounting,
}

const INGRESS_REASONS: [&str; 7] = [
    "internal-control",
    "configured-generated",
    "volatile-target",
    "dependency-tree",
    "cache-tree",
    "temporary-file",
    "wg-runtime",
];

#[derive(Default)]
struct IngressAccounting {
    ignored: [AtomicU64; INGRESS_REASONS.len()],
    overflowed: AtomicU64,
}

impl IngressAccounting {
    fn record_ignored(&self, reason: &str) {
        if let Some(index) = INGRESS_REASONS.iter().position(|known| *known == reason) {
            saturating_atomic_increment(&self.ignored[index]);
        }
    }

    fn record_overflow(&self) {
        saturating_atomic_increment(&self.overflowed);
    }

    fn drain_into(&self, observer: &mut WorktreeObserver, now: i64) -> Result<bool> {
        let mut changed = false;
        for (index, reason) in INGRESS_REASONS.iter().enumerate() {
            let count = self.ignored[index].swap(0, Ordering::AcqRel);
            if count > 0 {
                let slot = observer
                    .state
                    .projection
                    .ignored_churn
                    .entry((*reason).into())
                    .or_default();
                *slot = slot.saturating_add(count);
                changed = true;
            }
        }
        let overflows = self.overflowed.swap(0, Ordering::AcqRel);
        if overflows > 0 {
            observer.state.projection.health = ObserverHealth::RescanRequired;
            observer.state.projection.degraded_reason =
                Some("rescan-required:bounded watcher wake queue overflow".into());
            observer.state.projection.watcher_overflows = observer
                .state
                .projection
                .watcher_overflows
                .saturating_add(overflows);
            observer.persist()?;
            let _ = observer.reconcile_at(ReconcileSource::Overflow, now)?;
            return Ok(true);
        }
        if changed {
            observer.persist()?;
        }
        Ok(false)
    }
}

fn saturating_atomic_increment(value: &AtomicU64) {
    let _ = value.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_add(1))
    });
}

fn candidate_watch_directories(
    root: &Path,
    policy: &CandidatePathPolicy,
) -> Result<BTreeSet<PathBuf>> {
    let mut directories = BTreeSet::new();
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.path() == root || !entry.file_type().is_dir() {
                return true;
            }
            let Ok(relative) = entry.path().strip_prefix(root) else {
                return false;
            };
            let Ok(path) = relative_path(relative) else {
                return true;
            };
            if policy.explicit_may_be_beneath(&path) {
                return true;
            }
            !is_internal_control(&path)
                && volatile_reason(&path).is_none()
                && !policy.generated_directory(&path)
        });
    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_dir() {
            continue;
        }
        directories.insert(entry.path().to_path_buf());
        if directories.len() > MAX_WATCHED_SOURCE_DIRECTORIES {
            bail!(
                "candidate watcher directory limit exceeded; use bounded periodic reconciliation"
            );
        }
    }
    Ok(directories)
}

fn sync_candidate_watches(
    watcher: &mut RecommendedWatcher,
    root: &Path,
    policy: &CandidatePathPolicy,
    watched: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    let desired = candidate_watch_directories(root, policy)?;
    // Backend notifications are hints. Watch only source-capable directories;
    // excluded build trees are never subscribed, so their kernel traffic does
    // not create an unbounded post-build callback drain. A periodic stable scan
    // refreshes this bounded set and remains the exact evidence authority.
    for directory in desired.difference(watched) {
        watcher.watch(directory, RecursiveMode::NonRecursive)?;
    }
    for directory in watched.difference(&desired) {
        let _ = watcher.unwatch(directory);
    }
    *watched = desired;
    Ok(())
}

fn classify_ingress_event(
    root: &Path,
    policy: &CandidatePathPolicy,
    event: &Event,
    accounting: &IngressAccounting,
) -> bool {
    if event.paths.is_empty() {
        return true;
    }
    let mut candidate = false;
    for absolute in &event.paths {
        let Ok(relative) = absolute.strip_prefix(root) else {
            // An unexpected backend path must cause a stable scan rather than
            // being guessed away.
            candidate = true;
            continue;
        };
        let Ok(path) = relative_path(relative) else {
            candidate = true;
            continue;
        };
        if path.is_empty() {
            candidate = true;
        } else if let Some(reason) = policy.ingress_exclusion(&path) {
            accounting.record_ignored(reason);
        } else {
            candidate = true;
        }
    }
    candidate
}

pub fn run_watch_loop(storage: &Path, parent_pid: Option<u32>) -> Result<()> {
    let Some(_lease) = WatcherLease::acquire(storage)? else {
        // Another live observer owns the exact attempt directory. The existing
        // native watcher plus its periodic reconciler remain authoritative.
        return Ok(());
    };
    let mut observer = WorktreeObserver::open(storage)?;
    observer.claim_observer_epoch_at(chrono::Utc::now().timestamp())?;
    let callback_identity = observer.projection().source.identity.clone();
    refresh_authority(&mut observer, storage, parent_pid)?;
    let _ = observer.reconcile_callback_at(
        &callback_identity,
        ReconcileSource::Startup,
        chrono::Utc::now().timestamp(),
    )?;
    let (tx, rx) = mpsc::sync_channel::<WatchWake>(WATCHER_WAKE_QUEUE_CAPACITY);
    let root = PathBuf::from(&observer.projection().source.canonical_worktree_root);
    let callback_root = root.clone();
    let callback_policy = observer.policy.clone();
    let ingress = Arc::new(IngressAccounting::default());
    let callback_ingress = Arc::clone(&ingress);
    let watcher_result: notify::Result<RecommendedWatcher> =
        notify::recommended_watcher(move |event: notify::Result<Event>| match event {
            Ok(event) => {
                // Fingerprinting necessarily opens/reads candidate files. The
                // native backend reports those Open/Close events as Access;
                // treating them as wakeups creates a self-exciting reconcile
                // loop even after external churn stops.
                if matches!(event.kind, EventKind::Access(_)) {
                    return;
                }
                let candidate = classify_ingress_event(
                    &callback_root,
                    &callback_policy,
                    &event,
                    &callback_ingress,
                );
                let wake = if candidate {
                    WatchWake::Candidate
                } else {
                    WatchWake::IgnoredAccounting
                };
                if let Err(mpsc::TrySendError::Full(_)) = tx.try_send(wake)
                    && candidate
                {
                    // Only dropping a possible source wake requires a stable
                    // full reconciliation. Ignored accounting is already held
                    // in fixed atomic category counters.
                    callback_ingress.record_overflow();
                }
            }
            Err(_) => callback_ingress.record_overflow(),
        });
    let mut watched_directories = BTreeSet::new();
    let mut watcher = match watcher_result {
        Ok(mut watcher) => match sync_candidate_watches(
            &mut watcher,
            &root,
            &observer.policy,
            &mut watched_directories,
        ) {
            Ok(()) => Some(watcher),
            Err(error) => {
                observer.mark_watcher_unavailable_at(
                    &error.to_string(),
                    chrono::Utc::now().timestamp(),
                )?;
                None
            }
        },
        Err(error) => {
            observer
                .mark_watcher_unavailable_at(&error.to_string(), chrono::Utc::now().timestamp())?;
            None
        }
    };
    let interval = Duration::from_secs(observer.config().reconcile_interval_secs);
    loop {
        if read_projection(storage)
            .map(|projection| projection.source.identity != callback_identity)
            .unwrap_or(true)
        {
            break;
        }
        refresh_authority(&mut observer, storage, parent_pid)?;
        match rx.recv_timeout(interval) {
            Ok(wake) => {
                if matches!(wake, WatchWake::Candidate) {
                    std::thread::sleep(Duration::from_millis(observer.config().debounce_ms));
                }
                // Authority may have changed while recv_timeout was blocked.
                // Recheck immediately before classifying candidate bytes.
                refresh_authority(&mut observer, storage, parent_pid)?;
                let now = chrono::Utc::now().timestamp();
                let overflow_reconciled = ingress.drain_into(&mut observer, now)?;
                if matches!(wake, WatchWake::Candidate) && !overflow_reconciled {
                    let _ = observer.reconcile_callback_at(
                        &callback_identity,
                        ReconcileSource::Event,
                        now,
                    )?;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                refresh_authority(&mut observer, storage, parent_pid)?;
                let now = chrono::Utc::now().timestamp();
                let overflow_reconciled = ingress.drain_into(&mut observer, now)?;
                if !overflow_reconciled {
                    let _ = observer.reconcile_callback_at(
                        &callback_identity,
                        ReconcileSource::Periodic,
                        now,
                    )?;
                }
                if let Some(active_watcher) = watcher.as_mut()
                    && let Err(error) = sync_candidate_watches(
                        active_watcher,
                        &root,
                        &observer.policy,
                        &mut watched_directories,
                    )
                {
                    observer.mark_watcher_unavailable_at(&error.to_string(), now)?;
                    watcher = None;
                    watched_directories.clear();
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                refresh_authority(&mut observer, storage, parent_pid)?;
                let now = chrono::Utc::now().timestamp();
                let _ = ingress.drain_into(&mut observer, now)?;
                observer.mark_watcher_unavailable_at("native watcher disconnected", now)?;
                watcher = None;
                let _ = observer.reconcile_callback_at(
                    &callback_identity,
                    ReconcileSource::Periodic,
                    now,
                )?;
                std::thread::sleep(interval);
            }
        }
        // A removed/replaced root is a permanent fail-closed hold.  The state
        // remains durable; do not follow a newly-created path.
        if observer.projection().health == ObserverHealth::ClassificationHold {
            break;
        }
        let _keep_watcher_alive = &watcher;
    }
    Ok(())
}

struct WatcherLease {
    path: PathBuf,
    pid: u32,
}

impl WatcherLease {
    fn acquire(storage: &Path) -> Result<Option<Self>> {
        let path = storage.join("watcher.lock");
        let pid = std::process::id();
        for _ in 0..2 {
            match OpenOptions::new().create_new(true).write(true).open(&path) {
                Ok(mut file) => {
                    writeln!(file, "{pid}")?;
                    file.sync_all()?;
                    return Ok(Some(Self { path, pid }));
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let owner = fs::read_to_string(&path)
                        .ok()
                        .and_then(|text| text.trim().parse::<u32>().ok());
                    if owner.is_some_and(|owner| watcher_owner_is_live(owner, storage)) {
                        return Ok(None);
                    }
                    let _ = fs::remove_file(&path);
                }
                Err(error) => return Err(error.into()),
            }
        }
        bail!("worktree observer watcher lease could not be acquired")
    }
}

impl Drop for WatcherLease {
    fn drop(&mut self) {
        let owned = fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            == Some(self.pid);
        if owned {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Reattach missing observer processes at daemon startup. Existing live
/// watcher leases make this idempotent; each child reconciles immediately and
/// then every snapshotted interval.
pub fn restart_current_observers(wg_dir: &Path) -> Result<usize> {
    let executable = std::env::current_exe()?;
    let mut started = 0usize;
    for projection in list_projections(wg_dir).into_iter().take(256) {
        let identity = &projection.source.identity;
        let key = crate::attempt_runtime::AttemptRuntimeKey::new(
            &identity.task_id,
            identity.generation,
            &identity.attempt_id,
            identity.attempt_fence,
            identity.worktree_lease_epoch,
        );
        let state_dir =
            crate::attempt_runtime::component_for_update(wg_dir, &key, "worktree-observer")?;
        if !state_dir.exists() {
            continue;
        }
        if !source_still_current(&state_dir, &projection) {
            continue;
        }
        let lock_owner = watcher_lock_owner(&state_dir);
        if lock_owner.is_some_and(|owner| watcher_owner_is_live(owner, &state_dir)) {
            continue;
        }
        let mut command = Command::new(&executable);
        command
            .arg("--dir")
            .arg(wg_dir)
            .arg("worktree-observer-run")
            .arg("--state-dir")
            .arg(&state_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                command.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
        }
        command.spawn()?;
        started = started.saturating_add(1);
    }
    Ok(started)
}

fn refresh_authority(
    observer: &mut WorktreeObserver,
    storage: &Path,
    parent_pid: Option<u32>,
) -> Result<()> {
    if !observer.projection().preservation_mode
        && (!source_still_current(storage, observer.projection())
            || parent_pid.is_some_and(|pid| !process_alive(pid)))
    {
        observer.enter_preservation_at(false, chrono::Utc::now().timestamp())?;
    }
    Ok(())
}

fn source_still_current(storage: &Path, projection: &ObserverProjection) -> bool {
    let mut ancestor = Some(storage);
    let wg_dir = loop {
        let Some(path) = ancestor else {
            return false;
        };
        if path.join("graph.jsonl").is_file() {
            break path;
        }
        ancestor = path.parent();
    };
    let graph_path = wg_dir.join("graph.jsonl");
    let Ok(graph) = crate::parser::load_graph(&graph_path) else {
        return false;
    };
    graph
        .get_task(&projection.source.identity.task_id)
        .is_some_and(|task| {
            task.status == crate::graph::Status::InProgress
                && task.lifecycle.generation == projection.source.identity.generation
                && task.lifecycle.fence == projection.source.identity.attempt_fence
                && task
                    .lifecycle
                    .current_attempt
                    .as_ref()
                    .is_some_and(|attempt| {
                        attempt.id == projection.source.identity.attempt_id
                            && attempt.disposition.is_none()
                    })
        })
}

fn watcher_lock_owner(storage: &Path) -> Option<u32> {
    fs::read_to_string(storage.join("watcher.lock"))
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
}

fn watcher_owner_is_live(pid: u32, storage: &Path) -> bool {
    if !process_alive(pid) {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        let Ok(command_line) = fs::read(format!("/proc/{pid}/cmdline")) else {
            return false;
        };
        let expected = storage.to_string_lossy();
        command_line
            .split(|byte| *byte == 0)
            .any(|argument| argument == expected.as_bytes())
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}
#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    true
}

fn same_attempt_lease(a: &ObserverIdentity, b: &ObserverIdentity) -> bool {
    a.task_id == b.task_id
        && a.generation == b.generation
        && a.attempt_id == b.attempt_id
        && a.attempt_fence == b.attempt_fence
        && a.worktree_id == b.worktree_id
        && a.worktree_lease_epoch == b.worktree_lease_epoch
}

fn source_for_root(root: &Path, identity: ObserverIdentity) -> Result<ObserverSource> {
    let root_identity = root_identity(root)?;
    Ok(ObserverSource {
        identity,
        canonical_worktree_root: root_identity.canonical_path.clone(),
        root_identity,
    })
}

fn validate_root(source: &ObserverSource) -> Result<()> {
    let current = root_identity(Path::new(&source.canonical_worktree_root))?;
    if current != source.root_identity {
        bail!("worktree-root-identity-mismatch");
    }
    Ok(())
}

fn root_identity(root: &Path) -> Result<RootIdentity> {
    let symlink = fs::symlink_metadata(root)
        .with_context(|| format!("cannot stat observer root {}", root.display()))?;
    if symlink.file_type().is_symlink() || !symlink.is_dir() {
        bail!("observer root must be a real directory, not a symlink or special entry");
    }
    let canonical = fs::canonicalize(root)?;
    let metadata = fs::metadata(&canonical)?;
    #[cfg(unix)]
    let (device, file_identity) = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let (device, file_identity) = (0, blake3_u64(canonical.to_string_lossy().as_bytes()));
    Ok(RootIdentity {
        canonical_path: canonical.to_string_lossy().into_owned(),
        device,
        file_identity,
    })
}

#[cfg(not(unix))]
fn blake3_u64(bytes: &[u8]) -> u64 {
    let digest = blake3::hash(bytes);
    u64::from_le_bytes(digest.as_bytes()[..8].try_into().unwrap())
}

#[derive(Debug)]
struct TrackedEntry {
    mode: u32,
    oid: String,
}

fn tracked_entries(root: &Path) -> Result<BTreeMap<String, TrackedEntry>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "--stage", "-z"])
        .output()?;
    if !output.status.success() {
        bail!("git-index-unavailable");
    }
    let mut map = BTreeMap::new();
    for record in output.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let Some(tab) = record.iter().position(|b| *b == b'\t') else {
            bail!("git-index-record-invalid");
        };
        let header = std::str::from_utf8(&record[..tab])
            .map_err(|_| anyhow::anyhow!("non-utf8 tracked path classification hold"))?;
        let path = std::str::from_utf8(&record[tab + 1..])
            .map_err(|_| anyhow::anyhow!("non-utf8 tracked path classification hold"))?
            .replace('\\', "/");
        let mut fields = header.split_whitespace();
        let mode = u32::from_str_radix(
            fields
                .next()
                .ok_or_else(|| anyhow::anyhow!("missing git mode"))?,
            8,
        )?;
        let oid = fields
            .next()
            .ok_or_else(|| anyhow::anyhow!("missing git oid"))?
            .to_string();
        let stage = fields.next().unwrap_or("0");
        if stage != "0" {
            bail!("unmerged-index-entry:{path}");
        }
        map.insert(path, TrackedEntry { mode, oid });
    }
    Ok(map)
}

fn ignored_paths(root: &Path, paths: &[String]) -> Result<BTreeSet<String>> {
    if paths.is_empty() {
        return Ok(BTreeSet::new());
    }
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["check-ignore", "--no-index", "-z", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("git check-ignore stdin unavailable"))?;
        for path in paths {
            stdin.write_all(path.as_bytes())?;
            stdin.write_all(&[0])?;
        }
    }
    let output = child.wait_with_output()?;
    if !output.status.success() && output.status.code() != Some(1) {
        bail!("git-ignore-classification-unavailable");
    }
    let mut set = BTreeSet::new();
    for path in output.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        set.insert(
            std::str::from_utf8(path)
                .map_err(|_| anyhow::anyhow!("non-utf8 ignored path classification hold"))?
                .replace('\\', "/"),
        );
    }
    Ok(set)
}

#[derive(Default)]
struct ExcludedAccumulator {
    by_reason: BTreeMap<String, (blake3::Hasher, u64)>,
}

impl ExcludedAccumulator {
    fn record(&mut self, reason: &str, path: &str, signature: &str) {
        let (hasher, count) = self
            .by_reason
            .entry(reason.into())
            .or_insert_with(|| (blake3::Hasher::new(), 0));
        hasher.update(&(path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update(&(signature.len() as u64).to_le_bytes());
        hasher.update(signature.as_bytes());
        *count = count.saturating_add(1);
    }

    fn into_signatures(self) -> BTreeMap<String, ExcludedSignature> {
        self.by_reason
            .into_iter()
            .map(|(reason, (hasher, count))| {
                (
                    format!("@{reason}"),
                    ExcludedSignature {
                        reason,
                        signature: format!("b3:{}:{count}", hasher.finalize().to_hex()),
                    },
                )
            })
            .collect()
    }
}

fn scan_manifest(root: &Path, policy: &CandidatePathPolicy) -> Result<ManifestSnapshot> {
    let tracked = tracked_entries(root)?;
    let mut discovered = Vec::<(String, PathBuf, fs::FileType)>::new();
    let mut excluded_accumulator = ExcludedAccumulator::default();
    {
        let walker = WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                if entry.path() == root || !entry.file_type().is_dir() {
                    return true;
                }
                let Ok(relative) = entry.path().strip_prefix(root) else {
                    return false;
                };
                let Ok(path) = relative_path(relative) else {
                    return true;
                };
                let exclusion = if is_internal_control(&path) {
                    Some("internal-control")
                } else if policy.generated_directory(&path) {
                    Some("configured-generated")
                } else {
                    volatile_reason(&path)
                };
                let Some(reason) = exclusion else {
                    return true;
                };
                let prefix = format!("{}/", path.trim_end_matches('/'));
                let tracked_beneath = tracked
                    .keys()
                    .any(|candidate| candidate.starts_with(&prefix));
                if tracked_beneath || policy.explicit_may_be_beneath(&path) {
                    return true;
                }
                // Drop the entire excluded subtree at walk ingress. Retain one
                // bounded, category-level metadata signal rather than a path or
                // content record for each generated entry.
                let signature = diagnostic_signature(entry.path())
                    .unwrap_or_else(|_| "unreadable-excluded-root".into());
                excluded_accumulator.record(reason, &path, &signature);
                false
            });
        for item in walker {
            let entry = item?;
            if entry.path() == root {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|_| anyhow::anyhow!("escaping-path"))?;
            let path = relative_path(relative)?;
            if entry.file_type().is_dir() {
                continue;
            }
            discovered.push((path, entry.path().to_path_buf(), entry.file_type()));
            if discovered.len() > 200_000 {
                bail!("candidate-scan-entry-limit-exceeded");
            }
        }
    }
    // Git may reject a case-colliding index before a worktree is created. If a
    // platform nevertheless materializes only one spelling, do not silently
    // collapse the other tracked identity into it: an exact tracked path that
    // the directory walk did not report but the filesystem still resolves is
    // an explicit checkout/materialization mismatch. Ordinary tracked
    // deletions remain valid because the exact path does not resolve.
    let discovered_paths = discovered
        .iter()
        .map(|(path, _, _)| path.as_str())
        .collect::<BTreeSet<_>>();
    for (path, tracked_entry) in &tracked {
        if tracked_entry.mode == 0o160000 || discovered_paths.contains(path.as_str()) {
            continue;
        }
        if fs::symlink_metadata(root.join(path)).is_ok() {
            let materialized = discovered_paths
                .iter()
                .find(|candidate| candidate.to_lowercase() == path.to_lowercase())
                .copied()
                .unwrap_or("<non-exact-filesystem-alias>");
            bail!(
                "tracked-path-materialization-is-not-exact:{path}:materialized-as={materialized}"
            );
        }
    }

    let untracked: Vec<String> = discovered
        .iter()
        .map(|(p, _, _)| p)
        .filter(|p| !tracked.contains_key(*p))
        .cloned()
        .collect();
    let ignored = ignored_paths(root, &untracked)?;
    let mut entries = BTreeMap::new();
    for (path, absolute, file_type) in discovered {
        // Preserve exact, case-distinct paths when the checkout filesystem can
        // materialize both of them. Folding names here made observer attach
        // reject valid Linux repositories (and wedged every subsequent spawn)
        // even though the manifest and Git index are keyed by exact paths.
        // A case-insensitive checkout cannot yield two distinct directory
        // entries in this scan; its ordinary Git materialization checks remain
        // the authority for that platform.
        let tracked_entry = tracked.get(&path);
        let class = policy.classify(&path, tracked_entry.is_some(), ignored.contains(&path));
        match class {
            PathClass::Excluded(reason) => {
                excluded_accumulator.record(reason, &path, &diagnostic_signature(&absolute)?);
            }
            PathClass::Candidate(class_name) => {
                if !(file_type.is_file() || file_type.is_symlink()) {
                    bail!("special-candidate-entry:{path}");
                }
                let entry =
                    fingerprint_entry(&absolute, &path, class_name, tracked_entry.map(|t| t.mode))?;
                entries.insert(path, entry);
            }
        }
    }
    let mut excluded = excluded_accumulator.into_signatures();
    // Gitlinks are index identities, not recursively followed source.
    for (path, tracked_entry) in &tracked {
        if tracked_entry.mode == 0o160000 {
            match policy.classify(path, true, false) {
                PathClass::Excluded(reason) => {
                    excluded.insert(
                        path.clone(),
                        ExcludedSignature {
                            reason: reason.into(),
                            signature: tracked_entry.oid.clone(),
                        },
                    );
                }
                PathClass::Candidate(class_name) => {
                    entries.insert(
                        path.clone(),
                        ManifestEntry {
                            path: path.clone(),
                            kind: CandidateKind::Gitlink,
                            git_mode: 0o160000,
                            content_digest: format!("gitlink:{}", tracked_entry.oid),
                            size: 0,
                            class: class_name.into(),
                        },
                    );
                }
            }
        }
    }
    let digest = manifest_digest(&entries)?;
    Ok(ManifestSnapshot {
        digest,
        entries,
        excluded,
    })
}

fn relative_path(path: &Path) -> Result<String> {
    if path.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        bail!("escaping-path");
    }
    let value = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("non-utf8 path classification hold"))?
        .replace('\\', "/");
    if value.len() > 4096 {
        bail!("path-too-long classification hold");
    }
    Ok(value)
}

fn fingerprint_entry(
    absolute: &Path,
    path: &str,
    class: &str,
    tracked_mode: Option<u32>,
) -> Result<ManifestEntry> {
    let before =
        fs::symlink_metadata(absolute).with_context(|| format!("unreadable-candidate:{path}"))?;
    if before.file_type().is_symlink() {
        let target = fs::read_link(absolute)?;
        if symlink_escapes(path, &target) {
            bail!("escaping-symlink:{path}");
        }
        let bytes = os_bytes(target.as_os_str());
        let after = fs::symlink_metadata(absolute)?;
        if metadata_changed_during_scan(&before, &after) {
            bail!("scan-unstable:{path}");
        }
        return Ok(ManifestEntry {
            path: bounded(path, MAX_PATH_RENDER_BYTES),
            kind: CandidateKind::Symlink,
            git_mode: 0o120000,
            content_digest: format!("b3:{}", blake3::hash(&bytes).to_hex()),
            size: bytes.len() as u64,
            class: class.into(),
        });
    }
    if !before.is_file() {
        bail!("special-candidate-entry:{path}");
    }
    let mut file = File::open(absolute).with_context(|| format!("unreadable-candidate:{path}"))?;
    let mut hasher = blake3::Hasher::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        size = size.saturating_add(count as u64);
    }
    let after = fs::symlink_metadata(absolute)?;
    if metadata_changed_during_scan(&before, &after) || size != after.len() {
        bail!("scan-unstable:{path}");
    }
    let git_mode = regular_git_mode(&after, tracked_mode);
    Ok(ManifestEntry {
        path: bounded(path, MAX_PATH_RENDER_BYTES),
        kind: CandidateKind::Regular,
        git_mode,
        content_digest: format!("b3:{}", hasher.finalize().to_hex()),
        size,
        class: class.into(),
    })
}

fn symlink_escapes(link_path: &str, target: &Path) -> bool {
    if target.is_absolute() {
        return true;
    }
    let mut depth = link_path.split('/').count().saturating_sub(1) as i64;
    for component in target.components() {
        match component {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            Component::Normal(_) => depth += 1,
            Component::RootDir | Component::Prefix(_) => return true,
            Component::CurDir => {}
        }
    }
    false
}

fn os_bytes(value: &OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        value.as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        value.to_string_lossy().as_bytes().to_vec()
    }
}

fn regular_git_mode(metadata: &fs::Metadata, _tracked_mode: Option<u32>) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 != 0 {
            0o100755
        } else {
            0o100644
        }
    }
    #[cfg(not(unix))]
    {
        _tracked_mode.filter(|m| *m == 0o100755).unwrap_or(0o100644)
    }
}

fn metadata_changed_during_scan(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    before.len() != after.len() || before.modified().ok() != after.modified().ok()
}

fn diagnostic_signature(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    let kind = if metadata.file_type().is_symlink() {
        "symlink"
    } else if metadata.is_file() {
        "file"
    } else if metadata.is_dir() {
        "dir"
    } else {
        "special"
    };
    let modified = metadata
        .modified()
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    Ok(format!("{kind}:{}:{modified}", metadata.len()))
}

fn manifest_digest(entries: &BTreeMap<String, ManifestEntry>) -> Result<String> {
    #[derive(Serialize)]
    struct CandidateSemantics<'a> {
        path: &'a str,
        kind: &'a CandidateKind,
        git_mode: u32,
        content_digest: &'a str,
        size: u64,
    }
    let semantics = entries
        .values()
        .map(|entry| CandidateSemantics {
            path: &entry.path,
            kind: &entry.kind,
            git_mode: entry.git_mode,
            content_digest: &entry.content_digest,
            size: entry.size,
        })
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&semantics)?;
    Ok(format!("b3:{}", blake3::hash(&bytes).to_hex()))
}

fn same_candidate_semantics(a: &ManifestEntry, b: &ManifestEntry) -> bool {
    a.path == b.path
        && a.kind == b.kind
        && a.git_mode == b.git_mode
        && a.content_digest == b.content_digest
        && a.size == b.size
}

fn manifest_delta(before: &ManifestSnapshot, after: &ManifestSnapshot) -> Vec<ChangedPath> {
    let paths: BTreeSet<&String> = before.entries.keys().chain(after.entries.keys()).collect();
    paths
        .into_iter()
        .filter_map(|path| {
            let old = before.entries.get(path);
            let new = after.entries.get(path);
            if old
                .zip(new)
                .is_some_and(|(a, b)| same_candidate_semantics(a, b))
            {
                return None;
            }
            let operation = match (old, new) {
                (None, Some(_)) => "add",
                (Some(_), None) => "delete",
                (Some(a), Some(b)) if a.kind != b.kind => "atomic-replacement",
                (Some(a), Some(b)) if a.git_mode != b.git_mode => "mode-change",
                (Some(a), Some(b))
                    if a.kind == CandidateKind::Symlink && a.content_digest != b.content_digest =>
                {
                    "symlink-target-change"
                }
                (Some(a), Some(b))
                    if a.kind == CandidateKind::Gitlink && a.content_digest != b.content_digest =>
                {
                    "gitlink-change"
                }
                _ => "content-change",
            };
            Some(ChangedPath {
                path: bounded(path, MAX_PATH_RENDER_BYTES),
                class: new.or(old).map(|e| e.class.clone()).unwrap_or_default(),
                operation: operation.into(),
                before_digest: old.map(|e| e.content_digest.clone()),
                after_digest: new.map(|e| e.content_digest.clone()),
                byte_delta: new
                    .map_or(0, |e| e.size.min(i64::MAX as u64) as i64)
                    .saturating_sub(old.map_or(0, |e| e.size.min(i64::MAX as u64) as i64)),
            })
        })
        .take(MAX_CHANGED_PATHS)
        .collect()
}

fn count_excluded_churn(
    before: &BTreeMap<String, ExcludedSignature>,
    after: &BTreeMap<String, ExcludedSignature>,
    counts: &mut BTreeMap<String, u64>,
) {
    let paths: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    for path in paths {
        if before.get(path) != after.get(path) {
            let reason = after
                .get(path)
                .or_else(|| before.get(path))
                .map(|v| v.reason.as_str())
                .unwrap_or("excluded-other");
            let slot = counts.entry(reason.into()).or_default();
            *slot = slot.saturating_add(1);
        }
    }
    while counts.len() > 16 {
        if let Some(key) = counts.keys().next_back().cloned() {
            counts.remove(&key);
        }
    }
}

fn bounded(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_string();
    }
    let mut end = max;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.flush()?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    serde_json::from_slice(
        &fs::read(path).with_context(|| format!("cannot read {}", path.display()))?,
    )
    .with_context(|| format!("cannot parse {}", path.display()))
}
