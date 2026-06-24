//! Selecting which agent (retry attempt) a per-task view should display.
//!
//! A task that is retried spawns a NEW agent for each attempt while its log
//! accumulates entries from *every* attempt. The oldest entries belong to the
//! FIRST (often failed) attempt, the newest to the live one. A naive "first
//! `agent-*` mentioned in the log" pick therefore sticks a per-task view (the
//! TUI Log pane, `wg show` tail, …) on the failed attempt long after the
//! dispatcher has moved on to a live retry agent — the task looks "stuck on the
//! old log" even though the graph is progressing. See `fix-tui-retry-log`.
//!
//! This module centralises the ordering + default-selection so a per-task view
//! follows the CURRENT / latest / alive agent **by default**, while still
//! letting the user manually pin (and cycle to) an older attempt.

use chrono::{DateTime, Utc};

/// Result of resolving which attempt agent a per-task view should show.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AttemptSelection {
    /// Every attempt agent id for the task, **oldest attempt first**, newest
    /// last. Drives the manual prev/next switcher and the "attempt N/M" label.
    pub ordered: Vec<String>,
    /// The agent id the view should display by default.
    pub selected: Option<String>,
}

/// Sort key that orders attempts chronologically (oldest → newest).
///
/// WG mints agent ids from a monotonically increasing counter
/// (`agent-275`, `agent-280`, …), so a larger trailing integer means a later
/// (newer) attempt. Ids without a parseable trailing integer sort *after* all
/// numbered ids, deterministically by the full string, so unusual ids never
/// crash the ordering — they just land at the end.
fn attempt_order_key(agent_id: &str) -> (u64, &str) {
    let suffix = agent_id
        .rsplit('-')
        .next()
        .and_then(|s| s.parse::<u64>().ok());
    (suffix.unwrap_or(u64::MAX), agent_id)
}

fn push_unique(ordered: &mut Vec<String>, id: &str) {
    if !id.is_empty() && !ordered.iter().any(|x| x == id) {
        ordered.push(id.to_string());
    }
}

/// Resolve the attempt agent list + default selection for a task.
///
/// * `log_mentioned` — agent ids referenced in the task log, in the order they
///   appear (chronological). May contain duplicates; they are de-duplicated.
/// * `registry_ids` — agent ids the agent registry currently associates with
///   this task (covers agents that have not yet written a log entry).
/// * `assigned` — the task's current `assigned` agent, if any. This is the
///   live/current attempt and is the default selection.
/// * `manual_pin` — an agent id the user explicitly cycled to; it overrides the
///   default selection as long as it is still one of the candidates.
///
/// Selection precedence: a still-valid `manual_pin` wins; otherwise the
/// `assigned` (live/current) agent; otherwise the newest attempt.
pub fn select_attempt_agent(
    log_mentioned: &[String],
    registry_ids: &[String],
    assigned: Option<&str>,
    manual_pin: Option<&str>,
) -> AttemptSelection {
    // Gather unique candidates from every source.
    let mut ordered: Vec<String> = Vec::new();
    for id in log_mentioned {
        push_unique(&mut ordered, id);
    }
    for id in registry_ids {
        push_unique(&mut ordered, id);
    }
    if let Some(a) = assigned {
        push_unique(&mut ordered, a);
    }

    // Order oldest → newest by the agent-id counter.
    ordered.sort_by(|a, b| attempt_order_key(a).cmp(&attempt_order_key(b)));

    let in_set = |id: &str| ordered.iter().any(|x| x == id);
    let selected = manual_pin
        .filter(|p| in_set(p))
        .map(str::to_string)
        .or_else(|| assigned.filter(|a| in_set(a)).map(str::to_string))
        .or_else(|| ordered.last().cloned());

    AttemptSelection { ordered, selected }
}

