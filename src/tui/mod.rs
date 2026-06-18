// `markdown` now lives in the top-level library crate so non-TUI
// callers (like `wg nex`'s terminal surface) can share the parser.
// Re-export here so existing `crate::tui::markdown::...` call
// sites keep working.
pub use worksgood::markdown;
pub use worksgood::syntect_convert;
pub mod pty_pane;
pub mod viz_viewer;
