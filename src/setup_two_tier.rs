//! The two-tier Pi model plane as surfaced by interactive `wg setup`.
//!
//! Every route-driven deployment resolves its dispatch roles through exactly
//! two routes: the **strong** tier (workers + heavy generative roles) and the
//! **weak** tier (cheap, recoverable one-shots). Interactive setup must ask
//! for both, show the resulting role table before writing, and make it clear
//! the routes stay re-changeable. This module holds that surface's pure core
//! — explainer wording, answer normalization, and the role-table renderer —
//! so it can be unit-tested without a terminal (the `wg` binary's wizard
//! calls into it; see `src/commands/setup.rs`).

use crate::config::{Config, DispatchRole, Tier};
use anyhow::{Context, Result};
use std::fmt::Write as _;

/// The names of the strong-tier roles, as shown in the wizard explainer.
///
/// Strong = workers + heavy generative roles: real implementation work and
/// the correctness/merge gates that must not silently downgrade.
pub const STRONG_ROLE_LIST: &str = "task_agent, creator, merger, evolver, verification";

/// The names of the weak-tier roles, as shown in the wizard explainer.
///
/// Weak = cheap, recoverable one-shots: scoring, routing, and compaction
/// passes whose verdicts are recoverable (re-run or escalate) and whose cost
/// dominates at scale.
pub const WEAK_ROLE_LIST: &str = "evaluator, assigner, flip_inference, flip_comparison, triage, placer, compactor, chat_compactor, coordinator_eval, reviewer";

/// Default strong route offered by the wizard when nothing is configured yet.
pub const DEFAULT_STRONG_ROUTE: &str = "pi:openrouter:z-ai/glm-5.2";

/// Explain the two-tier Pi model plane before the prompts.
///
/// Separated from the dialoguer prompts so the wording is unit-testable
/// without a terminal.
pub fn two_tier_explainer_lines() -> Vec<String> {
    vec![
        "WG drives every dispatch role from two routes; Pi owns login, providers, and models."
            .to_string(),
        "WG stores only the exact routes you enter here.".to_string(),
        String::new(),
        "STRONG tier — workers and heavy generative roles (reasoning: high):".to_string(),
        format!("  {STRONG_ROLE_LIST}"),
        String::new(),
        "WEAK tier — cheap, recoverable one-shots (reasoning: low):".to_string(),
        format!("  {WEAK_ROLE_LIST}"),
        String::new(),
        "Reusing the strong route for the weak tier is valid — single-model".to_string(),
        "deployments keep working. You can change either route any time with".to_string(),
        "`wg config -m <route>` / `wg config --set-model <role> <route>` or".to_string(),
        "`wg profile select <name>`; Pi owns the model plane.".to_string(),
    ]
}

/// Normalize the wizard's strong/weak answers into `(strong, weak)`.
///
/// The weak route is `Some` only when it is a **distinct** valid
/// `pi:<provider>:<model>` route; an empty answer or a repeat of the strong
/// route means "reuse strong" (single-model deployment, no `tiers.fast` key
/// written). Both answers must be exact Pi routes when present.
pub fn resolve_two_tier_answers(
    strong_answer: &str,
    weak_answer: &str,
) -> Result<(String, Option<String>)> {
    let strong = strong_answer.trim();
    crate::config::parse_exact_pi_route(strong).with_context(|| {
        format!("STRONG route '{strong}' is not an exact pi:<provider>:<model> route")
    })?;
    let weak = weak_answer.trim();
    if weak.is_empty() || weak == strong {
        return Ok((strong.to_string(), None));
    }
    crate::config::parse_exact_pi_route(weak).with_context(|| {
        format!("WEAK route '{weak}' is not an exact pi:<provider>:<model> route")
    })?;
    Ok((strong.to_string(), Some(weak.to_string())))
}