/// Extract the first `agent-<digits>` identifier appearing anywhere in archived
/// executor output.
///
/// A per-attempt archive (`.wg/log/agents/<task>/<ts>/output.txt`) holds the
/// raw executor stream. For the claude CLI its first record is a COMPACT JSON
/// line (no inter-token whitespace) whose `cwd` field embeds the attempt's
/// worktree path, e.g. `"cwd":"…/.wg-worktrees/agent-5740"`. A
/// whitespace-tokenising scan (`split_whitespace().find(|w|
/// w.starts_with("agent-"))`) never matches that — the id is inside a quoted
/// JSON field, not a standalone token — so `wg show`'s attempt history rendered
/// NO agent attribution at all (see task `rshow`).
///
/// This scans the raw text for the literal `agent-` followed by ≥1 ASCII digit
/// and returns the first hit. It is executor-agnostic (any output naming the
/// agent dir / worktree resolves) and serves only as a *fallback* — the
/// archive's authoritative `agent-id` file is preferred when present.
pub fn extract_agent_id_from_output(content: &str) -> Option<String> {
    const NEEDLE: &str = "agent-";
    let bytes = content.as_bytes();
    let mut search_from = 0;
    while let Some(rel) = content[search_from..].find(NEEDLE) {
        let start = search_from + rel;
        let digits_start = start + NEEDLE.len();
        let digit_len = bytes[digits_start..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if digit_len > 0 {
            return Some(content[start..digits_start + digit_len].to_string());
        }
        // No digits followed this `agent-`; resume scanning past it.
        search_from = digits_start.max(start + 1);
    }
    None
}

/// Attribute each evaluation to the attempt it actually scored.
///
/// `attempt_timestamps` are the archived attempts' timestamps (RFC 3339, oldest
/// first — the archive dir names). `eval_timestamps` are the recorded
/// evaluation timestamps for the same task. Returns, parallel to
/// `attempt_timestamps`, the index into `eval_timestamps` of the evaluation
/// belonging to each attempt, or `None` when no eval scored that attempt.
///
/// An eval scores the most-recent attempt that existed when it ran, so it is
/// attributed to the latest attempt whose timestamp is ≤ the eval's timestamp
/// (an eval predating every attempt falls to the earliest attempt, so none are
/// silently dropped). When several evals map to one attempt, the latest wins.
/// This replaces the prior "show the single newest eval against EVERY attempt"
/// behaviour, which made a retried attempt 1 display attempt 2's score.
pub fn attribute_evals_to_attempts(
    attempt_timestamps: &[String],
    eval_timestamps: &[String],
) -> Vec<Option<usize>> {
    let mut result: Vec<Option<usize>> = vec![None; attempt_timestamps.len()];
    if attempt_timestamps.is_empty() {
        return result;
    }
    let parse = |s: &str| {
        DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&Utc))
    };
    let attempts: Vec<Option<DateTime<Utc>>> =
        attempt_timestamps.iter().map(|s| parse(s)).collect();

    for (ei, ets) in eval_timestamps.iter().enumerate() {
        let Some(et) = parse(ets) else { continue };
        // Latest attempt whose timestamp ≤ eval time; default to the earliest
        // attempt (index 0) when the eval predates all of them.
        let mut target = 0usize;
        for (i, at) in attempts.iter().enumerate() {
            if let Some(at) = at
                && *at <= et
            {
                target = i;
            }
        }
        // Keep the latest eval when several land on the same attempt.
        let replace = match result[target] {
            None => true,
            Some(prev) => match parse(&eval_timestamps[prev]) {
                Some(pt) => et >= pt,
                None => true,
            },
        };
        if replace {
            result[target] = Some(ei);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reported bug: after a fail→retry the log mentions the failed agent
    /// FIRST, but the view must default to the live (assigned) agent.
    #[test]
    fn retry_defaults_to_live_assigned_not_first_log_mention() {
        let log = vec!["agent-275".to_string(), "agent-280".to_string()];
        let registry = vec!["agent-275".to_string(), "agent-280".to_string()];
        let sel = select_attempt_agent(&log, &registry, Some("agent-280"), None);
        assert_eq!(sel.ordered, vec!["agent-275", "agent-280"]);
        assert_eq!(
            sel.selected.as_deref(),
            Some("agent-280"),
            "must follow the live/assigned agent, not the first (failed) log mention"
        );
    }

    /// Ordering is by the numeric counter, not by log-mention or string order,
    /// so e.g. agent-99 (older) sorts before agent-280 (newer).
    #[test]
    fn orders_attempts_by_numeric_counter() {
        // Deliberately scrambled input order; agent-9 < agent-99 < agent-280.
        let log = vec!["agent-280".to_string(), "agent-9".to_string()];
        let registry = vec!["agent-99".to_string()];
        let sel = select_attempt_agent(&log, &registry, Some("agent-280"), None);
        assert_eq!(sel.ordered, vec!["agent-9", "agent-99", "agent-280"]);
        assert_eq!(sel.selected.as_deref(), Some("agent-280"));
    }

    /// A still-valid manual pin overrides the live default so the user can
    /// inspect an older attempt without it snapping back on the next refresh.
    #[test]
    fn manual_pin_overrides_live_default() {
        let log = vec!["agent-275".to_string(), "agent-280".to_string()];
        let registry = vec!["agent-280".to_string()];
        let sel = select_attempt_agent(&log, &registry, Some("agent-280"), Some("agent-275"));
        assert_eq!(sel.selected.as_deref(), Some("agent-275"));
    }

    /// A stale manual pin (agent no longer a candidate) is ignored and the
    /// view falls back to the live default.
    #[test]
    fn stale_manual_pin_is_ignored() {
        let log = vec!["agent-280".to_string()];
        let registry = vec!["agent-280".to_string()];
        let sel = select_attempt_agent(&log, &registry, Some("agent-280"), Some("agent-1"));
        assert_eq!(sel.selected.as_deref(), Some("agent-280"));
    }

    /// During the brief window after a retry reset but before the new agent
    /// claims (`assigned` is None), the view falls back to the newest attempt.
    #[test]
    fn no_assigned_falls_back_to_newest_attempt() {
        let log = vec!["agent-275".to_string()];
        let registry = vec!["agent-275".to_string(), "agent-280".to_string()];
        let sel = select_attempt_agent(&log, &registry, None, None);
        assert_eq!(sel.selected.as_deref(), Some("agent-280"));
    }

    /// No candidates at all → no selection (empty pane, not a panic).
    #[test]
    fn empty_inputs_select_nothing() {
        let sel = select_attempt_agent(&[], &[], None, None);
        assert!(sel.ordered.is_empty());
        assert_eq!(sel.selected, None);
    }

    /// Single attempt → that agent, regardless of source.
    #[test]
    fn single_attempt_selects_it() {
        let sel = select_attempt_agent(&[], &["agent-7".to_string()], None, None);
        assert_eq!(sel.ordered, vec!["agent-7"]);
        assert_eq!(sel.selected.as_deref(), Some("agent-7"));
    }

    /// Ids without a numeric suffix sort last but never crash the ordering.
    #[test]
    fn non_numeric_ids_sort_last_deterministically() {
        let log = vec!["coordinator".to_string(), "agent-5".to_string()];
        let sel = select_attempt_agent(&log, &[], None, None);
        assert_eq!(sel.ordered, vec!["agent-5", "coordinator"]);
        assert_eq!(sel.selected.as_deref(), Some("coordinator"));
    }

    /// The reported `wg show` bug: the agent id is buried inside the COMPACT
    /// JSON `cwd` field of the init record (no surrounding whitespace), so a
    /// whitespace-split scan finds nothing. The substring scanner must recover
    /// it.
    #[test]
    fn extracts_agent_id_from_compact_json_cwd() {
        let init = r#"{"type":"system","subtype":"init","cwd":"/home/bot/wg/.wg-worktrees/agent-5740","session_id":"abc"}"#;
        assert_eq!(
            extract_agent_id_from_output(init).as_deref(),
            Some("agent-5740")
        );
    }

    /// Plain whitespace-delimited mentions still resolve.
    #[test]
    fn extracts_agent_id_from_plain_text() {
        assert_eq!(
            extract_agent_id_from_output("spawned agent-12 for the task").as_deref(),
            Some("agent-12")
        );
    }

    /// The FIRST `agent-<n>` wins (the init line names the running agent first).
    #[test]
    fn extracts_first_agent_id_when_several_present() {
        let out = "cwd .wg-worktrees/agent-5740 ... later note about agent-99";
        assert_eq!(
            extract_agent_id_from_output(out).as_deref(),
            Some("agent-5740")
        );
    }

    /// `agent-` with no trailing digits is skipped, not mis-returned; the real
    /// numbered id later in the text is found.
    #[test]
    fn skips_agent_dash_without_digits() {
        assert_eq!(
            extract_agent_id_from_output("the agent-registry holds agent-7").as_deref(),
            Some("agent-7")
        );
        assert_eq!(extract_agent_id_from_output("no ids here").as_deref(), None);
        assert_eq!(extract_agent_id_from_output("agent-").as_deref(), None);
    }

    /// The headline eval-attribution bug: a 2-attempt task whose single eval ran
    /// after both archives must attach ONLY to attempt 2 — never echo the same
    /// score onto the retried attempt 1.
    #[test]
    fn eval_attributes_to_the_attempt_it_scored() {
        let attempts = vec![
            "2026-06-24T13:43:03Z".to_string(),
            "2026-06-24T14:13:01Z".to_string(),
        ];
        let evals = vec!["2026-06-24T14:14:34.325169408+00:00".to_string()];
        assert_eq!(
            attribute_evals_to_attempts(&attempts, &evals),
            vec![None, Some(0)],
            "the single post-retry eval belongs to attempt 2 only"
        );
    }

    /// Each attempt with its own eval gets its own score.
    #[test]
    fn eval_attributes_per_attempt_window() {
        let attempts = vec![
            "2026-06-24T13:00:00Z".to_string(),
            "2026-06-24T14:00:00Z".to_string(),
        ];
        let evals = vec![
            "2026-06-24T13:30:00Z".to_string(), // scored attempt 1
            "2026-06-24T14:30:00Z".to_string(), // scored attempt 2
        ];
        assert_eq!(
            attribute_evals_to_attempts(&attempts, &evals),
            vec![Some(0), Some(1)]
        );
    }

    /// Several evals on one attempt → the latest one wins; the other attempt is
    /// untouched.
    #[test]
    fn eval_keeps_latest_when_multiple_map_to_one_attempt() {
        let attempts = vec![
            "2026-06-24T13:00:00Z".to_string(),
            "2026-06-24T14:00:00Z".to_string(),
        ];
        let evals = vec![
            "2026-06-24T14:10:00Z".to_string(),
            "2026-06-24T14:20:00Z".to_string(), // later — wins for attempt 2
        ];
        assert_eq!(
            attribute_evals_to_attempts(&attempts, &evals),
            vec![None, Some(1)]
        );
    }

    /// An eval predating every attempt falls to the earliest attempt rather than
    /// being dropped.
    #[test]
    fn eval_predating_all_attempts_falls_to_earliest() {
        let attempts = vec!["2026-06-24T13:00:00Z".to_string()];
        let evals = vec!["2026-06-24T12:00:00Z".to_string()];
        assert_eq!(
            attribute_evals_to_attempts(&attempts, &evals),
            vec![Some(0)]
        );
    }

    /// No archived attempts → empty result (the live attempt is rendered
    /// separately and carries no historical eval).
    #[test]
    fn eval_attribution_with_no_attempts_is_empty() {
        assert!(attribute_evals_to_attempts(&[], &["2026-06-24T12:00:00Z".to_string()]).is_empty());
    }
}
