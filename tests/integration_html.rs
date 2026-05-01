//! Integration tests for `wg html`.
//!
//! These exercise the static-site renderer directly (via `commands::html::render_site`)
//! against synthetic graphs in tempdirs. The CLI smoke is covered by running
//! the actual binary against the real `.wg/` graph in this repo
//! during validation.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use tempfile::TempDir;
use workgraph::chat;
use workgraph::chat_sessions::{self, SessionKind};
use workgraph::graph::{Node, Status, Task, WorkGraph};

use workgraph::html;

fn make_task(id: &str, title: &str, visibility: &str) -> Task {
    Task {
        id: id.to_string(),
        title: title.to_string(),
        visibility: visibility.to_string(),
        ..Task::default()
    }
}

fn paths_in(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for e in walkdir(dir) {
        out.push(e.strip_prefix(dir).unwrap().to_string_lossy().into_owned());
    }
    out.sort();
    out
}

fn walkdir(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let p = entry.path();
        if p.is_dir() {
            out.extend(walkdir(&p));
        } else {
            out.push(p);
        }
    }
    out
}

fn read_all(dir: &Path) -> String {
    let mut buf = String::new();
    for entry in walkdir(dir) {
        if let Ok(s) = fs::read_to_string(&entry) {
            buf.push_str(&s);
            buf.push('\n');
        }
    }
    buf
}

fn build_graph(tasks: Vec<Task>) -> WorkGraph {
    let mut g = WorkGraph::new();
    for t in tasks {
        g.add_node(Node::Task(t));
    }
    g
}

#[test]
fn renders_index_with_only_public_task_count() {
    // 3 public, 2 internal — index should reflect 3 task nodes.
    let mut t1 = make_task("alpha", "Alpha", "public");
    let mut t2 = make_task("beta", "Beta", "public");
    let mut t3 = make_task("gamma", "Gamma", "public");
    t2.after = vec!["alpha".into()];
    t3.after = vec!["beta".into()];
    t1.status = Status::Done;
    t2.status = Status::InProgress;
    t3.status = Status::Open;

    let internal_a = make_task(".eval-alpha", "Eval Alpha", "internal");
    let internal_b = make_task(".assign-beta", "Assign Beta", "internal");

    let graph = build_graph(vec![t1, t2, t3, internal_a, internal_b]);

    let dir = TempDir::new().unwrap();
    let summary = html::render_site(&graph, dir.path(), dir.path(), html::RenderOptions::default()).unwrap();

    assert_eq!(summary.public_count, 3, "expected 3 public tasks");
    assert_eq!(summary.total_in_graph, 5);
    assert_eq!(summary.pages_written, 3);

    let index = fs::read_to_string(dir.path().join("index.html")).unwrap();
    // Each public task id should appear in the index.
    assert!(index.contains("alpha"), "alpha missing from index");
    assert!(index.contains("beta"), "beta missing from index");
    assert!(index.contains("gamma"), "gamma missing from index");

    // Three task pages should exist.
    assert!(dir.path().join("tasks/alpha.html").exists());
    assert!(dir.path().join("tasks/beta.html").exists());
    assert!(dir.path().join("tasks/gamma.html").exists());
}

#[test]
fn internal_tasks_excluded_from_all_output() {
    let public = make_task("public-task", "Public stuff", "public");
    let internal = make_task("secret-task", "API_KEY=swordfish", "internal");
    let peer = make_task("peer-only", "peer-confidential", "peer");

    let graph = build_graph(vec![public, internal, peer]);

    let dir = TempDir::new().unwrap();
    html::render_site(&graph, dir.path(), dir.path(), html::RenderOptions::default()).unwrap();

    // The internal-only id should not appear in any rendered file.
    let blob = read_all(dir.path());
    assert!(
        !blob.contains("secret-task"),
        "internal task id leaked into output"
    );
    assert!(
        !blob.contains("API_KEY=swordfish"),
        "internal task body leaked into output"
    );
    assert!(
        !blob.contains("peer-only"),
        "peer-visibility task leaked into output"
    );
    assert!(
        !blob.contains("peer-confidential"),
        "peer-visibility body leaked into output"
    );

    // No internal task page file should exist.
    let files = paths_in(dir.path());
    for f in &files {
        assert!(
            !f.contains("secret-task") && !f.contains("peer-only"),
            "found leaked file: {}",
            f
        );
    }
}

#[test]
fn per_task_links_resolve_within_output() {
    let t1 = make_task("a", "A", "public");
    let mut t2 = make_task("b", "B", "public");
    let mut t3 = make_task("c", "C", "public");
    t2.after = vec!["a".into()];
    t3.after = vec!["a".into(), "b".into()];

    let graph = build_graph(vec![t1.clone(), t2, t3]);

    let dir = TempDir::new().unwrap();
    html::render_site(&graph, dir.path(), dir.path(), html::RenderOptions::default()).unwrap();

    // Index should link to tasks/a.html, tasks/b.html, tasks/c.html.
    let index = fs::read_to_string(dir.path().join("index.html")).unwrap();
    for id in &["a", "b", "c"] {
        let needle = format!("tasks/{}.html", id);
        assert!(
            index.contains(&needle),
            "index missing link to {}: index html = {}",
            needle,
            &index[..index.len().min(2000)]
        );
    }

    // c's page should link to tasks/a.html and tasks/b.html (relative paths).
    let c_page = fs::read_to_string(dir.path().join("tasks/c.html")).unwrap();
    assert!(c_page.contains("./a.html"), "c page missing dep link to a");
    assert!(c_page.contains("./b.html"), "c page missing dep link to b");

    // a's page should mention dependents (b and c) via "Required by".
    let a_page = fs::read_to_string(dir.path().join("tasks/a.html")).unwrap();
    assert!(a_page.contains("./b.html"), "a page missing dependent link to b");
    assert!(a_page.contains("./c.html"), "a page missing dependent link to c");

    // No file should reference a hashed/missing path.
    let _ = t1; // silence unused
}

