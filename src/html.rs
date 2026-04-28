//! `wg html`: render the workgraph as a static HTML site.
//!
//! Index page shows the ASCII viz from `wg viz` rendered verbatim in a
//! monospace `<pre>` element. Task identifiers in the ASCII are wrapped in
//! clickable spans that open an inline side inspector panel. Per-task detail
//! pages remain available as deeplink targets.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;

use crate::graph::{Status, Task, WorkGraph};
use crate::parser::load_graph;

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

/// Status palette — CSS-formatted `rgb(r,g,b)` strings.
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

const STYLE_CSS: &str = include_str!("html_assets/style.css");

// ── ASCII viz capture ────────────────────────────────────────────────────────

/// Capture `wg viz --all --no-tui` output for the given workgraph directory.
/// Falls back to an empty string if the subprocess fails.
fn capture_viz_ascii(workgraph_dir: &Path) -> String {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return String::new(),
    };
    let result = std::process::Command::new(&exe)
        .arg("--dir")
        .arg(workgraph_dir)
        .arg("viz")
        .arg("--all")
        .arg("--no-tui")
        .arg("--columns")
        .arg("120")
        .output();
    match result {
        Ok(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).into_owned()
        }
        _ => String::new(),
    }
}

/// Scan `text` for any occurrence of a known task ID (from `task_ids`) and
/// wrap each match in a clickable `<span data-task-id="…">`. Non-task-ID
/// characters are HTML-escaped. Works on char boundaries so multibyte UTF-8
/// (box-drawing chars, Unicode symbols) passes through correctly.
fn make_task_ids_clickable(text: &str, task_ids: &HashSet<&str>) -> String {
    if task_ids.is_empty() {
        return escape_html(text);
    }

    let mut out = String::with_capacity(text.len() * 2);
    let mut rest = text;

    while !rest.is_empty() {
        // Find the start of the next potential identifier (ASCII alphanumeric or . _ -)
        let id_start = rest
            .char_indices()
            .find(|(_, c)| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-');

        match id_start {
            None => {
                // No more identifiers — escape the remainder
                out.push_str(&escape_html(rest));
                break;
            }
            Some((pos, _)) => {
                // Emit everything before this identifier start (escaped)
                out.push_str(&escape_html(&rest[..pos]));
                rest = &rest[pos..];

                // Find the end of the identifier span
                let id_end = rest
                    .char_indices()
                    .find(|(_, c)| {
                        !c.is_ascii_alphanumeric() && *c != '-' && *c != '_' && *c != '.'
                    })
                    .map(|(i, _)| i)
                    .unwrap_or(rest.len());

                let candidate = &rest[..id_end];
                if task_ids.contains(candidate) {
                    out.push_str("<span class=\"task-link\" data-task-id=\"");
                    out.push_str(candidate); // task IDs are ASCII-safe
                    out.push_str("\">");
                    out.push_str(candidate);
                    out.push_str("</span>");
                } else {
                    out.push_str(&escape_html(candidate));
                }
                rest = &rest[id_end..];
            }
        }
    }
    out
}

// ── Evaluation score loading ─────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
struct EvalSummary {
    score: f64,
    dimensions: Vec<(String, f64)>,
}

/// Load latest evaluation score per task from agency/evaluations/*.json.
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

        let mut dims: Vec<(String, f64)> = v
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

        // Keep only the latest timestamp per task_id
        let keep = match latest.get(&task_id) {
            None => true,
            Some((existing_ts, _)) => &timestamp > existing_ts,
        };
        if keep {
            latest.insert(
                task_id,
                (timestamp, EvalSummary { score, dimensions: dims }),
            );
        }
    }

    latest
        .into_iter()
        .map(|(task_id, (_, summary))| (task_id, summary))
        .collect()
}

// ── Inline task JSON ─────────────────────────────────────────────────────────

