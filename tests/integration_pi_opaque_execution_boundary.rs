//! Focused entry point for the opaque-assignment controlled scenario.
//!
//! The scenario owns daemons and fake Pi processes, so it must run through
//! `worksgood::smoke`'s Linux subreaper rather than as an ad-hoc shell script.

use std::path::PathBuf;
use worksgood::smoke::{Manifest, run_scenarios};

#[test]
fn opaque_assignment_controlled_process_scenario() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest_path = root.join("tests/smoke/manifest.toml");
    let manifest = Manifest::load_from(&manifest_path).expect("load smoke manifest");
    let scenario = manifest
        .scenarios
        .iter()
        .find(|scenario| scenario.name == "pi_opaque_execution_assignment")
        .expect("opaque assignment scenario remains in the grow-only manifest");

    let prior = std::env::var_os("WG_BIN");
    // SAFETY: this integration test has one test case and no sibling threads.
    // The child smoke process receives the exact cargo-built candidate binary.
    unsafe { std::env::set_var("WG_BIN", env!("CARGO_BIN_EXE_wg")) };
    let report = run_scenarios(&[scenario], manifest_path.parent().unwrap());
    match prior {
        Some(value) => unsafe { std::env::set_var("WG_BIN", value) },
        None => unsafe { std::env::remove_var("WG_BIN") },
    }

    assert!(!report.blocks_done(), "{}", report.render());
}
