use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use anyhow::Result;
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;

use crate::commands::viz::{VizOptions, VizOutput};
use workgraph::graph::{Status, TokenUsage, format_tokens, parse_token_usage_live};
use workgraph::parser::load_graph;
use workgraph::{AgentRegistry, AgentStatus};

/// Loaded detail for the HUD panel showing info about the selected task.
#[derive(Default)]
pub struct HudDetail {
    /// Task ID this detail was loaded for (to detect stale data).
    pub task_id: String,
    /// All content lines assembled for rendering (with section headers).
    pub rendered_lines: Vec<String>,
}

/// Task status counts for the status bar.
#[derive(Default)]
pub struct TaskCounts {
    pub total: usize,
    pub done: usize,
    pub open: usize,
    pub in_progress: usize,
    pub failed: usize,
    pub blocked: usize,
}

/// A single fuzzy match result for a line.
pub struct FuzzyLineMatch {
    /// Index into the original `lines`/`plain_lines` arrays.
    pub line_idx: usize,
    /// Fuzzy match score (higher = better). Used for sorting/ranking.
    #[allow(dead_code)]
    pub score: i64,
    /// Character positions within the plain line where the match occurs.
    /// These are *char* indices (not byte indices).
    pub char_positions: Vec<usize>,
}

/// Main application state for the viz viewer.
pub struct VizApp {
    /// Path to the workgraph directory.
    pub workgraph_dir: PathBuf,
    /// Viz options passed from CLI (--all, --status, --critical-path, etc.).
    viz_options: VizOptions,
    /// Whether the app should quit on next loop iteration.
    pub should_quit: bool,

    // ── Viz content ──
    /// Raw lines from `wg viz` output (may contain ANSI color codes).
    pub lines: Vec<String>,
    /// Stripped lines (no ANSI) for search matching and width calculation.
    pub plain_lines: Vec<String>,
    /// Sanitized lines for search — box-drawing/arrow chars replaced with spaces.
    search_lines: Vec<String>,
    /// Maximum line width in plain content (for horizontal scroll bounds).
    pub max_line_width: usize,

    // ── Viewport scroll ──
    pub scroll: ViewportScroll,

    // ── Search / Filter ──
    /// Whether the user is currently typing a search query.
    pub search_active: bool,
    /// The current search input buffer.
    pub search_input: String,
    /// Lines that fuzzy-match the current query, with scores and positions.
    pub fuzzy_matches: Vec<FuzzyLineMatch>,
    /// Index into `fuzzy_matches` for the currently focused match.
    pub current_match: Option<usize>,
    /// When filter is active, indices of original lines that are visible.
    /// `None` means show all lines (no filter).
    pub filtered_indices: Option<Vec<usize>>,
    /// The fuzzy matcher instance (reused across searches).
    matcher: SkimMatcherV2,

    // ── Task stats ──
    pub task_counts: TaskCounts,
    /// Aggregate token usage across all tasks in the graph.
    pub total_usage: TokenUsage,
    /// Per-task token usage keyed by task ID (for computing visible-task totals).
    pub task_token_map: HashMap<String, TokenUsage>,

    // ── Token display toggle ──
    /// When true, show total workgraph token usage; when false, show visible-tasks only.
    pub show_total_tokens: bool,

    // ── Help overlay ──
    pub show_help: bool,

    // ── Mouse capture ──
    /// Whether mouse capture is currently enabled.
    pub mouse_enabled: bool,

    // ── Jump target (transient highlight after Enter) ──
    /// After pressing Enter on a search match, stores (original_line_index, when_set).
    /// Render code applies a transient yellow highlight that fades after ~2 seconds.
    pub jump_target: Option<(usize, Instant)>,

    // ── Task selection / edge tracing ──
    /// Ordered list of task IDs as they appear in the viz output (top to bottom).
    pub task_order: Vec<String>,
    /// Map from task ID to its line index in the viz output.
    pub node_line_map: HashMap<String, usize>,
    /// Forward edges: task_id → dependent task IDs.
    pub forward_edges: HashMap<String, Vec<String>>,
    /// Reverse edges: task_id → dependency task IDs.
    pub reverse_edges: HashMap<String, Vec<String>>,
    /// Currently selected task index into `task_order`.
    pub selected_task_idx: Option<usize>,
    /// Whether edge trace highlighting is visible (toggled by Tab).
    pub trace_visible: bool,
    /// Transitive upstream (dependency) task IDs of the selected task.
    pub upstream_set: HashSet<String>,
    /// Transitive downstream (dependent) task IDs of the selected task.
    pub downstream_set: HashSet<String>,
    /// Per-character edge map: (line, visible_column) → list of (source_id, target_id).
    /// Maps edge/connector characters to the graph edge(s) they represent.
    /// Shared arc column positions may carry multiple edges.
    pub char_edge_map: std::collections::HashMap<(usize, usize), Vec<(String, String)>>,
    /// Cycle membership from VizOutput: task_id → set of SCC members.
    cycle_members: HashMap<String, HashSet<String>>,
    /// Set of task IDs in the same SCC as the currently selected task.
    /// Empty if the selected task is not in any cycle.
    pub cycle_set: HashSet<String>,

    // ── HUD (info panel) ──
    /// Loaded HUD detail for the currently selected task.
    pub hud_detail: Option<HudDetail>,
    /// Scroll offset within the HUD panel (vertical).
    pub hud_scroll: usize,

    // ── Live refresh ──
    /// Last observed modification time of graph.jsonl.
    last_graph_mtime: Option<SystemTime>,
    /// Monotonic instant of last data refresh.
    pub last_refresh: Instant,
    /// Display string for last refresh time (HH:MM:SS).
    pub last_refresh_display: String,
    /// Refresh interval.
    refresh_interval: std::time::Duration,
}

/// Scroll state for a 2D viewport.
pub struct ViewportScroll {
    /// First visible line index (vertical offset into the visible set).
    pub offset_y: usize,
    /// First visible column index (horizontal offset).
    pub offset_x: usize,
    /// Total content height in lines (filtered count when filter active).
    pub content_height: usize,
    /// Total content width in columns.
    pub content_width: usize,
    /// Viewport height (set each frame from terminal size).
    pub viewport_height: usize,
    /// Viewport width (set each frame from terminal size).
    pub viewport_width: usize,
}

impl VizApp {
    /// Create a new VizApp.
    ///
    /// `mouse_override`: `Some(false)` forces mouse off (--no-mouse),
    /// `None` means auto-detect (disable in tmux split panes).
    pub fn new(
        workgraph_dir: PathBuf,
        viz_options: VizOptions,
        mouse_override: Option<bool>,
    ) -> Self {
        let mouse_enabled = match mouse_override {
            Some(v) => v,
            None => !detect_tmux_split(),
        };
        let graph_mtime = std::fs::metadata(workgraph_dir.join("graph.jsonl"))
            .and_then(|m| m.modified())
            .ok();
        let mut app = Self {
            workgraph_dir,
            viz_options,
            should_quit: false,
            lines: Vec::new(),
            plain_lines: Vec::new(),
            search_lines: Vec::new(),
            max_line_width: 0,
            scroll: ViewportScroll::new(),
            search_active: false,
            search_input: String::new(),
            fuzzy_matches: Vec::new(),
            current_match: None,
            filtered_indices: None,
            matcher: SkimMatcherV2::default(),
            task_counts: TaskCounts::default(),
            total_usage: TokenUsage {
                cost_usd: 0.0,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
            },
            task_token_map: HashMap::new(),
            show_total_tokens: false,
            show_help: false,
            mouse_enabled,
            jump_target: None,
            task_order: Vec::new(),
            node_line_map: HashMap::new(),
            forward_edges: HashMap::new(),
            reverse_edges: HashMap::new(),
            selected_task_idx: None,
            trace_visible: true,
            upstream_set: HashSet::new(),
            downstream_set: HashSet::new(),
            char_edge_map: std::collections::HashMap::new(),
            cycle_members: HashMap::new(),
            cycle_set: HashSet::new(),
            hud_detail: None,
            hud_scroll: 0,
            last_graph_mtime: graph_mtime,
            last_refresh: Instant::now(),
            last_refresh_display: chrono::Local::now().format("%H:%M:%S").to_string(),
            refresh_interval: std::time::Duration::from_millis(1500),
        };
        app.load_viz();
        app.load_stats();
        app
    }

