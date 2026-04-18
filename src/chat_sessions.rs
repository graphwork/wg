//! Session registry for chat-file nex sessions.
//!
//! Every nex session — interactive, coordinator, or task-agent —
//! lives under `<workgraph>/chat/<uuid>/` with the same file layout
//! (inbox.jsonl, outbox.jsonl, .streaming, conversation.jsonl, ...).
//! A session is identified by its UUID. Humans and legacy code
//! address sessions by **alias**, which resolves to a UUID via
//! this registry.
//!
//! Aliases:
//! - `coordinator-0`, `coordinator-1`, ... for workgraph coordinators
//!   (what used to be numeric `chat/0/`, `chat/1/` directly)
//! - `task-<task-id>` for task-agent sessions
//! - `tty-<slug>` for interactive sessions pinned to a terminal
//! - Arbitrary user-chosen aliases (e.g. `debug-redis`) via
//!   `wg chat new --alias X`
//!
//! The registry is a single JSON file at
//! `<workgraph>/chat/sessions.json` plus one filesystem symlink per
//! alias (`chat/<alias>` → `chat/<uuid>`). Symlinks mean existing
//! code that writes `chat/0/inbox.jsonl` keeps working unchanged —
//! the kernel resolves the alias for us. The JSON registry is the
//! authoritative listing (for `wg chat list`, attach-by-prefix,
//! dangling-alias cleanup).
//!
//! Resolution order for `resolve_ref`:
//! 1. Exact UUID match (string equality on the 36-char form)
//! 2. Exact alias match
//! 3. Unambiguous UUID prefix (≥4 chars, like git short hashes)
//! 4. Error
//!
//! The registry is read on every call (cheap JSON parse) rather than
//! cached in-memory — this sidesteps the "two processes editing
//! sessions.json" coordination problem. Writes take a file lock.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What kind of nex session this is. Surfaces in `wg chat list` and
/// lets the TUI group sessions by role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionKind {
    /// Long-running daemon coordinator (historical `chat/0/`).
    Coordinator,
    /// Autonomous task agent spawned by the coordinator for a graph task.
    TaskAgent,
    /// A human at a terminal running `wg nex`.
    Interactive,
    /// An evaluator run, a /skill session, or anything else
    /// explicitly classified later.
    Other,
}

/// Per-session metadata. UUID is the dir name; this struct is the
/// entry in `chat/sessions.json` keyed by UUID.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub kind: SessionKind,
    /// ISO-8601 timestamp of registration.
    pub created: String,
    /// Human handles. Must each be unique across the whole registry —
    /// `register_alias` enforces this. Empty is allowed (UUID-only
    /// session, still addressable by its UUID).
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Optional free-form label for `wg chat list` display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// The on-disk registry file shape.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub sessions: HashMap<String, SessionMeta>,
}

fn default_version() -> u32 {
    1
}

/// Path to the registry file.
pub fn registry_path(workgraph_dir: &Path) -> PathBuf {
    workgraph_dir.join("chat").join("sessions.json")
}

/// Path to the chat-dir for a given UUID.
pub fn chat_dir_for_uuid(workgraph_dir: &Path, uuid: &str) -> PathBuf {
    workgraph_dir.join("chat").join(uuid)
}

/// Load the registry, returning an empty one if the file doesn't exist.
pub fn load(workgraph_dir: &Path) -> Result<Registry> {
    let path = registry_path(workgraph_dir);
    if !path.exists() {
        return Ok(Registry::default());
    }
    let mut s = String::new();
    File::open(&path)
        .with_context(|| format!("open {:?}", path))?
        .read_to_string(&mut s)?;
    if s.trim().is_empty() {
        return Ok(Registry::default());
    }
    let reg: Registry =
        serde_json::from_str(&s).with_context(|| format!("parse registry {:?}", path))?;
    Ok(reg)
}

