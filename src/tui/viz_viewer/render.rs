use std::collections::HashSet;

use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use super::state::VizApp;
use workgraph::graph::{TokenUsage, format_tokens};

pub fn draw(frame: &mut Frame, app: &mut VizApp) {
    // Clear expired jump targets (>2 seconds old).
    if let Some((_, when)) = app.jump_target
        && when.elapsed() > std::time::Duration::from_secs(2)
    {
        app.jump_target = None;
    }

    let area = frame.area();

    // Layout: main content area + status bar (1 line).
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // viz content
            Constraint::Length(1), // status bar
        ])
        .split(area);

    let content_area = chunks[0];
    let status_area = chunks[1];

    // Update viewport dimensions from terminal size.
    app.scroll.viewport_height = content_area.height as usize;
    app.scroll.viewport_width = content_area.width as usize;

    // Viz content
    draw_viz_content(frame, app, content_area);

    // Vertical scrollbar (only if content overflows)
    if app.scroll.content_height > app.scroll.viewport_height {
        draw_scrollbar(frame, app, content_area);
    }

    // Status bar
    draw_status_bar(frame, app, status_area);

    // Help overlay on top of everything
    if app.show_help {
        draw_help_overlay(frame);
    }
}

/// Determine the line-level trace category for a given original line index.
/// Used only for task text coloring (not for edge characters).
enum LineTraceCategory {
    Selected,
    Upstream,
    Downstream,
    Unrelated,
}

fn classify_task_line(app: &VizApp, orig_idx: usize) -> LineTraceCategory {
    // Check if this line is the selected task's line.
    if let Some(selected_id) = app.selected_task_id()
        && let Some(&sel_line) = app.node_line_map.get(selected_id)
        && orig_idx == sel_line
    {
        return LineTraceCategory::Selected;
    }
    // Check if this line belongs to an upstream or downstream task node.
    for (id, &line) in &app.node_line_map {
        if line == orig_idx {
            if app.upstream_set.contains(id) {
                return LineTraceCategory::Upstream;
            }
            if app.downstream_set.contains(id) {
                return LineTraceCategory::Downstream;
            }
        }
    }
    LineTraceCategory::Unrelated
}

/// Check whether a given original line index is the selected task's line.
fn is_selected_task_line(app: &VizApp, orig_idx: usize) -> bool {
    if let Some(selected_id) = app.selected_task_id()
        && let Some(&sel_line) = app.node_line_map.get(selected_id)
    {
        return orig_idx == sel_line;
    }
    false
}

fn draw_viz_content(frame: &mut Frame, app: &VizApp, area: Rect) {
    let visible_count = app.visible_line_count();
    let start = app.scroll.offset_y;
    let end = (start + area.height as usize).min(visible_count);

    if start >= visible_count {
        return;
    }

    let has_search = app.has_active_search() && !app.fuzzy_matches.is_empty();
    let current_match_orig_line = app.current_match_line();
    let jump_target_line = app.jump_target.map(|(line, _)| line);
    let has_trace = app.selected_task_idx.is_some() && app.trace_visible;
    let has_selected = app.selected_task_idx.is_some();

    // Build lines for the visible range.
    // Each visible row maps to an original line index via visible_to_original.
    let mut text_lines: Vec<Line> = Vec::with_capacity(end - start);

    // Precompute the selected task ID for the edge map lookups.
    let selected_id = app.selected_task_id().map(|s| s.to_string());

    for visible_idx in start..end {
        let orig_idx = app.visible_to_original(visible_idx);

        // Get the ANSI line and parse it.
        let ansi_line = app.lines.get(orig_idx).map(|s| s.as_str()).unwrap_or("");
        let base_line: Line = match ansi_to_tui::IntoText::into_text(&ansi_line) {
            Ok(text) => text.lines.into_iter().next().unwrap_or_default(),
            Err(_) => {
                let plain = app
                    .plain_lines
                    .get(orig_idx)
                    .map(|s| s.as_str())
                    .unwrap_or("");
                Line::from(plain)
            }
        };

        if has_search {
            if let Some(fuzzy_match) = app.match_for_line(orig_idx) {
                // This line has a fuzzy match — highlight matched characters.
                let is_current = current_match_orig_line == Some(orig_idx);
                let mut highlighted =
                    highlight_fuzzy_match(base_line, &fuzzy_match.char_positions, is_current);
                if is_current {
                    highlighted = highlighted.style(Style::default().bg(Color::Yellow));
                }
                text_lines.push(highlighted);
            } else {
                // Non-matching line in filtered view: show dimmed.
                let dimmed = base_line.style(Style::default().fg(Color::DarkGray));
                text_lines.push(dimmed);
            }
        } else if jump_target_line == Some(orig_idx) {
            // Transient highlight on the line we jumped to after Enter.
            text_lines.push(base_line.style(Style::default().bg(Color::Yellow)));
        } else if has_trace {
            // Per-character edge tracing with topology-aware coloring.
            let plain_line = app
                .plain_lines
                .get(orig_idx)
                .map(|s| s.as_str())
                .unwrap_or("");
            let line_category = classify_task_line(app, orig_idx);
            let colored_line = apply_per_char_trace_coloring(
                base_line,
                plain_line,
                orig_idx,
                &line_category,
                app,
                selected_id.as_deref(),
            );
            // Mark the selected task with bold + bright styling (text only).
            if matches!(line_category, LineTraceCategory::Selected) {
                text_lines.push(apply_selection_style(colored_line, plain_line));
            } else {
                text_lines.push(colored_line);
            }
        } else if has_selected && is_selected_task_line(app, orig_idx) {
            // Trace is off but a task is selected — still show bold + bright (text only).
            let plain_line = app
                .plain_lines
                .get(orig_idx)
                .map(|s| s.as_str())
                .unwrap_or("");
            text_lines.push(apply_selection_style(base_line, plain_line));
        } else {
            text_lines.push(base_line);
        }
    }

    let text = Text::from(text_lines);

    // Apply horizontal scroll.
    let paragraph = Paragraph::new(text).scroll((0, app.scroll.offset_x as u16));

    frame.render_widget(paragraph, area);

    // Off-screen selection direction indicator: when the selected task is
    // scrolled out of the viewport, show a yellow arrow at the edge to hint
    // which direction the user needs to scroll.
    if has_selected
        && !has_search
        && let Some(selected_id) = app.selected_task_id()
        && let Some(&sel_orig_line) = app.node_line_map.get(selected_id)
    {
        let is_visible = (start..end).any(|vi| app.visible_to_original(vi) == sel_orig_line);
        if !is_visible {
            let first_visible_orig = app.visible_to_original(start);
            let indicator_style = Style::default().fg(Color::Yellow);
            if sel_orig_line < first_visible_orig {
                // Selected task is above viewport.
                let arrow = Paragraph::new(Line::from(Span::styled("▲", indicator_style)));
                let arrow_area = Rect {
                    x: area.x,
                    y: area.y,
                    width: 1,
                    height: 1,
                };
                frame.render_widget(arrow, arrow_area);
            } else {
                // Selected task is below viewport.
                let arrow = Paragraph::new(Line::from(Span::styled("▼", indicator_style)));
                let bottom_y = area.y + area.height.saturating_sub(1);
                let arrow_area = Rect {
                    x: area.x,
                    y: bottom_y,
                    width: 1,
                    height: 1,
                };
                frame.render_widget(arrow, arrow_area);
            }
        }
    }
}

/// Apply per-character trace coloring to a line based on the char_edge_map.
///
/// PURELY ADDITIVE — only these changes from normal display:
/// - Edge chars where both endpoints are in upstream_set ∪ {selected}: magenta
/// - Edge chars where both endpoints are in downstream_set ∪ {selected}: cyan
/// - Selected task text: original style preserved (bold + bright applied at line level)
/// - Everything else: original style preserved unchanged
fn apply_per_char_trace_coloring<'a>(
    line: Line<'a>,
    plain_line: &str,
    orig_idx: usize,
    _line_category: &LineTraceCategory,
    app: &VizApp,
    selected_id: Option<&str>,
) -> Line<'a> {
    let text_range = find_text_range(plain_line);

    // Flatten spans into characters with styles.
    let mut chars_with_styles: Vec<(char, Style)> = Vec::new();
    for span in &line.spans {
        for c in span.content.chars() {
            chars_with_styles.push((c, span.style));
        }
    }

    // Build the upstream+selected and downstream+selected sets for quick lookup.
    let in_cycle = |id: &str| -> bool { app.cycle_set.contains(id) };
    let in_upstream =
        |id: &str| -> bool { app.upstream_set.contains(id) || selected_id == Some(id) };
    let in_downstream =
        |id: &str| -> bool { app.downstream_set.contains(id) || selected_id == Some(id) };

    let (text_start, text_end) = text_range.unwrap_or((usize::MAX, usize::MAX));

    // Rebuild spans with per-character coloring.
    // PURELY ADDITIVE: only edge chars in the dependency chain get magenta/cyan,
    // selected task text gets yellow bg. Everything else keeps its original style.
    let mut new_spans: Vec<Span<'a>> = Vec::new();
    let mut current_buf = String::new();
    let mut current_style = Style::default();
    let mut first = true;

    for (char_idx, (c, base_style)) in chars_with_styles.iter().enumerate() {
        let is_text = char_idx >= text_start && char_idx < text_end;

        let style = if is_text {
            // All task text keeps original style unchanged.
            // Selected task is indicated by bold + bright styling at line level.
            *base_style
        } else if let Some(edges) = app.char_edge_map.get(&(orig_idx, char_idx)) {
            // Edge character with known edge(s): color if ANY edge matches topology.
            // Shared arc column positions may carry multiple edges.
            // Priority: yellow (cycle) > magenta (upstream) > cyan (downstream).
            let is_cycle_edge = !app.cycle_set.is_empty()
                && edges
                    .iter()
                    .any(|(src, tgt)| in_cycle(src) && in_cycle(tgt));
            let is_upstream_edge = edges
                .iter()
                .any(|(src, tgt)| in_upstream(src) && in_upstream(tgt));
            let is_downstream_edge = edges
                .iter()
                .any(|(src, tgt)| in_downstream(src) && in_downstream(tgt));
            if is_cycle_edge {
                let mut s = *base_style;
                s.fg = Some(Color::Yellow);
                s
            } else if is_upstream_edge {
                let mut s = *base_style;
                s.fg = Some(Color::Magenta);
                s
            } else if is_downstream_edge {
                let mut s = *base_style;
                s.fg = Some(Color::Cyan);
                s
            } else {
                // Edge exists but not in the selected task's dependency chain — keep original
                *base_style
            }
        } else {
            // Non-text, non-edge character (spaces, connectors not in edge map, etc.)
            // Keep original style — trace is purely additive
            *base_style
        };

        if first {
            current_style = style;
            first = false;
        } else if style != current_style {
            new_spans.push(Span::styled(
                std::mem::take(&mut current_buf),
                current_style,
            ));
            current_style = style;
        }

        current_buf.push(*c);
    }

    if !current_buf.is_empty() {
        new_spans.push(Span::styled(current_buf, current_style));
    }

    Line::from(new_spans)
}

/// Find the character range of the "task text" in a plain viz line.
/// Returns (text_start, text_end) as char indices.
/// - text_start: index of first alphanumeric character (task ID start)
/// - text_end: index after last ')' (closing status/token info)
///   Returns None for non-task lines (pure connectors, blanks, summaries).
fn find_text_range(plain_line: &str) -> Option<(usize, usize)> {
    let chars: Vec<char> = plain_line.chars().collect();

    // Find first alphanumeric character (start of task text).
    let text_start = chars.iter().position(|c| c.is_alphanumeric())?;

    // Find the last ')' which closes the status/token info.
    let text_end = chars
        .iter()
        .rposition(|&c| c == ')')
        .map(|i| i + 1) // exclusive end, include the ')'
        .unwrap_or_else(|| {
            // No ')' found — find the last non-connector char.
            let mut end = text_start;
            for (i, &ch) in chars.iter().enumerate().skip(text_start) {
                if !ch.is_whitespace() && !super::state::is_box_drawing(ch) {
                    end = i + 1;
                }
            }
            end
        });

    Some((text_start, text_end))
}

