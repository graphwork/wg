//! Host-captured deterministic validation evidence for completion review.
//!
//! A worker summary or task log may describe tests, but neither is validation
//! authority.  The ordinary `wg done` path executes configured commands itself,
//! streams stdout/stderr through bounded capture, and stores this canonical
//! envelope in the completion object store.  Review resolution re-checks the
//! envelope against the exact requirements, source revision, lifecycle attempt,
//! fence, repository, and selected manifest before a model sees it.

use crate::completion_manifest::{
    CompletionManifest, ContentDigest, IncompleteEvidence, IncompleteEvidenceKind, OutputRef,
    ResolvedReviewBundle,
};
use crate::completion_review::CompletionReviewBinding;
use crate::completion_task::requirements_digest;
use crate::graph::{
    CompletionContract as GraphCompletionContract, CompletionRepairBoundary,
    CompletionRepairDisposition, CompletionRepairPolicy, CompletionRepairState, Status, Task,
    WorkGraph, parse_delay,
};
use crate::identity::canonical_json;
use crate::simple_land::CompletionContract;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

pub const DETERMINISTIC_VALIDATION_VERSION: u32 = 1;
pub const DETERMINISTIC_VALIDATION_MEDIA_TYPE: &str =
    "application/vnd.worksgood.deterministic-validation+json";
