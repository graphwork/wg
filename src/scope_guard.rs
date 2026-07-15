//! R8 disposable scope guard.
//!
//! A task created with `wg add --scope disposable` runs its agent with
//! `WG_SCOPE=disposable` in the environment (the dispatcher's [`SpawnPlan`]
//! propagates it — see `dispatch::plan`). A disposable-scoped agent must not be
//! able to mint durable/persistent graph state: it may not run `wg agent
//! create`, `wg add --tag persistent`, or an ordinary durable `wg add`.
//!
//! This is the policy gate for R8 (docs/02 §3.2 in the family-team project):
//! the action is structurally possible today — any agent can call `wg agent
//! create` with any role, or mint durable follow-up work with `wg add` — so we
//! deny it at the CLI boundary when the caller is disposable-scoped. The `wg
//! add` boundary is **default-deny**: from disposable scope the only allowed
//! add is *explicitly* disposable child work (`--scope disposable` /
//! `--tag disposable`); every other add is refused (see
//! [`resolve_add_scope_for`]). Only the reserved scope value `disposable`
//! restricts anything; every other value (including unscoped) is unaffected.
//!
//! [`SpawnPlan`]: crate::dispatch::plan::SpawnPlan

use anyhow::{Result, bail};

/// Environment variable the dispatcher sets on a scoped worker and the guard
/// reads back at the CLI boundary.
pub const WG_SCOPE_ENV: &str = "WG_SCOPE";

/// The one scope value that is actually restricted today.
pub const SCOPE_DISPOSABLE: &str = "disposable";

/// Tag prefix used to persist a task's scope on the task itself
/// (e.g. `scope:disposable`). Kept as a tag so no schema field is added.
pub const SCOPE_TAG_PREFIX: &str = "scope:";

/// A privileged spawn a disposable-scoped agent is forbidden from performing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistentSpawn {
    /// `wg agent create` — mints a persistent named agent/persona.
    Agent,
    /// `wg add --tag persistent` — creates a task tagged `persistent`.
    Task,
}

impl PersistentSpawn {
    /// The user-facing command this action corresponds to.
    fn command(self) -> &'static str {
        match self {
            PersistentSpawn::Agent => "wg agent create",
            PersistentSpawn::Task => "wg add --tag persistent",
        }
    }
}

