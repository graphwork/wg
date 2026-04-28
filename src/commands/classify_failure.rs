//! Internal `wg classify-failure` subcommand.
//!
//! Reads the raw_stream.jsonl produced by the wrapper and prints the
//! FailureClass kebab string to stdout. Called by the wrapper script
//! before invoking `wg fail --class <CLASS>`. Hidden from user-facing help.

use anyhow::Result;
use std::path::Path;

use super::spawn::raw_stream_classifier::classify_from_raw_stream;

pub fn run(raw_stream: Option<&str>, exit_code: i32) -> Result<()> {
    let class = match raw_stream {
        Some(path) => classify_from_raw_stream(Path::new(path), exit_code),
        None => {
            use workgraph::graph::FailureClass;
            if exit_code == 124 {
                FailureClass::AgentHardTimeout
            } else {
                FailureClass::AgentExitNonzero
            }
        }
    };
    println!("{}", class);
    Ok(())
}
