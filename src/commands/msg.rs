//! Message queue CLI commands: send, list, read, poll.

use anyhow::{Context, Result};
use std::io::Read;
use std::path::Path;

use worksgood::graph::{
    Node, create_user_board_task, is_user_board, next_user_board_seq, resolve_user_board_alias,
};
use worksgood::messages;
use worksgood::parser::modify_graph;

/// Status icon for CLI display.
fn status_icon_for(status: &messages::DeliveryStatus) -> &'static str {
    match status {
        messages::DeliveryStatus::Sent => "\u{2709}",       // ✉
        messages::DeliveryStatus::Delivered => "\u{1f4ec}", // 📬
        messages::DeliveryStatus::Read => "\u{1f441}",      // 👁
        messages::DeliveryStatus::Acknowledged => "\u{2705}", // ✅
    }
}

/// Send a message to a task's queue.
pub fn run_send(
    dir: &Path,
    task_id: &str,
    body: Option<&str>,
    sender: &str,
    priority: &str,
    stdin: bool,
) -> Result<()> {
    // Resolve user board alias (.user-erik → .user-erik-N)
    let (graph, _path) = super::load_workgraph(dir)?;
    let resolved_id = resolve_user_board_alias(&graph, task_id);

    // Lazy init: if targeting a user board that doesn't exist, auto-create it
    let effective_id = if graph.get_task(&resolved_id).is_none() && is_user_board(&resolved_id) {
        // Extract handle from the alias (e.g., ".user-erik" → "erik")
        let handle = resolved_id.strip_prefix(".user-").unwrap_or(&resolved_id);
        // Strip trailing -N if it's there
        let handle = if let Some(pos) = handle.rfind('-') {
            let suffix = &handle[pos + 1..];
            if suffix.parse::<u32>().is_ok() {
                &handle[..pos]
            } else {
                handle
            }
        } else {
            handle
        };
        let seq = next_user_board_seq(&graph, handle);
        let task = create_user_board_task(handle, seq);
        let new_id = task.id.clone();
        let graph_path = super::graph_path(dir);
        modify_graph(&graph_path, |graph| {
            graph.add_node(Node::Task(task));
            true
        })
        .context("Failed to auto-create user board")?;
        eprintln!("Auto-created user board '{}'", new_id);
        super::notify_graph_changed(dir);
        new_id
    } else if graph.get_task(&resolved_id).is_some() {
        resolved_id
    } else {
        // Not a user board — use original error path
        graph.get_task_or_err(task_id)?;
        task_id.to_string()
    };

    let message_body = if stdin {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("Failed to read from stdin")?;
        buf.trim_end().to_string()
    } else {
        body.ok_or_else(|| anyhow::anyhow!("Message body required (or use --stdin)"))?
            .to_string()
    };

    if message_body.is_empty() {
        anyhow::bail!("Message body cannot be empty");
    }

    let id = messages::send_message(dir, &effective_id, &message_body, sender, priority)?;
    println!("Message #{} sent to '{}'", id, effective_id);

    Ok(())
}

/// List all messages for a task.
pub fn run_list(dir: &Path, task_id: &str, json: bool) -> Result<()> {
    // Resolve user board alias and validate task exists
    let (graph, _path) = super::load_workgraph(dir)?;
    let task_id = resolve_user_board_alias(&graph, task_id);
    graph.get_task_or_err(&task_id)?;

    let msgs = messages::list_messages(dir, &task_id)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&msgs)?);
        return Ok(());
    }

    if msgs.is_empty() {
        println!("No messages for task '{}'", task_id);
        return Ok(());
    }

    println!("Messages for '{}' ({} total):", task_id, msgs.len());
    println!();

    for msg in &msgs {
        let priority_marker = if msg.priority == "urgent" {
            " [URGENT]"
        } else {
            ""
        };
        let status_icon = status_icon_for(&msg.status);
        println!(
            "  #{} {} [{}] {}{}",
            msg.id, status_icon, msg.timestamp, msg.sender, priority_marker
        );
        // Indent multi-line bodies
        for line in msg.body.lines() {
            println!("    {}", line);
        }
        println!();
    }

    Ok(())
}

