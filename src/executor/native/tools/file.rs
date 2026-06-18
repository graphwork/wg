//! File tools: read_file, write_file, edit_file, glob, grep.

use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use regex::Regex;
use serde_json::json;
use tokio::sync::Mutex;

use super::file_cache::FileCache;
use super::fuzzy_match::{FuzzyOutcome, MatchLevel, fuzzy_replace};
use super::{Tool, ToolOutput, ToolRegistry, truncate_for_tool};
use crate::executor::native::client::ToolDefinition;

/// Whether `--yolo` mode is active for the current nex process. When on,
/// the workspace write sandbox is lifted so `write_file`/`edit_file` may
/// target paths outside the cwd subtree. Set by `wg nex --yolo` (or the
/// `WG_NEX_YOLO` env var) which normalizes `WG_NEX_YOLO` to `1`/`0` at
/// startup — see `crate::commands::nex`.
fn yolo_enabled() -> bool {
    std::env::var("WG_NEX_YOLO").ok().is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Resolve a user-provided path against the current working directory and
/// reject anything that escapes the cwd subtree.
///
/// This is the load-bearing sandbox for `write_file` and `edit_file`: a
/// hallucinating model that emits `/home/user/some-other-repo/src/foo.rs`
/// would otherwise happily write there. With this gate, the write is
/// refused before it touches disk.
///
/// Allowed:
///   - relative paths under cwd (e.g. `src/foo.rs`)
///   - absolute paths inside cwd (e.g. cwd-prefixed)
///
/// Rejected:
///   - absolute paths outside cwd (`/etc/passwd`, `/home/other/...`)
///   - relative paths that escape via `..` after canonicalization
///
/// When `allow_outside_cwd` is true (yolo mode), the boundary check is
/// skipped: the path is still resolved/normalized but writes outside the
/// cwd subtree are permitted.
///
/// Non-existent targets are handled by canonicalizing the deepest existing
/// ancestor and appending the remaining (not-yet-created) components.
fn resolve_inside_cwd(input: &str, allow_outside_cwd: bool) -> Result<PathBuf, String> {
    let cwd = std::env::current_dir()
        .map_err(|e| format!("cannot determine current working directory: {}", e))?;
    let cwd_canonical = cwd
        .canonicalize()
        .map_err(|e| format!("cannot canonicalize cwd {:?}: {}", cwd, e))?;

    let raw = Path::new(input);
    let target = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        cwd.join(raw)
    };

    // If the target exists, canonicalize it directly. Otherwise walk up to
    // find the deepest existing ancestor, canonicalize that, then append the
    // remaining (non-existent) tail components.
    let canonical = match target.canonicalize() {
        Ok(c) => c,
        Err(_) => {
            let mut ancestor = target.as_path();
            while !ancestor.exists() {
                ancestor = match ancestor.parent() {
                    Some(p) => p,
                    None => {
                        return Err(format!("cannot resolve path: {}", input));
                    }
                };
            }
            let real_ancestor = ancestor
                .canonicalize()
                .map_err(|e| format!("cannot canonicalize {:?}: {}", ancestor, e))?;
            let suffix = target
                .strip_prefix(ancestor)
                .map_err(|_| format!("internal path resolution error for: {}", input))?;
            real_ancestor.join(suffix)
        }
    };

    if !allow_outside_cwd && !canonical.starts_with(&cwd_canonical) {
        return Err(format!(
            "path '{}' resolves to '{}' which is outside the working directory '{}'. \
             Writes are restricted to the cwd subtree. Use a path inside the current \
             working directory, or tell the user the action you intended so they can \
             take it themselves. (Pass --yolo to lift this restriction.)",
            input,
            canonical.display(),
            cwd_canonical.display()
        ));
    }
    Ok(canonical)
}

/// Register all file tools into the registry.
pub fn register_file_tools(registry: &mut ToolRegistry) {
    let cache = Arc::new(Mutex::new(FileCache::new()));
    registry.register(Box::new(ReadFileTool { cache }));
    registry.register(Box::new(WriteFileTool));
    registry.register(Box::new(EditFileTool));
    registry.register(Box::new(GlobTool));
    registry.register(Box::new(GrepTool));
}

// ── read_file ───────────────────────────────────────────────────────────