pub const CONFIGURED_VALIDATION_EVIDENCE_KIND: &str = "deterministic-validation/configured/v1";
pub const BASELINE_VALIDATION_EVIDENCE_KIND: &str = "deterministic-validation/baseline/v1";
pub const SMOKE_FAILURE_EVIDENCE_KIND: &str = "completion-smoke/failure/v1";
const DETERMINISTIC_VALIDATION_PREFIX: &str = "deterministic-validation/";
const MAX_CAPTURE_BYTES_PER_STREAM: usize = 32 * 1024;
const MAX_COMMAND_BYTES: usize = 16 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 900;
const MAX_TIMEOUT_SECS: u64 = 3600;
const VALIDATION_AUTHORITY_VERSION: u32 = 1;
const VALIDATION_AUTHORITY_DIR: &str = "completion/v3/validation-authority";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationPurpose {
    Configured,
    Baseline,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ValidationCommandIdentity {
    pub configured_index: u32,
    pub shell: String,
    pub argv: Vec<String>,
    pub command_digest: ContentDigest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ValidationLifecycleBinding {
    pub task_id: String,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    pub attempt_fence: u64,
    pub requirements_digest: ContentDigest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ValidationRepositoryBinding {
    /// Digest of the canonical Git common directory.  This identifies the
    /// repository without disclosing a host path to the reviewer.
    pub repository_identity: ContentDigest,
    /// Digest of the canonical worktree root used for this execution.
    pub worktree_identity: ContentDigest,
    /// Digest of the canonical command cwd plus its repository-relative form.
    pub cwd_identity: ContentDigest,
    pub cwd_relative: String,
    pub before_head_oid: String,
    pub after_head_oid: String,
    pub before_tree_oid: String,
    pub after_tree_oid: String,
    pub integrated_main_oid: String,
    pub before_status_digest: ContentDigest,
    pub after_status_digest: ContentDigest,
    /// Digest of tracked working-tree changes plus exact untracked file bytes.
    /// Optional only for read compatibility with validation/v1 evidence written
    /// before deterministic repair feedback was introduced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_candidate_content_digest: Option<ContentDigest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_candidate_content_digest: Option<ContentDigest>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BoundedValidationOutput {
    /// Digest of every byte read from the stream, including bytes omitted from
    /// `content` by the review-size bound.
    pub digest: ContentDigest,
    pub total_bytes: u64,
    pub captured_bytes: u64,
    pub truncated: bool,
    pub encoding: String,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ValidationExitStatus {
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    pub timed_out: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeterministicValidationEvidence {
    pub evidence_version: u32,
    pub capture_origin: String,
    pub purpose: ValidationPurpose,
    pub command: ValidationCommandIdentity,
    pub lifecycle: ValidationLifecycleBinding,
    pub repository: ValidationRepositoryBinding,
    pub started_at: String,
    pub finished_at: String,
    pub duration_ms: u64,
    pub exit: ValidationExitStatus,
    pub stdout: BoundedValidationOutput,
    pub stderr: BoundedValidationOutput,
}

impl DeterministicValidationEvidence {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        Ok(canonical_json(&serde_json::to_value(self)?))
    }

    /// Deterministic validation is hard authority.  A command is acceptable
    /// only when it passed and observed one unchanged candidate/worktree state.
    pub fn authoritative_pass(&self, contract: CompletionContract) -> bool {
        let repository_unchanged = self.repository.before_head_oid
            == self.repository.after_head_oid
            && self.repository.before_tree_oid == self.repository.after_tree_oid
            && self.repository.before_status_digest == self.repository.after_status_digest
            && self.repository.before_candidate_content_digest
                == self.repository.after_candidate_content_digest;
        let clean_land = contract != CompletionContract::Land
            || self.repository.before_status_digest == ContentDigest::of_bytes(b"");
        self.exit.success
            && self.exit.code == Some(0)
            && self.exit.signal.is_none()
            && !self.exit.timed_out
            && repository_unchanged
            && clean_land
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ValidationCaptureError {
    #[error("validation command is empty or exceeds the {MAX_COMMAND_BYTES}-byte bound")]
    InvalidCommand,
    #[error("resolve validation lifecycle binding: {0}")]
    Lifecycle(String),
    #[error("resolve validation repository binding: {0}")]
    Repository(String),
    #[error("spawn deterministic validation: {0}")]
    Spawn(#[source] io::Error),
    #[error("read deterministic validation output: {0}")]
    Output(#[source] io::Error),
    #[error("wait for deterministic validation: {0}")]
    Wait(#[source] io::Error),
    #[error("validation output reader panicked")]
    ReaderPanic,
    #[error("persist deterministic validation authority: {0}")]
    Authority(String),
}

#[derive(Clone, Debug)]
struct RepositoryState {
    repository_identity: ContentDigest,
    worktree_identity: ContentDigest,
    cwd_identity: ContentDigest,
    cwd_relative: String,
    head_oid: String,
    tree_oid: String,
    integrated_main_oid: String,
    status_digest: ContentDigest,
    candidate_content_digest: ContentDigest,
}

/// Commands configured as deterministic completion authority.  The historical
/// singular `verify` field remains load-compatible; new tasks use
/// `validation_commands`. Empty values and exact duplicates are ignored.
pub fn configured_validation_commands(task: &Task) -> Vec<String> {
    let mut commands = Vec::new();
    let mut seen = BTreeSet::new();
    for command in task
        .verify
        .iter()
        .chain(task.validation_commands.iter())
        .map(|command| command.trim())
        .filter(|command| !command.is_empty())
    {
        if seen.insert(command.to_string()) {
            commands.push(command.to_string());
        }
    }
    commands
}

pub fn land_baseline_command() -> &'static str {
    "git diff --check refs/heads/main..HEAD"
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CompletionValidationCheck {
    pub purpose: String,
    pub command: String,
    pub provenance: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CompletionPreflight {
    pub checks: Vec<CompletionValidationCheck>,
    pub evidence_capture: String,
    pub prose_is_authority: bool,
    pub repair_boundary: CompletionRepairBoundary,
    pub deterministic_repair_budget: u32,
    pub boundary_explanation: String,
}

pub fn effective_repair_policy(task: &Task) -> CompletionRepairPolicy {
    task.completion_repair_policy.clone().unwrap_or_default()
}

/// Resolve the exact checks the ordinary completion controller will enforce.
/// This intentionally does not parse commands from task prose.
pub fn completion_preflight(task: &Task) -> CompletionPreflight {
    let mut checks = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(command) = task
        .verify
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        if seen.insert(command.to_string()) {
            checks.push(CompletionValidationCheck {
                purpose: "configured".into(),
                command: command.to_string(),
                provenance: "legacy task.verify (operator/repository-authorized)".into(),
            });
        }
    }
    for (index, command) in task.validation_commands.iter().enumerate() {
        let command = command.trim();
        if !command.is_empty() && seen.insert(command.to_string()) {
            checks.push(CompletionValidationCheck {
                purpose: "configured".into(),
                command: command.to_string(),
                provenance: format!(
                    "task.validation_commands[{index}] (operator/repository-authorized)"
                ),
            });
        }
    }
    if task.completion_contract == GraphCompletionContract::Land {
        checks.push(CompletionValidationCheck {
            purpose: "baseline".into(),
            command: land_baseline_command().into(),
            provenance: "completion contract: land (built-in, cannot be relaxed)".into(),
        });
    } else {
        checks.push(CompletionValidationCheck {
            purpose: "artifact-integrity".into(),
            command: "<WG regular-file and immutable-artifact integrity check>".into(),
            provenance: format!(
                "completion contract: {} (built-in)",
                task.completion_contract
            ),
        });
    }
    let policy = effective_repair_policy(task);
    let boundary_explanation = match policy.boundary {
        CompletionRepairBoundary::TaskOnly => {
            "repair only files directly required by this task; any gate-exposed fixture or unrelated production change needs operator approval"
        }
        CompletionRepairBoundary::TaskAndValidationFixtures => {
            "repair task implementation plus tests/fixtures directly exercised by the unchanged required checks; unrelated production behavior, gate reduction, or arbitrary repository cleanup needs operator approval"
        }
        CompletionRepairBoundary::Repository => {
            "operator explicitly approved repository-wide repair needed by the unchanged checks; security fences and gate reduction remain forbidden"
        }
    };
    CompletionPreflight {
        checks,
        evidence_capture: "wg done executes each command in the retained worktree and registers a host-bound immutable deterministic-validation/v1 evidence object before semantic review".into(),
        prose_is_authority: false,
        repair_boundary: policy.boundary,
        deterministic_repair_budget: policy.deterministic_repair_budget.max(1),
        boundary_explanation: boundary_explanation.into(),
    }
}

pub fn format_completion_preflight(task: &Task) -> String {
    let plan = completion_preflight(task);
    let mut lines = vec![
        "## Completion preflight (resolved before execution)".to_string(),
        "".to_string(),
        "Required deterministic checks, in enforced order:".to_string(),
    ];
    for (index, check) in plan.checks.iter().enumerate() {
        lines.push(format!(
            "{}. `{}` — provenance: {}",
            index + 1,
            check.command,
            check.provenance
        ));
    }
    lines.extend([
        format!("Evidence: {}.", plan.evidence_capture),
        "Commands merely mentioned in `## Validation` prose are criteria, not executable authority; propose a missing hard check with `wg fail TASK --intent request-contract-correction --reason <PROPOSAL>`. Only an operator-approved contract update adds it.".into(),
        format!(
            "Permitted repair boundary: `{}` — {}.",
            plan.repair_boundary, plan.boundary_explanation
        ),
        format!(
            "Deterministic repair budget: {} opportunities for this task episode. A failed known command returns evidence-backed feedback to this same attempt/session/worktree; change meaningful candidate bytes and rerun the unchanged `wg done`. Repeating unchanged bytes does not replenish the budget.",
            plan.deterministic_repair_budget
        ),
        "For scope ambiguity use `wg fail TASK --intent request-help --reason <ONE SPECIFIC DECISION>`. Intentional abandonment remains `wg fail TASK --intent deliberate-stop --reason <WHY>`.".into(),
    ]);
    lines.join("\n")
}

const COMPLETION_REPAIR_STATE_VERSION: u32 = 1;
const MAX_REPAIR_EXCERPT_CHARS: usize = 2_048;

fn validation_identity(evidence: &DeterministicValidationEvidence) -> ContentDigest {
    ContentDigest::of_bytes(&canonical_json(&serde_json::json!({
        "purpose": evidence.purpose,
        "configured_index": evidence.command.configured_index,
        "command_digest": evidence.command.command_digest,
        "requirements_digest": evidence.lifecycle.requirements_digest,
    })))
}

fn repair_candidate_identity(evidence: &DeterministicValidationEvidence) -> ContentDigest {
    ContentDigest::of_bytes(&canonical_json(&serde_json::json!({
        "repository": evidence.repository.repository_identity,
        "worktree": evidence.repository.worktree_identity,
        "head": evidence.repository.before_head_oid,
        "tree": evidence.repository.before_tree_oid,
        "status": evidence.repository.before_status_digest,
        "content": evidence.repository.before_candidate_content_digest,
        "requirements": evidence.lifecycle.requirements_digest,
    })))
}

fn validation_exit_category(evidence: &DeterministicValidationEvidence) -> &'static str {
    if evidence.exit.timed_out {
        "timeout"
    } else if evidence.exit.signal.is_some() {
        "signal"
    } else if evidence.exit.code.is_some_and(|code| code != 0) {
        "nonzero-exit"
    } else if evidence.repository.before_head_oid != evidence.repository.after_head_oid
        || evidence.repository.before_tree_oid != evidence.repository.after_tree_oid
        || evidence.repository.before_status_digest != evidence.repository.after_status_digest
        || evidence.repository.before_candidate_content_digest
            != evidence.repository.after_candidate_content_digest
    {
        "candidate-mutated-during-check"
    } else if !evidence.exit.success || evidence.exit.code != Some(0) {
        "unknown-exit"
    } else {
        "contract-precondition"
    }
}

fn diagnostic_excerpt(evidence: &DeterministicValidationEvidence) -> String {
    let raw = if !evidence.stderr.content.trim().is_empty() {
        &evidence.stderr.content
    } else {
        &evidence.stdout.content
    };
    let redacted = crate::chat_runtime::redact_text(raw.trim());
    let mut excerpt: String = redacted.chars().take(MAX_REPAIR_EXCERPT_CHARS).collect();
    if redacted.chars().count() > MAX_REPAIR_EXCERPT_CHARS {
        excerpt.push_str(" …[bounded]");
    }
    if excerpt.is_empty() {
        "<no diagnostic output; inspect immutable evidence>".into()
    } else {
        excerpt
    }
}

/// Record one failed, host-captured check against the exact live source tuple.
/// Unique candidate bytes consume the finite episode budget; unchanged replay
/// escalates deterministically without running a model.
pub fn record_deterministic_repair_failure(
    task: &mut Task,
    evidence: &DeterministicValidationEvidence,
    evidence_ref: &crate::completion_manifest::EvidenceRef,
) -> Result<CompletionRepairState, String> {
    if task.id != evidence.lifecycle.task_id
        || task.lifecycle.generation != evidence.lifecycle.generation
        || task.lifecycle.fence != evidence.lifecycle.attempt_fence
        || task
            .lifecycle
            .current_attempt
            .as_ref()
            .map(|attempt| attempt.id.as_str())
            != evidence.lifecycle.attempt_id.as_deref()
        || requirements_digest(task).ok().as_ref() != Some(&evidence.lifecycle.requirements_digest)
    {
        return Err("deterministic repair evidence does not bind the current source attempt/fence/requirements".into());
    }
    let policy = effective_repair_policy(task);
    let limit = policy.deterministic_repair_budget.max(1);
    let candidate_identity = repair_candidate_identity(evidence);
    let validation_identity = validation_identity(evidence);
    let mut failed_candidates = task
        .completion_repair
        .as_ref()
        .map(|state| state.failed_candidates.clone())
        .unwrap_or_default();
    let repeated = failed_candidates.contains(&candidate_identity);
    let exhausted = !repeated && failed_candidates.len() >= limit as usize;
    if !repeated && failed_candidates.len() < limit as usize + 1 {
        failed_candidates.push(candidate_identity.clone());
    }
    let opportunities_used = u32::try_from(failed_candidates.len())
        .unwrap_or(u32::MAX)
        .min(limit);
    let disposition = if repeated || exhausted {
        CompletionRepairDisposition::NeedsAttention
    } else {
        CompletionRepairDisposition::Repairing
    };
    let reason_code = if repeated {
        "unchanged-candidate-repeated"
    } else if exhausted {
        "deterministic-repair-budget-exhausted"
    } else {
        "deterministic-check-failed"
    };
    let safe_next = match disposition {
        CompletionRepairDisposition::Repairing => format!(
            "repair within `{}` in this same worktree/session, change meaningful candidate bytes, then rerun the unchanged `wg done {}`",
            policy.boundary, task.id
        ),
        CompletionRepairDisposition::NeedsAttention => {
            let next_limit = limit.saturating_add(1).min(100);
            if next_limit > limit {
                format!(
                    "operator: inspect evidence {} and either authorize one more changed candidate with `wg contract {} --deterministic-repair-budget {}` or deliberately stop with `wg fail {} --intent deliberate-stop --reason <WHY>`; do not relax the gate",
                    evidence_ref.content_digest, task.id, next_limit, task.id
                )
            } else {
                format!(
                    "operator: inspect evidence {} and deliberately stop with `wg fail {} --intent deliberate-stop --reason <WHY>` or approve an exact added check/scope correction; the repair ceiling cannot increase further",
                    evidence_ref.content_digest, task.id
                )
            }
        }
        CompletionRepairDisposition::Resolved => {
            unreachable!("failure cannot create a resolved repair state")
        }
    };
    let feedback_id = format!(
        "b3:{}",
        blake3::hash(&canonical_json(&serde_json::json!({
            "task": task.id,
            "attempt": evidence.lifecycle.attempt_id,
            "fence": evidence.lifecycle.attempt_fence,
            "validation": validation_identity,
            "candidate": candidate_identity,
            "reason": reason_code,
        })))
        .to_hex()
    );
    let previous_attention = task
        .completion_repair
        .as_ref()
        .and_then(|state| state.attention_event_id.clone());
    let attention_event_id = (disposition == CompletionRepairDisposition::NeedsAttention)
        .then(|| previous_attention.unwrap_or_else(|| format!("attention:{feedback_id}")));
    let state = CompletionRepairState {
        version: COMPLETION_REPAIR_STATE_VERSION,
        disposition,
        task_id: task.id.clone(),
        generation: evidence.lifecycle.generation,
        attempt_id: evidence.lifecycle.attempt_id.clone(),
        fence: evidence.lifecycle.attempt_fence,
        requirements_digest: evidence.lifecycle.requirements_digest.clone(),
        validation_identity,
        candidate_identity,
        evidence: evidence_ref.clone(),
        command: evidence
            .command
            .argv
            .last()
            .cloned()
            .unwrap_or_else(|| "<missing command>".into()),
        exit_category: validation_exit_category(evidence).into(),
        diagnostic_excerpt: diagnostic_excerpt(evidence),
        opportunities_used,
        opportunity_limit: limit,
        failed_candidates,
        reason_code: reason_code.into(),
        safe_next,
        feedback_id,
        attention_event_id,
        updated_at: Utc::now().to_rfc3339(),
    };
    task.completion_repair = Some(state.clone());
    Ok(state)
}

pub fn request_repair_attention(
    task: &mut Task,
    reason_code: &str,
    safe_next: String,
) -> Result<bool, String> {
    let state = task
        .completion_repair
        .as_mut()
        .ok_or_else(|| "no evidence-backed deterministic repair is active".to_string())?;
    if state.task_id != task.id
        || state.generation != task.lifecycle.generation
        || state.fence != task.lifecycle.fence
        || state.attempt_id.as_deref()
            != task
                .lifecycle
                .current_attempt
                .as_ref()
                .map(|attempt| attempt.id.as_str())
    {
        return Err("repair request is stale for the current source attempt/fence".into());
    }
    let event_id = format!("attention:{}:{reason_code}", state.feedback_id);
    let changed = state.disposition != CompletionRepairDisposition::NeedsAttention
        || state.reason_code != reason_code
        || state.attention_event_id.as_deref() != Some(event_id.as_str());
    state.disposition = CompletionRepairDisposition::NeedsAttention;
    state.reason_code = reason_code.into();
    state.safe_next = safe_next;
    state.attention_event_id = Some(event_id);
    state.updated_at = Utc::now().to_rfc3339();
    Ok(changed)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StalledChain {
    pub root_task_id: String,
    pub root_blocker: String,
    pub active_repair: bool,
    pub affected_downstream: Vec<String>,
    pub safe_operator_action: String,
    pub attention_event_id: String,
}

/// Deterministic, cycle-safe read projection. Only explicit completion repair
/// attention is a root; active repair and ordinary bounded waits are not stalls.
pub fn stalled_chains(graph: &WorkGraph) -> Vec<StalledChain> {
    let mut reverse: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for task in graph.tasks() {
        for predecessor in &task.after {
            reverse
                .entry(predecessor.clone())
                .or_default()
                .push(task.id.clone());
        }
    }
    for children in reverse.values_mut() {
        children.sort();
        children.dedup();
    }
    let mut chains = Vec::new();
    for root in graph.tasks() {
        let Some(repair) = root.completion_repair.as_ref() else {
            continue;
        };
        if repair.disposition != CompletionRepairDisposition::NeedsAttention
            || root.status == Status::Done
        {
            continue;
        }
        // Nodes that are also transitive predecessors of the root are in its
        // structural SCC. The cycle itself is supported graph structure, not
        // downstream impact evidence, so traverse through it but do not list it.
        let mut ancestors = BTreeSet::new();
        let mut ancestor_queue = std::collections::VecDeque::from([root.id.clone()]);
        while let Some(current) = ancestor_queue.pop_front() {
            let Some(task) = graph.get_task(&current) else {
                continue;
            };
            for predecessor in &task.after {
                if ancestors.insert(predecessor.clone()) {
                    ancestor_queue.push_back(predecessor.clone());
                }
            }
        }
        let mut visited = BTreeSet::from([root.id.clone()]);
        let mut affected = BTreeSet::new();
        let mut queue = std::collections::VecDeque::from([root.id.clone()]);
        while let Some(current) = queue.pop_front() {
            for child in reverse.get(&current).into_iter().flatten() {
                if !visited.insert(child.clone()) {
                    continue;
                }
                let Some(child_task) = graph.get_task(child) else {
                    continue;
                };
                if ancestors.contains(child) {
                    queue.push_back(child.clone());
                    continue;
                }
                // Active work, a bounded wait, completion in progress, and
                // terminal/satisfied work are not stalled by this root.
                if child_task.assigned.is_none()
                    && matches!(
                        child_task.status,
                        Status::Open | Status::Blocked | Status::Incomplete
                    )
                {
                    affected.insert(child.clone());
                    queue.push_back(child.clone());
                }
            }
        }
        chains.push(StalledChain {
            root_task_id: root.id.clone(),
            root_blocker: format!("{}: {}", repair.reason_code, repair.exit_category),
            active_repair: false,
            affected_downstream: affected.into_iter().collect(),
            safe_operator_action: repair.safe_next.clone(),
            attention_event_id: repair
                .attention_event_id
                .clone()
                .unwrap_or_else(|| format!("attention:{}", repair.feedback_id)),
        });
    }
    chains.sort_by(|a, b| a.root_task_id.cmp(&b.root_task_id));
    chains
}

#[derive(Serialize)]
struct ValidationCaptureAuthority<'a> {
    authority_version: u32,
    evidence_digest: &'a ContentDigest,
    task_id: &'a str,
    requirements_digest: &'a ContentDigest,
    generation: u64,
    attempt_id: &'a Option<String>,
    attempt_fence: u64,
    command_digest: &'a ContentDigest,
    repository_identity: &'a ContentDigest,
    source_revision: &'a str,
    source_tree: &'a str,
}

fn capture_authority_bytes(
    evidence_digest: &ContentDigest,
    evidence: &DeterministicValidationEvidence,
) -> Vec<u8> {
    canonical_json(
        &serde_json::to_value(ValidationCaptureAuthority {
            authority_version: VALIDATION_AUTHORITY_VERSION,
            evidence_digest,
            task_id: &evidence.lifecycle.task_id,
            requirements_digest: &evidence.lifecycle.requirements_digest,
            generation: evidence.lifecycle.generation,
            attempt_id: &evidence.lifecycle.attempt_id,
            attempt_fence: evidence.lifecycle.attempt_fence,
            command_digest: &evidence.command.command_digest,
            repository_identity: &evidence.repository.repository_identity,
            source_revision: &evidence.repository.before_head_oid,
            source_tree: &evidence.repository.before_tree_oid,
        })
        .expect("validation authority serializes"),
    )
}

/// Record that WG itself captured an evidence object. The marker lives in the
/// protected control plane, is create-once, and is re-derived from the exact
/// CAS object on every review. Diagnostic `completion-object` calls cannot mint
/// this authority merely by copying worker-authored JSON.
pub fn register_capture_authority(
    workgraph_dir: &Path,
    evidence_digest: &ContentDigest,
    evidence: &DeterministicValidationEvidence,
) -> Result<(), ValidationCaptureError> {
    let evidence_bytes = evidence
        .canonical_bytes()
        .map_err(|error| ValidationCaptureError::Authority(error.to_string()))?;
    if ContentDigest::of_bytes(&evidence_bytes) != *evidence_digest {
        return Err(ValidationCaptureError::Authority(
            "evidence digest does not name the canonical capture".into(),
        ));
    }
    let root = workgraph_dir.join(VALIDATION_AUTHORITY_DIR);
    reject_authority_symlink(&root)?;
    fs::create_dir_all(&root)
        .map_err(|error| ValidationCaptureError::Authority(error.to_string()))?;
    let name = evidence_digest
        .as_str()
        .strip_prefix("b3:")
        .expect("validated digest");
    let path = root.join(name);
    reject_authority_symlink(&path)?;
    let bytes = capture_authority_bytes(evidence_digest, evidence);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|error| ValidationCaptureError::Authority(error.to_string()))?;
            File::open(&root)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| ValidationCaptureError::Authority(error.to_string()))?;
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let existing = fs::read(&path)
                .map_err(|error| ValidationCaptureError::Authority(error.to_string()))?;
            if existing == bytes {
                Ok(())
            } else {
                Err(ValidationCaptureError::Authority(
                    "existing create-once authority marker differs".into(),
                ))
            }
        }
        Err(error) => Err(ValidationCaptureError::Authority(error.to_string())),
    }
}

fn reject_authority_symlink(path: &Path) -> Result<(), ValidationCaptureError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(
            ValidationCaptureError::Authority(format!("symlink refused at {}", path.display())),
        ),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ValidationCaptureError::Authority(error.to_string())),
    }
}

/// Execute one validation command with bounded stream capture.  Non-zero and
/// timed-out commands still return an evidence envelope so callers can retain
/// and surface the exact failure; only host/capture failures return `Err`.
pub fn capture_validation(
    task: &Task,
    command: &str,
    configured_index: u32,
    purpose: ValidationPurpose,
    cwd: &Path,
) -> Result<DeterministicValidationEvidence, ValidationCaptureError> {
    let command = command.trim();
    if command.is_empty() || command.len() > MAX_COMMAND_BYTES {
        return Err(ValidationCaptureError::InvalidCommand);
    }
    let before = repository_state(cwd).map_err(ValidationCaptureError::Repository)?;
    let requirements_digest = requirements_digest(task)
        .map_err(|error| ValidationCaptureError::Lifecycle(error.to_string()))?;
    let command_identity = command_identity(command, configured_index);
    let started = Utc::now();
    let started_instant = Instant::now();
    let mut process = Command::new("bash");
    process
        .args(["-lc", command])
        .current_dir(cwd)
        .env("TERM", "dumb")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // A separate process group lets timeout kill descendants that still
        // hold the capture pipes, keeping wall time bounded as well as output.
        process.process_group(0);
    }
    let mut child = process.spawn().map_err(ValidationCaptureError::Spawn)?;
    let stdout = child
        .stdout
        .take()
        .expect("piped stdout exists for validation capture");
    let stderr = child
        .stderr
        .take()
        .expect("piped stderr exists for validation capture");
    let stdout_reader = std::thread::spawn(move || read_bounded(stdout));
    let stderr_reader = std::thread::spawn(move || read_bounded(stderr));

    let timeout = validation_timeout(task);
    let (status, timed_out) = loop {
        match child.try_wait().map_err(ValidationCaptureError::Wait)? {
            Some(status) => break (status, false),
            None if started_instant.elapsed() >= timeout => {
                terminate_validation_process(&mut child);
                let status = child.wait().map_err(ValidationCaptureError::Wait)?;
                break (status, true);
            }
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    };
    let duration_ms = u64::try_from(started_instant.elapsed().as_millis()).unwrap_or(u64::MAX);
    let finished = Utc::now();
    let stdout = stdout_reader
        .join()
        .map_err(|_| ValidationCaptureError::ReaderPanic)?
        .map_err(ValidationCaptureError::Output)?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| ValidationCaptureError::ReaderPanic)?
        .map_err(ValidationCaptureError::Output)?;
    let after = repository_state(cwd).map_err(ValidationCaptureError::Repository)?;

    Ok(DeterministicValidationEvidence {
        evidence_version: DETERMINISTIC_VALIDATION_VERSION,
        capture_origin: "wg_done".to_string(),
        purpose,
        command: command_identity,
        lifecycle: ValidationLifecycleBinding {
            task_id: task.id.clone(),
            generation: task.lifecycle.generation,
            attempt_id: task
                .lifecycle
                .current_attempt
                .as_ref()
                .map(|attempt| attempt.id.clone()),
            attempt_fence: task.lifecycle.fence,
            requirements_digest,
        },
        repository: ValidationRepositoryBinding {
            repository_identity: before.repository_identity,
            worktree_identity: before.worktree_identity,
            cwd_identity: before.cwd_identity,
            cwd_relative: before.cwd_relative,
            before_head_oid: before.head_oid,
            after_head_oid: after.head_oid,
            before_tree_oid: before.tree_oid,
            after_tree_oid: after.tree_oid,
            integrated_main_oid: before.integrated_main_oid,
            before_status_digest: before.status_digest,
            after_status_digest: after.status_digest,
            before_candidate_content_digest: Some(before.candidate_content_digest),
            after_candidate_content_digest: Some(after.candidate_content_digest),
        },
        started_at: started.to_rfc3339(),
        finished_at: finished.to_rfc3339(),
        duration_ms,
        exit: exit_status(status, timed_out),
        stdout,
        stderr,
    })
}

