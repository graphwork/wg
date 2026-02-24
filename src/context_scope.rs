//! Context scope for controlling how much context an agent receives in its prompt.
//!
//! Each tier is a strict superset of the one below:
//! - `Clean`: Bare executor — no wg CLI instructions
//! - `Task`: Task-aware — standard default with wg workflow instructions
//! - `Graph`: Graph-aware — adds project description, 1-hop neighborhood, status summary
//! - `Full`: System-aware — adds full graph summary, CLAUDE.md, system preamble

use std::fmt;
use std::str::FromStr;

/// Context scope controlling how much context an agent receives.
///
/// Variants are ordered from least to most context. The derived `PartialOrd`/`Ord`
/// follows variant declaration order, enabling `scope >= ContextScope::Task` checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ContextScope {
    Clean,
    Task,
    Graph,
    Full,
}

impl FromStr for ContextScope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "clean" => Ok(ContextScope::Clean),
            "task" => Ok(ContextScope::Task),
            "graph" => Ok(ContextScope::Graph),
            "full" => Ok(ContextScope::Full),
            _ => Err(format!(
                "Invalid context scope '{}'. Valid values: clean, task, graph, full",
                s
            )),
        }
    }
}

impl fmt::Display for ContextScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContextScope::Clean => write!(f, "clean"),
            ContextScope::Task => write!(f, "task"),
            ContextScope::Graph => write!(f, "graph"),
            ContextScope::Full => write!(f, "full"),
        }
    }
}

/// Resolve the effective context scope using the priority hierarchy:
/// task > role > coordinator config > default ("task").
pub fn resolve_context_scope(
    task_scope: Option<&str>,
    role_scope: Option<&str>,
    config_scope: Option<&str>,
) -> ContextScope {
    task_scope
        .and_then(|s| s.parse().ok())
        .or_else(|| role_scope.and_then(|s| s.parse().ok()))
        .or_else(|| config_scope.and_then(|s| s.parse().ok()))
        .unwrap_or(ContextScope::Task)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_scopes() {
        assert_eq!("clean".parse::<ContextScope>().unwrap(), ContextScope::Clean);
        assert_eq!("task".parse::<ContextScope>().unwrap(), ContextScope::Task);
        assert_eq!("graph".parse::<ContextScope>().unwrap(), ContextScope::Graph);
        assert_eq!("full".parse::<ContextScope>().unwrap(), ContextScope::Full);
    }

    #[test]
    fn test_parse_case_insensitive() {
        assert_eq!("Clean".parse::<ContextScope>().unwrap(), ContextScope::Clean);
        assert_eq!("TASK".parse::<ContextScope>().unwrap(), ContextScope::Task);
        assert_eq!("Graph".parse::<ContextScope>().unwrap(), ContextScope::Graph);
        assert_eq!("FULL".parse::<ContextScope>().unwrap(), ContextScope::Full);
    }

    #[test]
    fn test_parse_invalid() {
        assert!("bogus".parse::<ContextScope>().is_err());
        assert!("".parse::<ContextScope>().is_err());
        assert!("tasks".parse::<ContextScope>().is_err());
    }

    #[test]
    fn test_display() {
        assert_eq!(ContextScope::Clean.to_string(), "clean");
        assert_eq!(ContextScope::Task.to_string(), "task");
        assert_eq!(ContextScope::Graph.to_string(), "graph");
        assert_eq!(ContextScope::Full.to_string(), "full");
    }

    #[test]
    fn test_ordering() {
        assert!(ContextScope::Clean < ContextScope::Task);
        assert!(ContextScope::Task < ContextScope::Graph);
        assert!(ContextScope::Graph < ContextScope::Full);
        assert!(ContextScope::Clean < ContextScope::Full);
    }

    #[test]
    fn test_ordering_comparisons() {
        assert!(ContextScope::Task >= ContextScope::Task);
        assert!(ContextScope::Graph >= ContextScope::Task);
        assert!(ContextScope::Full >= ContextScope::Task);
        assert!(!(ContextScope::Clean >= ContextScope::Task));
    }

    #[test]
    fn test_resolve_task_overrides_all() {
        let scope = resolve_context_scope(Some("graph"), Some("clean"), Some("full"));
        assert_eq!(scope, ContextScope::Graph);
    }

    #[test]
    fn test_resolve_role_overrides_config() {
        let scope = resolve_context_scope(None, Some("graph"), Some("clean"));
        assert_eq!(scope, ContextScope::Graph);
    }

    #[test]
    fn test_resolve_config_used_as_fallback() {
        let scope = resolve_context_scope(None, None, Some("full"));
        assert_eq!(scope, ContextScope::Full);
    }

    #[test]
    fn test_resolve_default_is_task() {
        let scope = resolve_context_scope(None, None, None);
        assert_eq!(scope, ContextScope::Task);
    }

    #[test]
    fn test_resolve_invalid_values_skipped() {
        // Invalid task scope falls through to role
        let scope = resolve_context_scope(Some("bogus"), Some("graph"), None);
        assert_eq!(scope, ContextScope::Graph);

        // All invalid falls through to default
        let scope = resolve_context_scope(Some("bogus"), Some("nope"), Some("bad"));
        assert_eq!(scope, ContextScope::Task);
    }
}
