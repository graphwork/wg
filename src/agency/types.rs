use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::graph::TrustLevel;

// ---------------------------------------------------------------------------
// Content reference (replaces SkillRef)
// ---------------------------------------------------------------------------

/// Reference to content, which can come from various sources.
/// Renamed from `SkillRef` to reflect that role components are broader than "skills".
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentRef {
    Name(String),
    File(PathBuf),
    Url(String),
    Inline(String),
}

/// A resolved skill/content with its name and content loaded into memory.
#[derive(Debug, Clone)]
pub struct ResolvedSkill {
    pub name: String,
    pub content: String,
}

// ---------------------------------------------------------------------------
// Primitive metadata types
// ---------------------------------------------------------------------------

/// Category of a role component, describing its origin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ComponentCategory {
    /// Directly translated from a human skill
    Translated,
    /// Enhanced version of a human skill
    Enhanced,
    /// Novel machine-only capability
    Novel,
}

/// Access control policy for federation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccessPolicy {
    Private,
    Shared,
    Open,
}

/// Access control metadata for a primitive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessControl {
    pub owner: String,
    pub policy: AccessPolicy,
}

impl Default for AccessControl {
    fn default() -> Self {
        Self {
            owner: "local".to_string(),
            policy: AccessPolicy::Open,
        }
    }
}

/// Reference from a primitive to a deployment (task assignment).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentRef {
    pub agent_id: String,
    pub task_id: String,
    pub timestamp: String,
    pub score: Option<f64>,
}

/// Staleness reason for a composition cache entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StalenessReason {
    Superseded,
    Retired,
}

/// A staleness flag on a composition cache entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StalenessFlag {
    pub primitive_id: String,
    pub reason: StalenessReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor_id: Option<String>,
    pub flagged_at: String,
}

// ---------------------------------------------------------------------------
// Shared performance / lineage types
// ---------------------------------------------------------------------------

/// Reference to an evaluation, stored inline in a PerformanceRecord.
///
/// `context_id` provides cross-reference: for components it holds role_id,
/// for roles it holds tradeoff_id, for agents it holds role_id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationRef {
    pub score: f64,
    pub task_id: String,
    pub timestamp: String,
    pub context_id: String,
}

/// Aggregated performance data for any entity (primitive or cache).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PerformanceRecord {
    pub task_count: u32,
    pub avg_score: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evaluations: Vec<EvaluationRef>,
}

/// Lineage metadata for tracking evolutionary history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lineage {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parent_ids: Vec<String>,
    #[serde(default)]
    pub generation: u32,
    #[serde(default = "default_created_by")]
    pub created_by: String,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
}

fn default_created_by() -> String {
    "human".to_string()
}

impl Default for Lineage {
    fn default() -> Self {
        Lineage {
            parent_ids: Vec::new(),
            generation: 0,
            created_by: "human".to_string(),
            created_at: Utc::now(),
        }
    }
}

impl Lineage {
    /// Create lineage for a mutation (single parent).
    pub fn mutation(parent_id: &str, parent_generation: u32, run_id: &str) -> Self {
        Lineage {
            parent_ids: vec![parent_id.to_string()],
            generation: parent_generation.saturating_add(1),
            created_by: format!("evolver-{}", run_id),
            created_at: Utc::now(),
        }
    }

    /// Create lineage for a crossover (two parents).
    pub fn crossover(parent_ids: &[&str], max_parent_generation: u32, run_id: &str) -> Self {
        Lineage {
            parent_ids: parent_ids
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
            generation: max_parent_generation.saturating_add(1),
            created_by: format!("evolver-{}", run_id),
            created_at: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// Primitives
// ---------------------------------------------------------------------------

/// A role component — a single capability, stored as a first-class primitive.
///
/// Stored in `primitives/components/{hash}.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleComponent {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: ComponentCategory,
    pub content: ContentRef,
    pub performance: PerformanceRecord,
    #[serde(default)]
    pub lineage: Lineage,
    #[serde(default)]
    pub access_control: AccessControl,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domain_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub former_agents: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub former_deployments: Vec<DeploymentRef>,
}

/// A desired outcome — what success looks like, stored as a first-class primitive.
///
/// Stored in `primitives/outcomes/{hash}.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesiredOutcome {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub success_criteria: Vec<String>,
    pub performance: PerformanceRecord,
    #[serde(default)]
    pub lineage: Lineage,
    #[serde(default)]
    pub access_control: AccessControl,
    #[serde(default = "default_true")]
    pub requires_human_oversight: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domain_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub former_agents: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub former_deployments: Vec<DeploymentRef>,
}

fn default_true() -> bool {
    true
}

/// A trade-off configuration — how an agent navigates competing considerations.
///
/// Replaces the old `Motivation` struct. Stored in `primitives/tradeoffs/{hash}.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeoffConfig {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptable_tradeoffs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unacceptable_tradeoffs: Vec<String>,
    pub performance: PerformanceRecord,
    #[serde(default)]
    pub lineage: Lineage,
    #[serde(default)]
    pub access_control: AccessControl,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domain_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub former_agents: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub former_deployments: Vec<DeploymentRef>,
}

