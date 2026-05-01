//! Shared testing state for worktrees
//!
//! This module implements shared testing state that allows worktrees to share build artifacts and declared fixtures.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

/// Configuration for shared testing state in worktrees
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SharedStateConfig {
    /// List of paths (relative to repo root) that get symlinked into every worktree after creation
    #[serde(default)]
    pub shared_paths: Vec<String>,
}

impl SharedStateConfig {
    /// Create a new shared state configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a path to be shared across worktrees
    pub fn add_shared_path(&mut self, path: impl Into<String>) {
        self.shared_paths.push(path.into());
    }
}

/// Create symlinks for shared paths in a worktree
pub fn setup_shared_paths(worktree_root: &Path, shared_paths: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let worktree_root = worktree_root.canonicalize()?;
    
    // Create the shared directory if it doesn't exist
    let shared_dir = worktree_root.join(".wg").join("shared");
    fs::create_dir_all(&shared_dir)?;
    
    for path_str in shared_paths {
        let src_path = Path::new(path_str);
        
        // Skip if the source path is empty or invalid
        if src_path.to_string_lossy().is_empty() {
            continue;
        }
        
        // If the source path doesn't exist, we can't create a symlink
        if !src_path.exists() {
            // Create the directory structure if needed for the destination
            let dest_path = worktree_root.join(src_path);
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent)?;
            }
            continue;
        }
        
        // Create symlink from shared location to worktree location
        let dest_path = worktree_root.join(src_path);
        let shared_path = shared_dir.join(src_path);
        
        // Create parent directories for the shared path
        if let Some(parent) = shared_path.parent() {
            fs::create_dir_all(parent)?;
        }
        
        // Create symlink from shared location to worktree location
        if dest_path.exists() {
            // Remove existing file/directory if it exists
            if dest_path.is_dir() {
                fs::remove_dir_all(&dest_path)?;
            } else {
                fs::remove_file(&dest_path)?;
            }
        }
        
        // Create symlink
        if let Err(e) = std::os::unix::fs::symlink(&shared_path, &dest_path) {
            // If symlink fails, try copying as fallback
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                // Remove and retry
                if dest_path.is_dir() {
                    fs::remove_dir_all(&dest_path)?;
                } else {
                    fs::remove_file(&dest_path)?;
                }
                std::os::unix::fs::symlink(&shared_path, &dest_path)?;
            } else {
                return Err(format!("Failed to create symlink from {} to {}: {}", 
                                  shared_path.display(), dest_path.display(), e).into());
            }
        }
    }
    
    Ok(())
}

/// Get the shared target directory path
pub fn get_shared_target_dir(repo_root: &Path) -> PathBuf {
    repo_root.join(".wg").join("shared-target")
}

/// Set up environment variables for worktree agents
pub fn setup_agent_environment(repo_root: &Path) -> std::collections::HashMap<String, String> {
    let mut env_vars = std::collections::HashMap::new();
    
    // Set CARGO_TARGET_DIR to a shared path
    let shared_target_dir = get_shared_target_dir(repo_root);
    env_vars.insert(
        "CARGO_TARGET_DIR".to_string(),
        shared_target_dir.to_string_lossy().to_string(),
    );
    
    env_vars
}