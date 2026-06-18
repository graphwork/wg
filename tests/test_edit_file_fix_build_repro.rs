//! Repro for the `add-fuzzy-robust` task: a "fix build errors" flow makes
//! TARGETED edits instead of repeated full-file rewrites.
//!
//! ## Background
//!
//! Watching nex (qwen3) build a Game-of-Life TUI, after each `cargo build`
//! error the model REWROTE THE WHOLE FILE with `write_file` (main.rs 2.3KB →
//! 17KB → 19.5KB → …) instead of patching. Root cause: the strict exact-match
//! `edit_file` failed whenever the model's `old_string` was slightly off
//! (indentation it eyeballed wrong, a stray trailing space, a CRLF/LF
//! difference), so the model fell back to `write_file`.
//!
//! This test reproduces that exact situation: it feeds `edit_file` the kind of
//! *imperfect* `old_string`s a model produces after reading a file, fixing a
//! sequence of build errors. With the fuzzy matcher each targeted edit now
//! lands — no full rewrite needed. Under the old strict matcher every one of
//! these would have returned "old_string not found".

use std::fs;
use tempfile::{Builder, TempDir};

use worksgood::executor::native::tools::{ToolOutput, ToolRegistry};

fn make_tool_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    worksgood::executor::native::tools::file::register_file_tools(&mut registry);
    registry
}

fn temp_dir() -> TempDir {
    Builder::new()
        .prefix("edit-file-repro-")
        .tempdir_in(std::env::current_dir().unwrap())
        .unwrap()
}

async fn edit_file(registry: &ToolRegistry, path: &str, old: &str, new: &str) -> ToolOutput {
    let input = serde_json::json!({ "path": path, "old_string": old, "new_string": new });
    registry.execute("edit_file", &input).await
}

/// A small program with three "compile errors" the agent will fix one by one,
/// each via a targeted edit whose `old_string` is imperfect in a different way.
const BUGGY_SRC: &str = "fn main() {\n\
    \x20   let mut grid = vec![vec![false; W]; H];\n\
    \x20   let count = neighbours(&grid, 0, 0)\n\
    \x20   println!(\"{}\", count);\n\
}\n";

#[tokio::test]
async fn fix_build_errors_uses_targeted_edits_not_rewrites() {
    let dir = temp_dir();
    let file_path = dir.path().join("main.rs");
    fs::write(&file_path, BUGGY_SRC).unwrap();
    let path = file_path.to_str().unwrap();
    let registry = make_tool_registry();

    // ── Error 1: undefined `W`/`H`. The model copies the line but over-indents
    //    it (6 spaces instead of the file's 4). Strict matching would fail here;
    //    fuzzy indentation tolerance lands it AND re-anchors the replacement to
    //    the file's real 4-space indentation.
    let r1 = edit_file(
        &registry,
        path,
        "      let mut grid = vec![vec![false; W]; H];",
        "      let mut grid = vec![vec![false; 64]; 32];",
    )
    .await;
    assert!(
        !r1.is_error,
        "edit 1 (indentation off) should land: {:?}",
        r1
    );

    // ── Error 2: missing semicolon. The model reproduces the line but adds a
    //    stray trailing space. Trailing-whitespace tolerance lands it.
    let r2 = edit_file(
        &registry,
        path,
        "    let count = neighbours(&grid, 0, 0) ",
        "    let count = neighbours(&grid, 0, 0);",
    )
    .await;
    assert!(
        !r2.is_error,
        "edit 2 (trailing space) should land: {:?}",
        r2
    );

    // ── Error 3: tweak the println. The model uses \r\n line endings (pasted
    //    from a Windows terminal) but the file is LF. Line-ending tolerance
    //    lands it.
    let r3 = edit_file(
        &registry,
        path,
        "    println!(\"{}\", count);\r\n}",
        "    println!(\"count = {}\", count);\n}",
    )
    .await;
    assert!(!r3.is_error, "edit 3 (CRLF vs LF) should land: {:?}", r3);

    // Every fix was a targeted edit; the file now reflects all three.
    let final_src = fs::read_to_string(&file_path).unwrap();
    assert!(
        final_src.contains("vec![vec![false; 64]; 32]"),
        "{final_src}"
    );
    assert!(
        final_src.contains("neighbours(&grid, 0, 0);"),
        "{final_src}"
    );
    assert!(
        final_src.contains("println!(\"count = {}\", count);"),
        "{final_src}"
    );
    // Indentation stayed consistent with the file (4 spaces), not the 2 the
    // model supplied — the replacement was re-anchored.
    assert!(
        final_src.contains("\n    let mut grid = vec![vec![false; 64]; 32];"),
        "replacement should be re-indented to the file: {final_src}"
    );
}

/// When a targeted edit genuinely cannot match, the tool returns a near-miss
/// diagnostic pointing at the closest line — NOT a bare "not found" that pushes
/// the model toward a full rewrite.
#[tokio::test]
async fn unmatchable_edit_returns_near_miss_not_bare_failure() {
    let dir = temp_dir();
    let file_path = dir.path().join("lib.rs");
    fs::write(
        &file_path,
        "pub fn total(items: &[u32]) -> u32 {\n    items.iter().sum()\n}\n",
    )
    .unwrap();
    let path = file_path.to_str().unwrap();
    let registry = make_tool_registry();

    // Model misremembers the signature (`totals`, `i32`).
    let r = edit_file(
        &registry,
        path,
        "pub fn totals(items: &[i32]) -> i32 {",
        "pub fn total(items: &[u32]) -> u64 {",
    )
    .await;

    assert!(r.is_error, "genuinely-wrong old_string should not match");
    let msg = r.content;
    assert!(
        msg.contains("Closest candidate"),
        "should show candidate: {msg}"
    );
    assert!(
        msg.contains("pub fn total(items"),
        "should echo the real line: {msg}"
    );
    // Steers the model back to a targeted retry, away from write_file rewrites.
    assert!(
        msg.contains("write_file"),
        "should discourage full rewrite: {msg}"
    );
}