/// Capture an in-process deterministic completion check against the same
/// lifecycle/repository binding used by shell validation. This is used for the
/// existing owned smoke gate, whose Rust subreaper harness cannot safely be
/// represented as (or re-run through) an arbitrary shell command.
pub fn capture_validation_operation<F>(
    task: &Task,
    label: &str,
    configured_index: u32,
    purpose: ValidationPurpose,
    cwd: &Path,
    operation: F,
) -> Result<DeterministicValidationEvidence, ValidationCaptureError>
where
    F: FnOnce() -> Result<(), String>,
{
    if label.is_empty() || label.len() > MAX_COMMAND_BYTES {
        return Err(ValidationCaptureError::InvalidCommand);
    }
    let before = repository_state(cwd).map_err(ValidationCaptureError::Repository)?;
    let requirements_digest = requirements_digest(task)
        .map_err(|error| ValidationCaptureError::Lifecycle(error.to_string()))?;
    let started = Utc::now();
    let started_instant = Instant::now();
    let result = operation();
    let duration_ms = u64::try_from(started_instant.elapsed().as_millis()).unwrap_or(u64::MAX);
    let finished = Utc::now();
    let after = repository_state(cwd).map_err(ValidationCaptureError::Repository)?;
    let (exit, stderr) = match result {
        Ok(()) => (
            ValidationExitStatus {
                success: true,
                code: Some(0),
                signal: None,
                timed_out: false,
            },
            bounded_bytes(&[]),
        ),
        Err(error) => (
            ValidationExitStatus {
                success: false,
                code: Some(1),
                signal: None,
                timed_out: false,
            },
            bounded_bytes(error.as_bytes()),
        ),
    };
    Ok(DeterministicValidationEvidence {
        evidence_version: DETERMINISTIC_VALIDATION_VERSION,
        capture_origin: "wg_done_owned_smoke".to_string(),
        purpose,
        command: command_identity(label, configured_index),
        lifecycle: ValidationLifecycleBinding {
            task_id: task.id.clone(),
            generation: task.lifecycle.generation,
            attempt_id: task
                .lifecycle
                .current_attempt
                .as_ref()
                .map(|attempt| attempt.id.clone()),
            attempt_fence: task.lifecycle.fence,
            requirements_digest,
        },
        repository: ValidationRepositoryBinding {
            repository_identity: before.repository_identity,
            worktree_identity: before.worktree_identity,
            cwd_identity: before.cwd_identity,
            cwd_relative: before.cwd_relative,
            before_head_oid: before.head_oid,
            after_head_oid: after.head_oid,
            before_tree_oid: before.tree_oid,
            after_tree_oid: after.tree_oid,
            integrated_main_oid: before.integrated_main_oid,
            before_status_digest: before.status_digest,
            after_status_digest: after.status_digest,
            before_candidate_content_digest: Some(before.candidate_content_digest),
            after_candidate_content_digest: Some(after.candidate_content_digest),
        },
        started_at: started.to_rfc3339(),
        finished_at: finished.to_rfc3339(),
        duration_ms,
        exit,
        stdout: bounded_bytes(&[]),
        stderr,
    })
}

