//! Per-task profile resolution for dispatch.
//!
//! `wg publish <id> --profile <name>` stamps a named profile onto every task
//! in a weakly-connected component (see `commands::resume`). At dispatch time,
//! a stamped task must route through that profile's `(executor, model,
//! endpoint)` instead of the globally-active profile.
//!
//! A named profile file is a *complete* `Config` snapshot
//! (`profile::named` doc). So the cleanest injection is "pick which `Config`
//! to hand `plan_spawn`": when `task.profile` is set, hand it the profile's
//! config; otherwise hand it the global config unchanged. `plan_spawn`'s
//! existing executor/model/endpoint cascade then transparently honors the
//! profile's `coordinator.*`, `[models.*]`, and `[llm_endpoints]`.
//!
//! Backward compatibility: `task.profile == None` ⇒ the global config is
//! returned unchanged, so behavior is identical to today.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::config::Config;
use crate::graph::Task;

/// Memoizes loaded profile configs by name within a single dispatcher tick so
/// a component of N tasks loads + parses each profile file at most once.
#[derive(Default)]
pub struct ProfileCache {
    /// `name -> Some(config)` on success, `name -> None` if the profile could
    /// not be loaded (missing/parse error) — cached either way so we don't
    /// re-attempt a failing load for every task in the component.
    loaded: HashMap<String, Option<Config>>,
}

impl ProfileCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load + cache the config for `name`. Returns `None` if the profile is
    /// missing or fails to parse — the caller falls back to the global config.
    fn get(&mut self, name: &str) -> Option<&Config> {
        self.loaded
            .entry(name.to_string())
            .or_insert_with(|| load_profile_config(name))
            .as_ref()
    }
}

/// Load a named profile's complete `Config` snapshot, or `None` if it cannot
/// be loaded. Errors are intentionally swallowed into `None`: a profile that
/// was deleted after stamping must degrade to the global config at dispatch
/// (surfaced via spawn provenance), never crash the spawn.
pub fn load_profile_config(name: &str) -> Option<Config> {
    match crate::profile::named::load(name) {
        Ok(p) => Some(p.config),
        Err(e) => {
            eprintln!(
                "[dispatch-profile] profile '{}' could not be loaded ({}); \
                 falling back to global config",
                name, e
            );
            None
        }
    }
}

/// Return the effective `Config` for a task. When `task.profile` is set and
/// loads, returns that profile's config (owned); otherwise returns the global
/// config borrowed unchanged.
pub fn effective_config_for_task<'a>(
    task: &Task,
    global: &'a Config,
    cache: &mut ProfileCache,
) -> Cow<'a, Config> {
    match task.profile.as_deref() {
        Some(name) => match cache.get(name) {
            Some(cfg) => Cow::Owned(cfg.clone()),
            None => Cow::Borrowed(global),
        },
        None => Cow::Borrowed(global),
    }
}

