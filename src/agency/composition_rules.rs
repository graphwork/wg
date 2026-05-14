//! Composition rules overlay parsed from `~/.agency/composition-rules.csv`
//! (or any path supplied by the caller).
//!
//! The file shape mirrors agency v1.2.4's CSV column order:
//!
//! ```text
//! agent_type,rule,max_role_components,max_desired_outcomes,max_trade_off_configs,all_projects,project_ids
//! assigner,balanced,2,1,1,true,
//! evaluator,strict,3,1,1,true,
//! evolver,exploratory,5,2,1,false,proj-a;proj-b
//! ```
//!
//! The overlay caps the *number* of role components / desired outcomes /
//! trade-off configurations a given functional-agent type may compose at
//! assignment time. `all_projects=false` plus a non-empty `project_ids`
//! list scopes a rule to specific WG projects.
//!
//! `CompositionRulesWatcher` re-reads the file on demand using mtime
//! invalidation — no notify thread, no daemon restart required.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// One row from `composition-rules.csv`.
#[derive(Debug, Clone)]
pub struct CompositionRule {
    /// Functional-agent type this rule applies to (`assigner`, `evaluator`,
    /// `evolver`, `agent_creator`, `task`).
    pub agent_type: String,
    /// Free-form policy hint (e.g. `balanced`, `strict`, `open`,
    /// `exploratory`). Mirrors the agency CSV `rule` column.
    pub rule: String,
    /// Maximum number of role components this agent_type may compose.
    /// `None` means unlimited.
    pub max_role_components: Option<u32>,
    /// Maximum number of desired outcomes per role.
    pub max_desired_outcomes: Option<u32>,
    /// Maximum number of trade-off configurations per agent.
    pub max_trade_off_configs: Option<u32>,
    /// If true, this rule applies to every WG project.
    pub all_projects: bool,
    /// If `all_projects=false`, the rule only applies to these project IDs.
    /// Empty list with `all_projects=false` means the rule applies nowhere.
    pub project_ids: Vec<String>,
}

impl CompositionRule {
    /// Returns true if `count` is within the role-components cap.
    pub fn role_components_within_cap(&self, count: usize) -> bool {
        self.max_role_components
            .is_none_or(|cap| count as u64 <= u64::from(cap))
    }

    /// Returns true if `count` is within the desired-outcomes cap.
    pub fn desired_outcomes_within_cap(&self, count: usize) -> bool {
        self.max_desired_outcomes
            .is_none_or(|cap| count as u64 <= u64::from(cap))
    }

    /// Returns true if `count` is within the trade-off-configs cap.
    pub fn trade_off_configs_within_cap(&self, count: usize) -> bool {
        self.max_trade_off_configs
            .is_none_or(|cap| count as u64 <= u64::from(cap))
    }
}

/// All rules loaded from the overlay file.
#[derive(Debug, Clone, Default)]
pub struct CompositionRulesOverlay {
    pub rules: Vec<CompositionRule>,
}

impl CompositionRulesOverlay {
    /// Look up the rule for a bare agent_type (`"assigner"`, `"evaluator"`,
    /// `"evolver"`, `"agent_creator"`, `"task"`). Returns the first match.
    pub fn rule_for(&self, agent_type: &str) -> Option<&CompositionRule> {
        self.rules.iter().find(|r| r.agent_type == agent_type)
    }

    /// Look up a rule by an `agency` scope string (`"meta:assigner"`,
    /// `"meta:evaluator"`, etc.). Strips the `meta:` prefix and dispatches
    /// to [`Self::rule_for`]. Returns `None` for `"task"` unless an explicit
    /// `task` row is present.
    pub fn rule_for_scope(&self, scope: &str) -> Option<&CompositionRule> {
        let agent_type = scope.strip_prefix("meta:").unwrap_or(scope);
        self.rule_for(agent_type)
    }
}

