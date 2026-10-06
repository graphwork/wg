//! Canonical WG task-status palette — the **single source of truth** for how a
//! task status is coloured across every WG surface.
//!
//! The graph view's palette is the explicit-RGB table in
//! `tui::viz_viewer::state::flash_color_for_status` (the same RGB set
//! `src/html.rs::status_color` follows, and the one the TUI flashes on a status
//! transition). That function now delegates here, so the palette exists in
//! exactly one place.
//!
//! Two consumers export this table verbatim so no client can drift:
//!   * `wg viz --json` → a `"palette"` object (`status -> [r,g,b]`);
//!   * the daemon's `GetFleet` tree payload → the same `"palette"` object.
//!
//! The Pi plugin (`worksgood-pi`) renders *these exact RGB values* on its
//! default path (see `worksgood-pi/src/status-palette.ts`). A checked-in
//! cross-language fixture (`worksgood-pi/test/fixtures/status-palette.json`) is
//! asserted against this table by a Rust test and against the plugin by a
//! TypeScript test, so a palette change must be applied in both languages or a
//! test fails loudly.
//!
//! ## Canonical vs. HTML — an intentional, documented divergence
//!
//! This is the palette for the **graph view** (`wg viz`, `GetFleet`, the Pi
//! fleet/graph panel): `InProgress` is the cyan flash `(60, 200, 220)`.
//! `src/html.rs::status_color` is a *different, HTML-only* surface that renders
//! the task-list badge colour `Color::Yellow` → `rgb(229,229,16)` for
//! in-progress tasks instead. That divergence is pre-existing and pinned by the
//! HTML/TUI-parity tests; the graph view intentionally uses the state.rs table
//! as instructed. Do not "fix" one to match the other without an explicit
//! decision.

use crate::graph::Status;

/// RGB for a status. The canonical graph-view palette.
pub fn status_rgb(status: &Status) -> (u8, u8, u8) {
    match status {
        Status::Done => (80, 220, 100),       // green
        Status::Failed => (220, 60, 60),      // red
        Status::InProgress => (60, 200, 220), // cyan
        Status::Open => (200, 200, 80),       // yellow
        Status::Blocked => (180, 120, 60),    // orange
        Status::Abandoned => (140, 100, 160), // muted purple
        Status::Waiting | Status::PendingValidation => (60, 160, 220), // blue
        Status::PendingEval => (140, 230, 80), // chartreuse
        Status::FailedPendingEval => (210, 130, 70), // warm coral
        Status::Incomplete => (255, 165, 0),  // orange
    }
}

/// RGB for a status **name** — the `Display` / viz vocabulary (`"done"`,
/// `"in-progress"`, `"pending-validation"`, …). Returns `None` for an unknown
/// name so callers can choose their own fallback.
pub fn status_rgb_by_name(name: &str) -> Option<(u8, u8, u8)> {
    let status = match name {
        "open" => Status::Open,
        "in-progress" => Status::InProgress,
        "waiting" => Status::Waiting,
        "done" => Status::Done,
        "blocked" => Status::Blocked,
        "failed" => Status::Failed,
        "abandoned" => Status::Abandoned,
        "pending-validation" => Status::PendingValidation,
        "pending-eval" => Status::PendingEval,
        "failed-pending-eval" => Status::FailedPendingEval,
        "incomplete" => Status::Incomplete,
        _ => return None,
    };
    Some(status_rgb(&status))
}

/// Every status name in the palette, in a stable order, paired with its RGB.
///
/// `Waiting` and `PendingValidation` are distinct graph statuses that share one
/// colour, so both names appear. This is the set the cross-language fixture
/// pins.
pub fn status_palette() -> Vec<(&'static str, (u8, u8, u8))> {
    vec![
        ("open", status_rgb(&Status::Open)),
        ("in-progress", status_rgb(&Status::InProgress)),
        ("waiting", status_rgb(&Status::Waiting)),
        ("done", status_rgb(&Status::Done)),
        ("blocked", status_rgb(&Status::Blocked)),
        ("failed", status_rgb(&Status::Failed)),
        ("abandoned", status_rgb(&Status::Abandoned)),
        ("pending-validation", status_rgb(&Status::PendingValidation)),
        ("pending-eval", status_rgb(&Status::PendingEval)),
        (
            "failed-pending-eval",
            status_rgb(&Status::FailedPendingEval),
        ),
        ("incomplete", status_rgb(&Status::Incomplete)),
    ]
}

/// The palette as a JSON object (`status -> [r, g, b]`), for `wg viz --json`
/// and the `GetFleet` tree payload.
pub fn palette_json() -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (name, (r, g, b)) in status_palette() {
        map.insert(name.to_string(), serde_json::json!([r, g, b]));
    }
    serde_json::Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_progress_is_the_cyan_graph_palette_not_html_yellow() {
        // Pin the documented divergence: the graph view uses the state.rs cyan
        // flash, NOT html.rs's Color::Yellow badge colour.
        assert_eq!(status_rgb(&Status::InProgress), (60, 200, 220));
    }

    #[test]
    fn status_palette_covers_every_display_name() {
        let names: Vec<&str> = status_palette().iter().map(|(n, _)| *n).collect();
        for status in [
            Status::Open,
            Status::InProgress,
            Status::Waiting,
            Status::Done,
            Status::Blocked,
            Status::Failed,
            Status::Abandoned,
            Status::PendingValidation,
            Status::PendingEval,
            Status::FailedPendingEval,
            Status::Incomplete,
        ] {
            assert!(
                names.contains(&status.to_string().as_str()),
                "palette missing status name {}",
                status
            );
        }
    }

    #[test]
    fn palette_json_is_name_keyed_rgb() {
        let json = palette_json();
        assert_eq!(json["done"], serde_json::json!([80, 220, 100]));
        assert_eq!(json["in-progress"], serde_json::json!([60, 200, 220]));
        assert_eq!(
            json["pending-validation"],
            serde_json::json!([60, 160, 220])
        );
        assert_eq!(json["incomplete"], serde_json::json!([255, 165, 0]));
    }

    /// Cross-language parity pin: the checked-in fixture the TypeScript plugin
    /// test reads MUST equal this table. A palette change in Rust that is not
    /// mirrored in the fixture (and therefore in the plugin tests) fails here.
    #[test]
    fn checked_in_fixture_matches_exported_palette() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("worksgood-pi/test/fixtures/status-palette.json");
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let fixture: serde_json::Value = serde_json::from_str(&raw).expect("fixture is valid JSON");
        assert_eq!(
            fixture,
            palette_json(),
            "worksgood-pi/test/fixtures/status-palette.json drifted from status_palette()"
        );
    }
}