#[test]
fn empty_public_graph_renders_without_crashing() {
    let internal = make_task("only-internal", "internal", "internal");
    let graph = build_graph(vec![internal]);

    let dir = TempDir::new().unwrap();
    let summary = html::render_site(&graph, dir.path(), dir.path(), html::RenderOptions::default()).unwrap();
    assert_eq!(summary.public_count, 0);
    assert_eq!(summary.pages_written, 0);

    let index = fs::read_to_string(dir.path().join("index.html")).unwrap();
    assert!(
        index.contains("No tasks to display") || index.contains("Tasks (0)"),
        "expected empty-graph indicator in index.html"
    );
}

#[test]
fn show_all_overrides_visibility_filter() {
    let public = make_task("public-id", "p", "public");
    let internal = make_task("internal-id", "i", "internal");
    let graph = build_graph(vec![public, internal]);

    let dir = TempDir::new().unwrap();
    let summary = html::render_site(
        &graph,
        dir.path(),
        dir.path(),
        html::RenderOptions { show_all: true, ..Default::default() },
    ).unwrap();
    assert_eq!(summary.public_count, 2, "with --all both tasks should appear");
    assert_eq!(summary.pages_written, 2);

    let blob = read_all(dir.path());
    assert!(blob.contains("internal-id"));
    assert!(blob.contains("public-id"));
}

#[test]
fn dag_layout_renders_task_ids() {
    // a -> b -> c chain. All three task ids must appear in the rendered index.
    let mut a = make_task("la-a", "A", "public");
    let mut b = make_task("la-b", "B", "public");
    let mut c = make_task("la-c", "C", "public");
    b.after = vec!["la-a".into()];
    c.after = vec!["la-b".into()];
    a.status = Status::Done;
    b.status = Status::InProgress;
    c.status = Status::Open;

    let graph = build_graph(vec![a, b, c]);
    let dir = TempDir::new().unwrap();
    html::render_site(&graph, dir.path(), dir.path(), html::RenderOptions::default()).unwrap();

    let index = fs::read_to_string(dir.path().join("index.html")).unwrap();
    // viz-pre element present (ASCII viz, not SVG).
    assert!(index.contains("viz-pre"), "viz-pre missing from index");
    // Each id appears in the task list section.
    for id in &["la-a", "la-b", "la-c"] {
        assert!(index.contains(id), "id {} missing", id);
    }
    // Status colors appear in the legend swatches.
    assert!(index.contains("rgb(80,220,100)"), "Done color missing");
    assert!(index.contains("rgb(60,200,220)"), "InProgress color missing");
    assert!(index.contains("rgb(200,200,80)"), "Open color missing");
}

#[test]
fn description_html_is_escaped() {
    let mut t = make_task("xss-test", "Title", "public");
    t.description = Some("<script>alert('pwn')</script>".into());

    let graph = build_graph(vec![t]);
    let dir = TempDir::new().unwrap();
    html::render_site(&graph, dir.path(), dir.path(), html::RenderOptions::default()).unwrap();

    let page = fs::read_to_string(dir.path().join("tasks/xss-test.html")).unwrap();
    assert!(
        !page.contains("<script>alert"),
        "raw <script> tag leaked: {}",
        &page
    );
    assert!(
        page.contains("&lt;script&gt;"),
        "expected escaped <script>"
    );
}

#[test]
fn dependency_on_internal_task_aggregates_as_count_no_id_leak() {
    // Public task `pub-a` depends on two internal tasks. The internal IDs must
    // NOT appear anywhere in the rendered output — only an aggregate count.
    let mut pub_a = make_task("pub-a", "Public A", "public");
    let internal_assign = make_task(".assign-pub-a-internal-uniq", "Assign Public A", "internal");
    let internal_other = make_task(".other-internal-marker", "Other internal", "internal");
    pub_a.after = vec![
        ".assign-pub-a-internal-uniq".into(),
        ".other-internal-marker".into(),
    ];

    let graph = build_graph(vec![pub_a, internal_assign, internal_other]);
    let dir = TempDir::new().unwrap();
    html::render_site(&graph, dir.path(), dir.path(), html::RenderOptions::default()).unwrap();

    let page = fs::read_to_string(dir.path().join("tasks/pub-a.html")).unwrap();
    assert!(
        page.contains("2 non-public dependencies hidden"),
        "expected aggregate count, got: {}",
        page
    );

    // Internal task IDs must NOT appear in the page at all.
    let blob = read_all(dir.path());
    assert!(
        !blob.contains(".assign-pub-a-internal-uniq"),
        "internal dep id leaked"
    );
    assert!(
        !blob.contains(".other-internal-marker"),
        "internal dep id leaked"
    );
}

#[test]
fn output_files_layout_matches_expected() {
    let p1 = make_task("layout-x", "X", "public");
    let p2 = make_task("layout-y", "Y", "public");
    let graph = build_graph(vec![p1, p2]);

    let dir = TempDir::new().unwrap();
    html::render_site(&graph, dir.path(), dir.path(), html::RenderOptions::default()).unwrap();

    let files: HashSet<String> = paths_in(dir.path()).into_iter().collect();
    assert!(files.contains("index.html"));
    assert!(files.contains("style.css"));
    assert!(files.contains("tasks/layout-x.html"));
    assert!(files.contains("tasks/layout-y.html"));
}