/// Read unread messages for an agent (advances cursor).
pub fn run_read(dir: &Path, task_id: &str, agent_id: &str, json: bool) -> Result<()> {
    // Resolve user board alias and validate task exists
    let (graph, _path) = super::load_workgraph(dir)?;
    let task_id = resolve_user_board_alias(&graph, task_id);
    graph.get_task_or_err(&task_id)?;

    let unread = messages::read_unread(dir, &task_id, agent_id)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&unread)?);
        return Ok(());
    }

    if unread.is_empty() {
        println!(
            "No unread messages for task '{}' (agent: {})",
            task_id, agent_id
        );
        return Ok(());
    }

    println!(
        "{} unread message{} for '{}' (agent: {}):",
        unread.len(),
        if unread.len() == 1 { "" } else { "s" },
        task_id,
        agent_id
    );
    println!();

    for msg in &unread {
        let priority_marker = if msg.priority == "urgent" {
            " [URGENT]"
        } else {
            ""
        };
        let status_icon = status_icon_for(&msg.status);
        println!(
            "  #{} {} [{}] {}{}",
            msg.id, status_icon, msg.timestamp, msg.sender, priority_marker
        );
        for line in msg.body.lines() {
            println!("    {}", line);
        }
        println!();
    }

    Ok(())
}

/// Poll for new messages (exit code 0 = new messages, 1 = none).
///
/// Does NOT advance the cursor (messages remain "unread" for `read`).
pub fn run_poll(dir: &Path, task_id: &str, agent_id: &str, json: bool) -> Result<bool> {
    // Resolve user board alias and validate task exists
    let (graph, _path) = super::load_workgraph(dir)?;
    let task_id = resolve_user_board_alias(&graph, task_id);
    graph.get_task_or_err(&task_id)?;

    let new_msgs = messages::poll_messages(dir, &task_id, agent_id)?;

    if new_msgs.is_empty() {
        if !json {
            println!(
                "No new messages for task '{}' (agent: {})",
                task_id, agent_id
            );
        } else {
            println!("[]");
        }
        return Ok(false);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&new_msgs)?);
    } else {
        println!(
            "{} new message{} for '{}' (agent: {}):",
            new_msgs.len(),
            if new_msgs.len() == 1 { "" } else { "s" },
            task_id,
            agent_id
        );
        println!();
        for msg in &new_msgs {
            let priority_marker = if msg.priority == "urgent" {
                " [URGENT]"
            } else {
                ""
            };
            let status_icon = status_icon_for(&msg.status);
            println!(
                "  #{} {} [{}] {}{}",
                msg.id, status_icon, msg.timestamp, msg.sender, priority_marker
            );
            for line in msg.body.lines() {
                println!("    {}", line);
            }
            println!();
        }
    }

    Ok(true)
}

// ── Cross-graph (key-based) messaging over the WG node inbox (Wave 4) ──────────
//
// `wg msg --to wgid:` routes a signed (optionally sealed) `SignedEvent` to a peer on
// another graph over the default transport rung (the node HTTP store-and-forward
// inbox, ADR-fed-002 §D1). Delivery target is found by the resolution cascade
// (`federation::resolve_peer_endpoint`: cached endpoint → directory → DHT); the
// authoring/verification machinery is shared with `wg identity send/poll`, so the
// transport is untrusted and every byte is self-verifying.

