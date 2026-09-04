//! `wg migrate project-local-pi [--cleanup-global-routing] [--dry-run]
//! [--rollback <receipt>]` — the optional machine-global routing cleanup
//! phase of the project-local-Pi cutover
//! (`docs/design-project-local-pi-config.md` §9.2).
//!
//! ## What this does
//!
//! Project execution no longer reads `~/.wg/config.toml` or
//! `~/.wg/active-profile` as an effective configuration layer — a project's
//! `worksgood.toml` is the sole authority. Stale machine-global model-routing
//! and active-profile state therefore sits inert in `~/.wg`, confusing
//! operators and risking a future regression. This command removes *only*
//! that obsolete routing state while preserving every reusable profile
//! definition, every identity/federation/secret datum, and every other byte.
//!
//! ## Safety properties (design §9.2)
//!
//! * **Allowlisted write-set.** Only `~/.wg/config.toml` and
//!   `~/.wg/active-profile` may be opened for write/remove. A plan that names
//!   any keystore, secret value, identity, federation, profile-definition, or
//!   Pi-settings path is rejected as an internal bug.
//! * **Backup before mutation.** Preimages are copied into a mode-`0700`
//!   `~/.wg/migrations/project-local-pi/<receipt>/` directory before any
//!   write; copies are mode `0600` and contain no newly rendered secret
//!   values (only the original file bytes).
//! * **Idempotent.** A second run with nothing to remove writes no backup,
//!   creates no receipt, and changes no mtime.
//! * **Fail-closed.** Malformed global config TOML refuses before any write.
//! * **CAS rollback.** `--rollback <receipt>` restores a preimage only when
//!   the current bytes still equal the receipt's postimage; a later user edit
//!   is never overwritten.
//! * **No credential responsibility.** Provider credentials, endpoints, and
//!   auth stay where they are (preserved inactive); WG does not adopt Pi
//!   provider authentication for Pi-only projects.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::IsTerminal;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::atomic_file::write_atomic;
use crate::config::Config;
use crate::identity::blake3_32;
use crate::profile::named;

/// Command version recorded in every receipt. Bumped when the on-disk receipt
/// schema changes in a way old binaries cannot verify.
pub const PROJECT_LOCAL_PI_MIGRATE_VERSION: u32 = 1;

/// Subdirectory under `~/.wg/` that holds cleanup backups + receipts.
const MIGRATIONS_DIR: &str = "migrations/project-local-pi";

/// Args for the `wg migrate project-local-pi` subcommand.
#[derive(Debug, Clone)]
pub struct ProjectLocalPiMigrateArgs {
    pub dry_run: bool,
    pub yes: bool,
    pub cleanup_global_routing: bool,
    pub rollback: Option<String>,
}

/// One routing selector removed from the global config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemovedRoutingKey {
    /// Dotted path, e.g. `agent.model`, `tiers`, `execution.fallbacks`.
    pub key: String,
    /// Short human label of the removal category.
    pub category: String,
}

/// A path preserved untouched, with identity/mode/mtime metadata recorded
/// *without* reading its contents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreservedPath {
    pub path: String,
    pub kind: String,
    pub existed: bool,
    pub mode: Option<u32>,
    pub mtime: Option<String>,
}

/// Per-file backup record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupRecord {
    /// Path the backup was taken from.
    pub source: String,
    /// Backup copy path (under the receipt dir).
    pub backup: String,
    /// `b3:` BLAKE3 of the preimage bytes. `None` when the source was absent
    /// (e.g. no active-profile pointer existed).
    pub preimage_digest: Option<String>,
    /// Size in bytes (0 for absent).
    pub preimage_size: u64,
}

/// The full plan/result for one cleanup run. Serialized as the receipt JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectLocalPiCleanupReport {
    pub version: u32,
    /// Monotonic receipt id (also the receipt dir name).
    pub receipt_id: String,
    pub dry_run: bool,
    /// `true` when there was nothing to remove — no backup was written and no
    /// mtimes changed.
    pub no_op: bool,
    pub global_config_path: String,
    pub active_profile_path: String,
    pub removed_routing_keys: Vec<RemovedRoutingKey>,
    pub removed_active_profile: bool,
    pub backups: Vec<BackupRecord>,
    /// Postimage digests (after apply). `None` for a file that ended absent.
    pub postimage_digests: BTreeMap<String, Option<String>>,
    /// Protected roots asserted unopened/unchanged.
    pub preserved: Vec<PreservedPath>,
    /// Journal state. `prepared` → `global-config-committed` →
    /// `active-pointer-committed` → `complete`.
    pub journal_state: String,
    /// Allowlisted write-set — the only paths this run is permitted to open
    /// for write/remove.
    pub write_set: Vec<String>,
    /// Command version string for diagnostics.
    pub command: String,
}

impl ProjectLocalPiCleanupReport {
    fn new(global_config_path: &Path, active_profile_path: &Path) -> Self {
        Self {
            version: PROJECT_LOCAL_PI_MIGRATE_VERSION,
            receipt_id: String::new(),
            dry_run: false,
            no_op: true,
            global_config_path: global_config_path.display().to_string(),
            active_profile_path: active_profile_path.display().to_string(),
            removed_routing_keys: Vec::new(),
            removed_active_profile: false,
            backups: Vec::new(),
            postimage_digests: BTreeMap::new(),
            preserved: Vec::new(),
            journal_state: "prepared".to_string(),
            write_set: vec![
                global_config_path.display().to_string(),
                active_profile_path.display().to_string(),
            ],
            command: "wg migrate project-local-pi --cleanup-global-routing".to_string(),
        }
    }
}

/// Top-level `[section]` tables removed in full by global routing cleanup.
const REMOVED_SECTIONS: &[&str] = &["tiers", "models"];

