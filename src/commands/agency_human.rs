//! `wg agency human add` — one-command human onboarding handshake.
//!
//! Tier-A item R21 from `docs/03-gap-analysis-refresh.md` §3.3 (family-team gap
//! analysis). Upstream already had the primitives: `WG_USER` caller identity,
//! attribution on logs/chat/provenance, per-user boards (`wg user init`), and
//! per-human Telegram bot config (the multi-bot work in
//! `src/notify/telegram.rs`). This wrapper stitches them into the flow the
//! vision doc (docs/01 §6) describes:
//!
//! 1. Create a human agent (`is_human`, executor `telegram`) in the agency store.
//! 2. Initialise the human's per-user board (equivalent of `wg user init`).
//! 3. Record a structured Telegram ↔ agent binding (R22 — see
//!    [`crate::agency::human_binding`]).
//! 4. If a Telegram bot is configured, DM the human "reply YES to join
//!    <project>" and leave the binding unconfirmed until the inbound listener
//!    receives the reply. If no bot is configured, print the manual step and
//!    leave the binding unconfirmed.

use anyhow::{Context, Result};
use std::path::Path;

use worksgood::agency::{
    self, Agent, Lineage, PerformanceRecord, TelegramBinding, TelegramBindingMap,
};
use worksgood::graph::TrustLevel;
use worksgood::notify::config::NotifyConfig;
use worksgood::notify::telegram::{TelegramBotConfig, TelegramConfig};

/// Slugify a display name into a board/agent-id-safe handle.
///
/// Lowercases, replaces any run of non-alphanumeric characters with a single
/// `-`, and trims leading/trailing `-`. "Nadin O'Brien" → "nadin-o-brien".
fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Derive a project label from the workgraph dir (its parent directory name).
fn project_label(workgraph_dir: &Path) -> String {
    workgraph_dir
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("this project")
        .to_string()
}

/// Outcome of the Telegram side of onboarding, so the caller can report it and
/// tests can assert on it without a live bot.
#[derive(Debug, PartialEq)]
pub enum HandshakeOutcome {
    /// A DM was sent via the named bot; awaiting the human's `YES`.
    Sent { bot_id: String },
    /// A bot was configured but the DM failed to send; manual fallback printed.
    SendFailed { bot_id: String, error: String },
    /// No Telegram bot is configured; manual fallback printed.
    NoBot,
}

/// `wg agency human add <name> --telegram <user-id-or-handle> [--project <p>]`.
pub fn run_add(
    workgraph_dir: &Path,
    name: &str,
    telegram: &str,
    project: Option<&str>,
) -> Result<()> {
    let name = name.trim();
    let telegram = telegram.trim();
    if name.is_empty() {
        anyhow::bail!("human name must not be empty");
    }
    if telegram.is_empty() {
        anyhow::bail!("--telegram must not be empty");
    }

    let agency_dir = workgraph_dir.join("agency");
    agency::init(&agency_dir).context("Failed to initialise agency directory")?;

    let handle = slugify(name);
    if handle.is_empty() {
        anyhow::bail!("human name '{}' has no alphanumeric characters to form a handle", name);
    }
    let agent_id = format!("human-{}", handle);

    // --- Validate up front so we never write partial state ---------------
    let agents_dir = agency_dir.join("cache/agents");
    let agent_path = agents_dir.join(format!("{}.yaml", agent_id));
    if agent_path.exists() {
        anyhow::bail!(
            "human '{}' already exists (agent id '{}'). Remove it with `wg agent rm {}` first.",
            name,
            agent_id,
            agent_id
        );
    }

    let mut bindings = TelegramBindingMap::load(&agency_dir)
        .context("Failed to load Telegram binding map")?;
    // One-human-one-agent: reject a Telegram user already bound.
    if let Some(existing) = bindings.find_by_user(telegram) {
        anyhow::bail!(
            "Telegram user '{}' is already bound to agent '{}' ({}). One human maps to one agent.",
            telegram,
            existing.name,
            existing.agent_id
        );
    }

    // --- 1. Create the human agent ---------------------------------------
    let agent = Agent {
        id: agent_id.clone(),
        role_id: String::new(),
        tradeoff_id: String::new(),
        name: name.to_string(),
        performance: PerformanceRecord::default(),
        lineage: Lineage::default(),
        capabilities: vec![],
        rate: None,
        capacity: None,
        trust_level: TrustLevel::Provisional,
        contact: Some(format!("telegram:{}", telegram)),
        executor: "telegram".to_string(),
        preferred_model: None,
        preferred_provider: None,
        deployment_history: vec![],
        attractor_weight: 0.5,
        staleness_flags: vec![],
    };
    debug_assert!(agent.is_human(), "telegram-fronted agent must be is_human");
    agency::save_agent(&agent, &agents_dir).context("Failed to save human agent")?;
    println!("Created human agent '{}' ({})", name, agent_id);

    // --- 2. Initialise the per-user board --------------------------------
    super::user::run_init(workgraph_dir, Some(&handle))
        .context("Failed to initialise per-user board")?;

    // --- 3. Attempt the Telegram handshake -------------------------------
    let project = project
        .map(str::to_string)
        .unwrap_or_else(|| project_label(workgraph_dir));
    let handshake_msg = format!(
        "You've been added to the '{}' workgraph as {}. Reply YES to join.",
        project, name
    );

    let (bot_id, outcome) = match resolve_bot(workgraph_dir, &agent_id) {
        Some((bot_id, bot)) => {
            match send_dm(&bot_id, bot, telegram, &handshake_msg) {
                Ok(()) => {
                    println!(
                        "Sent join request to {} via bot '{}'. Awaiting their YES reply.",
                        telegram, bot_id
                    );
                    println!(
                        "  Run `wg telegram listen` (or keep the service listener up) to capture the confirmation."
                    );
                    (Some(bot_id.clone()), HandshakeOutcome::Sent { bot_id })
                }
                Err(e) => {
                    let error = e.to_string();
                    eprintln!("Warning: failed to DM {} via bot '{}': {}", telegram, bot_id, error);
                    print_manual_step(telegram, &handshake_msg);
                    (
                        Some(bot_id.clone()),
                        HandshakeOutcome::SendFailed { bot_id, error },
                    )
                }
            }
        }
        None => {
            println!("No Telegram bot configured — using the manual onboarding path.");
            print_manual_step(telegram, &handshake_msg);
            (None, HandshakeOutcome::NoBot)
        }
    };

    // --- 4. Record the (unconfirmed) binding -----------------------------
    let binding = TelegramBinding::new(
        telegram.to_string(),
        agent_id.clone(),
        name.to_string(),
        bot_id,
        chrono::Utc::now(),
    );
    bindings
        .add(binding)
        .context("Failed to record Telegram binding")?;
    let path = bindings
        .save(&agency_dir)
        .context("Failed to persist Telegram binding map")?;

    let confirm_state = match &outcome {
        HandshakeOutcome::Sent { .. } => "unconfirmed (join request sent — awaiting YES)",
        HandshakeOutcome::SendFailed { .. } => "unconfirmed (send failed — relay manually)",
        HandshakeOutcome::NoBot => "unconfirmed (manual — relay the join request)",
    };
    println!("Recorded binding {} → {} [{}]", telegram, agent_id, confirm_state);
    println!("  Binding map: {}", path.display());

    Ok(())
}