    /// Load viz output by calling the viz module directly.
    pub fn load_viz(&mut self) {
        match self.generate_viz() {
            Ok(viz_output) => {
                self.lines = viz_output
                    .text
                    .lines()
                    .map(String::from)
                    .filter(|l| {
                        let stripped = String::from_utf8(strip_ansi_escapes::strip(l.as_bytes()))
                            .unwrap_or_default();
                        !stripped.trim_start().starts_with("Legend:")
                    })
                    .collect();
                self.plain_lines = self
                    .lines
                    .iter()
                    .map(|l| {
                        String::from_utf8(strip_ansi_escapes::strip(l.as_bytes()))
                            .unwrap_or_default()
                    })
                    .collect();
                self.search_lines = self
                    .plain_lines
                    .iter()
                    .map(|l| sanitize_for_search(l))
                    .collect();
                self.max_line_width = self.plain_lines.iter().map(|l| l.len()).max().unwrap_or(0);

                // Store graph metadata for interactive edge tracing.
                self.node_line_map = viz_output.node_line_map;
                self.task_order = viz_output.task_order;
                self.forward_edges = viz_output.forward_edges;
                self.reverse_edges = viz_output.reverse_edges;
                self.char_edge_map = viz_output.char_edge_map;
                self.cycle_members = viz_output.cycle_members;

                // Preserve selection if possible (e.g., after refresh).
                if let Some(idx) = self.selected_task_idx {
                    if idx >= self.task_order.len() {
                        self.selected_task_idx = if self.task_order.is_empty() {
                            None
                        } else {
                            Some(self.task_order.len() - 1)
                        };
                    }
                } else if !self.task_order.is_empty() {
                    // Default to first task on initial load.
                    self.selected_task_idx = Some(0);
                }
                self.recompute_trace();

                self.update_scroll_bounds();
            }
            Err(_) => {
                self.lines = vec!["(error loading graph)".to_string()];
                self.plain_lines = self.lines.clone();
                self.search_lines = self.plain_lines.clone();
                self.max_line_width = self.lines[0].len();
                self.task_order.clear();
                self.node_line_map.clear();
                self.forward_edges.clear();
                self.reverse_edges.clear();
                self.selected_task_idx = None;
                self.upstream_set.clear();
                self.downstream_set.clear();
                self.char_edge_map.clear();
                self.cycle_members.clear();
                self.cycle_set.clear();
                self.update_scroll_bounds();
            }
        }
    }

    fn generate_viz(&self) -> Result<VizOutput> {
        crate::commands::viz::generate_viz_output(&self.workgraph_dir, &self.viz_options)
    }

    /// Update scroll content bounds based on current filter state.
    pub fn update_scroll_bounds(&mut self) {
        let height = match &self.filtered_indices {
            Some(indices) => indices.len(),
            None => self.lines.len(),
        };
        self.scroll.content_height = height;
        self.scroll.content_width = self.max_line_width;
        self.scroll.clamp();
    }

    /// Get the number of visible lines (filtered or all).
    pub fn visible_line_count(&self) -> usize {
        match &self.filtered_indices {
            Some(indices) => indices.len(),
            None => self.lines.len(),
        }
    }

    /// Map a visible row index to an original line index.
    pub fn visible_to_original(&self, visible_idx: usize) -> usize {
        match &self.filtered_indices {
            Some(indices) => indices.get(visible_idx).copied().unwrap_or(0),
            None => visible_idx,
        }
    }

    /// Map an original line index to its position in the visible set.
    fn original_to_visible(&self, orig_idx: usize) -> Option<usize> {
        match &self.filtered_indices {
            Some(indices) => indices.iter().position(|&i| i == orig_idx),
            None => {
                if orig_idx < self.lines.len() {
                    Some(orig_idx)
                } else {
                    None
                }
            }
        }
    }

    // ── Task selection / edge tracing ──

    /// Move task selection to the previous task in the viz order.
    /// Does NOT wrap around — stays at top when already at first task.
    pub fn select_prev_task(&mut self) {
        if self.task_order.is_empty() {
            return;
        }
        let idx = match self.selected_task_idx {
            Some(0) => return, // already at top, do nothing
            Some(i) => i - 1,
            None => 0,
        };
        self.selected_task_idx = Some(idx);
        self.recompute_trace();
        self.scroll_to_selected_task();
    }

    /// Move task selection to the next task in the viz order.
    /// Does NOT wrap around — stays at bottom when already at last task.
    pub fn select_next_task(&mut self) {
        if self.task_order.is_empty() {
            return;
        }
        let idx = match self.selected_task_idx {
            Some(i) if i + 1 >= self.task_order.len() => return, // already at bottom, do nothing
            Some(i) => i + 1,
            None => 0,
        };
        self.selected_task_idx = Some(idx);
        self.recompute_trace();
        self.scroll_to_selected_task();
    }

    /// Select the first task in the viz order.
    pub fn select_first_task(&mut self) {
        if self.task_order.is_empty() {
            return;
        }
        self.selected_task_idx = Some(0);
        self.recompute_trace();
        self.scroll_to_selected_task();
    }

    /// Select the last task in the viz order.
    pub fn select_last_task(&mut self) {
        if self.task_order.is_empty() {
            return;
        }
        self.selected_task_idx = Some(self.task_order.len() - 1);
        self.recompute_trace();
        self.scroll_to_selected_task();
    }

