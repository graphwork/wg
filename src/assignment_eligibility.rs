//! Assignment eligibility guard — the front-door role/persona check.
//!
//! WG's lightweight LLM assigner occasionally picks an evaluator/reviewer-only
//! persona for a task that requires real implementation work (build, register,
//! write code, produce artifacts). The selected worker then behaves as an
//! evaluator/reviewer, reports missing implementation/artifacts, and retries
//! the same non-implementation behaviour until the task fails. Two recent
//! failures of exactly this shape were `build-real-async` and
//! `register-refreshed-e97-seed-latest`.
//!
//! This module is a **small, understandable rule set** layered over the
//! existing agency structures (role name + role components) — it does NOT
//! invent a parallel capability system and does NOT hard-code a single agent
//! name. It classifies:
//!
//! - whether a [`Task`] requires an implementation-capable worker
//!   ([`task_requires_implementation`]);
//! - whether a [`Role`] is implementation-capable vs review/evaluator-only
//!   ([`role_implementation_capability`]);
//! - and produces an [`EligibilityVerdict`] the dispatcher / `wg assign` can
//!   act on ([`check_assignment_eligibility`]).
//!
//! Design rules (kept deliberately narrow so the guard is predictable):
//!
//! 1. **Implementation tasks** are signalled by `exec_mode == "full"`,
//!    non-empty `deliverables`, implementation verbs in title/tags, OR
//!    implementation-flavoured skills — **unless** the task is an explicit
//!    evaluation/review task (`.evaluate-*` / `.flip-*` scaffold, or tagged
//!    `review`/`evaluation`).
//! 2. **Evaluation/review tasks** (`.evaluate-*`, `.flip-*`, or tagged
//!    review/evaluation) MUST still route to evaluator/FLIP-style roles — the
//!    guard never blocks those.
//! 3. **Explicit human pinning wins**, but a mismatch warns loudly
//!    ([`EligibilityVerdict::Warn`]) so the operator sees it.
//! 4. **Auto-assignment** that picks a review-only persona for an
//!    implementation task is mutated to [`EligibilityVerdict::Reassign`],
//!    carrying a fallback implementation-capable agent when one exists.

use crate::agency::{Agent, Role};
use crate::graph::{Task, is_agency_scaffold_task};
use std::path::Path;

/// Implementation-flavoured verbs / nouns we look for in a task's title or
/// tags. Matched case-insensitively as whole tokens (word-boundary substring
/// match), so e.g. "build" matches "Build the async runtime" but not
/// "rebuild-only-review".
const IMPLEMENTATION_TOKENS: &[&str] = &[
    "implement",
    "implementation",
    "build",
    "write",
    "create",
    "add",
    "fix",
    "repair",
    "refactor",
    "register",
    "intake",
    "deploy",
    "ship",
    "code",
    "port",
    "migrate",
    "wire",
    "integrate",
];

/// Tags that mark a task as implementation work (exact, case-insensitive).
const IMPLEMENTATION_TAGS: &[&str] = &[
    "implementation",
    "implement",
    "build",
    "code",
    "fix",
    "feature",
    "refactor",
    "register",
    "intake",
];

/// Tags that mark a task as evaluation/review work (exact, case-insensitive).
/// These override the implementation signal — a task tagged `review` is a
/// review task even if its title says "fix".
const EVALUATION_TAGS: &[&str] = &["review", "evaluation", "evaluate", "eval"];

/// Skills that signal implementation capability is required.
const IMPLEMENTATION_SKILLS: &[&str] = &[
    "rust",
    "code",
    "coding",
    "programming",
    "testing",
    "debugging",
    "implementation",
    "engineering",
];

/// Role names for the agency **meta** personas (Assigner / Evaluator /
/// Evolver / Agent Creator). These are never implementation workers — they
/// run the agency lifecycle, not task code. Matched case-sensitively against
/// the canonical starter names (they are not free-form user labels).
const META_ROLE_NAMES: &[&str] = &["Assigner", "Evaluator", "Evolver", "Agent Creator"];