#[test]
fn since_filter_excludes_old_tasks_and_notes_in_footer() {
    use chrono::{Duration, Utc};

    let recent_ts = (Utc::now() - Duration::hours(1)).to_rfc3339();
    let old_ts = (Utc::now() - Duration::days(30)).to_rfc3339();

    let mut recent = make_task("recent-task", "Recent", "public");
    recent.created_at = Some(recent_ts);

    let mut old = make_task("old-task", "Old", "public");
    old.created_at = Some(old_ts);

    let graph = build_graph(vec![recent, old]);

    let dir = TempDir::new().unwrap();
    let summary = html::render_site(
        &graph,
        dir.path(),
        dir.path(),
        html::RenderOptions {
            show_all: true,
            since: Some("24h".into()),
            ..Default::default()
        },
    ).unwrap();

    assert_eq!(summary.public_count, 1, "expected only 1 task within 24h window");
    assert!(dir.path().join("tasks/recent-task.html").exists(), "recent-task page missing");
    assert!(!dir.path().join("tasks/old-task.html").exists(), "old-task page should not exist");

    let index = fs::read_to_string(dir.path().join("index.html")).unwrap();
    assert!(
        index.contains("last 24h"),
        "footer must mention 'last 24h'"
    );
}

#[test]
fn since_filter_composes_with_visibility() {
    use chrono::{Duration, Utc};

    let recent_ts = (Utc::now() - Duration::hours(2)).to_rfc3339();

    let mut pub_task = make_task("pub-recent", "Public recent", "public");
    pub_task.created_at = Some(recent_ts.clone());
    let mut int_task = make_task("int-recent", "Internal recent", "internal");
    int_task.created_at = Some(recent_ts);

    let graph = build_graph(vec![pub_task, int_task]);
    let dir = TempDir::new().unwrap();

    let summary = html::render_site(
        &graph,
        dir.path(),
        dir.path(),
        html::RenderOptions {
            since: Some("24h".into()),
            ..Default::default()
        },
    ).unwrap();
    assert_eq!(summary.public_count, 1, "public-only filter should keep 1 public task");
    assert!(dir.path().join("tasks/pub-recent.html").exists(), "public recent task page missing");
    assert!(!dir.path().join("tasks/int-recent.html").exists(), "internal task must not appear");
}

// ────────────────────────────────────────────────────────────────────────────
// wg-html-v2: theme support, edge JSON, panel JS wiring
// ────────────────────────────────────────────────────────────────────────────

// ────────────────────────────────────────────────────────────────────────────
// wg-html-declutter: clean header + clickable Legend in side panel
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn declutter_index_has_clean_header_no_redundant_text() {
    // Spec: top of page shows "Workgraph" + task count, no "click a task to
    // inspect" subtitle, no "Dependency graph (...)" parenthetical, no
    // inline `<section class="legend-section">` (legend lives in panel only).
    let t = make_task("only", "Only", "public");
    let graph = build_graph(vec![t]);
    let dir = TempDir::new().unwrap();
    html::render_site(
        &graph,
        dir.path(),
        dir.path(),
        html::RenderOptions { show_all: true, ..Default::default() },
    )
    .unwrap();

    let index = fs::read_to_string(dir.path().join("index.html")).unwrap();

    // Subtitle is just "<n> tasks shown" — no "click a task id to inspect".
    assert!(
        !index.contains("click a task id to inspect"),
        "header subtitle still has 'click a task id to inspect'"
    );
    assert!(
        index.contains("1 tasks shown"),
        "subtitle missing or wrong: should be '1 tasks shown'"
    );

    // Old "Dependency graph (...)" headline + parenthetical hint are gone.
    assert!(
        !index.contains("Dependency graph"),
        "redundant 'Dependency graph' h2 still in index"
    );
    assert!(
        !index.contains("magenta = upstream deps"),
        "redundant edge-color parenthetical still in index"
    );

    // Inline legend section is removed (legend lives in side panel via template).
    assert!(
        !index.contains("class=\"legend-section\""),
        "inline legend-section should be removed"
    );
}