// ---------------------------------------------------------------------------
// Composition cache
// ---------------------------------------------------------------------------

/// A role — a composition of component IDs + an outcome ID.
///
/// Stored in `cache/roles/{hash}.yaml`. No longer bundles skills inline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Role {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Sorted component IDs for deterministic hashing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub component_ids: Vec<String>,
    /// ID of the DesiredOutcome primitive.
    #[serde(default)]
    pub outcome_id: String,
    pub performance: PerformanceRecord,
    #[serde(default)]
    pub lineage: Lineage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_context_scope: Option<String>,
    /// Default execution weight for tasks assigned to agents with this role.
    /// Values: "full" (default), "light" (read-only tools), "bare" (wg CLI only), "shell" (no LLM).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_exec_mode: Option<String>,
}

fn default_executor() -> String {
    "claude".to_string()
}

/// A first-class agent entity — a role paired with a trade-off configuration.
///
/// Stored in `cache/agents/{hash}.yaml`.
/// Agent ID = SHA-256(role_id + tradeoff_id).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    pub role_id: String,
    #[serde(alias = "motivation_id")]
    pub tradeoff_id: String,
    pub name: String,
    pub performance: PerformanceRecord,
    #[serde(default)]
    pub lineage: Lineage,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity: Option<f64>,
    #[serde(default, skip_serializing_if = "is_default_trust")]
    pub trust_level: TrustLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
    #[serde(
        default = "default_executor",
        skip_serializing_if = "is_default_executor"
    )]
    pub executor: String,
    /// Preferred model for this agent (e.g., "opus", "sonnet", "haiku",
    /// or a full model ID like "claude-opus-4-6").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_model: Option<String>,
    /// Preferred provider for this agent (e.g., "anthropic", "openrouter").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deployment_history: Vec<DeploymentRef>,
    #[serde(default = "default_attractor_weight")]
    pub attractor_weight: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub staleness_flags: Vec<StalenessFlag>,
}

fn default_attractor_weight() -> f64 {
    0.5
}

/// Executor types that represent human operators (not AI agents).
const HUMAN_EXECUTORS: &[&str] = &["matrix", "email", "shell"];

/// Returns true if the given executor string represents a human operator.
pub fn is_human_executor(executor: &str) -> bool {
    HUMAN_EXECUTORS.contains(&executor)
}

/// Providers that are not Anthropic-native and should default to the "native" executor.
const NON_ANTHROPIC_PROVIDERS: &[&str] = &["openrouter", "oai-compat", "openai", "local"];

impl Agent {
    /// Returns true if this agent uses a human executor (matrix, email, shell).
    pub fn is_human(&self) -> bool {
        is_human_executor(&self.executor)
    }

    /// Return the effective executor, considering provider-based auto-detection.
    ///
    /// If executor was explicitly set to a non-default value, returns that.
    /// Otherwise, if `preferred_provider` is openrouter/openai/local, returns "native".
    pub fn effective_executor(&self) -> &str {
        self.effective_executor_for_model(None)
    }