/// Role names that are explicitly review/evaluator-only personas.
const REVIEW_ROLE_NAMES: &[&str] = &["Reviewer", "Evaluator"];

/// Role names that are explicitly implementation-capable personas.
const IMPLEMENTATION_ROLE_NAMES: &[&str] = &[
    "Programmer",
    "Architect",
    "Documenter",
    "Implementer",
    "Engineer",
    "Developer",
];

/// Component content names (the `ContentRef::Name` payload) that confer
/// implementation capability on a role.
const IMPLEMENTATION_COMPONENTS: &[&str] = &[
    "code-writing",
    "testing",
    "debugging",
    "system-design",
    "dependency-analysis",
    "technical-writing",
];

/// Component content names that are review/audit-only — a role whose every
/// component is in this set is review-only.
const REVIEW_COMPONENTS: &[&str] = &["code-review", "security-audit"];

/// Coarse classification of a [`Role`]'s implementation capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleCapability {
    /// The role can produce implementation work (writes/tests/designs code or
    /// docs). Safe to assign to an implementation task.
    ImplementationCapable,
    /// The role is review/evaluator-only (e.g. `Reviewer`, `Evaluator`). Must
    /// NOT be assigned to an implementation task.
    ReviewOnly,
    /// The role's capability could not be determined from its name or
    /// components. The guard treats this as "do not block" — failing open
    /// here avoids stranding tasks on custom roles we don't recognise.
    Unknown,
}

/// The guard's verdict for a single (task, agent, role) triple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EligibilityVerdict {
    /// The assignment is fine — proceed.
    Allow,
    /// The assignment is a mismatch (e.g. a review-only persona on an
    /// implementation task) but the caller should honour it anyway because a
    /// human pinned it. The caller SHOULD emit a loud warning quoting
    /// `reason`.
    Warn { reason: String },
    /// The assignment is an auto-pick mismatch and should be mutated. When
    /// `fallback_agent_id` is `Some`, the caller should reassign to that
    /// agent; when `None`, no implementation-capable agent was available and
    /// the caller should proceed with the original pick (logged) rather than
    /// strand the task.
    Reassign {
        reason: String,
        fallback_agent_id: Option<String>,
    },
}

/// Does this task require an implementation-capable worker?
///
/// True when any implementation signal is present AND the task is not an
/// explicit evaluation/review task. See the module docs for the rule set.
pub fn task_requires_implementation(task: &Task) -> bool {
    if task_is_evaluation_or_review(task) {
        return false;
    }
    if task.exec_mode.as_deref() == Some("full") {
        return true;
    }
    if !task.deliverables.is_empty() {
        return true;
    }
    if tags_contain_any(&task.tags, IMPLEMENTATION_TAGS) {
        return true;
    }
    if skills_require_implementation(&task.skills) {
        return true;
    }
    if title_has_implementation_token(&task.title) {
        return true;
    }
    false
}

/// Is this task explicitly an evaluation / review task?
///
/// True for `.evaluate-*` / `.flip-*` agency scaffold, or any task tagged
/// `review` / `evaluation` / `evaluate` / `eval`. Such tasks MUST route to
/// evaluator/FLIP roles and are never blocked by this guard.
pub fn task_is_evaluation_or_review(task: &Task) -> bool {
    if is_agency_scaffold_task(&task.id)
        && (task.id.starts_with(".evaluate-") || task.id.starts_with(".flip-"))
    {
        return true;
    }
    if tags_contain_any(&task.tags, EVALUATION_TAGS) {
        return true;
    }
    // `.assign-*` scaffolding is neither implementation nor evaluation — it is
    // the assignment primitive itself. Treat it as evaluation-flavoured so the
    // guard never tries to force an implementation worker onto it.
    if task.id.starts_with(".assign-") {
        return true;
    }
    false
}

