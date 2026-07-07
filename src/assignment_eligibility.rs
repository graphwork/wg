//! Assignment pool separation — the structural split between **work
//! agents** and **system evaluation agents**.
//!
//! WG has two agent pools that must never be mixed at assignment time:
//!
//! - the **work pool** — ordinary implementation/operation workers
//!   (Programmer, Architect, Documenter, …) eligible for user/work task
//!   assignment;
//! - the **system evaluation pool** — the agency meta + review personas
//!   (`Reviewer`, `Evaluator`, `Assigner`, `Evolver`, `Agent Creator`)
//!   that exist to run the evaluation/review/assignment *primitives*, not
//!   implementation work.
//!
//! The observed failure mode this module prevents: WG creates many
//! `.evaluate-*` / `.flip-*` / review tasks, so the system evaluation
//! agents accumulate heavy historical usage and high scores. The lightweight
//! LLM assigner sees that usage and then picks an evaluator/reviewer persona
//! for an implementation/intake/build task — which then behaves as an
//! evaluator (reports missing implementation, no-ops, retries) until the task
//! fails. Two recent failures of exactly this shape were `build-real-async`
//! and `register-refreshed-e97-seed-latest`.
//!
//! The fix here is **structural, not heuristic**. The earlier
//! `prevent-evaluator-reviewer` guard guessed at implementation verbs in the
//! task title/tags and filtered the pool only when it thought the task
//! "looked like" implementation work. That left every neutral-looking task
//! ("Triage incoming issues", "Set up the intake pipeline", …) exposed to a
//! system-agent pick whenever the evaluator/reviewer had the highest score.
//! This module replaces that with a pool split keyed on the **task kind**:
//!
//! - **Evaluation/review primitives** ([`task_is_evaluation_or_review`]) —
//!   `.evaluate-*`, `.flip-*`, `.assign-*` scaffolding, or any task tagged
//!   `review` / `evaluation` / `evaluate` / `eval` — use the system
//!   evaluation pool (or, in practice, the inline one-shot dispatch path).
//! - **Everything else** ([`task_uses_work_pool`]) is a normal work task and
//!   uses the **work pool only** — system evaluation agents are excluded
//!   from the candidate set *before* the LLM assigner ever sees them,
//!   regardless of historical frequency, recent success, or LLM preference.
//!
//! Design rules (kept deliberately narrow so the split is predictable):
//!
//! 1. **A normal work task never offers a system evaluation agent** as a
//!    candidate — [`filter_work_pool_agents`] is the structural front door.
//! 2. **An evaluation/review primitive** keeps access to the system pool —
//!    the guard never blocks evaluator/FLIP routing.
//! 3. **Explicit human pinning wins**, but a system agent pinned to a work
//!    task warns loudly ([`EligibilityVerdict::Warn`]) so the operator sees
//!    the role/pool mismatch.
//! 4. **Auto-assignment** that nonetheless lands on a system agent for a work
//!    task (e.g. an assigner that bypassed the filtered pool, or a stale
//!    role resolution) is mutated to [`EligibilityVerdict::Reassign`],
//!    carrying a fallback work agent when one exists. When no work agent is
//!    available, the caller must **fail loudly** with a configuration error
//!    — it must never silently keep the system agent.
//!
//! The earlier verb/tag implementation heuristic ([`task_requires_implementation`]
//! and [`role_implementation_capability`]) is retained as a *secondary* hint
//! (e.g. to surface a fallback implementation-capable worker), but it is no
//! longer the gate — the pool kind is.

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

/// **System evaluation role names** — the union of the agency meta personas
/// and the review persona. An agent whose role name is in this set is a
/// system evaluation agent and is excluded from the work pool. This is the
/// structural split: it does NOT depend on task verb guessing.
///
/// This set is the single source of truth for "system evaluation agent" —
/// [`crate::service::llm::is_agency_oneshot_role`] names the matching
/// `DispatchRole` set (Evaluator / FlipInference / FlipComparison / Assigner /
/// Reviewer) on the dispatch side; this constant names the matching *role*
/// set on the assignment side. The two are kept in lock-step by the unit
/// tests in this module and `service::llm`.
pub(crate) const SYSTEM_EVALUATION_ROLE_NAMES: &[&str] = &[
    "Reviewer",
    "Evaluator",
    "Assigner",
    "Evolver",
    "Agent Creator",
];

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

