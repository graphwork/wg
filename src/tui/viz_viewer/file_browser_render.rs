use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use tui_tree_widget::Tree;

use super::file_browser::{FileBrowser, FileBrowserFocus};
use super::state::VizApp;

/// Draw the Files tab: two-pane file browser (tree + preview).
pub fn draw_files_tab(frame: &mut Frame, app: &mut VizApp, area: Rect) {
    if app.file_browser.is_none() {
        let msg = Paragraph::new("Initializing file browser...")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(msg, area);
        return;
    }

    if area.height < 3 || area.width < 10 {
        return;
    }

    // Split into main content area + status bar at bottom
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let content_area = outer[0];
    let status_area = outer[1];

    // Split content horizontally: 35% tree, 65% preview
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(content_area);

    let tree_area = panes[0];
    let preview_area = panes[1];

    // Store areas for mouse hit-testing.
    app.last_file_tree_area = tree_area;
    app.last_file_preview_area = preview_area;

    let fb = app.file_browser.as_mut().unwrap();

    // Load preview if needed
    fb.load_preview();

    draw_tree_pane(frame, fb, tree_area);
    draw_preview_pane(frame, fb, preview_area);
    draw_status_bar(frame, fb, status_area);
}

/// Draw the directory tree pane (left side).
fn draw_tree_pane(frame: &mut Frame, fb: &mut FileBrowser, area: Rect) {
    let is_focused = fb.focus == FileBrowserFocus::Tree;
    let border_color = if is_focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    // If searching, reserve a row at the bottom for the search input
    let (tree_area, search_area) = if fb.searching {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(area);
        (split[0], Some(split[1]))
    } else {
        (area, None)
    };

    let block = Block::default()
        .title(" .wg/ ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let tree_widget = Tree::new(&fb.tree_items)
        .unwrap_or_else(|_| {
            // Fallback: empty tree if identifiers collide somehow
            Tree::new(&[]).unwrap()
        })
        .block(block)
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(if is_focused {
                    Color::Cyan
                } else {
                    Color::DarkGray
                })
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ")
        .node_closed_symbol("▶ ")
        .node_open_symbol("▼ ")
        .node_no_children_symbol("  ");

    frame.render_stateful_widget(tree_widget, tree_area, &mut fb.tree_state);

    // Draw search input bar if active
    if let Some(search_rect) = search_area {
        let input_text = format!("/{}", fb.search_query);
        let search_bar = Paragraph::new(input_text).style(
            Style::default()
                .fg(Color::Yellow)
                .bg(Color::Black)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_widget(search_bar, search_rect);
    }
}

/// Draw the file preview pane (right side).
fn draw_preview_pane(frame: &mut Frame, fb: &FileBrowser, area: Rect) {
    let is_focused = fb.focus == FileBrowserFocus::Preview;
    let border_color = if is_focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    let title = match &fb.preview_cache {
        Some(cache) => {
            let name = cache
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            format!(" {} ", name)
        }
        None => " Preview ".to_string(),
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    match &fb.preview_cache {
        Some(cache) => {
            let visible_lines: Vec<Line> = cache
                .lines
                .iter()
                .skip(fb.preview_scroll)
                .cloned()
                .collect();
            let text = Text::from(visible_lines);
            let paragraph = Paragraph::new(text).block(block).wrap(Wrap { trim: false });
            frame.render_widget(paragraph, area);
        }
        None => {
            // Show helpful message when nothing is selected or a directory is selected
            let msg = if fb.tree_state.selected().is_empty() {
                "Select a file to preview"
            } else {
                "Select a file to preview\n\nUse j/k to navigate\nEnter/l to expand directories"
            };
            let paragraph = Paragraph::new(msg)
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(paragraph, area);
        }
    }
}

/// Draw the status bar at the bottom of the Files tab.
fn draw_status_bar(frame: &mut Frame, fb: &FileBrowser, area: Rect) {
    let content = match &fb.preview_cache {
        Some(cache) => {
            let path_str = cache
                .path
                .strip_prefix(&fb.root)
                .unwrap_or(&cache.path)
                .display();
            if cache.is_binary {
                format!(
                    " {} │ Binary │ {} │ READ-ONLY",
                    path_str,
                    format_size(cache.file_size),
                )
            } else {
                let trunc = if cache.truncated { " [truncated]" } else { "" };
                format!(
                    " {} │ {} lines{} │ {} │ READ-ONLY",
                    path_str,
                    cache.line_count,
                    trunc,
                    format_size(cache.file_size),
                )
            }
        }
        None => {
            let selected = fb.selected_path();
            match selected {
                Some(p) if p.is_dir() => {
                    let rel = p.strip_prefix(&fb.root).unwrap_or(&p);
                    format!(" {}/ │ Directory │ READ-ONLY", rel.display())
                }
                _ => " .wg/ │ READ-ONLY".to_string(),
            }
        }
    };

    let bar = Paragraph::new(content).style(
        Style::default()
            .fg(Color::White)
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    frame.render_widget(bar, area);
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