struct ReadFileTool {
    cache: Arc<Mutex<FileCache>>,
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file. Returns numbered lines. Use `offset`/`limit` to \
                          slice. For LLM-answered queries over the content use `summarize` \
                          (map-reduce, good for 'find X' / 'list all Y') or `reader` \
                          (deep traversal with a working directory)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to read"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Line number to start reading from (1-based)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to read (default: 2000)"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn execute(&self, input: &serde_json::Value) -> ToolOutput {
        let path_str = match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolOutput::error("Missing required parameter: path".to_string()),
        };

        let offset = input.get("offset").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
        let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(2000) as usize;

        // Get mtime for cache validation; error on stat failure.
        let mtime = match fs::metadata(path_str).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(e) => {
                return ToolOutput::error(format!("Failed to read file '{}': {}", path_str, e));
            }
        };

        let path_buf = PathBuf::from(path_str);

        // Try cache first
        let cached: Option<String> = {
            let mut cache = self.cache.lock().await;
            cache.get(&path_buf, mtime)
        };

        let (content, from_cache) = if let Some(hit) = cached {
            (hit, true)
        } else {
            match fs::read_to_string(path_str) {
                Ok(content) => {
                    let mut cache = self.cache.lock().await;
                    cache.insert(path_buf, content.clone(), mtime);
                    (content, false)
                }
                Err(e) => {
                    return ToolOutput::error(format!("Failed to read file '{}': {}", path_str, e));
                }
            }
        };

        let lines: Vec<&str> = content.lines().collect();
        let start = if offset > 0 { offset - 1 } else { 0 };
        let end = (start + limit).min(lines.len());

        // Bounds check: return error if offset exceeds file length
        if start >= lines.len() {
            return ToolOutput::error(format!(
                "File has {} lines, offset {} is out of range",
                lines.len(),
                offset
            ));
        }

        let mut output = String::new();
        for (i, line) in lines[start..end].iter().enumerate() {
            let line_num = start + i + 1;
            // Truncate long lines
            let truncated = if line.len() > 2000 {
                &line[..line.floor_char_boundary(2000)]
            } else {
                line
            };
            output.push_str(&format!("{:>6}\t{}\n", line_num, truncated));
        }

        if from_cache {
            output.push_str("\n[cached read, file unchanged]\n");
        }

        let total_lines = lines.len();
        if end < total_lines {
            output.push_str(&format!(
                "\n[truncated: showed lines {}..{} of {}]\n",
                start + 1,
                end,
                total_lines,
            ));
        }

        ToolOutput::success(truncate_for_tool(&output, "read_file"))
    }
}

// ── write_file ──────────────────────────────────────────────────────────

struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "write_file".to_string(),
            description: "Save content to a file on disk at the given path, replacing the \
                          entire file. Best for CREATING a new file or a full rewrite. To \
                          CHANGE an existing file, prefer edit_file with a targeted \
                          old_string/new_string replacement instead of rewriting the whole \
                          file — repeatedly rewriting a file to fix small errors is wasteful \
                          and reintroduces bugs. Writes are restricted to the current working \
                          directory tree — paths outside cwd are rejected. Do NOT use this to \
                          display or return content to the user — include it in your text \
                          response instead."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to write (must resolve inside cwd)"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write to the file"
                    }
                },
                "required": ["path", "content"]
            }),
        }
    }

    async fn execute(&self, input: &serde_json::Value) -> ToolOutput {
        let path_input = match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolOutput::error("Missing required parameter: path".to_string()),
        };
        let content = match input.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return ToolOutput::error("Missing required parameter: content".to_string()),
        };

        let safe_path = match resolve_inside_cwd(path_input, yolo_enabled()) {
            Ok(p) => p,
            Err(e) => return ToolOutput::error(e),
        };

        if let Some(parent) = safe_path.parent()
            && !parent.exists()
            && let Err(e) = fs::create_dir_all(parent)
        {
            return ToolOutput::error(format!("Failed to create directories: {}", e));
        }

        match fs::write(&safe_path, content) {
            Ok(()) => ToolOutput::success(format!(
                "Successfully wrote {} bytes to {}",
                content.len(),
                safe_path.display()
            )),
            Err(e) => ToolOutput::error(format!(
                "Failed to write file '{}': {}",
                safe_path.display(),
                e
            )),
        }
    }
}

