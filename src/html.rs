//! `wg html`: render the workgraph as a static, clickable HTML viewer.
//!
//! Design goals (v2):
//! - **TUI parity**: render `wg viz --all` verbatim in a monospace `<pre>`,
//!   with the same color palette as the TUI (status colors from
//!   `flash_color_for_status`, edge highlights from
//!   `tui::viz_viewer::render`'s upstream/downstream selection logic).
//! - **Universal clickability**: every task id and status indicator in the
//!   viz is wrapped in a `<span class="task-link" data-task-id>` that opens
//!   a side-panel detail overlay matching what `wg show <task>` displays.
//! - **Edge highlighting**: clicking a task highlights its `--after` (upstream)
//!   edges in magenta and its consumers (downstream) in cyan, with everything
//!   else dimmed so the selection's relationships pop. This uses the
//!   `char_edge_map` produced by the viz layer (per-character edge attribution).
//! - **Theme support**: dark theme by default, with `prefers-color-scheme`
//!   auto-detection on first load and a manual toggle persisted via
//!   localStorage.
//! - **Static-rsync friendly**: vanilla JS, inline JSON, no XHR, no server.
//!   Open `<out>/index.html` over `file://` and everything works.
//!
//! The structured viz data is captured by subprocessing the same `wg`
//! binary with `viz --all --no-tui --json`. This keeps the implementation
//! contained in the library crate without requiring it to depend on the
//! binary's `commands::viz` module directly.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use pulldown_cmark::{html as md_html, Options, Parser as MdParser};
use regex::Regex;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use crate::chat::{self, ChatMessage};
use crate::chat_id;
use crate::graph::{Status, Task, WorkGraph};
use crate::messages::{self as msg_queue, CoordinatorMessageStatus, Message, MessageStats};
use crate::parser::load_graph;

// ────────────────────────────────────────────────────────────────────────────
// Public API
// ────────────────────────────────────────────────────────────────────────────

/// Parse a human-readable duration string (e.g. "1h", "24h", "7d", "30d", "2w") into a
/// chrono Duration. Returns an error with a clear message on invalid input.
pub fn parse_since(s: &str) -> Result<Duration> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("Empty duration string — use a value like 1h, 24h, 7d, 30d");
    }
    let (num_str, unit) = if let Some(n) = s.strip_suffix('h') {
        (n, 'h')
    } else if let Some(n) = s.strip_suffix('d') {
        (n, 'd')
    } else if let Some(n) = s.strip_suffix('w') {
        (n, 'w')
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 'm')
    } else {
        anyhow::bail!(
            "Invalid --since value '{}': expected a number followed by h/d/w/m (e.g. 1h, 24h, 7d, 30d)",
            s
        );
    };

    let num: i64 = num_str.parse().map_err(|_| {
        anyhow::anyhow!(
            "Invalid --since value '{}': '{}' is not a valid number",
            s,
            num_str
        )
    })?;

    if num <= 0 {
        anyhow::bail!("--since value must be positive (got '{}')", s);
    }

    Ok(match unit {
        'h' => Duration::hours(num),
        'd' => Duration::days(num),
        'w' => Duration::weeks(num),
        'm' => Duration::minutes(num),
        _ => unreachable!(),
    })
}

#[derive(Debug, Clone)]
pub struct RenderSummary {
    pub out_dir: std::path::PathBuf,
    pub total_in_graph: usize,
    pub public_count: usize,
    pub pages_written: usize,
    pub show_all: bool,
    pub since: Option<String>,
    /// Number of chat task pages whose transcript was rendered (only counts
    /// chats that actually had at least one inbox/outbox message).
    pub chat_transcripts_shown: usize,
    /// Number of chat task pages whose transcript was omitted by the
    /// visibility filter (would have been shown with `--all`).
    pub chat_transcripts_hidden_by_visibility: usize,
}

/// Render-mode options. `Default::default()` reproduces the historical
/// `wg html` behavior: include all tasks (TUI parity), no chat transcripts,
/// no time filter.
#[derive(Debug, Clone, Default)]
pub struct RenderOptions {
    /// Include tasks regardless of `visibility`. When `false`, only
    /// `visibility = public` tasks are rendered (the `--public-only` mode).
    pub show_all: bool,
    /// Time-window filter (e.g. `"24h"`, `"7d"`).
    pub since: Option<String>,
    /// Render chat transcripts on chat task pages (`.chat-N`/`.coordinator-N`).
    /// When `false`, chat task pages still render but the conversation
    /// section is omitted entirely (the historical default).
    pub include_chat: bool,
    /// When `include_chat` is true, also include transcripts whose chat
    /// task's `visibility != public`. When `false`, only public chats'
    /// transcripts are rendered; non-public chats show a hidden-marker line.
    /// Has no effect unless `include_chat = true`.
    pub all_chats: bool,
    /// Project metadata rendered at the top of the index page (title /
    /// byline / abstract). When `None`, the cascade is resolved from
    /// `[project]` config + `<workgraph_dir>/about.md`. To skip metadata
    /// entirely (back-compat path used in some tests), pass
    /// `Some(ProjectMeta::default())`.
    pub project_meta: Option<ProjectMeta>,
}

/// Resolved project metadata for the rendered page header.
///
/// Cascade (first hit wins per field):
///   1. Per-deployment override (in `html-publish.toml`)
///   2. Project-level fields in `<workgraph_dir>/config.toml [project]`
///   3. `<workgraph_dir>/about.md` (abstract only)
///   4. Defaults: title = directory name, byline = empty, abstract = empty
///
/// All three fields are `Option<String>` — when all are `None`/empty, the
/// renderer omits the project-header entirely (no empty block).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectMeta {
    /// Page title (e.g. "Poietic Inc — Active Work"). When `None`, the
    /// minimal "Workgraph" header is used.
    pub title: Option<String>,
    /// One-line byline / tagline shown under the title. Plain text.
    pub byline: Option<String>,
    /// Markdown body rendered as HTML below the byline. Long content is
    /// collapsed behind a "show more" affordance in the rendered page.
    pub abstract_md: Option<String>,
}

impl ProjectMeta {
    /// True when nothing would be rendered — the renderer uses this to
    /// decide whether to omit the project-header block entirely.
    pub fn is_empty(&self) -> bool {
        let blank = |s: &Option<String>| s.as_deref().map(|t| t.trim().is_empty()).unwrap_or(true);
        blank(&self.title) && blank(&self.byline) && blank(&self.abstract_md)
    }
}

/// Resolve project metadata from the project-level cascade only (no
/// per-deployment overrides). Used by plain `wg html` and as the base
/// layer that `wg html publish run` overlays per-deployment fields onto.
///
/// Reads:
///   - `<workgraph_dir>/config.toml [project]` for `title` / `byline`
///     (falls back to the legacy `name` field if `title` is unset).
///   - `<workgraph_dir>/about.md` for the abstract.
///
/// Errors during read are treated as "field unset" — the caller still
/// gets a usable `ProjectMeta`; nothing about HTML generation should fail
/// because of a missing/malformed metadata source.
pub fn resolve_project_meta(workgraph_dir: &Path) -> ProjectMeta {
    let mut meta = ProjectMeta::default();

    // Project-level config.
    if let Ok(cfg) = crate::config::Config::load(workgraph_dir) {
        if meta.title.is_none() {
            meta.title = cfg.project.title.clone().or_else(|| cfg.project.name.clone());
        }
        if meta.byline.is_none() {
            meta.byline = cfg.project.byline.clone();
        }
    }

    // about.md fallback for the abstract.
    if meta.abstract_md.is_none() {
        let about = workgraph_dir.join("about.md");
        if about.exists() {
            if let Ok(body) = fs::read_to_string(&about) {
                let trimmed = body.trim();
                if !trimmed.is_empty() {
                    meta.abstract_md = Some(body);
                }
            }
        }
    }

    meta
}

/// Public render entry point. Builds the complete static site.
pub fn render_site(
    graph: &WorkGraph,
    workgraph_dir: &Path,
    out_dir: &Path,
    opts: RenderOptions,
) -> Result<RenderSummary> {
    let show_all = opts.show_all;
    let since = opts.since.as_deref();
    let include_chat = opts.include_chat;
    let all_chats = opts.all_chats;
    // Resolve project metadata. When the caller didn't pass anything,
    // fall back to the project-level cascade (config + about.md).
    let project_meta = opts
        .project_meta
        .clone()
        .unwrap_or_else(|| resolve_project_meta(workgraph_dir));
    let since_cutoff: Option<DateTime<Utc>> = since
        .map(|s| parse_since(s).map(|d| Utc::now() - d))
        .transpose()?;

    fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create output dir: {}", out_dir.display()))?;
    let tasks_dir = out_dir.join("tasks");
    fs::create_dir_all(&tasks_dir)
        .with_context(|| format!("failed to create tasks dir: {}", tasks_dir.display()))?;

    let all_tasks: Vec<&Task> = graph.tasks().collect();

    // Visibility filter (only when --public-only).
    let visibility_filtered: Vec<&Task> = if show_all {
        all_tasks.clone()
    } else {
        all_tasks
            .iter()
            .filter(|t| t.visibility == "public")
            .copied()
            .collect()
    };

    // Time-window filter (applied on top of visibility filter).
    let included: Vec<&Task> = if let Some(cutoff) = since_cutoff {
        visibility_filtered
            .into_iter()
            .filter(|t| task_in_window(t, cutoff))
            .collect()
    } else {
        visibility_filtered
    };
    let included_ids: HashSet<&str> = included.iter().map(|t| t.id.as_str()).collect();

    // Detect whether any agency tasks (.evaluate-/.assign-/.flip-/.place-/.create-)
    // are in scope. When present we render two viz captures — one substantive,
    // one with the agency layer included — so the in-page toggle can switch
    // between them without a server round-trip. (Web equivalent of the TUI
    // period-key behavior.)
    let has_agency_tasks = included.iter().any(|t| is_agency_task(&t.id));

    // Capture structured viz output (text + node positions + char-level edge map).
    // Always grab the substantive (no agency) viz; if agency tasks are present,
    // also grab the agency-included viz for the toggle-on state.
    let viz = capture_viz_json(workgraph_dir, show_all, false);
    let viz_agency = if has_agency_tasks {
        Some(capture_viz_json(workgraph_dir, show_all, true))
    } else {
        None
    };

    // Eval scores per task.
    let evals = load_eval_scores(workgraph_dir);

    // Per-task message bundles (only populated for tasks with at least one
    // message). Drives the envelope indicator + messages section.
    let task_messages = load_task_messages(workgraph_dir, &included);

    // Compute reachable upstream + downstream sets per task. Used by the JS
    // layer to highlight the "before" / "after" pattern of edges on click.
    let edge_reach = compute_edge_reachability(graph, &included_ids);

    // Build the inline JSON blobs.
    let tasks_json = build_tasks_json(graph, &included, &evals, &task_messages, &included_ids);
    let edges_json = build_edges_json(&edge_reach);
    let cycles_json = build_cycles_json(&viz);

    // Write static assets.
    fs::write(out_dir.join("style.css"), STYLE_CSS).context("failed to write style.css")?;
    fs::write(out_dir.join("panel.js"), PANEL_JS).context("failed to write panel.js")?;

    // Per-task pages (deep-link targets — work with file:// URLs).
    let mut pages_written = 0usize;
    let mut chat_transcripts_shown = 0usize;
    let mut chat_transcripts_hidden = 0usize;
    for task in &included {
        let eval = evals.get(&task.id);
        let chat_decision = decide_chat_render(task, include_chat, all_chats);
        let chat_block = match &chat_decision {
            ChatRender::Render(session_ref) => {
                let messages = load_chat_messages(workgraph_dir, session_ref);
                if !messages.is_empty() {
                    chat_transcripts_shown += 1;
                }
                Some(render_conversation_block(&messages))
            }
            ChatRender::HiddenByVisibility(visibility) => {
                chat_transcripts_hidden += 1;
                Some(render_chat_hidden_notice(visibility))
            }
            ChatRender::None => None,
        };
        let msg_bundle = task_messages.get(&task.id);
        let messages_block = render_messages_section(msg_bundle);
        let html = render_task_page(
            task,
            graph,
            &included_ids,
            eval,
            chat_block.as_deref(),
            if messages_block.is_empty() {
                None
            } else {
                Some(messages_block.as_str())
            },
        );
        let path = tasks_dir.join(format!("{}.html", url_encode_id(&task.id)));
        fs::write(&path, html).with_context(|| format!("failed to write {}", path.display()))?;
        pages_written += 1;
    }

    // Render the index (after chat counts are known so the header can show them).
    let index_html = render_index(
        graph,
        &included,
        &included_ids,
        &viz,
        viz_agency.as_ref(),
        &tasks_json,
        &edges_json,
        &cycles_json,
        &task_messages,
        show_all,
        since,
        include_chat,
        chat_transcripts_shown,
        chat_transcripts_hidden,
        has_agency_tasks,
        &project_meta,
    );
    fs::write(out_dir.join("index.html"), &index_html).context("failed to write index.html")?;

    Ok(RenderSummary {
        out_dir: out_dir.to_path_buf(),
        total_in_graph: graph.tasks().count(),
        public_count: included.len(),
        pages_written,
        show_all,
        since: since.map(|s| s.to_string()),
        chat_transcripts_shown,
        chat_transcripts_hidden_by_visibility: chat_transcripts_hidden,
    })
}