/// `wg msg send --to <wgid|peer> --from <identity> [--seal] [--store <url>]` — deliver
/// a signed cross-graph message to a recipient on another WG, over the node inbox.
///
/// `to` may be a raw `wgid:`/`did:key:` address or a `federation.yaml` peer name. The
/// delivery endpoint is resolved by the cascade unless `--store` overrides it with an
/// explicit node/dir URL. Tries each resolved endpoint in fallback-ladder order.
#[allow(clippy::too_many_arguments)]
pub fn run_send_fed(
    workgraph_dir: &Path,
    from: &str,
    to: &str,
    store_override: Option<&str>,
    body: &str,
    kind: &str,
    seal: bool,
    json: bool,
) -> Result<()> {
    use worksgood::federation::resolve_peer_endpoint;

    // Decide the delivery endpoints + the recipient's canonical wgid.
    let (recipient_wgid, endpoints): (String, Vec<String>) = if let Some(store) = store_override {
        // Explicit transport: `to` must be (or resolve to) a wgid for addressing.
        let resolved = resolve_peer_endpoint(to, workgraph_dir);
        let wgid = match resolved {
            Ok(r) => r.wgid,
            Err(_) => normalize_to_wgid(to)?,
        };
        (wgid, vec![store.to_string()])
    } else {
        let resolved = resolve_peer_endpoint(to, workgraph_dir).with_context(|| {
            format!("resolving delivery endpoints for '{to}' (pass --store to override)")
        })?;
        if !json {
            println!(
                "resolved {} via {} → {} endpoint(s)",
                resolved.wgid,
                resolved.source,
                resolved.endpoints.len()
            );
        }
        (resolved.wgid, resolved.endpoints)
    };

    if endpoints.is_empty() {
        anyhow::bail!("no delivery endpoints for '{to}'");
    }

    // Fallback ladder: try each endpoint until one accepts (ADR-fed-002 §D1/§D2).
    let mut last_err: Option<anyhow::Error> = None;
    for ep in &endpoints {
        match crate::commands::identity_cmd::run_send(
            workgraph_dir,
            from,
            std::slice::from_ref(&recipient_wgid),
            ep,
            body,
            kind,
            seal,
            false, // `wg msg` uses single-recipient sealing, not sealed-sender
            json,
        ) {
            Ok(()) => return Ok(()),
            Err(e) => {
                if !json {
                    eprintln!("  endpoint {ep} failed: {e}; trying next rung…");
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("delivery failed on all endpoints")))
}

/// `wg msg poll --as <identity> [--store <url>]` — poll this graph's node inbox for
/// signed cross-graph messages and authenticate each by key. With `--store` omitted,
/// uses the `node:` URL configured in `federation.yaml`.
pub fn run_poll_fed(
    workgraph_dir: &Path,
    as_identity: &str,
    store_override: Option<&str>,
    require_fresh: Option<&str>,
    json: bool,
) -> Result<()> {
    let store = match store_override {
        Some(s) => s.to_string(),
        None => {
            let cfg = worksgood::federation::load_federation_config(workgraph_dir)?;
            cfg.node.ok_or_else(|| {
                anyhow::anyhow!(
                    "no node inbox configured — set `node: http://host:port` in \
                     federation.yaml or pass --store <url>"
                )
            })?
        }
    };
    crate::commands::identity_cmd::run_poll(workgraph_dir, as_identity, &store, require_fresh, json)
}

/// Normalize a `wgid:`/`did:key:` address to the canonical `wgid:` (for `--store`
/// overrides where the cascade is bypassed).
fn normalize_to_wgid(to: &str) -> Result<String> {
    let pubkey = worksgood::identity::keys::pubkey_from_wgid(to)
        .with_context(|| format!("'{to}' is not a wgid:/did:key: address"))?;
    Ok(worksgood::identity::keys::wgid_from_pubkey(&pubkey))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use worksgood::graph::{Node, Task, WorkGraph};
    use worksgood::parser::save_graph;

    fn make_task(id: &str, title: &str) -> Task {
        Task {
            id: id.to_string(),
            title: title.to_string(),
            ..Task::default()
        }
    }

    fn setup_graph(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("task-1", "Test Task")));
        let path = super::super::graph_path(dir);
        save_graph(&graph, &path).unwrap();
    }

    #[test]
    fn test_send_message() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".wg");
        setup_graph(&dir);

        run_send(&dir, "task-1", Some("Hello world"), "user", "normal", false).unwrap();

        let msgs = messages::list_messages(&dir, "task-1").unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].body, "Hello world");
    }

    #[test]
    fn test_send_to_nonexistent_task() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".wg");
        setup_graph(&dir);

        let result = run_send(&dir, "nonexistent", Some("Hello"), "user", "normal", false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_send_empty_body_fails() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".wg");
        setup_graph(&dir);

        let result = run_send(&dir, "task-1", Some(""), "user", "normal", false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn test_list_messages() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".wg");
        setup_graph(&dir);

        run_send(&dir, "task-1", Some("First"), "user", "normal", false).unwrap();
        run_send(
            &dir,
            "task-1",
            Some("Second"),
            "coordinator",
            "urgent",
            false,
        )
        .unwrap();

        // Should not error
        let result = run_list(&dir, "task-1", false);
        assert!(result.is_ok());

        let result = run_list(&dir, "task-1", true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_read_and_poll() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".wg");
        setup_graph(&dir);

        run_send(&dir, "task-1", Some("Message"), "user", "normal", false).unwrap();

        // Poll returns true (has messages)
        let has_new = run_poll(&dir, "task-1", "agent-1", false).unwrap();
        assert!(has_new);

        // Read advances cursor
        let result = run_read(&dir, "task-1", "agent-1", false);
        assert!(result.is_ok());

        // Poll returns false (no new messages)
        let has_new = run_poll(&dir, "task-1", "agent-1", false).unwrap();
        assert!(!has_new);
    }
}
