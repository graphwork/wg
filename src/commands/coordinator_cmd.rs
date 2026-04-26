//! CLI handlers for `wg coordinator` subcommands.

use anyhow::Result;
use std::path::Path;

use workgraph::chat_sessions::{self, SessionKind};

/// `wg coordinator list [--archived] [--all]`
pub fn run_list(dir: &Path, archived: bool, all: bool, json: bool) -> Result<()> {
    let sessions = if all {
        chat_sessions::list(dir)?
    } else if archived {
        chat_sessions::list_archived(dir)?
    } else {
        chat_sessions::list_active(dir)?
    };

    let coordinators: Vec<_> = sessions
        .into_iter()
        .filter(|(_, meta)| meta.kind == SessionKind::Coordinator)
        .collect();

    if json {
        let items: Vec<serde_json::Value> = coordinators
            .iter()
            .map(|(uuid, meta)| {
                let coord_name = meta
                    .aliases
                    .iter()
                    .find(|a| a.starts_with("coordinator-"))
                    .cloned()
                    .unwrap_or_default();
                serde_json::json!({
                    "uuid": uuid,
                    "name": coord_name,
                    "label": meta.label,
                    "created": meta.created,
                    "archived_at": meta.archived_at,
                    "aliases": meta.aliases,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else if coordinators.is_empty() {
        let what = if all {
            ""
        } else if archived {
            "archived "
        } else {
            "active "
        };
        println!("No {}coordinator sessions.", what);
    } else {
        let header = if all {
            "All"
        } else if archived {
            "Archived"
        } else {
            "Active"
        };
        println!("{} coordinator sessions:", header);
        for (uuid, meta) in &coordinators {
            let coord_name = meta
                .aliases
                .iter()
                .find(|a| a.starts_with("coordinator-"))
                .cloned()
                .unwrap_or_else(|| uuid[..8].to_string());
            let label = meta
                .label
                .as_deref()
                .unwrap_or("");
            let status = if meta.archived_at.is_some() {
                " [archived]"
            } else {
                ""
            };
            println!("  {} — {}{}", coord_name, label, status);
        }
    }

    Ok(())
}

/// Normalize a coordinator reference: accept "3", "coordinator-3", etc.
fn normalize_coord_ref(name: &str) -> String {
    if name.parse::<u32>().is_ok() {
        format!("coordinator-{}", name)
    } else {
        name.to_string()
    }
}

/// `wg coordinator archive <name>`
pub fn run_archive(dir: &Path, name: &str, json: bool) -> Result<()> {
    let reference = normalize_coord_ref(name);
    let uuid = chat_sessions::archive_session(dir, &reference)?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "archived": true,
                "name": reference,
                "uuid": uuid,
            }))?
        );
    } else {
        println!("Archived coordinator session '{}'.", reference);
        println!("Chat history preserved in .archive/. Use `wg coordinator restore {}` to bring it back.", name);
    }

    Ok(())
}

/// `wg coordinator restore <name>`
pub fn run_restore(dir: &Path, name: &str, json: bool) -> Result<()> {
    let reference = normalize_coord_ref(name);
    let uuid = chat_sessions::restore_session(dir, &reference)?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "restored": true,
                "name": reference,
                "uuid": uuid,
            }))?
        );
    } else {
        println!("Restored coordinator session '{}'.", reference);
    }

    Ok(())
}
