//! Thin command adapter for immutable WorkSave capture.
//!
//! Lifecycle/terminal adapters construct the authenticated request. This module
//! only selects the graph-owned evidence store and renders the receipt; it does
//! not project task status, promote a ref, or remove a worktree.

use std::path::Path;

use anyhow::Result;
use worksgood::work_save::{
    CapturedWorkSave, WorkSaveCaptureRequest, WorkSaveStore, capture_work_save,
};

/// Capture and durably publish one exact-attempt WorkSave.
///
/// `wg_dir` is the authoritative graph directory (normally `.wg`). Replaying
/// the same request after a crash is content/ref idempotent.
pub fn capture(wg_dir: &Path, request: &WorkSaveCaptureRequest) -> Result<CapturedWorkSave> {
    let store = WorkSaveStore::open(wg_dir)?;
    capture_work_save(&store, request)
}

/// Capture and print the immutable receipt for an internal command caller.
pub fn run(
    wg_dir: &Path,
    request: &WorkSaveCaptureRequest,
    json: bool,
) -> Result<CapturedWorkSave> {
    let captured = capture(wg_dir, request)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&captured)?);
    } else {
        println!("WorkSave captured: {}", captured.receipt_cid);
        println!("  ref: {}", captured.receipt.immutable_ref);
        println!("  tree: {}", captured.receipt.saved_tree_oid);
        println!("  clean: {}", captured.receipt.clean);
    }
    Ok(captured)
}