    /// Return the effective executor, with model-prefix compatibility override.
    ///
    /// Same as [`effective_executor`], but additionally consults the model
    /// spec: if the candidate executor would be `claude` AND the model spec
    /// has a non-Anthropic provider prefix (e.g. `local:`, `openrouter:`,
    /// `oai-compat:`, `openai:`), the agency CANNOT route through the claude
    /// CLI — it does not know how to invoke those models and would 404. In
    /// that case we override to the executor the model actually requires
    /// (typically `native`) and log a one-line conflict resolution.
    ///
    /// This is the autohaiku regression: agent.executor=claude (default) +
    /// model=local:qwen3-coder produced 100% failure because every spawn
    /// hit the claude CLI which 404'd on `qwen3-coder`. The bug was that
    /// the agency picked claude despite the model spec explicitly saying
    /// "local". This method enforces that contract.
    pub fn effective_executor_for_model(&self, model: Option<&str>) -> &str {
        let candidate = if !is_default_executor(&self.executor) {
            self.executor.as_str()
        } else if let Some(ref provider) = self.preferred_provider {
            if NON_ANTHROPIC_PROVIDERS.contains(&provider.as_str()) {
                "native"
            } else {
                self.executor.as_str()
            }
        } else {
            self.executor.as_str()
        };

        // Model-prefix override: only kicks in when candidate is claude and
        // the model spec has a non-claude/non-codex provider prefix. This
        // preserves explicit non-claude executor choices made by the agent.
        if candidate == "claude"
            && let Some(model_str) = model
        {
            let spec = crate::config::parse_model_spec(model_str);
            if let Some(ref provider) = spec.provider {
                let required = crate::config::provider_to_executor(provider);
                if required != "claude" {
                    eprintln!(
                        "[agency] effective_executor: agent role '{}' implies claude but model '{}' requires '{}'; routing to '{}'",
                        self.role_id, model_str, required, required,
                    );
                    return required;
                }
            }
        }

        candidate
    }
}

fn is_default_trust(level: &TrustLevel) -> bool {
    *level == TrustLevel::Provisional
}

fn is_default_executor(executor: &str) -> bool {
    executor == "claude"
}

// ---------------------------------------------------------------------------
// Rubric spectrum
// ---------------------------------------------------------------------------

/// Discrete rubric level for an evaluation score.
///
/// Maps a continuous [0, 1] score onto a five-level spectrum used
/// in prompt rendering and human-readable reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RubricLevel {
    /// 0.0–0.2: fundamental failures
    Failing,
    /// 0.2–0.4: significant deficiencies
    BelowExpectations,
    /// 0.4–0.6: acceptable but unremarkable
    MeetsExpectations,
    /// 0.6–0.8: solid, reliable work
    ExceedsExpectations,
    /// 0.8–1.0: exceptional, best-in-class
    Exceptional,
}

impl RubricLevel {
    /// Human-readable label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Failing => "Failing",
            Self::BelowExpectations => "Below Expectations",
            Self::MeetsExpectations => "Meets Expectations",
            Self::ExceedsExpectations => "Exceeds Expectations",
            Self::Exceptional => "Exceptional",
        }
    }
}

impl std::fmt::Display for RubricLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// Classify a score in [0, 1] to a rubric level.
///
/// Boundary convention: lower-inclusive, upper-exclusive except for the
/// top bucket which is upper-inclusive.
pub fn classify_rubric_level(score: f64) -> RubricLevel {
    match score {
        s if s < 0.2 => RubricLevel::Failing,
        s if s < 0.4 => RubricLevel::BelowExpectations,
        s if s < 0.6 => RubricLevel::MeetsExpectations,
        s if s < 0.8 => RubricLevel::ExceedsExpectations,
        _ => RubricLevel::Exceptional,
    }
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

/// An evaluation of agent performance on a specific task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evaluation {
    #[serde(default)]
    pub id: String,
    pub task_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent_id: String,
    #[serde(default)]
    pub role_id: String,
    #[serde(default, alias = "motivation_id")]
    pub tradeoff_id: String,
    #[serde(alias = "value")]
    pub score: f64,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub dimensions: HashMap<String, f64>,
    #[serde(alias = "reasoning")]
    pub notes: String,
    #[serde(alias = "evaluated_by")]
    pub evaluator: String,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default = "default_eval_source")]
    pub source: String,
}

fn default_eval_source() -> String {
    "llm".to_string()
}

// ---------------------------------------------------------------------------
// Iteration / Retry Types
// ---------------------------------------------------------------------------

/// How propagation should be applied to dependents when a task retries.
/// Used in IterationConfig.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum PropagationPolicy {
    /// Conservative: only dependents with changed interface re-run
    #[default]
    Conservative,
    /// Aggressive: all dependents re-run
    Aggressive,
    /// Conditional: re-run if score delta exceeds threshold
    Conditional(f32),
}

/// Retry strategy recommended by the evaluator.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetryStrategy {
    /// Retry with the same model/executor
    SameModel,
    /// Retry with a stronger model
    UpgradeModel,
    /// Escalate to a human for review
    EscalateToHuman,
}

