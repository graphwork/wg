//! `wg fed-node` — run/inspect the WG-Fed node store-and-forward inbox.
//!
//! This is the **default transport rung** of ADR-fed-002 (§D1 rung 1): the promoted
//! daemon (doc 02 §2.1) exposed as an HTTP store-and-forward inbox so signed
//! `SignedEvent`s addressed by `wgid:` move **between graphs on different hosts**,
//! holding messages for **offline** recipients until they poll. The node is untrusted
//! (every byte is self-verifying); it is a convenience, never a mandatory root (§D2).

use std::path::Path;

use anyhow::Result;

use worksgood::identity::node;

/// `wg fed-node serve --addr <host:port> [--store <dir>]` — run the inbox server
/// (blocking). With `--store` omitted, defaults to `<workgraph_dir>/fed-node`.
pub fn run_serve(workgraph_dir: &Path, addr: &str, store: Option<&str>) -> Result<()> {
    let default = node::default_store_dir(workgraph_dir);
    let store_dir = store
        .map(|s| s.to_string())
        .unwrap_or_else(|| default.to_string_lossy().to_string());
    node::serve(addr, &store_dir)
}

/// `wg fed-node store-path` — print the default node store dir (scriptable).
pub fn run_store_path(workgraph_dir: &Path) -> Result<()> {
    println!("{}", node::default_store_dir(workgraph_dir).display());
    Ok(())
}