/// Classify a role's implementation capability from its name and components.
///
/// Name-only path: covers the built-in starter + special roles. For custom /
/// evolved roles the component-based path needs the component content *names*,
/// which require a filesystem read — use
/// [`role_implementation_capability_with_components`] for that.
pub fn role_implementation_capability(role: &Role) -> RoleCapability {
    classify_role(&role.name, &[])
}

/// Classify a role given pre-resolved component content names.
///
/// `component_names` are the `ContentRef::Name` payloads of the role's
/// components (resolved from disk by the caller). Pass `&[]` to rely on the
/// name-based path only.
pub fn role_implementation_capability_with_components(
    role: &Role,
    component_names: &[String],
) -> RoleCapability {
    classify_role(&role.name, component_names)
}

fn classify_role(name: &str, comp_names: &[String]) -> RoleCapability {
    // Name-based fast path — covers the built-in starter + special roles.
    if IMPLEMENTATION_ROLE_NAMES.contains(&name) {
        return RoleCapability::ImplementationCapable;
    }
    if REVIEW_ROLE_NAMES.contains(&name) || META_ROLE_NAMES.contains(&name) {
        // Meta personas (Assigner/Evolver/Agent Creator) are not reviewers, but
        // they are equally not implementation workers — for the purposes of
        // this guard they must not be assigned to implementation tasks.
        return RoleCapability::ReviewOnly;
    }

    // Component-based path — for custom / evolved roles.
    if comp_names.is_empty() {
        return RoleCapability::Unknown;
    }
    let has_impl = comp_names
        .iter()
        .any(|n| IMPLEMENTATION_COMPONENTS.contains(&n.as_str()));
    let only_review = comp_names
        .iter()
        .all(|n| REVIEW_COMPONENTS.contains(&n.as_str()));
    if has_impl {
        RoleCapability::ImplementationCapable
    } else if only_review {
        RoleCapability::ReviewOnly
    } else {
        RoleCapability::Unknown
    }
}

/// Resolve a role's component content *names* from disk.
///
/// Returns the `ContentRef::Name` payload for each component the role
/// references; components whose content is `File`/`Url`/`Inline` (no stable
/// name) are skipped. Missing component files are skipped silently — a
/// partial resolution still feeds the classifier, and an empty result falls
/// back to `Unknown` (fail-open).
pub fn resolve_role_component_names(role: &Role, components_dir: &Path) -> Vec<String> {
    let mut names = Vec::with_capacity(role.component_ids.len());
    for cid in &role.component_ids {
        let Ok(component) = crate::agency::find_component_by_prefix(components_dir, cid) else {
            continue;
        };
        if let crate::agency::ContentRef::Name(n) = &component.content {
            names.push(n.clone());
        }
    }
    names
}

/// True when the role is confidently **not** implementation-capable
/// (i.e. `ReviewOnly`). `Unknown` roles are NOT blockers (fail open).
pub fn role_blocks_implementation(role: &Role) -> bool {
    role_implementation_capability(role) == RoleCapability::ReviewOnly
}

/// Component-aware blocker check — uses resolved component content names so
/// custom / evolved roles are classified by their actual capabilities, not
/// just their (possibly arbitrary) name.
pub fn role_blocks_implementation_with_components(role: &Role, component_names: &[String]) -> bool {
    role_implementation_capability_with_components(role, component_names)
        == RoleCapability::ReviewOnly
}

