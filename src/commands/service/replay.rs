//! Offline daemon-planner replay command.

use anyhow::{Context, Result};
use std::path::Path;

use worksgood::service::{DecisionTrace, replay_bytes};

pub fn run(trace_path: &Path, output: Option<&Path>, json: bool) -> Result<()> {
    let trace: DecisionTrace = serde_json::from_slice(
        &std::fs::read(trace_path)
            .with_context(|| format!("failed to read replay trace {}", trace_path.display()))?,
    )
    .with_context(|| format!("failed to parse replay trace {}", trace_path.display()))?;
    let bytes = replay_bytes(&trace)?;
    if let Some(path) = output {
        worksgood::atomic_file::write_atomic(path, &bytes)
            .with_context(|| format!("failed to write replay report {}", path.display()))?;
    }
    match output {
        None => println!(
            "{}",
            String::from_utf8(bytes).expect("JSON replay output is UTF-8")
        ),
        Some(_) if json => println!(
            "{}",
            String::from_utf8(bytes).expect("JSON replay output is UTF-8")
        ),
        Some(path) => println!(
            "Replayed {} observation(s) offline to {}",
            trace.observations.len(),
            path.display()
        ),
    }
    Ok(())
}