pub fn run(
    workgraph_dir: &Path,
    out: &Path,
    all: bool,
    since: Option<&str>,
    include_chat: bool,
    all_chats: bool,
    json: bool,
) -> Result<()> {
    let graph_path = workgraph_dir.join("graph.jsonl");
    if !graph_path.exists() {
        anyhow::bail!(
            "Workgraph not initialized at {}. Run `wg init` first.",
            workgraph_dir.display()
        );
    }
    let graph = load_graph(&graph_path).context("failed to load graph")?;

    let summary = render_site(
        &graph,
        workgraph_dir,
        out,
        RenderOptions {
            show_all: all,
            since: since.map(|s| s.to_string()),
            include_chat,
            all_chats,
            // None → render_site falls back to the project-level cascade
            // (config.toml [project] + about.md). Plain `wg html` doesn't
            // know about per-deployment overrides; those come from
            // `wg html publish run`, which threads its own ProjectMeta in.
            project_meta: None,
        },
    )?;

    if json {
        let payload = serde_json::json!({
            "out_dir": summary.out_dir.display().to_string(),
            "total_in_graph": summary.total_in_graph,
            "public_count": summary.public_count,
            "pages_written": summary.pages_written,
            "show_all": summary.show_all,
            "since": summary.since,
            "chat_transcripts_shown": summary.chat_transcripts_shown,
            "chat_transcripts_hidden_by_visibility": summary.chat_transcripts_hidden_by_visibility,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        let filter = if summary.show_all {
            format!("all tasks ({} included)", summary.public_count)
        } else {
            format!(
                "{} public of {} total",
                summary.public_count, summary.total_in_graph,
            )
        };
        println!(
            "Wrote {} pages to {} ({})",
            summary.pages_written + 1,
            summary.out_dir.display(),
            filter,
        );
        if include_chat {
            if summary.chat_transcripts_shown == 0
                && summary.chat_transcripts_hidden_by_visibility > 0
            {
                println!(
                    "No chat transcripts shown (all {} chats are non-public; pass --all to include them).",
                    summary.chat_transcripts_hidden_by_visibility,
                );
            } else {
                println!(
                    "Showing {} chat transcript{} ({} omitted by visibility).",
                    summary.chat_transcripts_shown,
                    if summary.chat_transcripts_shown == 1 { "" } else { "s" },
                    summary.chat_transcripts_hidden_by_visibility,
                );
            }
        }
        println!("Open {}/index.html in a browser.", summary.out_dir.display());
    }

    Ok(())
}

// ────────────────────────────────────────────────────────────────────────────
// Static assets (compiled in via include_str!)
// ────────────────────────────────────────────────────────────────────────────

const STYLE_CSS: &str = include_str!("html_assets/style.css");
const PANEL_JS: &str = include_str!("html_assets/panel.js");

// ────────────────────────────────────────────────────────────────────────────
// Status helpers
// ────────────────────────────────────────────────────────────────────────────

/// Return true if the task has any timestamp that falls within the window.
fn task_in_window(task: &Task, cutoff: DateTime<Utc>) -> bool {
    let timestamps: &[Option<&str>] = &[
        task.created_at.as_deref(),
        task.started_at.as_deref(),
        task.completed_at.as_deref(),
        task.last_iteration_completed_at.as_deref(),
    ];
    timestamps.iter().flatten().any(|ts| {
        DateTime::parse_from_rfc3339(ts)
            .map(|dt| dt.with_timezone(&Utc) >= cutoff)
            .unwrap_or(false)
    })
}

/// Status color — RGB triples mirror the TUI palette in
/// `tui::viz_viewer::state::flash_color_for_status` (state.rs:271).
fn status_color(status: Status) -> &'static str {
    match status {
        Status::Done => "rgb(80,220,100)",
        Status::Failed => "rgb(220,60,60)",
        Status::InProgress => "rgb(60,200,220)",
        Status::Open => "rgb(200,200,80)",
        Status::Blocked => "rgb(180,120,60)",
        Status::Abandoned => "rgb(140,100,160)",
        Status::Waiting | Status::PendingValidation => "rgb(60,160,220)",
        Status::PendingEval => "rgb(140,230,80)",
        Status::FailedPendingEval => "rgb(210,130,70)", // warm coral: between failed-red and pending-yellow
        Status::Incomplete => "rgb(255,165,0)",
    }
}

fn status_class(status: Status) -> &'static str {
    match status {
        Status::Done => "done",
        Status::Failed => "failed",
        Status::InProgress => "in-progress",
        Status::Open => "open",
        Status::Blocked => "blocked",
        Status::Abandoned => "abandoned",
        Status::Waiting => "waiting",
        Status::PendingValidation => "pending-validation",
        Status::PendingEval => "pending-eval",
        Status::FailedPendingEval => "failed-pending-eval",
        Status::Incomplete => "incomplete",
    }
}

fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn url_encode_id(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for ch in id.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(ch),
            _ => out.push_str(&format!("%{:02X}", ch as u32)),
        }
    }
    out
}

fn task_filename(id: &str) -> String {
    format!("tasks/{}.html", url_encode_id(id))
}

/// Convert markdown text to an HTML string using pulldown-cmark.
/// Raw HTML in the input is not passed through (safe by default).
fn markdown_to_html(text: &str) -> String {
    let opts = Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS;
    let parser = MdParser::new_ext(text, opts);
    let mut out = String::with_capacity(text.len() * 2);
    md_html::push_html(&mut out, parser);
    out
}

// ────────────────────────────────────────────────────────────────────────────
// Structured viz capture (subprocess `wg viz --json`)
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
struct VizJson {
    /// Rendered ASCII (may contain ANSI escapes).
    #[serde(default)]
    text: String,
    /// task_id → line index.
    #[serde(default)]
    node_lines: BTreeMap<String, usize>,
    /// per-character edge cells.
    #[serde(default)]
    char_edges: Vec<CharEdge>,
    /// task_id → list of cycle members (only for tasks in non-trivial SCCs).
    #[serde(default)]
    cycle_members: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct CharEdge {
    line: usize,
    col: usize,
    from: String,
    to: String,
}

/// Capture viz output as structured JSON via subprocess. Falls back to an
/// empty viz if the subprocess fails (the page still renders, just without
/// the ASCII tree section).
///
/// `show_all` corresponds to viz's `--all` (visibility/WCC scope).
/// `show_agency` corresponds to viz's `--show-internal` (include agency-style
/// `.evaluate-*`, `.assign-*`, etc. tasks in the rendered ASCII layout).
fn capture_viz_json(workgraph_dir: &Path, show_all: bool, show_agency: bool) -> VizJson {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return VizJson::default(),
    };
    // The `--json` flag is the global one (clap-level); placing it before the
    // subcommand keeps clap from rejecting it as a subcommand-local option.
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--dir")
        .arg(workgraph_dir)
        .arg("--json")
        .arg("viz")
        .arg("--no-tui")
        .arg("--columns")
        .arg("140")
        .arg("--edge-color")
        .arg("gray");
    if show_all {
        cmd.arg("--all");
    }
    if show_agency {
        cmd.arg("--show-internal");
    }
    let out = match cmd.output() {
        Ok(o) if o.status.success() => o,
        _ => return VizJson::default(),
    };
    serde_json::from_slice(&out.stdout).unwrap_or_default()
}

/// Match the TUI's `is_agency_task_id` (src/tui/viz_viewer/event.rs:5566) —
/// these prefixes mark internal agency-pipeline tasks (.evaluate-, .assign-,
/// .place-, .flip-, .create-).
pub(crate) fn is_agency_task(id: &str) -> bool {
    id.starts_with(".evaluate-")
        || id.starts_with(".assign-")
        || id.starts_with(".place-")
        || id.starts_with(".flip-")
        || id.starts_with(".create-")
}

// ────────────────────────────────────────────────────────────────────────────
// ANSI strip + viz HTML rendering
// ────────────────────────────────────────────────────────────────────────────

/// Remove ANSI CSI escape sequences (\x1b[...m) from text. We strip them
/// because we apply our own coloring via CSS classes per character.
fn strip_ansi(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            // CSI: skip until a final byte in 0x40..=0x7e
            i += 2;
            while i < bytes.len() {
                let b = bytes[i];
                i += 1;
                if (0x40..=0x7e).contains(&b) {
                    break;
                }
            }
        } else {
            // Push the next valid UTF-8 character (avoid splitting multibyte chars).
            let ch_start = i;
            let first = bytes[i];
            let len = if first < 0x80 {
                1
            } else if first < 0xc0 {
                1 // shouldn't happen for valid UTF-8 — treat as 1
            } else if first < 0xe0 {
                2
            } else if first < 0xf0 {
                3
            } else {
                4
            };
            let end = (ch_start + len).min(bytes.len());
            if let Ok(s) = std::str::from_utf8(&bytes[ch_start..end]) {
                out.push_str(s);
            }
            i = end;
        }
    }
    out
}

/// Render the captured viz text into clickable HTML.
///
/// Strategy:
/// 1. Strip ANSI escapes (we apply our own coloring).
/// 2. For each line, build a per-character "marker" map:
///    - `Marker::TaskLink(id, status)` for cells that fall inside a task-id
///      label (or the trailing status-indicator parens).
///    - `Marker::Edge(edges)` for cells in the `char_edge_map`.
///    - `Marker::Plain` otherwise.
/// 3. Walk character cells, opening/closing spans on marker transitions.
///
/// `extra_pre_class` is appended to the outer `<pre>` element's class list so
/// callers can render multiple viz blocks (e.g. substantive vs. agency-included)
/// and switch between them via CSS.
fn render_viz_html(
    viz: &VizJson,
    graph: &WorkGraph,
    included_ids: &HashSet<&str>,
    extra_pre_class: &str,
) -> String {
    let plain = strip_ansi(&viz.text);
    if plain.trim().is_empty() {
        let cls = if extra_pre_class.is_empty() {
            "viz-pre".to_string()
        } else {
            format!("viz-pre {}", extra_pre_class)
        };
        return format!("<pre class=\"{}\">(no tasks to display)</pre>", cls);
    }

    // Per-line cells of (column → list of edges). Note: char_edge_map columns
    // are visible-column indices (not byte offsets). We line up by chars.
    let mut edges_by_pos: HashMap<(usize, usize), Vec<(String, String)>> = HashMap::new();
    for e in &viz.char_edges {
        edges_by_pos
            .entry((e.line, e.col))
            .or_default()
            .push((e.from.clone(), e.to.clone()));
    }

    // node_lines maps task_id → line index. We'll also need its status.
    let task_status: HashMap<&str, Status> = graph
        .tasks()
        .map(|t| (t.id.as_str(), t.status))
        .collect();

    // For each line, identify task-id occurrences. The viz typically renders a
    // single task on its own line, but a task id may appear multiple times
    // (e.g., a header summary line). We mark every literal occurrence of any
    // included task id within the line.
    let mut task_id_strs: Vec<&str> = included_ids.iter().copied().collect();
    // Match longest ids first so 'foo-bar' doesn't mask 'foo'.
    task_id_strs.sort_by(|a, b| b.len().cmp(&a.len()));

    let mut html = String::with_capacity(plain.len() * 2);
    if extra_pre_class.is_empty() {
        html.push_str("<pre class=\"viz-pre\">");
    } else {
        html.push_str("<pre class=\"viz-pre ");
        html.push_str(extra_pre_class);
        html.push_str("\">");
    }

    for (line_idx, line) in plain.lines().enumerate() {
        // Collect `(start_char_idx, end_char_idx, task_id)` ranges where a
        // task id (and its trailing "  (status...)" decorator) lives. The
        // decorator is included so that clicking the status-glyph parens
        // opens the same task as clicking the id.
        let line_chars: Vec<char> = line.chars().collect();
        let line_str: String = line_chars.iter().collect();
        // (id_start, decorator_start, end, id, status). decorator_start == end
        // means no `(status · ...)` decorator follows the id.
        let mut task_ranges: Vec<(usize, usize, usize, &str, Status)> = Vec::new();
        // Time-suffix ranges (e.g. " 5m", " 1d") that follow the parenthetical.
        // Rendered with the muted-foreground colour to demote them below the
        // active info inside the parens.
        let mut time_ranges: Vec<(usize, usize)> = Vec::new();

        // Use byte-index find, then convert to char index.
        for &id in &task_id_strs {
            // Find every occurrence of the id in this line whose surrounding
            // characters are not identifier-y (so we don't match 'foo' inside
            // 'foo-bar').
            let mut byte_search_start = 0usize;
            while let Some(rel) = line[byte_search_start..].find(id) {
                let byte_pos = byte_search_start + rel;
                let byte_end = byte_pos + id.len();
                // Boundary check
                let prev_ok = byte_pos == 0
                    || line[..byte_pos]
                        .chars()
                        .next_back()
                        .map(|c| !is_id_char(c))
                        .unwrap_or(true);
                let next_ok = byte_end == line.len()
                    || line[byte_end..]
                        .chars()
                        .next()
                        .map(|c| !is_id_char(c))
                        .unwrap_or(true);
                if prev_ok && next_ok {
                    let char_start = line[..byte_pos].chars().count();
                    let id_end = char_start + id.chars().count();
                    // Extend the range across an immediately-following
                    // status decorator like "  (in-progress · ...)" so the
                    // whole label is clickable. The decorator_start marks
                    // where the leading spaces+`(` begin so render_line can
                    // colour the decorator white separately from the
                    // status-coloured id.
                    let char_end = extend_through_status_decorator(&line_chars, id_end);
                    let decorator_start = if char_end > id_end { id_end } else { id_end };
                    let st = task_status.get(id).copied().unwrap_or(Status::Open);
                    task_ranges.push((char_start, decorator_start, char_end, id, st));
                    if let Some(time) = find_time_suffix_after(&line_chars, char_end) {
                        time_ranges.push(time);
                    }
                }
                byte_search_start = byte_pos + id.len();
            }
        }

        // Resolve overlaps: prefer earlier start, longer end. Sort and dedupe.
        task_ranges.sort_by(|a, b| (a.0, std::cmp::Reverse(a.2)).cmp(&(b.0, std::cmp::Reverse(b.2))));
        let mut nonoverlapping: Vec<(usize, usize, usize, &str, Status)> = Vec::new();
        for r in task_ranges {
            if let Some(last) = nonoverlapping.last() {
                if r.0 < last.2 {
                    continue;
                }
            }
            nonoverlapping.push(r);
        }
        time_ranges.sort_by_key(|t| t.0);
        time_ranges.dedup();

        render_line(
            &mut html,
            line_idx,
            &line_chars,
            &nonoverlapping,
            &time_ranges,
            &edges_by_pos,
            &line_str,
        );
        html.push('\n');
    }

    html.push_str("</pre>");
    html
}