/// Configuration for task iteration/retry behavior.
/// Attached to tasks via --max-retries, --propagation, --retry-strategy flags.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct IterationConfig {
    /// Maximum number of retries allowed (evaluator-triggered)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
    /// How to propagate retries to dependent tasks
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub propagation: Option<PropagationPolicy>,
    /// What retry strategy to use
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_strategy: Option<RetryStrategy>,
}

// ---------------------------------------------------------------------------
// Evaluation source type conventions
// ---------------------------------------------------------------------------

/// Standard evaluation source types.
pub mod eval_source {
    /// Auto-evaluator (LLM judge).
    pub const LLM: &str = "llm";
    /// Human evaluation.
    pub const MANUAL: &str = "manual";
    /// FLIP (roundtrip intent fidelity) evaluation.
    pub const FLIP: &str = "flip";
    /// Constraint-fidelity lint (detect orchestrator-fabricated constraints).
    pub const CONSTRAINT_FIDELITY: &str = "constraint-fidelity";
    /// Human reviewing evaluator output (meta-evaluation).
    pub const META_HUMAN_REVIEW: &str = "meta:human-review";

    /// Build a peer-evaluation source string: `meta:peer-eval:{evaluator_id}`.
    pub fn meta_peer_eval(evaluator_id: &str) -> String {
        format!("meta:peer-eval:{}", evaluator_id)
    }

    /// Build an outcome-correlation source string: `meta:outcome-correlation:{metric}`.
    pub fn meta_outcome_correlation(metric: &str) -> String {
        format!("meta:outcome-correlation:{}", metric)
    }

    /// Returns true if the source is a meta-evaluation type.
    pub fn is_meta(source: &str) -> bool {
        source.starts_with("meta:")
    }
}

// ---------------------------------------------------------------------------
// Metadata / display types
// ---------------------------------------------------------------------------

/// Summary counts of entities in a store.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoreCounts {
    pub components: usize,
    pub outcomes: usize,
    pub tradeoffs: usize,
    pub roles: usize,
    pub agents: usize,
    pub evaluations: usize,
}

/// A node in a lineage ancestry tree.
#[derive(Debug, Clone)]
pub struct AncestryNode {
    pub id: String,
    pub name: String,
    pub generation: u32,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub parent_ids: Vec<String>,
}

/// An entry in the artifact manifest written to artifacts.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactEntry {
    pub path: String,
    pub size: Option<u64>,
}

// ---------------------------------------------------------------------------
// Run mode continuum types
// ---------------------------------------------------------------------------

/// What dimension was varied in a learning experiment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExperimentDimension {
    /// A single role component was swapped.
    RoleComponent {
        /// None if this is a new addition.
        replaced: Option<String>,
        introduced: String,
    },
    /// The trade-off configuration was swapped.
    TradeoffConfig {
        replaced: Option<String>,
        introduced: String,
    },
    /// Everything composed fresh (no controlled variable).
    NovelComposition,
}

/// Metadata recorded when an assignment is made in learning mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentExperiment {
    /// The base composition used as the control (None for NovelComposition).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_composition: Option<String>,
    /// What was varied.
    pub dimension: ExperimentDimension,
    /// Whether this was triggered by the bizarre ideation schedule.
    #[serde(default)]
    pub bizarre_ideation: bool,
    /// UCB scores of alternatives considered (for post-hoc analysis).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub ucb_scores: HashMap<String, f64>,
}

/// How a task assignment was routed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssignmentMode {
    /// Deliberate learning experiment.
    Learning(AssignmentExperiment),
    /// Forced learning episode (exploration_interval trigger).
    ForcedExploration(AssignmentExperiment),
}

// ---------------------------------------------------------------------------
// Assignment source tracking
// ---------------------------------------------------------------------------

/// Tracks how an assignment was sourced — natively via workgraph's built-in
/// pipeline, or externally via the Agency server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssignmentSource {
    Native,
    Agency { agency_task_id: String },
}

fn default_assignment_source() -> AssignmentSource {
    AssignmentSource::Native
}