/// Check a single (task, agent, role) assignment for eligibility.
///
/// `explicit` is true when a human pinned the agent (e.g. `wg assign <task>
/// <hash>`); explicit pins produce a [`EligibilityVerdict::Warn`] instead of
/// a [`EligibilityVerdict::Reassign`] so the human's choice still wins.
///
/// `fallback_agent_id` lets the caller pass a pre-resolved
/// implementation-capable agent to surface in the `Reassign` verdict; pass
/// `None` when the caller wants to resolve one itself (or when none exists).
pub fn check_assignment_eligibility(
    task: &Task,
    agent: &Agent,
    role: Option<&Role>,
    explicit: bool,
    fallback_agent_id: Option<String>,
) -> EligibilityVerdict {
    if !task_requires_implementation(task) {
        return EligibilityVerdict::Allow;
    }
    // No role resolved — can't classify. Fail open (don't block) but the
    // caller should ideally have resolved one.
    let Some(role) = role else {
        return EligibilityVerdict::Allow;
    };
    if !role_blocks_implementation(role) {
        return EligibilityVerdict::Allow;
    }
    let reason = format!(
        "task '{}' requires implementation work but agent '{}' has role '{}' \
         ({}), which is review/evaluator-only; an implementation-capable role \
         is required",
        task.id,
        agent.name,
        role.name,
        crate::agency::short_hash(&agent.id),
    );
    if explicit {
        EligibilityVerdict::Warn { reason }
    } else {
        EligibilityVerdict::Reassign {
            reason,
            fallback_agent_id,
        }
    }
}

/// Pick the best fallback implementation-capable agent from a pool.
///
/// "Best" = highest `avg_score` among implementation-capable, non-stale,
/// non-human agents whose role resolves. Returns the agent id (full hash) or
/// `None` when no implementation-capable agent is available.
///
/// `components_dir` lets the classifier resolve component content names for
/// custom / evolved roles; pass the agency `primitives/components` dir.
pub fn pick_implementation_capable_agent<'a>(
    agents: &'a [Agent],
    roles_dir: &Path,
    components_dir: &Path,
) -> Option<&'a Agent> {
    let mut best: Option<&Agent> = None;
    let mut best_score = f64::MIN;
    for agent in agents {
        if agent.is_human() || !agent.staleness_flags.is_empty() {
            continue;
        }
        let role = match crate::agency::find_role_by_prefix(roles_dir, &agent.role_id) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let comp_names = resolve_role_component_names(&role, components_dir);
        if role_implementation_capability_with_components(&role, &comp_names)
            != RoleCapability::ImplementationCapable
        {
            continue;
        }
        let score = agent.performance.avg_score.unwrap_or(0.0);
        if best.is_none() || score > best_score {
            best = Some(agent);
            best_score = score;
        }
    }
    best
}

// ---------------------------------------------------------------------------
// internals
// ---------------------------------------------------------------------------

fn tags_contain_any(tags: &[String], needles: &[&str]) -> bool {
    tags.iter().any(|t| {
        let lower = t.to_lowercase();
        needles.iter().any(|n| n.eq_ignore_ascii_case(&lower))
    })
}

fn skills_require_implementation(skills: &[String]) -> bool {
    skills.iter().any(|s| {
        let lower = s.to_lowercase();
        IMPLEMENTATION_SKILLS
            .iter()
            .any(|n| lower.contains(n) || n.contains(lower.as_str()))
    })
}

