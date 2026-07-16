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
//! add` boundary is **default-deny**: from disposable scope the *only* allowed
//! add is an **explicit, scope-carrying** disposable child — `--scope
//! disposable`, which persists a `scope:disposable` tag; every other add is
//! refused (see [`resolve_add_scope_for`]). Only the reserved scope value
//! `disposable` restricts anything; every other value (including unscoped) is
//! unaffected.
//!
//! Why *scope-carrying* and not a bare `disposable` tag (Erik, PR #56 rd3): the
//! dispatcher's [`plan_spawn`] recovers a worker's scope only from a
//! `scope:<value>` tag (see [`scope_from_tags`]). A bare `disposable` tag is
//! therefore invisible to the dispatcher, so a child created with `--tag
//! disposable` would spawn **unscoped** and could mint durable grandchildren —
//! containment defeated one generation later. The allowed route must persist
//! `scope:disposable` so `WG_SCOPE=disposable` propagates and the child is
//! itself contained. [`resolve_add_scope_for`] guarantees this invariant: any
//! tag set it returns for a disposable caller carries `scope:disposable`.
//!
//! [`plan_spawn`]: crate::dispatch::plan::plan_spawn
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
/// from disposable scope the *only* thing that may be created is an **explicit,
/// scope-carrying disposable child** — i.e. `--scope disposable`, which
/// persists a `scope:disposable` tag. Concretely:
///
///   * an explicit child `--scope disposable` (a `scope:disposable` tag) is
///     **allowed** — a disposable agent may spawn a disposable child, and the
///     `scope:disposable` tag guarantees the dispatcher propagates
///     `WG_SCOPE=disposable` so the child is itself contained;
///   * an explicit `persistent` tag, or an explicit `scope:<x>` naming any scope
///     other than `disposable`, is **denied** — a disposable agent may not
///     escalate a child into durable / persistent graph state;
///   * a **bare `disposable` tag** (`--tag disposable`, no `scope:` prefix) is
///     **denied**. This is Erik's PR #56 rd3 hole: a bare tag is invisible to
///     [`plan_spawn`] (which reads only `scope:` tags via [`scope_from_tags`]),
///     so the child would spawn *unscoped* and could mint durable grandchildren.
///     The caller must opt in with the scope-carrying `--scope disposable`; and
///   * an ordinary untagged durable `wg add "x"` (minting durable follow-up work
///     simply by omitting the tag) is **denied**. It is *not* silently
///     downgraded — the caller must opt in with `--scope disposable`.
///
/// The returned `Vec` is the tag set that should be persisted on the new task.
/// **Invariant:** for a disposable caller, any `Ok` result carries a
/// `scope:disposable` tag — so no allowed form can reach [`plan_spawn`] as an
/// unscoped worker.
///
/// [`plan_spawn`]: crate::dispatch::plan::plan_spawn
pub fn resolve_add_scope_for(caller_scope: Option<&str>, tags: &[String]) -> Result<Vec<String>> {
    // Only the reserved `disposable` scope constrains anything.
    if caller_scope != Some(SCOPE_DISPOSABLE) {
        return Ok(tags.to_vec());
    }

    // Explicit persistent tag → hard deny (the original R8 case).
    if tags.iter().any(|t| t == "persistent") {
        check_scope(caller_scope, PersistentSpawn::Task)?;
    }

    // The ONLY allowed add from disposable scope is an explicit, scope-carrying
    // `--scope disposable` (a `scope:disposable` tag). Any other `scope:<x>` is
    // an escalation → deny.
    if let Some(child_scope) = scope_from_tags(tags) {
        if child_scope != SCOPE_DISPOSABLE {
            bail!(
                "scope=disposable forbids `wg add --scope {child_scope}`: a disposable \
                 agent may only create disposable-scoped children (R8). Drop the \
                 `--scope {child_scope}` override, or have a persistent agent create \
                 durable work."
            );
        }
        // Child is explicitly disposable-scoped. Uphold the module invariant: the
        // returned tag set MUST carry `scope:disposable` so `plan_spawn`
        // propagates `WG_SCOPE=disposable` and the child cannot mint durable
        // grandchildren. It does here by construction (the `scope:disposable`
        // tag we just matched); assert it so a future refactor can't regress the
        // containment boundary.
        debug_assert_eq!(
            scope_from_tags(tags).as_deref(),
            Some(SCOPE_DISPOSABLE),
            "allowed disposable child must carry scope:disposable"
        );
        return Ok(tags.to_vec());
    }

    // Everything else is refused. This includes:
    //   * a BARE `disposable` tag (Erik's PR #56 rd3 hole) — it is NOT a `scope:`
    //     tag, so `plan_spawn` would spawn the child unscoped, able to mint
    //     durable grandchildren; and
    //   * an ordinary untagged durable `wg add "x"` — minting durable follow-up
    //     work by omitting the tag.
    // Only an explicit, scope-carrying `--scope disposable` is allowed.
    bail!(
        "scope=disposable forbids this `wg add`: a disposable agent may only create an \
         explicitly disposable, scope-carrying child — pass `--scope disposable`, which \
         persists a `scope:disposable` tag the dispatcher propagates as WG_SCOPE. A bare \
         `--tag disposable` is not accepted: it carries no `scope:` prefix, so the child \
         would spawn unscoped and could mint durable grandchildren. Durable follow-up work \
         must be created by a persistent agent (R8)."
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
    fn disposable_bare_disposable_tag_child_is_denied() {
        // Erik's PR #56 rd3 hole: a BARE `disposable` tag carries no `scope:`
        // prefix, so `plan_spawn` (which reads only `scope:` tags) would spawn
        // the child UNSCOPED — free to mint durable grandchildren. It must be
        // refused; only the scope-carrying `--scope disposable` is allowed.
        assert!(
            resolve_add_scope_for(Some(SCOPE_DISPOSABLE), &["disposable".to_string()]).is_err(),
            "a bare `--tag disposable` from disposable scope must be refused"
        );
    }

    #[test]
    fn disposable_allowed_result_always_carries_scope_tag() {
        // Module invariant: every Ok result for a disposable caller carries
        // `scope:disposable`, so no allowed form reaches plan_spawn unscoped.
        let resolved =
            resolve_add_scope_for(Some(SCOPE_DISPOSABLE), &["scope:disposable".to_string()])
                .unwrap();
        assert_eq!(
            scope_from_tags(&resolved).as_deref(),
            Some(SCOPE_DISPOSABLE),
            "allowed disposable add must carry scope:disposable for propagation"
        );
    }
}