/// Walk one line's character cells emitting plain text, edge spans, or
/// task-link spans as appropriate.
fn render_line(
    out: &mut String,
    line_idx: usize,
    line_chars: &[char],
    task_ranges: &[(usize, usize, usize, &str, Status)],
    time_ranges: &[(usize, usize)],
    edges_by_pos: &HashMap<(usize, usize), Vec<(String, String)>>,
    _line_str: &str,
) {
    let mut col = 0usize;
    let mut range_iter = task_ranges.iter().peekable();
    let mut time_iter = time_ranges.iter().peekable();

    while col < line_chars.len() {
        // Are we at the start of a task-link range?
        if let Some(&(start, decorator_start, end, id, status)) = range_iter.peek() {
            if col == *start {
                let s_class = status_class(*status);
                let agency_cls = if is_agency_task(id) { " is-agency" } else { "" };
                let chat_class = if chat_id::is_chat_task_id(id) { " chat-agent" } else { "" };
                out.push_str("<span class=\"task-link");
                out.push_str(agency_cls);
                out.push_str(chat_class);
                out.push_str("\" data-task-id=\"");
                out.push_str(&escape_html(id));
                out.push_str("\" data-status=\"");
                out.push_str(s_class);
                out.push_str("\">");
                // Id portion — coloured by status via [data-status].
                let span_end = (*end).min(line_chars.len());
                let decor_start = (*decorator_start).min(span_end);
                for c in &line_chars[col..decor_start] {
                    out.push_str(&escape_html(&c.to_string()));
                }
                // Decorator portion — clickable but rendered with the
                // foreground colour so the parens read as active info instead
                // of inheriting the status hue (which dominates and inverts
                // the visual hierarchy with the gray time suffix).
                if decor_start < span_end {
                    out.push_str("<span class=\"decorator\">");
                    for c in &line_chars[decor_start..span_end] {
                        out.push_str(&escape_html(&c.to_string()));
                    }
                    out.push_str("</span>");
                }
                out.push_str("</span>");
                col = span_end;
                range_iter.next();
                continue;
            }
        }

        // Are we at the start of a time-suffix range?
        if let Some(&&(t_start, t_end)) = time_iter.peek() {
            if col == t_start {
                let span_end = t_end.min(line_chars.len());
                out.push_str("<span class=\"time-suffix\">");
                for c in &line_chars[col..span_end] {
                    out.push_str(&escape_html(&c.to_string()));
                }
                out.push_str("</span>");
                col = span_end;
                time_iter.next();
                continue;
            }
        }

        // Otherwise emit a single character — wrapped in an edge span if a
        // char_edge_map entry exists at (line_idx, col).
        let c = line_chars[col];
        if let Some(edges) = edges_by_pos.get(&(line_idx, col)) {
            // Build the data-edges attribute as `from1>to1|from2>to2|…`.
            // We use `>` as the separator (not in task ids) and `|` for list.
            let mut data = String::new();
            for (i, (from, to)) in edges.iter().enumerate() {
                if i > 0 {
                    data.push('|');
                }
                data.push_str(&escape_html(from));
                data.push('>');
                data.push_str(&escape_html(to));
            }
            out.push_str("<span class=\"edge\" data-edges=\"");
            out.push_str(&data);
            out.push_str("\">");
            out.push_str(&escape_html(&c.to_string()));
            out.push_str("</span>");
        } else {
            // Plain text cell — wrapped in a `text-cell` span only when its
            // line contains a task-link (so the dim-others rule doesn't dim
            // structure-less header rows). For simplicity we always emit a
            // text-cell span — the dimming rule fires only with `body[data-
            // selected]` so unrelated pages aren't affected.
            out.push_str(&escape_html(&c.to_string()));
        }
        col += 1;
    }
}

/// True if `c` is part of a task identifier.
fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'
}

/// Locate the relative-time suffix (e.g. ` 5m`, ` 1d`, ` 2mo`) that follows
/// the parenthetical decorator. Skips an optional intervening delay hint
/// (` ⏳<duration>`). Returns `(start_col, end_col)` covering the leading
/// space + digits + unit so the whole token can be coloured as one piece.
fn find_time_suffix_after(line_chars: &[char], after_paren_col: usize) -> Option<(usize, usize)> {
    let mut i = after_paren_col;

    // Skip optional `⏳<duration>` delay hint (with leading space).
    let mut j = i;
    while j < line_chars.len() && line_chars[j] == ' ' {
        j += 1;
    }
    if j < line_chars.len() && line_chars[j] == '⏳' {
        j += 1;
        while j < line_chars.len() && line_chars[j] != ' ' {
            j += 1;
        }
        i = j;
    }

    // Match `\s+\d+(unit)` where unit ∈ {mo, s, m, h, d, w, y}.
    let token_start = i;
    let mut k = i;
    while k < line_chars.len() && line_chars[k] == ' ' {
        k += 1;
    }
    if k == token_start {
        return None;
    }
    let digits_start = k;
    while k < line_chars.len() && line_chars[k].is_ascii_digit() {
        k += 1;
    }
    if k == digits_start {
        return None;
    }
    // `mo` must be matched before `m` so the longer unit wins.
    let unit_chars: &[&[char]] = &[&['m', 'o'], &['s'], &['m'], &['h'], &['d'], &['w'], &['y']];
    for unit in unit_chars {
        if k + unit.len() <= line_chars.len() && line_chars[k..k + unit.len()] == **unit {
            let after = k + unit.len();
            // Boundary: next char must not be a letter (so `1d` matches but
            // `done` doesn't, and `5mo` isn't truncated to `5m`).
            if after >= line_chars.len() || !line_chars[after].is_ascii_alphabetic() {
                return Some((token_start, after));
            }
        }
    }
    None
}

