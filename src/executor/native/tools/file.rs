//! File tools: read_file, write_file, edit_file, glob, grep.

use std::fs;
use std::sync::Arc;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::sync::Mutex;
use regex::Regex;
use serde_json::json;

use super::file_cache::FileCache;
use super::{Tool, ToolOutput, ToolRegistry, truncate_for_tool};
use crate::executor::native::client::ToolDefinition;

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
            description: "Read the contents of a file. Returns numbered lines.".to_string(),
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
                    return ToolOutput::error(format!(
                        "Failed to read file '{}': {}",
                        path_str, e
                    ));
                }
            }
        };

        let lines: Vec<&str> = content.lines().collect();
        let start = if offset > 0 { offset - 1 } else { 0 };
        let end = (start + limit).min(lines.len());

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
            description: "Write content to a file. Creates parent directories if needed."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to write"
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
        let path = match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolOutput::error("Missing required parameter: path".to_string()),
        };
        let content = match input.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return ToolOutput::error("Missing required parameter: content".to_string()),
        };

        let path = Path::new(path);

        // Create parent directories if needed
        if let Some(parent) = path.parent()
            && !parent.exists()
            && let Err(e) = fs::create_dir_all(parent)
        {
            return ToolOutput::error(format!("Failed to create directories: {}", e));
        }

        match fs::write(path, content) {
            Ok(()) => ToolOutput::success(format!(
                "Successfully wrote {} bytes to {}",
                content.len(),
                path.display()
            )),
            Err(e) => {
                ToolOutput::error(format!("Failed to write file '{}': {}", path.display(), e))
            }
        }
    }
}

// ── edit_file ───────────────────────────────────────────────────────────

/// Returns a snippet of content around the expected match position.
/// Shows lines before and after for context with >>> marker on the relevant line.
fn context_snippet(content: &str, expected_pos: usize, context_lines: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return String::new();
    }

    // Find which line contains expected_pos
    let mut char_count = 0usize;
    let mut line_containing_pos = 0usize;

    for (i, line) in lines.iter().enumerate() {
        let line_len = line.len() + 1; // +1 for newline
        if char_count + line_len > expected_pos {
            line_containing_pos = i;
            break;
        }
        char_count += line_len;
    }

    let start = line_containing_pos.saturating_sub(context_lines);
    let end = (line_containing_pos + context_lines + 1).min(lines.len());

    let mut snippet = String::new();
    for (i, line) in lines[start..end].iter().enumerate() {
        let line_num = start + i + 1;
        let marker = if start + i == line_containing_pos { ">>>" } else { "   " };
        snippet.push_str(&format!("{}{:>4}| {}\n", marker, line_num, line));
    }
    snippet
}

/// Detects line ending type in content
fn detect_line_endings(content: &str) -> &'static str {
    if content.contains("\r\n") {
        "CRLF (\\r\\n)"
    } else if content.contains('\n') {
        "LF (\\n)"
    } else if content.contains('\r') {
        "CR (\\r)"
    } else {
        "none (file may be a single line)"
    }
}