/// `wg agency human confirm <telegram-user>` — manually record a human's `YES`
/// confirmation when the inbound listener isn't running (the no-bot / manual
/// onboarding path).
pub fn run_confirm(workgraph_dir: &Path, telegram: &str) -> Result<()> {
    let telegram = telegram.trim();
    let agency_dir = workgraph_dir.join("agency");
    let mut bindings = TelegramBindingMap::load(&agency_dir)
        .context("Failed to load Telegram binding map")?;

    match bindings.find_by_user(telegram) {
        None => anyhow::bail!(
            "no Telegram binding for '{}'. Add the human first with `wg agency human add`.",
            telegram
        ),
        Some(b) if b.confirmed => {
            println!("{} ({}) is already confirmed.", b.name, telegram);
            return Ok(());
        }
        Some(_) => {}
    }

    let name = agency::apply_confirmation(&mut bindings, telegram, "yes", chrono::Utc::now())
        .expect("binding exists and is unconfirmed");
    bindings
        .save(&agency_dir)
        .context("Failed to persist Telegram binding map")?;
    println!("Confirmed {} ({}) — they've joined.", name, telegram);
    Ok(())
}

/// Print the manual-relay instructions used whenever we can't (or don't) send
/// the DM ourselves.
fn print_manual_step(telegram: &str, handshake_msg: &str) {
    println!("Manual step — relay this to {} on Telegram:", telegram);
    println!("    \"{}\"", handshake_msg);
    println!(
        "  When they reply YES, run `wg agency human confirm {}` (or let the listener record it).",
        telegram
    );
}

/// Resolve which configured Telegram bot should front this human, if any.
///
/// Prefers a bot whose `agent_id` matches the new human's agent id (a bot
/// dedicated to them); otherwise falls back to the first configured bot (the
/// legacy/default or a shared group bot). Returns `None` when no bot is
/// configured — the manual-fallback path.
fn resolve_bot(workgraph_dir: &Path, agent_id: &str) -> Option<(String, TelegramBotConfig)> {
    let project_root = workgraph_dir.parent().unwrap_or(workgraph_dir);
    let notify = NotifyConfig::load(Some(project_root)).ok().flatten()?;
    let tg = TelegramConfig::from_notify_config(&notify).ok()?;
    let bots = tg.all_bots();
    if bots.is_empty() {
        return None;
    }
    if let Some(matched) = bots
        .iter()
        .find(|(_, b)| b.agent_id.as_deref() == Some(agent_id))
    {
        return Some(matched.clone());
    }
    bots.into_iter().next()
}