/// Extract a scope value from a task's tags (`scope:<value>`), if present.
///
/// The first `scope:` tag wins. Returns `None` when the task is unscoped.
pub fn scope_from_tags(tags: &[String]) -> Option<String> {
    tags.iter()
        .find_map(|t| t.strip_prefix(SCOPE_TAG_PREFIX))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Pure policy check: does `scope` forbid `action`?
///
/// Only `scope == Some("disposable")` restricts anything; every other value
/// (including `None`) is allowed. Kept pure — the scope is passed in rather than
/// read from the environment — so it is deterministic under parallel tests.
pub fn check_scope(scope: Option<&str>, action: PersistentSpawn) -> Result<()> {
    if scope == Some(SCOPE_DISPOSABLE) {
        bail!(
            "scope=disposable forbids `{}`: a disposable agent may not mint a persistent \
             persona (R8). Have a persistent agent perform this action, or drop \
             `--scope disposable`.",
            action.command()
        );
    }
    Ok(())
}

/// Read the current process scope from `WG_SCOPE` (empty/whitespace ⇒ unscoped).
pub fn current_scope() -> Option<String> {
    std::env::var(WG_SCOPE_ENV)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Enforce the scope policy for `action` against the current process
/// environment. Called at the `wg agent create` CLI boundary.
pub fn enforce(action: PersistentSpawn) -> Result<()> {
    check_scope(current_scope().as_deref(), action)
}

/// Resolve the tag set for a `wg add`, enforcing the R8 disposable boundary as
/// a **default-deny** policy against the current process environment.
///
/// See [`resolve_add_scope_for`] for the rule; this wrapper simply reads the
/// caller's scope from `WG_SCOPE`. Called at the `wg add` CLI boundary.
pub fn resolve_add_scope(tags: &[String]) -> Result<Vec<String>> {
    resolve_add_scope_for(current_scope().as_deref(), tags)
}

/// Pure core of [`resolve_add_scope`] — the caller's scope is passed in rather
/// than read from the environment, so it is deterministic under parallel tests.
///
/// When the caller is **not** disposable-scoped the tags are returned verbatim.
///
/// When the caller **is** `disposable`-scoped the policy is **default-deny**:
/// from disposable scope the *only* thing that may be created is **explicitly
/// disposable / non-durable child work**. Concretely:
///
///   * an explicit child `--scope disposable` (a `scope:disposable` tag), or an
///     explicit `disposable` tag, is **allowed** verbatim — a disposable agent
///     may spawn a disposable child;
///   * an explicit `persistent` tag, or an explicit `scope:<x>` naming any scope
///     other than `disposable`, is **denied** — a disposable agent may not
///     escalate a child into durable / persistent graph state; and
///   * an ordinary untagged durable `wg add "x"` (the case Erik flagged on
///     PR #56 — minting durable follow-up work simply by omitting the tag) is
///     **denied**. It is *not* silently downgraded: the merge plan requires
///     that only *explicitly* disposable child work be allowed, so the caller
///     must opt in with `--scope disposable` (or `--tag disposable`).
///
/// The returned `Vec` is the tag set that should be persisted on the new task.
pub fn resolve_add_scope_for(caller_scope: Option<&str>, tags: &[String]) -> Result<Vec<String>> {
    // Only the reserved `disposable` scope constrains anything.
    if caller_scope != Some(SCOPE_DISPOSABLE) {
        return Ok(tags.to_vec());
    }

    // Explicit persistent tag → hard deny (the original R8 case).
    if tags.iter().any(|t| t == "persistent") {
        check_scope(caller_scope, PersistentSpawn::Task)?;
    }

    // An explicit child `--scope` is either the allowed disposable case or an
    // escalation to deny.
    if let Some(child_scope) = scope_from_tags(tags) {
        if child_scope != SCOPE_DISPOSABLE {
            bail!(
                "scope=disposable forbids `wg add --scope {child_scope}`: a disposable \
                 agent may only create disposable-scoped children (R8). Drop the \
                 `--scope {child_scope}` override, or have a persistent agent create \
                 durable work."
            );
        }
        // Child is explicitly disposable-scoped — allowed, verbatim.
        return Ok(tags.to_vec());
    }

    // A child explicitly tagged `disposable` (non-durable) is allowed, verbatim.
    if tags.iter().any(|t| t == "disposable") {
        return Ok(tags.to_vec());
    }

    // Ordinary untagged durable `wg add "x"` from disposable scope: DENY.
    // This is Erik's blocking case — a disposable worker must not mint durable
    // follow-up graph work by omitting the tag. Only *explicitly* disposable
    // child work is allowed from disposable scope; the caller must opt in.
    bail!(
        "scope=disposable forbids a durable `wg add`: a disposable agent may only create \
         explicitly disposable child work — pass `--scope disposable` (or `--tag disposable`) \
         to mint a disposable child. Durable follow-up graph work must be created by a \
         persistent agent (R8)."
    );
}

/// Validate a user-supplied `--scope` value and return the tag that persists it.
///
/// Scope values are lowercase alphanumerics plus `-`/`_` (they become part of a
/// tag and an env var value). Returns `scope:<value>`.
pub fn scope_tag(value: &str) -> Result<String> {
    let v = value.trim();
    if v.is_empty() {
        bail!("--scope value cannot be empty");
    }
    if !v
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!(
            "invalid --scope value '{}': use letters, digits, '-' or '_'",
            value
        );
    }
    Ok(format!("{}{}", SCOPE_TAG_PREFIX, v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disposable_forbids_both_spawns() {
        assert!(check_scope(Some(SCOPE_DISPOSABLE), PersistentSpawn::Agent).is_err());
        assert!(check_scope(Some(SCOPE_DISPOSABLE), PersistentSpawn::Task).is_err());
    }

    #[test]
    fn non_disposable_is_allowed() {
        assert!(check_scope(None, PersistentSpawn::Agent).is_ok());
        assert!(check_scope(Some("persistent"), PersistentSpawn::Task).is_ok());
        assert!(check_scope(Some("team"), PersistentSpawn::Agent).is_ok());
    }

    #[test]
    fn tag_roundtrip() {
        assert_eq!(scope_tag("disposable").unwrap(), "scope:disposable");
        assert_eq!(
            scope_from_tags(&["scope:disposable".to_string()]).as_deref(),
            Some("disposable")
        );
        assert!(scope_tag("").is_err());
        assert!(scope_tag("has space").is_err());
    }

    #[test]
    fn resolve_add_scope_leaves_non_disposable_callers_untouched() {
        // Unscoped / non-disposable callers: tags pass through verbatim, no scope inherited.
        assert_eq!(
            resolve_add_scope_for(None, &["urgent".to_string()]).unwrap(),
            vec!["urgent".to_string()]
        );
        assert_eq!(
            resolve_add_scope_for(Some("team"), &["persistent".to_string()]).unwrap(),
            vec!["persistent".to_string()]
        );
    }

    #[test]
    fn disposable_untagged_add_is_denied() {
        // The case Erik flagged: an ordinary untagged durable add from disposable
        // scope must be REFUSED — a disposable agent may not mint durable
        // follow-up work by omitting the tag. It is not silently downgraded.
        assert!(resolve_add_scope_for(Some(SCOPE_DISPOSABLE), &["urgent".to_string()]).is_err());
        // Even a fully untagged add is refused.
        assert!(resolve_add_scope_for(Some(SCOPE_DISPOSABLE), &[]).is_err());
    }

    #[test]
    fn disposable_add_persistent_tag_is_denied() {
        assert!(
            resolve_add_scope_for(Some(SCOPE_DISPOSABLE), &["persistent".to_string()]).is_err()
        );
    }

    #[test]
    fn disposable_add_escalating_scope_is_denied() {
        // Trying to hand a child a non-disposable scope is an escalation.
        assert!(
            resolve_add_scope_for(Some(SCOPE_DISPOSABLE), &["scope:team".to_string()]).is_err()
        );
    }

    #[test]
    fn disposable_explicit_disposable_scope_child_is_allowed() {
        // The one allowed case: an explicit --scope disposable child. Allowed
        // verbatim and not double-tagged.
        let resolved =
            resolve_add_scope_for(Some(SCOPE_DISPOSABLE), &["scope:disposable".to_string()])
                .unwrap();
        assert_eq!(
            resolved
                .iter()
                .filter(|t| t.as_str() == "scope:disposable")
                .count(),
            1
        );
        assert_eq!(
            scope_from_tags(&resolved).as_deref(),
            Some(SCOPE_DISPOSABLE)
        );
    }

    #[test]
    fn disposable_explicit_disposable_tag_child_is_allowed() {
        // A child explicitly tagged `disposable` (non-durable) is allowed verbatim.
        let resolved =
            resolve_add_scope_for(Some(SCOPE_DISPOSABLE), &["disposable".to_string()]).unwrap();
        assert_eq!(resolved, vec!["disposable".to_string()]);
    }
}