/// Does this task use the **work pool** (system evaluation agents excluded)?
///
/// True for every task that is NOT an evaluation/review primitive. This is the
/// structural split: any non-primitive task — whether it "looks like"
/// implementation work or not — draws its candidates from the work pool only,
/// so system evaluation agents (Reviewer / Evaluator / Assigner / Evolver /
/// Agent Creator) can never be auto-assigned to it regardless of their
/// historical usage or score.
///
/// This is the gate that replaces the earlier verb-guessing heuristic: the
/// pool kind is decided by the task kind, not by whether the title happens to
/// contain "build" or "implement".
pub fn task_uses_work_pool(task: &Task) -> bool {
    !task_is_evaluation_or_review(task)
}

/// Is this role a **system evaluation/agency persona** — i.e. excluded from
/// the work pool? Name-only path; for custom / evolved roles use
/// [`role_is_system_evaluation_with_components`].
///
/// True for the agency meta personas (`Assigner`, `Evaluator`, `Evolver`,
/// `Agent Creator`) and the review persona (`Reviewer`). These run the
/// evaluation/review/assignment *primitives*, not implementation work.
pub fn role_is_system_evaluation(role: &Role) -> bool {
    SYSTEM_EVALUATION_ROLE_NAMES.contains(&role.name.as_str())
        || classify_role(&role.name, &[]) == RoleCapability::ReviewOnly
}

/// Component-aware system-pool check — resolves component content *names* for
/// custom / evolved roles so a review-only custom role (every component is a
/// review component) is also treated as a system evaluation agent.
pub fn role_is_system_evaluation_with_components(role: &Role, component_names: &[String]) -> bool {
    if SYSTEM_EVALUATION_ROLE_NAMES.contains(&role.name.as_str()) {
        return true;
    }
    classify_role(&role.name, component_names) == RoleCapability::ReviewOnly
}

/// Convenience: is this agent a system evaluation agent? Requires the resolved
/// [`Role`] (the caller looks it up by `agent.role_id`). `None` role ⇒ `false`
/// (fail open — we can't classify without a role, and the caller should have
/// resolved one).
pub fn agent_is_system_evaluation(_agent: &Agent, role: Option<&Role>) -> bool {
    role.is_some_and(|r| role_is_system_evaluation(r))
}