struct EditFileTool;

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "edit_file".to_string(),
            description: "PREFERRED way to change an existing file: make a small, targeted \
                          string replacement instead of rewriting the whole file with \
                          write_file. Replaces old_string with new_string. Matching is \
                          robust — whitespace, indentation, and line endings (\\n vs \\r\\n) \
                          are matched leniently, and the replacement is re-indented to the \
                          file's actual indentation, so you do NOT need byte-perfect old_string. \
                          old_string must identify a unique location; include a few surrounding \
                          lines for context if it would otherwise be ambiguous. If it can't be \
                          matched you get a near-miss diagnostic showing the closest line — fix \
                          old_string and retry rather than falling back to a full rewrite. After \
                          a failed build, fix each error with a focused edit_file call."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to edit"
                    },
                    "old_string": {
                        "type": "string",
                        "description": "The text to find and replace. Whitespace/indentation/line-ending differences are tolerated; it must identify a unique location (add surrounding context lines if needed)."
                    },
                    "new_string": {
                        "type": "string",
                        "description": "The replacement text. It is automatically re-indented to match the file when indentation is the only difference."
                    },
                    "normalize_whitespace": {
                        "type": "boolean",
                        "description": "Opt-in: also collapse runs of interior whitespace when matching (e.g. treat 'a  b' and 'a b' as equal). Off by default to avoid over-matching. Leading/trailing whitespace, indentation, and line endings are ALREADY tolerated without this flag."
                    },
                    "normalize_line_endings": {
                        "type": "boolean",
                        "description": "Deprecated/no-op: \\n and \\r\\n are always treated as equivalent now. Accepted for backward compatibility."
                    }
                },
                "required": ["path", "old_string", "new_string"]
            }),
        }
    }

    async fn execute(&self, input: &serde_json::Value) -> ToolOutput {
        let path_input = match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolOutput::error("Missing required parameter: path".to_string()),
        };
        let old_string = match input.get("old_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolOutput::error("Missing required parameter: old_string".to_string()),
        };
        let new_string = match input.get("new_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolOutput::error("Missing required parameter: new_string".to_string()),
        };
        // `normalize_whitespace` is an opt-in to ALSO collapse interior
        // whitespace (the most aggressive level). Leading/trailing whitespace,
        // indentation, and line endings are tolerated automatically without
        // any flag. `normalize_line_endings` is now a no-op (always on) and is
        // read only to stay backward-compatible with callers that set it.
        let allow_collapse = input
            .get("normalize_whitespace")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let _ = input.get("normalize_line_endings");

        let safe_path = match resolve_inside_cwd(path_input, yolo_enabled()) {
            Ok(p) => p,
            Err(e) => return ToolOutput::error(e),
        };
        let path = safe_path.to_string_lossy().into_owned();
        let path: &str = &path;

        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => return ToolOutput::error(format!("Failed to read file '{}': {}", path, e)),
        };

        match fuzzy_replace(&content, old_string, new_string, allow_collapse) {
            FuzzyOutcome::Unique { new_content, level } => match fs::write(path, &new_content) {
                Ok(()) => {
                    let note = if level == MatchLevel::Exact {
                        format!("Successfully edited {}", path)
                    } else {
                        format!("Successfully edited {} ({} match)", path, level.label())
                    };
                    ToolOutput::success(note)
                }
                Err(e) => ToolOutput::error(format!("Failed to write file '{}': {}", path, e)),
            },
            FuzzyOutcome::Ambiguous { count, level } => ToolOutput::error(format!(
                "old_string found {} times in '{}' ({} match). It must be unique — \
                 provide more surrounding context so it identifies a single location.",
                count,
                path,
                level.label()
            )),
            FuzzyOutcome::NoMatch { diagnostic } => {
                ToolOutput::error(format!("{} in '{}'", diagnostic, path))
            }
        }
    }
}

// ── glob ────────────────────────────────────────────────────────────────

struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "glob".to_string(),
            description: "Find files matching a glob pattern.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern to match files (e.g., '**/*.rs', 'src/**/*.ts')"
                    },
                    "path": {
                        "type": "string",
                        "description": "Base directory to search in (default: current directory)"
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn execute(&self, input: &serde_json::Value) -> ToolOutput {
        let pattern = match input.get("pattern").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolOutput::error("Missing required parameter: pattern".to_string()),
        };

        let base = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");

        // Combine base path with pattern
        let full_pattern = if pattern.starts_with('/') {
            pattern.to_string()
        } else {
            format!("{}/{}", base, pattern)
        };

        match glob::glob(&full_pattern) {
            Ok(paths) => {
                let mut results: Vec<String> = Vec::new();
                for entry in paths {
                    match entry {
                        Ok(path) => results.push(path.display().to_string()),
                        Err(e) => results.push(format!("[error: {}]", e)),
                    }
                }
                if results.is_empty() {
                    ToolOutput::success("No files matched the pattern.".to_string())
                } else {
                    ToolOutput::success(truncate_for_tool(&results.join("\n"), "glob"))
                }
            }
            Err(e) => ToolOutput::error(format!("Invalid glob pattern '{}': {}", pattern, e)),
        }
    }
}