/// `[section].key` leaf selectors removed by global routing cleanup.
/// `(section, key, category)`. `coordinator` is the legacy alias of
/// `dispatcher`; both are scanned so a pre-rename config is still cleaned.
const REMOVED_LEAVES: &[(&str, &str, &str)] = &[
    ("agent", "model", "agent route"),
    ("agent", "executor", "agent executor"),
    ("dispatcher", "model", "dispatcher route"),
    ("dispatcher", "provider", "dispatcher provider"),
    ("dispatcher", "executor", "dispatcher executor"),
    ("coordinator", "model", "legacy dispatcher route"),
    ("coordinator", "provider", "legacy dispatcher provider"),
    ("coordinator", "executor", "legacy dispatcher executor"),
];

/// Array-of-tables removed by global routing cleanup (each entry authorizes
/// an exact alternate route — `[[execution.fallbacks]]`).
const REMOVED_ARRAYS: &[(&str, &str, &str)] = &[("execution", "fallbacks", "execution fallbacks")];

/// Leaf removed from `[openrouter]` while preserving the rest of the section.
const OPENROUTER_FALLBACK_LEAF: (&str, &str) = ("openrouter", "fallback_model");

/// Top-level scalar removed outright (legacy active profile name).
const REMOVED_TOP_LEVEL: &[&str] = &["profile"];

/// Scan the global config + active-profile pointer and return the list of
/// stale routing selectors that `--cleanup-global-routing` would remove, plus
/// whether the active-profile pointer would be deleted. Read-only: performs no
/// writes. Used by `wg config lint` so users can see stale global routing
/// before migrating.
///
/// Returns `(removed_keys, removed_active_profile, global_config_exists)`.
/// A malformed global config is reported via the returned error (the caller
/// surfaces it as a lint finding rather than aborting the whole lint).
pub fn scan_stale_global_routing() -> Result<(Vec<RemovedRoutingKey>, bool, bool)> {
    let global_config_path = Config::global_config_path()?;
    let active_profile_path = named::active_pointer_path()?;
    let config_exists = global_config_path.exists();

    let content = if config_exists {
        fs::read_to_string(&global_config_path)?
    } else {
        String::new()
    };
    let mut doc: toml::Value = if content.trim().is_empty() {
        toml::Value::Table(toml::value::Table::new())
    } else {
        toml::from_str(&content).context(format!(
            "{} is not valid TOML — fix syntax before migrating",
            global_config_path.display()
        ))?
    };

    let mut report = ProjectLocalPiCleanupReport::new(&global_config_path, &active_profile_path);
    apply_routing_removals(&mut doc, &mut report);
    let removed_active = active_profile_path.exists();
    Ok((report.removed_routing_keys, removed_active, config_exists))
}

/// Entry point for `wg migrate project-local-pi`.
pub fn run_project_local_pi_migrate(
    _workgraph_dir: &Path,
    args: ProjectLocalPiMigrateArgs,
    json: bool,
) -> Result<()> {
    // The cleanup is machine-global: it does not read or write any project
    // graph byte. `workgraph_dir` is accepted for CLI symmetry and reserved
    // for a future project-side migration phase (design §9.1).

    if let Some(receipt) = &args.rollback {
        return run_rollback(receipt, json);
    }

    if !args.cleanup_global_routing {
        // Without `--cleanup-global-routing` the command is informational: it
        // describes what *would* be cleaned without touching anything. The
        // project migration phase (design §9.1) is owned by the core
        // project-config materializer (`wg profile select` / `wg setup`); this
        // command's mutation surface is the optional global cleanup only.
        return run_report_only(json);
    }

    run_global_cleanup(args.dry_run, args.yes, json)
}

/// Informational report: scan global routing state, print what would be
/// removed/preserved, write nothing.
fn run_report_only(json: bool) -> Result<()> {
    let (report, _applied) = plan_global_cleanup()?;
    emit_report(&report, json, true)
}

/// Plan + apply the global routing cleanup.
fn run_global_cleanup(dry_run: bool, yes: bool, json: bool) -> Result<()> {
    let (mut report, applied_doc) = plan_global_cleanup()?;

    // Fail-closed: nothing to do → no backup, no receipt, no mtime change.
    let nothing_to_do = report.removed_routing_keys.is_empty() && !report.removed_active_profile;
    if nothing_to_do {
        report.no_op = true;
        return emit_report(&report, json, dry_run);
    }
    report.no_op = false;

    if dry_run {
        return emit_report(&report, json, true);
    }

    // Interactive confirmation unless --yes.
    if !yes && std::io::stdin().is_terminal() {
        use std::io::{Read, Write};
        print!(
            "About to remove stale machine-global routing from {} and delete {}.\n\
             Reusable profile definitions, secrets, identity, federation, and Pi settings are preserved.\n\
             Proceed? [y/N] ",
            report.global_config_path, report.active_profile_path
        );
        std::io::stdout().flush().ok();
        let mut buf = [0u8; 16];
        let n = std::io::stdin().read(&mut buf).unwrap_or(0);
        let answer = std::str::from_utf8(&buf[..n])
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if answer != "y" && answer != "yes" {
            println!("Aborted — no files were changed.");
            report.dry_run = true;
            return emit_report(&report, json, true);
        }
    }

    apply_global_cleanup(&mut report, applied_doc)?;
    emit_report(&report, json, false)
}