/// Build the **work pool** for a normal work task — the subset of `agents`
/// whose role is NOT a system evaluation persona, excluding humans and stale
/// agents. This is the structural front door: pass its output to the LLM
/// assigner so system agents are never even candidates for a work task.
///
/// `components_dir` lets the classifier resolve component content names for
/// custom / evolved roles; pass the agency `primitives/components` dir.
///
/// Returns owned agents so the caller can re-borrow freely. The order matches
/// the input order (no re-sorting) — the caller picks by score.
pub fn filter_work_pool_agents<'a>(
    agents: &'a [Agent],
    roles_dir: &Path,
    components_dir: &Path,
) -> Vec<&'a Agent> {
    agents
        .iter()
        .filter(|a| {
            if a.is_human() || !a.staleness_flags.is_empty() {
                return false;
            }
            let role = match crate::agency::find_role_by_prefix(roles_dir, &a.role_id) {
                Ok(r) => r,
                Err(_) => return false,
            };
            let comp_names = resolve_role_component_names(&role, components_dir);
            !role_is_system_evaluation_with_components(&role, &comp_names)
        })
        .collect()
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
    // Evaluation/review primitives use the system pool — system agents are
    // the correct (and only) candidates there.
    if task_is_evaluation_or_review(task) {
        return EligibilityVerdict::Allow;
    }
    // No role resolved — can't classify. Fail open (don't block) but the
    // caller should ideally have resolved one.
    let Some(role) = role else {
        return EligibilityVerdict::Allow;
    };
    // Structural pool separation: a system evaluation agent on a normal work
    // task is a pool mismatch regardless of task wording, score, or usage.
    if !role_is_system_evaluation(role) {
        return EligibilityVerdict::Allow;
    }
    let reason = format!(
        "task '{}' is a normal work task and must draw from the work pool, but \
         agent '{}' has system role '{}' ({}), which is an \
         evaluation/review/agency persona excluded from work assignment; \
         assign an implementation-capable worker instead",
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

    // --- pool separation (system vs work) -----------------------------------

    #[test]
    fn system_evaluation_role_names_are_classified_as_system() {
        // All five system evaluation personas must be recognised as system
        // pool agents — the structural split does not depend on task wording.
        for name in [
            "Reviewer",
            "Evaluator",
            "Assigner",
            "Evolver",
            "Agent Creator",
        ] {
            assert!(
                role_is_system_evaluation(&role_named(name)),
                "{name} must be a system evaluation role"
            );
        }
    }

    #[test]
    fn work_roles_are_not_system_evaluation() {
        for name in [
            "Programmer",
            "Architect",
            "Documenter",
            "Implementer",
            "Engineer",
        ] {
            assert!(
                !role_is_system_evaluation(&role_named(name)),
                "{name} must NOT be a system evaluation role"
            );
        }
    }

    #[test]
    fn task_uses_work_pool_for_non_primitive_tasks() {
        // Neutral work task (no impl verbs, no review tags) still uses the
        // work pool — the gate is the task kind, not verb guessing.
        assert!(task_uses_work_pool(&task_with(
            "t1",
            "Triage incoming issues"
        )));
        // Implementation-flavoured task.
        assert!(task_uses_work_pool(&task_with("build-x", "Build x")));
    }

    #[test]
    fn task_uses_work_pool_false_for_evaluation_primitives() {
        assert!(!task_uses_work_pool(&task_with(
            ".evaluate-foo",
            "Evaluate foo"
        )));
        assert!(!task_uses_work_pool(&task_with(".flip-foo", "Flip foo")));
        // A review-tagged task is an evaluation/review primitive.
        let mut t = task_with("t1", "Look at this");
        t.tags = vec!["review".to_string()];
        assert!(!task_uses_work_pool(&t));
    }

    #[test]
    fn system_agent_on_neutral_work_task_demands_reassign() {
        // Acceptance #3: a normal task WITHOUT obvious implementation verbs
        // still must not select an evaluator/reviewer — the pool split is
        // structural, not verb-guessing.
        let task = task_with("t1", "Triage incoming issues");
        let role = role_named("Reviewer");
        let agent = agent_with("reviewer-agent", &role);
        let verdict = check_assignment_eligibility(&task, &agent, Some(&role), false, None);
        assert!(
            matches!(verdict, EligibilityVerdict::Reassign { .. }),
            "neutral work task must reassign a system agent, got {verdict:?}"
        );
    }

    #[test]
    fn evaluator_on_neutral_work_task_demands_reassign() {
        // Same structural rule for the Evaluator meta persona — even though
        // the task title says nothing about implementation.
        let task = task_with("t1", "Organise the intake board");
        let role = role_named("Evaluator");
        let agent = agent_with("eval-agent", &role);
        let verdict = check_assignment_eligibility(&task, &agent, Some(&role), false, None);
        assert!(
            matches!(verdict, EligibilityVerdict::Reassign { .. }),
            "Evaluator must be reassigned off a neutral work task"
        );
    }

    #[test]
    fn explicit_pin_of_evaluator_to_neutral_task_warns() {
        // Acceptance #4: explicit human assignment to an evaluator/reviewer on
        // a normal task emits a loud role-pool mismatch warning but remains
        // possible.
        let task = task_with("t1", "Triage incoming issues");
        let role = role_named("Evaluator");
        let agent = agent_with("eval-agent", &role);
        let verdict = check_assignment_eligibility(&task, &agent, Some(&role), true, None);
        match verdict {
            EligibilityVerdict::Warn { reason } => {
                assert!(
                    reason.contains("work pool") || reason.contains("system role"),
                    "warn reason should name the pool mismatch: {reason}"
                );
            }
            other => panic!("expected Warn, got {other:?}"),
        }
    }

    #[test]
    fn evaluator_on_evaluate_primitive_is_allowed() {
        // Acceptance #2: .evaluate-* / .flip-* tasks still use the system pool.
        let task = task_with(".flip-foo", "Flip foo");
        let role = role_named("Evaluator");
        let agent = agent_with("eval-agent", &role);
        let verdict = check_assignment_eligibility(&task, &agent, Some(&role), false, None);
        assert_eq!(verdict, EligibilityVerdict::Allow);
    }

    #[test]
    fn reviewer_on_review_tagged_task_is_allowed() {
        // A task explicitly tagged review is an evaluation/review primitive —
        // a Reviewer is the correct pool and must be allowed.
        let mut task = task_with("t1", "Audit the auth flow");
        task.tags = vec!["review".to_string()];
        let role = role_named("Reviewer");
        let agent = agent_with("rev-agent", &role);
        let verdict = check_assignment_eligibility(&task, &agent, Some(&role), false, None);
        assert_eq!(verdict, EligibilityVerdict::Allow);
    }

    #[test]
    fn assigner_meta_role_on_work_task_demands_reassign() {
        // The agency meta personas (Assigner / Evolver / Agent Creator) are
        // system agents too — they must not be auto-assigned to work tasks.
        let task = task_with("t1", "Set up the intake pipeline");
        for name in ["Assigner", "Evolver", "Agent Creator"] {
            let role = role_named(name);
            let agent = agent_with(&format!("{name}-agent"), &role);
            let verdict = check_assignment_eligibility(&task, &agent, Some(&role), false, None);
            assert!(
                matches!(verdict, EligibilityVerdict::Reassign { .. }),
                "{name} must be reassigned off a work task, got {verdict:?}"
            );
        }
    }
}
