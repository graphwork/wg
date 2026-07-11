//! R8 — `--scope disposable` guard (docs/02 §3.2 in the family-team project).
//!
//! A task created with `wg add --scope disposable` runs its agent with
//! `WG_SCOPE=disposable`. A disposable-scoped agent must not be able to mint a
//! *persistent* persona: it may not run `wg agent create` or
//! `wg add --tag persistent`. These tests pin the policy at the library
//! boundary so the CLI handlers can rely on it.

use worksgood::scope_guard::{check_scope, scope_from_tags, PersistentSpawn, SCOPE_DISPOSABLE};

/// The load-bearing R8 policy: a disposable scope forbids every persistent
/// spawn, while unscoped / non-disposable scopes are unaffected.
#[test]
fn test_scoped_disposable_cannot_spawn_persistent() {
    // disposable is denied both privileged spawns
    assert!(
        check_scope(Some(SCOPE_DISPOSABLE), PersistentSpawn::Agent).is_err(),
        "disposable scope must forbid `wg agent create`"
    );
    assert!(
        check_scope(Some(SCOPE_DISPOSABLE), PersistentSpawn::Task).is_err(),
        "disposable scope must forbid `wg add --tag persistent`"
    );

    // unscoped and non-disposable scopes are allowed
    assert!(
        check_scope(None, PersistentSpawn::Agent).is_ok(),
        "unscoped agents may create persistent agents"
    );
    assert!(
        check_scope(Some("persistent"), PersistentSpawn::Task).is_ok(),
        "a persistent-scoped agent may create persistent tasks"
    );
    assert!(
        check_scope(Some("team"), PersistentSpawn::Agent).is_ok(),
        "only the reserved `disposable` scope is restricted"
    );
}

/// Scope is persisted on a task as a `scope:<value>` tag, which is how the
/// dispatcher recovers it to set `WG_SCOPE` on the spawned worker.
#[test]
fn test_scope_persisted_as_tag() {
    assert_eq!(
        scope_from_tags(&["scope:disposable".to_string(), "urgent".to_string()]).as_deref(),
        Some("disposable"),
    );
    assert_eq!(scope_from_tags(&["urgent".to_string()]), None);
}