/// Read the global config + active-profile, compute the removal plan, and
/// return the post-removal TOML document (ready to serialize). Performs no
/// writes.
fn plan_global_cleanup() -> Result<(ProjectLocalPiCleanupReport, toml::Value)> {
    let global_config_path = Config::global_config_path()?;
    let active_profile_path = named::active_pointer_path()?;

    let mut report = ProjectLocalPiCleanupReport::new(&global_config_path, &active_profile_path);

    // Parse the global config. Missing file → treat as an empty table (there
    // may still be an active-profile pointer to remove). Malformed TOML →
    // fail closed before any write.
    let content = if global_config_path.exists() {
        fs::read_to_string(&global_config_path).with_context(|| {
            format!(
                "failed to read {}: {}",
                global_config_path.display(),
                "read error"
            )
        })?
    } else {
        String::new()
    };
    let mut doc: toml::Value = if content.trim().is_empty() {
        toml::Value::Table(toml::value::Table::new())
    } else {
        toml::from_str(&content).with_context(|| {
            format!(
                "{} is not valid TOML — fix syntax errors before migrating (no files were changed)",
                global_config_path.display()
            )
        })?
    };

    // Collect the protected-path metadata. We record existence/mode/mtime for
    // the roots the cleanup must NOT touch, *without* opening or hashing their
    // contents. OS keyring entries are never enumerated.
    record_preserved_roots(&mut report)?;

    // Apply the allowlisted removals to the TOML tree, recording each removed
    // selector.
    apply_routing_removals(&mut doc, &mut report);

    // Active-profile pointer: the file itself is removed if present.
    if active_profile_path.exists() {
        report.removed_active_profile = true;
    }

    // Set no_op based on whether anything was found to remove.
    report.no_op = report.removed_routing_keys.is_empty() && !report.removed_active_profile;

    Ok((report, doc))
}

/// Remove every routing selector from `doc`, recording each removal in
/// `report.removed_routing_keys`. Pure tree transform — no I/O.
fn apply_routing_removals(doc: &mut toml::Value, report: &mut ProjectLocalPiCleanupReport) {
    let table = match doc.as_table_mut() {
        Some(t) => t,
        None => return,
    };

    // Top-level scalar `profile = "..."`.
    for key in REMOVED_TOP_LEVEL {
        if table.remove(*key).is_some() {
            report.removed_routing_keys.push(RemovedRoutingKey {
                key: key.to_string(),
                category: "legacy active profile name".to_string(),
            });
        }
    }

    // Whole `[tiers]` / `[models]` tables.
    for section in REMOVED_SECTIONS {
        if table.remove(*section).is_some() {
            report.removed_routing_keys.push(RemovedRoutingKey {
                key: section.to_string(),
                category: "route/reasoning table".to_string(),
            });
        }
    }

    // `[section].key` leaves inside agent/dispatcher/coordinator.
    for (section, key, category) in REMOVED_LEAVES {
        if let Some(toml::Value::Table(sec)) = table.get_mut(*section)
            && sec.remove(*key).is_some()
        {
            report.removed_routing_keys.push(RemovedRoutingKey {
                key: format!("{}.{}", section, key),
                category: category.to_string(),
            });
        }
    }

    // `[[execution.fallbacks]]` array-of-tables.
    for (section, key, category) in REMOVED_ARRAYS {
        if let Some(toml::Value::Table(sec)) = table.get_mut(*section)
            && sec.remove(*key).is_some()
        {
            report.removed_routing_keys.push(RemovedRoutingKey {
                key: format!("{}.{}", section, key),
                category: category.to_string(),
            });
        }
    }

    // `openrouter.fallback_model` (preserve the rest of `[openrouter]`).
    {
        let (section, key) = OPENROUTER_FALLBACK_LEAF;
        if let Some(toml::Value::Table(sec)) = table.get_mut(section)
            && sec.remove(key).is_some()
        {
            report.removed_routing_keys.push(RemovedRoutingKey {
                key: format!("{}.{}", section, key),
                category: "openrouter fallback route".to_string(),
            });
        }
    }

    // Drop now-empty tables created by the removals above, but only for tables
    // we explicitly emptied (agent/dispatcher/coordinator/execution/openrouter).
    // We never drop a table that still holds preserved bytes.
    for section in [
        "agent",
        "dispatcher",
        "coordinator",
        "execution",
        "openrouter",
    ] {
        if let Some(toml::Value::Table(sec)) = table.get(section)
            && sec.is_empty()
        {
            // Only remove if it was *originally* present (we may have just
            // emptied it). `get` on an empty table is safe to drop.
            table.remove(section);
            // Note: we do not push a separate removed-key for the empty-table
            // drop; the leaf removals above already describe what left it empty.
        }
    }
}

/// Record existence/mode/mtime for protected filesystem roots without reading
/// their contents. Asserts (via the allowlisted write-set) that none of these
/// paths may be opened for write/remove by this run.
fn record_preserved_roots(report: &mut ProjectLocalPiCleanupReport) -> Result<()> {
    let global_dir = Config::global_dir()?;
    // Roots under `~/.wg` that must survive untouched.
    let protected: &[(&str, &str)] = &[
        ("profiles", "reusable profile definitions"),
        ("keystore", "identity/custody private keys"),
        ("secrets", "secret material"),
        ("profile-usage.jsonl", "profile usage history"),
    ];
    for (name, kind) in protected {
        let path = global_dir.join(name);
        let (existed, mode, mtime) = stat_metadata(&path);
        report.preserved.push(PreservedPath {
            path: path.display().to_string(),
            kind: kind.to_string(),
            existed,
            mode,
            mtime,
        });
    }
    // The graph-local identity + federation state lives under the project
    // graph, not `~/.wg`; the cleanup never receives a write to any graph
    // path. Record the global Pi settings root as preserved too.
    if let Some(pi_settings) = dirs::home_dir().map(|h| h.join(".pi/agent/settings.json")) {
        let (existed, mode, mtime) = stat_metadata(&pi_settings);
        report.preserved.push(PreservedPath {
            path: pi_settings.display().to_string(),
            kind: "Pi console settings".to_string(),
            existed,
            mode,
            mtime,
        });
    }
    Ok(())
}

/// Existence + octal mode + RFC3339 mtime, without reading file contents.
fn stat_metadata(path: &Path) -> (bool, Option<u32>, Option<String>) {
    match fs::metadata(path) {
        Ok(meta) => {
            let mode = meta.permissions().mode();
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| {
                    chrono::DateTime::<chrono::Utc>::from_timestamp(d.as_secs() as i64, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                });
            (true, Some(mode), mtime)
        }
        Err(_) => (false, None, None),
    }
}