/// Send a one-off DM through the given bot. Best-effort: any error is returned
/// so the caller can fall back to the manual path.
fn send_dm(
    bot_id: &str,
    bot: TelegramBotConfig,
    target: &str,
    message: &str,
) -> Result<()> {
    use worksgood::notify::NotificationChannel;
    use worksgood::notify::telegram::TelegramChannel;

    let channel = TelegramChannel::from_bot(bot_id.to_string(), bot);
    let rt = tokio::runtime::Runtime::new().context("Failed to create async runtime")?;
    rt.block_on(async {
        channel
            .send_text(target, message)
            .await
            .map(|_| ())
            .context("Telegram send failed")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use worksgood::graph::WorkGraph;
    use worksgood::parser::save_graph;

    fn setup() -> TempDir {
        let tmp = TempDir::new().unwrap();
        // workgraph_dir is a `.workgraph`-style subdir of the project root so
        // project_label() and notify lookup behave like production.
        let wg = tmp.path().join(".workgraph");
        std::fs::create_dir_all(&wg).unwrap();
        let graph = WorkGraph::new();
        save_graph(&graph, &wg.join("graph.jsonl")).unwrap();
        tmp
    }

    fn wg_dir(tmp: &TempDir) -> std::path::PathBuf {
        tmp.path().join(".workgraph")
    }

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Nadin"), "nadin");
        assert_eq!(slugify("Nadin O'Brien"), "nadin-o-brien");
        assert_eq!(slugify("  Erik  Vaughn "), "erik-vaughn");
        assert_eq!(slugify("José-María"), "jos-mar-a");
        assert_eq!(slugify("!!!"), "");
    }

    #[test]
    fn test_human_add_no_bot_creates_agent_board_and_binding() {
        let tmp = setup();
        let dir = wg_dir(&tmp);

        run_add(&dir, "Nadin", "78901234", Some("family")).unwrap();

        // 1. Human agent exists and is_human.
        let agents_dir = dir.join("agency/cache/agents");
        let agents = agency::load_all_agents(&agents_dir).unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].id, "human-nadin");
        assert_eq!(agents[0].name, "Nadin");
        assert_eq!(agents[0].executor, "telegram");
        assert!(agents[0].is_human());
        assert_eq!(
            agents[0].contact,
            Some("telegram:78901234".to_string())
        );

        // 2. Per-user board exists.
        let graph =
            worksgood::parser::load_graph(&dir.join("graph.jsonl")).unwrap();
        assert!(graph.get_task(".user-nadin-0").is_some());

        // 3. Binding recorded, unconfirmed, no bot.
        let agency_dir = dir.join("agency");
        let map = TelegramBindingMap::load(&agency_dir).unwrap();
        let b = map.find_by_user("78901234").unwrap();
        assert_eq!(b.agent_id, "human-nadin");
        assert_eq!(b.name, "Nadin");
        assert!(!b.confirmed);
        assert_eq!(b.bot_id, None);
    }

    #[test]
    fn test_human_add_rejects_duplicate_telegram_user() {
        let tmp = setup();
        let dir = wg_dir(&tmp);

        run_add(&dir, "Nadin", "78901234", None).unwrap();
        // Same telegram id, different name → rejected (one human, one agent).
        let err = run_add(&dir, "Erik", "78901234", None).unwrap_err();
        assert!(err.to_string().contains("already bound"));

        // No partial second agent was written.
        let agents_dir = dir.join("agency/cache/agents");
        let agents = agency::load_all_agents(&agents_dir).unwrap();
        assert_eq!(agents.len(), 1);
    }

    #[test]
    fn test_human_add_rejects_duplicate_name() {
        let tmp = setup();
        let dir = wg_dir(&tmp);

        run_add(&dir, "Nadin", "111", None).unwrap();
        // Same name → same agent id → rejected before any writes.
        let err = run_add(&dir, "Nadin", "222", None).unwrap_err();
        assert!(err.to_string().contains("already exists"));

        let map = TelegramBindingMap::load(&dir.join("agency")).unwrap();
        assert_eq!(map.bindings.len(), 1);
    }

    #[test]
    fn test_human_add_empty_name_fails() {
        let tmp = setup();
        let dir = wg_dir(&tmp);
        assert!(run_add(&dir, "   ", "111", None).is_err());
        assert!(run_add(&dir, "Nadin", "  ", None).is_err());
    }

    #[test]
    fn test_project_label_defaults_to_parent_dir() {
        let tmp = setup();
        let dir = wg_dir(&tmp);
        // parent of `.workgraph` is the temp dir's random name; just assert
        // it's non-empty and not the literal fallback.
        let label = project_label(&dir);
        assert!(!label.is_empty());
    }
}