fn bounded_bytes(bytes: &[u8]) -> BoundedValidationOutput {
    let captured = &bytes[..bytes.len().min(MAX_CAPTURE_BYTES_PER_STREAM)];
    let (encoding, content) = match String::from_utf8(captured.to_vec()) {
        Ok(text) => ("utf-8".to_string(), text),
        Err(_) => ("hex".to_string(), hex::encode(captured)),
    };
    BoundedValidationOutput {
        digest: ContentDigest::of_bytes(bytes),
        total_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        captured_bytes: u64::try_from(captured.len()).unwrap_or(u64::MAX),
        truncated: captured.len() < bytes.len(),
        encoding,
        content,
    }
}

fn validation_timeout(task: &Task) -> Duration {
    let seconds = task
        .verify_timeout
        .as_deref()
        .and_then(parse_delay)
        .or_else(|| {
            std::env::var("WG_VERIFY_TIMEOUT")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
        })
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(1, MAX_TIMEOUT_SECS);
    Duration::from_secs(seconds)
}

fn command_identity(command: &str, configured_index: u32) -> ValidationCommandIdentity {
    let shell = "bash".to_string();
    let argv = vec!["-lc".to_string(), command.to_string()];
    let bytes = canonical_json(&serde_json::json!({
        "configured_index": configured_index,
        "shell": shell,
        "argv": argv,
    }));
    ValidationCommandIdentity {
        configured_index,
        shell: "bash".to_string(),
        argv: vec!["-lc".to_string(), command.to_string()],
        command_digest: ContentDigest::of_bytes(&bytes),
    }
}