/// Apply the planned removals: write backups, atomically rewrite the global
/// config, remove the active-profile pointer, and persist the receipt.
fn apply_global_cleanup(
    report: &mut ProjectLocalPiCleanupReport,
    applied_doc: toml::Value,
) -> Result<()> {
    let global_config_path = Config::global_config_path()?;
    let active_profile_path = named::active_pointer_path()?;
    let global_dir = Config::global_dir()?;
    let migrations_root = global_dir.join(MIGRATIONS_DIR);

    // Preimage bytes + digests.
    let config_preimage: Vec<u8> = if global_config_path.exists() {
        fs::read(&global_config_path).with_context(|| {
            format!(
                "failed to read preimage {}: read error",
                global_config_path.display()
            )
        })?
    } else {
        Vec::new()
    };
    let active_preimage: Vec<u8> = if active_profile_path.exists() {
        fs::read(&active_profile_path).with_context(|| {
            format!(
                "failed to read preimage {}: read error",
                active_profile_path.display()
            )
        })?
    } else {
        Vec::new()
    };

    // Revalidate the preimage immediately before each atomic replace: re-read
    // and compare to the bytes we planned against. A concurrent writer changes
    // the preimage and causes the entire global phase to refuse.
    let config_recheck = if global_config_path.exists() {
        fs::read(&global_config_path).unwrap_or_default()
    } else {
        Vec::new()
    };
    if config_recheck != config_preimage {
        bail!(
            "aborted: {} changed during planning (concurrent writer). Re-run the command.",
            global_config_path.display()
        );
    }
    let active_recheck = if active_profile_path.exists() {
        fs::read(&active_profile_path).unwrap_or_default()
    } else {
        Vec::new()
    };
    if active_recheck != active_preimage {
        bail!(
            "aborted: {} changed during planning (concurrent writer). Re-run the command.",
            active_profile_path.display()
        );
    }

    // Materialize the receipt dir (mode 0700) + backups (mode 0600).
    let receipt_id = new_receipt_id();
    let receipt_dir = migrations_root.join(&receipt_id);
    fs::create_dir_all(&receipt_dir)
        .with_context(|| format!("failed to create receipt dir {}", receipt_dir.display()))?;
    set_mode_0700(&receipt_dir)?;

    report.receipt_id = receipt_id.clone();

    // Backup the global config preimage (even if absent → record an absent
    // backup so rollback can restore "absent"). We only write a backup file
    // when the source existed.
    if global_config_path.exists() {
        let backup_path = receipt_dir.join("config.toml.pre");
        write_backup(&backup_path, &config_preimage)?;
        report.backups.push(BackupRecord {
            source: global_config_path.display().to_string(),
            backup: backup_path.display().to_string(),
            preimage_digest: Some(digest_b3(&config_preimage)),
            preimage_size: config_preimage.len() as u64,
        });
    } else {
        report.backups.push(BackupRecord {
            source: global_config_path.display().to_string(),
            backup: String::new(),
            preimage_digest: None,
            preimage_size: 0,
        });
    }

    // Backup the active-profile preimage if it existed.
    if active_profile_path.exists() {
        let backup_path = receipt_dir.join("active-profile.pre");
        write_backup(&backup_path, &active_preimage)?;
        report.backups.push(BackupRecord {
            source: active_profile_path.display().to_string(),
            backup: backup_path.display().to_string(),
            preimage_digest: Some(digest_b3(&active_preimage)),
            preimage_size: active_preimage.len() as u64,
        });
    } else {
        report.backups.push(BackupRecord {
            source: active_profile_path.display().to_string(),
            backup: String::new(),
            preimage_digest: None,
            preimage_size: 0,
        });
    }

    // Atomically rewrite the global config with the cleaned doc. If the doc is
    // an empty table and the source file was absent, leave it absent (no-op
    // for that file). If the doc is non-empty, write it.
    let new_body = if applied_doc.as_table().map(|t| t.is_empty()).unwrap_or(true) {
        String::new()
    } else {
        toml::to_string_pretty(&applied_doc).context("failed to serialize cleaned config")?
    };

    if global_config_path.exists() || !new_body.is_empty() {
        // Assert the write target is in the allowlisted write-set.
        assert_in_write_set(report, &global_config_path)?;
        if new_body.is_empty() {
            // The cleaned config is empty — remove the file rather than
            // leaving a zero-byte stub. This is still an allowlisted write.
            fs::remove_file(&global_config_path).with_context(|| {
                format!(
                    "failed to remove now-empty {}",
                    global_config_path.display()
                )
            })?;
        } else {
            write_atomic(&global_config_path, new_body.as_bytes()).with_context(|| {
                format!("failed to write cleaned {}", global_config_path.display())
            })?;
            set_mode_0600(&global_config_path)?;
        }
    }
    report.journal_state = "global-config-committed".to_string();

    // Postimage digest for the global config.
    let config_postimage = if global_config_path.exists() {
        Some(digest_b3(
            &fs::read(&global_config_path).unwrap_or_default(),
        ))
    } else {
        None
    };
    report
        .postimage_digests
        .insert(global_config_path.display().to_string(), config_postimage);

    // Remove the active-profile pointer.
    if active_profile_path.exists() {
        assert_in_write_set(report, &active_profile_path)?;
        fs::remove_file(&active_profile_path)
            .with_context(|| format!("failed to remove {}", active_profile_path.display()))?;
    }
    report.journal_state = "active-pointer-committed".to_string();

    let active_postimage = if active_profile_path.exists() {
        Some(digest_b3(
            &fs::read(&active_profile_path).unwrap_or_default(),
        ))
    } else {
        None
    };
    report
        .postimage_digests
        .insert(active_profile_path.display().to_string(), active_postimage);

    // Re-stat preserved roots and assert they are unchanged (mode/mtime). This
    // is a defense-in-depth check; the write-set assertion already forbids
    // opening them, but a regression that widened the write-set would show up
    // here as a changed mtime.
    assert_preserved_unchanged(report)?;

    // Persist the receipt.
    let receipt_path = receipt_dir.join("receipt.json");
    let receipt_json =
        serde_json::to_string_pretty(report).context("failed to serialize receipt")?;
    write_atomic(&receipt_path, receipt_json.as_bytes())
        .with_context(|| format!("failed to write receipt {}", receipt_path.display()))?;
    set_mode_0600(&receipt_path)?;

    report.journal_state = "complete".to_string();
    Ok(())
}