/// If position `start_col` lies right after a task id, see whether the next
/// chars match `  (` (two spaces then a left paren). If so, extend the range
/// through the matching closing paren so the whole "(status · ...)" decorator
/// is part of the clickable region.
fn extend_through_status_decorator(line_chars: &[char], start_col: usize) -> usize {
    let mut i = start_col;
    // Skip spaces.
    let space_start = i;
    while i < line_chars.len() && line_chars[i] == ' ' {
        i += 1;
    }
    // If we didn't find at least one space + `(`, return the original start.
    if i == space_start || i >= line_chars.len() || line_chars[i] != '(' {
        return start_col;
    }
    // Walk to the matching `)` (no nesting in our format).
    let mut depth = 0;
    while i < line_chars.len() {
        match line_chars[i] {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    // Unmatched paren — extend to end of line.
    line_chars.len()
}

// ────────────────────────────────────────────────────────────────────────────
// Message queue surfacing (parity with TUI envelope indicator)
// ────────────────────────────────────────────────────────────────────────────

/// Per-task message bundle surfaced into the HTML export. Mirrors the data the
/// TUI uses to draw its envelope indicator (`✉`/`↩`/`✓`) and message panel.
///
/// Two perspectives co-exist (matching the TUI exactly):
/// - `incoming` / `outgoing` are agent-perspective counts from
///   `messages::MessageStats` — outgoing = sent BY the task's assigned agent,
///   incoming = sent BY anyone else. This drives the count text.
/// - `status` is the coordinator/user perspective from
///   `messages::coordinator_message_status` — Unseen/Seen/Replied describes
///   how the TUI cursor compares to the latest non-coordinator message. This
///   drives the icon glyph and color.
#[derive(Debug, Clone)]
struct TaskMessages {
    /// All messages on the task's queue, ordered by id.
    messages: Vec<Message>,
    /// Coordinator-perspective read state (`Unseen` / `Seen` / `Replied`).
    /// `None` when the task has only coordinator-side messages (e.g. just
    /// `wg msg send` from the CLI with no agent reply yet).
    status: Option<CoordinatorMessageStatus>,
    /// Count of messages NOT sent by the task's assigned agent (a.k.a.
    /// "from the user's perspective, things sent IN to the task"). Mirrors
    /// `MessageStats::incoming`.
    incoming: usize,
    /// Count of messages sent by the task's assigned agent. Mirrors
    /// `MessageStats::outgoing`.
    outgoing: usize,
    /// True iff there are unread incoming messages relative to the assigned
    /// agent's read cursor. Mirrors `MessageStats::has_unread`. Drives the
    /// `has-unread-msg` CSS class.
    has_unread: bool,
}

impl TaskMessages {
    fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

/// Load the per-task message bundle, only emitting an entry for tasks that
/// have at least one message. Filtering at this layer keeps the inline JSON
/// size bounded by the actual queue activity, not the full task count.
fn load_task_messages(workgraph_dir: &Path, included: &[&Task]) -> HashMap<String, TaskMessages> {
    let mut out: HashMap<String, TaskMessages> = HashMap::new();
    for task in included {
        let messages = match msg_queue::list_messages(workgraph_dir, &task.id) {
            Ok(m) if !m.is_empty() => m,
            _ => continue,
        };
        let stats: MessageStats =
            msg_queue::message_stats(workgraph_dir, &task.id, task.assigned.as_deref());
        let status = msg_queue::coordinator_message_status(workgraph_dir, &task.id);
        out.insert(
            task.id.clone(),
            TaskMessages {
                messages,
                status,
                incoming: stats.incoming,
                outgoing: stats.outgoing,
                has_unread: stats.has_unread,
            },
        );
    }
    out
}

/// Stable lowercase identifier for the coordinator status (used as a CSS class
/// suffix and a JSON value). Returns `"none"` when there is no incoming
/// message — callers that don't want to render anything in that case should
/// check `is_empty()` on the bundle first.
fn msg_status_class(status: Option<&CoordinatorMessageStatus>) -> &'static str {
    match status {
        Some(CoordinatorMessageStatus::Unseen) => "unseen",
        Some(CoordinatorMessageStatus::Seen) => "seen",
        Some(CoordinatorMessageStatus::Replied) => "replied",
        None => "none",
    }
}

/// Single envelope glyph used in the HTML. Matches the TUI status icons:
/// ✉ for unseen, ↩ for seen-but-not-replied, ✓ for replied. Falling back to
/// ✉ when `status == None` keeps the visual signal even on outgoing-only
/// queues (rare — `coordinator_message_status` returns `None` in that case).
fn msg_status_icon(status: Option<&CoordinatorMessageStatus>) -> char {
    match status {
        Some(CoordinatorMessageStatus::Unseen) => '✉',
        Some(CoordinatorMessageStatus::Seen) => '↩',
        Some(CoordinatorMessageStatus::Replied) => '✓',
        None => '✉',
    }
}

/// Build the task-list-row indicator span. Returns the empty string when the
/// task has no messages so callers can unconditionally interpolate the result.
/// The element is clickable; panel.js uses the `data-msg-action="messages"`
/// attribute to scroll the inspector to the Messages section after opening.
fn render_msg_indicator_inline(bundle: Option<&TaskMessages>, task_id: &str) -> String {
    let bundle = match bundle {
        Some(b) if !b.is_empty() => b,
        _ => return String::new(),
    };
    let status_cls = msg_status_class(bundle.status.as_ref());
    let icon = msg_status_icon(bundle.status.as_ref());
    let count_str = if bundle.outgoing > 0 {
        format!("{}/{}", bundle.incoming, bundle.outgoing)
    } else {
        format!("{}", bundle.incoming)
    };
    let unread_cls = if bundle.has_unread {
        " has-unread-msg"
    } else {
        ""
    };
    let title = match bundle.status.as_ref() {
        Some(CoordinatorMessageStatus::Unseen) => {
            format!("{} unread message(s) — click to open inspector", bundle.incoming)
        }
        Some(CoordinatorMessageStatus::Seen) => {
            format!("{} message(s), all seen — click to open inspector", bundle.incoming)
        }
        Some(CoordinatorMessageStatus::Replied) => {
            format!("{} message(s), replied — click to open inspector", bundle.incoming)
        }
        None => format!(
            "{} message(s) — click to open inspector",
            bundle.incoming + bundle.outgoing
        ),
    };
    format!(
        " <span class=\"msg-indicator msg-{cls}{unread_cls}\" \
         data-task-id=\"{task}\" data-msg-action=\"messages\" \
         title=\"{title}\" aria-label=\"{title}\">\
         <span class=\"msg-glyph\">{icon}</span>\
         <span class=\"msg-count\">{count}</span>\
         </span>",
        cls = status_cls,
        unread_cls = unread_cls,
        task = escape_html(task_id),
        title = escape_html(&title),
        icon = icon,
        count = escape_html(&count_str),
    )
}

/// Render the full Messages section used both in the per-task page and the
/// inspector side panel. Empty bundles produce an empty string so callers can
/// drop the section entirely. The DOM id `messages-section` is the scroll
/// target for the indicator-click action.
fn render_messages_section(bundle: Option<&TaskMessages>) -> String {
    let bundle = match bundle {
        Some(b) if !b.is_empty() => b,
        _ => return String::new(),
    };
    let status_cls = msg_status_class(bundle.status.as_ref());
    let summary = match bundle.status.as_ref() {
        Some(CoordinatorMessageStatus::Unseen) => format!(
            "{} message{} ({} unread)",
            bundle.messages.len(),
            if bundle.messages.len() == 1 { "" } else { "s" },
            bundle.incoming,
        ),
        Some(CoordinatorMessageStatus::Seen) => format!(
            "{} message{} (all seen)",
            bundle.messages.len(),
            if bundle.messages.len() == 1 { "" } else { "s" },
        ),
        Some(CoordinatorMessageStatus::Replied) => format!(
            "{} message{} (replied)",
            bundle.messages.len(),
            if bundle.messages.len() == 1 { "" } else { "s" },
        ),
        None => format!(
            "{} message{}",
            bundle.messages.len(),
            if bundle.messages.len() == 1 { "" } else { "s" },
        ),
    };
    let icon = msg_status_icon(bundle.status.as_ref());
    let mut s = String::new();
    s.push_str(&format!(
        "<section id=\"messages-section\" class=\"messages-section msg-{cls}\">\n\
         <h2><span class=\"msg-glyph\">{icon}</span> Messages \
         <span class=\"messages-summary\">{summary}</span></h2>\n",
        cls = status_cls,
        icon = icon,
        summary = escape_html(&summary),
    ));
    s.push_str("<ol class=\"messages-list\">\n");
    for msg in &bundle.messages {
        let is_coordinator = matches!(msg.sender.as_str(), "tui" | "user" | "coordinator");
        let role_cls = if is_coordinator {
            "msg-outgoing"
        } else {
            "msg-incoming"
        };
        let priority_cls = if msg.priority == "urgent" {
            " msg-urgent"
        } else {
            ""
        };
        s.push_str(&format!(
            "<li class=\"msg-row {role} {status_row}{prio}\">\
             <div class=\"msg-head\">\
             <span class=\"msg-id\">#{id}</span>\
             <span class=\"msg-sender\">{sender}</span>\
             <span class=\"msg-ts\">{ts}</span>\
             <span class=\"msg-status\">{status_label}</span>\
             </div>\
             <pre class=\"msg-body\">{body}</pre>\
             </li>\n",
            role = role_cls,
            status_row = format!("msg-row-{}", msg.status),
            prio = priority_cls,
            id = msg.id,
            sender = escape_html(&msg.sender),
            ts = escape_html(&msg.timestamp),
            status_label = escape_html(&msg.status.to_string()),
            body = escape_html(&msg.body),
        ));
    }
    s.push_str("</ol>\n</section>\n");
    s
}

/// Serialize the bundle into the inline tasks JSON consumed by panel.js.
fn task_messages_to_json(bundle: &TaskMessages) -> serde_json::Value {
    let msgs: Vec<serde_json::Value> = bundle
        .messages
        .iter()
        .map(|m| {
            serde_json::json!({
                "id": m.id,
                "sender": m.sender,
                "timestamp": m.timestamp,
                "body": m.body,
                "priority": m.priority,
                "status": m.status.to_string(),
            })
        })
        .collect();
    serde_json::json!({
        "status": msg_status_class(bundle.status.as_ref()),
        "incoming": bundle.incoming,
        "outgoing": bundle.outgoing,
        "has_unread": bundle.has_unread,
        "icon": msg_status_icon(bundle.status.as_ref()).to_string(),
        "messages": msgs,
    })
}

// ────────────────────────────────────────────────────────────────────────────
// Eval scores
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
struct EvalSummary {
    score: f64,
    dimensions: Vec<(String, f64)>,
}

fn load_eval_scores(workgraph_dir: &Path) -> HashMap<String, EvalSummary> {
    let evals_dir = workgraph_dir.join("agency").join("evaluations");
    let mut latest: HashMap<String, (String, EvalSummary)> = HashMap::new();
    let entries = match fs::read_dir(&evals_dir) {
        Ok(e) => e,
        Err(_) => return HashMap::new(),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e != "json").unwrap_or(true) {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let v: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let task_id = match v.get("task_id").and_then(|x| x.as_str()) {
            Some(id) => id.to_string(),
            None => continue,
        };
        let score = match v.get("score").and_then(|x| x.as_f64()) {
            Some(s) => s,
            None => continue,
        };
        let timestamp = v
            .get("timestamp")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let dims: Vec<(String, f64)> = v
            .get("dimensions")
            .and_then(|d| d.as_object())
            .map(|obj| {
                let mut pairs: Vec<(String, f64)> = obj
                    .iter()
                    .filter_map(|(k, val)| val.as_f64().map(|f| (k.clone(), f)))
                    .collect();
                pairs.sort_by(|a, b| a.0.cmp(&b.0));
                pairs
            })
            .unwrap_or_default();

        let keep = match latest.get(&task_id) {
            None => true,
            Some((existing_ts, _)) => &timestamp > existing_ts,
        };
        if keep {
            latest.insert(
                task_id,
                (
                    timestamp,
                    EvalSummary {
                        score,
                        dimensions: dims,
                    },
                ),
            );
        }
    }
    latest
        .into_iter()
        .map(|(task_id, (_, summary))| (task_id, summary))
        .collect()
}

// ────────────────────────────────────────────────────────────────────────────
// Reachability (used for highlight on selection)
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct EdgeReach {
    /// task_id → ancestor set (visible upstream tasks reachable via --after).
    upstream: HashMap<String, BTreeSet<String>>,
    /// task_id → descendant set (visible downstream tasks reachable via --before).
    downstream: HashMap<String, BTreeSet<String>>,
}

fn compute_edge_reachability(graph: &WorkGraph, included: &HashSet<&str>) -> EdgeReach {
    // Build forward + reverse adjacency limited to the included set.
    let mut forward: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut reverse: HashMap<&str, Vec<&str>> = HashMap::new();
    for task in graph.tasks() {
        if !included.contains(task.id.as_str()) {
            continue;
        }
        for blocker in &task.after {
            if included.contains(blocker.as_str()) {
                forward.entry(blocker).or_default().push(task.id.as_str());
                reverse.entry(task.id.as_str()).or_default().push(blocker);
            }
        }
    }

    let mut up: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut down: HashMap<String, BTreeSet<String>> = HashMap::new();

    for &id in included {
        // Upstream BFS via reverse adjacency.
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut queue: Vec<&str> = reverse.get(id).cloned().unwrap_or_default();
        while let Some(n) = queue.pop() {
            if seen.insert(n.to_string()) {
                if let Some(parents) = reverse.get(n) {
                    for p in parents {
                        queue.push(p);
                    }
                }
            }
        }
        up.insert(id.to_string(), seen);

        // Downstream BFS via forward adjacency.
        let mut seen2: BTreeSet<String> = BTreeSet::new();
        let mut queue2: Vec<&str> = forward.get(id).cloned().unwrap_or_default();
        while let Some(n) = queue2.pop() {
            if seen2.insert(n.to_string()) {
                if let Some(children) = forward.get(n) {
                    for c in children {
                        queue2.push(c);
                    }
                }
            }
        }
        down.insert(id.to_string(), seen2);
    }

    EdgeReach {
        upstream: up,
        downstream: down,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Inline JSON builders
// ────────────────────────────────────────────────────────────────────────────

fn task_to_json(
    task: &Task,
    graph: &WorkGraph,
    eval: Option<&EvalSummary>,
    msgs: Option<&TaskMessages>,
    included_ids: &HashSet<&str>,
) -> serde_json::Value {
    let log_entries: Vec<serde_json::Value> = task
        .log
        .iter()
        .rev()
        .take(40)
        .rev()
        .map(|e| {
            serde_json::json!({
                "timestamp": e.timestamp,
                "message": e.message,
            })
        })
        .collect();

    let after_visible: Vec<&str> = task
        .after
        .iter()
        .map(|s| s.as_str())
        .filter(|id| included_ids.contains(id))
        .collect();
    let before_visible: Vec<&str> = graph
        .tasks()
        .filter(|t| t.after.iter().any(|a| a == &task.id))
        .filter(|t| included_ids.contains(t.id.as_str()))
        .map(|t| t.id.as_str())
        .collect();

    let mut obj = serde_json::json!({
        "id": task.id,
        "title": task.title,
        "status": task.status.to_string(),
        "after": after_visible,
        "before": before_visible,
        "tags": task.tags,
        "log": log_entries,
        "loop_iteration": task.loop_iteration,
        "detail_href": task_filename(&task.id),
    });

    if let Some(m) = &task.model {
        obj["model"] = serde_json::Value::String(m.clone());
    }
    if let Some(a) = &task.agent {
        obj["agent"] = serde_json::Value::String(a.clone());
    }
    if let Some(exec) = &task.exec {
        obj["exec"] = serde_json::Value::String(exec.clone());
    }
    if let Some(c) = &task.created_at {
        obj["created_at"] = serde_json::Value::String(c.clone());
    }
    if let Some(s) = &task.started_at {
        obj["started_at"] = serde_json::Value::String(s.clone());
    }
    if let Some(c) = &task.completed_at {
        obj["completed_at"] = serde_json::Value::String(c.clone());
    }
    if let Some(reason) = &task.failure_reason {
        obj["failure_reason"] = serde_json::Value::String(reason.clone());
    }
    if let Some(d) = &task.description {
        let truncated = if d.chars().count() > 8000 {
            let mut s = d.chars().take(8000).collect::<String>();
            s.push('…');
            s
        } else {
            d.clone()
        };
        let rendered_html = markdown_to_html(&truncated);
        obj["description"] = serde_json::Value::String(truncated);
        obj["description_html"] = serde_json::Value::String(rendered_html);
    }
    if let Some(ev) = eval {
        obj["eval_score"] = serde_json::json!(ev.score);
        let dims: serde_json::Value = ev
            .dimensions
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::json!(v)))
            .collect::<serde_json::Map<_, _>>()
            .into();
        obj["eval_dims"] = dims;
    }
    if let Some(bundle) = msgs.filter(|b| !b.is_empty()) {
        obj["msg"] = task_messages_to_json(bundle);
    }
    obj
}

fn build_tasks_json(
    graph: &WorkGraph,
    included: &[&Task],
    evals: &HashMap<String, EvalSummary>,
    task_messages: &HashMap<String, TaskMessages>,
    included_ids: &HashSet<&str>,
) -> String {
    let map: serde_json::Map<String, serde_json::Value> = included
        .iter()
        .map(|t| {
            let eval = evals.get(&t.id);
            let msgs = task_messages.get(&t.id);
            (
                t.id.clone(),
                task_to_json(t, graph, eval, msgs, included_ids),
            )
        })
        .collect();
    let json_str = serde_json::to_string(&serde_json::Value::Object(map))
        .unwrap_or_else(|_| "{}".to_string());
    json_str.replace("</script>", "<\\/script>")
}

fn build_edges_json(reach: &EdgeReach) -> String {
    let mut map = serde_json::Map::new();
    let mut keys: BTreeSet<&String> = BTreeSet::new();
    keys.extend(reach.upstream.keys());
    keys.extend(reach.downstream.keys());
    for k in keys {
        let up: Vec<&String> = reach
            .upstream
            .get(k)
            .map(|s| s.iter().collect::<Vec<_>>())
            .unwrap_or_default();
        let down: Vec<&String> = reach
            .downstream
            .get(k)
            .map(|s| s.iter().collect::<Vec<_>>())
            .unwrap_or_default();
        map.insert(
            k.clone(),
            serde_json::json!({
                "up": up,
                "down": down,
            }),
        );
    }
    serde_json::to_string(&serde_json::Value::Object(map))
        .unwrap_or_else(|_| "{}".to_string())
        .replace("</script>", "<\\/script>")
}

fn build_cycles_json(viz: &VizJson) -> String {
    serde_json::to_string(&viz.cycle_members)
        .unwrap_or_else(|_| "{}".to_string())
        .replace("</script>", "<\\/script>")
}

// ────────────────────────────────────────────────────────────────────────────
// Page render
// ────────────────────────────────────────────────────────────────────────────

/// Build the rich legend HTML shown in the side panel when the user clicks
/// the **Legend** button. Covers edge colors, status colors, click
/// behaviors, theme toggle, and other CLI affordances. This is the single
/// source of UI explanation — the page header stays clean.
fn render_legend_panel() -> String {
    let statuses = [
        Status::Open,
        Status::InProgress,
        Status::Done,
        Status::Failed,
        Status::Blocked,
        Status::Waiting,
        Status::PendingValidation,
        Status::PendingEval,
        Status::FailedPendingEval,
        Status::Abandoned,
        Status::Incomplete,
    ];
    let mut s = String::new();
    s.push_str("<div class=\"panel-legend\">\n");
    s.push_str("<div class=\"panel-header\"><code class=\"panel-id\">Legend</code></div>\n");
    s.push_str("<p class=\"panel-title\">Visual conventions and interactions</p>\n");

    // Edge colors — pulled from CSS variables so dark/light themes track the
    // same swatches used in the actual viz.
    s.push_str("<details open><summary>Edge colors</summary>\n");
    s.push_str("<ul class=\"legend legend-edges\">\n");
    s.push_str("<li><span class=\"swatch\" style=\"background:var(--edge-upstream)\"></span>magenta — upstream dependencies (what this task waits on)</li>\n");
    s.push_str("<li><span class=\"swatch\" style=\"background:var(--edge-downstream)\"></span>cyan — downstream consumers (what waits on this task)</li>\n");
    s.push_str("<li><span class=\"swatch\" style=\"background:var(--edge-cycle)\"></span>yellow — cycle membership (back-edges in the same loop)</li>\n");
    s.push_str("</ul>\n</details>\n");

    // Status colors — same palette as the TUI.
    s.push_str("<details open><summary>Status colors</summary>\n");
    s.push_str("<ul class=\"legend legend-statuses\">\n");
    for st in statuses {
        s.push_str(&format!(
            "  <li><span class=\"swatch\" style=\"background:{color}\"></span>{name}</li>\n",
            color = status_color(st),
            name = st,
        ));
    }
    s.push_str("</ul>\n</details>\n");

    // Click behaviors.
    s.push_str("<details open><summary>Interactions</summary>\n");
    s.push_str("<ul class=\"legend-text\">\n");
    s.push_str("<li>Click any task id or status glyph to open its detail panel.</li>\n");
    s.push_str("<li>Click links inside the panel to navigate between related tasks.</li>\n");
    s.push_str("<li>Press <kbd>Esc</kbd> or click outside the panel to close it.</li>\n");
    s.push_str("<li>Use the <strong>theme toggle</strong> in the page header to switch dark/light.</li>\n");
    s.push_str("</ul>\n</details>\n");

    // Other CLI affordances — hint at flags users may not know about.
    s.push_str("<details><summary>CLI flags</summary>\n");
    s.push_str("<ul class=\"legend-text\">\n");
    s.push_str("<li><code>wg html --chat</code> — include rendered chat transcripts in task pages.</li>\n");
    s.push_str("<li><code>wg html --since 24h</code> — filter to recent tasks (e.g. <code>1h</code>, <code>7d</code>).</li>\n");
    s.push_str("<li><code>wg html --all</code> — include non-public tasks (default is public visibility).</li>\n");
    s.push_str("</ul>\n</details>\n");

    s.push_str("</div>\n");
    s
}

fn render_footer(
    total_in_graph: usize,
    total_shown: usize,
    show_all: bool,
    since_label: Option<&str>,
) -> String {
    let now = chrono::Utc::now().to_rfc3339();
    let filter_note = if show_all {
        format!(
            "Showing {} of {} tasks{}.",
            total_shown,
            total_in_graph,
            since_label
                .map(|s| format!(", last {}", s))
                .unwrap_or_default(),
        )
    } else {
        let hidden = total_in_graph.saturating_sub(total_shown);
        if let Some(label) = since_label {
            format!(
                "Showing {} of {} tasks: <strong>--public-only</strong>, last {}. {} non-public tasks hidden.",
                total_shown, total_in_graph, label, hidden,
            )
        } else {
            format!(
                "Visibility filter: <strong>--public-only</strong>. Showing {} of {} tasks; \
                 {} non-public tasks hidden.",
                total_shown, total_in_graph, hidden,
            )
        }
    };
    format!(
        "<p>{filter}</p>\n\
         <p class=\"meta\">Rendered by <code>wg html</code> at {now}.</p>\n",
        filter = filter_note,
        now = now,
    )
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
fn render_index(
    graph: &WorkGraph,
    included: &[&Task],
    included_ids: &HashSet<&str>,
    viz: &VizJson,
    viz_agency: Option<&VizJson>,
    tasks_json: &str,
    edges_json: &str,
    cycles_json: &str,
    task_messages: &HashMap<String, TaskMessages>,
    show_all: bool,
    since_label: Option<&str>,
    include_chat: bool,
    chat_transcripts_shown: usize,
    chat_transcripts_hidden: usize,
    has_agency_tasks: bool,
    project_meta: &ProjectMeta,
) -> String {
    let total_in_graph = graph.tasks().count();
    let total_shown = included.len();

    // When agency tasks exist we render two viz blocks (substantive vs.
    // agency-included) and CSS shows one based on body[data-show-agency]. When
    // there are no agency tasks the toggle is omitted and we only emit the
    // single substantive viz with no extra class.
    let viz_html = if has_agency_tasks {
        let mut combined = render_viz_html(viz, graph, included_ids, "viz-substantive");
        if let Some(va) = viz_agency {
            combined.push('\n');
            combined.push_str(&render_viz_html(va, graph, included_ids, "viz-agency"));
        }
        combined
    } else {
        render_viz_html(viz, graph, included_ids, "")
    };

    // Ordered task list (by status then id). Agency entries get a data-agency
    // marker so CSS can hide them by default and dim them when the toggle is on.
    let mut ordered: Vec<&&Task> = included.iter().collect();
    ordered.sort_by_key(|t| (t.status.to_string(), t.id.clone()));
    let mut list = String::new();
    list.push_str("<ul class=\"task-list\">\n");
    for t in &ordered {
        let agency = is_agency_task(&t.id);
        let msg_bundle = task_messages.get(&t.id);
        let has_msg = msg_bundle.map(|b| !b.is_empty()).unwrap_or(false);
        let unread = msg_bundle.map(|b| b.has_unread).unwrap_or(false);
        let mut li_classes: Vec<&str> = Vec::new();
        if agency {
            li_classes.push("is-agency");
        }
        if has_msg {
            li_classes.push("has-msg");
        }
        if unread {
            li_classes.push("has-unread-msg");
        }
        let li_attrs = if li_classes.is_empty() {
            String::new()
        } else if agency {
            // Preserve the legacy `data-agency="true"` so existing CSS / tests
            // that rely on the attribute keep working.
            format!(" class=\"{}\" data-agency=\"true\"", li_classes.join(" "))
        } else {
            format!(" class=\"{}\"", li_classes.join(" "))
        };
        let msg_inline = render_msg_indicator_inline(msg_bundle, &t.id);
        list.push_str(&format!(
            "  <li{li_attrs}><a href=\"{href}\" data-task-id=\"{id_attr}\"><span class=\"badge {cls}\">{status}</span> \
             <code>{id}</code>{msg_inline} — {title}</a></li>\n",
            li_attrs = li_attrs,
            href = task_filename(&t.id),
            id_attr = escape_html(&t.id),
            cls = status_class(t.status),
            status = t.status,
            id = escape_html(&t.id),
            msg_inline = msg_inline,
            title = escape_html(&t.title),
        ));
    }
    list.push_str("</ul>\n");

    let legend_panel = render_legend_panel();
    let footer = render_footer(total_in_graph, total_shown, show_all, since_label);

    // Header chat banner — only when --chat is active. Tells the user how many
    // transcripts were rendered (and how many were filtered out by visibility),
    // so a `wg html --chat` against an all-internal graph doesn't look broken.
    let chat_banner = if include_chat {
        if chat_transcripts_shown == 0 && chat_transcripts_hidden > 0 {
            format!(
                "<p class=\"chat-banner\">Showing 0 chat transcripts \
                 ({} hidden by visibility — pass <code>--all</code> to include them).</p>\n",
                chat_transcripts_hidden,
            )
        } else if chat_transcripts_hidden > 0 {
            format!(
                "<p class=\"chat-banner\">Showing {} chat transcript{} \
                 ({} omitted by visibility).</p>\n",
                chat_transcripts_shown,
                if chat_transcripts_shown == 1 { "" } else { "s" },
                chat_transcripts_hidden,
            )
        } else {
            format!(
                "<p class=\"chat-banner\">Showing {} chat transcript{}.</p>\n",
                chat_transcripts_shown,
                if chat_transcripts_shown == 1 { "" } else { "s" },
            )
        }
    } else {
        String::new()
    };

    let title_suffix = if show_all { "all tasks" } else { "public mirror" };

    // Project-header block (above the dependency graph). Omitted entirely
    // when no fields are set — we don't want a useless empty block sitting
    // above the viz. The abstract is rendered as markdown and collapses
    // behind a "show more" toggle when the body is taller than ~5 lines.
    let project_header_html = render_project_header(project_meta);
    // Browser title prefers the project title when set; otherwise falls
    // back to the legacy "Workgraph — <suffix>" form so existing fixtures
    // keep matching.
    let browser_title = match project_meta.title.as_deref() {
        Some(t) if !t.trim().is_empty() => format!("{} — Workgraph", t.trim()),
        _ => format!("Workgraph — {}", title_suffix),
    };

    // Agency toggle UI (button + body data attribute) — emitted only when
    // there are actual agency tasks in scope. Web equivalent of the TUI's
    // period-key behavior. Default is hidden ("false"); the bootstrap script
    // overrides the body attribute before paint to avoid a flash if the user
    // has already opted in.
    let agency_toggle_btn = if has_agency_tasks {
        "<button id=\"agency-toggle\" class=\"agency-toggle\" type=\"button\" \
         aria-pressed=\"false\" title=\"Toggle agency / meta tasks (.evaluate-, .assign-, .flip-)\">\
         Show meta tasks</button>\n"
    } else {
        ""
    };
    let agency_bootstrap = if has_agency_tasks {
        "/* Agency-toggle bootstrap — runs before paint to avoid a flash. */\n\
         (function () {\n\
             try {\n\
                 var saved = localStorage.getItem('wg-html-show-agency');\n\
                 if (saved === 'true') {\n\
                     document.documentElement.setAttribute('data-show-agency', 'true');\n\
                 }\n\
             } catch (_) {}\n\
         })();\n"
    } else {
        ""
    };
    // The default state is recorded directly on the <html> element so
    // (a) the bootstrap can override it before paint when the user previously
    // opted in, and (b) CSS can target `:root[data-show-agency='true']` /
    // `:root[data-show-agency='false']` without needing JS to wait for body.
    let html_show_agency = if has_agency_tasks {
        " data-show-agency=\"false\""
    } else {
        ""
    };

    format!(
        "<!DOCTYPE html>\n\
         <html lang=\"en\"{html_show_agency}>\n\
         <head>\n\
         <meta charset=\"utf-8\" />\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n\
         <title>{browser_title}</title>\n\
         <link rel=\"stylesheet\" href=\"style.css\" />\n\
         <script>\n\
         /* Theme bootstrap — runs before paint to avoid a flash. */\n\
         (function () {{\n\
             try {{\n\
                 var saved = localStorage.getItem('wg-html-theme');\n\
                 if (saved === 'dark' || saved === 'light') {{\n\
                     document.documentElement.setAttribute('data-theme', saved);\n\
                 }}\n\
             }} catch (_) {{}}\n\
         }})();\n\
         {agency_bootstrap}\
         </script>\n\
         </head>\n\
         <body>\n\
         <header class=\"page-header\">\n\
         <div>\n\
         <h1>Workgraph</h1>\n\
         <p class=\"subtitle\">{n} tasks shown</p>\n\
         {chat_banner}\
         </div>\n\
         <div class=\"header-controls\">\n\
         {agency_toggle_btn}\
         <button id=\"legend-toggle\" class=\"legend-toggle\" type=\"button\" aria-label=\"Show legend\">Legend</button>\n\
         <button id=\"theme-toggle\" class=\"theme-toggle\" type=\"button\">Light theme</button>\n\
         </div>\n\
         </header>\n\
         {project_header_html}\
         <div class=\"page-layout\">\n\
         <main class=\"main-content\">\n\
         <section class=\"dag-section\">\n\
         <div class=\"viz-wrap\">{viz}</div>\n\
         </section>\n\
         <section class=\"list-section\">\n\
         <h2>Tasks ({total_shown})</h2>\n\
         {list}\n\
         </section>\n\
         </main>\n\
         <aside id=\"side-panel\" class=\"side-panel\" aria-label=\"Task detail\">\n\
         <div id=\"panel-resize-handle\" class=\"panel-resize-handle\" role=\"separator\" aria-orientation=\"vertical\" aria-label=\"Resize detail panel\" tabindex=\"-1\"></div>\n\
         <button id=\"panel-close\" class=\"panel-close\" type=\"button\" aria-label=\"Close detail panel\">×</button>\n\
         <div id=\"panel-content\"></div>\n\
         </aside>\n\
         </div>\n\
         <template id=\"wg-legend-template\">{legend_panel}</template>\n\
         <footer>{footer}</footer>\n\
         <script id=\"wg-tasks-json\">window.WG_TASKS = {tasks_json};</script>\n\
         <script id=\"wg-edges-json\">window.WG_EDGES = {edges_json};</script>\n\
         <script id=\"wg-cycles-json\">window.WG_CYCLES = {cycles_json};</script>\n\
         <script src=\"panel.js\"></script>\n\
         </body>\n\
         </html>\n",
        browser_title = escape_html(&browser_title),
        n = total_shown,
        viz = viz_html,
        legend_panel = legend_panel,
        list = list,
        total_shown = total_shown,
        footer = footer,
        chat_banner = chat_banner,
        tasks_json = tasks_json,
        edges_json = edges_json,
        cycles_json = cycles_json,
        agency_toggle_btn = agency_toggle_btn,
        agency_bootstrap = agency_bootstrap,
        html_show_agency = html_show_agency,
        project_header_html = project_header_html,
    )
}

/// Render the project-header block. Returns an empty string when
/// `meta.is_empty()` so the page falls back to the existing minimal
/// "Workgraph / N tasks shown" header.
fn render_project_header(meta: &ProjectMeta) -> String {
    if meta.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    out.push_str("<header class=\"project-header\">\n");
    if let Some(t) = meta.title.as_deref() {
        let t = t.trim();
        if !t.is_empty() {
            out.push_str(&format!(
                "<h1 class=\"project-title\">{}</h1>\n",
                escape_html(t)
            ));
        }
    }
    if let Some(b) = meta.byline.as_deref() {
        let b = b.trim();
        if !b.is_empty() {
            out.push_str(&format!(
                "<p class=\"project-byline\">{}</p>\n",
                escape_html(b)
            ));
        }
    }
    if let Some(a) = meta.abstract_md.as_deref() {
        let a = a.trim();
        if !a.is_empty() {
            // Render markdown to HTML using the same renderer as the
            // task description body (pulldown-cmark, no raw HTML).
            // The wrapper carries `data-abstract-len` (line count) so
            // the JS can decide whether to install the "show more"
            // collapser without re-counting in CSS.
            let line_count = a.lines().count();
            let rendered = markdown_to_html(a);
            out.push_str(&format!(
                "<div class=\"project-abstract description-rendered\" data-abstract-lines=\"{}\">{}</div>\n",
                line_count, rendered,
            ));
        }
    }
    out.push_str("</header>\n");
    out
}

// ────────────────────────────────────────────────────────────────────────────
// Per-task page (deep-link target)
// ────────────────────────────────────────────────────────────────────────────

fn render_task_page(
    task: &Task,
    graph: &WorkGraph,
    included_ids: &HashSet<&str>,
    eval: Option<&EvalSummary>,
    chat_block: Option<&str>,
    messages_block: Option<&str>,
) -> String {
    let title = escape_html(&task.title);
    let id = escape_html(&task.id);
    let status_str = task.status.to_string();
    let status_cls = status_class(task.status);

    let mut deps_html = String::from("<ul class=\"deps\">");
    let mut hidden_dep_count = 0usize;
    if task.after.is_empty() {
        deps_html.push_str("<li class=\"none\">(none)</li>");
    } else {
        for dep in &task.after {
            if included_ids.contains(dep.as_str()) {
                let dep_status = graph
                    .get_task(dep)
                    .map(|t| t.status)
                    .unwrap_or(Status::Open);
                deps_html.push_str(&format!(
                    "<li><a href=\"{href}\"><span class=\"badge {cls}\">{st}</span> <code>{id}</code></a></li>",
                    href = format!("./{}.html", url_encode_id(dep)),
                    cls = status_class(dep_status),
                    st = dep_status,
                    id = escape_html(dep),
                ));
            } else {
                hidden_dep_count += 1;
            }
        }
        if hidden_dep_count > 0 {
            deps_html.push_str(&format!(
                "<li class=\"hidden-dep\"><span class=\"note\">{} non-public dependenc{} hidden</span></li>",
                hidden_dep_count,
                if hidden_dep_count == 1 { "y" } else { "ies" },
            ));
        }
    }
    deps_html.push_str("</ul>");

    let dependents: Vec<&str> = {
        let mut v: Vec<&str> = graph
            .tasks()
            .filter(|t| t.after.iter().any(|a| a == &task.id))
            .map(|t| t.id.as_str())
            .collect();
        v.sort();
        v
    };

    let mut dependents_html = String::from("<ul class=\"deps\">");
    let mut hidden_dependents_count = 0usize;
    if dependents.is_empty() {
        dependents_html.push_str("<li class=\"none\">(none)</li>");
    } else {
        for d in dependents {
            if included_ids.contains(d) {
                let dep_status = graph.get_task(d).map(|t| t.status).unwrap_or(Status::Open);
                dependents_html.push_str(&format!(
                    "<li><a href=\"{href}\"><span class=\"badge {cls}\">{st}</span> <code>{id}</code></a></li>",
                    href = format!("./{}.html", url_encode_id(d)),
                    cls = status_class(dep_status),
                    st = dep_status,
                    id = escape_html(d),
                ));
            } else {
                hidden_dependents_count += 1;
            }
        }
        if hidden_dependents_count > 0 {
            dependents_html.push_str(&format!(
                "<li class=\"hidden-dep\"><span class=\"note\">{} non-public dependent{} hidden</span></li>",
                hidden_dependents_count,
                if hidden_dependents_count == 1 { "" } else { "s" },
            ));
        }
    }
    dependents_html.push_str("</ul>");

    let description_html = match &task.description {
        Some(d) if !d.trim().is_empty() => {
            let rendered = markdown_to_html(d);
            format!(
                "<div class=\"desc-header\">\
                 <button id=\"desc-toggle\" class=\"desc-toggle\" type=\"button\">raw</button>\
                 </div>\
                 <div id=\"desc-pretty\" class=\"description-rendered\">{rendered}</div>\
                 <pre id=\"desc-raw\" class=\"description\" style=\"display:none\">{raw}</pre>\
                 <script>\
                 (function(){{\
                 var KEY='wg-html-desc-view';\
                 var btn=document.getElementById('desc-toggle');\
                 var pretty=document.getElementById('desc-pretty');\
                 var raw=document.getElementById('desc-raw');\
                 function apply(v){{\
                 if(v==='raw'){{\
                 pretty.style.display='none';\
                 raw.style.display='';\
                 btn.textContent='pretty';\
                 }}else{{\
                 pretty.style.display='';\
                 raw.style.display='none';\
                 btn.textContent='raw';\
                 }}\
                 }}\
                 var saved;try{{saved=localStorage.getItem(KEY);}}catch(_){{saved=null;}}\
                 apply(saved==='raw'?'raw':'pretty');\
                 btn.addEventListener('click',function(){{\
                 var cur=pretty.style.display==='none'?'raw':'pretty';\
                 var nxt=cur==='pretty'?'raw':'pretty';\
                 try{{localStorage.setItem(KEY,nxt);}}catch(_){{}}\
                 apply(nxt);\
                 }});\
                 }})();\
                 </script>",
                rendered = rendered,
                raw = escape_html(d),
            )
        }
        _ => "<p class=\"none\">(no description)</p>".to_string(),
    };

    let mut meta_rows: Vec<(String, String)> = Vec::new();
    meta_rows.push((
        "Status".into(),
        format!("<span class=\"badge {}\">{}</span>", status_cls, status_str),
    ));
    if let Some(a) = &task.assigned {
        meta_rows.push(("Assigned".into(), format!("<code>{}</code>", escape_html(a))));
    }
    if let Some(agent) = &task.agent {
        meta_rows.push((
            "Agent identity".into(),
            format!("<code>{}</code>", escape_html(agent)),
        ));
    }
    if let Some(model) = &task.model {
        meta_rows.push((
            "Model".into(),
            format!("<code>{}</code>", escape_html(model)),
        ));
    }
    if let Some(c) = &task.created_at {
        meta_rows.push(("Created".into(), escape_html(c)));
    }
    if let Some(s) = &task.started_at {
        meta_rows.push(("Started".into(), escape_html(s)));
    }
    if let Some(c) = &task.completed_at {
        meta_rows.push(("Completed".into(), escape_html(c)));
    }
    if !task.tags.is_empty() {
        let tags = task
            .tags
            .iter()
            .map(|t| format!("<code>{}</code>", escape_html(t)))
            .collect::<Vec<_>>()
            .join(", ");
        meta_rows.push(("Tags".into(), tags));
    }
    if let Some(usage) = &task.token_usage {
        meta_rows.push((
            "Tokens".into(),
            format!("{} in / {} out", usage.total_input(), usage.output_tokens),
        ));
    }
    if let Some(reason) = &task.failure_reason {
        meta_rows.push(("Failure reason".into(), escape_html(reason)));
    }
    if let Some(ev) = eval {
        meta_rows.push(("Eval score".into(), format!("{:.2}", ev.score)));
        for (dim, val) in &ev.dimensions {
            meta_rows.push((
                format!("  └ {}", dim.replace('_', " ")),
                format!("{:.2}", val),
            ));
        }
    }

    let mut meta_html = String::from("<table class=\"meta-table\"><tbody>");
    for (k, v) in meta_rows {
        meta_html.push_str(&format!("<tr><th>{}</th><td>{}</td></tr>", k, v));
    }
    meta_html.push_str("</tbody></table>");

    // Log entries (last 50 to give more context than v1's 30)
    let log_html = if task.log.is_empty() {
        "<p class=\"none\">(no log entries)</p>".to_string()
    } else {
        let mut s = String::from("<ul class=\"task-log\">");
        for entry in task.log.iter().rev().take(50).rev() {
            let ts = escape_html(&entry.timestamp);
            let msg = escape_html(&entry.message);
            s.push_str(&format!(
                "<li><span class=\"log-ts\">{ts}</span> {msg}</li>"
            ));
        }
        s.push_str("</ul>");
        s
    };

    format!(
        "<!DOCTYPE html>\n\
         <html lang=\"en\">\n\
         <head>\n\
         <meta charset=\"utf-8\" />\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n\
         <title>{id} — Workgraph</title>\n\
         <link rel=\"stylesheet\" href=\"../style.css\" />\n\
         <script>\n\
         (function () {{\n\
             try {{\n\
                 var saved = localStorage.getItem('wg-html-theme');\n\
                 if (saved === 'dark' || saved === 'light') {{\n\
                     document.documentElement.setAttribute('data-theme', saved);\n\
                 }}\n\
             }} catch (_) {{}}\n\
         }})();\n\
         </script>\n\
         </head>\n\
         <body class=\"task-page\">\n\
         <header>\n\
         <p class=\"breadcrumb\"><a href=\"../index.html\">← graph</a></p>\n\
         <h1><code>{id}</code></h1>\n\
         <p class=\"task-title\">{title}</p>\n\
         </header>\n\
         <main>\n\
         <section><h2>Metadata</h2>{meta}</section>\n\
         <section><h2>Description</h2>{desc}</section>\n\
         <section><h2>Depends on</h2>{deps}</section>\n\
         <section><h2>Required by</h2>{revdeps}</section>\n\
         {messages}\
         {chat}\
         <section><h2>Log</h2>{log}</section>\n\
         </main>\n\
         <footer><p class=\"meta\">Visibility = <code>{vis}</code></p></footer>\n\
         </body>\n\
         </html>\n",
        id = id,
        title = title,
        meta = meta_html,
        desc = description_html,
        deps = deps_html,
        revdeps = dependents_html,
        messages = messages_block.unwrap_or(""),
        chat = chat_block.unwrap_or(""),
        log = log_html,
        vis = escape_html(&task.visibility),
    )
}

// ────────────────────────────────────────────────────────────────────────────
// Chat transcript rendering (`--chat` flag)
// ────────────────────────────────────────────────────────────────────────────

/// Decision for whether and how to render a task's chat transcript.
enum ChatRender {
    /// Render the transcript using the given chat session reference (alias).
    Render(String),
    /// Task is a chat task but transcript is filtered out by visibility —
    /// emit a hidden-marker line instead. Carries the visibility string for
    /// the user-facing message.
    HiddenByVisibility(String),
    /// Task is not a chat task, or `--chat` was not requested. No section
    /// emitted at all.
    None,
}

fn decide_chat_render(task: &Task, include_chat: bool, all_chats: bool) -> ChatRender {
    if !include_chat {
        return ChatRender::None;
    }
    if !chat_id::is_chat_task_id(&task.id) {
        return ChatRender::None;
    }
    let vis = task.visibility.as_str();
    if all_chats || vis == "public" {
        // The chat session reference is the task id minus its leading `.`
        // (the chat sessions registry stores aliases without the prefix —
        // e.g. task `.chat-12` maps to chat ref `chat-12`).
        let session_ref = task.id.trim_start_matches('.').to_string();
        ChatRender::Render(session_ref)
    } else {
        ChatRender::HiddenByVisibility(vis.to_string())
    }
}

/// Read both inbox (user messages) and outbox (agent responses) for a chat
/// session and return them in chronological order.
fn load_chat_messages(workgraph_dir: &Path, session_ref: &str) -> Vec<ChatMessage> {
    let inbox = chat::read_inbox_ref(workgraph_dir, session_ref).unwrap_or_default();
    let outbox = chat::read_outbox_since_ref(workgraph_dir, session_ref, 0).unwrap_or_default();
    let mut all: Vec<ChatMessage> = inbox.into_iter().chain(outbox.into_iter()).collect();
    all.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    all
}

/// Best-effort secret-redaction patterns. Documented for transparency:
/// this is NOT a security guarantee — it is a safety net against accidental
/// shell-history leaks (api-key-shaped strings, env-var assignments, paths
/// to the secrets dir). Anyone publishing a transcript SHOULD review it
/// manually before publishing.
///
/// Patterns:
///   1. `sk-[A-Za-z0-9]{20,}` — OpenAI / Anthropic / generic API-key shape.
///   2. `(?i)(OPENAI_API_KEY|ANTHROPIC_API_KEY|GITHUB_TOKEN|SECRET_KEY|ACCESS_TOKEN|API_KEY|AUTH_TOKEN|BEARER_TOKEN|PASSWORD|PASSWD)\s*=\s*\S+`
///      — env-var assignments and shell `KEY=value` exports.
///   3. paths under `~/.wg/secrets` (or `/home/<user>/.wg/secrets`,
///      `/Users/<user>/.wg/secrets`, `$HOME/.wg/secrets`).
fn sanitize_transcript(s: &str) -> String {
    static API_KEY: OnceLock<Regex> = OnceLock::new();
    static ENV_KEY: OnceLock<Regex> = OnceLock::new();
    static SECRET_PATH: OnceLock<Regex> = OnceLock::new();

    let api_key = API_KEY.get_or_init(|| {
        Regex::new(r"sk-[A-Za-z0-9]{20,}").expect("api-key regex")
    });
    let env_key = ENV_KEY.get_or_init(|| {
        Regex::new(
            r"(?i)\b(OPENAI_API_KEY|ANTHROPIC_API_KEY|GITHUB_TOKEN|SECRET_KEY|ACCESS_TOKEN|API_KEY|AUTH_TOKEN|BEARER_TOKEN|PASSWORD|PASSWD)\s*=\s*\S+",
        )
        .expect("env-key regex")
    });
    let secret_path = SECRET_PATH.get_or_init(|| {
        Regex::new(r"(?:~|\$HOME|/home/[^/\s]+|/Users/[^/\s]+)/\.wg/secrets[^\s]*")
            .expect("secret-path regex")
    });

    let s = api_key.replace_all(s, "[redacted]");
    let s = env_key.replace_all(&s, "[redacted]");
    let s = secret_path.replace_all(&s, "[redacted]");
    s.into_owned()
}

/// Render a 'Conversation' section containing every message in the chat,
/// in order, with role + timestamp + sanitized content. Code fences are
/// preserved verbatim (escaped) inside `<pre class="chat-code">`.
fn render_conversation_block(messages: &[ChatMessage]) -> String {
    let mut s = String::new();
    s.push_str("<section class=\"chat-conversation\"><h2>Conversation</h2>\n");
    if messages.is_empty() {
        s.push_str("<p class=\"none\">(empty transcript)</p>\n</section>\n");
        return s;
    }
    s.push_str(&format!(
        "<p class=\"chat-meta\">{} message{} in transcript.</p>\n",
        messages.len(),
        if messages.len() == 1 { "" } else { "s" },
    ));
    s.push_str("<ol class=\"chat-messages\">\n");
    for msg in messages {
        let role_cls = match msg.role.as_str() {
            "user" => "chat-msg-user",
            "coordinator" | "assistant" | "agent" => "chat-msg-agent",
            _ => "chat-msg-other",
        };
        s.push_str(&format!(
            "<li class=\"chat-msg {role_cls}\">\
             <div class=\"chat-msg-head\">\
             <span class=\"chat-role\">{role}</span> \
             <span class=\"chat-ts\">{ts}</span>\
             </div>\
             {body}\
             </li>\n",
            role_cls = role_cls,
            role = escape_html(&msg.role),
            ts = escape_html(&msg.timestamp),
            body = render_chat_body(&msg.content),
        ));
    }
    s.push_str("</ol>\n</section>\n");
    s
}

/// Convert the message body to safe HTML. Triple-backtick fenced code blocks
/// become `<pre class="chat-code">` blocks (no syntax highlighting at this
/// stage — same treatment as task descriptions); other text is wrapped in
/// `<pre class="chat-text">` to preserve linebreaks. All content is run
/// through `sanitize_transcript` first.
fn render_chat_body(content: &str) -> String {
    let sanitized = sanitize_transcript(content);
    // Light fenced-code block detection: split on lines beginning with ```.
    // Even if the message has no code fences this still produces correct
    // output (a single text block).
    let mut out = String::new();
    let mut in_code = false;
    let mut buf = String::new();
    for line in sanitized.split_inclusive('\n') {
        if line.trim_end_matches('\n').trim_start().starts_with("```") {
            // Flush current buffer.
            if !buf.is_empty() {
                out.push_str(&format_chat_block(&buf, in_code));
                buf.clear();
            }
            in_code = !in_code;
            continue;
        }
        buf.push_str(line);
    }
    if !buf.is_empty() {
        out.push_str(&format_chat_block(&buf, in_code));
    }
    out
}

fn format_chat_block(text: &str, is_code: bool) -> String {
    let trimmed = text.trim_matches('\n');
    if trimmed.is_empty() {
        return String::new();
    }
    if is_code {
        format!(
            "<pre class=\"chat-code\">{}</pre>\n",
            escape_html(trimmed)
        )
    } else {
        format!(
            "<pre class=\"chat-text\">{}</pre>\n",
            escape_html(trimmed)
        )
    }
}

fn render_chat_hidden_notice(visibility: &str) -> String {
    format!(
        "<section class=\"chat-conversation chat-hidden\"><h2>Conversation</h2>\n\
         <p class=\"chat-hidden-note\">Chat transcript hidden (visibility: {}). \
         Use <code>--all</code> to include.</p>\n\
         </section>\n",
        escape_html(visibility),
    )
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_csi() {
        let s = "\x1b[31mhello\x1b[0m world";
        assert_eq!(strip_ansi(s), "hello world");
    }

    #[test]
    fn strip_ansi_preserves_unicode() {
        let s = "\x1b[36m├→\x1b[0m foo";
        assert_eq!(strip_ansi(s), "├→ foo");
    }

    #[test]
    fn extend_through_status_decorator_works() {
        let line: Vec<char> = "task-x  (in-progress · 5m) more".chars().collect();
        let after_id = "task-x".chars().count();
        let end = extend_through_status_decorator(&line, after_id);
        let consumed: String = line[after_id..end].iter().collect();
        assert!(consumed.starts_with("  ("));
        assert!(consumed.ends_with(')'));
    }

    #[test]
    fn extend_through_status_decorator_no_match_returns_original() {
        let line: Vec<char> = "task-x more text".chars().collect();
        let after_id = "task-x".chars().count();
        let end = extend_through_status_decorator(&line, after_id);
        assert_eq!(end, after_id);
    }

    #[test]
    fn parse_since_basic() {
        assert_eq!(parse_since("1h").unwrap(), Duration::hours(1));
        assert_eq!(parse_since("24h").unwrap(), Duration::hours(24));
        assert_eq!(parse_since("7d").unwrap(), Duration::days(7));
        assert_eq!(parse_since("2w").unwrap(), Duration::weeks(2));
        assert!(parse_since("0h").is_err());
        assert!(parse_since("abc").is_err());
    }

    // ────────────────────────────────────────────────────────────────────
    // Chat agent color (smoke-polish-wg-html)
    // ────────────────────────────────────────────────────────────────────

    #[test]
    fn chat_task_span_gets_chat_agent_class() {
        // render_line must emit class="task-link chat-agent" for .chat-N ids
        let mut out = String::new();
        let ranges: Vec<(usize, usize, usize, &str, Status)> =
            vec![(0, 7, 7, ".chat-1", Status::Open)];
        let line_chars: Vec<char> = ".chat-1".chars().collect();
        let edges: HashMap<(usize, usize), Vec<(String, String)>> = HashMap::new();
        render_line(&mut out, 0, &line_chars, &ranges, &[], &edges, ".chat-1");
        assert!(
            out.contains("task-link chat-agent"),
            "chat task span must have 'chat-agent' class; got: {out}"
        );
    }

    #[test]
    fn regular_task_span_no_chat_agent_class() {
        let mut out = String::new();
        let ranges: Vec<(usize, usize, usize, &str, Status)> =
            vec![(0, 7, 7, "my-task", Status::Open)];
        let line_chars: Vec<char> = "my-task".chars().collect();
        let edges: HashMap<(usize, usize), Vec<(String, String)>> = HashMap::new();
        render_line(&mut out, 0, &line_chars, &ranges, &[], &edges, "my-task");
        assert!(
            !out.contains("chat-agent"),
            "regular task span must NOT have 'chat-agent' class; got: {out}"
        );
    }

    // ────────────────────────────────────────────────────────────────────
    // Decorator + time-suffix coloring (fix-task-list)
    // ────────────────────────────────────────────────────────────────────

    #[test]
    fn decorator_parens_get_decorator_class() {
        // Whole `id  (status · ...)` block is one task-link span; the
        // parens portion is wrapped in a nested `.decorator` so CSS can
        // colour it white instead of inheriting the status hue.
        let mut out = String::new();
        let line = "verify-end-to  (done · →44k ←6.6k ◎1.3M §8.7k) 1d";
        let line_chars: Vec<char> = line.chars().collect();
        let id_chars = "verify-end-to".chars().count();
        let end = extend_through_status_decorator(&line_chars, id_chars);
        let ranges: Vec<(usize, usize, usize, &str, Status)> =
            vec![(0, id_chars, end, "verify-end-to", Status::Done)];
        let time = find_time_suffix_after(&line_chars, end).expect("time suffix");
        let edges: HashMap<(usize, usize), Vec<(String, String)>> = HashMap::new();
        render_line(&mut out, 0, &line_chars, &ranges, &[time], &edges, line);
        assert!(
            out.contains("<span class=\"decorator\">  (done"),
            "decorator span should wrap the parens block; got: {out}"
        );
        assert!(
            out.contains("<span class=\"time-suffix\"> 1d</span>"),
            "time suffix span should wrap ` 1d`; got: {out}"
        );
    }

    #[test]
    fn time_suffix_detection_units() {
        // Each compact unit produced by format_duration must be detected.
        for unit_str in ["5s", "12m", "3h", "1d", "2w", "4mo", "1y"] {
            let line = format!("task-x  (done) {}", unit_str);
            let line_chars: Vec<char> = line.chars().collect();
            let id_end = "task-x".chars().count();
            let decor_end = extend_through_status_decorator(&line_chars, id_end);
            let time = find_time_suffix_after(&line_chars, decor_end);
            assert!(
                time.is_some(),
                "time suffix `{unit_str}` should be detected in `{line}`"
            );
            let (start, end) = time.unwrap();
            // Range starts at the leading space and consumes the whole unit.
            assert_eq!(line_chars[start], ' ', "range starts with space");
            let captured: String = line_chars[start + 1..end].iter().collect();
            assert_eq!(captured, unit_str);
        }
    }

    #[test]
    fn time_suffix_does_not_match_other_words() {
        // Words like `done` look like time tokens but must not match (the
        // boundary check rejects letters after the digit-unit prefix).
        let line: Vec<char> = "task-x  (open) phase".chars().collect();
        let id_end = "task-x".chars().count();
        let decor_end = extend_through_status_decorator(&line, id_end);
        assert!(find_time_suffix_after(&line, decor_end).is_none());
    }

    // ────────────────────────────────────────────────────────────────────
    // Markdown description rendering (smoke-polish-wg-html)
    // ────────────────────────────────────────────────────────────────────

    #[test]
    fn markdown_to_html_renders_headings_and_lists() {
        let md = "## Hello\n\n- item one\n- item two\n";
        let html = markdown_to_html(md);
        assert!(html.contains("<h2>"), "heading not rendered: {html}");
        assert!(html.contains("<li>"), "list items not rendered: {html}");
        assert!(!html.contains("## Hello"), "raw heading leaked into output: {html}");
    }

    #[test]
    fn markdown_to_html_renders_code_blocks() {
        let md = "```rust\nfn main() {}\n```\n";
        let html = markdown_to_html(md);
        assert!(html.contains("<code"), "code block not rendered: {html}");
        assert!(html.contains("fn main()"), "code content missing: {html}");
    }

    #[test]
    fn task_json_includes_description_html() {
        use crate::graph::WorkGraph;
        let mut task = Task::default();
        task.id = "test-task".to_string();
        task.description = Some("## Title\n\n- item\n".to_string());
        let graph = WorkGraph::new();
        let included: HashSet<&str> = std::iter::once("test-task").collect();
        let json = task_to_json(&task, &graph, None, None, &included);
        let desc_html = json.get("description_html").and_then(|v| v.as_str());
        assert!(desc_html.is_some(), "description_html field missing");
        let html = desc_html.unwrap();
        assert!(html.contains("<h2>"), "heading not in description_html: {html}");
        assert!(html.contains("<li>"), "list not in description_html: {html}");
    }

    // ────────────────────────────────────────────────────────────────────
    // Chat-transcript sanitizer
    // ────────────────────────────────────────────────────────────────────

    #[test]
    fn sanitizer_redacts_sk_api_key() {
        let s = "here is a key: sk-abcdefghijklmnopqrstuvwxyz1234 in text";
        let out = sanitize_transcript(s);
        assert!(!out.contains("sk-abcdefghijklmnopqrstuvwxyz1234"), "raw sk key leaked: {out}");
        assert!(out.contains("[redacted]"), "redaction marker missing: {out}");
    }

    #[test]
    fn sanitizer_redacts_env_var_assignments() {
        for sample in [
            "OPENAI_API_KEY=hunter2",
            "export ANTHROPIC_API_KEY=swordfish",
            "GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz12345678",
            "API_KEY=letmein",
            "PASSWORD=qwerty",
        ] {
            let out = sanitize_transcript(sample);
            assert!(out.contains("[redacted]"), "no redaction for {sample:?}: {out}");
            for secret in ["hunter2", "swordfish", "ghp_abcdefghijklmnopqrstuvwxyz12345678", "letmein", "qwerty"] {
                if sample.contains(secret) {
                    assert!(!out.contains(secret), "secret {secret:?} leaked from {sample:?}: {out}");
                }
            }
        }
    }

    #[test]
    fn sanitizer_redacts_paths_under_wg_secrets() {
        for sample in [
            "see ~/.wg/secrets/openai.key for the key",
            "stored at /home/erik/.wg/secrets/anthropic.token today",
            "macOS path: /Users/dev/.wg/secrets/github.pat",
            "shell: $HOME/.wg/secrets/bot.json",
        ] {
            let out = sanitize_transcript(sample);
            assert!(out.contains("[redacted]"), "no redaction for {sample:?}: {out}");
            assert!(!out.contains("/secrets/openai.key"), "openai.key path leaked: {out}");
            assert!(!out.contains("/secrets/anthropic.token"), "anthropic.token path leaked: {out}");
            assert!(!out.contains("/secrets/github.pat"), "github.pat path leaked: {out}");
            assert!(!out.contains("/secrets/bot.json"), "bot.json path leaked: {out}");
        }
    }

    #[test]
    fn sanitizer_does_not_touch_innocent_text() {
        let sample = "Plain text with no secrets — paths like /home/erik/.cargo/bin are fine.";
        assert_eq!(sanitize_transcript(sample), sample);
    }

    // ────────────────────────────────────────────────────────────────────
    // Chat-render decision
    // ────────────────────────────────────────────────────────────────────

    fn chat_task(id: &str, vis: &str) -> Task {
        let mut t = Task::default();
        t.id = id.to_string();
        t.visibility = vis.to_string();
        t
    }

    #[test]
    fn decide_chat_render_off_when_chat_disabled() {
        let t = chat_task(".chat-1", "public");
        assert!(matches!(decide_chat_render(&t, false, false), ChatRender::None));
    }

    #[test]
    fn decide_chat_render_off_for_non_chat_tasks() {
        let mut t = Task::default();
        t.id = "regular-task".into();
        t.visibility = "public".into();
        assert!(matches!(decide_chat_render(&t, true, true), ChatRender::None));
    }

    #[test]
    fn decide_chat_render_public_chat_is_rendered() {
        let t = chat_task(".chat-7", "public");
        match decide_chat_render(&t, true, false) {
            ChatRender::Render(r) => assert_eq!(r, "chat-7"),
            other => panic!("expected Render, got {other:?}"),
        }
    }

    #[test]
    fn decide_chat_render_internal_hidden_unless_all_chats() {
        let t = chat_task(".chat-3", "internal");
        assert!(matches!(
            decide_chat_render(&t, true, false),
            ChatRender::HiddenByVisibility(_)
        ));
        match decide_chat_render(&t, true, true) {
            ChatRender::Render(r) => assert_eq!(r, "chat-3"),
            other => panic!("expected Render with --all, got {other:?}"),
        }
    }

    #[test]
    fn decide_chat_render_legacy_coordinator_id_works() {
        let t = chat_task(".coordinator-2", "public");
        match decide_chat_render(&t, true, false) {
            ChatRender::Render(r) => assert_eq!(r, "coordinator-2"),
            other => panic!("expected Render for legacy id, got {other:?}"),
        }
    }

    // ────────────────────────────────────────────────────────────────────
    // Message indicator surfacing (parity with TUI envelope)
    // ────────────────────────────────────────────────────────────────────

    fn make_msg(id: u64, sender: &str, body: &str) -> Message {
        Message {
            id,
            timestamp: format!("2025-01-01T00:00:{:02}Z", id),
            sender: sender.to_string(),
            body: body.to_string(),
            priority: "normal".to_string(),
            status: msg_queue::DeliveryStatus::Sent,
            read_at: None,
        }
    }

    /// Build a TaskMessages with explicit agent-perspective counts. The
    /// production code derives these from `MessageStats`, which depends on
    /// the assigned-agent identity — too brittle to recompute in tests, so
    /// just pass the values you want to assert on.
    fn bundle_counts(
        messages: Vec<Message>,
        status: Option<CoordinatorMessageStatus>,
        incoming: usize,
        outgoing: usize,
    ) -> TaskMessages {
        let has_unread = matches!(status, Some(CoordinatorMessageStatus::Unseen)) && incoming > 0;
        TaskMessages {
            messages,
            status,
            incoming,
            outgoing,
            has_unread,
        }
    }

    #[test]
    fn msg_indicator_inline_empty_when_no_messages() {
        let s = render_msg_indicator_inline(None, "task-x");
        assert!(s.is_empty(), "expected empty for None bundle, got {s}");
        let empty_bundle = bundle_counts(Vec::new(), None, 0, 0);
        let s = render_msg_indicator_inline(Some(&empty_bundle), "task-x");
        assert!(s.is_empty(), "expected empty for empty bundle, got {s}");
    }

    #[test]
    fn msg_indicator_inline_unseen_carries_unread_class() {
        let b = bundle_counts(
            vec![make_msg(1, "user", "hi")],
            Some(CoordinatorMessageStatus::Unseen),
            1,
            0,
        );
        let s = render_msg_indicator_inline(Some(&b), "task-x");
        assert!(s.contains("msg-indicator"), "no msg-indicator class: {s}");
        assert!(s.contains("msg-unseen"), "missing msg-unseen class: {s}");
        assert!(
            s.contains("has-unread-msg"),
            "missing has-unread-msg class: {s}"
        );
        assert!(s.contains("✉"), "missing envelope glyph: {s}");
        assert!(
            s.contains("data-msg-action=\"messages\""),
            "missing click action attribute: {s}"
        );
        assert!(
            s.contains("data-task-id=\"task-x\""),
            "missing data-task-id: {s}"
        );
    }

    #[test]
    fn msg_indicator_inline_seen_no_unread_class() {
        let b = bundle_counts(
            vec![make_msg(1, "user", "hi")],
            Some(CoordinatorMessageStatus::Seen),
            1,
            0,
        );
        let s = render_msg_indicator_inline(Some(&b), "task-x");
        assert!(s.contains("msg-seen"), "missing msg-seen class: {s}");
        assert!(
            !s.contains("has-unread-msg"),
            "should NOT carry unread class when seen: {s}"
        );
        assert!(s.contains("↩"), "expected seen glyph ↩: {s}");
    }

    #[test]
    fn msg_indicator_inline_replied_uses_check_glyph() {
        // 1 incoming + 1 outgoing — the "agent replied" pattern.
        let b = bundle_counts(
            vec![
                make_msg(1, "user", "hi"),
                make_msg(2, "agent-x", "ack"),
            ],
            Some(CoordinatorMessageStatus::Replied),
            1,
            1,
        );
        let s = render_msg_indicator_inline(Some(&b), "task-x");
        assert!(s.contains("msg-replied"), "missing msg-replied class: {s}");
        assert!(s.contains("✓"), "expected replied glyph ✓: {s}");
        // Count format `incoming/outgoing` when there are outgoing messages.
        assert!(s.contains("1/1"), "expected count 1/1, got: {s}");
    }

    #[test]
    fn msg_indicator_count_format_incoming_only() {
        // Two incoming, zero outgoing → indicator shows "2", not "2/0".
        let b = bundle_counts(
            vec![
                make_msg(1, "user", "a"),
                make_msg(2, "user", "b"),
            ],
            Some(CoordinatorMessageStatus::Unseen),
            2,
            0,
        );
        let s = render_msg_indicator_inline(Some(&b), "task-x");
        assert!(
            s.contains("class=\"msg-count\">2<"),
            "expected count 2 (no outgoing), got: {s}"
        );
        assert!(!s.contains("2/0"), "should not render 2/0 form: {s}");
    }

    #[test]
    fn msg_indicator_escapes_task_id() {
        let b = bundle_counts(
            vec![make_msg(1, "user", "hi")],
            Some(CoordinatorMessageStatus::Unseen),
            1,
            0,
        );
        // A task id with HTML-ish chars must not break out of attribute quotes.
        let s = render_msg_indicator_inline(Some(&b), "weird\"<id>");
        assert!(!s.contains("\"<id>"), "raw HTML chars leaked: {s}");
        assert!(s.contains("&quot;"), "expected quote-escape: {s}");
    }

    #[test]
    fn messages_section_empty_when_bundle_empty() {
        assert!(render_messages_section(None).is_empty());
        let b = bundle_counts(Vec::new(), None, 0, 0);
        assert!(render_messages_section(Some(&b)).is_empty());
    }

    #[test]
    fn messages_section_emits_each_message() {
        let b = bundle_counts(
            vec![
                make_msg(1, "user", "first"),
                make_msg(2, "agent-x", "reply"),
            ],
            Some(CoordinatorMessageStatus::Replied),
            1,
            1,
        );
        let s = render_messages_section(Some(&b));
        assert!(
            s.contains("id=\"messages-section\""),
            "missing scroll-target id: {s}"
        );
        assert!(s.contains("class=\"messages-section msg-replied\""), "missing status class: {s}");
        assert!(s.contains("first"), "missing first message body: {s}");
        assert!(s.contains("reply"), "missing reply body: {s}");
        // The msg-incoming / msg-outgoing CSS classes are coordinator-perspective
        // (us vs them). Senders "user"/"tui"/"coordinator" are outgoing, others
        // are incoming. Here `user` = outgoing, `agent-x` = incoming.
        assert!(s.contains("msg-incoming"), "missing incoming class: {s}");
        assert!(s.contains("msg-outgoing"), "missing outgoing class: {s}");
        assert!(s.contains("#1"), "missing message id #1: {s}");
        assert!(s.contains("#2"), "missing message id #2: {s}");
    }

    #[test]
    fn messages_section_escapes_html_in_body() {
        let b = bundle_counts(
            vec![make_msg(1, "user", "<script>alert(1)</script>")],
            Some(CoordinatorMessageStatus::Unseen),
            1,
            0,
        );
        let s = render_messages_section(Some(&b));
        assert!(
            !s.contains("<script>alert(1)</script>"),
            "raw script tag leaked: {s}"
        );
        assert!(
            s.contains("&lt;script&gt;"),
            "expected escaped script tag: {s}"
        );
    }

    #[test]
    fn task_messages_to_json_round_trip() {
        let b = bundle_counts(
            vec![
                make_msg(1, "user", "hi"),
                make_msg(2, "agent-x", "yo"),
            ],
            Some(CoordinatorMessageStatus::Replied),
            1,
            1,
        );
        let v = task_messages_to_json(&b);
        assert_eq!(v["status"], "replied");
        assert_eq!(v["incoming"], 1);
        assert_eq!(v["outgoing"], 1);
        assert_eq!(v["has_unread"], false);
        assert_eq!(v["icon"], "✓");
        let arr = v["messages"].as_array().expect("messages array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["sender"], "user");
        assert_eq!(arr[0]["body"], "hi");
        assert_eq!(arr[0]["status"], "sent");
    }

    #[test]
    fn task_to_json_embeds_msg_bundle_when_present() {
        use crate::graph::WorkGraph;
        let mut task = Task::default();
        task.id = "test-task".to_string();
        let graph = WorkGraph::new();
        let included: HashSet<&str> = std::iter::once("test-task").collect();

        // Without bundle → no msg field.
        let json_no_msg = task_to_json(&task, &graph, None, None, &included);
        assert!(json_no_msg.get("msg").is_none(), "msg field leaked when no bundle");

        // With bundle → msg field present.
        let b = bundle_counts(
            vec![make_msg(1, "user", "hi")],
            Some(CoordinatorMessageStatus::Unseen),
            1,
            0,
        );
        let json_msg = task_to_json(&task, &graph, None, Some(&b), &included);
        let m = json_msg.get("msg").expect("msg field present");
        assert_eq!(m["status"], "unseen");
        assert_eq!(m["incoming"], 1);
        assert_eq!(m["has_unread"], true);
    }

    #[test]
    fn task_to_json_skips_msg_field_for_empty_bundle() {
        use crate::graph::WorkGraph;
        let mut task = Task::default();
        task.id = "test-task".to_string();
        let graph = WorkGraph::new();
        let included: HashSet<&str> = std::iter::once("test-task").collect();
        let empty = bundle_counts(Vec::new(), None, 0, 0);
        let json = task_to_json(&task, &graph, None, Some(&empty), &included);
        assert!(
            json.get("msg").is_none(),
            "empty bundle should not emit msg field"
        );
    }

    /// End-to-end: send a message via the message queue, run the loader, and
    /// verify the indicator shows up. Mirrors the live smoke flow:
    ///   wg msg send <task> 'test'  →  wg html  →  indicator visible.
    #[test]
    fn load_task_messages_surfaces_after_msg_send() {
        use crate::graph::{Node, WorkGraph};
        use crate::messages::send_message;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&dir).unwrap();

        let mut graph = WorkGraph::new();
        let mut task = Task::default();
        task.id = "smoke-task".to_string();
        task.title = "Smoke task".to_string();
        graph.add_node(Node::Task(task.clone()));

        // Simulate `wg msg send smoke-task 'test'` — sender defaults to "user"
        // when invoked from a non-task context.
        send_message(&dir, "smoke-task", "test", "user", "normal").unwrap();

        let task_ref = graph.get_task("smoke-task").unwrap();
        let included: Vec<&Task> = vec![task_ref];
        let map = load_task_messages(&dir, &included);

        let bundle = map.get("smoke-task").expect("bundle for smoke-task");
        assert_eq!(bundle.messages.len(), 1, "expected 1 message");
        // No assigned agent → MessageStats counts everyone as incoming.
        assert_eq!(bundle.incoming, 1);
        assert_eq!(bundle.outgoing, 0);
        // No assigned agent cursor → has_unread is true.
        assert!(bundle.has_unread, "expected unread for fresh message");

        let inline = render_msg_indicator_inline(Some(bundle), &task_ref.id);
        assert!(
            inline.contains("msg-indicator"),
            "indicator missing after msg send: {inline}"
        );
        assert!(
            inline.contains("has-unread-msg"),
            "expected unread class for fresh user message: {inline}"
        );
    }
}

#[cfg(test)]
impl std::fmt::Debug for ChatRender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChatRender::None => write!(f, "ChatRender::None"),
            ChatRender::Render(r) => write!(f, "ChatRender::Render({r:?})"),
            ChatRender::HiddenByVisibility(v) => {
                write!(f, "ChatRender::HiddenByVisibility({v:?})")
            }
        }
    }
}
