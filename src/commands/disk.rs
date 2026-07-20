//! Scriptable disk-sentinel doctor and conservative owned-cache cleanup.

use anyhow::{Result, anyhow};
use std::path::Path;

use crate::cli::DiskCommand;

pub fn run(dir: &Path, command: DiskCommand, json: bool) -> Result<()> {
    let config = worksgood::config::Config::load_or_default(dir);
    let resource = &config.coordinator.resource_management;
    match command {
        DiskCommand::Doctor { cached } => {
            let snapshot = if cached {
                worksgood::disk_sentinel::load_snapshot(dir)?.ok_or_else(|| {
                    anyhow!("no cached disk-sentinel snapshot; run `wg disk doctor`")
                })?
            } else {
                worksgood::disk_sentinel::refresh_snapshot(dir, resource)?
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&snapshot)?);
            } else {
                println!("Disk sentinel: {:?}", snapshot.level);
                println!("  {}", snapshot.reason);
                for mount in &snapshot.mounts {
                    println!(
                        "  mount {} [{}]: {:.1}% free ({} bytes)",
                        mount.path, mount.mount_id, mount.free_percent, mount.free_bytes
                    );
                }
                println!(
                    "  projected headroom: {} bytes; active targets: {} (heavy: {})",
                    snapshot.projected_headroom_bytes,
                    snapshot.active_builds,
                    snapshot.active_build_heavy
                );
                println!(
                    "  worktrees={} bytes, .wg/agents={} bytes, .wg/log={} bytes",
                    snapshot.worktrees.bytes, snapshot.agents.bytes, snapshot.log.bytes
                );
                for target in &snapshot.targets {
                    println!(
                        "  target {} owner={}/{} size={} growth={}/s stale={}",
                        target.path,
                        target.task_id,
                        target.agent_id,
                        target.bytes,
                        target.growth_bytes_per_sec,
                        target.stale
                    );
                }
            }
        }
        DiskCommand::Cleanup { execute } => {
            let report = worksgood::disk_sentinel::cleanup_owned(dir, resource, execute)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "Disk cleanup {}: considered={}, reaped={}, freed={} bytes, compressed={}, compression-saved={} bytes, deduplicated={}, dedup-saved={} bytes",
                    if execute { "applied" } else { "dry-run" },
                    report.considered,
                    report.reaped,
                    report.bytes_freed,
                    report.compressed_files,
                    report.compression_bytes_saved,
                    report.deduplicated_files,
                    report.deduplication_bytes_saved
                );
                for preserved in report.preserved {
                    println!("  preserved {} — {}", preserved.path, preserved.reason);
                }
                if !execute {
                    println!("Dry run: use `wg disk cleanup --execute` to apply safe candidates.");
                }
            }
        }
    }
    Ok(())
}