/// Parse `composition-rules.csv` into [`CompositionRulesOverlay`].
///
/// Returns an empty overlay if the file does not exist (the file is optional).
/// Returns a parse error for malformed content so the caller can surface a
/// useful message to the user.
pub fn load_composition_rules(path: &Path) -> std::io::Result<CompositionRulesOverlay> {
    if !path.exists() {
        return Ok(CompositionRulesOverlay::default());
    }
    let text = std::fs::read_to_string(path)?;
    parse_composition_rules(&text)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Parse the CSV body into rules.
///
/// Header row is required and validated by column name (order-tolerant).
fn parse_composition_rules(text: &str) -> Result<CompositionRulesOverlay, String> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .trim(csv::Trim::All)
        .flexible(true)
        .from_reader(text.as_bytes());

    let headers = reader
        .headers()
        .map_err(|e| format!("invalid CSV header: {}", e))?
        .clone();
    let header_index =
        |name: &str| -> Option<usize> { headers.iter().position(|h| h.eq_ignore_ascii_case(name)) };

    let agent_type_idx = header_index("agent_type")
        .ok_or_else(|| "missing required column: agent_type".to_string())?;
    let rule_idx = header_index("rule");
    let max_rc_idx = header_index("max_role_components");
    let max_do_idx = header_index("max_desired_outcomes");
    let max_to_idx = header_index("max_trade_off_configs");
    let all_proj_idx = header_index("all_projects");
    let proj_ids_idx = header_index("project_ids");

    let mut rules = Vec::new();
    for (line_no, record) in reader.records().enumerate() {
        let record = record.map_err(|e| format!("CSV row {} parse error: {}", line_no + 2, e))?;
        let agent_type = record
            .get(agent_type_idx)
            .ok_or_else(|| format!("CSV row {} missing agent_type column", line_no + 2))?
            .trim()
            .to_string();
        if agent_type.is_empty() {
            continue;
        }

        let rule = rule_idx
            .and_then(|i| record.get(i))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let max_role_components = parse_u32_cell(max_rc_idx, &record)?;
        let max_desired_outcomes = parse_u32_cell(max_do_idx, &record)?;
        let max_trade_off_configs = parse_u32_cell(max_to_idx, &record)?;
        let all_projects = all_proj_idx
            .and_then(|i| record.get(i))
            .map(|s| matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes"))
            .unwrap_or(true);
        let project_ids: Vec<String> = proj_ids_idx
            .and_then(|i| record.get(i))
            .map(|s| {
                s.split([';', ','])
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        rules.push(CompositionRule {
            agent_type,
            rule,
            max_role_components,
            max_desired_outcomes,
            max_trade_off_configs,
            all_projects,
            project_ids,
        });
    }

    Ok(CompositionRulesOverlay { rules })
}

fn parse_u32_cell(idx: Option<usize>, record: &csv::StringRecord) -> Result<Option<u32>, String> {
    let Some(i) = idx else { return Ok(None) };
    let Some(raw) = record.get(i) else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse::<u32>()
        .map(Some)
        .map_err(|e| format!("invalid u32 cell {:?}: {}", trimmed, e))
}

/// mtime-invalidated cached reader of `composition-rules.csv`.
///
/// `current()` re-reads the file when its mtime has advanced since the last
/// read. This satisfies the file-watch validation requirement (reload after
/// edit without daemon restart) without spawning a notify thread.
pub struct CompositionRulesWatcher {
    path: PathBuf,
    last_mtime: Option<SystemTime>,
    last_existed: bool,
    cached: CompositionRulesOverlay,
}

impl CompositionRulesWatcher {
    /// Create a watcher; loads the file immediately if present.
    pub fn new(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let mut watcher = Self {
            path,
            last_mtime: None,
            last_existed: false,
            cached: CompositionRulesOverlay::default(),
        };
        watcher.refresh();
        watcher
    }

    /// Returns the current overlay, re-reading from disk if the file's
    /// mtime has advanced (or if a previously-missing file now exists).
    pub fn current(&mut self) -> &CompositionRulesOverlay {
        self.refresh();
        &self.cached
    }

    /// Path being watched.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn refresh(&mut self) {
        let meta = std::fs::metadata(&self.path);
        match meta {
            Ok(m) => {
                let mtime = m.modified().ok();
                let needs_reload = !self.last_existed
                    || match (self.last_mtime, mtime) {
                        (Some(prev), Some(curr)) => curr > prev,
                        _ => true,
                    };
                if needs_reload {
                    match load_composition_rules(&self.path) {
                        Ok(overlay) => {
                            self.cached = overlay;
                            self.last_mtime = mtime;
                            self.last_existed = true;
                        }
                        Err(e) => {
                            eprintln!(
                                "Warning: failed to reload composition rules from {}: {}",
                                self.path.display(),
                                e
                            );
                        }
                    }
                }
            }
            Err(_) => {
                if self.last_existed {
                    self.cached = CompositionRulesOverlay::default();
                    self.last_existed = false;
                    self.last_mtime = None;
                }
            }
        }
    }
}

/// Default location for the composition rules overlay: `~/.agency/composition-rules.csv`.
pub fn default_overlay_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".agency").join("composition-rules.csv"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parse_full_row() {
        let text = "agent_type,rule,max_role_components,max_desired_outcomes,max_trade_off_configs,all_projects,project_ids\n\
                    assigner,balanced,2,1,1,true,\n";
        let overlay = parse_composition_rules(text).unwrap();
        assert_eq!(overlay.rules.len(), 1);
        let r = &overlay.rules[0];
        assert_eq!(r.agent_type, "assigner");
        assert_eq!(r.max_role_components, Some(2));
        assert!(r.all_projects);
    }

    #[test]
    fn parse_blank_max_means_unlimited() {
        let text = "agent_type,rule,max_role_components,max_desired_outcomes,max_trade_off_configs,all_projects,project_ids\n\
                    evaluator,open,,,,true,\n";
        let overlay = parse_composition_rules(text).unwrap();
        assert_eq!(overlay.rules.len(), 1);
        let r = &overlay.rules[0];
        assert_eq!(r.max_role_components, None);
        assert_eq!(r.max_desired_outcomes, None);
        assert_eq!(r.max_trade_off_configs, None);
    }

    #[test]
    fn parse_project_ids_split_by_semicolon() {
        let text = "agent_type,rule,max_role_components,max_desired_outcomes,max_trade_off_configs,all_projects,project_ids\n\
                    evolver,exploratory,5,2,1,false,proj-a;proj-b;proj-c\n";
        let overlay = parse_composition_rules(text).unwrap();
        assert_eq!(
            overlay.rules[0].project_ids,
            vec!["proj-a", "proj-b", "proj-c"]
        );
        assert!(!overlay.rules[0].all_projects);
    }

    #[test]
    fn missing_file_is_empty_overlay() {
        let overlay = load_composition_rules(Path::new("/nonexistent.csv")).unwrap();
        assert!(overlay.rules.is_empty());
    }

    #[test]
    fn watcher_reloads_after_edit() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("rules.csv");
        std::fs::write(&path, "agent_type,max_role_components\nassigner,5\n").unwrap();
        let mut watcher = CompositionRulesWatcher::new(&path);
        assert_eq!(
            watcher
                .current()
                .rule_for("assigner")
                .unwrap()
                .max_role_components,
            Some(5)
        );

        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&path, "agent_type,max_role_components\nassigner,1\n").unwrap();

        assert_eq!(
            watcher
                .current()
                .rule_for("assigner")
                .unwrap()
                .max_role_components,
            Some(1)
        );
    }
}