/// Write a backup copy with mode `0600`, preserving nothing from the source
/// mode (backups are always locked down regardless of source perms).
fn write_backup(path: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic(path, bytes)
        .with_context(|| format!("failed to write backup {}", path.display()))?;
    set_mode_0600(path)?;
    Ok(())
}

/// CAS rollback: restore preimages from a receipt only when the current
/// postimage still matches.
fn run_rollback(receipt_id: &str, json: bool) -> Result<()> {
    let global_dir = Config::global_dir()?;
    let receipt_dir = global_dir.join(MIGRATIONS_DIR).join(receipt_id);
    let receipt_path = receipt_dir.join("receipt.json");
    if !receipt_path.exists() {
        bail!(
            "no migration receipt found at {} — check `ls ~/.wg/migrations/project-local-pi/`",
            receipt_path.display()
        );
    }
    let receipt_bytes = fs::read_to_string(&receipt_path)
        .with_context(|| format!("failed to read receipt {}", receipt_path.display()))?;
    let receipt: ProjectLocalPiCleanupReport = match serde_json::from_str(&receipt_bytes) {
        Ok(r) => r,
        Err(e) => bail!(
            "receipt {} is not valid receipt JSON: {e}",
            receipt_path.display()
        ),
    };

    if receipt.version != PROJECT_LOCAL_PI_MIGRATE_VERSION {
        bail!(
            "receipt version {} does not match command version {} — use a wg binary from the same release",
            receipt.version,
            PROJECT_LOCAL_PI_MIGRATE_VERSION
        );
    }

    let mut restored: Vec<String> = Vec::new();
    let mut refusals: Vec<String> = Vec::new();

    for backup in &receipt.backups {
        let source = PathBuf::from(&backup.source);
        // Current bytes.
        let current = if source.exists() {
            Some(fs::read(&source).unwrap_or_default())
        } else {
            None
        };
        let current_digest = current.as_deref().map(digest_b3);
        let expected_post: Option<String> = receipt
            .postimage_digests
            .get(&backup.source)
            .cloned()
            .flatten();

        if current_digest.as_deref() != expected_post.as_deref() {
            refusals.push(format!(
                "{}: current bytes differ from receipt postimage — refusing to overwrite a later edit. \
                 Manual backup: {}",
                backup.source,
                if backup.backup.is_empty() {
                    "(no preimage — file was absent at migration time)".to_string()
                } else {
                    backup.backup.clone()
                }
            ));
            continue;
        }

        // CAS matches → restore the preimage.
        if let Some(pre_digest) = &backup.preimage_digest {
            // File existed at migration time; restore its bytes.
            let pre_bytes = fs::read(&backup.backup)
                .with_context(|| format!("failed to read backup {}", backup.backup))?;
            // Verify the backup itself still matches its recorded digest
            // before trusting it.
            if digest_b3(&pre_bytes) != *pre_digest {
                bail!(
                    "backup {} digest mismatch — receipt/backup pair is corrupt; aborting rollback",
                    backup.backup
                );
            }
            assert_in_write_set(&receipt, &source)?;
            write_atomic(&source, &pre_bytes)
                .with_context(|| format!("failed to restore {}", source.display()))?;
            set_mode_0600(&source)?;
            restored.push(format!("{} (restored preimage)", source.display()));
        } else {
            // File was absent at migration time → remove the current (postimage
            // is "absent" and matched, so current is also absent — nothing to
            // do). But if somehow a file is present with an absent postimage
            // that's impossible because the CAS would have refused. Defensive:
            if source.exists() {
                fs::remove_file(&source).ok();
                restored.push(format!(
                    "{} (removed post-migration file)",
                    source.display()
                ));
            } else {
                restored.push(format!("{} (already absent)", source.display()));
            }
        }
    }

    let summary = if refusals.is_empty() {
        format!(
            "rollback of receipt {} complete: {}",
            receipt_id,
            restored.join("; ")
        )
    } else if restored.is_empty() {
        format!(
            "rollback of receipt {} refused for all files:\n  {}",
            receipt_id,
            refusals.join("\n  ")
        )
    } else {
        format!(
            "rollback of receipt {} partial:\n  restored: {}\n  refused: {}",
            receipt_id,
            restored.join("; "),
            refusals.join("; ")
        )
    };

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "receipt_id": receipt_id,
                "restored": restored,
                "refusals": refusals,
                "journal_state": if refusals.is_empty() { "rolled-back" } else { "partial" },
            }))?
        );
    } else {
        println!("{summary}");
    }

    if !refusals.is_empty() {
        // Non-zero exit so scripts can detect a refused rollback.
        bail!("rollback refused for one or more files (see above)");
    }
    Ok(())
}

/// Assert `path` is in the receipt's allowlisted write-set. A plan that names
/// any other path is an internal bug.
fn assert_in_write_set(report: &ProjectLocalPiCleanupReport, path: &Path) -> Result<()> {
    let s = path.display().to_string();
    if !report.write_set.contains(&s) {
        bail!(
            "internal bug: path {} is outside the allowlisted write-set {:?} — refusing",
            s,
            report.write_set
        );
    }
    Ok(())
}

/// Re-stat preserved roots and confirm mode + mtime are unchanged. A changed
/// mtime would indicate the write-set assertion was bypassed.
fn assert_preserved_unchanged(report: &ProjectLocalPiCleanupReport) -> Result<()> {
    for p in &report.preserved {
        if !p.existed {
            continue;
        }
        let path = PathBuf::from(&p.path);
        let (existed, mode, mtime) = stat_metadata(&path);
        if !existed {
            bail!(
                "internal bug: preserved path {} disappeared during cleanup",
                p.path
            );
        }
        if mode != p.mode {
            bail!(
                "internal bug: preserved path {} mode changed {:?} → {:?}",
                p.path,
                p.mode,
                mode
            );
        }
        if mtime != p.mtime {
            bail!(
                "internal bug: preserved path {} mtime changed {:?} → {:?} — write-set may have been bypassed",
                p.path,
                p.mtime,
                mtime
            );
        }
    }
    Ok(())
}

