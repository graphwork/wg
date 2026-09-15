//! Focused entry point for the explicit-execution-selection terminal scenario.
//!
//! The scenario owns daemon processes, so it must run through
//! `worksgood::smoke`'s Linux subreaper rather than as an ad-hoc shell script.

use std::path::{Path, PathBuf};
use worksgood::smoke::{Manifest, run_scenarios};

#[test]
fn explicit_execution_selection_terminal_scenario() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest_path = root.join("tests/smoke/manifest.toml");
    let manifest = Manifest::load_from(&manifest_path).expect("load smoke manifest");
    let scenario = manifest
        .scenarios
        .iter()
        .find(|scenario| scenario.name == "explicit_execution_selection")
        .expect("explicit execution scenario remains in the grow-only manifest");

    let candidate_dir = Path::new(env!("CARGO_BIN_EXE_wg"))
        .parent()
        .expect("cargo-built candidate has a parent directory");
    let prior_path = std::env::var_os("PATH");
    let mut paths = vec![candidate_dir.to_path_buf()];
    if let Some(prior) = prior_path.as_ref() {
        paths.extend(std::env::split_paths(prior));
    }
    let candidate_path = std::env::join_paths(paths).expect("compose candidate PATH");

    // SAFETY: this integration test has one test case and no sibling threads.
    unsafe { std::env::set_var("PATH", candidate_path) };
    let report = run_scenarios(&[scenario], manifest_path.parent().unwrap());
    match prior_path {
        Some(value) => unsafe { std::env::set_var("PATH", value) },
        None => unsafe { std::env::remove_var("PATH") },
    }

    assert!(!report.blocks_done(), "{}", report.render());
}