/// Persisted alongside each task assignment.
///
/// Stored in `.workgraph/agency/assignments/<task_id>.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskAssignmentRecord {
    pub task_id: String,
    pub agent_id: String,
    pub composition_id: String,
    pub timestamp: String,
    pub mode: AssignmentMode,
    /// Agency-side task ID, populated when assignment came from Agency.
    /// Used to POST evaluation results back to Agency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agency_task_id: Option<String>,
    /// How this assignment was sourced (native pipeline vs. Agency server).
    #[serde(default = "default_assignment_source")]
    pub assignment_source: AssignmentSource,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_agent_with_executor(executor: &str, preferred_provider: Option<&str>) -> Agent {
        Agent {
            id: "test-agent".to_string(),
            role_id: "test-role".to_string(),
            tradeoff_id: "test-tradeoff".to_string(),
            name: "TestAgent".to_string(),
            performance: PerformanceRecord {
                task_count: 0,
                avg_score: None,
                evaluations: vec![],
            },
            lineage: Lineage::default(),
            capabilities: vec![],
            rate: None,
            capacity: None,
            trust_level: TrustLevel::Provisional,
            contact: None,
            executor: executor.to_string(),
            preferred_model: None,
            preferred_provider: preferred_provider.map(String::from),
            deployment_history: vec![],
            attractor_weight: 0.5,
            staleness_flags: vec![],
        }
    }

    /// Regression: agency must NEVER pick the claude executor when the model spec
    /// has a non-Anthropic provider prefix (e.g. `local:`, `openrouter:`,
    /// `oai-compat:`, `openai:`). The claude CLI cannot route those models —
    /// it returns 404. Bug observed in autohaiku: agent.executor=claude (default)
    /// + model=local:qwen3-coder → 100% failure rate. Fix: agency overrides
    /// executor to a model-compatible one (native) when the conflict is detected.
    #[test]
    fn test_agency_does_not_pick_claude_for_local_model() {
        let agent = test_agent_with_executor("claude", None);
        // Sanity: with no model context, default behavior is preserved.
        assert_eq!(agent.effective_executor(), "claude");

        // The bug: agent's role implies claude, but the model is local:qwen3-coder.
        // claude CLI cannot run qwen3-coder → must route to native.
        assert_eq!(
            agent.effective_executor_for_model(Some("local:qwen3-coder")),
            "native",
            "agency must override claude → native when model has local: prefix"
        );

        // Same for the other non-Anthropic provider prefixes.
        assert_eq!(
            agent.effective_executor_for_model(Some("openrouter:deepseek/deepseek-v3.2")),
            "native",
        );
        assert_eq!(
            agent.effective_executor_for_model(Some("oai-compat:llama3")),
            "native",
        );
        assert_eq!(
            agent.effective_executor_for_model(Some("openai:gpt-4o")),
            "native",
        );
    }

    /// claude:opus + claude executor is a valid combination — no override needed.
    #[test]
    fn test_agency_keeps_claude_for_anthropic_model() {
        let agent = test_agent_with_executor("claude", None);
        assert_eq!(
            agent.effective_executor_for_model(Some("claude:opus")),
            "claude",
        );
        // Bare aliases (no provider prefix) are also assumed claude-compatible
        // when the agent's executor is claude.
        assert_eq!(agent.effective_executor_for_model(Some("opus")), "claude");
        assert_eq!(agent.effective_executor_for_model(Some("sonnet")), "claude");
    }

    /// codex executor explicitly chosen + non-Anthropic model: do NOT override
    /// to native. The agent's explicit non-default executor wins, since the
    /// override only fires when claude is the candidate (the broken combo).
    #[test]
    fn test_agency_does_not_override_explicit_non_claude_executor() {
        let agent = test_agent_with_executor("codex", None);
        assert_eq!(
            agent.effective_executor_for_model(Some("local:qwen3-coder")),
            "codex",
        );
    }

    /// Existing YAML files without `assignment_source` should deserialize
    /// with the default value (Native).
    #[test]
    fn test_assignment_record_default_source() {
        let yaml = r#"
task_id: my-task
agent_id: agent-1
composition_id: comp-1
timestamp: "2026-03-19T00:00:00Z"
mode:
  type: learning
  base_composition: null
  dimension:
    type: novel_composition
  bizarre_ideation: false
  ucb_scores: {}
"#;
        let record: TaskAssignmentRecord = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(record.assignment_source, AssignmentSource::Native);
    }

    /// Roundtrip: serialize Agency variant then deserialize back.
    #[test]
    fn test_assignment_source_agency_roundtrip() {
        let source = AssignmentSource::Agency {
            agency_task_id: "ext-task-42".to_string(),
        };
        let yaml = serde_yaml::to_string(&source).unwrap();
        let deserialized: AssignmentSource = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(deserialized, source);
    }

    /// Roundtrip: serialize Native variant then deserialize back.
    #[test]
    fn test_assignment_source_native_roundtrip() {
        let source = AssignmentSource::Native;
        let yaml = serde_yaml::to_string(&source).unwrap();
        let deserialized: AssignmentSource = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(deserialized, source);
    }
}
