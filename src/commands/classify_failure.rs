//! Internal `wg classify-failure` subcommand.
//!
//! Reads the raw_stream.jsonl produced by the wrapper and prints the
//! FailureClass kebab string to stdout. Called by the wrapper script
//! before invoking `wg fail --class <CLASS>`. Hidden from user-facing help.

use anyhow::Result;
use std::path::Path;

use super::spawn::raw_stream_classifier::{
    classify_from_raw_stream, classify_no_operational_output,
};

pub fn run(raw_stream: Option<&str>, exit_code: i32) -> Result<()> {
    let class = match raw_stream {
        Some(path) => classify_from_raw_stream(Path::new(path), exit_code),
        None => {
            use worksgood::graph::FailureClass;
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

/// Classify a NoOperationalOutput (guardrail G4) run from the observable
/// signals gathered by the wrapper. Reads the agent's output.log to derive
/// `output_log_nonempty` AND scans it for filesystem-mutation tokens (the
/// `output_log_has_mutations` signal) which is OR'd into the wrapper-supplied
/// `has_file_writes`. Prints `no-operational-output` when the signature
/// matches, or `none` otherwise.
pub fn run_no_op(
    output_log: &str,
    clean_exit: bool,
    artifacts_empty: bool,
    has_file_writes: bool,
) -> Result<()> {
    use super::spawn::raw_stream_classifier::output_log_has_mutations;
    let content = std::fs::read_to_string(output_log).unwrap_or_default();
    let output_log_nonempty = !content.trim().is_empty();
    // Either the wrapper's git-status signal OR an output.log mutation token
    // counts as "the agent acted" — both satisfy G4's has_file_writes.
    let effective_has_file_writes = has_file_writes || output_log_has_mutations(&content);
    let class = classify_no_operational_output(
        clean_exit,
        artifacts_empty,
        effective_has_file_writes,
        output_log_nonempty,
    );
    match class {
        Some(c) => println!("{}", c),
        None => println!("none"),
    }
    Ok(())
}