/// Render the two-tier role table (like `wg config --models`) for the config
/// the wizard is about to write: which roles resolve to which tier, the exact
/// route each inherits, and the reasoning each gets.
pub fn render_two_tier_role_table(config: &Config) -> Result<String> {
    let strong = config.resolve_tier_route(Tier::Standard)?;
    let weak = config.resolve_tier_route(Tier::Fast)?;
    let mut out = String::new();
    let _ = writeln!(out, "Role routing preview:");
    let _ = writeln!(out, "  effective strong = {}", strong.route);
    let _ = writeln!(out, "  effective weak   = {}", weak.route);
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  {:<18} {:<7} {:<44} {:<9} SOURCE",
        "ROLE", "TIER", "EXACT ROUTE", "REASONING"
    );
    let rows: Vec<DispatchRole> = std::iter::once(DispatchRole::Default)
        .chain(DispatchRole::ALL.iter().copied())
        .collect();
    for role in rows {
        let resolved = config.resolve_execution_route_for_role(role)?;
        // Tier label follows the role's designed tier mapping (the same one
        // the resolver walks): Fast = weak one-shots, Standard/Premium =
        // strong workers + heavy roles.
        let tier = match role.default_tier() {
            Tier::Fast => "weak",
            Tier::Standard | Tier::Premium => "strong",
        };
        let _ = writeln!(
            out,
            "  {:<18} {:<7} {:<44} {:<9} {}",
            role,
            tier,
            resolved.route,
            resolved
                .reasoning
                .map(|value| value.as_str())
                .unwrap_or("(omit)"),
            resolved.source
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ReasoningLevel;
    use crate::config_defaults::{RouteParams, SetupRoute, config_for_route};

    const STRONG: &str = "pi:openrouter:z-ai/glm-5.2";
    const WEAK: &str = "pi:openrouter:deepseek/deepseek-chat";

    /// Build the config the wizard would write from `(strong, weak)`.
    fn wizard_config(strong: &str, weak: Option<&str>) -> Config {
        let mut config = config_for_route(
            SetupRoute::Pi,
            RouteParams {
                model: Some(strong.to_string()),
                ..Default::default()
            },
        );
        if let Some(weak) = weak {
            config.set_pi_tiers(Some(strong), Some(weak));
        }
        config
    }

    #[test]
    fn distinct_models_produce_a_weak_tier() {
        let (strong, weak) = resolve_two_tier_answers(STRONG, WEAK).unwrap();
        assert_eq!(strong, STRONG);
        assert_eq!(weak.as_deref(), Some(WEAK));

        let config = wizard_config(&strong, weak.as_deref());
        assert_eq!(config.tiers.standard.as_deref(), Some(STRONG));
        assert_eq!(config.tiers.fast.as_deref(), Some(WEAK));
        assert_eq!(config.tiers.standard_reasoning, Some(ReasoningLevel::High));
        assert_eq!(config.tiers.fast_reasoning, Some(ReasoningLevel::Low));
        // Workers resolve the strong route; cheap one-shots resolve the weak one.
        let task_agent = config
            .resolve_execution_route_for_role(DispatchRole::TaskAgent)
            .unwrap();
        assert_eq!(task_agent.route, STRONG);
        assert_eq!(task_agent.reasoning, Some(ReasoningLevel::High));
        let evaluator = config
            .resolve_execution_route_for_role(DispatchRole::Evaluator)
            .unwrap();
        assert_eq!(evaluator.route, WEAK);
        assert_eq!(evaluator.reasoning, Some(ReasoningLevel::Low));
        config.validate_pi_model_plane().unwrap();
    }

    #[test]
    fn reuse_strong_when_empty_or_equal_single_model_keeps_working() {
        // Empty answer = "press Enter to reuse strong".
        let (strong, weak) = resolve_two_tier_answers(STRONG, "").unwrap();
        assert_eq!(strong, STRONG);
        assert_eq!(weak, None);
        // Repeating the strong route is a valid single-model answer too.
        let (_, weak) = resolve_two_tier_answers(STRONG, STRONG).unwrap();
        assert_eq!(weak, None);

        let config = wizard_config(STRONG, None);
        assert_eq!(config.tiers.fast, None);
        assert_eq!(config.tiers.standard, None);
        let evaluator = config
            .resolve_execution_route_for_role(DispatchRole::Evaluator)
            .unwrap();
        assert_eq!(evaluator.route, STRONG);
        config.validate_pi_model_plane().unwrap();
    }

    #[test]
    fn non_exact_routes_are_rejected_with_the_tier_named() {
        for bad in [
            "opus",
            "openrouter:anthropic/claude-opus-4-7",
            "pi:onlymodel",
        ] {
            let error = resolve_two_tier_answers(STRONG, bad).unwrap_err();
            assert!(error.to_string().contains("WEAK route"), "{error:#}");
        }
        let error = resolve_two_tier_answers("haiku", "").unwrap_err();
        assert!(error.to_string().contains("STRONG route"), "{error:#}");
    }

    #[test]
    fn role_table_labels_tier_and_reasoning_before_write() {
        let config = wizard_config(STRONG, Some(WEAK));
        let table = render_two_tier_role_table(&config).unwrap();
        assert!(
            table.contains("effective strong = pi:openrouter:z-ai/glm-5.2"),
            "{table}"
        );
        assert!(
            table.contains("effective weak   = pi:openrouter:deepseek/deepseek-chat"),
            "{table}"
        );
        for line in table.lines().filter(|l| l.contains("task_agent")) {
            assert!(line.contains("strong"), "{line}");
            assert!(line.contains("high"), "{line}");
        }
        for line in table
            .lines()
            .filter(|l| l.contains("evaluator") || l.contains("triage"))
        {
            assert!(line.contains("weak"), "{line}");
            assert!(line.contains("low"), "{line}");
        }
    }

    #[test]
    fn role_table_for_single_model_shows_weak_roles_inheriting_strong_route() {
        let config = wizard_config(STRONG, None);
        let table = render_two_tier_role_table(&config).unwrap();
        // Single-model: the weak-tier roles inherit the strong route.
        let evaluator = table
            .lines()
            .find(|l| l.contains("evaluator"))
            .expect("evaluator row");
        assert!(evaluator.contains("weak"), "{evaluator}");
        assert!(evaluator.contains(STRONG), "{evaluator}");
        let task_agent = table
            .lines()
            .find(|l| l.contains("task_agent"))
            .expect("task_agent row");
        assert!(task_agent.contains("strong"), "{task_agent}");
        assert!(task_agent.contains(STRONG), "{task_agent}");
    }

    #[test]
    fn explainer_names_roles_and_re_changeability() {
        let text = two_tier_explainer_lines().join("\n");
        assert!(text.contains(STRONG_ROLE_LIST), "{text}");
        assert!(text.contains("evaluator, assigner"), "{text}");
        assert!(
            text.contains("Reusing the strong route for the weak tier is valid"),
            "{text}"
        );
        assert!(text.contains("wg profile select"), "{text}");
        assert!(text.contains("wg config"), "{text}");
        assert!(text.contains("Pi owns the model plane"), "{text}");
    }
}