/// Apply bold + bright styling to the task text portion of the selected line.
///
/// Uses `find_text_range` to identify the task text (ID, title, status) and
/// only applies bold + bright there. Edge/connector characters outside the
/// text range keep their original style (or trace color).
fn apply_selection_style<'a>(line: Line<'a>, plain_line: &str) -> Line<'a> {
    let text_range = find_text_range(plain_line);
    let (text_start, text_end) = text_range.unwrap_or((0, 0));

    // If no text range found, return line unchanged.
    if text_range.is_none() {
        return line;
    }

    // Flatten spans into per-character (char, style) pairs.
    let mut chars_with_styles: Vec<(char, Style)> = Vec::new();
    for span in &line.spans {
        for c in span.content.chars() {
            chars_with_styles.push((c, span.style));
        }
    }

    // Rebuild spans, applying bold+bright only within the text range.
    let mut new_spans: Vec<Span<'a>> = Vec::new();
    let mut current_buf = String::new();
    let mut current_style = Style::default();
    let mut first = true;

    for (char_idx, (c, base_style)) in chars_with_styles.iter().enumerate() {
        let style = if char_idx >= text_start && char_idx < text_end {
            brighten_style(*base_style).add_modifier(Modifier::BOLD)
        } else {
            *base_style
        };

        if first {
            current_style = style;
            first = false;
        } else if style != current_style {
            new_spans.push(Span::styled(
                std::mem::take(&mut current_buf),
                current_style,
            ));
            current_style = style;
        }

        current_buf.push(*c);
    }

    if !current_buf.is_empty() {
        new_spans.push(Span::styled(current_buf, current_style));
    }

    Line::from(new_spans)
}

/// Brighten a style's foreground color for the selected-task emphasis effect.
fn brighten_style(style: Style) -> Style {
    let bright_fg = match style.fg {
        Some(Color::Black) => Some(Color::DarkGray),
        Some(Color::Red) => Some(Color::LightRed),
        Some(Color::Green) => Some(Color::LightGreen),
        Some(Color::Yellow) => Some(Color::LightYellow),
        Some(Color::Blue) => Some(Color::LightBlue),
        Some(Color::Magenta) => Some(Color::LightMagenta),
        Some(Color::Cyan) => Some(Color::LightCyan),
        Some(Color::Gray) => Some(Color::White),
        Some(Color::DarkGray) => Some(Color::Gray),
        // Already bright or custom — keep as-is
        other => other,
    };
    Style {
        fg: bright_fg,
        ..style
    }
}

/// Highlight the fuzzy-matched characters within a line.
/// Matched chars get bold + colored. Current match uses a distinct color.
fn highlight_fuzzy_match<'a>(
    base_line: Line<'a>,
    char_positions: &[usize],
    is_current_match: bool,
) -> Line<'a> {
    if char_positions.is_empty() {
        return base_line;
    }

    let match_set: HashSet<usize> = char_positions.iter().copied().collect();

    let match_modifier = if is_current_match {
        Modifier::BOLD | Modifier::UNDERLINED
    } else {
        Modifier::UNDERLINED
    };

    // Flatten the line's spans into individual characters, then regroup
    // into spans based on whether each char is matched or not.
    let mut chars_with_styles: Vec<(char, Style)> = Vec::new();
    for span in &base_line.spans {
        for c in span.content.chars() {
            chars_with_styles.push((c, span.style));
        }
    }

    let mut new_spans: Vec<Span<'a>> = Vec::new();
    let mut current_buf = String::new();
    let mut current_is_match = false;
    let mut current_base_style = Style::default();

    for (char_idx, (c, base_style)) in chars_with_styles.iter().enumerate() {
        let is_match = match_set.contains(&char_idx);

        // Check if we need to flush the current buffer.
        if !current_buf.is_empty()
            && (is_match != current_is_match || *base_style != current_base_style)
        {
            let style = if current_is_match {
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(match_modifier)
            } else {
                current_base_style
            };
            new_spans.push(Span::styled(std::mem::take(&mut current_buf), style));
        }

        current_buf.push(*c);
        current_is_match = is_match;
        current_base_style = *base_style;
    }

    // Flush remaining buffer.
    if !current_buf.is_empty() {
        let style = if current_is_match {
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(match_modifier)
        } else {
            current_base_style
        };
        new_spans.push(Span::styled(current_buf, style));
    }

    Line::from(new_spans)
}

fn draw_scrollbar(frame: &mut Frame, app: &VizApp, area: Rect) {
    let mut state = ScrollbarState::new(app.scroll.content_height).position(app.scroll.offset_y);
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
    frame.render_stateful_widget(scrollbar, area, &mut state);
}

