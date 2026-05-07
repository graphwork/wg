//! `wg html publish` — manage rsync-based html deployments.
//!
//! Stores deployments in `<workgraph_dir>/html-publish.toml`. Each deployment
//! captures the rsync target, optional html flags, optional cron schedule, and
//! last-run state. `wg html publish run <name>` runs `wg html` then rsyncs the
//! output to the target. `wg html publish add --schedule <expr>` registers a
//! cron-enabled wg task that calls `wg html publish run <name>` on the
//! schedule.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

const PUBLISH_FILE: &str = "html-publish.toml";
const PUBLISH_TASK_PREFIX: &str = ".html-publish-";

/// Top-level on-disk schema.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct PublishConfig {
    #[serde(default)]
    pub deployments: Vec<Deployment>,
}

/// One rsync deployment (named) with optional schedule + html flags.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Deployment {
    pub name: String,
    /// rsync target, e.g. `user@host:/var/www/wg/`
    pub rsync_target: String,
    /// Cron schedule (5- or 6-field). `None` = manual-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    /// `--since` flag passed to `wg html`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    /// If true, pass `--public-only` to `wg html`.
    #[serde(default)]
    pub public_only: bool,
    /// Whether `wg html` includes chat transcripts. Default: false.
    #[serde(default)]
    pub include_chat: bool,
    /// Output staging dir. `None` = derive from name in temp dir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_dir: Option<String>,
    /// Optional SSH key path (sets `-e 'ssh -i <key>'` on rsync).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_key: Option<String>,
    /// Optional ~/.ssh/config Host alias (passed in target if not already
    /// host-form). Stored alongside `ssh_key` for transparency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_config_host: Option<String>,
    /// Extra rsync flags (replaces defaults if present).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rsync_flags: Option<Vec<String>>,
    /// ISO-8601 timestamp of most recent run attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<String>,
    /// "ok" or "fail" for the most recent run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
    /// Last error message (if last run failed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// ID of the wg task created to drive scheduling (when `schedule` is set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_task_id: Option<String>,

    /// Per-deployment override for the rendered page title. Wins over
    /// `[project].title` / `[project].name` in `<workgraph_dir>/config.toml`
    /// and avoids the default `hostname:/repo/path` source label in public
    /// exports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Per-deployment override for the rendered page byline. Wins over
    /// `[project].byline` in `<workgraph_dir>/config.toml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byline: Option<String>,

    /// Path to a markdown file rendered as the page abstract. Resolved
    /// relative to the workgraph dir (typical value: `about.md`). When
    /// unset, the renderer falls back to `<workgraph_dir>/about.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abstract_path: Option<String>,
}

impl Deployment {
    pub fn new(name: String, rsync_target: String) -> Self {
        Self {
            name,
            rsync_target,
            schedule: None,
            since: None,
            public_only: false,
            include_chat: false,
            out_dir: None,
            ssh_key: None,
            ssh_config_host: None,
            rsync_flags: None,
            last_run_at: None,
            last_status: None,
            last_error: None,
            schedule_task_id: None,
            title: None,
            byline: None,
            abstract_path: None,
        }
    }
}

/// Path to the publish-config file inside a workgraph dir.
pub fn publish_config_path(workgraph_dir: &Path) -> PathBuf {
    workgraph_dir.join(PUBLISH_FILE)
}

/// Default staging dir for a deployment when `--out` was not supplied.
pub fn default_out_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("wg-html-publish-{}", name))
}

/// Default rsync flags applied when the deployment has no explicit override.
///
/// Kept conservative for compatibility with older rsync versions. Users on
/// rsync >= 3.2.3 who want the destination path auto-created on first run
/// can pass `--mkpath` at `add` time, which appends `--mkpath` to the
/// default. For full control, pass `--rsync-flags '<custom>'`.
pub fn default_rsync_flags() -> Vec<String> {
    vec!["-avz".to_string(), "--delete".to_string()]
}

/// Whitespace-split an rsync_flags string into argv tokens. Empty input → empty vec.
pub fn parse_rsync_flags_str(s: &str) -> Vec<String> {
    s.split_whitespace().map(|t| t.to_string()).collect()
}

/// Cron-task ID for a deployment.
pub fn cron_task_id(name: &str) -> String {
    format!("{}{}", PUBLISH_TASK_PREFIX, name)
}