fn read_bounded(mut reader: impl Read) -> io::Result<BoundedValidationOutput> {
    let mut hasher = blake3::Hasher::new();
    let mut captured = Vec::with_capacity(MAX_CAPTURE_BYTES_PER_STREAM);
    let mut total = 0_u64;
    let mut buffer = [0_u8; 8192];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total = total.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        let remaining = MAX_CAPTURE_BYTES_PER_STREAM.saturating_sub(captured.len());
        captured.extend_from_slice(&buffer[..read.min(remaining)]);
    }
    let digest = ContentDigest::parse(format!("b3:{}", hasher.finalize().to_hex()))
        .expect("BLAKE3 digest has canonical shape");
    let (encoding, content) = match String::from_utf8(captured.clone()) {
        Ok(text) => ("utf-8".to_string(), text),
        Err(_) => ("hex".to_string(), hex::encode(&captured)),
    };
    Ok(BoundedValidationOutput {
        digest,
        total_bytes: total,
        captured_bytes: u64::try_from(captured.len()).unwrap_or(u64::MAX),
        truncated: total > u64::try_from(captured.len()).unwrap_or(u64::MAX),
        encoding,
        content,
    })
}

fn repository_state(cwd: &Path) -> Result<RepositoryState, String> {
    let cwd = cwd
        .canonicalize()
        .map_err(|error| format!("canonicalize cwd {}: {error}", cwd.display()))?;
    let worktree = PathBuf::from(git(&cwd, &["rev-parse", "--show-toplevel"])?);
    let worktree = worktree
        .canonicalize()
        .map_err(|error| format!("canonicalize worktree {}: {error}", worktree.display()))?;
    let common = PathBuf::from(git(&cwd, &["rev-parse", "--git-common-dir"])?);
    let common = if common.is_absolute() {
        common
    } else {
        cwd.join(common)
    };
    let common = common
        .canonicalize()
        .map_err(|error| format!("canonicalize Git common dir {}: {error}", common.display()))?;
    let cwd_relative = cwd
        .strip_prefix(&worktree)
        .map_err(|_| "validation cwd is outside the Git worktree".to_string())?;
    let cwd_relative = if cwd_relative.as_os_str().is_empty() {
        ".".to_string()
    } else {
        cwd_relative.to_string_lossy().replace('\\', "/")
    };
    let raw_status = git_bytes(&cwd, &["status", "--porcelain=v1", "--untracked-files=all"])?;
    let status = normalized_status(&raw_status);
    let candidate_content_digest = repository_candidate_content_digest(&worktree)?;
    Ok(RepositoryState {
        repository_identity: ContentDigest::of_bytes(common.as_os_str().as_encoded_bytes()),
        worktree_identity: ContentDigest::of_bytes(worktree.as_os_str().as_encoded_bytes()),
        cwd_identity: ContentDigest::of_bytes(cwd.as_os_str().as_encoded_bytes()),
        cwd_relative,
        head_oid: git(&cwd, &["rev-parse", "HEAD"])?,
        tree_oid: git(&cwd, &["rev-parse", "HEAD^{tree}"])?,
        integrated_main_oid: git(&cwd, &["rev-parse", "refs/heads/main"])?,
        status_digest: ContentDigest::of_bytes(&status),
        candidate_content_digest,
    })
}

fn normalized_status(raw: &[u8]) -> Vec<u8> {
    let mut normalized = Vec::new();
    for line in String::from_utf8_lossy(raw).lines() {
        let path = line.get(3..).unwrap_or(line).trim();
        if path == ".wg-cleanup-pending"
            || path == ".wg"
            || path.starts_with(".wg/")
            || path.starts_with(".workgraph/")
        {
            continue;
        }
        normalized.extend_from_slice(line.as_bytes());
        normalized.push(b'\n');
    }
    normalized
}

fn repository_candidate_content_digest(worktree: &Path) -> Result<ContentDigest, String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"worksgood-candidate-content/v1\0");
    let tracked = git_bytes(
        worktree,
        &[
            "diff",
            "--no-ext-diff",
            "--binary",
            "--full-index",
            "HEAD",
            "--",
        ],
    )?;
    hasher.update(&(tracked.len() as u64).to_le_bytes());
    hasher.update(&tracked);

    let untracked = git_bytes(
        worktree,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    for raw_path in untracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let path = git_path_from_bytes(raw_path);
        let normalized = path.to_string_lossy().replace('\\', "/");
        if normalized == ".wg-cleanup-pending"
            || normalized == ".wg"
            || normalized.starts_with(".wg/")
            || normalized.starts_with(".workgraph/")
        {
            continue;
        }
        hasher.update(&(raw_path.len() as u64).to_le_bytes());
        hasher.update(raw_path);
        let full_path = worktree.join(&path);
        let metadata = fs::symlink_metadata(&full_path)
            .map_err(|error| format!("inspect candidate {}: {error}", full_path.display()))?;
        if metadata.file_type().is_symlink() {
            hasher.update(b"symlink\0");
            let target = fs::read_link(&full_path).map_err(|error| {
                format!("read candidate symlink {}: {error}", full_path.display())
            })?;
            hasher.update(target.as_os_str().as_encoded_bytes());
        } else if metadata.is_file() {
            hasher.update(b"file\0");
            hasher.update(&metadata.len().to_le_bytes());
            let mut file = File::open(&full_path)
                .map_err(|error| format!("read candidate {}: {error}", full_path.display()))?;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let read = file
                    .read(&mut buffer)
                    .map_err(|error| format!("read candidate {}: {error}", full_path.display()))?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
        } else {
            // Never block on FIFOs/devices. Their presence and path still bind
            // the candidate, while deterministic validation remains fail closed.
            hasher.update(b"special\0");
        }
    }
    ContentDigest::parse(format!("b3:{}", hasher.finalize().to_hex()))
}

#[cfg(unix)]
fn git_path_from_bytes(raw: &[u8]) -> PathBuf {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    PathBuf::from(OsString::from_vec(raw.to_vec()))
}

#[cfg(not(unix))]
fn git_path_from_bytes(raw: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(raw).into_owned())
}

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let bytes = git_bytes(cwd, args)?;
    String::from_utf8(bytes)
        .map(|value| value.trim().to_string())
        .map_err(|error| format!("git {} returned non-UTF-8 output: {error}", args.join(" ")))
}

