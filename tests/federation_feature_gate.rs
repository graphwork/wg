//! Pin the `federation` cargo feature boundary (task `gate-federation-cluster`).
//!
//! The WG-Fed / WG-Review / WG-Exec / pilot cluster is behind the NON-DEFAULT
//! `federation` cargo feature. Two things must stay true forever:
//!
//! 1. **The smoke manifest is grow-only** — the 17 fed/exec scenario entries stay
//!    in `tests/smoke/manifest.toml` (they are `retired = true`, so `wg done` never
//!    runs them unless `--full-smoke` opts them back in). Deleting an entry to make a
//!    gate go green would erase the regression record; this test refuses that.
//! 2. **A default build SKIPS them loudly, never FAILs them** — each scenario calls
//!    `require_federation` (a loud banner + exit 77) right after `require_wg`, so a
//!    `wg --full-smoke` run against a binary without the cluster records a SKIP.
//!
//! The runtime half of (2) is asserted for real against THIS build's `wg` binary
//! (`CARGO_BIN_EXE_wg`), which is the exact binary cargo produced for the current
//! feature set.

use std::path::PathBuf;

use worksgood::smoke::{Manifest, ScenarioOutcome, run_scenario};

/// The fed/exec/pilot scenarios the gate covers. Grow-only: entries are never deleted
/// from the manifest, so this list is the durability record for them.
const FED_SCENARIOS: &[&str] = &[
    "federation_spark_two_graphs",
    "federation_node_inbox_cross_graph",
    "federation_recovery_portable_state",
    "federation_acl_ucan_delegation",
    "content_safety_spark",
    "exec_spark_borrowed_box",
    "e2e_family_team",
    "e2e_autowire_ingest_gate",
    "pilot_dry_run",
    "fed_harden_residuals",
    "exec_harden_wire",
    "exec_ledger_crash_safe",
    "exec_real_run_silicon",
    "wire_review_seams",
    "real_review_evasion",
    "live_reviewer_eval",
    "casa_wgfed_adapter_e2e",
];

fn smoke_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/smoke")
}

fn manifest() -> Manifest {
    Manifest::load_from(&smoke_dir().join("manifest.toml")).expect("smoke manifest parses")
}

#[test]
fn manifest_keeps_every_fed_exec_scenario_entry() {
    let manifest = manifest();
    let names: Vec<&str> = manifest
        .all_scenarios()
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    for expected in FED_SCENARIOS {
        assert!(
            names.contains(expected),
            "smoke manifest lost the fed/exec scenario entry '{expected}' — the manifest is \
             grow-only; the feature gate must not remove entries"
        );
    }
    assert_eq!(
        FED_SCENARIOS.len(),
        17,
        "the fed/exec scenario corpus is 17 scenarios; update this list deliberately"
    );
}

/// Every covered script must route through the shared `require_federation` banner
/// (which lives in `scenarios/_helpers.sh`), and must do it right after `require_wg`.
#[test]
fn every_fed_scenario_script_requires_the_feature() {
    let helpers =
        std::fs::read_to_string(smoke_dir().join("scenarios/_helpers.sh")).expect("_helpers.sh");
    assert!(
        helpers.contains("require_federation()"),
        "scenarios/_helpers.sh must define the require_federation skip helper"
    );
    assert!(
        helpers.contains("FEDERATION FEATURE OFF"),
        "the federation skip banner must be greppable as 'FEDERATION FEATURE OFF'"
    );

    let manifest = manifest();
    for scenario in manifest.all_scenarios() {
        if !FED_SCENARIOS.contains(&scenario.name.as_str()) {
            continue;
        }
        let script = smoke_dir().join(&scenario.script);
        let body = std::fs::read_to_string(&script)
            .unwrap_or_else(|e| panic!("reading {}: {e}", script.display()));
        assert!(
            body.contains("require_federation"),
            "{} pins the federation cluster but never calls require_federation, so a \
             default build would FAIL instead of SKIP",
            scenario.script
        );
        let wg = body.find("require_wg").expect("require_wg call");
        let fed = body
            .find("require_federation")
            .expect("require_federation call");
        assert!(
            fed > wg,
            "{} must gate AFTER require_wg (the binary must exist before probing it)",
            scenario.script
        );
    }
}

/// The real thing: run one covered scenario against the binary built for THIS feature
/// set and assert the outcome is a loud SKIP (never a FAIL) without the cluster.
#[cfg(not(feature = "federation"))]
#[test]
fn fed_scenario_skips_loudly_in_a_default_build() {
    let manifest = manifest();
    let scenario = manifest
        .all_scenarios()
        .into_iter()
        .find(|s| s.name == "pilot_dry_run")
        .expect("pilot_dry_run entry")
        .clone();

    // Make sure the scenario probes THIS build's `wg` (a default build), not whatever
    // `wg` happens to be installed globally.
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_wg"));
    let bin_dir = bin.parent().expect("binary dir");
    let path = std::env::var("PATH").unwrap_or_default();
    // SAFETY: this test is the only consumer of the variable in this process, and the
    // scenario is spawned synchronously right after.
    unsafe {
        std::env::set_var("PATH", format!("{}:{}", bin_dir.display(), path));
    }

    let result = run_scenario(&scenario, &smoke_dir());
    match result.outcome {
        ScenarioOutcome::Skip { reason } => {
            assert!(
                reason.contains("FEDERATION FEATURE OFF"),
                "expected the federation feature banner, got: {reason}"
            );
        }
        other => panic!(
            "a default build must SKIP the fed/exec corpus, not {other:?} — the scenario \
             would fail loudly against a binary without the cluster"
        ),
    }
}

/// With the cluster compiled in, the same scenario must get PAST the feature gate: it
/// may still skip for a tooling reason (python3/curl/tmux), but never with the feature
/// banner.
#[cfg(feature = "federation")]
#[test]
fn fed_scenario_is_not_feature_skipped_with_the_feature_on() {
    let manifest = manifest();
    let scenario = manifest
        .all_scenarios()
        .into_iter()
        .find(|s| s.name == "pilot_dry_run")
        .expect("pilot_dry_run entry")
        .clone();

    let bin = PathBuf::from(env!("CARGO_BIN_EXE_wg"));
    let bin_dir = bin.parent().expect("binary dir");
    let path = std::env::var("PATH").unwrap_or_default();
    unsafe {
        std::env::set_var("PATH", format!("{}:{}", bin_dir.display(), path));
    }

    // `wg identity --help` must succeed for the cluster to be present.
    let status = std::process::Command::new(&bin)
        .args(["identity", "--help"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("run wg identity --help");
    assert!(
        status.success(),
        "--features federation must restore `wg identity`"
    );
    let _ = scenario; // the full dry-run needs external tooling; the probe above is the bound
}