// ── grep ────────────────────────────────────────────────────────────────

struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "grep".to_string(),
            description: "Search file contents using a regex pattern. Returns matching lines with file paths and line numbers.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex pattern to search for"
                    },
                    "path": {
                        "type": "string",
                        "description": "File or directory to search in (default: current directory)"
                    },
                    "glob": {
                        "type": "string",
                        "description": "Glob pattern to filter files (e.g., '*.rs')"
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn execute(&self, input: &serde_json::Value) -> ToolOutput {
        let pattern_str = match input.get("pattern").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolOutput::error("Missing required parameter: pattern".to_string()),
        };

        let search_path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");

        let glob_filter = input.get("glob").and_then(|v| v.as_str());

        let re = match Regex::new(pattern_str) {
            Ok(r) => r,
            Err(e) => return ToolOutput::error(format!("Invalid regex '{}': {}", pattern_str, e)),
        };

        let path = PathBuf::from(search_path);
        let mut results = Vec::new();
        let max_results = 500;

        if path.is_file() {
            search_file(&path, &re, &mut results, max_results);
        } else if path.is_dir() {
            let glob_pattern = glob_filter.and_then(|g| glob::Pattern::new(g).ok());

            for entry in walkdir::WalkDir::new(&path)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_file())
            {
                if results.len() >= max_results {
                    break;
                }

                let entry_path = entry.path();

                // Apply glob filter if specified
                if let Some(ref pat) = glob_pattern
                    && let Some(name) = entry_path.file_name().and_then(|n| n.to_str())
                    && !pat.matches(name)
                {
                    continue;
                }

                // Skip binary files and hidden directories
                if is_likely_binary(entry_path)
                    || entry_path
                        .components()
                        .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
                {
                    continue;
                }

                search_file(entry_path, &re, &mut results, max_results);
            }
        } else {
            return ToolOutput::error(format!("Path not found: {}", search_path));
        }

        if results.is_empty() {
            ToolOutput::success("No matches found.".to_string())
        } else {
            let truncated = results.len() >= max_results;
            let mut output = results.join("\n");
            if truncated {
                output.push_str(&format!(
                    "\n\n[Results truncated at {} matches]",
                    max_results
                ));
            }
            ToolOutput::success(truncate_for_tool(&output, "grep"))
        }
    }
}

fn search_file(path: &Path, re: &Regex, results: &mut Vec<String>, max: usize) {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return,
    };

    let reader = std::io::BufReader::new(file);
    for (line_num, line) in reader.lines().enumerate() {
        if results.len() >= max {
            break;
        }
        if let Ok(line) = line
            && re.is_match(&line)
        {
            results.push(format!("{}:{}: {}", path.display(), line_num + 1, line));
        }
    }
}