    /// Recompute the transitive upstream/downstream sets and line mappings
    /// based on the currently selected task.
    pub fn recompute_trace(&mut self) {
        self.upstream_set.clear();
        self.downstream_set.clear();
        self.cycle_set.clear();

        let selected_id = match self.selected_task_idx {
            Some(idx) => match self.task_order.get(idx) {
                Some(id) => id.clone(),
                None => return,
            },
            None => return,
        };

        // Compute transitive upstream (dependencies) via BFS on reverse_edges.
        {
            let mut queue = std::collections::VecDeque::new();
            for dep in self.reverse_edges.get(&selected_id).into_iter().flatten() {
                if self.upstream_set.insert(dep.clone()) {
                    queue.push_back(dep.clone());
                }
            }
            while let Some(id) = queue.pop_front() {
                for dep in self.reverse_edges.get(&id).into_iter().flatten() {
                    if self.upstream_set.insert(dep.clone()) {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        // Compute transitive downstream (dependents) via BFS on forward_edges.
        {
            let mut queue = std::collections::VecDeque::new();
            for dep in self.forward_edges.get(&selected_id).into_iter().flatten() {
                if self.downstream_set.insert(dep.clone()) {
                    queue.push_back(dep.clone());
                }
            }
            while let Some(id) = queue.pop_front() {
                for dep in self.forward_edges.get(&id).into_iter().flatten() {
                    if self.downstream_set.insert(dep.clone()) {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        // Compute cycle membership for the selected task.
        if let Some(members) = self.cycle_members.get(&selected_id) {
            self.cycle_set = members.clone();
        }

        // Invalidate HUD so it reloads for the new selection.
        self.invalidate_hud();
    }

    /// Scroll the viewport so the selected task is visible.
    fn scroll_to_selected_task(&mut self) {
        let task_id = match self.selected_task_idx.and_then(|i| self.task_order.get(i)) {
            Some(id) => id,
            None => return,
        };
        let orig_line = match self.node_line_map.get(task_id) {
            Some(&line) => line,
            None => return,
        };
        if let Some(visible_pos) = self.original_to_visible(orig_line)
            && (visible_pos < self.scroll.offset_y
                || visible_pos >= self.scroll.offset_y + self.scroll.viewport_height)
        {
            let half = self.scroll.viewport_height / 2;
            self.scroll.offset_y = visible_pos.saturating_sub(half);
            self.scroll.clamp();
        }
    }

    /// Select the task at the given original line index, if any.
    /// Returns true if a task was found and selected.
    pub fn select_task_at_line(&mut self, orig_line: usize) -> bool {
        // Reverse lookup: find which task_id lives at this line.
        let task_id = self
            .node_line_map
            .iter()
            .find(|&(_, line)| *line == orig_line)
            .map(|(id, _)| id.clone());
        let task_id = match task_id {
            Some(id) => id,
            None => return false,
        };
        // Find its index in task_order.
        let idx = match self.task_order.iter().position(|id| *id == task_id) {
            Some(i) => i,
            None => return false,
        };
        self.selected_task_idx = Some(idx);
        self.recompute_trace();
        true
    }

    /// Get the currently selected task ID, if any.
    pub fn selected_task_id(&self) -> Option<&str> {
        self.selected_task_idx
            .and_then(|i| self.task_order.get(i))
            .map(|s| s.as_str())
    }

    // ── Search ──

    /// Called on every keystroke while search is active.
    /// Performs incremental fuzzy matching and updates the filter.
    pub fn update_search(&mut self) {
        let query = &self.search_input;
        if query.is_empty() {
            self.fuzzy_matches.clear();
            self.current_match = None;
            self.filtered_indices = None;
            self.update_scroll_bounds();
            return;
        }

        // Run fuzzy matching on sanitized lines (box-drawing chars stripped).
        self.fuzzy_matches.clear();
        for (i, search_line) in self.search_lines.iter().enumerate() {
            if let Some((score, indices)) = self.matcher.fuzzy_indices(search_line, query) {
                // `indices` are byte positions — convert to char positions.
                let char_positions = byte_positions_to_char_positions(search_line, &indices);
                self.fuzzy_matches.push(FuzzyLineMatch {
                    line_idx: i,
                    score,
                    char_positions,
                });
            }
        }

        // Sort by score descending for match navigation order.
        // But keep original line order for the match index (navigate top-to-bottom).
        // fuzzy_matches are already in line order since we iterate lines sequentially.

        // Build filtered view: matching lines + their tree ancestors + section context.
        self.filtered_indices = Some(compute_filtered_indices(
            &self.plain_lines,
            &self.fuzzy_matches,
        ));

        self.update_scroll_bounds();

        // Set current match to the first match.
        if !self.fuzzy_matches.is_empty() {
            self.current_match = Some(0);
            self.scroll_to_current_match();
        } else {
            self.current_match = None;
        }
    }

    /// Accept the current search (Enter key). Exit search mode, show all lines,
    /// keep match highlights and viewport position (vim-style search).
    pub fn accept_search(&mut self) {
        self.search_active = false;
        self.filtered_indices = None;
        self.update_scroll_bounds();
        // Keep search_input, fuzzy_matches, current_match for highlights + navigation.
    }

    /// Accept search and jump to the current match with a transient highlight.
    /// Called when the user presses Enter on a search match.
    pub fn accept_search_and_jump(&mut self) {
        // Capture the current match's original line index before clearing filter.
        let target_line = self.current_match_line();
        self.accept_search();

        if let Some(orig_line) = target_line {
            // Set the transient highlight target.
            self.jump_target = Some((orig_line, Instant::now()));

            // Scroll to center on the target line in the full (unfiltered) view.
            let half = self.scroll.viewport_height / 2;
            self.scroll.offset_y = orig_line.saturating_sub(half);
            self.scroll.clamp();
        }
    }

    /// Jump to the next search match.
    pub fn next_match(&mut self) {
        if self.fuzzy_matches.is_empty() {
            return;
        }
        let next = match self.current_match {
            Some(idx) => (idx + 1) % self.fuzzy_matches.len(),
            None => 0,
        };
        self.current_match = Some(next);
        self.scroll_to_current_match();
    }

    /// Jump to the previous search match.
    pub fn prev_match(&mut self) {
        if self.fuzzy_matches.is_empty() {
            return;
        }
        let prev = match self.current_match {
            Some(0) => self.fuzzy_matches.len() - 1,
            Some(idx) => idx - 1,
            None => self.fuzzy_matches.len() - 1,
        };
        self.current_match = Some(prev);
        self.scroll_to_current_match();
    }

    /// Clear the search state entirely, restoring the full unfiltered view.
    pub fn clear_search(&mut self) {
        self.search_active = false;
        self.search_input.clear();
        self.fuzzy_matches.clear();
        self.current_match = None;
        self.filtered_indices = None;
        self.update_scroll_bounds();
    }

    /// Return a human-readable search status string for the status bar.
    pub fn search_status(&self) -> String {
        if self.search_active {
            if self.search_input.is_empty() {
                "/".to_string()
            } else if self.fuzzy_matches.is_empty() {
                format!("/{} [no matches]", self.search_input)
            } else {
                let idx = self.current_match.unwrap_or(0);
                format!(
                    "/{} [{}/{}]",
                    self.search_input,
                    idx + 1,
                    self.fuzzy_matches.len()
                )
            }
        } else if !self.search_input.is_empty() && !self.fuzzy_matches.is_empty() {
            // Accepted search — highlights visible, navigating with n/N/Tab.
            let idx = self.current_match.unwrap_or(0);
            format!(
                "/{} [{}/{}]",
                self.search_input,
                idx + 1,
                self.fuzzy_matches.len()
            )
        } else {
            String::new()
        }
    }

    /// Check if any search/filter is active (for rendering decisions).
    pub fn has_active_search(&self) -> bool {
        !self.search_input.is_empty() && !self.fuzzy_matches.is_empty()
    }

    /// Get the fuzzy match info for an original line index, if any.
    pub fn match_for_line(&self, orig_idx: usize) -> Option<&FuzzyLineMatch> {
        self.fuzzy_matches.iter().find(|m| m.line_idx == orig_idx)
    }

    /// Get the original line index of the current match (for highlight).
    pub fn current_match_line(&self) -> Option<usize> {
        self.current_match
            .and_then(|idx| self.fuzzy_matches.get(idx))
            .map(|m| m.line_idx)
    }

    /// Scroll the viewport so the current match is visible (centered).
    fn scroll_to_current_match(&mut self) {
        if let Some(match_idx) = self.current_match {
            let orig_line = self.fuzzy_matches[match_idx].line_idx;
            // Convert to visible position.
            if let Some(visible_pos) = self.original_to_visible(orig_line)
                && (visible_pos < self.scroll.offset_y
                    || visible_pos >= self.scroll.offset_y + self.scroll.viewport_height)
            {
                let half = self.scroll.viewport_height / 2;
                self.scroll.offset_y = visible_pos.saturating_sub(half);
                self.scroll.clamp();
            }
        }
    }

    // ── Refresh ──

    /// Re-run search on new content after a graph refresh.
    fn rerun_search(&mut self) {
        if self.search_input.is_empty() {
            return;
        }
        // Re-run the fuzzy match with the current query.
        self.fuzzy_matches.clear();
        for (i, search_line) in self.search_lines.iter().enumerate() {
            if let Some((score, indices)) =
                self.matcher.fuzzy_indices(search_line, &self.search_input)
            {
                let char_positions = byte_positions_to_char_positions(search_line, &indices);
                self.fuzzy_matches.push(FuzzyLineMatch {
                    line_idx: i,
                    score,
                    char_positions,
                });
            }
        }
        if self.search_active {
            self.filtered_indices = Some(compute_filtered_indices(
                &self.plain_lines,
                &self.fuzzy_matches,
            ));
        }
        self.update_scroll_bounds();
        // Try to preserve current match position.
        if !self.fuzzy_matches.is_empty() {
            if self.current_match.is_none()
                || self.current_match.unwrap() >= self.fuzzy_matches.len()
            {
                self.current_match = Some(0);
            }
        } else {
            self.current_match = None;
        }
    }

    /// Load task counts and token usage from the graph + live agent output.
    pub fn load_stats(&mut self) {
        let graph_path = self.workgraph_dir.join("graph.jsonl");
        let graph = match load_graph(&graph_path) {
            Ok(g) => g,
            Err(_) => {
                self.task_counts = TaskCounts::default();
                self.total_usage = TokenUsage {
                    cost_usd: 0.0,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_input_tokens: 0,
                    cache_creation_input_tokens: 0,
                };
                self.task_token_map.clear();
                return;
            }
        };

        let mut counts = TaskCounts::default();
        let mut total_usage = TokenUsage {
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
        };
        let mut task_token_map: HashMap<String, TokenUsage> = HashMap::new();

        // Build a map of agent_id -> live token usage for in-progress agents
        let mut live_agent_usage: HashMap<String, TokenUsage> = HashMap::new();
        if let Ok(registry) = AgentRegistry::load(&self.workgraph_dir) {
            for (id, agent) in &registry.agents {
                if agent.status != AgentStatus::Working || agent.output_file.is_empty() {
                    continue;
                }
                let path = std::path::Path::new(&agent.output_file);
                if let Some(usage) = parse_token_usage_live(path) {
                    live_agent_usage.insert(id.clone(), usage);
                }
            }
        }

        for task in graph.tasks() {
            counts.total += 1;
            match task.status {
                Status::Done => counts.done += 1,
                Status::Open => counts.open += 1,
                Status::InProgress => counts.in_progress += 1,
                Status::Failed => counts.failed += 1,
                Status::Blocked => counts.blocked += 1,
                Status::Abandoned => counts.done += 1, // count with done
            }

            // Use stored token_usage if available, otherwise check live agent data
            let usage = task.token_usage.as_ref().or_else(|| {
                task.assigned
                    .as_ref()
                    .and_then(|aid| live_agent_usage.get(aid))
            });

            if let Some(usage) = usage {
                total_usage.accumulate(usage);
                task_token_map.insert(task.id.clone(), usage.clone());
            }
        }

        self.task_counts = counts;
        self.total_usage = total_usage;
        self.task_token_map = task_token_map;
    }

    /// Check if the graph has changed on disk and refresh if needed.
    pub fn maybe_refresh(&mut self) {
        if self.last_refresh.elapsed() < self.refresh_interval {
            return;
        }

        let current_mtime = std::fs::metadata(self.workgraph_dir.join("graph.jsonl"))
            .and_then(|m| m.modified())
            .ok();

        let graph_changed = current_mtime != self.last_graph_mtime;
        let needs_token_refresh = self.task_counts.in_progress > 0;

        if graph_changed || needs_token_refresh {
            if graph_changed {
                self.last_graph_mtime = current_mtime;
                self.load_viz();
                if !self.search_input.is_empty() {
                    self.rerun_search();
                }
            }
            self.load_stats();
            self.invalidate_hud();
            self.last_refresh_display = chrono::Local::now().format("%H:%M:%S").to_string();
        }

        self.last_refresh = Instant::now();
    }

    /// Cycle through layout modes (tree ↔ diamond).
    pub fn cycle_layout(&mut self) {
        use crate::commands::viz::LayoutMode;
        self.viz_options.layout = match self.viz_options.layout {
            LayoutMode::Tree => LayoutMode::Diamond,
            LayoutMode::Diamond => LayoutMode::Tree,
        };
        self.force_refresh();
    }

    /// Get the current layout mode name for display.
    #[allow(dead_code)]
    pub fn layout_name(&self) -> &'static str {
        use crate::commands::viz::LayoutMode;
        match self.viz_options.layout {
            LayoutMode::Tree => "tree",
            LayoutMode::Diamond => "diamond",
        }
    }

    /// Compute aggregate token usage for tasks currently visible on screen.
    /// Extracts task IDs from the plain_lines visible in the viewport.
    pub fn visible_token_usage(&self) -> TokenUsage {
        let mut usage = TokenUsage {
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
        };
        // Collect unique task IDs from all visible lines (not just viewport)
        let visible_count = self.visible_line_count();
        let mut seen = HashSet::new();
        for visible_idx in 0..visible_count {
            let orig_idx = self.visible_to_original(visible_idx);
            if let Some(plain) = self.plain_lines.get(orig_idx)
                && let Some(task_id) = extract_task_id(plain)
                && seen.insert(task_id.clone())
                && let Some(task_usage) = self.task_token_map.get(&task_id)
            {
                usage.accumulate(task_usage);
            }
        }
        usage
    }

    /// Toggle mouse capture on/off.
    pub fn toggle_mouse(&mut self) {
        self.mouse_enabled = !self.mouse_enabled;
    }

    /// Toggle edge trace highlighting on/off.
    pub fn toggle_trace(&mut self) {
        self.trace_visible = !self.trace_visible;
        if !self.trace_visible {
            self.hud_detail = None;
            self.hud_scroll = 0;
        } else {
            self.load_hud_detail();
        }
    }

    /// Load HUD detail for the currently selected task.
    /// Called when selection changes or trace is toggled on.
    pub fn load_hud_detail(&mut self) {
        let task_id = match self.selected_task_id() {
            Some(id) => id.to_string(),
            None => {
                self.hud_detail = None;
                return;
            }
        };

        // Skip reload if already loaded for this task.
        if let Some(ref detail) = self.hud_detail
            && detail.task_id == task_id
        {
            return;
        }

        self.hud_scroll = 0;

        let graph_path = self.workgraph_dir.join("graph.jsonl");
        let graph = match load_graph(&graph_path) {
            Ok(g) => g,
            Err(_) => {
                self.hud_detail = None;
                return;
            }
        };

        let task = match graph.tasks().find(|t| t.id == task_id) {
            Some(t) => t.clone(),
            None => {
                self.hud_detail = None;
                return;
            }
        };

        let mut lines: Vec<String> = Vec::new();

        // ── Header ──
        lines.push(format!("── {} ──", task.id));
        lines.push(format!("Title: {}", task.title));
        lines.push(format!("Status: {:?}", task.status));
        if let Some(ref agent) = task.assigned {
            lines.push(format!("Agent: {}", agent));
        }
        lines.push(String::new());

        // ── Description ──
        if let Some(ref desc) = task.description {
            lines.push("── Description ──".to_string());
            for (i, line) in desc.lines().enumerate() {
                if i >= 10 {
                    lines.push("  ...".to_string());
                    break;
                }
                lines.push(format!("  {}", line));
            }
            lines.push(String::new());
        }

        // ── Agent prompt ──
        if let Some(ref agent_id) = task.assigned {
            let prompt_path = self
                .workgraph_dir
                .join("agents")
                .join(agent_id)
                .join("prompt.txt");
            if prompt_path.exists() {
                lines.push("── Prompt ──".to_string());
                if let Ok(file) = std::fs::File::open(&prompt_path) {
                    let reader = BufReader::new(file);
                    for (i, line) in reader.lines().enumerate() {
                        if i >= 10 {
                            lines.push("  ...".to_string());
                            break;
                        }
                        if let Ok(l) = line {
                            lines.push(format!("  {}", l));
                        }
                    }
                }
                lines.push(String::new());
            }
        }

        // ── Agent output (tail) ──
        if let Some(ref agent_id) = task.assigned {
            let output_path = self
                .workgraph_dir
                .join("agents")
                .join(agent_id)
                .join("output.log");
            if output_path.exists() {
                lines.push("── Output (tail) ──".to_string());
                if let Ok(content) = std::fs::read_to_string(&output_path) {
                    let all_lines: Vec<&str> = content.lines().collect();
                    let start = all_lines.len().saturating_sub(10);
                    for line in &all_lines[start..] {
                        lines.push(format!("  {}", line));
                    }
                }
                lines.push(String::new());
            }
        }

        // ── Evaluation ──
        let evals_dir = self.workgraph_dir.join("agency").join("evaluations");
        if evals_dir.exists() {
            let prefix = format!("eval-{}-", task.id);
            if let Ok(entries) = std::fs::read_dir(&evals_dir) {
                let mut eval_found = false;
                // Find the most recent evaluation for this task.
                let mut eval_files: Vec<_> = entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
                    .collect();
                eval_files.sort_by_key(|b| std::cmp::Reverse(b.file_name()));
                if let Some(entry) = eval_files.first()
                    && let Ok(content) = std::fs::read_to_string(entry.path())
                    && let Ok(eval) = serde_json::from_str::<serde_json::Value>(&content)
                {
                    eval_found = true;
                    lines.push("── Evaluation ──".to_string());
                    if let Some(score) = eval.get("score").and_then(|v| v.as_f64()) {
                        lines.push(format!("  Score: {:.2}", score));
                    }
                    if let Some(notes) = eval.get("notes").and_then(|v| v.as_str()) {
                        // Show first ~3 lines of notes.
                        for (i, line) in notes.lines().enumerate() {
                            if i >= 3 {
                                lines.push("  ...".to_string());
                                break;
                            }
                            lines.push(format!("  {}", line));
                        }
                    }
                    if let Some(dims) = eval.get("dimensions").and_then(|v| v.as_object()) {
                        let dim_strs: Vec<String> = dims
                            .iter()
                            .map(|(k, v)| format!("{}:{:.2}", k, v.as_f64().unwrap_or(0.0)))
                            .collect();
                        lines.push(format!("  Dims: {}", dim_strs.join(", ")));
                    }
                    lines.push(String::new());
                }
                let _ = eval_found;
            }
        }

        // ── Token usage ──
        if let Some(ref usage) = task.token_usage {
            lines.push("── Tokens ──".to_string());
            lines.push(format!(
                "  Input:  {} (→{})",
                format_tokens(usage.total_input()),
                format_tokens(usage.input_tokens)
            ));
            lines.push(format!(
                "  Output: {} (←{})",
                format_tokens(usage.output_tokens),
                format_tokens(usage.output_tokens)
            ));
            if usage.cache_read_input_tokens > 0 || usage.cache_creation_input_tokens > 0 {
                lines.push(format!(
                    "  Cache read:  {} (◎)",
                    format_tokens(usage.cache_read_input_tokens)
                ));
                lines.push(format!(
                    "  Cache write: {} (⊳)",
                    format_tokens(usage.cache_creation_input_tokens)
                ));
            }
            if usage.cost_usd > 0.0 {
                lines.push(format!("  Cost: ${:.4}", usage.cost_usd));
            }
            lines.push(String::new());
        }

        // ── Dependencies ──
        if !task.after.is_empty() || !task.before.is_empty() {
            lines.push("── Dependencies ──".to_string());
            if !task.after.is_empty() {
                lines.push(format!("  After:  {}", task.after.join(", ")));
            }
            if !task.before.is_empty() {
                lines.push(format!("  Before: {}", task.before.join(", ")));
            }
            lines.push(String::new());
        }

        // ── Timing ──
        let has_timing =
            task.created_at.is_some() || task.started_at.is_some() || task.completed_at.is_some();
        if has_timing {
            lines.push("── Timing ──".to_string());
            if let Some(ref ts) = task.created_at {
                lines.push(format!("  Created:   {}", format_timestamp(ts)));
            }
            if let Some(ref ts) = task.started_at {
                lines.push(format!("  Started:   {}", format_timestamp(ts)));
            }
            if let Some(ref ts) = task.completed_at {
                lines.push(format!("  Completed: {}", format_timestamp(ts)));
            }
            // Duration
            if let (Some(start), Some(end)) = (&task.started_at, &task.completed_at)
                && let (Ok(s), Ok(e)) = (
                    chrono::DateTime::parse_from_rfc3339(start),
                    chrono::DateTime::parse_from_rfc3339(end),
                )
            {
                let dur = (e - s).num_seconds();
                lines.push(format!(
                    "  Duration:  {}",
                    workgraph::format_duration(dur, false)
                ));
            }
            lines.push(String::new());
        }

        // ── Failure reason ──
        if let Some(ref reason) = task.failure_reason {
            lines.push("── Failure ──".to_string());
            lines.push(format!("  {}", reason));
            lines.push(String::new());
        }

        self.hud_detail = Some(HudDetail {
            task_id,
            rendered_lines: lines,
        });
    }

    /// Invalidate HUD detail so it reloads on next render.
    pub fn invalidate_hud(&mut self) {
        self.hud_detail = None;
    }

    /// Scroll the HUD panel up.
    pub fn hud_scroll_up(&mut self, amount: usize) {
        self.hud_scroll = self.hud_scroll.saturating_sub(amount);
    }

    /// Scroll the HUD panel down.
    pub fn hud_scroll_down(&mut self, amount: usize, max_lines: usize, viewport: usize) {
        let max_scroll = max_lines.saturating_sub(viewport);
        self.hud_scroll = (self.hud_scroll + amount).min(max_scroll);
    }

    /// Construct a VizApp from pre-built VizOutput for unit testing.
    /// Avoids needing a real workgraph directory on disk.
    #[cfg(test)]
    pub(crate) fn from_viz_output_for_test(viz: &crate::commands::viz::VizOutput) -> Self {
        let lines: Vec<String> = viz.text.lines().map(String::from).collect();
        let plain_lines: Vec<String> = lines
            .iter()
            .map(|l| String::from_utf8(strip_ansi_escapes::strip(l.as_bytes())).unwrap_or_default())
            .collect();
        let search_lines = plain_lines.iter().map(|l| sanitize_for_search(l)).collect();
        let max_line_width = plain_lines.iter().map(|l| l.len()).max().unwrap_or(0);

        let mut task_order: Vec<(String, usize)> = viz
            .node_line_map
            .iter()
            .map(|(id, &line)| (id.clone(), line))
            .collect();
        task_order.sort_by_key(|(_, line)| *line);
        let task_order: Vec<String> = task_order.into_iter().map(|(id, _)| id).collect();

        let selected_task_idx = if task_order.is_empty() { None } else { Some(0) };

        Self {
            workgraph_dir: std::path::PathBuf::from("/tmp/test-workgraph"),
            viz_options: crate::commands::viz::VizOptions::default(),
            should_quit: false,
            lines,
            plain_lines,
            search_lines,
            max_line_width,
            scroll: ViewportScroll::new(),
            search_active: false,
            search_input: String::new(),
            fuzzy_matches: Vec::new(),
            current_match: None,
            filtered_indices: None,
            matcher: SkimMatcherV2::default(),
            task_counts: TaskCounts::default(),
            total_usage: workgraph::graph::TokenUsage {
                cost_usd: 0.0,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
            },
            task_token_map: HashMap::new(),
            show_total_tokens: false,
            show_help: false,
            mouse_enabled: false,
            jump_target: None,
            task_order,
            node_line_map: viz.node_line_map.clone(),
            forward_edges: viz.forward_edges.clone(),
            reverse_edges: viz.reverse_edges.clone(),
            selected_task_idx,
            trace_visible: true,
            upstream_set: HashSet::new(),
            downstream_set: HashSet::new(),
            char_edge_map: viz.char_edge_map.clone(),
            cycle_members: viz.cycle_members.clone(),
            cycle_set: HashSet::new(),
            hud_detail: None,
            hud_scroll: 0,
            last_graph_mtime: None,
            last_refresh: Instant::now(),
            last_refresh_display: String::new(),
            refresh_interval: std::time::Duration::from_secs(3600),
        }
    }

    /// Force an immediate refresh (manual `r` key).
    pub fn force_refresh(&mut self) {
        self.last_graph_mtime = std::fs::metadata(self.workgraph_dir.join("graph.jsonl"))
            .and_then(|m| m.modified())
            .ok();
        self.load_viz();
        if !self.search_input.is_empty() {
            self.rerun_search();
        }
        self.load_stats();
        self.last_refresh_display = chrono::Local::now().format("%H:%M:%S").to_string();
        self.last_refresh = Instant::now();
    }
}

/// Detect if we're running inside a tmux split pane.
///
/// Compares the terminal size (from crossterm) with the tmux window size.
/// If the terminal is smaller than the tmux window, we're in a split pane
/// and mouse capture should be disabled by default (tmux needs mouse events
/// for pane selection/resize).
fn detect_tmux_split() -> bool {
    // Only applies if TMUX env var is set
    if std::env::var("TMUX").is_err() {
        return false;
    }

    // Get terminal size from crossterm
    let (term_cols, term_rows) = match crossterm::terminal::size() {
        Ok(size) => size,
        Err(_) => return false,
    };

    // Get tmux window size via `tmux display-message -p '#{window_width} #{window_height}'`
    let output = match std::process::Command::new("tmux")
        .args(["display-message", "-p", "#{window_width} #{window_height}"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return false,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = stdout.split_whitespace().collect();
    if parts.len() != 2 {
        return false;
    }

    let (tmux_cols, tmux_rows) = match (parts[0].parse::<u16>(), parts[1].parse::<u16>()) {
        (Ok(c), Ok(r)) => (c, r),
        _ => return false,
    };

    // If terminal is smaller than tmux window, we're in a split
    term_cols < tmux_cols || term_rows < tmux_rows
}

// ── Tree-aware filtering ──

/// Determine the "indent level" of a line: the char-index of the first alphanumeric character.
/// Returns `None` for lines with no alphanumeric characters (blank, pure box-drawing, etc.).
fn line_indent_level(plain: &str) -> Option<usize> {
    plain
        .chars()
        .enumerate()
        .find(|(_, c)| c.is_alphanumeric())
        .map(|(i, _)| i)
}

/// Check if a line is a summary/separator line (e.g., "  ╌╌ 12 tasks ╌╌").
fn is_summary_line(plain: &str) -> bool {
    plain.trim().starts_with('╌')
}

/// Compute the set of visible line indices given the fuzzy matches.
/// Includes matching lines, their tree ancestors, and section context.
fn compute_filtered_indices(
    plain_lines: &[String],
    fuzzy_matches: &[FuzzyLineMatch],
) -> Vec<usize> {
    if fuzzy_matches.is_empty() {
        return Vec::new();
    }

    let matching_lines: HashSet<usize> = fuzzy_matches.iter().map(|m| m.line_idx).collect();

    // Parse sections: each section is a group of non-empty lines,
    // separated by blank lines. The last non-blank line in a section
    // is typically a summary starting with ╌╌.
    let mut sections: Vec<(usize, usize)> = Vec::new(); // (start, end) inclusive
    let mut i = 0;
    while i < plain_lines.len() {
        // Skip blank lines between sections.
        if plain_lines[i].trim().is_empty() {
            i += 1;
            continue;
        }
        let start = i;
        while i < plain_lines.len() && !plain_lines[i].trim().is_empty() {
            i += 1;
        }
        sections.push((start, i - 1)); // end is inclusive
    }

    let mut visible: HashSet<usize> = HashSet::new();

    for &(sec_start, sec_end) in &sections {
        // Check if any line in this section matches.
        let section_has_match = (sec_start..=sec_end).any(|idx| matching_lines.contains(&idx));
        if !section_has_match {
            continue;
        }

        // For each matching line in this section, include it and its tree ancestors.
        for line_idx in sec_start..=sec_end {
            if !matching_lines.contains(&line_idx) {
                continue;
            }

            visible.insert(line_idx);

            // Walk backwards to find ancestor lines (lines with smaller indent).
            let match_indent = line_indent_level(&plain_lines[line_idx]);
            if match_indent.is_none() {
                continue;
            }
            let mut need_below = match_indent.unwrap();

            for ancestor_idx in (sec_start..line_idx).rev() {
                if is_summary_line(&plain_lines[ancestor_idx]) {
                    continue;
                }
                if let Some(indent) = line_indent_level(&plain_lines[ancestor_idx])
                    && indent < need_below
                {
                    visible.insert(ancestor_idx);
                    need_below = indent;
                    if indent == 0 {
                        break; // reached root
                    }
                }
            }
        }

        // Always include the summary line for sections that have matches.
        if is_summary_line(&plain_lines[sec_end]) {
            visible.insert(sec_end);
        }
    }

    // Build sorted result. Insert blank lines between sections for readability.
    let mut result: Vec<usize> = visible.into_iter().collect();
    result.sort();
    result
}

/// Extract a task ID from a plain (ANSI-stripped) viz line.
/// Task lines look like: `  ├→ task-id  (status · tokens)` or `task-id  (status)`.
/// Returns None for non-task lines (summaries, blanks, box-drawing-only lines).
fn extract_task_id(plain: &str) -> Option<String> {
    // Skip summary/separator lines
    if is_summary_line(plain) {
        return None;
    }
    // Find the first alphanumeric/hyphen/underscore sequence (the task ID).
    // Task IDs consist of [a-zA-Z0-9_-].
    let trimmed = plain.trim_start();
    // Strip leading tree connectors (box-drawing + arrows + spaces)
    let after_connectors: &str =
        trimmed.trim_start_matches(|c: char| is_box_drawing(c) || c == ' ');
    if after_connectors.is_empty() {
        return None;
    }
    // The task ID is the first "word" — characters that are alphanumeric, hyphen, or underscore.
    let id: String = after_connectors
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if id.is_empty() {
        return None;
    }
    // Verify it looks like a task line: after the ID there should be whitespace then '('
    let rest = &after_connectors[id.len()..];
    if rest.trim_start().starts_with('(') {
        Some(id)
    } else {
        None
    }
}

/// Replace box-drawing and arrow characters with spaces so fuzzy search
/// doesn't match on visual decoration (├│─◀▶╌ etc.).
fn sanitize_for_search(line: &str) -> String {
    line.chars()
        .map(|c| if is_box_drawing(c) { ' ' } else { c })
        .collect()
}

pub(super) fn is_box_drawing(c: char) -> bool {
    matches!(
        c,
        '│' | '├'
            | '└'
            | '┌'
            | '┐'
            | '┘'
            | '─'
            | '╌'
            | '◀'
            | '▶'
            | '←'
            | '→'
            | '↓'
            | '↑'
            | '╭'
            | '╮'
            | '╯'
            | '╰'
            | '┼'
            | '┤'
            | '┬'
            | '┴'
            | '▼'
            | '▲'
            | '►'
            | '◄'
    )
}

/// Format an ISO 8601 timestamp for HUD display (shorter, local time).
fn format_timestamp(ts: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(ts) {
        Ok(dt) => {
            let local = dt.with_timezone(&chrono::Local);
            local.format("%Y-%m-%d %H:%M:%S").to_string()
        }
        Err(_) => ts.to_string(),
    }
}

/// Convert byte positions (from fuzzy_indices) to char positions for a given string.
fn byte_positions_to_char_positions(s: &str, byte_positions: &[usize]) -> Vec<usize> {
    if byte_positions.is_empty() {
        return Vec::new();
    }
    let byte_set: HashSet<usize> = byte_positions.iter().copied().collect();
    let mut char_positions = Vec::with_capacity(byte_positions.len());
    for (char_idx, (byte_idx, _)) in s.char_indices().enumerate() {
        if byte_set.contains(&byte_idx) {
            char_positions.push(char_idx);
        }
    }
    char_positions
}

impl ViewportScroll {
    pub fn new() -> Self {
        Self {
            offset_y: 0,
            offset_x: 0,
            content_height: 0,
            content_width: 0,
            viewport_height: 0,
            viewport_width: 0,
        }
    }

    pub fn scroll_up(&mut self, amount: usize) {
        self.offset_y = self.offset_y.saturating_sub(amount);
    }

    pub fn scroll_down(&mut self, amount: usize) {
        let max_y = self.content_height.saturating_sub(self.viewport_height);
        self.offset_y = (self.offset_y + amount).min(max_y);
    }

    pub fn scroll_left(&mut self, amount: usize) {
        self.offset_x = self.offset_x.saturating_sub(amount);
    }

    pub fn scroll_right(&mut self, amount: usize) {
        let max_x = self.content_width.saturating_sub(self.viewport_width);
        self.offset_x = (self.offset_x + amount).min(max_x);
    }

    pub fn page_up(&mut self) {
        self.scroll_up(self.viewport_height / 2);
    }

    pub fn page_down(&mut self) {
        self.scroll_down(self.viewport_height / 2);
    }

    pub fn go_top(&mut self) {
        self.offset_y = 0;
    }

    pub fn go_bottom(&mut self) {
        self.offset_y = self.content_height.saturating_sub(self.viewport_height);
    }

    /// Clamp scroll offset to valid range after content changes.
    pub fn clamp(&mut self) {
        let max_y = self.content_height.saturating_sub(self.viewport_height);
        self.offset_y = self.offset_y.min(max_y);
        let max_x = self.content_width.saturating_sub(self.viewport_width);
        self.offset_x = self.offset_x.min(max_x);
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Tests for HUD state and behavior
// ══════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod hud_tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use workgraph::graph::{Node, Status, TokenUsage, WorkGraph};
    use workgraph::parser::save_graph;
    use workgraph::test_helpers::make_task_with_status;

    use crate::commands::viz::ascii::generate_ascii;
    use crate::commands::viz::{LayoutMode, VizOutput};

    /// Build a chain graph a -> b -> c plus standalone d, with rich metadata on task a.
    /// Returns (VizOutput, WorkGraph, TempDir) — keep TempDir alive while using the app.
    fn build_chain_plus_isolated() -> (VizOutput, WorkGraph, tempfile::TempDir) {
        let mut graph = WorkGraph::new();
        let mut a = make_task_with_status("a", "Task Alpha", Status::Done);
        a.description = Some(
            "This is the description for task Alpha.\nLine two.\nLine three.\nLine four."
                .to_string(),
        );
        a.assigned = Some("agent-001".to_string());
        a.created_at = Some("2026-01-15T10:00:00Z".to_string());
        a.started_at = Some("2026-01-15T10:05:00Z".to_string());
        a.completed_at = Some("2026-01-15T10:30:00Z".to_string());
        a.token_usage = Some(TokenUsage {
            cost_usd: 0.05,
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_input_tokens: 200,
            cache_creation_input_tokens: 100,
        });

        let mut b = make_task_with_status("b", "Task Bravo", Status::InProgress);
        b.after = vec!["a".to_string()];
        b.assigned = Some("agent-002".to_string());
        b.description = Some("Description for Bravo.".to_string());

        let mut c = make_task_with_status("c", "Task Charlie", Status::Open);
        c.after = vec!["b".to_string()];
        // No description, no agent, no tokens — for missing-data tests.

        let mut d = make_task_with_status("d", "Task Delta", Status::Failed);
        d.failure_reason = Some("Timed out after 30 minutes".to_string());
        d.description = Some("Delta task description.".to_string());

        graph.add_node(Node::Task(a));
        graph.add_node(Node::Task(b));
        graph.add_node(Node::Task(c));
        graph.add_node(Node::Task(d));

        // Create a temp directory with graph.jsonl so load_hud_detail works.
        let tmp = tempfile::tempdir().unwrap();
        let graph_path = tmp.path().join("graph.jsonl");
        save_graph(&graph, &graph_path).unwrap();

        let tasks: Vec<_> = graph.tasks().collect();
        let task_ids: HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        let result = generate_ascii(
            &graph,
            &tasks,
            &task_ids,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            LayoutMode::Tree,
            &HashSet::new(),
            "gray",
        );
        (result, graph, tmp)
    }

    /// Build a VizApp with a specific task selected, pointed at a real workgraph dir.
    fn build_app(viz: &VizOutput, selected_id: &str, workgraph_dir: &std::path::Path) -> VizApp {
        let mut app = VizApp::from_viz_output_for_test(viz);
        app.workgraph_dir = workgraph_dir.to_path_buf();
        let idx = app.task_order.iter().position(|id| id == selected_id);
        app.selected_task_idx = idx;
        app.recompute_trace();
        app
    }

    // ── TEST 1: HUD APPEARS WITH TAB ──

    #[test]
    fn hud_visible_when_trace_on_and_task_selected() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let app = build_app(&viz, "a", _tmp.path());

        assert!(app.trace_visible, "trace_visible should default to true");
        assert!(
            app.selected_task_idx.is_some(),
            "should have a selected task"
        );
        let show_hud = app.trace_visible && app.selected_task_idx.is_some();
        assert!(
            show_hud,
            "HUD should be visible when trace is on and task is selected"
        );
    }

    // ── TEST 2: HUD DISAPPEARS WITH TAB ──

    #[test]
    fn hud_hidden_when_trace_toggled_off() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "a", _tmp.path());

        app.toggle_trace();
        assert!(!app.trace_visible, "trace should be off after toggle");
        assert!(
            app.hud_detail.is_none(),
            "HUD detail should be cleared when trace is off"
        );
        assert_eq!(app.hud_scroll, 0, "HUD scroll should reset");

        let show_hud = app.trace_visible && app.selected_task_idx.is_some();
        assert!(!show_hud, "HUD should NOT be visible when trace is off");
    }

    #[test]
    fn hud_reappears_after_double_toggle() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "a", _tmp.path());

        app.toggle_trace(); // off
        app.toggle_trace(); // on
        assert!(app.trace_visible);
        let show_hud = app.trace_visible && app.selected_task_idx.is_some();
        assert!(show_hud, "HUD should reappear after toggling back on");
    }

    // ── TEST 3: HUD CONTENT CORRECT ──

    #[test]
    fn hud_shows_task_id_and_title() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "a", _tmp.path());
        app.load_hud_detail();

        let detail = app.hud_detail.as_ref().expect("HUD detail should load");
        assert_eq!(detail.task_id, "a");
        assert!(detail.rendered_lines.iter().any(|l| l.contains("── a ──")));
        assert!(
            detail
                .rendered_lines
                .iter()
                .any(|l| l.contains("Title: Task Alpha"))
        );
    }

    #[test]
    fn hud_shows_status() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "a", _tmp.path());
        app.load_hud_detail();

        let detail = app.hud_detail.as_ref().unwrap();
        assert!(
            detail
                .rendered_lines
                .iter()
                .any(|l| l.contains("Status: Done"))
        );
    }

    #[test]
    fn hud_shows_agent() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "a", _tmp.path());
        app.load_hud_detail();

