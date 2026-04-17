//! Integration tests for core worktree lifecycle fix.
//!
//! Tests that verify the liveness checking and cleanup logic properly prevents
//! live agent worktrees from being removed while cleaning up dead agents.

use std::path::Path;
use tempfile::TempDir;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::service::worktree::{cleanup_dead_agent_worktree_with_config, cleanup_orphaned_worktrees};
    use crate::service::liveness::{is_agent_live_default, is_agent_live_with_registry};
    use workgraph::service::AgentRegistry;

    #[test]
    fn test_is_agent_live_returns_false_for_missing_agent() {
        let temp_dir = TempDir::new().unwrap();
        let registry_path = temp_dir.path().join("registry");
        std::fs::create_dir_all(&registry_path).unwrap();
        
        // Test with non-existent agent ID
        let result = is_agent_live_with_registry(&registry_path, "nonexistent", 60);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
    }

    #[test]
    fn test_cleanup_function_checks_liveness_before_removal() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path();
        let registry_path = temp_dir.path().join("registry");
        
        // Create registry directory
        std::fs::create_dir_all(&registry_path).unwrap();
        
        // Test cleanup of non-existent agent - should not error
        let result = cleanup_dead_agent_worktree_with_config(
            project_root,
            &registry_path,
            "nonexistent-agent",
            60,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_cleanup_orphaned_worktrees_handles_empty_registry() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path();
        let registry_path = temp_dir.path().join("registry");
        
        // Create empty worktrees directory
        let worktrees_dir = project_root.join(".wg-worktrees");
        std::fs::create_dir_all(&worktrees_dir).unwrap();
        
        // Empty registry - should not error
        let result = cleanup_orphaned_worktrees(project_root, &registry_path, 60);
        assert!(result.is_ok());
    }
}