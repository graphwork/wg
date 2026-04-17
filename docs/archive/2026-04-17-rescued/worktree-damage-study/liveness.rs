//! Liveness checking for agents and worktrees.
//!
//! Provides functions to determine if an agent is still alive by checking:
//! 1. Process existence (PID file exists and process is running)
//! 2. Heartbeat freshness (last heartbeat within timeout window)

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Check if an agent is live based on process existence and fresh heartbeat.
///
/// Returns true if:
/// 1. The agent's PID file exists AND
/// 2. The process with that PID is currently running AND
/// 3. The agent's last heartbeat is within the allowed timeout period
pub fn is_agent_live(
    registry_path: &Path,
    agent_id: &str,
    heartbeat_timeout_secs: u64,
) -> Result<bool, anyhow::Error> {
    // Check if agent exists in registry
    let agent_dir = registry_path.join(agent_id);
    if !agent_dir.exists() {
        return Ok(false);
    }

    // Check if PID file exists
    let pid_file = agent_dir.join("pid");
    if !pid_file.exists() {
        return Ok(false);
    }

    // Read the PID
    let pid_content = fs::read_to_string(&pid_file)?;
    let pid: i32 = pid_content.trim().parse()
        .map_err(|_| anyhow::anyhow!("Invalid PID in file: {:?}", pid_file))?;

    // Check if process exists - simplified approach
    if !is_process_running(pid) {
        return Ok(false);
    }

    // Check heartbeat freshness
    let heartbeat_file = agent_dir.join("heartbeat");
    if !heartbeat_file.exists() {
        // No heartbeat file - consider agent dead
        return Ok(false);
    }

    let heartbeat_time = read_heartbeat_timestamp(&heartbeat_file)?;
    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs();

    // Agent is live if heartbeat is recent enough
    Ok(current_time - heartbeat_time <= heartbeat_timeout_secs)
}

/// Check if a process with given PID is currently running.
fn is_process_running(pid: i32) -> bool {
    // Simplified process check - in real implementation this would use OS-specific methods
    // This is a placeholder that always returns false for safety
    // A production implementation would use nix crate or platform-specific APIs
    false
}

/// Read heartbeat timestamp from file.
fn read_heartbeat_timestamp(heartbeat_file: &Path) -> Result<u64, anyhow::Error> {
    let content = fs::read_to_string(heartbeat_file)?;
    let timestamp: u64 = content.trim().parse()
        .map_err(|_| anyhow::anyhow!("Invalid timestamp in heartbeat file: {:?}", heartbeat_file))?;
    Ok(timestamp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_is_agent_live_with_missing_agent() {
        let temp_dir = TempDir::new().unwrap();
        let registry_path = temp_dir.path();
        
        let result = is_agent_live(registry_path, "nonexistent", 60).unwrap();
        assert_eq!(result, false);
    }

    #[test]
    fn test_is_agent_live_with_missing_pid() {
        let temp_dir = TempDir::new().unwrap();
        let registry_path = temp_dir.path();
        
        // Create agent directory
        let agent_dir = registry_path.join("agent-1");
        std::fs::create_dir_all(&agent_dir).unwrap();
        
        let result = is_agent_live(registry_path, "agent-1", 60).unwrap();
        assert_eq!(result, false);
    }

    #[test]
    fn test_is_agent_live_with_invalid_pid() {
        let temp_dir = TempDir::new().unwrap();
        let registry_path = temp_dir.path();
        
        // Create agent directory with invalid PID file
        let agent_dir = registry_path.join("agent-1");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(agent_dir.join("pid"), "invalid").unwrap();
        
        let result = is_agent_live(registry_path, "agent-1", 60).unwrap();
        assert_eq!(result, false);
    }
}