/// Resolve the effective config for an explicit profile name without a cache —
/// for one-shot call sites (`wg evaluate run`, `wg assign`) that resolve a
/// single task's profile. `None`/unloadable ⇒ returns `global` unchanged.
pub fn effective_config_owned(profile: Option<&str>, global: Config) -> Config {
    match profile {
        Some(name) => load_profile_config(name).unwrap_or(global),
        None => global,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn global_config() -> Config {
        let mut c = Config::default();
        c.agent.model = "global-model".to_string();
        c
    }

    #[test]
    fn no_profile_returns_global_borrowed() {
        let global = global_config();
        let task = Task::default();
        let mut cache = ProfileCache::new();
        let eff = effective_config_for_task(&task, &global, &mut cache);
        assert!(matches!(eff, Cow::Borrowed(_)));
        assert_eq!(eff.agent.model, "global-model");
    }

    #[test]
    fn missing_profile_falls_back_to_global() {
        let global = global_config();
        let mut task = Task::default();
        task.profile = Some("definitely-not-a-real-profile-xyz".to_string());
        let mut cache = ProfileCache::new();
        let eff = effective_config_for_task(&task, &global, &mut cache);
        // Unloadable profile ⇒ global config, unchanged.
        assert_eq!(eff.agent.model, "global-model");
    }

    #[test]
    fn effective_config_owned_no_profile_is_identity() {
        let global = global_config();
        let out = effective_config_owned(None, global);
        assert_eq!(out.agent.model, "global-model");
    }

    // The following tests repoint `WG_GLOBAL_DIR` at a fresh tempdir so
    // `profile::named::load` reads our test profile, never the real
    // `~/.wg/profiles`. They are `#[serial]` because the env var is process-
    // global.
    use serial_test::serial;

    /// RAII guard: set `WG_GLOBAL_DIR` to a path and restore on drop.
    struct GlobalDirGuard {
        saved: Option<String>,
    }
    impl GlobalDirGuard {
        fn set(path: &std::path::Path) -> Self {
            let saved = std::env::var("WG_GLOBAL_DIR").ok();
            unsafe { std::env::set_var("WG_GLOBAL_DIR", path) };
            Self { saved }
        }
    }
    impl Drop for GlobalDirGuard {
        fn drop(&mut self) {
            unsafe {
                match self.saved.take() {
                    Some(v) => std::env::set_var("WG_GLOBAL_DIR", v),
                    None => std::env::remove_var("WG_GLOBAL_DIR"),
                }
            }
        }
    }

    fn write_profile(global_dir: &std::path::Path, name: &str, toml: &str) {
        let profiles = global_dir.join("profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::write(profiles.join(format!("{name}.toml")), toml).unwrap();
    }

    /// A work task stamped with a profile resolves its `{executor, model}`
    /// through THAT profile, not the global config.
    #[test]
    #[serial]
    fn work_task_resolves_through_pinned_profile() {
        let global_dir = tempfile::tempdir().unwrap();
        let _g = GlobalDirGuard::set(global_dir.path());
        write_profile(
            global_dir.path(),
            "creditburn",
            "[coordinator]\nmodel = \"codex:gpt-5.5\"\n",
        );

        let mut task = Task::default();
        task.id = "work".to_string();
        task.profile = Some("creditburn".to_string());

        let global = Config::default();
        let mut cache = ProfileCache::new();
        let eff = effective_config_for_task(&task, &global, &mut cache);

        // The effective config IS the profile snapshot (global is untouched).
        assert_eq!(eff.coordinator.model.as_deref(), Some("codex:gpt-5.5"));
        assert_eq!(global.coordinator.model, None);

        // And `plan_spawn` (the single dispatch resolver) routes the work task
        // to the profile's executor — codex, not the default claude.
        let model = eff
            .resolve_model_for_role(crate::config::DispatchRole::TaskAgent)
            .spawn_model_spec();
        let plan = crate::dispatch::plan_spawn(&task, eff.as_ref(), None, Some(&model)).unwrap();
        assert_eq!(plan.executor, crate::dispatch::ExecutorKind::Codex);
    }

    /// A task's agency satellites resolve the evaluator role through the WCC
    /// profile — overriding the default `claude:haiku` agency pin. This is the
    /// exact resolution `wg evaluate run` performs via `resolve_agency_dispatch`.
    #[test]
    #[serial]
    fn agency_evaluator_resolves_through_pinned_profile() {
        let global_dir = tempfile::tempdir().unwrap();
        let _g = GlobalDirGuard::set(global_dir.path());
        write_profile(
            global_dir.path(),
            "creditburn",
            "[models.evaluator]\nmodel = \"codex:gpt-5.5-mini\"\n",
        );

        // Baseline: with no profile, the evaluator resolves to the default weak
        // tier (claude:haiku) on the claude CLI handler.
        let global = Config::default();
        let base = crate::service::llm::resolve_agency_dispatch(
            &global,
            crate::config::DispatchRole::Evaluator,
        );
        assert_eq!(base.raw_spec, "claude:haiku");
        assert_eq!(base.handler, crate::dispatch::ExecutorKind::Claude);

        // With the profile, the evaluator role resolves to the profile's model.
        let eff = effective_config_owned(Some("creditburn"), Config::default());
        let dispatch = crate::service::llm::resolve_agency_dispatch(
            &eff,
            crate::config::DispatchRole::Evaluator,
        );
        assert_eq!(dispatch.raw_spec, "codex:gpt-5.5-mini");
        assert_ne!(dispatch.raw_spec, "claude:haiku");
        assert_eq!(dispatch.handler, crate::dispatch::ExecutorKind::Codex);
    }
}
