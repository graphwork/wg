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
}