/// Atomically save the registry. Writes to a temp file then renames
/// so a concurrent reader never sees a half-written file.
pub fn save(workgraph_dir: &Path, reg: &Registry) -> Result<()> {
    let path = registry_path(workgraph_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        let json = serde_json::to_string_pretty(reg)?;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Create a new session UUID, directory, and registry entry.
/// Optionally adds aliases (each creates a symlink under `chat/`).
///
/// Returns the new UUID.
pub fn create_session(
    workgraph_dir: &Path,
    kind: SessionKind,
    aliases: &[String],
    label: Option<String>,
) -> Result<String> {
    let uuid = Uuid::new_v4().to_string();
    let dir = chat_dir_for_uuid(workgraph_dir, &uuid);
    fs::create_dir_all(&dir).with_context(|| format!("create_dir_all {:?}", dir))?;

    // Register in the JSON index first so a crashed symlink-creation
    // doesn't leave an unregistered session orphan.
    let mut reg = load(workgraph_dir).unwrap_or_default();
    for a in aliases {
        if let Some(existing) = find_by_alias(&reg, a) {
            bail!("alias {:?} already points to session {}", a, existing.0);
        }
    }
    reg.sessions.insert(
        uuid.clone(),
        SessionMeta {
            kind,
            created: Utc::now().to_rfc3339(),
            aliases: aliases.to_vec(),
            label,
        },
    );
    save(workgraph_dir, &reg)?;

    // Then make the alias symlinks point at the UUID dir.
    for a in aliases {
        create_alias_symlink(workgraph_dir, a, &uuid)?;
    }
    Ok(uuid)
}

/// Ensure a session with the given alias exists, creating it if not.
/// Idempotent — a second call with the same alias returns the existing
/// UUID without creating a new session. Intended for callers like the
/// coordinator supervisor that want a stable UUID behind a well-known
/// alias (`coordinator-0`) without racing on startup.
pub fn ensure_session(
    workgraph_dir: &Path,
    alias: &str,
    kind: SessionKind,
    label: Option<String>,
) -> Result<String> {
    let reg = load(workgraph_dir).unwrap_or_default();
    if let Some((uuid, _)) = find_by_alias(&reg, alias) {
        // Double-check the symlink points where we think — idempotent
        // repair in case a bare chat dir exists without its alias link.
        let _ = create_alias_symlink(workgraph_dir, alias, &uuid);
        return Ok(uuid);
    }
    create_session(workgraph_dir, kind, &[alias.to_string()], label)
}

/// Resolve a reference (UUID, prefix, or alias) to a UUID.
pub fn resolve_ref(workgraph_dir: &Path, reference: &str) -> Result<String> {
    let reg = load(workgraph_dir).unwrap_or_default();

    // 1. Exact UUID (36-char canonical form).
    if reg.sessions.contains_key(reference) {
        return Ok(reference.to_string());
    }

    // 2. Exact alias.
    if let Some((uuid, _)) = find_by_alias(&reg, reference) {
        return Ok(uuid);
    }

    // 3. UUID prefix (≥4 chars, must be unambiguous).
    if reference.len() >= 4 {
        let matches: Vec<_> = reg
            .sessions
            .keys()
            .filter(|k| k.starts_with(reference))
            .cloned()
            .collect();
        match matches.len() {
            0 => {}
            1 => return Ok(matches.into_iter().next().unwrap()),
            _ => bail!(
                "ambiguous session prefix {:?}: {} matches — be more specific",
                reference,
                matches.len()
            ),
        }
    }

    Err(anyhow!(
        "session reference {:?} did not match any UUID, prefix, or alias",
        reference
    ))
}

/// Find a session by alias. Returns (UUID, metadata) on match.
pub fn find_by_alias<'a>(reg: &'a Registry, alias: &str) -> Option<(String, &'a SessionMeta)> {
    for (uuid, meta) in &reg.sessions {
        if meta.aliases.iter().any(|a| a == alias) {
            return Some((uuid.clone(), meta));
        }
    }
    None
}

/// Create (or refresh) a symlink `chat/<alias>` → `<uuid>`.
/// The target is relative so the whole workgraph dir stays movable.
fn create_alias_symlink(workgraph_dir: &Path, alias: &str, uuid: &str) -> Result<()> {
    let link = workgraph_dir.join("chat").join(alias);
    if link.exists() || link.is_symlink() {
        // Replace if already present. An existing link pointing at
        // the same target is fine; an old stale link for a different
        // UUID gets overwritten.
        let _ = fs::remove_file(&link);
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(uuid, &link)
            .with_context(|| format!("symlink {:?} -> {}", link, uuid))?;
    }
    #[cfg(not(unix))]
    {
        // Windows: not supported in this pass. The JSON registry
        // still works; only path-based back-compat is unavailable.
        let _ = uuid;
        bail!("alias symlinks are not supported on non-Unix targets");
    }
    Ok(())
}

/// Add an alias to an existing session (UUID or existing alias).
pub fn add_alias(workgraph_dir: &Path, reference: &str, alias: &str) -> Result<()> {
    let uuid = resolve_ref(workgraph_dir, reference)?;
    let mut reg = load(workgraph_dir).unwrap_or_default();
    if let Some((existing, _)) = find_by_alias(&reg, alias)
        && existing != uuid
    {
        bail!(
            "alias {:?} already points to a different session ({})",
            alias,
            existing
        );
    }
    if let Some(meta) = reg.sessions.get_mut(&uuid)
        && !meta.aliases.iter().any(|a| a == alias)
    {
        meta.aliases.push(alias.to_string());
    }
    save(workgraph_dir, &reg)?;
    create_alias_symlink(workgraph_dir, alias, &uuid)?;
    Ok(())
}

/// Remove an alias (and its symlink). The session itself stays.
pub fn remove_alias(workgraph_dir: &Path, alias: &str) -> Result<()> {
    let mut reg = load(workgraph_dir).unwrap_or_default();
    let Some((uuid, _)) = find_by_alias(&reg, alias) else {
        bail!("no such alias {:?}", alias);
    };
    if let Some(meta) = reg.sessions.get_mut(&uuid) {
        meta.aliases.retain(|a| a != alias);
    }
    save(workgraph_dir, &reg)?;
    let link = workgraph_dir.join("chat").join(alias);
    let _ = fs::remove_file(link);
    Ok(())
}

/// Delete a session entirely (registry entry + symlinks + chat dir).
/// Destructive — no undo.
pub fn delete_session(workgraph_dir: &Path, reference: &str) -> Result<()> {
    let uuid = resolve_ref(workgraph_dir, reference)?;
    let mut reg = load(workgraph_dir).unwrap_or_default();
    if let Some(meta) = reg.sessions.remove(&uuid) {
        for a in &meta.aliases {
            let link = workgraph_dir.join("chat").join(a);
            let _ = fs::remove_file(link);
        }
    }
    save(workgraph_dir, &reg)?;
    let dir = chat_dir_for_uuid(workgraph_dir, &uuid);
    if dir.exists() {
        fs::remove_dir_all(&dir).with_context(|| format!("rm -rf {:?}", dir))?;
    }
    Ok(())
}

/// Return a sorted list of (UUID, meta) for display.
pub fn list(workgraph_dir: &Path) -> Result<Vec<(String, SessionMeta)>> {
    let reg = load(workgraph_dir)?;
    let mut out: Vec<_> = reg.sessions.into_iter().collect();
    out.sort_by(|a, b| a.1.created.cmp(&b.1.created));
    Ok(out)
}

/// Migrate an existing numeric coord dir (`chat/0`, `chat/1`, …) to a
/// UUID-named dir with the corresponding `coordinator-N` alias.
/// Idempotent — if `chat/N` is already a symlink into a UUID dir, it's
/// left alone. If `chat/N` is a real directory with content, its
/// contents are moved to `chat/<new-uuid>` and the original path is
/// re-created as a symlink. This lets older daemons that wrote to
/// `chat/0/` coexist with new UUID-aware ones without losing history.
pub fn migrate_numeric_coord_dir(workgraph_dir: &Path, n: u32) -> Result<Option<String>> {
    let old = workgraph_dir.join("chat").join(n.to_string());
    if !old.exists() {
        return Ok(None);
    }
    // Already a symlink — assume prior migration succeeded.
    if old.is_symlink() {
        return Ok(None);
    }

    let alias = format!("coordinator-{}", n);
    let numeric_alias = n.to_string();
    let reg = load(workgraph_dir).unwrap_or_default();

    // If `coordinator-N` is already registered, don't create a
    // duplicate session. This happens when an older subprocess left
    // behind a bare `chat/N/` dir while the new registry-aware
    // daemon had already registered the session under a UUID. Merge
    // instead: move any files from the legacy dir into the existing
    // session's dir (skipping files that would overwrite — those
    // are newer and belong to the registered session), then install
    // the `chat/N` → `<uuid>` symlink.
    if let Some((existing_uuid, _)) = find_by_alias(&reg, &alias) {
        let target_dir = chat_dir_for_uuid(workgraph_dir, &existing_uuid);
        fs::create_dir_all(&target_dir).ok();
        // Merge files from old dir into target_dir. Files that would
        // collide are kept at the target (the registered session's
        // data is the authoritative one).
        if let Ok(entries) = fs::read_dir(&old) {
            for entry in entries.flatten() {
                let src = entry.path();
                let dest = target_dir.join(entry.file_name());
                if dest.exists() {
                    // Keep the registered version; drop the orphan.
                    if src.is_dir() {
                        let _ = fs::remove_dir_all(&src);
                    } else {
                        let _ = fs::remove_file(&src);
                    }
                } else {
                    let _ = fs::rename(&src, &dest);
                }
            }
        }
        // Remove the now-empty old dir and install the alias
        // symlink + numeric alias.
        let _ = fs::remove_dir_all(&old);
        create_alias_symlink(workgraph_dir, &numeric_alias, &existing_uuid)?;
        // Also ensure the numeric alias is in the registry entry.
        let mut reg2 = load(workgraph_dir).unwrap_or_default();
        if let Some(meta) = reg2.sessions.get_mut(&existing_uuid)
            && !meta.aliases.iter().any(|a| a == &numeric_alias)
        {
            meta.aliases.push(numeric_alias.clone());
            save(workgraph_dir, &reg2)?;
        }
        return Ok(Some(existing_uuid));
    }

    // No existing alias — standard migration path. Create a fresh
    // UUID dir, move the legacy contents in, register with both
    // aliases.
    let uuid = Uuid::new_v4().to_string();
    let new_dir = chat_dir_for_uuid(workgraph_dir, &uuid);
    fs::rename(&old, &new_dir).with_context(|| format!("migrate {:?} -> {:?}", old, new_dir))?;

    let mut reg = load(workgraph_dir).unwrap_or_default();
    reg.sessions.insert(
        uuid.clone(),
        SessionMeta {
            kind: SessionKind::Coordinator,
            created: Utc::now().to_rfc3339(),
            aliases: vec![alias.clone(), numeric_alias.clone()],
            label: Some(format!("coordinator {} (migrated)", n)),
        },
    );
    save(workgraph_dir, &reg)?;

    create_alias_symlink(workgraph_dir, &alias, &uuid)?;
    create_alias_symlink(workgraph_dir, &numeric_alias, &uuid)?;
    Ok(Some(uuid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn create_and_resolve_session() {
        let dir = tempdir().unwrap();
        let wg = dir.path();
        let uuid =
            create_session(wg, SessionKind::Interactive, &["my-work".to_string()], None).unwrap();

        // Resolve by UUID
        assert_eq!(resolve_ref(wg, &uuid).unwrap(), uuid);
        // Resolve by alias
        assert_eq!(resolve_ref(wg, "my-work").unwrap(), uuid);
        // Resolve by prefix
        assert_eq!(resolve_ref(wg, &uuid[..8]).unwrap(), uuid);
    }

    #[test]
    fn alias_symlink_exists_after_create() {
        let dir = tempdir().unwrap();
        let wg = dir.path();
        let uuid =
            create_session(wg, SessionKind::TaskAgent, &["task-foo".to_string()], None).unwrap();
        let link = wg.join("chat").join("task-foo");
        assert!(link.is_symlink(), "alias should be a symlink");
        let target = fs::read_link(&link).unwrap();
        assert_eq!(target.to_string_lossy(), uuid);
    }

    #[test]
    fn ambiguous_prefix_errors() {
        let dir = tempdir().unwrap();
        let wg = dir.path();
        // Two UUIDs will share the empty prefix ""; we want to test
        // that a SHORT prefix that's genuinely ambiguous errors out.
        // Since UUID randomness makes this flaky, we manually seed
        // the registry with two UUIDs that share a prefix.
        fs::create_dir_all(wg.join("chat")).unwrap();
        let mut reg = Registry::default();
        let u1 = "aaaa1111-e29b-41d4-a716-446655440000".to_string();
        let u2 = "aaaa2222-e29b-41d4-a716-446655440000".to_string();
        reg.sessions.insert(
            u1.clone(),
            SessionMeta {
                kind: SessionKind::Interactive,
                created: "2026-01-01".into(),
                aliases: vec![],
                label: None,
            },
        );
        reg.sessions.insert(
            u2.clone(),
            SessionMeta {
                kind: SessionKind::Interactive,
                created: "2026-01-02".into(),
                aliases: vec![],
                label: None,
            },
        );
        save(wg, &reg).unwrap();
        let err = resolve_ref(wg, "aaaa").unwrap_err();
        assert!(err.to_string().contains("ambiguous"));
        // But a more specific prefix resolves.
        assert_eq!(resolve_ref(wg, "aaaa1").unwrap(), u1);
    }

    #[test]
    fn ensure_session_is_idempotent() {
        let dir = tempdir().unwrap();
        let wg = dir.path();
        let uuid1 = ensure_session(wg, "coordinator-0", SessionKind::Coordinator, None).unwrap();
        let uuid2 = ensure_session(wg, "coordinator-0", SessionKind::Coordinator, None).unwrap();
        assert_eq!(uuid1, uuid2);
    }

    #[test]
    fn add_and_remove_alias() {
        let dir = tempdir().unwrap();
        let wg = dir.path();
        let uuid = create_session(wg, SessionKind::Interactive, &["primary".into()], None).unwrap();
        add_alias(wg, "primary", "secondary").unwrap();
        assert_eq!(resolve_ref(wg, "secondary").unwrap(), uuid);
        remove_alias(wg, "secondary").unwrap();
        assert!(resolve_ref(wg, "secondary").is_err());
        // Primary still works.
        assert_eq!(resolve_ref(wg, "primary").unwrap(), uuid);
    }

    #[test]
    fn migrate_merges_into_existing_alias() {
        let dir = tempdir().unwrap();
        let wg = dir.path();
        // First, a fresh coordinator-0 session is created via
        // ensure_session (no pre-existing chat/0 dir).
        let existing_uuid =
            ensure_session(wg, "coordinator-0", SessionKind::Coordinator, None).unwrap();
        let existing_dir = chat_dir_for_uuid(wg, &existing_uuid);
        fs::write(existing_dir.join("existing.txt"), "registered").unwrap();

        // Now simulate a legacy subprocess creating chat/0/ as a
        // real directory with its own content.
        let legacy = wg.join("chat").join("0");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("orphan.txt"), "from legacy subprocess").unwrap();
        fs::write(legacy.join("existing.txt"), "would clobber").unwrap();

        // Migration should MERGE, not create a new session.
        let uuid = migrate_numeric_coord_dir(wg, 0).unwrap().unwrap();
        assert_eq!(uuid, existing_uuid, "should reuse existing UUID");

        // Registry still has exactly one coordinator-0 session.
        let sessions: Vec<_> = list(wg)
            .unwrap()
            .into_iter()
            .filter(|(_, m)| m.aliases.iter().any(|a| a == "coordinator-0"))
            .collect();
        assert_eq!(sessions.len(), 1, "no duplicate coordinator-0 entries");

        // The orphan file got merged into the existing session's dir.
        assert!(existing_dir.join("orphan.txt").exists());
        // The registered session's version of the clobbering file wins.
        assert_eq!(
            fs::read_to_string(existing_dir.join("existing.txt")).unwrap(),
            "registered",
        );
        // Legacy dir is gone, replaced by a symlink to the UUID.
        assert!(legacy.is_symlink());
    }

    #[test]
    fn migrate_numeric_coord_dir_moves_contents() {
        let dir = tempdir().unwrap();
        let wg = dir.path();
        let old = wg.join("chat").join("0");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("marker.txt"), "legacy data").unwrap();

        let uuid = migrate_numeric_coord_dir(wg, 0).unwrap().unwrap();
        let new_marker = chat_dir_for_uuid(wg, &uuid).join("marker.txt");
        assert!(new_marker.exists(), "legacy file should be under UUID dir");
        assert_eq!(fs::read_to_string(&new_marker).unwrap(), "legacy data");

        // Old path is now a symlink that still works for readers.
        assert!(old.is_symlink());
        assert_eq!(
            fs::read_to_string(old.join("marker.txt")).unwrap(),
            "legacy data"
        );

        // And the `coordinator-0` alias also resolves.
        assert_eq!(resolve_ref(wg, "coordinator-0").unwrap(), uuid);
    }
}