fn task_to_json(
    task: &Task,
    eval: Option<&EvalSummary>,
    included_ids: &HashSet<&str>,
) -> serde_json::Value {
    let log_entries: Vec<serde_json::Value> = task
        .log
        .iter()
        .rev()
        .take(20)
        .rev()
        .map(|e| {
            serde_json::json!({
                "timestamp": e.timestamp,
                "message": e.message,
            })
        })
        .collect();

    // Only expose deps that are in the included set — don't leak internal IDs.
    let after_visible: Vec<&str> = task
        .after
        .iter()
        .map(|s| s.as_str())
        .filter(|id| included_ids.contains(id))
        .collect();

    let mut obj = serde_json::json!({
        "id": task.id,
        "title": task.title,
        "status": task.status.to_string(),
        "after": after_visible,
        "tags": task.tags,
        "log": log_entries,
    });

    if let Some(m) = &task.model {
        obj["model"] = serde_json::Value::String(m.clone());
    }
    if let Some(a) = &task.agent {
        obj["agent"] = serde_json::Value::String(a.clone());
    }
    if let Some(d) = &task.description {
        // Truncate description to 3000 chars for the panel
        let truncated = if d.len() > 3000 {
            format!("{}…", &d[..3000])
        } else {
            d.clone()
        };
        obj["description"] = serde_json::Value::String(truncated);
    }
    if let Some(reason) = &task.failure_reason {
        obj["failure_reason"] = serde_json::Value::String(reason.clone());
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
    obj
}

/// Build the inline `window.WG_TASKS = {...}` JSON blob for all included tasks.
fn build_tasks_json(
    included: &[&Task],
    evals: &HashMap<String, EvalSummary>,
    included_ids: &HashSet<&str>,
) -> String {
    let map: serde_json::Map<String, serde_json::Value> = included
        .iter()
        .map(|t| {
            let eval = evals.get(&t.id);
            (t.id.clone(), task_to_json(t, eval, included_ids))
        })
        .collect();
    let json_str = serde_json::to_string(&serde_json::Value::Object(map))
        .unwrap_or_else(|_| "{}".to_string());
    // Safety: prevent </script> from breaking the script block
    json_str.replace("</script>", "<\\/script>")
}

// ── Index page ───────────────────────────────────────────────────────────────

fn render_legend() -> String {
    let entries = [
        Status::Open,
        Status::InProgress,
        Status::Done,
        Status::Failed,
        Status::Blocked,
        Status::Waiting,
        Status::PendingValidation,
        Status::PendingEval,
        Status::Abandoned,
        Status::Incomplete,
    ];
    let mut s = String::new();
    s.push_str("<ul class=\"legend\">\n");
    for st in entries {
        s.push_str(&format!(
            "  <li><span class=\"swatch\" style=\"background:{color}\"></span>{name}</li>\n",
            color = status_color(st),
            name = st,
        ));
    }
    s.push_str("</ul>\n");
    s
}

fn render_footer(
    total_in_graph: usize,
    total_public: usize,
    show_all: bool,
    since_label: Option<&str>,
) -> String {
    let now = chrono::Utc::now().to_rfc3339();
    let visibility_str = if show_all { "all tasks" } else { "public-only" };
    let filter_note = if show_all {
        format!(
            "Visibility filter: <strong>OFF</strong> (--all). Showing {} of {} tasks{}.",
            total_public,
            total_in_graph,
            since_label
                .map(|s| format!(", last {}", s))
                .unwrap_or_default(),
        )
    } else {
        let hidden = total_in_graph.saturating_sub(total_public);
        if let Some(label) = since_label {
            format!(
                "Showing {} of {} tasks: {}, last {}. {} internal/peer tasks are hidden.",
                total_public, total_in_graph, visibility_str, label, hidden,
            )
        } else {
            format!(
                "Visibility filter: <strong>public-only</strong>. Showing {} of {} tasks; \
                 {} internal/peer tasks are hidden.",
                total_public, total_in_graph, hidden,
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

static PANEL_JS: &str = r##"
(function() {
  var tasks = window.WG_TASKS || {};
  var panel = document.getElementById('side-panel');
  var panelContent = document.getElementById('panel-content');
  var closeBtn = document.getElementById('panel-close');

  function e(s) {
    return String(s)
      .replace(/&/g,'&amp;').replace(/</g,'&lt;')
      .replace(/>/g,'&gt;').replace(/"/g,'&quot;');
  }

  function statusCls(s) { return s.replace(/\s+/g,'-').toLowerCase(); }

  function openPanel(taskId) {
    var task = tasks[taskId];
    if (!task) return;
    var h = '';
    h += '<div class="panel-header">';
    h += '<code class="panel-id">' + e(task.id) + '</code> ';
    h += '<span class="badge ' + statusCls(task.status) + '">' + e(task.status) + '</span>';
    h += '</div>';
    h += '<p class="panel-title">' + e(task.title) + '</p>';
    if (task.model || task.agent) {
      h += '<p class="panel-meta">';
      if (task.model) h += '<strong>Model:</strong> <code>' + e(task.model) + '</code> &nbsp;';
      if (task.agent) h += '<strong>Agent:</strong> <code>' + e(task.agent.slice(0,8)) + '…</code>';
      h += '</p>';
    }
    if (task.tags && task.tags.length > 0) {
      h += '<p class="panel-tags">' + task.tags.map(function(t){return '<code>'+e(t)+'</code>';}).join(' ') + '</p>';
    }
    if (task.description) {
      var desc = task.description;
      h += '<details><summary>Description</summary><pre class="panel-desc">' + e(desc) + '</pre></details>';
    }
    if (task.after && task.after.length > 0) {
      h += '<details open><summary>Depends on (' + task.after.length + ')</summary><ul class="panel-deps">';
      task.after.forEach(function(depId) {
        var dep = tasks[depId];
        if (dep) {
          h += '<li><a href="#" class="dep-link" data-task-id="' + e(depId) + '">';
          h += '<span class="badge ' + statusCls(dep.status) + '">' + e(dep.status) + '</span> ';
          h += '<code>' + e(depId) + '</code></a></li>';
        } else {
          h += '<li><code>' + e(depId) + '</code></li>';
        }
      });
      h += '</ul></details>';
    }
    if (task.eval_score != null) {
      h += '<details open><summary>Eval score</summary>';
      h += '<p class="eval-score">' + task.eval_score.toFixed(2) + '</p>';
      if (task.eval_dims) {
        h += '<table class="eval-dims"><tbody>';
        Object.keys(task.eval_dims).sort().forEach(function(dim) {
          var v = task.eval_dims[dim];
          h += '<tr><td>' + e(dim.replace(/_/g,' ')) + '</td><td class="eval-dim-val">' + v.toFixed(2) + '</td></tr>';
        });
        h += '</tbody></table>';
      }
      h += '</details>';
    }
    if (task.log && task.log.length > 0) {
      h += '<details><summary>Log (' + task.log.length + ' entries)</summary><ul class="panel-log">';
      task.log.forEach(function(entry) {
        var ts = entry.timestamp ? entry.timestamp.slice(0,19).replace('T',' ') : '';
        h += '<li><span class="log-ts">' + e(ts) + '</span> ' + e(entry.message) + '</li>';
      });
      h += '</ul></details>';
    }
    if (task.failure_reason) {
      h += '<details open><summary>Failure reason</summary><pre class="panel-desc">' + e(task.failure_reason) + '</pre></details>';
    }
    h += '<p class="panel-deeplink"><a href="tasks/' + encodeURIComponent(taskId) + '.html">View full task page →</a></p>';
    panelContent.innerHTML = h;
    panel.classList.remove('hidden');

    // Bind dep-link clicks
    panelContent.querySelectorAll('.dep-link').forEach(function(a) {
      a.addEventListener('click', function(ev) {
        ev.preventDefault();
        openPanel(a.dataset.taskId);
      });
    });
  }

  // Bind task-link clicks in the viz
  document.querySelectorAll('.task-link').forEach(function(el) {
    el.style.cursor = 'pointer';
    el.addEventListener('click', function() { openPanel(el.dataset.taskId); });
  });

  // Close
  closeBtn.addEventListener('click', function() { panel.classList.add('hidden'); });
  document.addEventListener('keydown', function(ev) {
    if (ev.key === 'Escape') panel.classList.add('hidden');
  });
  document.addEventListener('click', function(ev) {
    if (!panel.classList.contains('hidden')
        && !panel.contains(ev.target)
        && !ev.target.classList.contains('task-link')) {
      panel.classList.add('hidden');
    }
  });
})();
"##;

fn render_index(
    graph: &WorkGraph,
    included: &[&Task],
    tasks_json: &str,
    ascii_viz: &str,
    show_all: bool,
    since_label: Option<&str>,
) -> String {
    let total_in_graph = graph.tasks().count();
    let total_public = included.len();

    // Status counts for the summary
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for t in included {
        *counts.entry(t.status.to_string()).or_insert(0) += 1;
    }

    let task_ids: HashSet<&str> = included.iter().map(|t| t.id.as_str()).collect();

    // ASCII viz: make task IDs clickable
    let viz_body = if ascii_viz.trim().is_empty() {
        escape_html("(no tasks to display)")
    } else {
        make_task_ids_clickable(ascii_viz, &task_ids)
    };
    let viz_html = format!("<pre class=\"viz-pre\">{}</pre>", viz_body);

    // Ordered task list
    let mut ordered: Vec<&&Task> = included.iter().collect();
    ordered.sort_by_key(|t| (t.status.to_string(), t.id.clone()));
    let mut list = String::new();
    list.push_str("<ul class=\"task-list\">\n");
    for t in &ordered {
        list.push_str(&format!(
            "  <li><a href=\"{href}\"><span class=\"badge {cls}\">{status}</span> \
             <code>{id}</code> — {title}</a></li>\n",
            href = task_filename(&t.id),
            cls = status_class(t.status),
            status = t.status,
            id = escape_html(&t.id),
            title = escape_html(&t.title),
        ));
    }
    list.push_str("</ul>\n");

    let legend = render_legend();
    let footer = render_footer(total_in_graph, total_public, show_all, since_label);

    format!(
        "<!DOCTYPE html>\n\
         <html lang=\"en\">\n\
         <head>\n\
         <meta charset=\"utf-8\" />\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n\
         <title>Workgraph</title>\n\
         <link rel=\"stylesheet\" href=\"style.css\" />\n\
         </head>\n\
         <body>\n\
         <header><h1>Workgraph</h1>\n\
         <p class=\"subtitle\">Task graph — {n} tasks shown.</p>\n\
         </header>\n\
         <div class=\"page-layout\">\n\
         <main class=\"main-content\">\n\
         <section class=\"dag-section\">\n\
         <h2>Dependency graph <span class=\"viz-hint\">(click a task id to inspect)</span></h2>\n\
         <div class=\"viz-wrap\">{viz}</div>\n\
         </section>\n\
         <section class=\"legend-section\">\n\
         <h2>Legend</h2>\n\
         {legend}\n\
         </section>\n\
         <section class=\"list-section\">\n\
         <h2>Tasks ({total_public})</h2>\n\
         {list}\n\
         </section>\n\
         </main>\n\
         <aside id=\"side-panel\" class=\"side-panel hidden\">\n\
         <button id=\"panel-close\" aria-label=\"Close\">×</button>\n\
         <div id=\"panel-content\"></div>\n\
         </aside>\n\
         </div>\n\
         <footer>{footer}</footer>\n\
         <script>window.WG_TASKS = {tasks_json};</script>\n\
         <script>{panel_js}</script>\n\
         </body>\n\
         </html>\n",
        n = total_public,
        viz = viz_html,
        legend = legend,
        list = list,
        total_public = total_public,
        footer = footer,
        tasks_json = tasks_json,
        panel_js = PANEL_JS,
    )
}

// ── Per-task page ─────────────────────────────────────────────────────────────

fn render_task_page(
    task: &Task,
    graph: &WorkGraph,
    included_ids: &HashSet<&str>,
    eval: Option<&EvalSummary>,
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
            format!("<pre class=\"description\">{}</pre>", escape_html(d))
        }
        _ => "<p class=\"none\">(no description)</p>".to_string(),
    };

    let mut meta_rows: Vec<(String, String)> = Vec::new();
    meta_rows.push((
        "Status".into(),
        format!(
            "<span class=\"badge {}\">{}</span>",
            status_cls, status_str
        ),
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

    // Log entries (last 30)
    let log_html = if task.log.is_empty() {
        "<p class=\"none\">(no log entries)</p>".to_string()
    } else {
        let mut s = String::from("<ul class=\"task-log\">");
        for entry in task.log.iter().rev().take(30).rev() {
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
         <section><h2>Log</h2>{log}</section>\n\
         </main>\n\
         <footer><p class=\"meta\">Public mirror — visibility = <code>{vis}</code></p></footer>\n\
         </body>\n\
         </html>\n",
        id = id,
        title = title,
        meta = meta_html,
        desc = description_html,
        deps = deps_html,
        revdeps = dependents_html,
        log = log_html,
        vis = escape_html(&task.visibility),
    )
}

// ── Public API ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RenderSummary {
    pub out_dir: std::path::PathBuf,
    pub total_in_graph: usize,
    pub public_count: usize,
    pub pages_written: usize,
    pub show_all: bool,
    pub since: Option<String>,
}

pub fn render_site(
    graph: &WorkGraph,
    workgraph_dir: &Path,
    out_dir: &Path,
    show_all: bool,
    since: Option<&str>,
) -> Result<RenderSummary> {
    // Parse --since into a cutoff timestamp.
    let since_cutoff: Option<DateTime<Utc>> = since
        .map(|s| parse_since(s).map(|d| Utc::now() - d))
        .transpose()?;

    fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create output dir: {}", out_dir.display()))?;
    let tasks_dir = out_dir.join("tasks");
    fs::create_dir_all(&tasks_dir)
        .with_context(|| format!("failed to create tasks dir: {}", tasks_dir.display()))?;

    let all_tasks: Vec<&Task> = graph.tasks().collect();

    // Visibility filter.
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

    // Load eval scores
    let evals = load_eval_scores(workgraph_dir);

    // Capture ASCII viz
    let ascii_viz = capture_viz_ascii(workgraph_dir);

    // Build inline JSON for all included tasks
    let tasks_json = build_tasks_json(&included, &evals, &included_ids);

    // Write style.css
    let css_path = out_dir.join("style.css");
    fs::write(&css_path, STYLE_CSS).context("failed to write style.css")?;

    // Write index.html
    let index_html = render_index(graph, &included, &tasks_json, &ascii_viz, show_all, since);
    let index_path = out_dir.join("index.html");
    fs::write(&index_path, &index_html).context("failed to write index.html")?;

    // Write per-task pages
    let mut pages_written = 0usize;
    for task in &included {
        let eval = evals.get(&task.id);
        let html = render_task_page(task, graph, &included_ids, eval);
        let path = tasks_dir.join(format!("{}.html", url_encode_id(&task.id)));
        fs::write(&path, html)
            .with_context(|| format!("failed to write {}", path.display()))?;
        pages_written += 1;
    }

    Ok(RenderSummary {
        out_dir: out_dir.to_path_buf(),
        total_in_graph: graph.tasks().count(),
        public_count: included.len(),
        pages_written,
        show_all,
        since: since.map(|s| s.to_string()),
    })
}

pub fn run(workgraph_dir: &Path, out: &Path, all: bool, since: Option<&str>, json: bool) -> Result<()> {
    let graph_path = workgraph_dir.join("graph.jsonl");
    if !graph_path.exists() {
        anyhow::bail!(
            "Workgraph not initialized at {}. Run `wg init` first.",
            workgraph_dir.display()
        );
    }
    let graph = load_graph(&graph_path).context("failed to load graph")?;

    let summary = render_site(&graph, workgraph_dir, out, all, since)?;

    if json {
        let payload = serde_json::json!({
            "out_dir": summary.out_dir.display().to_string(),
            "total_in_graph": summary.total_in_graph,
            "public_count": summary.public_count,
            "pages_written": summary.pages_written,
            "show_all": summary.show_all,
            "since": summary.since,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        let filter = if summary.show_all {
            "all tasks (visibility filter OFF)".to_string()
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
        println!("Open {}/index.html in a browser.", summary.out_dir.display());
    }

    Ok(())
}