/// Finds a similar region in content that might be what the user intended
fn find_similar_region<'a>(content: &'a str, search: &str) -> Option<(usize, String, String)> {
    // Get the first line of the search string (strip newlines)
    let search_trimmed = search.trim();
    if search_trimmed.is_empty() {
        return None;
    }

    let search_lines: Vec<&str> = search_trimmed.lines().collect();
    if search_lines.is_empty() {
        return None;
    }

    let first_line = search_lines[0].trim();
    if first_line.len() < 3 {
        return None;
    }

    // Try to find a line that shares significant prefix content
    let mut best_match: Option<(usize, &str)> = None;
    let mut best_score = 0;

    for (line_num, line) in content.lines().enumerate() {
        let line_trimmed = line.trim();
        if line_trimmed.is_empty() {
            continue;
        }

        // Calculate similarity based on common prefix
        let common_len = line_trimmed
            .chars()
            .zip(first_line.chars())
            .take_while(|(a, b)| a == b)
            .count();

        if common_len >= 3 && common_len > best_score {
            best_score = common_len;
            best_match = Some((line_num, line_trimmed));
        }
    }

    if let Some((line_num, matched_line)) = best_match {
        // Calculate position for context snippet
        let mut pos = 0usize;
        for (i, line) in content.lines().enumerate() {
            if i == line_num {
                break;
            }
            pos += line.len() + 1;
        }

        let snippet = context_snippet(content, pos, 3);

        // Build suggestion
        let mut suggestion = String::new();
        if matched_line != first_line {
            // Check for trailing whitespace differences
            let exp_trailing = first_line.trim_end();
            let act_trailing = matched_line.trim_end();
            if exp_trailing != act_trailing {
                suggestion.push_str(&format!(
                    "  Leading content differs:\n    expected: '{}'\n    actual:   '{}'\n",
                    exp_trailing.replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t"),
                    act_trailing.replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t")
                ));
            }

            // Check for trailing whitespace
            if first_line.len() != matched_line.len() {
                if first_line.ends_with(' ') && !matched_line.ends_with(' ') {
                    suggestion.push_str("  Note: expected trailing space is missing\n");
                } else if !first_line.ends_with(' ') && matched_line.ends_with(' ') {
                    suggestion.push_str("  Note: unexpected trailing space present\n");
                }
            }

            // Check newline differences
            let exp_has_newline = first_line.ends_with('\n') || first_line.ends_with('\r');
            let act_has_newline = matched_line.ends_with('\n') || matched_line.ends_with('\r');
            if exp_has_newline != act_has_newline {
                suggestion.push_str("  Note: newline handling differs (check if trailing newline is included)\n");
            }
        }

        if suggestion.is_empty() {
            suggestion.push_str("  No obvious whitespace differences detected");
        }

        return Some((pos, snippet, suggestion));
    }

    None
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
            description: "Perform a string replacement in a file. The old_string must appear exactly once in the file.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to edit"
                    },
                    "old_string": {
                        "type": "string",
                        "description": "The exact text to find and replace"
                    },
                    "new_string": {
                        "type": "string",
                        "description": "The replacement text"
                    }
                },
                "required": ["path", "old_string", "new_string"]
            }),
        }
    }

    async fn execute(&self, input: &serde_json::Value) -> ToolOutput {
        let path = match input.get("path").and_then(|v| v.as_str()) {
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

        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => return ToolOutput::error(format!("Failed to read file '{}': {}", path, e)),
        };

        let count = content.matches(old_string).count();
        if count == 0 {
            let line_ending = detect_line_endings(&content);
            let line_ending_suggestion: &str = if old_string.contains('\n') || old_string.contains('\r') {
                "\n\nTip: If your old_string contains newlines, make sure they match the file's line endings."
            } else {
                ""
            };

            let similar_content = find_similar_region(&content, old_string);

            let error_msg = if let Some((pos, snippet, suggestion)) = similar_content {
                format!(
                    "old_string not found in '{}'.\n\n\
                    File line endings: {}{}\n\n\
                    Similar content found at position {}:\n\
                    {}\n\
                    {}\
                    \n\
                    Common issues to check:\n\
                    - Extra or missing spaces/tabs at line ends\n\
                    - Different line endings (\\n vs \\r\\n)\n\
                    - Inclusion or exclusion of trailing newlines\n\
                    - Whitespace characters that look identical but differ (e.g., space vs tab)",
                    path, line_ending, line_ending_suggestion, pos, snippet, suggestion
                )
            } else {
                format!(
                    "old_string not found in '{}'.\n\n\
                    File line endings: {}{}\n\n\
                    File preview:\n\
                    {}\n\n\
                    Common issues to check:\n\
                    - Extra or missing spaces/tabs at line ends\n\
                    - Different line endings (\\n vs \\r\\n)\n\
                    - Inclusion or exclusion of trailing newlines\n\
                    - Whitespace characters that look identical but differ (e.g., space vs tab)",
                    path,
                    line_ending,
                    line_ending_suggestion,
                    context_snippet(&content, 0, 10)
                )
            };
            return ToolOutput::error(error_msg);
        }
        if count > 1 {
            // Find all match positions and show context for each
            let mut matches = Vec::new();
            let mut search_start = 0usize;
            while let Some(pos) = content[search_start..].find(old_string) {
                let abs_pos = search_start + pos;
                let line_num = content[..abs_pos].chars().filter(|&c| c == '\n').count() + 1;
                matches.push((abs_pos, line_num));
                search_start = abs_pos + 1;
            }

            let mut error_msg = format!(
                "old_string found {} times in '{}'. It must be unique.\n\n\
                Matches occurred at:",
                count, path
            );

            for (pos, line_num) in &matches {
                let snippet = context_snippet(&content, *pos, 2);
                error_msg.push_str(&format!(
                    "\n\n--- Match at line {}, position {} ---\n{}",
                    line_num, pos, snippet
                ));
            }

            error_msg.push_str("\n\nTip: Provide more surrounding context to make the match unique.");
            return ToolOutput::error(error_msg);
        }

        let new_content = content.replacen(old_string, new_string, 1);
        match fs::write(path, &new_content) {
            Ok(()) => ToolOutput::success(format!("Successfully edited {}", path)),
            Err(e) => ToolOutput::error(format!("Failed to write file '{}': {}", path, e)),
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