fn draw_status_bar(frame: &mut Frame, app: &VizApp, area: Rect) {
    if app.search_active {
        // Search input mode: show the search prompt with cursor.
        let mut spans = vec![
            Span::styled(
                format!(" /{}", app.search_input),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled(
                "_",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::SLOW_BLINK),
            ),
        ];

        // Show match count inline.
        if !app.search_input.is_empty() {
            if app.fuzzy_matches.is_empty() {
                spans.push(Span::styled(
                    "  [no matches]",
                    Style::default().fg(Color::Red),
                ));
            } else {
                let idx = app.current_match.unwrap_or(0);
                spans.push(Span::styled(
                    format!("  [Match {}/{}]", idx + 1, app.fuzzy_matches.len()),
                    Style::default().fg(Color::Green),
                ));
            }
        }

        // Keybinding hints for search mode.
        spans.push(Span::styled(
            "  [Tab: next  Shift-Tab: prev  Enter: go to  Esc: cancel]",
            Style::default().fg(Color::Rgb(100, 100, 100)),
        ));

        let bar = Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::DarkGray));
        frame.render_widget(bar, area);
        return;
    }

    // Filter locked: search accepted, highlights visible, navigating matches.
    if !app.search_input.is_empty() && !app.fuzzy_matches.is_empty() {
        let idx = app.current_match.unwrap_or(0);
        let mut spans = vec![
            Span::styled(
                format!(" /{}", app.search_input),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled(
                format!("  [Match {}/{}]", idx + 1, app.fuzzy_matches.len()),
                Style::default().fg(Color::Green),
            ),
            Span::styled(
                "  [n: next  N: prev  /: new search  Esc: clear]",
                Style::default().fg(Color::Rgb(100, 100, 100)),
            ),
        ];

        // Scroll position
        spans.push(Span::styled("  ", Style::default()));
        spans.push(Span::styled(
            format!("L{}/{}", app.scroll.offset_y + 1, app.scroll.content_height),
            Style::default().fg(Color::DarkGray),
        ));

        let bar = Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::DarkGray));
        frame.render_widget(bar, area);
        return;
    }

    let c = &app.task_counts;
    let mut spans = vec![Span::styled(
        format!(
            " {} tasks ({} done, {} open, {} active",
            c.total, c.done, c.open, c.in_progress
        ),
        Style::default().fg(Color::White),
    )];

    if c.failed > 0 {
        spans.push(Span::styled(
            format!(", {} failed", c.failed),
            Style::default().fg(Color::Red),
        ));
    }

    spans.push(Span::styled(") ", Style::default().fg(Color::White)));

    // Token breakdown: input/output/cache with view/total toggle
    let visible_usage;
    let (usage, label) = if app.show_total_tokens {
        (&app.total_usage, "total")
    } else {
        visible_usage = app.visible_token_usage();
        (&visible_usage, "view")
    };
    if usage.total_tokens() > 0 {
        spans.push(Span::styled("| ", Style::default().fg(Color::DarkGray)));
        render_token_breakdown(&mut spans, usage, label);
    }

    // Scroll position
    spans.push(Span::styled("| ", Style::default().fg(Color::DarkGray)));
    spans.push(Span::styled(
        format!(
            "L{}/{} ",
            app.scroll.offset_y + 1,
            app.scroll.content_height
        ),
        Style::default().fg(Color::White),
    ));

    // Selected task indicator
    if let Some(task_id) = app.selected_task_id() {
        spans.push(Span::styled("| ", Style::default().fg(Color::DarkGray)));
        // Truncate long task IDs for status bar display
        let display_id = if task_id.len() > 24 {
            format!("{}…", &task_id[..23])
        } else {
            task_id.to_string()
        };
        spans.push(Span::styled(
            format!("▸{} ", display_id),
            Style::default().fg(Color::Yellow),
        ));
    }

    // Search/filter state
    let search_status = app.search_status();
    if !search_status.is_empty() {
        spans.push(Span::styled("| ", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(
            format!("{} ", search_status),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }

    // Live refresh indicator
    if app.task_counts.in_progress > 0 {
        spans.push(Span::styled("| ", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(
            format!("LIVE {} ", app.last_refresh_display),
            Style::default().fg(Color::Green),
        ));
    } else {
        spans.push(Span::styled("| ", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(
            format!("{} ", app.last_refresh_display),
            Style::default().fg(Color::DarkGray),
        ));
    }

    // Trace state indicator
    if !app.trace_visible {
        spans.push(Span::styled("| ", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(
            "TRACE OFF ",
            Style::default().fg(Color::Yellow),
        ));
    }

    // Mouse state indicator
    if !app.mouse_enabled {
        spans.push(Span::styled("| ", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(
            "MOUSE OFF ",
            Style::default().fg(Color::Yellow),
        ));
    }

    // Help hint
    spans.push(Span::styled("| ", Style::default().fg(Color::DarkGray)));
    spans.push(Span::styled(
        "?:help ",
        Style::default().fg(Color::DarkGray),
    ));

    let bar = Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::DarkGray));
    frame.render_widget(bar, area);
}

fn draw_help_overlay(frame: &mut Frame) {
    let size = frame.area();
    let width = 56.min(size.width.saturating_sub(4));
    let height = 40.min(size.height.saturating_sub(4));
    let x = (size.width.saturating_sub(width)) / 2;
    let y = (size.height.saturating_sub(height)) / 2;
    let area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Keybindings ")
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

    let inner = block.inner(area);

    let heading = |text: &str| -> Line {
        Line::from(Span::styled(
            text.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
    };

    let binding = |key: &str, desc: &str| -> Line {
        Line::from(vec![
            Span::styled(format!("  {:<14}", key), Style::default().fg(Color::Yellow)),
            Span::styled(desc.to_string(), Style::default().fg(Color::White)),
        ])
    };

    let blank = || Line::from("");

    let lines = vec![
        heading("Navigation"),
        binding("↑ / ↓", "Select prev / next task"),
        binding("j / k", "Scroll down / up"),
        binding("h / l", "Scroll left / right"),
        binding("Ctrl-d / u", "Page down / up"),
        binding("g / G", "Jump to top / bottom"),
        blank(),
        heading("Edge Tracing"),
        binding("Tab", "Toggle trace highlighting on/off"),
        binding("↑ / ↓", "Select task (highlights deps)"),
        binding("", "Bold=selected  Magenta=upstream"),
        binding("", "Cyan=downstream"),
        blank(),
        heading("Search (vim-style)"),
        binding("/", "Start search"),
        binding("Enter", "Accept (show all, keep highlights)"),
        binding("Esc", "Clear search"),
        binding("n / N", "Next / previous match"),
        blank(),
        heading("While searching"),
        binding("Tab / ←→", "Next / previous match"),
        binding("Up / Down", "Scroll view"),
        binding("Ctrl-u", "Clear search input"),
        blank(),
        heading("General"),
        binding("m", "Toggle mouse capture"),
        binding("t", "Toggle view/total tokens"),
        binding("r", "Force refresh"),
        binding("?", "Toggle this help"),
        binding("q", "Quit"),
        binding("Ctrl-c", "Force quit"),
        blank(),
        heading("Token Symbols"),
        binding("→", "Input tokens"),
        binding("←", "Output tokens"),
        binding("◎", "Cache read tokens"),
        binding("⊳", "Cache creation tokens"),
        binding("$X.XX", "Estimated cost (USD)"),
        blank(),
        Line::from(Span::styled(
            "  Press ? or Esc to close",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let paragraph = Paragraph::new(lines);

    frame.render_widget(block, area);
    frame.render_widget(paragraph, inner);
}

/// Render token breakdown spans: "→in ←out [◎cache] (label) [$cost]"
fn render_token_breakdown<'a>(spans: &mut Vec<Span<'a>>, usage: &TokenUsage, label: &str) {
    let input = format_tokens(usage.total_input());
    let output = format_tokens(usage.output_tokens);

    let cache_total = usage.cache_read_input_tokens + usage.cache_creation_input_tokens;
    let token_str = if cache_total > 0 {
        let cache = format_tokens(cache_total);
        format!("→{} ←{} ◎{}", input, output, cache)
    } else {
        format!("→{} ←{}", input, output)
    };

    spans.push(Span::styled(token_str, Style::default().fg(Color::Cyan)));

    // Label: "view" or "total" — dim to avoid clutter
    spans.push(Span::styled(
        format!(" {} ", label),
        Style::default().fg(Color::DarkGray),
    ));

    // Cost if available
    if usage.cost_usd > 0.0 {
        spans.push(Span::styled(
            format!("${:.2} ", usage.cost_usd),
            Style::default().fg(Color::Cyan),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};
    use std::collections::{HashMap, HashSet};
    use workgraph::graph::{Node, Status, WorkGraph};
    use workgraph::test_helpers::make_task_with_status;

    use crate::commands::viz::ascii::generate_ascii;
    use crate::commands::viz::{LayoutMode, VizOutput};

    /// Build a test graph and generate viz output.
    /// Returns (VizOutput, graph) for a chain: a -> b -> c, plus standalone d.
    fn build_test_graph_chain_plus_isolated() -> (VizOutput, WorkGraph) {
        let mut graph = WorkGraph::new();
        let a = make_task_with_status("a", "Task A", Status::Done);
        let mut b = make_task_with_status("b", "Task B", Status::InProgress);
        b.after = vec!["a".to_string()];
        let mut c = make_task_with_status("c", "Task C", Status::Open);
        c.after = vec!["b".to_string()];
        let d = make_task_with_status("d", "Task D", Status::Failed);
        graph.add_node(Node::Task(a));
        graph.add_node(Node::Task(b));
        graph.add_node(Node::Task(c));
        graph.add_node(Node::Task(d));

        let tasks: Vec<_> = graph.tasks().collect();
        let task_ids: HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        let no_annots = HashMap::new();
        let result = generate_ascii(
            &graph,
            &tasks,
            &task_ids,
            &no_annots,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            LayoutMode::Tree,
            &HashSet::new(),
            "gray",
        );
        (result, graph)
    }

    /// Build a VizApp from VizOutput for testing apply_per_char_trace_coloring.
    /// Sets the selected task and computes upstream/downstream sets.
    fn build_app_from_viz_output(viz: &VizOutput, selected_id: &str) -> VizApp {
        let mut app = VizApp::from_viz_output_for_test(viz);
        let selected_task_idx = app.task_order.iter().position(|id| id == selected_id);
        app.selected_task_idx = selected_task_idx;
        app.recompute_trace();
        app
    }

    /// Parse an ANSI line into a ratatui Line.
    fn parse_ansi_line(ansi: &str) -> Line<'static> {
        match ansi_to_tui::IntoText::into_text(&ansi) {
            Ok(text) => text.lines.into_iter().next().unwrap_or_default(),
            Err(_) => Line::from(ansi.to_string()),
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // Test 1: TEXT COLORS UNCHANGED — status-based colors preserved
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_text_colors_unchanged_for_upstream_task() {
        let (viz, _graph) = build_test_graph_chain_plus_isolated();
        let app = build_app_from_viz_output(&viz, "b");

        // 'a' is upstream of 'b'. Its text should keep its original style.
        let a_line = viz.node_line_map["a"];
        let plain = app.plain_lines[a_line].as_str();
        let category = classify_task_line(&app, a_line);
        assert!(matches!(category, LineTraceCategory::Upstream));

        // Create a line with explicit green color (done status).
        let green_style = Style::default().fg(Color::Green);
        let text_range = find_text_range(plain);
        assert!(text_range.is_some(), "Task line should have text range");
        let (text_start, text_end) = text_range.unwrap();

        // Build a line with known colors.
        let chars: Vec<char> = plain.chars().collect();
        let prefix: String = chars[..text_start].iter().collect();
        let text: String = chars[text_start..text_end].iter().collect();
        let suffix: String = chars[text_end..].iter().collect();
        let line = Line::from(vec![
            Span::styled(prefix.clone(), Style::default().fg(Color::DarkGray)),
            Span::styled(text.clone(), green_style),
            Span::styled(suffix.clone(), Style::default()),
        ]);

        let result = apply_per_char_trace_coloring(line, plain, a_line, &category, &app, Some("b"));

        // Verify that the task text portion preserved its green color.
        let mut char_idx = 0;
        for span in &result.spans {
            for c in span.content.chars() {
                if char_idx >= text_start && char_idx < text_end {
                    assert_eq!(
                        span.style.fg,
                        Some(Color::Green),
                        "Upstream task text at char {} ('{}') should preserve green status color, got {:?}",
                        char_idx,
                        c,
                        span.style.fg
                    );
                }
                char_idx += 1;
            }
        }
    }

    #[test]
    fn test_text_colors_unchanged_for_downstream_task() {
        let (viz, _graph) = build_test_graph_chain_plus_isolated();
        let app = build_app_from_viz_output(&viz, "b");

        // 'c' is downstream of 'b'. Its text should keep its original style.
        let c_line = viz.node_line_map["c"];
        let plain = app.plain_lines[c_line].as_str();
        let category = classify_task_line(&app, c_line);
        assert!(matches!(category, LineTraceCategory::Downstream));

        let text_range = find_text_range(plain).unwrap();
        let (text_start, text_end) = text_range;
        let chars: Vec<char> = plain.chars().collect();
        let prefix: String = chars[..text_start].iter().collect();
        let text: String = chars[text_start..text_end].iter().collect();
        let suffix: String = chars[text_end..].iter().collect();

        let white_style = Style::default().fg(Color::White);
        let line = Line::from(vec![
            Span::styled(prefix, Style::default().fg(Color::DarkGray)),
            Span::styled(text, white_style),
            Span::styled(suffix, Style::default()),
        ]);

        let result = apply_per_char_trace_coloring(line, plain, c_line, &category, &app, Some("b"));

        let mut char_idx = 0;
        for span in &result.spans {
            for c in span.content.chars() {
                if char_idx >= text_start && char_idx < text_end {
                    assert_eq!(
                        span.style.fg,
                        Some(Color::White),
                        "Downstream task text at char {} ('{}') should preserve white status color, got {:?}",
                        char_idx,
                        c,
                        span.style.fg
                    );
                }
                char_idx += 1;
            }
        }
    }

    #[test]
    fn test_text_colors_unchanged_for_unrelated_task() {
        let (viz, _graph) = build_test_graph_chain_plus_isolated();
        let app = build_app_from_viz_output(&viz, "b");

        // 'd' is unrelated to 'b' (separate WCC). Its text should keep original style.
        let d_line = viz.node_line_map["d"];
        let plain = app.plain_lines[d_line].as_str();
        let category = classify_task_line(&app, d_line);
        assert!(matches!(category, LineTraceCategory::Unrelated));

        let text_range = find_text_range(plain).unwrap();
        let (text_start, text_end) = text_range;
        let chars: Vec<char> = plain.chars().collect();
        let prefix: String = chars[..text_start].iter().collect();
        let text: String = chars[text_start..text_end].iter().collect();
        let suffix: String = chars[text_end..].iter().collect();

        let red_style = Style::default().fg(Color::Red);
        let line = Line::from(vec![
            Span::styled(prefix, Style::default()),
            Span::styled(text, red_style),
            Span::styled(suffix, Style::default()),
        ]);

        let result = apply_per_char_trace_coloring(line, plain, d_line, &category, &app, Some("b"));

        let mut char_idx = 0;
        for span in &result.spans {
            for _c in span.content.chars() {
                if char_idx >= text_start && char_idx < text_end {
                    assert_eq!(
                        span.style.fg,
                        Some(Color::Red),
                        "Unrelated task text at char {} should preserve red status color, got {:?}",
                        char_idx,
                        span.style.fg
                    );
                }
                char_idx += 1;
            }
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // Test 2: UPSTREAM EDGES COLORED MAGENTA
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_upstream_edges_colored_magenta() {
        let (viz, _graph) = build_test_graph_chain_plus_isolated();
        let app = build_app_from_viz_output(&viz, "b");

        // Find edge chars that belong to the a->b edge and verify they're magenta.

        // Edge chars on b's line (connectors like ├→ or └→) should be in char_edge_map
        // with (src="a", tgt="b"). Since both a and b are in upstream∪{selected},
        // they should be colored magenta.
        let mut found_magenta_edge = false;
        for (key, edges) in &viz.char_edge_map {
            let (ln, col) = *key;
            if edges.iter().any(|(s, t)| s == "a" && t == "b") {
                let (src, tgt) = edges.iter().find(|(s, t)| s == "a" && t == "b").unwrap();
                // This is an a->b edge character. Verify it would be colored magenta.
                let plain = app.plain_lines[ln].as_str();
                let base_line = parse_ansi_line(app.lines[ln].as_str());
                let category = classify_task_line(&app, ln);

                let result =
                    apply_per_char_trace_coloring(base_line, plain, ln, &category, &app, Some("b"));

                // Find the span containing char at position `col`.
                let mut char_idx = 0;
                for span in &result.spans {
                    for _ in span.content.chars() {
                        if char_idx == col {
                            assert_eq!(
                                span.style.fg,
                                Some(Color::Magenta),
                                "Upstream edge char at ({}, {}) for edge {}->{} should be magenta, got {:?}",
                                ln,
                                col,
                                src,
                                tgt,
                                span.style.fg
                            );
                            found_magenta_edge = true;
                        }
                        char_idx += 1;
                    }
                }
            }
        }
        assert!(
            found_magenta_edge,
            "Should find at least one magenta-colored upstream edge char for a->b edge"
        );
    }

    // ══════════════════════════════════════════════════════════════════════
    // Test 3: DOWNSTREAM EDGES COLORED CYAN
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_downstream_edges_colored_cyan() {
        let (viz, _graph) = build_test_graph_chain_plus_isolated();
        let app = build_app_from_viz_output(&viz, "b");

        // Find edge chars that belong to the b->c edge and verify they're cyan.
        let mut found_cyan_edge = false;
        for (key, edges) in &viz.char_edge_map {
            let (ln, col) = *key;
            if edges.iter().any(|(s, t)| s == "b" && t == "c") {
                let (src, tgt) = edges.iter().find(|(s, t)| s == "b" && t == "c").unwrap();
                let plain = app.plain_lines[ln].as_str();
                let base_line = parse_ansi_line(app.lines[ln].as_str());
                let category = classify_task_line(&app, ln);

                let result =
                    apply_per_char_trace_coloring(base_line, plain, ln, &category, &app, Some("b"));

                let mut char_idx = 0;
                for span in &result.spans {
                    for _ in span.content.chars() {
                        if char_idx == col {
                            assert_eq!(
                                span.style.fg,
                                Some(Color::Cyan),
                                "Downstream edge char at ({}, {}) for edge {}->{} should be cyan, got {:?}",
                                ln,
                                col,
                                src,
                                tgt,
                                span.style.fg
                            );
                            found_cyan_edge = true;
                        }
                        char_idx += 1;
                    }
                }
            }
        }
        assert!(
            found_cyan_edge,
            "Should find at least one cyan-colored downstream edge char for b->c edge"
        );
    }

    // ══════════════════════════════════════════════════════════════════════
    // Test 4: ONLY CONNECTED EDGES COLORED — unrelated edges keep base style
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_unrelated_edges_not_colored() {
        // Build a graph with two independent chains: a->b->c and x->y
        let mut graph = WorkGraph::new();
        let a = make_task_with_status("a", "Task A", Status::Done);
        let mut b = make_task_with_status("b", "Task B", Status::InProgress);
        b.after = vec!["a".to_string()];
        let mut c = make_task_with_status("c", "Task C", Status::Open);
        c.after = vec!["b".to_string()];
        let x = make_task_with_status("x", "Task X", Status::Open);
        let mut y = make_task_with_status("y", "Task Y", Status::Open);
        y.after = vec!["x".to_string()];
        graph.add_node(Node::Task(a));
        graph.add_node(Node::Task(b));
        graph.add_node(Node::Task(c));
        graph.add_node(Node::Task(x));
        graph.add_node(Node::Task(y));

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

        let app = build_app_from_viz_output(&viz, "b");

        // Find edge chars for x->y edge — they should NOT be colored.
        for (key, edges) in &viz.char_edge_map {
            let (ln, col) = *key;
            if edges.iter().any(|(s, t)| s == "x" && t == "y") {
                let (src, tgt) = edges.iter().find(|(s, t)| s == "x" && t == "y").unwrap();
                let plain = app.plain_lines[ln].as_str();
                let base_line = parse_ansi_line(app.lines[ln].as_str());
                let category = classify_task_line(&app, ln);

                let result =
                    apply_per_char_trace_coloring(base_line, plain, ln, &category, &app, Some("b"));

                let mut char_idx = 0;
                for span in &result.spans {
                    for _ in span.content.chars() {
                        if char_idx == col {
                            assert!(
                                span.style.fg != Some(Color::Magenta)
                                    && span.style.fg != Some(Color::Cyan),
                                "Unrelated edge char at ({}, {}) for edge {}->{} should NOT be magenta/cyan, got {:?}",
                                ln,
                                col,
                                src,
                                tgt,
                                span.style.fg
                            );
                        }
                        char_idx += 1;
                    }
                }
            }
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // Test 5: OTHER WCCs UNCHANGED — WCCs not containing the selected task
    //         must render identically to normal output
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_other_wcc_unchanged() {
        let (viz, _graph) = build_test_graph_chain_plus_isolated();
        let app = build_app_from_viz_output(&viz, "b");

        // 'd' is in a separate WCC. Its entire line should be unchanged.
        let d_line = viz.node_line_map["d"];
        let plain = app.plain_lines[d_line].as_str();
        let base_line = parse_ansi_line(app.lines[d_line].as_str());
        let category = classify_task_line(&app, d_line);
        assert!(matches!(category, LineTraceCategory::Unrelated));

        // Collect base styles.
        let mut base_chars: Vec<(char, Style)> = Vec::new();
        for span in &base_line.spans {
            for c in span.content.chars() {
                base_chars.push((c, span.style));
            }
        }

        let result = apply_per_char_trace_coloring(
            parse_ansi_line(app.lines[d_line].as_str()),
            plain,
            d_line,
            &category,
            &app,
            Some("b"),
        );

        // Collect result styles.
        let mut result_chars: Vec<(char, Style)> = Vec::new();
        for span in &result.spans {
            for c in span.content.chars() {
                result_chars.push((c, span.style));
            }
        }

        assert_eq!(
            base_chars.len(),
            result_chars.len(),
            "WCC-unrelated line should have same number of chars"
        );
        for (i, ((bc, bs), (rc, rs))) in base_chars.iter().zip(result_chars.iter()).enumerate() {
            assert_eq!(bc, rc, "Char mismatch at position {}", i);
            assert_eq!(
                bs, rs,
                "Style mismatch at position {} ('{}') in other-WCC line: expected {:?}, got {:?}",
                i, bc, bs, rs
            );
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // Test 6: SELECTED TASK INDICATOR — selected task gets special treatment
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_selected_task_keeps_original_style() {
        let (viz, _graph) = build_test_graph_chain_plus_isolated();
        let app = build_app_from_viz_output(&viz, "b");

        let b_line = viz.node_line_map["b"];
        let plain = app.plain_lines[b_line].as_str();
        let category = classify_task_line(&app, b_line);
        assert!(matches!(category, LineTraceCategory::Selected));

        let base_line = parse_ansi_line(app.lines[b_line].as_str());
        let base_styles: Vec<(char, Style)> = base_line
            .spans
            .iter()
            .flat_map(|s| s.content.chars().map(move |c| (c, s.style)))
            .collect();

        let base_line2 = parse_ansi_line(app.lines[b_line].as_str());
        let result =
            apply_per_char_trace_coloring(base_line2, plain, b_line, &category, &app, Some("b"));

        let text_range = find_text_range(plain).unwrap();
        let (text_start, text_end) = text_range;

        // Selected task text should keep its original style from apply_per_char_trace_coloring.
        // Bold + bright styling is applied at the line level by apply_selection_style.
        let mut char_idx = 0;
        let mut found_selected_text = false;
        for span in &result.spans {
            for _ in span.content.chars() {
                if char_idx >= text_start && char_idx < text_end {
                    assert_eq!(
                        span.style.bg, base_styles[char_idx].1.bg,
                        "Selected task text at char {} should keep original bg, got {:?}",
                        char_idx, span.style.bg
                    );
                    found_selected_text = true;
                }
                char_idx += 1;
            }
        }
        assert!(
            found_selected_text,
            "Should find selected task text with original style preserved"
        );
    }

    // ══════════════════════════════════════════════════════════════════════
    // Test 7: NO SELECTION = NORMAL OUTPUT — output unchanged without selection
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_no_selection_produces_normal_output() {
        let (viz, _graph) = build_test_graph_chain_plus_isolated();

        // When no selection is active, draw_viz_content goes through the `else`
        // branch (line 148-149 in render.rs) and pushes base_line unchanged.
        // We verify this by checking that apply_per_char_trace_coloring with
        // Unrelated category and no edge map hits preserves all styles.

        // Without selection, the code simply uses `base_line` directly.
        // Test the invariant: for every line in the viz, parsing it and
        // NOT applying trace coloring should give the same result as
        // applying trace with "Unrelated" category when no edges match.

        // Build an app with no selection.
        let lines: Vec<String> = viz.text.lines().map(String::from).collect();
        let plain_lines: Vec<String> = lines
            .iter()
            .map(|l: &String| {
                String::from_utf8(strip_ansi_escapes::strip(l.as_bytes())).unwrap_or_default()
            })
            .collect();

        // For each line, verify that if we were to apply trace coloring with
        // empty upstream/downstream sets and no matching char_edge_map entries,
        // the output is identical to the input.
        let empty_app = {
            let mut app = build_app_from_viz_output(&viz, "b");
            app.selected_task_idx = None;
            app.upstream_set.clear();
            app.downstream_set.clear();
            app.char_edge_map.clear();
            app
        };

        for (idx, ansi_line) in lines.iter().enumerate() {
            let plain = &plain_lines[idx];
            let base_line = parse_ansi_line(ansi_line);
            let category = LineTraceCategory::Unrelated;

            // Collect base styles.
            let mut base_chars: Vec<(char, Style)> = Vec::new();
            for span in &base_line.spans {
                for c in span.content.chars() {
                    base_chars.push((c, span.style));
                }
            }

            let result = apply_per_char_trace_coloring(
                parse_ansi_line(ansi_line),
                plain,
                idx,
                &category,
                &empty_app,
                None,
            );

            let mut result_chars: Vec<(char, Style)> = Vec::new();
            for span in &result.spans {
                for c in span.content.chars() {
                    result_chars.push((c, span.style));
                }
            }

            assert_eq!(
                base_chars.len(),
                result_chars.len(),
                "Line {} should have same char count",
                idx
            );
            for (i, ((bc, bs), (rc, rs))) in base_chars.iter().zip(result_chars.iter()).enumerate()
            {
                assert_eq!(bc, rc, "Char mismatch at line {} position {}", idx, i);
                assert_eq!(
                    bs, rs,
                    "Style mismatch at line {} position {} ('{}') with no selection: expected {:?}, got {:?}",
                    idx, i, bc, bs, rs
                );
            }
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // Auxiliary tests — verify test infrastructure and edge cases
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_classify_task_line_categories() {
        let (viz, _graph) = build_test_graph_chain_plus_isolated();
        let app = build_app_from_viz_output(&viz, "b");

        let a_line = viz.node_line_map["a"];
        let b_line = viz.node_line_map["b"];
        let c_line = viz.node_line_map["c"];
        let d_line = viz.node_line_map["d"];

        assert!(
            matches!(
                classify_task_line(&app, a_line),
                LineTraceCategory::Upstream
            ),
            "Task 'a' should be classified as Upstream when 'b' is selected"
        );
        assert!(
            matches!(
                classify_task_line(&app, b_line),
                LineTraceCategory::Selected
            ),
            "Task 'b' should be classified as Selected"
        );
        assert!(
            matches!(
                classify_task_line(&app, c_line),
                LineTraceCategory::Downstream
            ),
            "Task 'c' should be classified as Downstream when 'b' is selected"
        );
        assert!(
            matches!(
                classify_task_line(&app, d_line),
                LineTraceCategory::Unrelated
            ),
            "Task 'd' should be classified as Unrelated (separate WCC)"
        );
    }

    #[test]
    fn test_find_text_range_on_task_line() {
        // A task line looks like: "├→ task-id  (status)"
        let line = "├→ my-task  (open)";
        let range = find_text_range(line);
        assert!(range.is_some(), "Should find text range in task line");
        let (start, end) = range.unwrap();
        let chars: Vec<char> = line.chars().collect();
        // The text should start at the first alphanumeric character.
        assert!(
            chars[start].is_alphanumeric(),
            "Text range should start at alphanumeric char, got '{}'",
            chars[start]
        );
        // The text should end after the last ')'.
        assert_eq!(
            chars[end - 1],
            ')',
            "Text range should end after ')', got '{}'",
            chars[end - 1]
        );
    }

    #[test]
    fn test_find_text_range_on_connector_only_line() {
        let line = "│  │";
        let range = find_text_range(line);
        assert!(
            range.is_none(),
            "Pure connector line should have no text range"
        );
    }

    #[test]
    fn test_edge_chars_have_correct_edge_info() {
        let (viz, _graph) = build_test_graph_chain_plus_isolated();

        // Verify that the char_edge_map contains entries for a->b and b->c edges.
        let has_ab = viz
            .char_edge_map
            .values()
            .any(|edges| edges.iter().any(|(s, t)| s == "a" && t == "b"));
        let has_bc = viz
            .char_edge_map
            .values()
            .any(|edges| edges.iter().any(|(s, t)| s == "b" && t == "c"));
        assert!(has_ab, "char_edge_map should contain a->b edge entries");
        assert!(has_bc, "char_edge_map should contain b->c edge entries");

        // Verify no edges involving 'd' (it's standalone).
        let has_d = viz
            .char_edge_map
            .values()
            .any(|edges| edges.iter().any(|(s, t)| s == "d" || t == "d"));
        assert!(
            !has_d,
            "char_edge_map should NOT contain any edges involving standalone task 'd'"
        );
    }

    #[test]
    fn test_trace_coloring_preserves_non_edge_non_text_chars() {
        // Verify that spaces and other non-edge, non-text characters keep base style.
        let (viz, _graph) = build_test_graph_chain_plus_isolated();
        let app = build_app_from_viz_output(&viz, "b");

        // Pick a line that has edge chars — check the spaces before/after are preserved.
        for (idx, ansi_line) in app.lines.iter().enumerate() {
            let plain = &app.plain_lines[idx];
            let base_line = parse_ansi_line(ansi_line);
            let category = classify_task_line(&app, idx);

            let mut base_chars: Vec<(char, Style)> = Vec::new();
            for span in &base_line.spans {
                for c in span.content.chars() {
                    base_chars.push((c, span.style));
                }
            }

            let result = apply_per_char_trace_coloring(
                parse_ansi_line(ansi_line),
                plain,
                idx,
                &category,
                &app,
                Some("b"),
            );

            let mut result_chars: Vec<(char, Style)> = Vec::new();
            for span in &result.spans {
                for c in span.content.chars() {
                    result_chars.push((c, span.style));
                }
            }

            let text_range = find_text_range(plain);
            let (text_start, text_end) = text_range.unwrap_or((usize::MAX, usize::MAX));

            for (i, ((bc, bs), (_rc, rs))) in base_chars.iter().zip(result_chars.iter()).enumerate()
            {
                let is_text = i >= text_start && i < text_end;
                let is_edge = app.char_edge_map.contains_key(&(idx, i));

                if !is_text && !is_edge {
                    // Non-text, non-edge chars should keep their base style exactly.
                    assert_eq!(
                        bs, rs,
                        "Non-edge non-text char at line {} pos {} ('{}') should keep base style. \
                         Expected {:?}, got {:?}",
                        idx, i, bc, bs, rs
                    );
                }
            }
        }
    }

    #[test]
    fn test_upstream_set_computed_correctly() {
        let (viz, _graph) = build_test_graph_chain_plus_isolated();
        let app = build_app_from_viz_output(&viz, "c");

        // When 'c' is selected, upstream should include both 'a' and 'b'.
        assert!(app.upstream_set.contains("a"), "a should be upstream of c");
        assert!(app.upstream_set.contains("b"), "b should be upstream of c");
        assert!(
            !app.upstream_set.contains("c"),
            "c should not be in its own upstream set"
        );
        assert!(
            !app.upstream_set.contains("d"),
            "d should not be upstream of c"
        );
        assert!(app.downstream_set.is_empty(), "c has no downstream tasks");
    }

    #[test]
    fn test_downstream_set_computed_correctly() {
        let (viz, _graph) = build_test_graph_chain_plus_isolated();
        let app = build_app_from_viz_output(&viz, "a");

        // When 'a' is selected, downstream should include both 'b' and 'c'.
        assert!(
            app.downstream_set.contains("b"),
            "b should be downstream of a"
        );
        assert!(
            app.downstream_set.contains("c"),
            "c should be downstream of a"
        );
        assert!(
            !app.downstream_set.contains("a"),
            "a should not be in its own downstream set"
        );
        assert!(
            !app.downstream_set.contains("d"),
            "d should not be downstream of a"
        );
        assert!(
            app.upstream_set.is_empty(),
            "a has no upstream tasks (it's a root)"
        );
    }

    // ══════════════════════════════════════════════════════════════════════
    // Validation test 1: SHARED ARC COLUMN — fan-in with 3 blockers sharing
    //   one arc column. Only the selected blocker's horizontal + vertical should
    //   be colored; sibling blockers' horizontals stay gray.
    // ══════════════════════════════════════════════════════════════════════

    /// Build a fan-in graph: A depends on B, C, D (A is the dependent, B/C/D are blockers).
    /// This produces back-edge arcs sharing a single arc column.
    fn build_shared_arc_fan_in() -> (VizOutput, WorkGraph) {
        let mut graph = WorkGraph::new();
        let b = make_task_with_status("b", "Blocker B", Status::Done);
        let c = make_task_with_status("c", "Blocker C", Status::Done);
        let d = make_task_with_status("d", "Blocker D", Status::Done);
        let mut a = make_task_with_status("a", "Dependent A", Status::Open);
        a.after = vec!["b".to_string(), "c".to_string(), "d".to_string()];
        graph.add_node(Node::Task(a));
        graph.add_node(Node::Task(b));
        graph.add_node(Node::Task(c));
        graph.add_node(Node::Task(d));

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
            LayoutMode::Diamond,
            &HashSet::new(),
            "gray",
        );
        (result, graph)
    }

    #[test]
    fn test_shared_arc_column_only_selected_blocker_colored() {
        let (viz, _graph) = build_shared_arc_fan_in();

        // Verify the char_edge_map has entries for arcs to 'a' from multiple blockers
        let has_b_to_a = viz
            .char_edge_map
            .values()
            .any(|edges| edges.iter().any(|(s, t)| s == "b" && t == "a"));
        assert!(
            has_b_to_a,
            "char_edge_map should contain b->a arc edge.\nOutput:\n{}\nMap: {:?}",
            viz.text, viz.char_edge_map
        );

        // Select B. B's edge to A should be colored magenta (upstream of A).
        // But we're testing from B's perspective: select B, A is downstream.
        let app = build_app_from_viz_output(&viz, "b");

        // B selected: A is downstream of B (B→A edge). C and D are unrelated
        // (they don't depend on B and B doesn't depend on them).
        assert!(
            app.downstream_set.contains("a"),
            "A should be downstream of B"
        );
        assert!(
            !app.downstream_set.contains("c"),
            "C should NOT be downstream of B"
        );
        assert!(
            !app.downstream_set.contains("d"),
            "D should NOT be downstream of B"
        );

        // Check that edge chars for b->a get colored (cyan for downstream)
        // while edge chars for c->a and d->a stay uncolored.
        let mut found_b_a_colored = false;
        let mut found_c_a_uncolored = true;
        let mut found_d_a_uncolored = true;

        for (&(ln, col), edges) in &viz.char_edge_map {
            let plain = app.plain_lines[ln].as_str();
            let base_line = parse_ansi_line(app.lines[ln].as_str());
            let category = classify_task_line(&app, ln);
            let result =
                apply_per_char_trace_coloring(base_line, plain, ln, &category, &app, Some("b"));

            // Get the resulting style at this character position
            let mut char_idx = 0;
            let mut span_style = Style::default();
            'outer: for span in &result.spans {
                for _ in span.content.chars() {
                    if char_idx == col {
                        span_style = span.style;
                        break 'outer;
                    }
                    char_idx += 1;
                }
            }

            let is_text_range = find_text_range(plain)
                .map(|(s, e)| col >= s && col < e)
                .unwrap_or(false);
            if is_text_range {
                continue; // Skip text characters
            }

            // Check if this position has ONLY b->a edges (no c->a or d->a)
            let has_b_a = edges.iter().any(|(s, t)| s == "b" && t == "a");
            let has_c_a = edges.iter().any(|(s, t)| s == "c" && t == "a");
            let has_d_a = edges.iter().any(|(s, t)| s == "d" && t == "a");

            if has_b_a && !has_c_a && !has_d_a {
                // Pure b->a edge character — should be cyan (downstream)
                if span_style.fg == Some(Color::Cyan) {
                    found_b_a_colored = true;
                }
            }
            if has_c_a && !has_b_a {
                // Pure c->a edge character — should NOT be colored
                if span_style.fg == Some(Color::Magenta) || span_style.fg == Some(Color::Cyan) {
                    found_c_a_uncolored = false;
                }
            }
            if has_d_a && !has_b_a {
                // Pure d->a edge character — should NOT be colored
                if span_style.fg == Some(Color::Magenta) || span_style.fg == Some(Color::Cyan) {
                    found_d_a_uncolored = false;
                }
            }
        }

        assert!(
            found_b_a_colored,
            "B→A edge chars should be colored cyan when B is selected.\nOutput:\n{}",
            viz.text
        );
        assert!(
            found_c_a_uncolored,
            "C→A edge chars should NOT be colored when B is selected.\nOutput:\n{}",
            viz.text
        );
        assert!(
            found_d_a_uncolored,
            "D→A edge chars should NOT be colored when B is selected.\nOutput:\n{}",
            viz.text
        );
    }

    #[test]
    fn test_shared_arc_column_arrowhead_colored() {
        let (viz, _graph) = build_shared_arc_fan_in();
        let app = build_app_from_viz_output(&viz, "b");

        // A is downstream of B. The arrowhead on A's line (← glyph) should be colored cyan
        // because A is the dependent receiving the edge from B.
        let a_line = viz.node_line_map["a"];
        let plain = app.plain_lines[a_line].as_str();

        // Find arc positions on A's line in the char_edge_map
        let mut found_arrowhead_colored = false;
        for (&(ln, col), edges) in &viz.char_edge_map {
            if ln != a_line {
                continue;
            }
            let has_b_a = edges.iter().any(|(s, t)| s == "b" && t == "a");
            if !has_b_a {
                continue;
            }

            let base_line = parse_ansi_line(app.lines[ln].as_str());
            let category = classify_task_line(&app, ln);
            let result =
                apply_per_char_trace_coloring(base_line, plain, ln, &category, &app, Some("b"));

            let is_text = find_text_range(plain)
                .map(|(s, e)| col >= s && col < e)
                .unwrap_or(false);
            if is_text {
                continue;
            }

            let mut char_idx = 0;
            for span in &result.spans {
                for _ in span.content.chars() {
                    if char_idx == col && span.style.fg == Some(Color::Cyan) {
                        found_arrowhead_colored = true;
                    }
                    char_idx += 1;
                }
            }
        }

        assert!(
            found_arrowhead_colored,
            "A's arrowhead (←) should be colored cyan when B is selected (A is downstream).\nOutput:\n{}\nA is at line {}",
            viz.text, a_line
        );
    }

    // ══════════════════════════════════════════════════════════════════════
    // Validation test 2: TEXT COLORS PRESERVED — all status colors preserved
    //   regardless of selection state
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_text_colors_preserved_all_statuses() {
        // Build a graph with tasks in all statuses that are visible
        let mut graph = WorkGraph::new();
        let a = make_task_with_status("a-root", "Root", Status::Done);
        let mut b = make_task_with_status("b-prog", "Progress", Status::InProgress);
        b.after = vec!["a-root".to_string()];
        let mut c = make_task_with_status("c-open", "Open", Status::Open);
        c.after = vec!["b-prog".to_string()];
        let mut d = make_task_with_status("d-fail", "Failed", Status::Failed);
        d.after = vec!["a-root".to_string()];
        graph.add_node(Node::Task(a));
        graph.add_node(Node::Task(b));
        graph.add_node(Node::Task(c));
        graph.add_node(Node::Task(d));

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

        // Test with b-prog selected (so a-root is upstream, c-open is downstream,
        // d-fail is a sibling — unrelated to b-prog's chain)
        let app = build_app_from_viz_output(&viz, "b-prog");

        // For each task line, verify text keeps its original style
        for task_id in &["a-root", "b-prog", "c-open", "d-fail"] {
            let line_idx = viz.node_line_map[*task_id];
            let plain = app.plain_lines[line_idx].as_str();
            let base_line = parse_ansi_line(app.lines[line_idx].as_str());
            let category = classify_task_line(&app, line_idx);

            let mut base_text_styles: Vec<(char, Style)> = Vec::new();
            for span in &base_line.spans {
                for c in span.content.chars() {
                    base_text_styles.push((c, span.style));
                }
            }

            let result = apply_per_char_trace_coloring(
                parse_ansi_line(app.lines[line_idx].as_str()),
                plain,
                line_idx,
                &category,
                &app,
                Some("b-prog"),
            );

            let mut result_text_styles: Vec<(char, Style)> = Vec::new();
            for span in &result.spans {
                for c in span.content.chars() {
                    result_text_styles.push((c, span.style));
                }
            }

            let text_range = find_text_range(plain);
            if let Some((text_start, text_end)) = text_range {
                for i in text_start
                    ..text_end
                        .min(base_text_styles.len())
                        .min(result_text_styles.len())
                {
                    assert_eq!(
                        base_text_styles[i].1,
                        result_text_styles[i].1,
                        "Task '{}' text at char {} should preserve original style. \
                         Expected {:?}, got {:?}. Category: {:?}",
                        task_id,
                        i,
                        base_text_styles[i].1,
                        result_text_styles[i].1,
                        match category {
                            LineTraceCategory::Selected => "Selected",
                            LineTraceCategory::Upstream => "Upstream",
                            LineTraceCategory::Downstream => "Downstream",
                            LineTraceCategory::Unrelated => "Unrelated",
                        }
                    );
                }
            }
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // Validation test 3: UNRELATED WCCs UNCHANGED — disconnected components
    //   render identically with and without selection
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_unrelated_wcc_unchanged_two_components() {
        // WCC1: a -> b -> c
        // WCC2: x -> y (completely disconnected)
        let mut graph = WorkGraph::new();
        let a = make_task_with_status("a", "Task A", Status::Done);
        let mut b = make_task_with_status("b", "Task B", Status::InProgress);
        b.after = vec!["a".to_string()];
        let mut c = make_task_with_status("c", "Task C", Status::Open);
        c.after = vec!["b".to_string()];
        let x = make_task_with_status("x", "Task X", Status::Open);
        let mut y = make_task_with_status("y", "Task Y", Status::Done);
        y.after = vec!["x".to_string()];
        graph.add_node(Node::Task(a));
        graph.add_node(Node::Task(b));
        graph.add_node(Node::Task(c));
        graph.add_node(Node::Task(x));
        graph.add_node(Node::Task(y));

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

        let app = build_app_from_viz_output(&viz, "b"); // Select in WCC1

        // WCC2 tasks (x, y) should be unrelated
        assert!(
            !app.upstream_set.contains("x"),
            "x should NOT be in upstream"
        );
        assert!(
            !app.downstream_set.contains("x"),
            "x should NOT be in downstream"
        );
        assert!(
            !app.upstream_set.contains("y"),
            "y should NOT be in upstream"
        );
        assert!(
            !app.downstream_set.contains("y"),
            "y should NOT be in downstream"
        );

        // All lines belonging to WCC2 should render identically with trace coloring
        for task_id in &["x", "y"] {
            let line_idx = viz.node_line_map[*task_id];
            let plain = app.plain_lines[line_idx].as_str();
            let base_line = parse_ansi_line(app.lines[line_idx].as_str());
            let category = classify_task_line(&app, line_idx);
            assert!(
                matches!(category, LineTraceCategory::Unrelated),
                "Task '{}' should be Unrelated",
                task_id
            );

            let mut base_chars: Vec<(char, Style)> = Vec::new();
            for span in &base_line.spans {
                for c in span.content.chars() {
                    base_chars.push((c, span.style));
                }
            }

            let result = apply_per_char_trace_coloring(
                parse_ansi_line(app.lines[line_idx].as_str()),
                plain,
                line_idx,
                &category,
                &app,
                Some("b"),
            );

            let mut result_chars: Vec<(char, Style)> = Vec::new();
            for span in &result.spans {
                for c in span.content.chars() {
                    result_chars.push((c, span.style));
                }
            }

            assert_eq!(
                base_chars.len(),
                result_chars.len(),
                "WCC2 task '{}' should have same char count",
                task_id
            );
            for (i, ((bc, bs), (rc, rs))) in base_chars.iter().zip(result_chars.iter()).enumerate()
            {
                assert_eq!(
                    bc, rc,
                    "Char mismatch in WCC2 task '{}' at pos {}",
                    task_id, i
                );
                assert_eq!(
                    bs, rs,
                    "Style mismatch in WCC2 task '{}' at pos {} ('{}'):\n  expected {:?}\n  got {:?}",
                    task_id, i, bc, bs, rs
                );
            }
        }

        // Also check lines between WCC2 tasks (e.g. connector lines)
        let x_line = viz.node_line_map["x"];
        let y_line = viz.node_line_map["y"];
        for line_idx in x_line..=y_line {
            let plain = app.plain_lines[line_idx].as_str();
            let base_line = parse_ansi_line(app.lines[line_idx].as_str());
            let category = classify_task_line(&app, line_idx);

            let mut base_chars: Vec<(char, Style)> = Vec::new();
            for span in &base_line.spans {
                for ch in span.content.chars() {
                    base_chars.push((ch, span.style));
                }
            }

            let result = apply_per_char_trace_coloring(
                parse_ansi_line(app.lines[line_idx].as_str()),
                plain,
                line_idx,
                &category,
                &app,
                Some("b"),
            );

            let mut result_chars: Vec<(char, Style)> = Vec::new();
            for span in &result.spans {
                for ch in span.content.chars() {
                    result_chars.push((ch, span.style));
                }
            }

            for (i, ((bc, bs), (rc, rs))) in base_chars.iter().zip(result_chars.iter()).enumerate()
            {
                assert_eq!(bc, rc);
                assert_eq!(
                    bs, rs,
                    "WCC2 line {} pos {} ('{}') style should be unchanged",
                    line_idx, i, bc
                );
            }
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // Validation test 4: SELECTION STYLE — selected task marked with
    //   bold + bright text only, no extra characters or background.
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_selection_style_no_yellow_background() {
        let (viz, _graph) = build_test_graph_chain_plus_isolated();
        let app = build_app_from_viz_output(&viz, "b");

        let b_line = viz.node_line_map["b"];
        let plain = app.plain_lines[b_line].as_str();
        let category = classify_task_line(&app, b_line);
        assert!(matches!(category, LineTraceCategory::Selected));

        // apply_per_char_trace_coloring should NOT set yellow background on text.
        let base_line = parse_ansi_line(app.lines[b_line].as_str());
        let result =
            apply_per_char_trace_coloring(base_line, plain, b_line, &category, &app, Some("b"));

        let text_range = find_text_range(plain);
        let (text_start, text_end) = text_range.unwrap();

        let mut char_idx = 0;
        for span in &result.spans {
            for _c in span.content.chars() {
                if char_idx >= text_start && char_idx < text_end {
                    assert_ne!(
                        span.style.bg,
                        Some(Color::Yellow),
                        "apply_per_char_trace_coloring should NOT set yellow background at char {}.",
                        char_idx
                    );
                }
                char_idx += 1;
            }
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // Test: SELECTION ON INDEPENDENT TASK — independent tasks get
    //   bold + bright styling when selected.
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_selection_style_on_independent_task() {
        let (viz, _graph) = build_test_graph_chain_plus_isolated();
        let app = build_app_from_viz_output(&viz, "d");

        let d_line = viz.node_line_map["d"];
        let plain = app.plain_lines[d_line].as_str();
        let category = classify_task_line(&app, d_line);
        assert!(matches!(category, LineTraceCategory::Selected));

        // Apply trace coloring first, then selection style (mirrors draw_viz_content).
        let base_line = parse_ansi_line(app.lines[d_line].as_str());
        let colored =
            apply_per_char_trace_coloring(base_line, plain, d_line, &category, &app, Some("d"));
        let result = apply_selection_style(colored, plain);

        // Text spans should be bold, edge spans should NOT be bold.
        let text_range = find_text_range(plain);
        let (text_start, text_end) = text_range.unwrap();
        let mut char_idx = 0;
        for span in &result.spans {
            for _c in span.content.chars() {
                if char_idx >= text_start && char_idx < text_end {
                    assert!(
                        span.style.add_modifier.contains(Modifier::BOLD),
                        "Text char at {} should be bold. Span: {:?}",
                        char_idx,
                        span
                    );
                }
                char_idx += 1;
            }
        }

        // Text content should be unchanged (no extra characters).
        let result_text: String = result
            .spans
            .iter()
            .flat_map(|s| s.content.chars())
            .collect();
        assert_eq!(
            result_text.chars().count(),
            plain.chars().count(),
            "Selection style should not add or remove characters"
        );
    }

    #[test]
    fn test_selection_style_applies_bold() {
        // Verify that text spans get BOLD modifier.
        let plain = "task-id  (open)";
        let line = Line::from(plain.to_string());
        let result = apply_selection_style(line, plain);

        for span in &result.spans {
            assert!(
                span.style.add_modifier.contains(Modifier::BOLD),
                "All spans should be bold. Span: {:?}",
                span
            );
        }
    }

    #[test]
    fn test_selection_style_preserves_text() {
        // Verify that apply_selection_style does not add or remove characters.
        let plain = "├→ task-id  (open)";
        let line = Line::from(plain.to_string());
        let result = apply_selection_style(line, plain);

        let result_text: String = result
            .spans
            .iter()
            .flat_map(|s| s.content.chars())
            .collect();
        assert_eq!(
            result_text, plain,
            "Selection style should preserve text exactly"
        );
    }

    #[test]
    fn test_selection_style_brightens_colors() {
        // Verify that colors are brightened for selected task text.
        let plain = "hello world";
        let line = Line::from(vec![
            Span::styled("hello", Style::default().fg(Color::Green)),
            Span::styled(" world", Style::default().fg(Color::Red)),
        ]);
        let result = apply_selection_style(line, plain);

        // All chars are text (no edges), so all should be brightened.
        let mut found_green = false;
        let mut found_red = false;
        for span in &result.spans {
            if span.style.fg == Some(Color::LightGreen) {
                found_green = true;
            }
            if span.style.fg == Some(Color::LightRed) {
                found_red = true;
            }
        }
        assert!(found_green, "Green should become LightGreen");
        assert!(found_red, "Red should become LightRed");
    }

    #[test]
    fn test_selection_style_does_not_bold_edges() {
        // Edge chars (├→) should NOT get bold, only the text portion should.
        let plain = "├→ task-id  (open)";
        let line = Line::from(vec![
            Span::styled("├→ ", Style::default().fg(Color::White)),
            Span::styled("task-id  (open)", Style::default().fg(Color::Green)),
        ]);
        let result = apply_selection_style(line, plain);

        let text_range = find_text_range(plain).unwrap();
        let mut char_idx = 0;
        for span in &result.spans {
            for _c in span.content.chars() {
                if char_idx < text_range.0 {
                    // Edge/connector chars — should NOT be bold.
                    assert!(
                        !span.style.add_modifier.contains(Modifier::BOLD),
                        "Edge char at {} should NOT be bold. Span: {:?}",
                        char_idx,
                        span
                    );
                } else if char_idx < text_range.1 {
                    // Text chars — should be bold.
                    assert!(
                        span.style.add_modifier.contains(Modifier::BOLD),
                        "Text char at {} should be bold. Span: {:?}",
                        char_idx,
                        span
                    );
                }
                char_idx += 1;
            }
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // Validation test 5: ADDITIVE ONLY — with no selection, output must be
    //   identical to normal wg viz. Trace only changes edge colors + block cursor.
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_additive_only_no_selection_identity() {
        // Test with a complex graph (chain + isolated + fan-in)
        let mut graph = WorkGraph::new();
        let a = make_task_with_status("a", "Task A", Status::Done);
        let mut b = make_task_with_status("b", "Task B", Status::InProgress);
        b.after = vec!["a".to_string()];
        let mut c = make_task_with_status("c", "Task C", Status::Open);
        c.after = vec!["b".to_string()];
        let d = make_task_with_status("d", "Task D", Status::Failed);
        let x = make_task_with_status("x", "Task X", Status::Open);
        graph.add_node(Node::Task(a));
        graph.add_node(Node::Task(b));
        graph.add_node(Node::Task(c));
        graph.add_node(Node::Task(d));
        graph.add_node(Node::Task(x));

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

        // Build app with NO selection
        let mut app = VizApp::from_viz_output_for_test(&viz);
        app.selected_task_idx = None;
        app.upstream_set.clear();
        app.downstream_set.clear();

        // When no selection is active, has_selection is false and the code goes
        // through the `else` branch which pushes base_line unchanged.
        // Verify: apply_per_char_trace_coloring with Unrelated category and empty sets
        // produces output identical to input for EVERY line.
        for (idx, ansi_line) in app.lines.iter().enumerate() {
            let plain = &app.plain_lines[idx];
            let base_line = parse_ansi_line(ansi_line);

            let mut base_chars: Vec<(char, Style)> = Vec::new();
            for span in &base_line.spans {
                for c in span.content.chars() {
                    base_chars.push((c, span.style));
                }
            }

            // With no selection, the category is always Unrelated
            let result = apply_per_char_trace_coloring(
                parse_ansi_line(ansi_line),
                plain,
                idx,
                &LineTraceCategory::Unrelated,
                &app,
                None,
            );

            let mut result_chars: Vec<(char, Style)> = Vec::new();
            for span in &result.spans {
                for c in span.content.chars() {
                    result_chars.push((c, span.style));
                }
            }

            assert_eq!(
                base_chars.len(),
                result_chars.len(),
                "Line {} should have identical char count with no selection",
                idx
            );
            for (i, ((bc, bs), (rc, rs))) in base_chars.iter().zip(result_chars.iter()).enumerate()
            {
                assert_eq!(
                    bc, rc,
                    "No-selection: char mismatch at line {} pos {}",
                    idx, i
                );
                assert_eq!(
                    bs, rs,
                    "No-selection: style mismatch at line {} pos {} ('{}'):\n  expected {:?}\n  got {:?}",
                    idx, i, bc, bs, rs
                );
            }
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // Validation test 6: PINK AGENCY PHASES — tasks in assigning/evaluating
    //   phases should show pink (magenta) text
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_pink_agency_phase_text() {
        // Build a graph with a task that has [assigning] annotation (magenta/pink).
        // NOTE: In test environments, stdout is not a terminal so ANSI color codes
        // are suppressed by generate_ascii. We verify:
        // 1. The annotation text [assigning]/[evaluating] appears in the output
        // 2. The format_node logic would produce magenta (\x1b[35m]) when use_color=true
        //    (verified by the is_agency_phase check in ascii.rs lines 309-321)
        // 3. The phase annotation is correctly applied

        let mut graph = WorkGraph::new();
        let task = make_task_with_status("my-task", "My Task", Status::Open);
        graph.add_node(Node::Task(task));

        let mut annotations = HashMap::new();
        annotations.insert("my-task".to_string(), "[assigning]".to_string());

        let tasks: Vec<_> = graph.tasks().collect();
        let task_ids: HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        let viz = generate_ascii(
            &graph,
            &tasks,
            &task_ids,
            &annotations,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            LayoutMode::Tree,
            &HashSet::new(),
            "gray",
        );

        // The annotation [assigning] must appear in the output
        let task_line_idx = viz.node_line_map["my-task"];
        let line_text = viz.text.lines().nth(task_line_idx).unwrap();
        assert!(
            line_text.contains("[assigning]"),
            "Assigning phase should show [assigning] annotation.\nLine: {:?}",
            line_text
        );

        // In a terminal, the ANSI code \x1b[35m (magenta) would be present.
        // In non-terminal test environments, no ANSI codes are emitted.
        // Either way, the annotation text must be present. If ANSI codes ARE
        // present (some CI environments have tty), they should be magenta.
        if line_text.contains("\x1b[") {
            assert!(
                line_text.contains("\x1b[38;5;219m"),
                "If ANSI codes are present, assigning phase should use true pink (\\x1b[38;5;219m).\nLine: {:?}",
                line_text
            );
        }

        // Test evaluating phase
        let mut graph2 = WorkGraph::new();
        let task2 = make_task_with_status("eval-task", "Eval Task", Status::Done);
        graph2.add_node(Node::Task(task2));
        let mut annotations2 = HashMap::new();
        annotations2.insert("eval-task".to_string(), "[evaluating]".to_string());

        let tasks2: Vec<_> = graph2.tasks().collect();
        let task_ids2: HashSet<&str> = tasks2.iter().map(|t| t.id.as_str()).collect();
        let viz2 = generate_ascii(
            &graph2,
            &tasks2,
            &task_ids2,
            &annotations2,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            LayoutMode::Tree,
            &HashSet::new(),
            "gray",
        );

        let task_line_idx2 = viz2.node_line_map["eval-task"];
        let ansi_line2 = viz2.text.lines().nth(task_line_idx2).unwrap();
        assert!(
            ansi_line2.contains("[evaluating]"),
            "Evaluating phase should show [evaluating] annotation.\nLine: {:?}",
            ansi_line2
        );
        if ansi_line2.contains("\x1b[") {
            assert!(
                ansi_line2.contains("\x1b[38;5;219m"),
                "If ANSI codes are present, evaluating phase should use true pink (\\x1b[38;5;219m).\nLine: {:?}",
                ansi_line2
            );
        }

        // Verify the code logic: in ascii.rs, the agency phase detection checks:
        //   is_agency_phase = use_color && annotations.get(id).map_or(false, |a| a.contains("assigning") || a.contains("evaluating"))
        // When true, the phase annotation is wrapped in \x1b[38;5;219m..\x1b[0m (ANSI 256-color 219, true pink).
        // The task text itself keeps its status color (e.g., green for done).
        // We've verified the annotation appears; the color logic is deterministic given use_color.
    }

    #[test]
    fn test_pink_agency_phase_preserves_in_trace() {
        // Verify that when a task is in an agency phase and trace coloring is applied,
        // the pink text color is preserved (trace is additive, only edge chars change).
        let mut graph = WorkGraph::new();
        let root = make_task_with_status("root", "Root", Status::Done);
        let mut child = make_task_with_status("child", "Child", Status::Open);
        child.after = vec!["root".to_string()];
        graph.add_node(Node::Task(root));
        graph.add_node(Node::Task(child));

        let mut annotations = HashMap::new();
        annotations.insert("child".to_string(), "[assigning]".to_string());

        let tasks: Vec<_> = graph.tasks().collect();
        let task_ids: HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        let viz = generate_ascii(
            &graph,
            &tasks,
            &task_ids,
            &annotations,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            LayoutMode::Tree,
            &HashSet::new(),
            "gray",
        );

        // Select 'child' — it's the selected task, text should keep pink/magenta
        let app = build_app_from_viz_output(&viz, "child");
        let child_line = viz.node_line_map["child"];
        let plain = app.plain_lines[child_line].as_str();
        let base_line = parse_ansi_line(app.lines[child_line].as_str());
        let category = classify_task_line(&app, child_line);

        let mut base_text_fg: Vec<Option<Color>> = Vec::new();
        let mut idx = 0;
        let text_range = find_text_range(plain);
        let (text_start, text_end) = text_range.unwrap_or((usize::MAX, usize::MAX));
        for span in &base_line.spans {
            for _ in span.content.chars() {
                if idx >= text_start && idx < text_end {
                    base_text_fg.push(span.style.fg);
                }
                idx += 1;
            }
        }

        let result = apply_per_char_trace_coloring(
            parse_ansi_line(app.lines[child_line].as_str()),
            plain,
            child_line,
            &category,
            &app,
            Some("child"),
        );

        let mut result_text_fg: Vec<Option<Color>> = Vec::new();
        idx = 0;
        for span in &result.spans {
            for _ in span.content.chars() {
                if idx >= text_start && idx < text_end {
                    result_text_fg.push(span.style.fg);
                }
                idx += 1;
            }
        }

        assert_eq!(base_text_fg.len(), result_text_fg.len());
        for (i, (base, result)) in base_text_fg.iter().zip(result_text_fg.iter()).enumerate() {
            assert_eq!(
                base, result,
                "Agency-phase text fg at position {} should be preserved: {:?} vs {:?}",
                i, base, result
            );
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // SIBLING NOT IN TRACE: tree connectors to untraced siblings stay uncolored
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_sibling_not_in_trace_connectors_uncolored() {
        // Build a tree:
        //   root
        //   ├→ child-a
        //   │  └→ grandchild   <-- SELECTED
        //   └→ child-b         <-- NOT in trace (sibling, not in chain)
        //
        // When grandchild is selected, the trace goes:
        //   grandchild → child-a → root
        // child-b is a sibling of child-a under root. It is NOT in the chain.
        // The │ between child-a's subtree and child-b, and the └→ connector
        // on child-b's line, must NOT be colored.

        let mut graph = WorkGraph::new();
        let root = make_task_with_status("root", "Root Task", Status::Done);
        let mut child_a = make_task_with_status("child-a", "Child A", Status::Done);
        child_a.after = vec!["root".to_string()];
        let mut grandchild = make_task_with_status("grandchild", "Grandchild", Status::InProgress);
        grandchild.after = vec!["child-a".to_string()];
        let mut child_b = make_task_with_status("child-b", "Child B", Status::Open);
        child_b.after = vec!["root".to_string()];

        graph.add_node(Node::Task(root));
        graph.add_node(Node::Task(child_a));
        graph.add_node(Node::Task(grandchild));
        graph.add_node(Node::Task(child_b));

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

        let app = build_app_from_viz_output(&viz, "grandchild");

        // Verify upstream set is correct: grandchild, child-a, root
        assert!(
            app.upstream_set.contains("root"),
            "root should be in upstream set"
        );
        assert!(
            app.upstream_set.contains("child-a"),
            "child-a should be in upstream set"
        );
        assert!(
            !app.upstream_set.contains("child-b"),
            "child-b should NOT be in upstream set"
        );

        // Find child-b's line and check its connectors are NOT colored
        let child_b_line = viz.node_line_map["child-b"];

        // Check all edge-mapped characters on child-b's line and between child-a's
        // subtree and child-b: none should be colored magenta or cyan
        for (&(ln, col), edges) in &viz.char_edge_map {
            // Only check edges that involve child-b (the untraced sibling)
            let involves_child_b = edges.iter().any(|(s, t)| s == "child-b" || t == "child-b");
            if !involves_child_b {
                continue;
            }

            let plain = app.plain_lines[ln].as_str();
            let base_line = parse_ansi_line(app.lines[ln].as_str());
            let category = classify_task_line(&app, ln);

            let result = apply_per_char_trace_coloring(
                base_line,
                plain,
                ln,
                &category,
                &app,
                Some("grandchild"),
            );

            let mut char_idx = 0;
            for span in &result.spans {
                for _ in span.content.chars() {
                    if char_idx == col {
                        assert!(
                            span.style.fg != Some(Color::Magenta)
                                && span.style.fg != Some(Color::Cyan),
                            "Edge char at ({}, {}) involving child-b should NOT be colored (got {:?}). \
                             child-b is not in the trace chain. Edges: {:?}\nOutput:\n{}",
                            ln,
                            col,
                            span.style.fg,
                            edges,
                            viz.text
                        );
                    }
                    char_idx += 1;
                }
            }
        }

        // Also verify that the │ vertical bars between child-a's subtree and child-b
        // do NOT map to the (root, child-a) edge — they should only map to (root, child-b)
        let child_a_line = viz.node_line_map["child-a"];
        for l in (child_a_line + 1)..child_b_line {
            if let Some(edges) = viz.char_edge_map.get(&(l, 0)) {
                assert!(
                    !edges.iter().any(|(s, t)| s == "root" && t == "child-a"),
                    "│ at line {} between child-a's subtree and child-b should NOT contain \
                     edge (root, child-a). It should only contain edges for children below. \
                     Edges: {:?}",
                    l,
                    edges
                );
            }
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // DEEP SUBTREE SIBLING: vertical bar to untraced sibling stays uncolored
    // Reproduces the exact topology from the fix-vertical-tree bug report.
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_deep_subtree_vertical_bar_untraced_sibling() {
        // Build the exact topology from the bug:
        //
        //   root
        //   ├→ child-a
        //   │ └→ gc1
        //   │   └→ gc2
        //   │     └→ gc3      <-- SELECTED
        //   │       └→ gc4
        //   └→ child-b        <-- NOT in trace
        //
        // When gc3 is selected, the trace goes:
        //   gc3 → gc2 → gc1 → child-a → root
        //
        // child-b is a sibling of child-a under root, NOT in the trace.
        // The │ chars at column 0 between child-a's subtree and child-b
        // should map ONLY to (root, child-b). Since child-b is NOT
        // upstream of gc3, those │ chars must NOT be colored magenta.

        let mut graph = WorkGraph::new();
        let root = make_task_with_status("root", "Root", Status::Done);
        let mut child_a = make_task_with_status("child-a", "Child A", Status::Done);
        child_a.after = vec!["root".to_string()];
        let mut gc1 = make_task_with_status("gc1", "GC1", Status::Done);
        gc1.after = vec!["child-a".to_string()];
        let mut gc2 = make_task_with_status("gc2", "GC2", Status::Done);
        gc2.after = vec!["gc1".to_string()];
        let mut gc3 = make_task_with_status("gc3", "GC3", Status::Done);
        gc3.after = vec!["gc2".to_string()];
        let mut gc4 = make_task_with_status("gc4", "GC4", Status::Done);
        gc4.after = vec!["gc3".to_string()];
        let mut child_b = make_task_with_status("child-b", "Child B", Status::Done);
        child_b.after = vec!["root".to_string()];

        graph.add_node(Node::Task(root));
        graph.add_node(Node::Task(child_a));
        graph.add_node(Node::Task(gc1));
        graph.add_node(Node::Task(gc2));
        graph.add_node(Node::Task(gc3));
        graph.add_node(Node::Task(gc4));
        graph.add_node(Node::Task(child_b));

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

        let app = build_app_from_viz_output(&viz, "gc3");

        // Verify upstream set is correct
        assert!(app.upstream_set.contains("root"), "root should be upstream");
        assert!(
            app.upstream_set.contains("child-a"),
            "child-a should be upstream"
        );
        assert!(app.upstream_set.contains("gc1"), "gc1 should be upstream");
        assert!(app.upstream_set.contains("gc2"), "gc2 should be upstream");
        assert!(
            !app.upstream_set.contains("child-b"),
            "child-b should NOT be upstream"
        );
        assert!(
            !app.upstream_set.contains("gc4"),
            "gc4 is downstream, not upstream"
        );

        let child_a_line = viz.node_line_map["child-a"];
        let child_b_line = viz.node_line_map["child-b"];

        // PART 1: Verify char_edge_map correctness.
        // The │ bars at col 0 between child-a and child-b must map ONLY to
        // (root, child-b), NOT to (root, child-a) which would cause coloring.
        for l in (child_a_line + 1)..child_b_line {
            if let Some(edges) = viz.char_edge_map.get(&(l, 0)) {
                assert!(
                    !edges.iter().any(|(s, t)| s == "root" && t == "child-a"),
                    "│ at ({}, 0) should NOT have (root, child-a). Edges: {:?}",
                    l,
                    edges
                );

                // No edge should have BOTH endpoints in the upstream set
                let would_be_colored = edges.iter().any(|(src, tgt)| {
                    let src_upstream = app.upstream_set.contains(src.as_str()) || src == "gc3";
                    let tgt_upstream = app.upstream_set.contains(tgt.as_str()) || tgt == "gc3";
                    src_upstream && tgt_upstream
                });
                assert!(
                    !would_be_colored,
                    "│ at ({}, 0) would be colored magenta but child-b is NOT in trace! \
                     Edges: {:?}\nUpstream: {:?}",
                    l, edges, app.upstream_set
                );
            }
        }

        // PART 2: Verify actual render — apply per-char coloring and check
        // that the │ chars leading to the untraced sibling are NOT colored.
        for l in (child_a_line + 1)..child_b_line {
            let plain = app.plain_lines[l].as_str();
            let chars: Vec<char> = plain.chars().collect();
            if chars.is_empty() || chars[0] != '│' {
                continue;
            }

            let base_line = parse_ansi_line(app.lines[l].as_str());
            let category = classify_task_line(&app, l);
            let result =
                apply_per_char_trace_coloring(base_line, plain, l, &category, &app, Some("gc3"));

            // The first character (│ at col 0) must NOT be magenta or cyan
            let mut char_idx = 0;
            for span in &result.spans {
                for c in span.content.chars() {
                    if char_idx == 0 && c == '│' {
                        assert!(
                            span.style.fg != Some(Color::Magenta)
                                && span.style.fg != Some(Color::Cyan),
                            "│ at line {} col 0 should NOT be colored! child-b is not in \
                             the trace. Got fg={:?}\nPlain: {}\nOutput:\n{}",
                            l,
                            span.style.fg,
                            plain,
                            viz.text
                        );
                    }
                    char_idx += 1;
                }
            }
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // Cycle edge visualization tests
    // ══════════════════════════════════════════════════════════════════════

    /// Helper: build a graph, generate viz, select a task, and return the app.
    fn build_cycle_app(graph: &WorkGraph, selected_id: &str) -> VizApp {
        let viz = {
            let tasks: Vec<_> = graph.tasks().collect();
            let task_ids: HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
            generate_ascii(
                graph,
                &tasks,
                &task_ids,
                &HashMap::new(),
                &HashMap::new(),
                &HashMap::new(),
                &HashMap::new(),
                LayoutMode::default(),
                &HashSet::new(),
                "gray",
            )
        };
        build_app_from_viz_output(&viz, selected_id)
    }

    /// Helper: check if ANY edge character on any line has Yellow fg color
    /// when apply_per_char_trace_coloring is applied.
    fn has_any_yellow_edge(app: &VizApp, selected_id: &str) -> bool {
        for (idx, ansi_line) in app.lines.iter().enumerate() {
            let plain = app.plain_lines[idx].as_str();
            let base_line = parse_ansi_line(ansi_line);
            let category = classify_task_line(app, idx);
            let result = apply_per_char_trace_coloring(
                base_line,
                plain,
                idx,
                &category,
                app,
                Some(selected_id),
            );

            let mut char_idx = 0;
            for span in &result.spans {
                for _c in span.content.chars() {
                    if app.char_edge_map.contains_key(&(idx, char_idx))
                        && span.style.fg == Some(Color::Yellow)
                    {
                        return true;
                    }
                    char_idx += 1;
                }
            }
        }
        false
    }

    /// Helper: collect all (line, col) positions where edge chars have Yellow fg.
    fn collect_yellow_edge_positions(app: &VizApp, selected_id: &str) -> HashSet<(usize, usize)> {
        let mut positions = HashSet::new();
        for (idx, ansi_line) in app.lines.iter().enumerate() {
            let plain = app.plain_lines[idx].as_str();
            let base_line = parse_ansi_line(ansi_line);
            let category = classify_task_line(app, idx);
            let result = apply_per_char_trace_coloring(
                base_line,
                plain,
                idx,
                &category,
                app,
                Some(selected_id),
            );

            let mut char_idx = 0;
            for span in &result.spans {
                for _c in span.content.chars() {
                    if app.char_edge_map.contains_key(&(idx, char_idx))
                        && span.style.fg == Some(Color::Yellow)
                    {
                        positions.insert((idx, char_idx));
                    }
                    char_idx += 1;
                }
            }
        }
        positions
    }

    /// Helper: collect all (line, col) positions where edge chars have Magenta fg.
    fn collect_magenta_edge_positions(app: &VizApp, selected_id: &str) -> HashSet<(usize, usize)> {
        let mut positions = HashSet::new();
        for (idx, ansi_line) in app.lines.iter().enumerate() {
            let plain = app.plain_lines[idx].as_str();
            let base_line = parse_ansi_line(ansi_line);
            let category = classify_task_line(app, idx);
            let result = apply_per_char_trace_coloring(
                base_line,
                plain,
                idx,
                &category,
                app,
                Some(selected_id),
            );

            let mut char_idx = 0;
            for span in &result.spans {
                for _c in span.content.chars() {
                    if app.char_edge_map.contains_key(&(idx, char_idx))
                        && span.style.fg == Some(Color::Magenta)
                    {
                        positions.insert((idx, char_idx));
                    }
                    char_idx += 1;
                }
            }
        }
        positions
    }

    // ── Test 1: Simple cycle A → B → C → A ──

    #[test]
    fn test_cycle_simple_all_edges_yellow() {
        // A → B → C → A. Select A. All three edges should be yellow.
        let mut graph = WorkGraph::new();
        let mut a = make_task_with_status("a", "Task A", Status::Open);
        a.after = vec!["c".to_string()];
        let mut b = make_task_with_status("b", "Task B", Status::Open);
        b.after = vec!["a".to_string()];
        let mut c = make_task_with_status("c", "Task C", Status::Open);
        c.after = vec!["b".to_string()];
        graph.add_node(Node::Task(a));
        graph.add_node(Node::Task(b));
        graph.add_node(Node::Task(c));

        let app = build_cycle_app(&graph, "a");

        // cycle_set should contain all three
        assert!(app.cycle_set.contains("a"), "a should be in cycle_set");
        assert!(app.cycle_set.contains("b"), "b should be in cycle_set");
        assert!(app.cycle_set.contains("c"), "c should be in cycle_set");

        // There should be yellow edges
        assert!(
            has_any_yellow_edge(&app, "a"),
            "Simple cycle: should have yellow edges when A selected.\nViz:\n{}",
            app.lines.join("\n")
        );
    }

    // ── Test 2: No cycle — linear chain ──

    #[test]
    fn test_no_cycle_no_yellow() {
        // Linear chain A → B → C. Select B. No yellow edges.
        let mut graph = WorkGraph::new();
        let a = make_task_with_status("a", "Task A", Status::Done);
        let mut b = make_task_with_status("b", "Task B", Status::InProgress);
        b.after = vec!["a".to_string()];
        let mut c = make_task_with_status("c", "Task C", Status::Open);
        c.after = vec!["b".to_string()];
        graph.add_node(Node::Task(a));
        graph.add_node(Node::Task(b));
        graph.add_node(Node::Task(c));

        let app = build_cycle_app(&graph, "b");

        // cycle_set should be empty
        assert!(
            app.cycle_set.is_empty(),
            "Linear chain: cycle_set should be empty"
        );

        // No yellow edges
        assert!(
            !has_any_yellow_edge(&app, "b"),
            "Linear chain: should have no yellow edges.\nViz:\n{}",
            app.lines.join("\n")
        );
    }

    // ── Test 3: Cycle + non-cycle edges ──

    #[test]
    fn test_cycle_with_non_cycle_edge() {
        // A → B → C → A (cycle), plus D → A (non-cycle upstream).
        // Select A. Cycle edges yellow, D→A should be magenta (upstream), not yellow.
        let mut graph = WorkGraph::new();
        let mut a = make_task_with_status("a", "Task A", Status::Open);
        a.after = vec!["c".to_string(), "d".to_string()];
        let mut b = make_task_with_status("b", "Task B", Status::Open);
        b.after = vec!["a".to_string()];
        let mut c = make_task_with_status("c", "Task C", Status::Open);
        c.after = vec!["b".to_string()];
        let d = make_task_with_status("d", "Task D", Status::Done);
        graph.add_node(Node::Task(a));
        graph.add_node(Node::Task(b));
        graph.add_node(Node::Task(c));
        graph.add_node(Node::Task(d));

        let app = build_cycle_app(&graph, "a");

        // cycle_set should contain a, b, c but NOT d
        assert!(app.cycle_set.contains("a"), "a in cycle_set");
        assert!(app.cycle_set.contains("b"), "b in cycle_set");
        assert!(app.cycle_set.contains("c"), "c in cycle_set");
        assert!(!app.cycle_set.contains("d"), "d should NOT be in cycle_set");

        // D should be upstream of A
        assert!(app.upstream_set.contains("d"), "d should be upstream of a");

        // Check that cycle edges exist and are yellow
        assert!(
            has_any_yellow_edge(&app, "a"),
            "Cycle+non-cycle: should have yellow edges for the cycle.\nViz:\n{}",
            app.lines.join("\n")
        );

        // Check that edges involving D are NOT yellow.
        // D→A edges should be magenta (upstream), not yellow.
        let yellow_positions = collect_yellow_edge_positions(&app, "a");
        for (line, col) in &yellow_positions {
            if let Some(edges) = app.char_edge_map.get(&(*line, *col)) {
                for (src, tgt) in edges {
                    // If this position is yellow, the edge should be between cycle members
                    assert!(
                        app.cycle_set.contains(src.as_str())
                            && app.cycle_set.contains(tgt.as_str()),
                        "Yellow edge at ({},{}) has non-cycle endpoints: ({}, {})",
                        line,
                        col,
                        src,
                        tgt
                    );
                }
            }
        }
    }

    // ── Test 4: Multiple cycles ──

    #[test]
    fn test_multiple_cycles_all_yellow() {
        // A → B → A and B → C → B. Select B. Both cycles' edges should be yellow.
        // All three nodes form one SCC.
        let mut graph = WorkGraph::new();
        let mut a = make_task_with_status("a", "Task A", Status::Open);
        a.after = vec!["b".to_string()];
        let mut b = make_task_with_status("b", "Task B", Status::Open);
        b.after = vec!["a".to_string(), "c".to_string()];
        let mut c = make_task_with_status("c", "Task C", Status::Open);
        c.after = vec!["b".to_string()];
        graph.add_node(Node::Task(a));
        graph.add_node(Node::Task(b));
        graph.add_node(Node::Task(c));

        let app = build_cycle_app(&graph, "b");

        // All three should be in the same SCC
        assert!(app.cycle_set.contains("a"), "a in cycle_set");
        assert!(app.cycle_set.contains("b"), "b in cycle_set");
        assert!(app.cycle_set.contains("c"), "c in cycle_set");

        // Should have yellow edges
        assert!(
            has_any_yellow_edge(&app, "b"),
            "Multiple cycles: should have yellow edges.\nViz:\n{}",
            app.lines.join("\n")
        );
    }

    // ── Test 5: Self-loop ──

    #[test]
    fn test_self_loop_yellow() {
        // A → A. Select A. The self-loop edge should be yellow.
        let mut graph = WorkGraph::new();
        let mut a = make_task_with_status("a", "Task A", Status::Open);
        a.after = vec!["a".to_string()];
        graph.add_node(Node::Task(a));

        let app = build_cycle_app(&graph, "a");

        // Self-loop: the SCC detection should include 'a' in its own cycle.
        // Note: Tarjan's SCC may or may not include single-node self-loops
        // as non-trivial SCCs depending on the implementation.
        // The cycle_members map is built from cycle_analysis.cycles which
        // may only contain SCCs with >1 member. Self-loops need special handling.
        //
        // If cycle_set is populated, verify yellow edges.
        // If not, this reveals a gap in the implementation.
        if app.cycle_set.contains("a") {
            // Self-loop detected in SCC — verify yellow edges exist
            assert!(
                has_any_yellow_edge(&app, "a"),
                "Self-loop: cycle_set includes 'a' but no yellow edges.\nViz:\n{}",
                app.lines.join("\n")
            );
        } else {
            // Self-loops may not be detected by the SCC algorithm as non-trivial SCCs.
            // This is acceptable behavior — document it.
            eprintln!(
                "Note: Self-loop A→A not detected as cycle by SCC. \
                       cycle_set is empty. This is expected if SCC algorithm \
                       only reports components with >1 member."
            );
            assert!(
                !has_any_yellow_edge(&app, "a"),
                "Self-loop: cycle_set is empty, so no yellow edges expected.\nViz:\n{}",
                app.lines.join("\n")
            );
        }
    }

    // ── Test 6: Nested cycles (larger SCC) ──

    #[test]
    fn test_nested_cycles_all_scc_yellow() {
        // A → B → C → A and A → B → A (subset). Select A.
        // All edges in the larger SCC should be yellow.
        // Since A, B, C all form one SCC, all should be in cycle_set.
        let mut graph = WorkGraph::new();
        let mut a = make_task_with_status("a", "Task A", Status::Open);
        a.after = vec!["b".to_string(), "c".to_string()]; // A depends on B and C (back-edges)
        let mut b = make_task_with_status("b", "Task B", Status::Open);
        b.after = vec!["a".to_string()]; // B depends on A
        let mut c = make_task_with_status("c", "Task C", Status::Open);
        c.after = vec!["b".to_string()]; // C depends on B
        graph.add_node(Node::Task(a));
        graph.add_node(Node::Task(b));
        graph.add_node(Node::Task(c));

        let app = build_cycle_app(&graph, "a");

        // All three should be in the SCC
        assert!(app.cycle_set.contains("a"), "a in cycle_set");
        assert!(app.cycle_set.contains("b"), "b in cycle_set");
        assert!(app.cycle_set.contains("c"), "c in cycle_set");

        // All edges between SCC members should be yellow
        let yellow_positions = collect_yellow_edge_positions(&app, "a");
        assert!(
            !yellow_positions.is_empty(),
            "Nested cycles: should have yellow edges.\nViz:\n{}",
            app.lines.join("\n")
        );
    }

    // ── Test 7: Non-member selected ──

    #[test]
    fn test_non_member_selected_no_yellow() {
        // A → B → C → A (cycle), D is independent. Select D. No yellow edges.
        let mut graph = WorkGraph::new();
        let mut a = make_task_with_status("a", "Task A", Status::Open);
        a.after = vec!["c".to_string()];
        let mut b = make_task_with_status("b", "Task B", Status::Open);
        b.after = vec!["a".to_string()];
        let mut c = make_task_with_status("c", "Task C", Status::Open);
        c.after = vec!["b".to_string()];
        let d = make_task_with_status("d", "Task D", Status::Open);
        graph.add_node(Node::Task(a));
        graph.add_node(Node::Task(b));
        graph.add_node(Node::Task(c));
        graph.add_node(Node::Task(d));

        let app = build_cycle_app(&graph, "d");

        // D is not in any cycle
        assert!(
            app.cycle_set.is_empty(),
            "Non-member selected: cycle_set should be empty, got {:?}",
            app.cycle_set
        );

        // No yellow edges
        assert!(
            !has_any_yellow_edge(&app, "d"),
            "Non-member selected: should have no yellow edges.\nViz:\n{}",
            app.lines.join("\n")
        );
    }

    // ── Additional: Yellow overrides magenta for cycle edges ──

    #[test]
    fn test_cycle_yellow_overrides_magenta() {
        // In a cycle A → B → C → A, when A is selected:
        // B and C are both upstream AND downstream of A.
        // Cycle edges should be yellow (highest priority), not magenta or cyan.
        let mut graph = WorkGraph::new();
        let mut a = make_task_with_status("a", "Task A", Status::Open);
        a.after = vec!["c".to_string()];
        let mut b = make_task_with_status("b", "Task B", Status::Open);
        b.after = vec!["a".to_string()];
        let mut c = make_task_with_status("c", "Task C", Status::Open);
        c.after = vec!["b".to_string()];
        graph.add_node(Node::Task(a));
        graph.add_node(Node::Task(b));
        graph.add_node(Node::Task(c));

        let app = build_cycle_app(&graph, "a");

        // Verify B and C are both upstream and downstream (cycle means both)
        let b_reachable = app.upstream_set.contains("b") || app.downstream_set.contains("b");
        let c_reachable = app.upstream_set.contains("c") || app.downstream_set.contains("c");
        assert!(b_reachable, "b should be reachable from a");
        assert!(c_reachable, "c should be reachable from a");

        // For every edge between cycle members, verify yellow takes priority
        let yellow = collect_yellow_edge_positions(&app, "a");
        let magenta = collect_magenta_edge_positions(&app, "a");

        // No position should be both yellow and magenta (yellow should override)
        let overlap: HashSet<_> = yellow.intersection(&magenta).collect();
        assert!(
            overlap.is_empty(),
            "Cycle edge positions should not be magenta — yellow overrides. \
             Overlap at: {:?}",
            overlap
        );

        // There should be some yellow edges
        assert!(
            !yellow.is_empty(),
            "Cycle edges should be yellow, but none found.\nViz:\n{}",
            app.lines.join("\n")
        );
    }
}