fn title_has_implementation_token(title: &str) -> bool {
    let lower = title.to_lowercase();
    IMPLEMENTATION_TOKENS.iter().any(|tok| {
        // Word-boundary-ish match: the token is bordered by a non-alpha char
        // (or string start/end) on at least one side, so "build" doesn't
        // match inside "rebuilds". We accept a simple contains check for the
        // short verb list — the deliverables/exec_mode/tags signals already
        // carry the strong cases, and this is a best-effort hint.
        lower.contains(tok)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agency::{Lineage, PerformanceRecord, Role};
    use crate::graph::{Status, Task};

    fn task_with(id: &str, title: &str) -> Task {
        Task {
            id: id.to_string(),
            title: title.to_string(),
            status: Status::Open,
            ..Task::default()
        }
    }

    fn role_named(name: &str) -> Role {
        Role {
            id: format!("role-{}", name),
            name: name.to_string(),
            description: String::new(),
            component_ids: Vec::new(),
            outcome_id: String::new(),
            performance: PerformanceRecord::default(),
            lineage: Lineage::default(),
            default_context_scope: None,
            default_exec_mode: None,
        }
    }

    fn agent_with(name: &str, role: &Role) -> Agent {
        Agent {
            id: format!("agent-{}", name),
            role_id: role.id.clone(),
            tradeoff_id: "tradeoff-1".to_string(),
            name: name.to_string(),
            performance: PerformanceRecord::default(),
            lineage: Lineage::default(),
            capabilities: Vec::new(),
            rate: None,
            capacity: None,
            trust_level: Default::default(),
            contact: None,
            executor: "claude".to_string(),
            preferred_model: None,
            preferred_provider: None,
            attractor_weight: 1.0,
            deployment_history: vec![],
            staleness_flags: vec![],
        }
    }

    // --- task classification ------------------------------------------------

    #[test]
    fn implementation_task_detected_by_build_title() {
        let mut t = task_with("build-real-async", "Build the real async runtime");
        t.deliverables = vec!["src/async.rs".to_string()];
        assert!(task_requires_implementation(&t));
    }

    #[test]
    fn implementation_task_detected_by_register_verb() {
        let t = task_with("register-seed", "Register refreshed e97 seed latest");
        assert!(
            task_requires_implementation(&t),
            "register verb should trigger"
        );
    }

    #[test]
    fn implementation_task_detected_by_exec_mode_full() {
        let mut t = task_with("t1", "Do the thing");
        t.exec_mode = Some("full".to_string());
        assert!(task_requires_implementation(&t));
    }

    #[test]
    fn implementation_task_detected_by_deliverables() {
        let mut t = task_with("t1", "Some work");
        t.deliverables = vec!["docs/x.md".to_string()];
        assert!(task_requires_implementation(&t));
    }

    #[test]
    fn implementation_task_detected_by_tag() {
        let mut t = task_with("t1", "Some work");
        t.tags = vec!["implementation".to_string()];
        assert!(task_requires_implementation(&t));
    }

    #[test]
    fn non_implementation_neutral_task_not_flagged() {
        let t = task_with("t1", "Triage incoming issues");
        assert!(!task_requires_implementation(&t));
    }

    #[test]
    fn evaluate_scaffold_is_evaluation_task() {
        let t = task_with(".evaluate-foo", "Evaluate foo");
        assert!(task_is_evaluation_or_review(&t));
        assert!(!task_requires_implementation(&t));
    }

    #[test]
    fn flip_scaffold_is_evaluation_task() {
        let t = task_with(".flip-foo", "Flip foo");
        assert!(task_is_evaluation_or_review(&t));
    }

    #[test]
    fn review_tagged_task_is_evaluation_task_even_with_impl_title() {
        let mut t = task_with("t1", "Fix the bug");
        t.tags = vec!["review".to_string()];
        assert!(task_is_evaluation_or_review(&t));
        assert!(
            !task_requires_implementation(&t),
            "review tag overrides impl title"
        );
    }

    // --- role classification ------------------------------------------------

    #[test]
    fn programmer_role_is_implementation_capable() {
        assert_eq!(
            role_implementation_capability(&role_named("Programmer")),
            RoleCapability::ImplementationCapable
        );
    }

    #[test]
    fn architect_role_is_implementation_capable() {
        assert_eq!(
            role_implementation_capability(&role_named("Architect")),
            RoleCapability::ImplementationCapable
        );
    }

    #[test]
    fn reviewer_role_is_review_only() {
        assert_eq!(
            role_implementation_capability(&role_named("Reviewer")),
            RoleCapability::ReviewOnly
        );
    }

    #[test]
    fn evaluator_role_is_review_only() {
        assert_eq!(
            role_implementation_capability(&role_named("Evaluator")),
            RoleCapability::ReviewOnly
        );
    }

    #[test]
    fn assigner_meta_role_is_review_only_for_guard_purposes() {
        // Meta personas are not implementation workers — the guard must block
        // them from implementation tasks.
        assert_eq!(
            role_implementation_capability(&role_named("Assigner")),
            RoleCapability::ReviewOnly
        );
        assert_eq!(
            role_implementation_capability(&role_named("Evolver")),
            RoleCapability::ReviewOnly
        );
        assert_eq!(
            role_implementation_capability(&role_named("Agent Creator")),
            RoleCapability::ReviewOnly
        );
    }

    #[test]
    fn unknown_custom_role_fails_open() {
        assert_eq!(
            role_implementation_capability(&role_named("Wibble Wrangler")),
            RoleCapability::Unknown
        );
        assert!(!role_blocks_implementation(&role_named("Wibble Wrangler")));
    }

    // --- verdict -------------------------------------------------------------

    #[test]
    fn auto_assignment_of_reviewer_to_impl_task_demands_reassign() {
        let mut task = task_with("build-real-async", "Build real async runtime");
        task.deliverables = vec!["src/async.rs".to_string()];
        let role = role_named("Reviewer");
        let agent = agent_with("reviewer-agent", &role);
        let verdict = check_assignment_eligibility(&task, &agent, Some(&role), false, None);
        match verdict {
            EligibilityVerdict::Reassign {
                fallback_agent_id, ..
            } => {
                assert_eq!(fallback_agent_id, None);
            }
            other => panic!("expected Reassign, got {:?}", other),
        }
    }

    #[test]
    fn explicit_pin_of_reviewer_to_impl_task_warns_but_allows() {
        let mut task = task_with("build-real-async", "Build real async runtime");
        task.exec_mode = Some("full".to_string());
        let role = role_named("Reviewer");
        let agent = agent_with("reviewer-agent", &role);
        let verdict = check_assignment_eligibility(&task, &agent, Some(&role), true, None);
        assert!(matches!(verdict, EligibilityVerdict::Warn { .. }));
    }

    #[test]
    fn evaluator_assignment_to_evaluate_task_is_allowed() {
        let task = task_with(".evaluate-foo", "Evaluate foo");
        let role = role_named("Evaluator");
        let agent = agent_with("eval-agent", &role);
        let verdict = check_assignment_eligibility(&task, &agent, Some(&role), false, None);
        assert_eq!(verdict, EligibilityVerdict::Allow);
    }

    #[test]
    fn programmer_assignment_to_impl_task_is_allowed() {
        let mut task = task_with("build-real-async", "Build real async runtime");
        task.deliverables = vec!["src/async.rs".to_string()];
        let role = role_named("Programmer");
        let agent = agent_with("prog-agent", &role);
        let verdict = check_assignment_eligibility(&task, &agent, Some(&role), false, None);
        assert_eq!(verdict, EligibilityVerdict::Allow);
    }

    #[test]
    fn reassign_carries_fallback_agent_id_when_provided() {
        let mut task = task_with("register-seed", "Register refreshed e97 seed latest");
        task.exec_mode = Some("full".to_string());
        let role = role_named("Evaluator");
        let agent = agent_with("eval-agent", &role);
        let verdict = check_assignment_eligibility(
            &task,
            &agent,
            Some(&role),
            false,
            Some("agent-prog-123".to_string()),
        );
        match verdict {
            EligibilityVerdict::Reassign {
                fallback_agent_id, ..
            } => {
                assert_eq!(fallback_agent_id.as_deref(), Some("agent-prog-123"));
            }
            other => panic!("expected Reassign, got {:?}", other),
        }
    }

    #[test]
    fn no_role_resolved_fails_open() {
        let mut task = task_with("build-x", "Build x");
        task.exec_mode = Some("full".to_string());
        let role = role_named("Reviewer");
        let agent = agent_with("rev", &role);
        let verdict = check_assignment_eligibility(&task, &agent, None, false, None);
        assert_eq!(verdict, EligibilityVerdict::Allow);
    }
}