fn is_likely_binary(path: &Path) -> bool {
    let binary_extensions = [
        "png", "jpg", "jpeg", "gif", "bmp", "ico", "svg", "woff", "woff2", "ttf", "eot", "mp3",
        "mp4", "avi", "mov", "zip", "tar", "gz", "bz2", "xz", "7z", "rar", "pdf", "doc", "docx",
        "xls", "xlsx", "ppt", "pptx", "exe", "dll", "so", "dylib", "o", "a", "class", "jar", "pyc",
        "wasm", "zst",
    ];

    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| binary_extensions.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── resolve_inside_cwd sandbox tests ──────────────────────────────
    //
    // These tests mutate the process-wide cwd, so they're serialized
    // against other cwd-sensitive tests via serial_test. Using a fresh
    // TempDir per test isolates them from each other.

    #[test]
    #[serial_test::serial]
    fn test_sandbox_allows_relative_path_inside_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        let resolved =
            resolve_inside_cwd("a/b/c.txt", false).expect("relative path should be allowed");
        assert!(resolved.starts_with(tmp.path().canonicalize().unwrap()));
        assert!(resolved.ends_with("a/b/c.txt"));
        std::env::set_current_dir(prev).unwrap();
    }

    #[test]
    #[serial_test::serial]
    fn test_sandbox_allows_absolute_path_inside_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        let canon_cwd = tmp.path().canonicalize().unwrap();
        let abs = canon_cwd.join("foo.txt");
        let resolved = resolve_inside_cwd(abs.to_str().unwrap(), false)
            .expect("abs path inside cwd should be OK");
        assert_eq!(resolved, abs);
        std::env::set_current_dir(prev).unwrap();
    }

    #[test]
    #[serial_test::serial]
    fn test_sandbox_rejects_absolute_path_outside_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        let err = resolve_inside_cwd("/etc/passwd", false).expect_err("should reject escape");
        assert!(
            err.contains("outside the working directory"),
            "got: {}",
            err
        );
        std::env::set_current_dir(prev).unwrap();
    }

    #[test]
    #[serial_test::serial]
    fn test_sandbox_rejects_dotdot_escape() {
        // cwd = tmp/inner, user passes "../outside.txt" which resolves to tmp/outside.txt
        let tmp = tempfile::tempdir().unwrap();
        let inner = tmp.path().join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&inner).unwrap();
        let err = resolve_inside_cwd("../outside.txt", false)
            .expect_err("dotdot escape should be rejected");
        assert!(
            err.contains("outside the working directory"),
            "got: {}",
            err
        );
        std::env::set_current_dir(prev).unwrap();
    }

    #[test]
    #[serial_test::serial]
    fn test_sandbox_permits_nonexistent_target_inside_cwd() {
        // The target file doesn't exist yet; sandbox should still validate its parent.
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        let resolved = resolve_inside_cwd("does/not/exist/yet.txt", false).expect("new paths OK");
        assert!(resolved.starts_with(tmp.path().canonicalize().unwrap()));
        std::env::set_current_dir(prev).unwrap();
    }

    // ─── yolo mode (allow_outside_cwd) tests ───────────────────────────

    #[test]
    #[serial_test::serial]
    fn test_yolo_allows_absolute_path_outside_cwd() {
        // The exact escape rejected above is permitted when yolo is on.
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        let resolved = resolve_inside_cwd("/etc/passwd", true)
            .expect("yolo should permit absolute path outside cwd");
        assert_eq!(resolved, Path::new("/etc/passwd"));
        std::env::set_current_dir(prev).unwrap();
    }

    #[test]
    #[serial_test::serial]
    fn test_yolo_allows_dotdot_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let inner = tmp.path().join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&inner).unwrap();
        let resolved =
            resolve_inside_cwd("../outside.txt", true).expect("yolo should permit dotdot escape");
        // Resolves to the parent (tmp) dir's sibling file, outside cwd (inner).
        assert!(resolved.ends_with("outside.txt"));
        assert!(!resolved.starts_with(inner.canonicalize().unwrap()));
        std::env::set_current_dir(prev).unwrap();
    }

    #[test]
    #[serial_test::serial]
    fn test_yolo_still_allows_paths_inside_cwd() {
        // yolo is a superset: paths inside cwd keep working.
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        let resolved =
            resolve_inside_cwd("a/b/c.txt", true).expect("inside-cwd path should still resolve");
        assert!(resolved.starts_with(tmp.path().canonicalize().unwrap()));
        std::env::set_current_dir(prev).unwrap();
    }

    #[tokio::test]
    async fn test_read_file_offset_beyond_end_returns_error() {
        use crate::executor::native::tools::file_cache::FileCache;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let cache = Arc::new(Mutex::new(FileCache::new()));
        let tool = ReadFileTool { cache };

        // Create a temp file with exactly 3 lines
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let temp_path = temp_file.path().to_str().unwrap();
        std::fs::write(temp_path, "line1\nline2\nline3\n").unwrap();

        // Call read_file with offset=10 (beyond the 3 lines in the file)
        let input = serde_json::json!({
            "path": temp_path,
            "offset": 10
        });

        let output = tool.execute(&input).await;

        // Should return an error, not panic
        assert!(
            output.is_error,
            "Expected error for offset beyond file length, got: {:?}",
            output
        );
        assert!(
            output.content.contains("out of range"),
            "Error message should mention 'out of range', got: {:?}",
            output.content
        );
    }
}