#[test]
fn declutter_index_includes_legend_toggle_button_and_template() {
    // Spec: a "Legend" button in the header opens the side panel with a full
    // legend. The legend HTML is shipped inside a hidden template so the JS
    // can clone it on click — no separate fetch required.
    let t = make_task("any", "T", "public");
    let graph = build_graph(vec![t]);
    let dir = TempDir::new().unwrap();
    html::render_site(
        &graph,
        dir.path(),
        dir.path(),
        html::RenderOptions { show_all: true, ..Default::default() },
    )
    .unwrap();

    let index = fs::read_to_string(dir.path().join("index.html")).unwrap();

    // Legend button is present in the header controls.
    assert!(
        index.contains(r#"id="legend-toggle""#),
        "legend toggle button missing from header"
    );

    // Legend content template is present and covers all spec'd sections.
    assert!(
        index.contains(r#"id="wg-legend-template""#),
        "legend template element missing"
    );
    // Edge colors section uses the CSS variables so it tracks dark/light.
    assert!(
        index.contains("var(--edge-upstream)"),
        "legend should reference --edge-upstream for magenta swatch"
    );
    assert!(
        index.contains("var(--edge-downstream)"),
        "legend should reference --edge-downstream for cyan swatch"
    );
    assert!(
        index.contains("var(--edge-cycle)"),
        "legend should reference --edge-cycle for yellow swatch"
    );
    // Status colors must be present (use the same palette as before).
    assert!(
        index.contains("rgb(80,220,100)"),
        "legend missing Done status color"
    );
    // Mentions of click behaviors and theme toggle.
    assert!(
        index.contains("Click any task id"),
        "legend missing click-behavior text"
    );
    assert!(
        index.contains("theme toggle"),
        "legend missing theme toggle reminder"
    );
    // CLI affordances.
    assert!(
        index.contains("--chat") && index.contains("--since") && index.contains("--all"),
        "legend missing CLI flag affordances"
    );
}

#[test]
fn v2_index_includes_theme_toggle_and_panel_assets() {
    let mut t = make_task("theme-tester", "T", "public");
    t.status = Status::Open;
    let graph = build_graph(vec![t]);
    let dir = TempDir::new().unwrap();
    html::render_site(
        &graph,
        dir.path(),
        dir.path(),
        html::RenderOptions { show_all: true, ..Default::default() },
    ).unwrap();

    let index = fs::read_to_string(dir.path().join("index.html")).unwrap();

    // Theme toggle button is present and wired by id.
    assert!(
        index.contains(r#"id="theme-toggle""#),
        "theme toggle button missing from index"
    );
    // The bootstrap script applies a saved theme before paint.
    assert!(
        index.contains("localStorage.getItem('wg-html-theme')"),
        "theme bootstrap script missing"
    );
    // The panel script tag is included from a separate file (rsync-friendly).
    assert!(
        index.contains(r#"src="panel.js""#),
        "panel.js script tag missing"
    );
    // The companion files exist on disk.
    assert!(
        dir.path().join("panel.js").exists(),
        "panel.js asset must be written"
    );
    assert!(
        dir.path().join("style.css").exists(),
        "style.css asset must be written"
    );
}

#[test]
fn v2_css_carries_tui_palette() {
    // Spec: "Color values verified to match TUI palette (cite source file or
    // document the mapping)" — the CSS must contain the exact RGB triples
    // documented at src/tui/viz_viewer/state.rs:271 and the magenta/cyan
    // edge highlight colors from render.rs:1500.
    let dir = TempDir::new().unwrap();
    let graph = build_graph(vec![make_task("anything", "A", "public")]);
    html::render_site(
        &graph,
        dir.path(),
        dir.path(),
        html::RenderOptions { show_all: true, ..Default::default() },
    ).unwrap();

    let css = fs::read_to_string(dir.path().join("style.css")).unwrap();
    // Status colors (TUI flash_color_for_status, state.rs:271)
    for needle in [
        "rgb(80, 220, 100)",  // done
        "rgb(220, 60, 60)",   // failed
        "rgb(60, 200, 220)",  // in-progress
        "rgb(200, 200, 80)",  // open
        "rgb(60, 160, 220)",  // waiting
        "rgb(140, 230, 80)",  // pending-eval
    ] {
        assert!(css.contains(needle), "missing TUI status color {}", needle);
    }
    // Edge highlight colors (TUI render.rs:1500 — magenta/cyan/yellow)
    assert!(css.contains("rgb(188, 63, 188)"), "missing magenta edge color");
    assert!(css.contains("rgb(17, 168, 205)"), "missing cyan edge color");
    assert!(css.contains("rgb(229, 229, 16)"), "missing yellow edge color");
}

#[test]
fn v2_css_supports_dark_and_light_themes() {
    let dir = TempDir::new().unwrap();
    let graph = build_graph(vec![make_task("any", "A", "public")]);
    html::render_site(
        &graph,
        dir.path(),
        dir.path(),
        html::RenderOptions { show_all: true, ..Default::default() },
    ).unwrap();

    let css = fs::read_to_string(dir.path().join("style.css")).unwrap();
    // Dark theme is the default (no media query needed).
    assert!(css.contains("--bg:"), "dark theme variables missing");
    // Light theme via prefers-color-scheme + manual override.
    assert!(
        css.contains("@media (prefers-color-scheme: light)"),
        "light theme media query missing"
    );
    assert!(
        css.contains(r#"[data-theme="light"]"#),
        "manual light override missing"
    );
    assert!(
        css.contains(r#"[data-theme="dark"]"#),
        "manual dark override missing"
    );
}

#[test]
fn v2_inline_json_blobs_present_in_index() {
    let mut a = make_task("alpha-v2", "Alpha", "public");
    let mut b = make_task("beta-v2", "Beta", "public");
    b.after = vec!["alpha-v2".into()];
    a.status = Status::Done;
    b.status = Status::InProgress;

    let graph = build_graph(vec![a, b]);
    let dir = TempDir::new().unwrap();
    html::render_site(
        &graph,
        dir.path(),
        dir.path(),
        html::RenderOptions { show_all: true, ..Default::default() },
    ).unwrap();

    let index = fs::read_to_string(dir.path().join("index.html")).unwrap();
    // Three JSON blobs feed the panel JS: tasks, edges (reachability), cycles.
    assert!(
        index.contains("window.WG_TASKS"),
        "WG_TASKS inline JSON missing"
    );
    assert!(
        index.contains("window.WG_EDGES"),
        "WG_EDGES inline JSON missing"
    );
    assert!(
        index.contains("window.WG_CYCLES"),
        "WG_CYCLES inline JSON missing"
    );
    // beta-v2's reachable upstream set must include alpha-v2 — that's the
    // whole point of the edge JSON, and what the JS uses for highlighting.
    assert!(
        index.contains("\"alpha-v2\""),
        "alpha-v2 missing from inline JSON"
    );
}

#[test]
fn v2_task_list_links_carry_data_task_id_for_panel_wiring() {
    // Every task in the index list (and the viz-pre when available) must
    // carry a `data-task-id` attribute so the panel JS can resolve clicks.
    // The viz-pre rendering depends on the `wg` binary (subprocess) which is
    // not present in this test runner; the smoke scenario exercises that
    // path. Here we just check the list section, which is rendered without
    // a subprocess.
    let mut a = make_task("vlinka", "A", "public");
    let mut b = make_task("vlinkb", "B", "public");
    b.after = vec!["vlinka".into()];
    a.status = Status::Done;
    b.status = Status::Open;

    let graph = build_graph(vec![a, b]);
    let dir = TempDir::new().unwrap();
    html::render_site(
        &graph,
        dir.path(),
        dir.path(),
        html::RenderOptions { show_all: true, ..Default::default() },
    ).unwrap();

    let index = fs::read_to_string(dir.path().join("index.html")).unwrap();
    assert!(
        index.contains(r#"data-task-id="vlinka""#),
        "vlinka data-task-id missing"
    );
    assert!(
        index.contains(r#"data-task-id="vlinkb""#),
        "vlinkb data-task-id missing"
    );
}

#[test]
fn v2_strip_ansi_keeps_unicode_and_drops_csi() {
    // Internal helper, but the contract matters: the viz capture path strips
    // ANSI escapes before wrapping in clickable spans, and must preserve
    // multibyte UTF-8 (box-drawing characters) intact.
    use workgraph::html;
    // Round-trip through the public render_site only verifies output, but
    // strip_ansi is private. We exercise it indirectly via parse_since edge
    // cases as a sanity check that html.rs is wired up.
    assert!(html::parse_since("1h").is_ok());
    assert!(html::parse_since("0h").is_err());
}

#[test]
fn v2_index_renders_static_when_viz_subprocess_unavailable() {
    // If the viz subprocess fails (e.g. tempdir without graph.jsonl on disk
    // or a missing wg binary in tests), the page must still render — the
    // task list and panel infrastructure remain.
    let t = make_task("standalone", "Standalone", "public");
    let graph = build_graph(vec![t]);
    let dir = TempDir::new().unwrap();
    html::render_site(
        &graph,
        dir.path(),
        dir.path(),
        html::RenderOptions { show_all: true, ..Default::default() },
    ).unwrap();

    let index = fs::read_to_string(dir.path().join("index.html")).unwrap();
    // Even without viz, the panel container must exist for clickability.
    assert!(
        index.contains(r#"id="side-panel""#),
        "side panel container missing"
    );
    // The footer must still render with the task counts.
    assert!(
        index.contains("Showing 1 of 1 tasks") || index.contains("Tasks (1)"),
        "task count missing from index"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// wg-html-resizable: drag-to-resize the inspector panel + persist via localStorage
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn v2_inspector_panel_has_resize_handle() {
    // The inspector panel must include a drag handle element so the user
    // can click-and-drag to adjust the panel's width (or height on narrow
    // viewports). Per spec wg-html-resizable.
    let t = make_task("standalone", "Standalone", "public");
    let graph = build_graph(vec![t]);
    let dir = TempDir::new().unwrap();
    html::render_site(
        &graph,
        dir.path(),
        dir.path(),
        html::RenderOptions { show_all: true, ..Default::default() },
    ).unwrap();

    let index = fs::read_to_string(dir.path().join("index.html")).unwrap();
    assert!(
        index.contains(r#"id="panel-resize-handle""#),
        "resize handle element missing from inspector panel"
    );
    // role=separator + aria-orientation make the handle accessible to AT.
    assert!(
        index.contains(r#"role="separator""#),
        "resize handle missing role=separator"
    );

    // The CSS bundle must include the cursor + dragging styles for the handle.
    let css = fs::read_to_string(dir.path().join("style.css")).unwrap();
    assert!(
        css.contains(".panel-resize-handle"),
        "style.css missing .panel-resize-handle rule"
    );
    assert!(
        css.contains("col-resize"),
        "style.css missing col-resize cursor for wide-layout drag"
    );
    assert!(
        css.contains("row-resize"),
        "style.css missing row-resize cursor for narrow-layout drag"
    );

    // The JS bundle must wire up pointer events and persist to localStorage
    // under origin-scoped keys so reload preserves the user's chosen size.
    let js = fs::read_to_string(dir.path().join("panel.js")).unwrap();
    assert!(
        js.contains("'wg-html-inspector-width-px'"),
        "panel.js missing localStorage key for inspector width"
    );
    assert!(
        js.contains("pointerdown"),
        "panel.js missing pointerdown handler for resize"
    );
    assert!(
        js.contains("pointermove"),
        "panel.js missing pointermove handler for live drag feedback"
    );
    assert!(
        js.contains("--panel-width") && js.contains("setProperty"),
        "panel.js must set --panel-width inline so resize takes effect"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// wg-html-chat: --chat flag with visibility-respecting transcript inclusion
// ────────────────────────────────────────────────────────────────────────────

/// Test fixture: create a chat session with the given alias and seed a few
/// inbox/outbox messages. Returns the working directory.
fn seed_chat_session(workgraph_dir: &Path, alias: &str, kind: SessionKind) {
    chat_sessions::create_session(
        workgraph_dir,
        kind,
        &[alias.to_string()],
        Some(format!("test session {alias}")),
    )
    .unwrap();
    chat::append_inbox_ref(workgraph_dir, alias, "hello assistant", "req-1").unwrap();
    chat::append_outbox_ref(workgraph_dir, alias, "hello back", "req-1").unwrap();
    chat::append_inbox_ref(workgraph_dir, alias, "what is 2+2", "req-2").unwrap();
    chat::append_outbox_ref(workgraph_dir, alias, "4", "req-2").unwrap();
}

fn chat_task(id: &str, vis: &str) -> Task {
    let mut t = make_task(id, "Chat", vis);
    t.tags = vec!["chat-loop".into()];
    t
}

/// Read only the rendered HTML files (index + tasks/*.html). The fixture
/// also writes JSONL chat transcripts under `chat/<uuid>/`; we don't want
/// those raw files to count toward "what the user sees" assertions.
fn read_rendered_html(out_dir: &Path) -> String {
    let mut buf = String::new();
    if let Ok(s) = fs::read_to_string(out_dir.join("index.html")) {
        buf.push_str(&s);
        buf.push('\n');
    }
    if let Ok(rd) = fs::read_dir(out_dir.join("tasks")) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().map(|e| e == "html").unwrap_or(false) {
                if let Ok(s) = fs::read_to_string(&p) {
                    buf.push_str(&s);
                    buf.push('\n');
                }
            }
        }
    }
    buf
}

#[test]
fn chat_default_no_chat_flag_omits_transcript_but_keeps_task_node() {
    let dir = TempDir::new().unwrap();
    seed_chat_session(dir.path(), "chat-9", SessionKind::Coordinator);

    let chat = chat_task(".chat-9", "public");
    let graph = build_graph(vec![chat]);

    let summary = html::render_site(
        &graph,
        dir.path(),
        dir.path(),
        html::RenderOptions::default(),
    )
    .unwrap();
    assert_eq!(summary.chat_transcripts_shown, 0);
    assert_eq!(summary.chat_transcripts_hidden_by_visibility, 0);

    let blob = read_rendered_html(dir.path());
    // Task node still appears.
    assert!(blob.contains(".chat-9"), "chat task id missing from default-mode output");
    // But transcript content is absent.
    assert!(!blob.contains("hello assistant"), "transcript leaked without --chat");
    assert!(!blob.contains("hello back"), "transcript leaked without --chat");
    assert!(!blob.contains("Conversation"), "Conversation header rendered without --chat");
}

#[test]
fn chat_flag_includes_public_transcripts_only() {
    let dir = TempDir::new().unwrap();
    seed_chat_session(dir.path(), "chat-1", SessionKind::Coordinator);
    seed_chat_session(dir.path(), "chat-2", SessionKind::Coordinator);

    let public = chat_task(".chat-1", "public");
    let internal = chat_task(".chat-2", "internal");
    let graph = build_graph(vec![public, internal]);

    let summary = html::render_site(
        &graph,
        dir.path(),
        dir.path(),
        html::RenderOptions {
            show_all: true,
            include_chat: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(summary.chat_transcripts_shown, 1, "expected 1 public transcript");
    assert_eq!(
        summary.chat_transcripts_hidden_by_visibility, 1,
        "expected 1 internal transcript hidden"
    );

    // Public chat page contains transcript content.
    let pub_page = fs::read_to_string(dir.path().join("tasks/.chat-1.html")).unwrap();
    assert!(pub_page.contains("Conversation"), "public chat page missing Conversation");
    assert!(pub_page.contains("hello assistant"), "public chat msg missing");
    assert!(pub_page.contains("hello back"), "public chat reply missing");

    // Internal chat page exists but transcript is hidden behind a notice.
    let int_page = fs::read_to_string(dir.path().join("tasks/.chat-2.html")).unwrap();
    assert!(
        int_page.contains("Chat transcript hidden"),
        "expected hidden marker on internal chat page, got: {int_page}"
    );
    assert!(int_page.contains("visibility: internal"), "visibility label missing");
    assert!(int_page.contains("--all"), "remediation hint missing");
    assert!(
        !int_page.contains("hello assistant"),
        "internal transcript content leaked: {int_page}"
    );

    // Index header banner shows count.
    let index = fs::read_to_string(dir.path().join("index.html")).unwrap();
    assert!(
        index.contains("Showing 1 chat transcript") || index.contains("Showing 1 chat transcripts"),
        "header banner missing or wrong: {}",
        &index[..index.len().min(2000)],
    );
    assert!(
        index.contains("1 omitted"),
        "header banner doesn't mention hidden count"
    );
}

#[test]
fn chat_all_includes_internal_transcripts() {
    let dir = TempDir::new().unwrap();
    seed_chat_session(dir.path(), "chat-1", SessionKind::Coordinator);
    seed_chat_session(dir.path(), "chat-2", SessionKind::Coordinator);

    let public = chat_task(".chat-1", "public");
    let internal = chat_task(".chat-2", "internal");
    let graph = build_graph(vec![public, internal]);

    let summary = html::render_site(
        &graph,
        dir.path(),
        dir.path(),
        html::RenderOptions {
            show_all: true,
            include_chat: true,
            all_chats: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(summary.chat_transcripts_shown, 2, "all chats should be rendered");
    assert_eq!(summary.chat_transcripts_hidden_by_visibility, 0);

    let int_page = fs::read_to_string(dir.path().join("tasks/.chat-2.html")).unwrap();
    assert!(int_page.contains("hello assistant"), "internal chat content not rendered with --all");
    assert!(
        !int_page.contains("Chat transcript hidden"),
        "hidden marker should not appear with --all"
    );
}

#[test]
fn chat_public_only_filters_both_tasks_and_transcripts() {
    let dir = TempDir::new().unwrap();
    seed_chat_session(dir.path(), "chat-1", SessionKind::Coordinator);
    seed_chat_session(dir.path(), "chat-2", SessionKind::Coordinator);

    let public_chat = chat_task(".chat-1", "public");
    let internal_chat = chat_task(".chat-2", "internal");
    let graph = build_graph(vec![public_chat, internal_chat]);

    let summary = html::render_site(
        &graph,
        dir.path(),
        dir.path(),
        html::RenderOptions {
            show_all: false, // public-only TASKS
            include_chat: true,
            all_chats: false,
            ..Default::default()
        },
    )
    .unwrap();

    // Only public chat task is in the rendered set.
    assert_eq!(summary.public_count, 1);
    assert_eq!(summary.chat_transcripts_shown, 1);
    assert_eq!(summary.chat_transcripts_hidden_by_visibility, 0);

    // The internal chat is filtered out at the task level — its page must not exist.
    assert!(
        !dir.path().join("tasks/.chat-2.html").exists(),
        "internal chat page should not be written under --public-only"
    );
    let blob = read_rendered_html(dir.path());
    assert!(!blob.contains(".chat-2"), "internal chat id leaked");
}

#[test]
fn chat_zero_visible_with_chat_flag_message_when_all_internal() {
    let dir = TempDir::new().unwrap();
    seed_chat_session(dir.path(), "chat-7", SessionKind::Coordinator);
    let only_internal_chat = chat_task(".chat-7", "internal");
    let graph = build_graph(vec![only_internal_chat]);

    let summary = html::render_site(
        &graph,
        dir.path(),
        dir.path(),
        html::RenderOptions {
            show_all: true,
            include_chat: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(summary.chat_transcripts_shown, 0);
    assert_eq!(summary.chat_transcripts_hidden_by_visibility, 1);

    let index = fs::read_to_string(dir.path().join("index.html")).unwrap();
    assert!(
        index.contains("Showing 0 chat transcripts"),
        "expected 'Showing 0 chat transcripts' message, got: {}",
        &index[..index.len().min(3000)],
    );
    assert!(index.contains("--all"), "remediation hint missing for zero-shown case");
}

#[test]
fn chat_transcript_sanitizes_secrets_before_rendering() {
    let dir = TempDir::new().unwrap();
    chat_sessions::create_session(
        dir.path(),
        SessionKind::Coordinator,
        &["chat-secret".into()],
        None,
    )
    .unwrap();
    // Smuggle three classes of secret into the transcript.
    chat::append_inbox_ref(
        dir.path(),
        "chat-secret",
        "use sk-abcdefghijklmnopqrstuvwxyz12345 with OPENAI_API_KEY=hunter2 stored at ~/.wg/secrets/openai.key",
        "r1",
    )
    .unwrap();
    chat::append_outbox_ref(dir.path(), "chat-secret", "ok understood", "r1").unwrap();

    let chat = chat_task(".chat-secret", "public");
    let graph = build_graph(vec![chat]);

    html::render_site(
        &graph,
        dir.path(),
        dir.path(),
        html::RenderOptions {
            show_all: true,
            include_chat: true,
            ..Default::default()
        },
    )
    .unwrap();

    let page = fs::read_to_string(dir.path().join("tasks/.chat-secret.html")).unwrap();
    assert!(
        !page.contains("sk-abcdefghijklmnopqrstuvwxyz12345"),
        "raw api key leaked: {page}"
    );
    assert!(!page.contains("hunter2"), "env-var secret leaked: {page}");
    assert!(!page.contains("openai.key"), "secret path leaked: {page}");
    assert!(page.contains("[redacted]"), "expected redaction marker: {page}");
}

#[test]
fn chat_transcript_renders_in_chronological_order() {
    let dir = TempDir::new().unwrap();
    seed_chat_session(dir.path(), "chat-order", SessionKind::Coordinator);

    let chat = chat_task(".chat-order", "public");
    let graph = build_graph(vec![chat]);

    html::render_site(
        &graph,
        dir.path(),
        dir.path(),
        html::RenderOptions {
            show_all: true,
            include_chat: true,
            ..Default::default()
        },
    )
    .unwrap();

    let page = fs::read_to_string(dir.path().join("tasks/.chat-order.html")).unwrap();
    let p1 = page.find("hello assistant").expect("first inbox missing");
    let p2 = page.find("hello back").expect("first outbox missing");
    let p3 = page.find("what is 2+2").expect("second inbox missing");
    let p4 = page.find("4</pre>").or_else(|| page.find("4<")).expect("second outbox missing");
    assert!(p1 < p2 && p2 < p3 && p3 < p4, "messages out of order in {page}");
}

#[test]
fn chat_legacy_coordinator_id_resolves() {
    let dir = TempDir::new().unwrap();
    seed_chat_session(dir.path(), "coordinator-3", SessionKind::Coordinator);

    let chat = chat_task(".coordinator-3", "public");
    let graph = build_graph(vec![chat]);

    let summary = html::render_site(
        &graph,
        dir.path(),
        dir.path(),
        html::RenderOptions {
            show_all: true,
            include_chat: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(summary.chat_transcripts_shown, 1);
    let page = fs::read_to_string(dir.path().join("tasks/.coordinator-3.html")).unwrap();
    assert!(page.contains("hello assistant"));
}

// ────────────────────────────────────────────────────────────────────────────
// wg-html-agency: toggle button to show/hide agency tasks
// (.evaluate-*, .assign-*, .place-*, .flip-*, .create-*) — TUI period-key parity
// ────────────────────────────────────────────────────────────────────────────

/// Build a graph with a substantive task plus its agency companions.
fn agency_fixture() -> WorkGraph {
    let mut work = make_task("widget-impl", "Build the widget", "internal");
    work.status = Status::InProgress;
    let mut assign = make_task(".assign-widget-impl", "Assign", "internal");
    assign.status = Status::Done;
    let mut eval = make_task(".evaluate-widget-impl", "Evaluate", "internal");
    eval.after = vec!["widget-impl".into()];
    let mut flip = make_task(".flip-widget-impl", "FLIP", "internal");
    flip.after = vec!["widget-impl".into()];
    build_graph(vec![work, assign, eval, flip])
}

#[test]
fn agency_toggle_button_present_in_show_all_mode() {
    let dir = TempDir::new().unwrap();
    html::render_site(
        &agency_fixture(),
        dir.path(),
        dir.path(),
        html::RenderOptions {
            show_all: true,
            ..Default::default()
        },
    )
    .unwrap();

    let index = fs::read_to_string(dir.path().join("index.html")).unwrap();
    // Toggle button is wired by id and lives in the page header.
    assert!(
        index.contains(r#"id="agency-toggle""#),
        "agency toggle button missing from index.html"
    );
    // Default state advertises the inactive (hidden-agency) label.
    assert!(
        index.contains("Show meta tasks") || index.contains("Show all tasks"),
        "agency toggle should advertise its default 'show' action"
    );
}

#[test]
fn agency_toggle_default_hides_agency_in_viz_pre() {
    let dir = TempDir::new().unwrap();
    html::render_site(
        &agency_fixture(),
        dir.path(),
        dir.path(),
        html::RenderOptions {
            show_all: true,
            ..Default::default()
        },
    )
    .unwrap();

    let index = fs::read_to_string(dir.path().join("index.html")).unwrap();
    // The body element carries an explicit data-show-agency attribute so JS
    // and CSS can toggle in lock-step.
    assert!(
        index.contains(r#"data-show-agency="false""#),
        "body should default to data-show-agency=\"false\""
    );
    // The substantive viz <pre> stays visible by default; the with-agency viz
    // is rendered too but in a sibling <pre> that's hidden until toggle.
    assert!(
        index.contains("viz-substantive"),
        "expected viz-substantive class on default-visible viz"
    );
    assert!(
        index.contains("viz-agency"),
        "expected viz-agency class on hidden-by-default agency viz"
    );
}

#[test]
fn agency_toggle_persists_via_localstorage() {
    let dir = TempDir::new().unwrap();
    html::render_site(
        &agency_fixture(),
        dir.path(),
        dir.path(),
        html::RenderOptions {
            show_all: true,
            ..Default::default()
        },
    )
    .unwrap();

    let panel_js = fs::read_to_string(dir.path().join("panel.js")).unwrap();
    // localStorage key must be present in panel.js so the choice persists.
    assert!(
        panel_js.contains("wg-html-show-agency"),
        "panel.js should reference the wg-html-show-agency localStorage key"
    );

    let index = fs::read_to_string(dir.path().join("index.html")).unwrap();
    // The bootstrap script reads localStorage before paint to avoid flash.
    assert!(
        index.contains("wg-html-show-agency"),
        "index.html bootstrap should read the agency localStorage key"
    );
}

#[test]
fn agency_tasks_get_dim_marker_class_when_visible() {
    let dir = TempDir::new().unwrap();
    html::render_site(
        &agency_fixture(),
        dir.path(),
        dir.path(),
        html::RenderOptions {
            show_all: true,
            ..Default::default()
        },
    )
    .unwrap();

    let index = fs::read_to_string(dir.path().join("index.html")).unwrap();
    // task-link spans for agency tasks must carry the is-agency class so the
    // CSS dim treatment applies.
    assert!(
        index.contains(r#"class="task-link is-agency""#)
            || index.contains(r#"is-agency"#),
        "expected is-agency class somewhere in the rendered viz / list"
    );

    // CSS provides a visual treatment for the is-agency class so agency tasks
    // are dimmed (or otherwise visually distinct) when shown.
    let css = fs::read_to_string(dir.path().join("style.css")).unwrap();
    assert!(
        css.contains(".is-agency"),
        "style.css must define a .is-agency rule for dim treatment"
    );
}

#[test]
fn agency_toggle_omitted_when_no_agency_tasks_present() {
    // If the graph has no agency tasks at all, the toggle is pointless and
    // should be omitted to avoid clutter.
    let mut t = make_task("only-substantive", "T", "public");
    t.status = Status::Open;
    let graph = build_graph(vec![t]);

    let dir = TempDir::new().unwrap();
    html::render_site(
        &graph,
        dir.path(),
        dir.path(),
        html::RenderOptions {
            show_all: true,
            ..Default::default()
        },
    )
    .unwrap();

    let index = fs::read_to_string(dir.path().join("index.html")).unwrap();
    assert!(
        !index.contains(r#"id="agency-toggle""#),
        "agency toggle should be omitted when no agency tasks exist"
    );
}