        let detail = app.hud_detail.as_ref().unwrap();
        assert!(
            detail
                .rendered_lines
                .iter()
                .any(|l| l.contains("Agent: agent-001"))
        );
    }

    #[test]
    fn hud_shows_description_excerpt() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "a", _tmp.path());
        app.load_hud_detail();

        let detail = app.hud_detail.as_ref().unwrap();
        assert!(
            detail
                .rendered_lines
                .iter()
                .any(|l| l.contains("── Description ──"))
        );
        assert!(
            detail
                .rendered_lines
                .iter()
                .any(|l| l.contains("This is the description for task Alpha."))
        );
    }

    #[test]
    fn hud_shows_token_usage() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "a", _tmp.path());
        app.load_hud_detail();

        let detail = app.hud_detail.as_ref().unwrap();
        assert!(
            detail
                .rendered_lines
                .iter()
                .any(|l| l.contains("── Tokens ──"))
        );
        assert!(
            detail
                .rendered_lines
                .iter()
                .any(|l| l.contains("Cost: $0.05"))
        );
        assert!(
            detail
                .rendered_lines
                .iter()
                .any(|l| l.contains("Cache read:"))
        );
    }

    #[test]
    fn hud_shows_dependencies() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "b", _tmp.path());
        app.load_hud_detail();

        let detail = app.hud_detail.as_ref().unwrap();
        assert!(
            detail
                .rendered_lines
                .iter()
                .any(|l| l.contains("── Dependencies ──"))
        );
        assert!(
            detail
                .rendered_lines
                .iter()
                .any(|l| l.contains("After:") && l.contains("a"))
        );
    }

    #[test]
    fn hud_shows_timing() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "a", _tmp.path());
        app.load_hud_detail();

        let detail = app.hud_detail.as_ref().unwrap();
        assert!(
            detail
                .rendered_lines
                .iter()
                .any(|l| l.contains("── Timing ──"))
        );
        assert!(detail.rendered_lines.iter().any(|l| l.contains("Created:")));
        assert!(detail.rendered_lines.iter().any(|l| l.contains("Started:")));
        assert!(
            detail
                .rendered_lines
                .iter()
                .any(|l| l.contains("Completed:"))
        );
        assert!(
            detail
                .rendered_lines
                .iter()
                .any(|l| l.contains("Duration:"))
        );
    }

    #[test]
    fn hud_shows_failure_reason() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "d", _tmp.path());
        app.load_hud_detail();

        let detail = app.hud_detail.as_ref().unwrap();
        assert!(
            detail
                .rendered_lines
                .iter()
                .any(|l| l.contains("── Failure ──"))
        );
        assert!(
            detail
                .rendered_lines
                .iter()
                .any(|l| l.contains("Timed out after 30 minutes"))
        );
    }

    // ── TEST 4: HUD UPDATES ON SELECTION ──

    #[test]
    fn hud_invalidates_on_selection_change() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "a", _tmp.path());
        app.load_hud_detail();
        assert_eq!(app.hud_detail.as_ref().unwrap().task_id, "a");

        app.select_next_task();
        // recompute_trace calls invalidate_hud
        assert!(
            app.hud_detail.is_none(),
            "HUD should be invalidated after selection change"
        );

        app.load_hud_detail();
        let new_id = app.hud_detail.as_ref().unwrap().task_id.clone();
        assert_ne!(new_id, "a", "HUD should now show a different task");
    }

    #[test]
    fn hud_content_changes_on_navigation() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "a", _tmp.path());
        app.load_hud_detail();
        let initial = app.hud_detail.as_ref().unwrap().rendered_lines.clone();

        app.select_next_task();
        app.load_hud_detail();
        let next = app.hud_detail.as_ref().unwrap().rendered_lines.clone();

        assert_ne!(
            initial, next,
            "HUD content should change when selecting a different task"
        );
    }

    #[test]
    fn hud_updates_on_prev_task() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "a", _tmp.path());

        app.select_next_task();
        app.load_hud_detail();
        let second_id = app.hud_detail.as_ref().unwrap().task_id.clone();

        app.select_prev_task();
        app.load_hud_detail();
        let back_id = app.hud_detail.as_ref().unwrap().task_id.clone();

        assert_ne!(
            second_id, back_id,
            "HUD should show different content after navigating back"
        );
    }

    // ── TEST 5: NARROW TERMINAL FALLBACK ──
    // (Layout tests are in render.rs test module below)

    // ── TEST 6: HUD SCROLLABLE ──

    #[test]
    fn hud_scroll_down() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "a", _tmp.path());
        app.load_hud_detail();

        let total = app.hud_detail.as_ref().unwrap().rendered_lines.len();
        assert!(total > 5, "precondition: need >5 lines to test scrolling");

        assert_eq!(app.hud_scroll, 0);
        app.hud_scroll_down(3, total, 10);
        assert_eq!(app.hud_scroll, 3);
    }

    #[test]
    fn hud_scroll_up() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "a", _tmp.path());
        app.load_hud_detail();

        let total = app.hud_detail.as_ref().unwrap().rendered_lines.len();
        app.hud_scroll_down(5, total, 10);
        assert_eq!(app.hud_scroll, 5);

        app.hud_scroll_up(2);
        assert_eq!(app.hud_scroll, 3);
    }

    #[test]
    fn hud_scroll_clamps_at_zero() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "a", _tmp.path());
        app.load_hud_detail();

        app.hud_scroll_up(10);
        assert_eq!(app.hud_scroll, 0, "scroll should not go below 0");
    }

    #[test]
    fn hud_scroll_clamps_at_max() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "a", _tmp.path());
        app.load_hud_detail();

        let total = app.hud_detail.as_ref().unwrap().rendered_lines.len();
        let viewport = 10;
        let max_scroll = total.saturating_sub(viewport);

        app.hud_scroll_down(1000, total, viewport);
        assert_eq!(app.hud_scroll, max_scroll, "scroll should clamp at max");
    }

    #[test]
    fn hud_scroll_resets_on_selection_change() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "a", _tmp.path());
        app.load_hud_detail();

        let total = app.hud_detail.as_ref().unwrap().rendered_lines.len();
        app.hud_scroll_down(5, total, 10);
        assert!(app.hud_scroll > 0);

        app.select_next_task();
        app.load_hud_detail();
        assert_eq!(app.hud_scroll, 0, "scroll should reset for new task");
    }

    // ── TEST 7: NO CRASH ON MISSING DATA ──

    #[test]
    fn hud_no_crash_no_agent() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "c", _tmp.path());
        app.load_hud_detail();

        let detail = app
            .hud_detail
            .as_ref()
            .expect("should load even with no agent");
        assert_eq!(detail.task_id, "c");
        assert!(
            !detail
                .rendered_lines
                .iter()
                .any(|l| l.starts_with("Agent:"))
        );
    }

    #[test]
    fn hud_no_crash_no_description() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "c", _tmp.path());
        app.load_hud_detail();

        let detail = app.hud_detail.as_ref().unwrap();
        assert!(
            !detail
                .rendered_lines
                .iter()
                .any(|l| l.contains("── Description ──"))
        );
    }

    #[test]
    fn hud_no_crash_no_tokens() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "c", _tmp.path());
        app.load_hud_detail();

        let detail = app.hud_detail.as_ref().unwrap();
        assert!(
            !detail
                .rendered_lines
                .iter()
                .any(|l| l.contains("── Tokens ──"))
        );
    }

    #[test]
    fn hud_no_crash_no_timing() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "c", _tmp.path());
        app.load_hud_detail();

        let detail = app.hud_detail.as_ref().unwrap();
        assert!(
            !detail
                .rendered_lines
                .iter()
                .any(|l| l.contains("── Timing ──"))
        );
    }

    #[test]
    fn hud_no_crash_no_failure() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "a", _tmp.path());
        app.load_hud_detail();

        let detail = app.hud_detail.as_ref().unwrap();
        assert!(
            !detail
                .rendered_lines
                .iter()
                .any(|l| l.contains("── Failure ──"))
        );
    }

    #[test]
    fn hud_no_crash_no_dependencies() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "d", _tmp.path());
        app.load_hud_detail();

        let detail = app.hud_detail.as_ref().unwrap();
        assert!(
            !detail
                .rendered_lines
                .iter()
                .any(|l| l.contains("── Dependencies ──"))
        );
    }

    #[test]
    fn hud_no_crash_no_selection() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = VizApp::from_viz_output_for_test(&viz);
        app.workgraph_dir = _tmp.path().to_path_buf();
        app.selected_task_idx = None;

        app.load_hud_detail();
        assert!(app.hud_detail.is_none());
    }

    #[test]
    fn hud_no_crash_empty_graph() {
        let empty_viz = crate::commands::viz::VizOutput {
            text: "(no tasks to display)".to_string(),
            node_line_map: HashMap::new(),
            task_order: Vec::new(),
            forward_edges: HashMap::new(),
            reverse_edges: HashMap::new(),
            char_edge_map: HashMap::new(),
            cycle_members: HashMap::new(),
        };

        let mut app = VizApp::from_viz_output_for_test(&empty_viz);
        assert!(app.selected_task_idx.is_none());

        app.load_hud_detail();
        assert!(app.hud_detail.is_none());

        // Toggle trace on empty graph should not panic
        app.toggle_trace();
        assert!(!app.trace_visible);
        app.toggle_trace();
        assert!(app.trace_visible);
    }

    // ── ADDITIONAL: skip-reload optimization ──

    #[test]
    fn hud_skips_reload_for_same_task() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "a", _tmp.path());
        app.load_hud_detail();
        assert_eq!(app.hud_detail.as_ref().unwrap().task_id, "a");

        // Second load should be a no-op
        app.load_hud_detail();
        assert_eq!(app.hud_detail.as_ref().unwrap().task_id, "a");
    }

    #[test]
    fn hud_invalidate_forces_reload() {
        let (viz, _, _tmp) = build_chain_plus_isolated();
        let mut app = build_app(&viz, "a", _tmp.path());
        app.load_hud_detail();
        assert!(app.hud_detail.is_some());

        app.invalidate_hud();
        assert!(app.hud_detail.is_none());

        app.load_hud_detail();
        assert_eq!(app.hud_detail.as_ref().unwrap().task_id, "a");
    }

    // ── ADDITIONAL: description truncation ──

    #[test]
    fn hud_description_truncated_to_10_lines() {
        let mut graph = WorkGraph::new();
        let mut task = make_task_with_status("long-desc", "Long Description Task", Status::Open);
        task.description = Some(
            (0..15)
                .map(|i| format!("Line {}", i))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        graph.add_node(Node::Task(task));

        let tmp = tempfile::tempdir().unwrap();
        let graph_path = tmp.path().join("graph.jsonl");
        save_graph(&graph, &graph_path).unwrap();

        let tasks: Vec<_> = graph.tasks().collect();
        let task_ids: HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        let viz = generate_ascii(
            &graph,
            &tasks,
            &task_ids,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            LayoutMode::Tree,
            &HashSet::new(),
            "gray",
        );

        let mut app = build_app(&viz, "long-desc", tmp.path());
        app.load_hud_detail();

        let detail = app.hud_detail.as_ref().unwrap();
        let desc_start = detail
            .rendered_lines
            .iter()
            .position(|l| l.contains("── Description ──"))
            .expect("should have description section");

        let desc_lines: Vec<_> = detail.rendered_lines[desc_start + 1..]
            .iter()
            .take_while(|l| !l.is_empty())
            .collect();

        // Should have at most 11 lines (10 content + 1 "  ..." truncation indicator)
        assert!(
            desc_lines.len() <= 11,
            "Description should be truncated, got {} lines",
            desc_lines.len()
        );
        assert!(
            desc_lines.iter().any(|l| l.contains("...")),
            "Truncated description should show '...' indicator"
        );
    }
}