fn git_bytes(cwd: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|error| format!("run git {}: {error}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

#[cfg(unix)]
fn terminate_validation_process(child: &mut Child) {
    let process_group = i32::try_from(child.id()).unwrap_or(i32::MAX);
    // SAFETY: `process_group(0)` above made the child PID its process-group
    // leader. A negative PID addresses only that isolated group.
    unsafe {
        libc::kill(-process_group, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn terminate_validation_process(child: &mut Child) {
    let _ = child.kill();
}

fn exit_status(status: ExitStatus, timed_out: bool) -> ValidationExitStatus {
    ValidationExitStatus {
        success: status.success() && !timed_out,
        code: status.code(),
        signal: exit_signal(&status),
        timed_out,
    }
}

#[cfg(unix)]
fn exit_signal(status: &ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

#[cfg(not(unix))]
fn exit_signal(_status: &ExitStatus) -> Option<i32> {
    None
}

/// Convert exact captured evidence into the hard validation predicate used by
/// both review admission and publication-derived Done. Generic historical
/// evidence remains resolvable, but a task with configured deterministic
/// commands must supply one exact host-captured pass for every command.
pub fn verify_validation_evidence(
    task: &Task,
    manifest: &CompletionManifest,
    binding: Option<&CompletionReviewBinding>,
    bundle: &ResolvedReviewBundle,
    repository_root: &Path,
    workgraph_dir: &Path,
) -> Result<(), IncompleteEvidence> {
    let expected_commands = configured_validation_commands(task);
    let structured = bundle
        .validation_evidence
        .iter()
        .filter(|evidence| {
            evidence
                .evidence_kind
                .starts_with(DETERMINISTIC_VALIDATION_PREFIX)
        })
        .collect::<Vec<_>>();
    if structured.is_empty() {
        if expected_commands.is_empty() {
            // Historical diagnostic manifests may carry generic immutable
            // evidence. They remain load/review compatible, but can never
            // satisfy a newly configured deterministic command.
            return Ok(());
        }
        return Err(incomplete(
            IncompleteEvidenceKind::Missing,
            "deterministic validation evidence",
            format!(
                "{} configured validation command(s) have no host-captured result",
                expected_commands.len()
            ),
        ));
    }

    let expected_repository_identity = live_repository_identity(repository_root)?;
    let mut configured = BTreeMap::new();
    let mut baseline_count = 0_usize;
    for evidence in &structured {
        let parsed: DeterministicValidationEvidence =
            serde_json::from_slice(&evidence.payload.bytes).map_err(|error| {
                incomplete(
                    IncompleteEvidenceKind::InvalidManifest,
                    evidence.evidence_kind.clone(),
                    format!("deterministic validation envelope is invalid: {error}"),
                )
            })?;
        verify_capture_authority(workgraph_dir, &evidence.payload.source_digest, &parsed)?;
        verify_one(
            task,
            manifest,
            binding,
            &parsed,
            &expected_repository_identity,
        )?;
        match parsed.purpose {
            ValidationPurpose::Configured => {
                if evidence.evidence_kind != CONFIGURED_VALIDATION_EVIDENCE_KIND {
                    return Err(incomplete(
                        IncompleteEvidenceKind::InvalidManifest,
                        evidence.evidence_kind.clone(),
                        "configured validation purpose uses the wrong evidence kind",
                    ));
                }
                if configured
                    .insert(parsed.command.configured_index, parsed)
                    .is_some()
                {
                    return Err(incomplete(
                        IncompleteEvidenceKind::InvalidManifest,
                        "deterministic validation evidence",
                        "duplicate configured validation index",
                    ));
                }
            }
            ValidationPurpose::Baseline => {
                if evidence.evidence_kind != BASELINE_VALIDATION_EVIDENCE_KIND {
                    return Err(incomplete(
                        IncompleteEvidenceKind::InvalidManifest,
                        evidence.evidence_kind.clone(),
                        "baseline validation purpose uses the wrong evidence kind",
                    ));
                }
                baseline_count += 1;
                if manifest.completion_contract == CompletionContract::Land
                    && parsed.command.argv.get(1).map(String::as_str)
                        != Some(land_baseline_command())
                {
                    return Err(incomplete(
                        IncompleteEvidenceKind::InvalidManifest,
                        "baseline deterministic validation",
                        "Land baseline command identity is not the configured WG integrity check",
                    ));
                }
            }
        }
    }

    for (index, command) in expected_commands.iter().enumerate() {
        let index = u32::try_from(index).unwrap_or(u32::MAX);
        let Some(actual) = configured.get(&index) else {
            return Err(incomplete(
                IncompleteEvidenceKind::Missing,
                format!("configured validation {index}"),
                "host-captured command result is missing",
            ));
        };
        if actual.command != command_identity(command, index) {
            return Err(incomplete(
                IncompleteEvidenceKind::DigestMismatch,
                format!("configured validation {index}"),
                "captured command identity does not match the immutable task configuration",
            ));
        }
    }
    if configured.len() != expected_commands.len() {
        return Err(incomplete(
            IncompleteEvidenceKind::InvalidManifest,
            "deterministic validation evidence",
            "manifest contains an unconfigured validation command",
        ));
    }
    if manifest.completion_contract == CompletionContract::Land
        && !structured.is_empty()
        && baseline_count != 1
    {
        return Err(incomplete(
            IncompleteEvidenceKind::Missing,
            "baseline deterministic validation",
            "Land completion requires exactly one host-captured git diff baseline",
        ));
    }
    Ok(())
}

fn verify_one(
    task: &Task,
    manifest: &CompletionManifest,
    binding: Option<&CompletionReviewBinding>,
    evidence: &DeterministicValidationEvidence,
    expected_repository_identity: &ContentDigest,
) -> Result<(), IncompleteEvidence> {
    if evidence.evidence_version != DETERMINISTIC_VALIDATION_VERSION
        || evidence.capture_origin != "wg_done"
    {
        return Err(incomplete(
            IncompleteEvidenceKind::InvalidManifest,
            "deterministic validation evidence",
            "unsupported validation evidence version or capture origin",
        ));
    }
    let expected_attempt_id = binding
        .and_then(|value| value.attempt_id.as_deref())
        .or_else(|| {
            task.lifecycle
                .current_attempt
                .as_ref()
                .map(|attempt| attempt.id.as_str())
        });
    let expected_fence = binding
        .map(|value| value.attempt_fence)
        .unwrap_or(task.lifecycle.fence);
    if evidence.lifecycle.task_id != task.id
        || evidence.lifecycle.generation != manifest.generation
        || evidence.lifecycle.generation != task.lifecycle.generation
        || evidence.lifecycle.attempt_id.as_deref() != expected_attempt_id
        || evidence.lifecycle.attempt_fence != expected_fence
        || evidence.lifecycle.requirements_digest != manifest.requirements_digest
    {
        return Err(incomplete(
            IncompleteEvidenceKind::DigestMismatch,
            "deterministic validation lifecycle binding",
            "evidence does not bind the reviewed task/requirements/generation/attempt/fence",
        ));
    }
    if evidence.repository.repository_identity != *expected_repository_identity
        || evidence.repository.before_head_oid != manifest.source_revision
    {
        return Err(incomplete(
            IncompleteEvidenceKind::GitObjectMismatch,
            "deterministic validation repository binding",
            "evidence was captured in a different repository or source revision",
        ));
    }
    if let Some(git) = manifest.outputs.iter().find_map(|output| match output {
        OutputRef::Git(git) => Some(git),
        OutputRef::Artifact(_) | OutputRef::External(_) => None,
    }) && (evidence.repository.before_head_oid != git.commit_oid
        || evidence.repository.before_tree_oid != git.tree_oid
        || evidence.repository.integrated_main_oid != git.integrated_main_oid)
    {
        return Err(incomplete(
            IncompleteEvidenceKind::GitObjectMismatch,
            "deterministic validation candidate binding",
            "evidence does not bind the reviewed Git commit/tree/base",
        ));
    }
    if !evidence.authoritative_pass(manifest.completion_contract) {
        return Err(incomplete(
            IncompleteEvidenceKind::DigestMismatch,
            "deterministic validation result",
            format!(
                "command did not pass on one unchanged candidate (exit={:?}, signal={:?}, timeout={})",
                evidence.exit.code, evidence.exit.signal, evidence.exit.timed_out
            ),
        ));
    }
    verify_timing(evidence)?;
    verify_output_shape("stdout", &evidence.stdout)?;
    verify_output_shape("stderr", &evidence.stderr)?;
    if evidence.command
        != command_identity(
            evidence
                .command
                .argv
                .get(1)
                .map(String::as_str)
                .unwrap_or(""),
            evidence.command.configured_index,
        )
    {
        return Err(incomplete(
            IncompleteEvidenceKind::DigestMismatch,
            "deterministic validation command identity",
            "command identity digest is inconsistent with its shell argv",
        ));
    }
    Ok(())
}

fn verify_timing(evidence: &DeterministicValidationEvidence) -> Result<(), IncompleteEvidence> {
    let started = DateTime::parse_from_rfc3339(&evidence.started_at).map_err(|error| {
        incomplete(
            IncompleteEvidenceKind::InvalidManifest,
            "deterministic validation timing",
            format!("invalid started_at: {error}"),
        )
    })?;
    let finished = DateTime::parse_from_rfc3339(&evidence.finished_at).map_err(|error| {
        incomplete(
            IncompleteEvidenceKind::InvalidManifest,
            "deterministic validation timing",
            format!("invalid finished_at: {error}"),
        )
    })?;
    if finished < started || evidence.duration_ms > MAX_TIMEOUT_SECS.saturating_mul(1000) + 5_000 {
        return Err(incomplete(
            IncompleteEvidenceKind::InvalidManifest,
            "deterministic validation timing",
            "timing is negative or exceeds the bounded command deadline",
        ));
    }
    Ok(())
}

fn verify_output_shape(
    name: &str,
    output: &BoundedValidationOutput,
) -> Result<(), IncompleteEvidence> {
    if output.captured_bytes > MAX_CAPTURE_BYTES_PER_STREAM as u64
        || output.captured_bytes > output.total_bytes
        || output.truncated != (output.total_bytes > output.captured_bytes)
        || !matches!(output.encoding.as_str(), "utf-8" | "hex")
    {
        return Err(incomplete(
            IncompleteEvidenceKind::InvalidManifest,
            format!("deterministic validation {name}"),
            "bounded stream metadata is inconsistent",
        ));
    }
    let captured = match output.encoding.as_str() {
        "utf-8" => output.content.as_bytes().to_vec(),
        "hex" => hex::decode(&output.content).map_err(|error| {
            incomplete(
                IncompleteEvidenceKind::InvalidManifest,
                format!("deterministic validation {name}"),
                format!("invalid hex capture: {error}"),
            )
        })?,
        _ => unreachable!(),
    };
    if captured.len() as u64 != output.captured_bytes {
        return Err(incomplete(
            IncompleteEvidenceKind::SizeMismatch,
            format!("deterministic validation {name}"),
            "captured stream byte count does not match its content",
        ));
    }
    if !output.truncated && ContentDigest::of_bytes(&captured) != output.digest {
        return Err(incomplete(
            IncompleteEvidenceKind::DigestMismatch,
            format!("deterministic validation {name}"),
            "complete captured stream does not match its digest",
        ));
    }
    Ok(())
}

pub fn verify_capture_authority(
    workgraph_dir: &Path,
    evidence_digest: &ContentDigest,
    evidence: &DeterministicValidationEvidence,
) -> Result<(), IncompleteEvidence> {
    let name = evidence_digest
        .as_str()
        .strip_prefix("b3:")
        .expect("validated digest");
    let path = workgraph_dir.join(VALIDATION_AUTHORITY_DIR).join(name);
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        incomplete(
            IncompleteEvidenceKind::Missing,
            "deterministic validation capture authority",
            format!("{}: {error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 4096 {
        return Err(incomplete(
            IncompleteEvidenceKind::ProtectedControlPlane,
            "deterministic validation capture authority",
            "authority marker is not a bounded regular file",
        ));
    }
    let actual = fs::read(&path).map_err(|error| {
        incomplete(
            IncompleteEvidenceKind::Inaccessible,
            "deterministic validation capture authority",
            error.to_string(),
        )
    })?;
    let expected = capture_authority_bytes(evidence_digest, evidence);
    if actual != expected {
        return Err(incomplete(
            IncompleteEvidenceKind::DigestMismatch,
            "deterministic validation capture authority",
            "protected create-once authority does not match the exact evidence object",
        ));
    }
    Ok(())
}

fn live_repository_identity(repository_root: &Path) -> Result<ContentDigest, IncompleteEvidence> {
    let root = repository_root.canonicalize().map_err(|error| {
        incomplete(
            IncompleteEvidenceKind::Inaccessible,
            "review repository",
            format!("canonicalize repository: {error}"),
        )
    })?;
    let common = PathBuf::from(git(&root, &["rev-parse", "--git-common-dir"]).map_err(
        |detail| {
            incomplete(
                IncompleteEvidenceKind::Inaccessible,
                "review repository",
                detail,
            )
        },
    )?);
    let common = if common.is_absolute() {
        common
    } else {
        root.join(common)
    };
    let common = fs::canonicalize(&common).map_err(|error| {
        incomplete(
            IncompleteEvidenceKind::Inaccessible,
            "review repository",
            format!("canonicalize Git common directory: {error}"),
        )
    })?;
    Ok(ContentDigest::of_bytes(
        common.as_os_str().as_encoded_bytes(),
    ))
}

fn incomplete(
    kind: IncompleteEvidenceKind,
    reference: impl Into<String>,
    detail: impl Into<String>,
) -> IncompleteEvidence {
    IncompleteEvidence::new(kind, reference, detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::completion_manifest::{
        COMPLETION_MANIFEST_VERSION, CompletionArtifactStore, EvidenceRef,
    };
    use crate::graph::{Status, Task};
    use crate::lifecycle::AttemptRef;
    use tempfile::TempDir;

    fn git_run(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success(), "git {} failed", args.join(" "));
    }

    fn fixture() -> (tempfile::TempDir, Task) {
        let temp = tempfile::tempdir().unwrap();
        git_run(temp.path(), &["init", "-q", "-b", "main"]);
        git_run(
            temp.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        git_run(temp.path(), &["config", "user.name", "Test"]);
        fs::write(temp.path().join("base.txt"), "base\n").unwrap();
        git_run(temp.path(), &["add", "base.txt"]);
        git_run(temp.path(), &["commit", "-qm", "base"]);
        let mut task = Task::default();
        task.id = "validation-fixture".into();
        task.title = "Validation fixture".into();
        task.status = Status::InProgress;
        task.validation_commands = vec!["printf 'ok\\n'".into()];
        task.lifecycle.generation = 2;
        task.lifecycle.fence = 7;
        task.lifecycle.current_attempt = Some(AttemptRef {
            id: "attempt-2-1".into(),
            generation: 2,
            fence: 7,
            actor_id: "agent-test".into(),
            disposition: None,
        });
        (temp, task)
    }

    fn evidence_ref(
        store: &CompletionArtifactStore,
        authority_dir: &Path,
        evidence: &DeterministicValidationEvidence,
    ) -> EvidenceRef {
        let artifact = store
            .put_bytes(
                &evidence.canonical_bytes().unwrap(),
                DETERMINISTIC_VALIDATION_MEDIA_TYPE,
            )
            .unwrap();
        register_capture_authority(authority_dir, &artifact.content_digest, evidence).unwrap();
        EvidenceRef {
            content_digest: artifact.content_digest,
            immutable_locator: artifact.immutable_locator,
            evidence_kind: CONFIGURED_VALIDATION_EVIDENCE_KIND.into(),
            media_type: artifact.media_type,
            size: artifact.size,
            review_projection: artifact.review_projection,
        }
    }

    #[test]
    fn capture_records_bounded_streams_exit_repository_and_timing() {
        let (temp, task) = fixture();
        let command = "python3 -c \"import sys; print('x'*70000); print('err', file=sys.stderr)\"";
        let evidence = capture_validation(
            &task,
            command,
            0,
            ValidationPurpose::Configured,
            temp.path(),
        )
        .unwrap();
        assert!(evidence.authoritative_pass(CompletionContract::Land));
        assert_eq!(evidence.exit.code, Some(0));
        assert!(evidence.stdout.truncated);
        assert_eq!(
            evidence.stdout.captured_bytes,
            MAX_CAPTURE_BYTES_PER_STREAM as u64
        );
        assert!(evidence.stdout.total_bytes > evidence.stdout.captured_bytes);
        assert!(!evidence.stderr.truncated);
        assert!(evidence.stderr.content.contains("err"));
        assert_eq!(evidence.repository.cwd_relative, ".");
        assert_eq!(
            evidence.repository.before_head_oid,
            evidence.repository.after_head_oid
        );
        assert!(!evidence.started_at.is_empty() && !evidence.finished_at.is_empty());
    }

    #[test]
    fn stale_attempt_and_candidate_evidence_fail_closed_before_review() {
        let (temp, task) = fixture();
        let store = CompletionArtifactStore::open(temp.path().join("store")).unwrap();
        let evidence = capture_validation(
            &task,
            &task.validation_commands[0],
            0,
            ValidationPurpose::Configured,
            temp.path(),
        )
        .unwrap();
        let authority_dir = temp.path().join(".wg");
        let reference = evidence_ref(&store, &authority_dir, &evidence);
        let output = store.put_bytes(b"report", "text/plain").unwrap();
        let manifest = CompletionManifest {
            manifest_version: COMPLETION_MANIFEST_VERSION,
            task_id: task.id.clone(),
            generation: task.lifecycle.generation,
            completion_contract: crate::simple_land::CompletionContract::Report,
            requirements_digest: requirements_digest(&task).unwrap(),
            source_revision: evidence.repository.before_head_oid.clone(),
            outputs: vec![OutputRef::Artifact(output)],
            validation_evidence: vec![reference],
            worker_summary_digest: ContentDigest::of_bytes(b"summary"),
        };
        let bundle = crate::completion_manifest::ReviewResolver::new(&store)
            .resolve(
                &manifest,
                &crate::completion_task::task_requirements_bytes(&task).unwrap(),
                b"summary",
            )
            .unwrap();
        let binding = CompletionReviewBinding {
            task_id: task.id.clone(),
            generation: 2,
            attempt_id: Some("attempt-2-1".into()),
            attempt_fence: 7,
            candidate_sequence: 1,
        };
        verify_validation_evidence(
            &task,
            &manifest,
            Some(&binding),
            &bundle,
            temp.path(),
            &authority_dir,
        )
        .unwrap();

        let stale = CompletionReviewBinding {
            attempt_fence: 8,
            ..binding
        };
        let error = verify_validation_evidence(
            &task,
            &manifest,
            Some(&stale),
            &bundle,
            temp.path(),
            &authority_dir,
        )
        .unwrap_err();
        assert_eq!(error.kind, IncompleteEvidenceKind::DigestMismatch);

        let mut mutated = manifest.clone();
        mutated.source_revision = "0".repeat(40);
        let error =
            verify_validation_evidence(&task, &mutated, None, &bundle, temp.path(), &authority_dir)
                .unwrap_err();
        assert_eq!(error.kind, IncompleteEvidenceKind::GitObjectMismatch);

        let authority = authority_dir.join(VALIDATION_AUTHORITY_DIR).join(
            manifest.validation_evidence[0]
                .content_digest
                .as_str()
                .strip_prefix("b3:")
                .unwrap(),
        );
        fs::remove_file(authority).unwrap();
        let error = verify_validation_evidence(
            &task,
            &manifest,
            None,
            &bundle,
            temp.path(),
            &authority_dir,
        )
        .unwrap_err();
        assert_eq!(error.kind, IncompleteEvidenceKind::Missing);
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_descendants_and_keeps_capture_wall_time_bounded() {
        let (temp, mut task) = fixture();
        task.verify_timeout = Some("1s".into());
        let started = Instant::now();
        let captured = capture_validation(
            &task,
            "sleep 30 & wait",
            0,
            ValidationPurpose::Configured,
            temp.path(),
        )
        .unwrap();
        assert!(captured.exit.timed_out, "{captured:?}");
        assert!(!captured.exit.success);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn tampered_or_failing_capture_never_becomes_validation_authority() {
        let (temp, task) = fixture();
        let mut captured = capture_validation(
            &task,
            &task.validation_commands[0],
            0,
            ValidationPurpose::Configured,
            temp.path(),
        )
        .unwrap();
        captured.stdout.digest = ContentDigest::of_bytes(b"forged stdout");
        let error = verify_output_shape("stdout", &captured.stdout).unwrap_err();
        assert_eq!(error.kind, IncompleteEvidenceKind::DigestMismatch);

        let failing = capture_validation(
            &task,
            "printf 'nope\\n' >&2; exit 9",
            0,
            ValidationPurpose::Configured,
            temp.path(),
        )
        .unwrap();
        assert_eq!(failing.exit.code, Some(9));
        assert!(!failing.authoritative_pass(CompletionContract::Land));
        assert!(failing.stderr.content.contains("nope"));
    }

    #[test]
    fn command_configuration_is_part_of_immutable_requirements() {
        let (_temp, mut task) = fixture();
        let first = requirements_digest(&task).unwrap();
        task.validation_commands = vec!["cargo test changed".into()];
        let changed = requirements_digest(&task).unwrap();
        assert_ne!(first, changed);
    }

    #[test]
    fn preflight_matches_enforced_commands_and_never_infers_prose() {
        let (_temp, mut task) = fixture();
        task.verify = Some("cargo fmt --check".into());
        task.validation_commands = vec!["cargo fmt --check".into(), "cargo clippy".into()];
        task.description =
            Some("## Validation\n- [ ] cargo test --locked smoke --no-fail-fast".into());
        let plan = completion_preflight(&task);
        assert_eq!(
            plan.checks
                .iter()
                .map(|check| check.command.as_str())
                .collect::<Vec<_>>(),
            vec!["cargo fmt --check", "cargo clippy", land_baseline_command()]
        );
        assert!(plan.checks[0].provenance.contains("task.verify"));
        assert!(!plan.prose_is_authority);
        assert!(
            plan.checks
                .iter()
                .all(|check| !check.command.contains("smoke"))
        );
        assert!(format_completion_preflight(&task).contains("host-bound immutable"));
    }

    #[test]
    fn deterministic_repair_is_candidate_bound_redacted_and_finite() {
        let (temp, mut task) = fixture();
        task.validation_commands = vec!["printf 'api_key=supersecret failure' >&2; exit 9".into()];
        let evidence_temp = TempDir::new().unwrap();
        let store = CompletionArtifactStore::open(evidence_temp.path().join("store")).unwrap();
        let first = capture_validation(
            &task,
            &task.validation_commands[0],
            0,
            ValidationPurpose::Configured,
            temp.path(),
        )
        .unwrap();
        let first_ref = evidence_ref(&store, &temp.path().join(".wg"), &first);
        let first_state =
            record_deterministic_repair_failure(&mut task, &first, &first_ref).unwrap();
        assert_eq!(
            first_state.disposition,
            CompletionRepairDisposition::Repairing
        );
        assert_eq!(first_state.opportunities_used, 1);
        assert!(first_state.diagnostic_excerpt.contains("[REDACTED]"));
        assert!(!first_state.diagnostic_excerpt.contains("supersecret"));

        // Cosmetic output/timestamp differences against unchanged source bytes
        // cannot replenish or advance the episode.
        let repeated = capture_validation(
            &task,
            "printf 'different output' >&2; exit 8",
            0,
            ValidationPurpose::Configured,
            temp.path(),
        )
        .unwrap();
        let repeated_ref = evidence_ref(&store, &temp.path().join(".wg"), &repeated);
        let repeated_state =
            record_deterministic_repair_failure(&mut task, &repeated, &repeated_ref).unwrap();
        assert_eq!(
            repeated_state.disposition,
            CompletionRepairDisposition::NeedsAttention
        );
        assert_eq!(repeated_state.reason_code, "unchanged-candidate-repeated");
        assert_eq!(repeated_state.opportunities_used, 1);
    }

    #[test]
    fn changed_candidates_cannot_extend_repair_episode_past_two() {
        let (temp, mut task) = fixture();
        task.validation_commands = vec!["exit 3".into()];
        let evidence_temp = TempDir::new().unwrap();
        let store = CompletionArtifactStore::open(evidence_temp.path().join("store")).unwrap();
        for index in 0..3 {
            if index > 0 {
                fs::write(
                    temp.path().join("candidate.txt"),
                    format!("revision {index}\n"),
                )
                .unwrap();
            }
            let captured = capture_validation(
                &task,
                &task.validation_commands[0],
                0,
                ValidationPurpose::Configured,
                temp.path(),
            )
            .unwrap();
            let reference = evidence_ref(&store, &temp.path().join(".wg"), &captured);
            let state =
                record_deterministic_repair_failure(&mut task, &captured, &reference).unwrap();
            if index < 2 {
                assert_eq!(state.disposition, CompletionRepairDisposition::Repairing);
            } else {
                assert_eq!(
                    state.disposition,
                    CompletionRepairDisposition::NeedsAttention
                );
                assert_eq!(state.reason_code, "deterministic-repair-budget-exhausted");
                assert_eq!(state.opportunities_used, 2);
            }
        }
    }

    #[test]
    fn stalled_chain_projection_is_cycle_safe_and_deduplicated() {
        let (temp, mut root) = fixture();
        root.id = "root".into();
        root.after = vec!["child".into()];
        root.validation_commands = vec!["exit 1".into()];
        let store = CompletionArtifactStore::open(temp.path().join("store")).unwrap();
        let captured = capture_validation(
            &root,
            &root.validation_commands[0],
            0,
            ValidationPurpose::Configured,
            temp.path(),
        )
        .unwrap();
        let reference = evidence_ref(&store, &temp.path().join(".wg"), &captured);
        record_deterministic_repair_failure(&mut root, &captured, &reference).unwrap();
        request_repair_attention(
            &mut root,
            "scope-approval-required",
            "operator decides once".into(),
        )
        .unwrap();
        let mut child = Task {
            id: "child".into(),
            title: "child".into(),
            status: Status::Open,
            after: vec!["root".into()],
            ..Task::default()
        };
        child.lifecycle.current_attempt = None;
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(root));
        graph.add_node(crate::graph::Node::Task(child));
        let chains = stalled_chains(&graph);
        assert_eq!(chains.len(), 1);
        assert_eq!(chains[0].root_task_id, "root");
        assert!(
            chains[0].affected_downstream.is_empty(),
            "the structural root↔child cycle is not itself downstream stall evidence"
        );

        let done = Task {
            id: "done-child".into(),
            title: "already satisfied".into(),
            status: Status::Done,
            after: vec!["root".into()],
            ..Task::default()
        };
        graph.add_node(crate::graph::Node::Task(done));
        assert!(stalled_chains(&graph)[0].affected_downstream.is_empty());

        let active = Task {
            id: "active-child".into(),
            title: "actively owned downstream".into(),
            status: Status::Open,
            assigned: Some("worker-2".into()),
            after: vec!["root".into()],
            ..Task::default()
        };
        graph.add_node(crate::graph::Node::Task(active));
        assert!(
            stalled_chains(&graph)[0].affected_downstream.is_empty(),
            "assigned active work is not a stalled-chain member"
        );
    }

    #[test]
    fn configured_command_requires_host_captured_evidence() {
        let (temp, task) = fixture();
        let store = CompletionArtifactStore::open(temp.path().join("store")).unwrap();
        let generic = store
            .put_bytes(b"worker says tests pass", "text/plain")
            .unwrap();
        let output = store.put_bytes(b"report", "text/plain").unwrap();
        let manifest = CompletionManifest {
            manifest_version: COMPLETION_MANIFEST_VERSION,
            task_id: task.id.clone(),
            generation: task.lifecycle.generation,
            completion_contract: crate::simple_land::CompletionContract::Report,
            requirements_digest: requirements_digest(&task).unwrap(),
            source_revision: git(temp.path(), &["rev-parse", "HEAD"]).unwrap(),
            outputs: vec![OutputRef::Artifact(output)],
            validation_evidence: vec![EvidenceRef {
                content_digest: generic.content_digest,
                immutable_locator: generic.immutable_locator,
                evidence_kind: "worker-prose".into(),
                media_type: generic.media_type,
                size: generic.size,
                review_projection: None,
            }],
            worker_summary_digest: ContentDigest::of_bytes(b"summary"),
        };
        let bundle = crate::completion_manifest::ReviewResolver::new(&store)
            .resolve(
                &manifest,
                &crate::completion_task::task_requirements_bytes(&task).unwrap(),
                b"summary",
            )
            .unwrap();
        let error = verify_validation_evidence(
            &task,
            &manifest,
            None,
            &bundle,
            temp.path(),
            &temp.path().join(".wg"),
        )
        .unwrap_err();
        assert_eq!(error.kind, IncompleteEvidenceKind::Missing);
    }
}
