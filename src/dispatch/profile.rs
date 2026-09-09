//! Legacy per-task profile stamps at the project execution boundary.
//!
//! Reusable profile definitions are machine-global *apply-time* inputs. A task
//! may still carry the legacy `profile` field for migration/provenance, but
//! dispatch must never reopen `~/.wg/profiles/<name>.toml`: doing so would let
//! machine state replace the authoritative project configuration at runtime.
//! Exact task/command model fields are resolved separately at their documented
//! higher precedence.

use std::borrow::Cow;

use crate::config::Config;
use crate::graph::Task;

/// Compatibility placeholder retained so dispatcher call sites do not need a
/// broad signature change during the one-release legacy-read window.
#[derive(Default)]
pub struct ProfileCache;

impl ProfileCache {
    pub fn new() -> Self {
        Self
    }
}

/// Return the already-resolved project configuration. A legacy task profile
/// stamp is inert at runtime and cannot import routes or non-routing settings
/// from a machine-global reusable definition.
pub fn effective_config_for_task<'a>(
    _task: &Task,
    project: &'a Config,
    _cache: &mut ProfileCache,
) -> Cow<'a, Config> {
    Cow::Borrowed(project)
}

/// One-shot counterpart to [`effective_config_for_task`]. The profile name is
/// migration metadata only; the supplied project config remains authoritative.
pub fn effective_config_owned(_profile: Option<&str>, project: Config) -> Config {
    project
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_config() -> Config {
        let mut config = Config::default();
        config.agent.model = "pi:project:worker".to_string();
        config
    }

    #[test]
    fn no_profile_returns_project_borrowed() {
        let project = project_config();
        let task = Task::default();
        let mut cache = ProfileCache::new();
        let effective = effective_config_for_task(&task, &project, &mut cache);
        assert!(matches!(effective, Cow::Borrowed(_)));
        assert_eq!(effective.agent.model, "pi:project:worker");
    }

    #[test]
    fn legacy_task_profile_stamp_cannot_replace_project_config() {
        let project = project_config();
        let mut task = Task::default();
        task.profile = Some("machine-global-profile".to_string());
        let mut cache = ProfileCache::new();
        let effective = effective_config_for_task(&task, &project, &mut cache);
        assert!(matches!(effective, Cow::Borrowed(_)));
        assert_eq!(effective.agent.model, "pi:project:worker");
    }

    #[test]
    fn one_shot_profile_name_cannot_replace_project_config() {
        let effective = effective_config_owned(Some("machine-global-profile"), project_config());
        assert_eq!(effective.agent.model, "pi:project:worker");
    }
}