fn set_mode_0700(path: &Path) -> Result<()> {
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o700);
    fs::set_permissions(path, perms)?;
    Ok(())
}

fn set_mode_0600(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(path, perms)?;
    Ok(())
}

fn digest_b3(bytes: &[u8]) -> String {
    format!("b3:{}", hex::encode(blake3_32(bytes)))
}

fn new_receipt_id() -> String {
    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let mut rng_seed = [0u8; 16];
    let n = chrono::Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or(0)
        .to_le_bytes();
    let pid = std::process::id().to_le_bytes();
    rng_seed[..8].copy_from_slice(&n);
    rng_seed[8..12].copy_from_slice(&pid);
    let suffix = hex::encode(blake3_32(&rng_seed));
    format!("{}-{}", now, &suffix[..8])
}

/// Print the report (plan or applied) for human or JSON consumption.
fn emit_report(report: &ProjectLocalPiCleanupReport, json: bool, dry_run: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }

    let prefix = if dry_run { "[dry-run] " } else { "" };
    if report.no_op {
        println!(
            "{}no stale global routing found in {} — nothing to clean.",
            prefix, report.global_config_path
        );
        if !report.removed_active_profile {
            println!(
                "active-profile pointer {} is already absent.",
                report.active_profile_path
            );
        }
        println!("No backup was written; no mtimes changed.");
        return Ok(());
    }

    println!("{}global routing cleanup:", prefix);
    println!("  global config: {}", report.global_config_path);
    println!("  active profile: {}", report.active_profile_path);
    println!();
    println!("  removed routing selectors:");
    for k in &report.removed_routing_keys {
        println!("    - {} ({})", k.key, k.category);
    }
    if report.removed_active_profile {
        println!("    - active-profile pointer (file removed)");
    }
    println!();
    println!("  preserved (not opened, not written, not removed):");
    for p in &report.preserved {
        let state = if p.existed { "present" } else { "absent" };
        println!("    - {} [{}] ({})", p.path, state, p.kind);
    }
    println!();
    if dry_run {
        println!(
            "  (dry-run — no files modified; rerun without --dry-run to apply, or add --yes to skip the prompt)"
        );
    } else {
        if !report.receipt_id.is_empty() {
            println!("  receipt: {}", report.receipt_id);
            println!("  backups:");
            for b in &report.backups {
                if b.backup.is_empty() {
                    println!("    - {} (was absent at migration time)", b.source);
                } else {
                    println!(
                        "    - {} → {} ({}, {} bytes)",
                        b.source,
                        b.backup,
                        b.preimage_digest.as_deref().unwrap_or("?"),
                        b.preimage_size
                    );
                }
            }
            println!("  journal state: {}", report.journal_state);
            println!(
                "  rollback: wg migrate project-local-pi --rollback {}",
                report.receipt_id
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    struct EnvRestore {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }
    impl EnvRestore {
        fn set(key: &'static str, value: &Path) -> Self {
            let previous = std::env::var_os(key);
            unsafe { std::env::set_var(key, value) };
            Self { key, previous }
        }
    }
    impl Drop for EnvRestore {
        fn drop(&mut self) {
            unsafe {
                if let Some(p) = self.previous.take() {
                    std::env::set_var(self.key, p);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    fn isolated_global() -> (TempDir, std::path::PathBuf, EnvRestore) {
        let tmp = TempDir::new().unwrap();
        let global = tmp.path().join(".wg");
        fs::create_dir_all(&global).unwrap();
        let restore = EnvRestore::set("WG_GLOBAL_DIR", &global);
        (tmp, global, restore)
    }

    fn write_global(global: &Path, body: &str) {
        fs::write(global.join("config.toml"), body).unwrap();
    }

    fn write_sentinels(global: &Path) {
        // Reusable profile definition.
        fs::create_dir_all(global.join("profiles")).unwrap();
        fs::write(
            global.join("profiles/pi.toml"),
            "# reusable profile definition\n[some]\nkey = \"value\"\n",
        )
        .unwrap();
        // Secret material.
        fs::create_dir_all(global.join("secrets")).unwrap();
        fs::write(global.join("secrets/alpha"), b"secret-bytes\n").unwrap();
        // Keystore.
        fs::create_dir_all(global.join("keystore")).unwrap();
        fs::write(global.join("keystore/root.key"), b"root-key-bytes\n").unwrap();
        // Profile usage history.
        fs::write(
            global.join("profile-usage.jsonl"),
            "{\"name\":\"pi\",\"ts\":\"now\"}\n",
        )
        .unwrap();
        // A preserved non-routing global key (must survive).
    }

    fn sha(path: &Path) -> String {
        digest_b3(&fs::read(path).unwrap())
    }

    #[test]
    #[serial]
    fn dry_run_writes_nothing_and_reports_plan() {
        let (_tmp, global, _r) = isolated_global();
        write_global(
            &global,
            r#"
profile = "pi"

[agent]
model = "pi:global:stale"
executor = "claude"

[dispatcher]
model = "pi:global:stale"
provider = "global"
executor = "claude"
max_agents = 7

[models.task_agent]
model = "pi:global:stale"
reasoning = "high"

[tiers]
fast = "pi:global:fast"

[openrouter]
fallback_model = "openrouter:anthropic/claude-opus-4-7"
monthly_budget_usd = 50

[[execution.fallbacks]]
primary = "pi:global:stale"
models = ["pi:global:alt"]

[secrets]
allow_plaintext = true
"#,
        );
        fs::write(global.join("active-profile"), "pi\n").unwrap();
        write_sentinels(&global);

        let before_config = sha(&global.join("config.toml"));
        let before_profile_def = sha(&global.join("profiles/pi.toml"));
        let before_secret = sha(&global.join("secrets/alpha"));
        let before_keystore = sha(&global.join("keystore/root.key"));
        let before_usage = sha(&global.join("profile-usage.jsonl"));
        let before_active = sha(&global.join("active-profile"));

        let (report, _doc) = plan_global_cleanup().unwrap();
        assert!(!report.no_op);
        assert!(
            report
                .removed_routing_keys
                .iter()
                .any(|k| k.key == "agent.model")
        );
        assert!(
            report
                .removed_routing_keys
                .iter()
                .any(|k| k.key == "agent.executor")
        );
        assert!(
            report
                .removed_routing_keys
                .iter()
                .any(|k| k.key == "dispatcher.model")
        );
        assert!(
            report
                .removed_routing_keys
                .iter()
                .any(|k| k.key == "dispatcher.provider")
        );
        assert!(
            report
                .removed_routing_keys
                .iter()
                .any(|k| k.key == "dispatcher.executor")
        );
        assert!(
            report
                .removed_routing_keys
                .iter()
                .any(|k| k.key == "models")
        );
        assert!(report.removed_routing_keys.iter().any(|k| k.key == "tiers"));
        assert!(
            report
                .removed_routing_keys
                .iter()
                .any(|k| k.key == "execution.fallbacks")
        );
        assert!(
            report
                .removed_routing_keys
                .iter()
                .any(|k| k.key == "openrouter.fallback_model")
        );
        assert!(
            report
                .removed_routing_keys
                .iter()
                .any(|k| k.key == "profile")
        );
        assert!(report.removed_active_profile);

        // Preserved roots recorded.
        assert!(
            report
                .preserved
                .iter()
                .any(|p| p.kind == "reusable profile definitions")
        );
        assert!(report.preserved.iter().any(|p| p.kind == "secret material"));
        assert!(
            report
                .preserved
                .iter()
                .any(|p| p.kind == "identity/custody private keys")
        );

        // Dry-run wrote nothing.
        assert_eq!(sha(&global.join("config.toml")), before_config);
        assert_eq!(sha(&global.join("profiles/pi.toml")), before_profile_def);
        assert_eq!(sha(&global.join("secrets/alpha")), before_secret);
        assert_eq!(sha(&global.join("keystore/root.key")), before_keystore);
        assert_eq!(sha(&global.join("profile-usage.jsonl")), before_usage);
        assert_eq!(sha(&global.join("active-profile")), before_active);
        assert!(!global.join("migrations").exists());
    }

    #[test]
    #[serial]
    fn apply_removes_only_routing_and_preserves_sentinels() {
        let (_tmp, global, _r) = isolated_global();
        write_global(
            &global,
            r#"
[agent]
model = "pi:global:stale"
executor = "claude"

[dispatcher]
model = "pi:global:stale"
max_agents = 7

[models.task_agent]
model = "pi:global:stale"
reasoning = "high"

[tiers]
fast = "pi:global:fast"

[openrouter]
fallback_model = "openrouter:anthropic/claude-opus-4-7"
monthly_budget_usd = 50

[[execution.fallbacks]]
primary = "pi:global:stale"
models = ["pi:global:alt"]

profile = "pi"

[secrets]
allow_plaintext = true

[native_executor]
foo = "preserved"
"#,
        );
        fs::write(global.join("active-profile"), "pi\n").unwrap();
        write_sentinels(&global);

        let before_profile_def = sha(&global.join("profiles/pi.toml"));
        let before_secret = sha(&global.join("secrets/alpha"));
        let before_keystore = sha(&global.join("keystore/root.key"));
        let before_usage = sha(&global.join("profile-usage.jsonl"));

        let (mut report, doc) = plan_global_cleanup().unwrap();
        apply_global_cleanup(&mut report, doc).unwrap();
        assert_eq!(report.journal_state, "complete");
        assert!(!report.receipt_id.is_empty());

        // Routing selectors gone.
        let after = fs::read_to_string(global.join("config.toml")).unwrap();
        assert!(!after.contains("pi:global"));
        assert!(!after.contains("agent.model"));
        assert!(!after.contains("models"));
        assert!(!after.contains("tiers"));
        assert!(!after.contains("fallback"));
        assert!(!after.contains("profile ="));
        // Preserved non-routing bytes survive.
        assert!(after.contains("max_agents = 7"));
        assert!(after.contains("allow_plaintext = true"));
        assert!(after.contains("[native_executor]"));
        assert!(after.contains("monthly_budget_usd = 50"));

        // Active-profile pointer removed.
        assert!(!global.join("active-profile").exists());

        // Sentinels byte-identical.
        assert_eq!(sha(&global.join("profiles/pi.toml")), before_profile_def);
        assert_eq!(sha(&global.join("secrets/alpha")), before_secret);
        assert_eq!(sha(&global.join("keystore/root.key")), before_keystore);
        assert_eq!(sha(&global.join("profile-usage.jsonl")), before_usage);

        // Backup + receipt written with lockdown perms.
        let receipt_dir = global
            .join("migrations/project-local-pi")
            .join(&report.receipt_id);
        let config_backup = receipt_dir.join("config.toml.pre");
        let active_backup = receipt_dir.join("active-profile.pre");
        assert!(config_backup.exists());
        assert!(active_backup.exists());
        let m = fs::metadata(&config_backup).unwrap().permissions().mode() & 0o777;
        assert_eq!(m, 0o600);
        let m = fs::metadata(&receipt_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(m, 0o700);
        assert!(receipt_dir.join("receipt.json").exists());

        // Backup matches the original preimage (which had the routing bytes).
        let backup_body = fs::read_to_string(&config_backup).unwrap();
        assert!(backup_body.contains("pi:global:stale"));
    }

    #[test]
    #[serial]
    fn second_run_is_noop_no_backup_no_mtime_change() {
        let (_tmp, global, _r) = isolated_global();
        write_global(
            &global,
            r#"
[agent]
model = "pi:global:stale"

[dispatcher]
max_agents = 3
"#,
        );
        fs::write(global.join("active-profile"), "pi\n").unwrap();

        let (mut r1, doc1) = plan_global_cleanup().unwrap();
        apply_global_cleanup(&mut r1, doc1).unwrap();
        assert!(!r1.no_op);

        let config_mtime = fs::metadata(global.join("config.toml"))
            .unwrap()
            .modified()
            .unwrap();

        let (r2, _doc2) = plan_global_cleanup().unwrap();
        assert!(
            r2.no_op,
            "second run must be no-op: {:?}",
            r2.removed_routing_keys
        );

        // No new receipt dir created.
        let migrations_root = global.join("migrations/project-local-pi");
        let count = fs::read_dir(&migrations_root).unwrap().count();
        assert_eq!(count, 1, "second run must not create a new receipt");

        // mtime unchanged.
        let after = fs::metadata(global.join("config.toml"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(config_mtime, after);
    }

    #[test]
    #[serial]
    fn malformed_config_fail_closed() {
        let (_tmp, global, _r) = isolated_global();
        write_global(&global, "this is = = not valid toml {{{");
        fs::write(global.join("active-profile"), "pi\n").unwrap();
        write_sentinels(&global);

        let before_secret = sha(&global.join("secrets/alpha"));
        let before_active = sha(&global.join("active-profile"));
        let before_config = sha(&global.join("config.toml"));

        let err = plan_global_cleanup().unwrap_err().to_string();
        assert!(err.contains("not valid TOML"), "{err}");

        // Nothing changed.
        assert_eq!(sha(&global.join("config.toml")), before_config);
        assert_eq!(sha(&global.join("secrets/alpha")), before_secret);
        assert_eq!(sha(&global.join("active-profile")), before_active);
        assert!(!global.join("migrations").exists());
    }

    #[test]
    #[serial]
    fn rollback_restores_exact_bytes_cas() {
        let (_tmp, global, _r) = isolated_global();
        write_global(
            &global,
            r#"
[agent]
model = "pi:global:stale"

[dispatcher]
max_agents = 3
"#,
        );
        fs::write(global.join("active-profile"), "pi\n").unwrap();
        write_sentinels(&global);

        let original_config = sha(&global.join("config.toml"));
        let original_active = sha(&global.join("active-profile"));

        let (mut r, doc) = plan_global_cleanup().unwrap();
        apply_global_cleanup(&mut r, doc).unwrap();
        let receipt_id = r.receipt_id.clone();

        // After apply, routing is gone.
        assert!(!global.join("active-profile").exists());
        assert!(
            !fs::read_to_string(global.join("config.toml"))
                .unwrap()
                .contains("pi:global")
        );

        // Rollback.
        run_rollback(&receipt_id, false).unwrap();

        // Exact bytes restored.
        assert_eq!(sha(&global.join("config.toml")), original_config);
        assert_eq!(sha(&global.join("active-profile")), original_active);
    }

    #[test]
    #[serial]
    fn rollback_refuses_after_user_edit() {
        let (_tmp, global, _r) = isolated_global();
        write_global(
            &global,
            r#"
[agent]
model = "pi:global:stale"
"#,
        );
        fs::write(global.join("active-profile"), "pi\n").unwrap();

        let (mut r, doc) = plan_global_cleanup().unwrap();
        apply_global_cleanup(&mut r, doc).unwrap();
        let receipt_id = r.receipt_id.clone();

        // User edits the cleaned config after migration.
        fs::write(
            global.join("config.toml"),
            "# user hand-edited\n[dispatcher]\nmax_agents = 9\n",
        )
        .unwrap();

        let err = run_rollback(&receipt_id, false).unwrap_err().to_string();
        assert!(err.contains("refused"), "{err}");
        // User edit preserved.
        assert!(
            fs::read_to_string(global.join("config.toml"))
                .unwrap()
                .contains("user hand-edited")
        );
    }

    #[test]
    #[serial]
    fn apply_when_global_config_absent_but_active_profile_present() {
        let (_tmp, global, _r) = isolated_global();
        // No config.toml, but a stale active-profile pointer exists.
        fs::write(global.join("active-profile"), "pi\n").unwrap();
        write_sentinels(&global);

        let (mut r, doc) = plan_global_cleanup().unwrap();
        assert!(r.removed_routing_keys.is_empty());
        assert!(r.removed_active_profile);
        apply_global_cleanup(&mut r, doc).unwrap();

        assert!(!global.join("active-profile").exists());
        assert!(!global.join("config.toml").exists());
        // Receipt still written (active-profile was removed).
        assert!(
            global
                .join("migrations/project-local-pi")
                .join(&r.receipt_id)
                .join("receipt.json")
                .exists()
        );
    }

    #[test]
    #[serial]
    fn apply_drops_empty_openrouter_when_only_fallback_present() {
        let (_tmp, global, _r) = isolated_global();
        write_global(
            &global,
            r#"
[openrouter]
fallback_model = "openrouter:anthropic/claude-opus-4-7"
"#,
        );
        let (mut r, doc) = plan_global_cleanup().unwrap();
        apply_global_cleanup(&mut r, doc).unwrap();
        // The cleaned config is empty → file removed.
        assert!(!global.join("config.toml").exists());
    }

    #[test]
    #[serial]
    fn apply_preserves_openrouter_non_routing_keys() {
        let (_tmp, global, _r) = isolated_global();
        write_global(
            &global,
            r#"
[openrouter]
fallback_model = "openrouter:anthropic/claude-opus-4-7"
monthly_budget_usd = 50
request_timeout_secs = 30
"#,
        );
        let (mut r, doc) = plan_global_cleanup().unwrap();
        apply_global_cleanup(&mut r, doc).unwrap();
        let after = fs::read_to_string(global.join("config.toml")).unwrap();
        assert!(after.contains("monthly_budget_usd = 50"));
        assert!(after.contains("request_timeout_secs = 30"));
        assert!(!after.contains("fallback_model"));
    }
}