/// Load (or initialize) the publish config for a workgraph dir.
pub fn load_config(workgraph_dir: &Path) -> Result<PublishConfig> {
    let path = publish_config_path(workgraph_dir);
    if !path.exists() {
        return Ok(PublishConfig::default());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let cfg: PublishConfig = toml::from_str(&raw)
        .with_context(|| format!("failed to parse {} as TOML", path.display()))?;
    Ok(cfg)
}

/// Persist the publish config for a workgraph dir.
pub fn save_config(workgraph_dir: &Path, cfg: &PublishConfig) -> Result<()> {
    let path = publish_config_path(workgraph_dir);
    let raw = toml::to_string_pretty(cfg).context("failed to serialize publish config")?;
    std::fs::write(&path, raw).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Find a deployment by name, returning a clone for inspection.
pub fn find_deployment(cfg: &PublishConfig, name: &str) -> Option<Deployment> {
    cfg.deployments.iter().find(|d| d.name == name).cloned()
}

#[allow(clippy::too_many_arguments)]
pub fn run_add(
    workgraph_dir: &Path,
    name: &str,
    rsync_target: &str,
    schedule: Option<&str>,
    since: Option<&str>,
    public_only: bool,
    include_chat: bool,
    out_dir: Option<&str>,
    ssh_key: Option<&str>,
    ssh_config_host: Option<&str>,
    rsync_flags: Option<&str>,
    mkpath: bool,
    title: Option<&str>,
    byline: Option<&str>,
    abstract_path: Option<&str>,
) -> Result<()> {
    if name.is_empty() {
        bail!("Deployment name must not be empty.");
    }
    if rsync_target.is_empty() {
        bail!("--rsync target must not be empty.");
    }
    if let Some(expr) = schedule {
        // Validate the cron expression up front.
        workgraph::cron::parse_cron_expression(expr)
            .with_context(|| format!("invalid cron expression: {}", expr))?;
    }

    // --mkpath is a single-flag opt-in that appends to the default flag set;
    // --rsync-flags is a full override that replaces the default. Combining
    // them is ambiguous (does --mkpath append to the override? to the
    // default?), so refuse — the user can spell out --mkpath inside their
    // --rsync-flags string if they want both.
    let has_explicit_flags = matches!(rsync_flags, Some(s) if !s.trim().is_empty());
    if mkpath && has_explicit_flags {
        bail!(
            "--mkpath and --rsync-flags are mutually exclusive. \
             --mkpath appends one flag to the default ('-avz --delete'); \
             --rsync-flags replaces the default entirely. Pick one."
        );
    }

    // Resolve the on-disk rsync_flags field:
    //   * --rsync-flags '...'  → store the parsed list (full override)
    //   * --mkpath             → store default + ['--mkpath']
    //   * neither              → leave as None (runtime falls back to default)
    let resolved_flags: Option<Vec<String>> = if has_explicit_flags {
        Some(parse_rsync_flags_str(rsync_flags.unwrap()))
    } else if mkpath {
        let mut v = default_rsync_flags();
        v.push("--mkpath".to_string());
        Some(v)
    } else {
        None
    };

    let mut cfg = load_config(workgraph_dir)?;
    if cfg.deployments.iter().any(|d| d.name == name) {
        bail!(
            "Deployment '{}' already exists. Remove it first or use a different name.",
            name
        );
    }

    let mut dep = Deployment::new(name.to_string(), rsync_target.to_string());
    dep.schedule = schedule.map(|s| s.to_string());
    dep.since = since.map(|s| s.to_string());
    dep.public_only = public_only;
    dep.include_chat = include_chat;
    dep.out_dir = out_dir.map(|s| s.to_string());
    dep.ssh_key = ssh_key.map(|s| s.to_string());
    dep.ssh_config_host = ssh_config_host.map(|s| s.to_string());
    dep.rsync_flags = resolved_flags;
    dep.title = title.map(|s| s.to_string());
    dep.byline = byline.map(|s| s.to_string());
    dep.abstract_path = abstract_path.map(|s| s.to_string());

    if let Some(expr) = schedule {
        let task_id = ensure_cron_task(workgraph_dir, name, expr)?;
        dep.schedule_task_id = Some(task_id);
    }

    cfg.deployments.push(dep);
    save_config(workgraph_dir, &cfg)?;

    println!("Added publish deployment '{}'", name);
    println!("  rsync target: {}", rsync_target);
    if let Some(expr) = schedule {
        println!("  schedule:     {}", expr);
        println!("  schedule task: {}", cron_task_id(name));
    } else {
        println!(
            "  schedule:     (manual-only — run with `wg html publish run {}`)",
            name
        );
    }
    Ok(())
}

pub fn run_list(workgraph_dir: &Path, json: bool) -> Result<()> {
    let cfg = load_config(workgraph_dir)?;
    if json {
        let items: Vec<serde_json::Value> = cfg
            .deployments
            .iter()
            .map(|d| {
                serde_json::json!({
                    "name": d.name,
                    "rsync_target": d.rsync_target,
                    "schedule": d.schedule,
                    "since": d.since,
                    "public_only": d.public_only,
                    "include_chat": d.include_chat,
                    "out_dir": d.out_dir,
                    "ssh_key": d.ssh_key,
                    "ssh_config_host": d.ssh_config_host,
                    "last_run_at": d.last_run_at,
                    "last_status": d.last_status,
                    "schedule_task_id": d.schedule_task_id,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }
    if cfg.deployments.is_empty() {
        println!("No publish deployments configured.");
        println!("  Add one with: wg html publish add <name> --rsync <user@host:/path/>");
        return Ok(());
    }
    println!("Publish deployments:");
    println!();
    for d in &cfg.deployments {
        let sched = d.schedule.as_deref().unwrap_or("(manual)");
        let last = d
            .last_run_at
            .as_deref()
            .map(|t| format!("{} [{}]", t, d.last_status.as_deref().unwrap_or("?")))
            .unwrap_or_else(|| "(never)".to_string());
        println!(
            "  {}\n    rsync:    {}\n    schedule: {}\n    last:     {}",
            d.name, d.rsync_target, sched, last
        );
        println!();
    }
    Ok(())
}

pub fn run_show(workgraph_dir: &Path, name: &str, json: bool) -> Result<()> {
    let cfg = load_config(workgraph_dir)?;
    let d = find_deployment(&cfg, name)
        .ok_or_else(|| anyhow::anyhow!("Deployment '{}' not found.", name))?;

    if json {
        let v = serde_json::json!({
            "name": d.name,
            "rsync_target": d.rsync_target,
            "schedule": d.schedule,
            "since": d.since,
            "public_only": d.public_only,
            "include_chat": d.include_chat,
            "out_dir": d.out_dir.clone()
                .unwrap_or_else(|| default_out_dir(&d.name).display().to_string()),
            "ssh_key": d.ssh_key,
            "ssh_config_host": d.ssh_config_host,
            "rsync_flags": d.rsync_flags.clone().unwrap_or_else(default_rsync_flags),
            "last_run_at": d.last_run_at,
            "last_status": d.last_status,
            "last_error": d.last_error,
            "schedule_task_id": d.schedule_task_id,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    println!("Deployment: {}", d.name);
    println!("  rsync target:    {}", d.rsync_target);
    println!(
        "  schedule:        {}",
        d.schedule.as_deref().unwrap_or("(manual)")
    );
    println!(
        "  since:           {}",
        d.since.as_deref().unwrap_or("(none)")
    );
    println!("  public_only:     {}", d.public_only);
    println!("  include_chat:    {}", d.include_chat);
    println!(
        "  out dir:         {}",
        d.out_dir
            .clone()
            .unwrap_or_else(|| default_out_dir(&d.name).display().to_string())
    );
    if let Some(k) = &d.ssh_key {
        println!("  ssh key:         {}", k);
    }
    if let Some(h) = &d.ssh_config_host {
        println!("  ssh config host: {}", h);
    }
    println!(
        "  rsync flags:     {}",
        d.rsync_flags
            .clone()
            .unwrap_or_else(default_rsync_flags)
            .join(" ")
    );
    println!(
        "  last run:        {}",
        d.last_run_at.as_deref().unwrap_or("(never)")
    );
    if let Some(s) = &d.last_status {
        println!("  last status:     {}", s);
    }
    if let Some(e) = &d.last_error {
        println!("  last error:      {}", e);
    }
    if let Some(t) = &d.schedule_task_id {
        println!("  schedule task:   {}", t);
    }
    Ok(())
}

pub fn run_remove(workgraph_dir: &Path, name: &str) -> Result<()> {
    let mut cfg = load_config(workgraph_dir)?;
    let pos = cfg
        .deployments
        .iter()
        .position(|d| d.name == name)
        .ok_or_else(|| anyhow::anyhow!("Deployment '{}' not found.", name))?;
    let removed = cfg.deployments.remove(pos);
    save_config(workgraph_dir, &cfg)?;

    // Best-effort: abandon the scheduling task if one exists.
    if let Some(task_id) = &removed.schedule_task_id {
        if let Err(e) = crate::commands::abandon::run(
            workgraph_dir,
            task_id,
            Some(&format!("Removed publish deployment '{}'.", name)),
            &[],
        ) {
            eprintln!(
                "Warning: could not abandon scheduling task '{}': {}",
                task_id, e
            );
        }
    }

    println!("Removed publish deployment '{}'.", name);
    Ok(())
}

pub fn run_edit(workgraph_dir: &Path) -> Result<()> {
    let path = publish_config_path(workgraph_dir);
    if !path.exists() {
        // Touch so the editor opens on something writable.
        std::fs::write(&path, "")
            .with_context(|| format!("failed to create {}", path.display()))?;
    }
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = Command::new(&editor)
        .arg(&path)
        .status()
        .with_context(|| format!("failed to launch editor {}", editor))?;
    if !status.success() {
        bail!("Editor exited with non-zero status: {}", status);
    }
    // Validate after edit.
    let _ = load_config(workgraph_dir)?;
    println!("Saved {}", path.display());
    Ok(())
}

/// Run a deployment immediately: build html, rsync, log result.
pub fn run_run(workgraph_dir: &Path, name: &str, dry_run: bool) -> Result<()> {
    let mut cfg = load_config(workgraph_dir)?;
    let dep_idx = cfg
        .deployments
        .iter()
        .position(|d| d.name == name)
        .ok_or_else(|| anyhow::anyhow!("Deployment '{}' not found.", name))?;

    let dep = cfg.deployments[dep_idx].clone();
    let started_at = chrono::Utc::now().to_rfc3339();
    let result = execute_run(workgraph_dir, &dep, dry_run);

    // Log-line to ~/.wg/publish.log (best-effort).
    let log_line = match &result {
        Ok(()) => format!("[{}] {} OK target={}", started_at, name, dep.rsync_target),
        Err(e) => format!(
            "[{}] {} FAIL target={} err={}",
            started_at, name, dep.rsync_target, e
        ),
    };
    append_global_log(&log_line);

    // Persist last-run state.
    cfg.deployments[dep_idx].last_run_at = Some(started_at);
    match &result {
        Ok(()) => {
            cfg.deployments[dep_idx].last_status = Some("ok".to_string());
            cfg.deployments[dep_idx].last_error = None;
        }
        Err(e) => {
            cfg.deployments[dep_idx].last_status = Some("fail".to_string());
            cfg.deployments[dep_idx].last_error = Some(e.to_string());
        }
    }
    save_config(workgraph_dir, &cfg)?;
    result
}

fn execute_run(workgraph_dir: &Path, dep: &Deployment, dry_run: bool) -> Result<()> {
    let out_dir: PathBuf = match &dep.out_dir {
        Some(s) => PathBuf::from(s),
        None => default_out_dir(&dep.name),
    };
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("failed to create staging dir {}", out_dir.display()))?;

    println!("[publish] building html → {}", out_dir.display());
    if !dry_run {
        // Build the html. Use the library API directly so this works
        // independently of the wg CLI binary on $PATH.
        // Mirrors `wg html` semantics:
        //   show_all = !public_only
        //   include_chat from --chat (deployment field)
        //   all_chats = include_chat && !public_only (mirror cli logic)
        let show_all = !dep.public_only;
        let include_chat = dep.include_chat;
        let all_chats = include_chat && !dep.public_only;

        // Resolve the project-metadata cascade for this deployment.
        // Per-deployment fields win over the project-level cascade
        // (config.toml [project] + about.md).
        let project_meta = resolve_deployment_meta(workgraph_dir, dep);

        let graph_path = workgraph_dir.join("graph.jsonl");
        if !graph_path.exists() {
            anyhow::bail!(
                "workgraph not initialized at {}. Run `wg init` first.",
                workgraph_dir.display()
            );
        }
        let graph = workgraph::parser::load_graph(&graph_path)
            .with_context(|| "failed to load graph for html publish")?;
        workgraph::html::render_site(
            &graph,
            workgraph_dir,
            &out_dir,
            workgraph::html::RenderOptions {
                show_all,
                since: dep.since.clone(),
                include_chat,
                all_chats,
                project_meta: Some(project_meta),
                source_title: None,
            },
        )
        .with_context(|| "wg html generation failed")?;
    }

    let mut cmd = Command::new("rsync");
    let flags = dep.rsync_flags.clone().unwrap_or_else(default_rsync_flags);
    cmd.args(&flags);

    if let Some(key) = &dep.ssh_key {
        cmd.arg("-e");
        cmd.arg(format!(
            "ssh -i {} -o StrictHostKeyChecking=accept-new",
            key
        ));
    }

    // rsync source must end with trailing slash to mirror contents.
    let mut src = out_dir.display().to_string();
    if !src.ends_with('/') {
        src.push('/');
    }
    cmd.arg(&src);
    cmd.arg(&dep.rsync_target);

    if dry_run {
        cmd.arg("--dry-run");
    }

    println!("[publish] rsync {:?} → {}", flags, dep.rsync_target);
    let status = cmd
        .status()
        .with_context(|| "failed to spawn rsync (is it installed?)")?;
    if !status.success() {
        bail!("rsync exited with non-zero status: {}", status);
    }
    println!("[publish] OK");
    Ok(())
}

/// Resolve the project-metadata cascade for a deployment:
///   1. per-deployment override (`title` / `byline` / `abstract_path`)
///   2. project-level (`<workgraph_dir>/config.toml [project]` →
///      `<workgraph_dir>/about.md`)
///   3. when byline/abstract exist without a title, default the project
///      header title to the workgraph dir name
///
/// Reads no abstract from disk if `abstract_path` is empty AND the
/// project-level cascade also has nothing — the default abstract is empty.
pub fn resolve_deployment_meta(
    workgraph_dir: &Path,
    dep: &Deployment,
) -> workgraph::html::ProjectMeta {
    // Project-level cascade (config + about.md). Provides the base layer.
    let mut meta = workgraph::html::resolve_project_meta(workgraph_dir);

    // Per-deployment overrides win over the project-level cascade.
    if let Some(t) = dep.title.as_ref().filter(|s| !s.trim().is_empty()) {
        meta.title = Some(t.clone());
    }
    if let Some(b) = dep.byline.as_ref().filter(|s| !s.trim().is_empty()) {
        meta.byline = Some(b.clone());
    }
    if let Some(p) = dep.abstract_path.as_ref().filter(|s| !s.trim().is_empty()) {
        let path = if Path::new(p).is_absolute() {
            std::path::PathBuf::from(p)
        } else {
            workgraph_dir.join(p)
        };
        if let Ok(body) = std::fs::read_to_string(&path) {
            let trimmed = body.trim();
            if !trimmed.is_empty() {
                meta.abstract_md = Some(body);
            }
        }
    }

    // Default title = directory name when neither override nor project
    // config supplied one, but only when metadata exists. Empty deployment
    // + empty project is handled in the renderer via the minimal source-title
    // header, and the project-header is omitted entirely.
    if meta.title.is_none() {
        if let Some(name) = workgraph_dir
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
        {
            // Only use the directory-name default when something else is
            // set (byline or abstract). Otherwise the "is_empty" check
            // collapses the header back to the minimal source-title form.
            if meta
                .byline
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false)
                || meta
                    .abstract_md
                    .as_deref()
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false)
            {
                meta.title = Some(name);
            }
        }
    }

    meta
}

fn append_global_log(line: &str) {
    if let Some(home) = dirs::home_dir() {
        let dir = home.join(".wg");
        let _ = std::fs::create_dir_all(&dir);
        let log = dir.join("html-publish.log");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
        {
            use std::io::Write;
            let _ = writeln!(f, "{}", line);
        }
    }
}

/// Create (or reuse) the cron-driven wg task that runs the deployment.
fn ensure_cron_task(workgraph_dir: &Path, name: &str, schedule: &str) -> Result<String> {
    let task_id = cron_task_id(name);
    let title = format!(".html-publish: {}", name);
    let description = format!(
        "## Description\n\nScheduled rsync deployment of `wg html` output.\n\nManaged by `wg html publish`. \
Edit via `wg html publish edit` or remove via `wg html publish remove {}`.\n",
        name
    );
    let exec = format!("wg html publish run {}", name);

    // Hand off to the existing add command, configured for cron + shell-exec.
    // exec_mode = "shell" runs the body via a subprocess, no LLM.
    crate::commands::add::run(
        workgraph_dir,
        &title,
        Some(&task_id),
        Some(&description),
        &[],            // after
        None,           // assign
        None,           // hours
        None,           // cost
        &[],            // tags
        &[],            // skills
        &[],            // inputs
        &[],            // deliverables
        None,           // max_retries
        None,           // model
        None,           // provider
        None,           // verify
        None,           // verify_timeout
        None,           // validation
        None,           // validator_agent
        None,           // validator_model
        None,           // max_iterations
        None,           // cycle_guard
        None,           // cycle_delay
        false,          // no_converge
        false,          // no_restart_on_failure
        None,           // max_failure_restarts
        "internal",     // visibility
        None,           // context_scope
        Some(&exec),    // exec
        None,           // timeout
        Some("shell"),  // exec_mode
        false,          // paused
        true,           // no_place — system task, skip placement
        &[],            // place_near
        &[],            // place_before
        None,           // delay
        None,           // not_before
        false,          // allow_phantom
        true,           // independent — no implicit --after on creator
        false,          // no_tier_escalation
        None,           // iteration_config
        None,           // priority
        Some(schedule), // cron
        false,          // subtask
    )
    .with_context(|| "failed to register cron task for publish deployment")?;

    Ok(task_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh_dir() -> TempDir {
        let tmp = TempDir::new().unwrap();
        // Initialize a minimal graph so `add::run` succeeds.
        let graph_path = tmp.path().join("graph.jsonl");
        let graph = workgraph::graph::WorkGraph::new();
        workgraph::parser::save_graph(&graph, &graph_path).unwrap();
        // Disable agency scaffolding so add does not create extra blockers.
        std::fs::write(
            tmp.path().join("config.toml"),
            b"[agency]\nauto_assign = false\nauto_evaluate = false\n",
        )
        .unwrap();
        tmp
    }

    /// 12-arg run_add wrapper that defaults the new flags to "neither set"
    /// — so the existing test bodies stay focused on what they actually
    /// exercise (cron, persistence, list, etc.) rather than re-spelling
    /// the rsync_flags+mkpath args every time.
    #[allow(clippy::too_many_arguments)]
    fn run_add_default(
        workgraph_dir: &Path,
        name: &str,
        rsync_target: &str,
        schedule: Option<&str>,
        since: Option<&str>,
        public_only: bool,
        include_chat: bool,
    ) -> Result<()> {
        run_add(
            workgraph_dir,
            name,
            rsync_target,
            schedule,
            since,
            public_only,
            include_chat,
            None,  // out_dir
            None,  // ssh_key
            None,  // ssh_config_host
            None,  // rsync_flags
            false, // mkpath
            None,  // title
            None,  // byline
            None,  // abstract_path
        )
    }

    #[test]
    fn publish_add_persists_deployment() {
        let tmp = fresh_dir();
        run_add_default(
            tmp.path(),
            "blog",
            "user@example.com:/var/www/blog/",
            None,
            None,
            false,
            false,
        )
        .unwrap();

        let cfg = load_config(tmp.path()).unwrap();
        assert_eq!(cfg.deployments.len(), 1);
        let d = &cfg.deployments[0];
        assert_eq!(d.name, "blog");
        assert_eq!(d.rsync_target, "user@example.com:/var/www/blog/");
        assert!(d.schedule.is_none());
        assert!(d.schedule_task_id.is_none());
        // No --mkpath, no --rsync-flags → on-disk rsync_flags stays None
        // and the runtime default ('-avz --delete', no --mkpath) applies.
        assert!(
            d.rsync_flags.is_none(),
            "no opt-in must leave rsync_flags=None so the existing default is preserved"
        );
        // Compatibility default: NO --mkpath. Older rsync still works.
        assert_eq!(
            default_rsync_flags(),
            vec!["-avz".to_string(), "--delete".to_string()]
        );
    }

    #[test]
    fn publish_add_duplicate_errors() {
        let tmp = fresh_dir();
        run_add_default(
            tmp.path(),
            "blog",
            "user@example.com:/var/www/blog/",
            None,
            None,
            false,
            false,
        )
        .unwrap();
        let err = run_add_default(
            tmp.path(),
            "blog",
            "user@example.com:/var/www/blog/",
            None,
            None,
            false,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn publish_add_invalid_cron_errors() {
        let tmp = fresh_dir();
        let err = run_add_default(
            tmp.path(),
            "blog",
            "user@example.com:/var/www/blog/",
            Some("not-a-cron-expr"),
            None,
            false,
            false,
        )
        .unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("cron"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn publish_add_with_schedule_creates_cron_task() {
        let tmp = fresh_dir();
        run_add_default(
            tmp.path(),
            "site",
            "user@example.com:/srv/site/",
            Some("*/15 * * * *"),
            None,
            false,
            false,
        )
        .unwrap();
        let cfg = load_config(tmp.path()).unwrap();
        assert_eq!(cfg.deployments.len(), 1);
        let d = &cfg.deployments[0];
        assert_eq!(d.schedule.as_deref(), Some("*/15 * * * *"));
        assert_eq!(d.schedule_task_id.as_deref(), Some(".html-publish-site"));

        let graph = workgraph::parser::load_graph(&tmp.path().join("graph.jsonl")).unwrap();
        assert!(
            graph.tasks().any(|t| t.id == ".html-publish-site"),
            "expected scheduled task .html-publish-site to exist"
        );
    }

    #[test]
    fn publish_remove_drops_deployment_and_task() {
        let tmp = fresh_dir();
        run_add_default(
            tmp.path(),
            "site",
            "user@example.com:/srv/site/",
            Some("*/30 * * * *"),
            None,
            false,
            false,
        )
        .unwrap();
        run_remove(tmp.path(), "site").unwrap();
        let cfg = load_config(tmp.path()).unwrap();
        assert!(cfg.deployments.is_empty());

        let graph = workgraph::parser::load_graph(&tmp.path().join("graph.jsonl")).unwrap();
        let task = graph
            .tasks()
            .find(|t| t.id == ".html-publish-site")
            .expect("scheduling task should still exist as abandoned");
        assert!(
            matches!(task.status, workgraph::graph::Status::Abandoned),
            "expected abandoned, got {:?}",
            task.status
        );
    }

    #[test]
    fn publish_show_missing_errors() {
        let tmp = fresh_dir();
        let err = run_show(tmp.path(), "missing", false).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn publish_list_empty() {
        let tmp = fresh_dir();
        run_list(tmp.path(), false).unwrap();
        run_list(tmp.path(), true).unwrap();
    }

    #[test]
    fn publish_list_with_data() {
        let tmp = fresh_dir();
        run_add_default(tmp.path(), "a", "u@h:/a/", None, None, false, false).unwrap();
        run_add_default(tmp.path(), "b", "u@h:/b/", None, Some("7d"), true, false).unwrap();
        run_list(tmp.path(), false).unwrap();
        run_list(tmp.path(), true).unwrap();
    }

    #[test]
    fn publish_run_against_local_path_target_succeeds() {
        let tmp = fresh_dir();
        let dest = TempDir::new().unwrap();
        let dest_path = dest.path().to_path_buf();

        let target = format!("{}/", dest_path.display());
        run_add_default(tmp.path(), "local", &target, None, None, false, false).unwrap();

        run_run(tmp.path(), "local", false).expect("publish run should succeed");

        let cfg = load_config(tmp.path()).unwrap();
        let d = cfg.deployments.iter().find(|d| d.name == "local").unwrap();
        assert_eq!(d.last_status.as_deref(), Some("ok"));
        assert!(d.last_run_at.is_some());
        assert!(
            dest_path.join("index.html").exists(),
            "rsync should have produced index.html at {}",
            dest_path.display()
        );
    }

    #[test]
    fn publish_run_records_failure_on_bad_rsync_target() {
        let tmp = fresh_dir();
        run_add_default(
            tmp.path(),
            "broken",
            "/nonexistent-root-only-path/wg-publish-test/",
            None,
            None,
            false,
            false,
        )
        .unwrap();

        let err = run_run(tmp.path(), "broken", false);
        let cfg = load_config(tmp.path()).unwrap();
        let d = cfg.deployments.iter().find(|d| d.name == "broken").unwrap();
        assert!(d.last_run_at.is_some());
        if err.is_err() {
            assert_eq!(d.last_status.as_deref(), Some("fail"));
            assert!(d.last_error.is_some());
        }
    }

    // ── --mkpath / --rsync-flags opt-ins ──────────────────────────────

    #[test]
    fn publish_add_no_optin_keeps_default_unchanged() {
        // Validation row: `add foo --rsync ...` (no flag) → resolved flags
        // = '-avz --delete' (current default). We assert via the runtime
        // resolution path that this is what gets passed to rsync.
        let tmp = fresh_dir();
        run_add_default(tmp.path(), "noop", "u@h:/p/", None, None, false, false).unwrap();
        let cfg = load_config(tmp.path()).unwrap();
        let d = &cfg.deployments[0];
        assert!(d.rsync_flags.is_none(), "no opt-in must persist as None");
        let resolved = d.rsync_flags.clone().unwrap_or_else(default_rsync_flags);
        assert_eq!(
            resolved,
            vec!["-avz".to_string(), "--delete".to_string()],
            "unchanged from pre-task behaviour: NO --mkpath"
        );
    }

    #[test]
    fn publish_add_mkpath_appends_to_default() {
        // Validation row: `add foo --rsync ... --mkpath` → rsync_flags
        // = '-avz --delete --mkpath'.
        let tmp = fresh_dir();
        run_add(
            tmp.path(),
            "withmk",
            "u@h:/p/",
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            true, // mkpath
            None, // title
            None, // byline
            None, // abstract_path
        )
        .unwrap();
        let cfg = load_config(tmp.path()).unwrap();
        let d = &cfg.deployments[0];
        assert_eq!(
            d.rsync_flags.as_deref(),
            Some(
                &[
                    "-avz".to_string(),
                    "--delete".to_string(),
                    "--mkpath".to_string()
                ][..]
            ),
            "--mkpath must append exactly one flag to the existing default"
        );
    }

    #[test]
    fn publish_add_rsync_flags_full_override() {
        // Validation row: `add foo --rsync ... --rsync-flags '-avzP'` →
        // rsync_flags = '-avzP' (default fully replaced).
        let tmp = fresh_dir();
        run_add(
            tmp.path(),
            "override",
            "u@h:/p/",
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            Some("-avzP"),
            false,
            None, // title
            None, // byline
            None, // abstract_path
        )
        .unwrap();
        let cfg = load_config(tmp.path()).unwrap();
        let d = &cfg.deployments[0];
        assert_eq!(
            d.rsync_flags.as_deref(),
            Some(&["-avzP".to_string()][..]),
            "--rsync-flags must completely replace the default — no -avz or --delete added"
        );
    }

    #[test]
    fn publish_add_rsync_flags_with_mkpath_fully_overrides() {
        // Variant of the override row: --rsync-flags can include --mkpath
        // explicitly when the user wants both. Default flags are NOT merged
        // in.
        let tmp = fresh_dir();
        run_add(
            tmp.path(),
            "both",
            "u@h:/p/",
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            Some("-avz --delete --mkpath -P"),
            false,
            None, // title
            None, // byline
            None, // abstract_path
        )
        .unwrap();
        let cfg = load_config(tmp.path()).unwrap();
        let d = &cfg.deployments[0];
        assert_eq!(
            d.rsync_flags.as_deref(),
            Some(
                &[
                    "-avz".to_string(),
                    "--delete".to_string(),
                    "--mkpath".to_string(),
                    "-P".to_string(),
                ][..]
            ),
            "explicit --rsync-flags must round-trip verbatim, even when it spells out --mkpath"
        );
    }

    #[test]
    fn publish_add_mkpath_and_rsync_flags_are_mutually_exclusive() {
        // Validation row: `--mkpath` and `--rsync-flags` together → error
        // at add time, no deployment written.
        let tmp = fresh_dir();
        let err = run_add(
            tmp.path(),
            "conflict",
            "u@h:/p/",
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            Some("-avzP"),
            true, // mkpath
            None, // title
            None, // byline
            None, // abstract_path
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("mutually exclusive"),
            "expected mutually-exclusive error, got: {}",
            msg
        );
        assert!(
            msg.contains("--mkpath") && msg.contains("--rsync-flags"),
            "error message must name both flags so the user knows which to drop, got: {}",
            msg
        );
        let cfg = load_config(tmp.path()).unwrap();
        assert!(
            cfg.deployments.is_empty(),
            "a rejected --mkpath+--rsync-flags add must NOT persist a deployment"
        );
    }

    #[test]
    fn publish_existing_deployment_without_rsync_flags_unchanged() {
        // Validation row: existing deployments (no rsync_flags field on
        // disk) MUST keep behaving as they did pre-task. Specifically, the
        // resolved flag set is the unchanged default ('-avz --delete') —
        // there is NO silent --mkpath upgrade.
        let tmp = fresh_dir();
        let dest = TempDir::new().unwrap();
        let target = format!("{}/", dest.path().display());
        let toml = format!(
            "[[deployments]]\nname = \"legacy\"\nrsync_target = \"{}\"\npublic_only = false\ninclude_chat = false\n",
            target
        );
        std::fs::write(publish_config_path(tmp.path()), toml).unwrap();
        let cfg = load_config(tmp.path()).unwrap();
        assert!(cfg.deployments[0].rsync_flags.is_none());
        let resolved = cfg.deployments[0]
            .rsync_flags
            .clone()
            .unwrap_or_else(default_rsync_flags);
        assert_eq!(
            resolved,
            vec!["-avz".to_string(), "--delete".to_string()],
            "legacy deployments must continue to use the unchanged default"
        );

        // And rerun — the destination already exists so even without
        // --mkpath the run succeeds. Validates the no-auto-rewrite contract
        // end-to-end.
        run_run(tmp.path(), "legacy", false).unwrap();
        let cfg_after = load_config(tmp.path()).unwrap();
        assert!(
            cfg_after.deployments[0].rsync_flags.is_none(),
            "run must NOT silently rewrite the on-disk rsync_flags field"
        );
    }

    #[test]
    fn publish_run_with_mkpath_creates_missing_remote_path() {
        // Live: --mkpath opt-in, fresh non-existent destination, run
        // succeeds and creates the path. Pre-fix this path produced rsync
        // exit 11 because the absent parent dir was not auto-created.
        let tmp = fresh_dir();
        let parent = TempDir::new().unwrap();
        let nested = parent.path().join("does/not/exist/yet");
        assert!(!nested.exists());
        let target = format!("{}/", nested.display());
        run_add(
            tmp.path(),
            "freshpath",
            &target,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            true, // mkpath
            None, // title
            None, // byline
            None, // abstract_path
        )
        .unwrap();
        run_run(tmp.path(), "freshpath", false)
            .expect("with --mkpath, fresh destination should be created");
        assert!(
            nested.join("index.html").exists(),
            "expected --mkpath to have created {} and rsync'd index.html into it",
            nested.display()
        );
    }

    #[test]
    fn parse_rsync_flags_str_basic() {
        assert_eq!(
            parse_rsync_flags_str("-avz --delete"),
            vec!["-avz".to_string(), "--delete".to_string()]
        );
        assert_eq!(parse_rsync_flags_str("   "), Vec::<String>::new());
        assert_eq!(parse_rsync_flags_str(""), Vec::<String>::new());
        assert_eq!(
            parse_rsync_flags_str("-a  --delete   --mkpath"),
            vec![
                "-a".to_string(),
                "--delete".to_string(),
                "--mkpath".to_string()
            ]
        );
    }

    // ── Project metadata (title / byline / abstract) ──────────────────

    #[test]
    fn publish_add_persists_title_byline_abstract() {
        let tmp = fresh_dir();
        run_add(
            tmp.path(),
            "branded",
            "u@h:/p/",
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            false,
            Some("My Project"),
            Some("a tagline"),
            Some("about.md"),
        )
        .unwrap();
        let cfg = load_config(tmp.path()).unwrap();
        let d = &cfg.deployments[0];
        assert_eq!(d.title.as_deref(), Some("My Project"));
        assert_eq!(d.byline.as_deref(), Some("a tagline"));
        assert_eq!(d.abstract_path.as_deref(), Some("about.md"));
    }

    #[test]
    fn publish_meta_cascade_per_deployment_overrides_project_config() {
        // Project-level config sets a title; per-deployment override wins.
        let tmp = fresh_dir();
        std::fs::write(
            tmp.path().join("config.toml"),
            b"[agency]\nauto_assign = false\nauto_evaluate = false\n\n\
              [project]\ntitle = \"Project Title\"\nbyline = \"project byline\"\n",
        )
        .unwrap();
        let mut dep = Deployment::new("d".to_string(), "u@h:/p/".to_string());
        dep.title = Some("Deployment Title".to_string());
        // No per-deployment byline → project-level value wins.
        let meta = resolve_deployment_meta(tmp.path(), &dep);
        assert_eq!(meta.title.as_deref(), Some("Deployment Title"));
        assert_eq!(meta.byline.as_deref(), Some("project byline"));
    }

    #[test]
    fn publish_meta_cascade_about_md_used_when_no_override() {
        let tmp = fresh_dir();
        std::fs::write(
            tmp.path().join("about.md"),
            b"# About\n\nA paragraph that becomes the abstract.\n",
        )
        .unwrap();
        let dep = Deployment::new("d".to_string(), "u@h:/p/".to_string());
        let meta = resolve_deployment_meta(tmp.path(), &dep);
        let body = meta
            .abstract_md
            .expect("abstract should fall back to about.md");
        assert!(body.contains("# About"));
        assert!(body.contains("A paragraph that becomes the abstract."));
    }

    #[test]
    fn publish_meta_cascade_abstract_path_overrides_about_md() {
        let tmp = fresh_dir();
        std::fs::write(tmp.path().join("about.md"), b"default about\n").unwrap();
        std::fs::write(
            tmp.path().join("custom-abstract.md"),
            b"deployment-specific abstract\n",
        )
        .unwrap();
        let mut dep = Deployment::new("d".to_string(), "u@h:/p/".to_string());
        dep.abstract_path = Some("custom-abstract.md".to_string());
        let meta = resolve_deployment_meta(tmp.path(), &dep);
        let body = meta.abstract_md.expect("abstract should be set");
        assert!(body.contains("deployment-specific"));
        assert!(!body.contains("default about"));
    }

    #[test]
    fn publish_meta_cascade_empty_when_nothing_configured() {
        let tmp = fresh_dir();
        let dep = Deployment::new("d".to_string(), "u@h:/p/".to_string());
        let meta = resolve_deployment_meta(tmp.path(), &dep);
        // When NOTHING is configured anywhere, ProjectMeta should be empty
        // — that's the signal to the renderer to omit the project-header
        // entirely (per spec: "If all three fields are empty: omit the
        // header entirely").
        assert!(meta.is_empty(), "expected empty meta, got: {:?}", meta);
    }

    #[test]
    fn publish_meta_cascade_legacy_name_field_used_as_title_fallback() {
        // The pre-task `[project].name` field had no formal semantics —
        // we treat it as a fallback for `title` so existing config files
        // with `name = "Foo"` automatically get a rendered header.
        let tmp = fresh_dir();
        std::fs::write(
            tmp.path().join("config.toml"),
            b"[agency]\nauto_assign = false\nauto_evaluate = false\n\n\
              [project]\nname = \"Legacy Name\"\n",
        )
        .unwrap();
        let dep = Deployment::new("d".to_string(), "u@h:/p/".to_string());
        let meta = resolve_deployment_meta(tmp.path(), &dep);
        assert_eq!(meta.title.as_deref(), Some("Legacy Name"));
    }

    #[test]
    fn publish_run_renders_project_header_in_html() {
        // Live: configure a deployment with title + byline + abstract;
        // run rsync against a local dir; verify the rendered index.html
        // contains all three.
        let tmp = fresh_dir();
        let dest = TempDir::new().unwrap();
        let target = format!("{}/", dest.path().display());
        run_add(
            tmp.path(),
            "branded",
            &target,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            false,
            Some("Poietic Inc"),
            Some("active work"),
            None,
        )
        .unwrap();

        // Drop an about.md — the renderer should pick it up and render
        // it as markdown.
        std::fs::write(
            tmp.path().join("about.md"),
            b"## Focus\n\n- compliance audit\n- haiku research\n",
        )
        .unwrap();

        run_run(tmp.path(), "branded", false).unwrap();

        let html = std::fs::read_to_string(dest.path().join("index.html"))
            .expect("index.html should have been rsynced into the dest dir");
        assert!(
            html.contains("class=\"project-header\""),
            "header CSS class missing"
        );
        assert!(html.contains("Poietic Inc"), "title missing from html");
        assert!(html.contains("active work"), "byline missing from html");
        // Markdown abstract should be rendered to HTML (h2 + ul + li).
        assert!(
            html.contains("<h2>Focus</h2>"),
            "h2 missing — abstract not rendered as markdown"
        );
        assert!(
            html.contains("<li>compliance audit</li>"),
            "list item missing"
        );
    }

    #[test]
    fn publish_run_omits_project_header_when_meta_empty() {
        // Live: a deployment with NO metadata + NO project config + NO
        // about.md MUST NOT render an empty <header class="project-header">
        // block. The minimal header/title still identify the source workgraph
        // with the default host:path label.
        let tmp = fresh_dir();
        let dest = TempDir::new().unwrap();
        let target = format!("{}/", dest.path().display());
        run_add_default(tmp.path(), "minimal", &target, None, None, false, false).unwrap();
        run_run(tmp.path(), "minimal", false).unwrap();

        let html = std::fs::read_to_string(dest.path().join("index.html"))
            .expect("index.html should have been rsynced");
        assert!(
            !html.contains("class=\"project-header\""),
            "project-header must be omitted when no metadata is configured (got the empty block)"
        );
        let source_title = workgraph::html::source_title_for_workgraph_dir(tmp.path());
        assert!(
            html.contains(&format!("<title>{source_title} — all tasks</title>")),
            "browser title should identify the source workgraph; got: {html}"
        );
        assert!(
            html.contains(&format!("<h1>{source_title}</h1>")),
            "minimal visible header should identify the source workgraph; got: {html}"
        );
        assert!(
            !html.contains("<title>workgraph"),
            "browser title must not fall back to generic workgraph"
        );
        // Sanity: the minimal page header is still present.
        assert!(html.contains("class=\"page-header\""));
    }
}
