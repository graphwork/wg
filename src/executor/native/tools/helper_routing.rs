//! Shared model/endpoint routing for native helper LLM calls.
//!
//! Helper tools such as `summarize` and `delegate` make their own LLM calls
//! inside an already-running nex/native session. They must inherit that
//! session route unless the user explicitly configured a helper override.

use std::path::Path;

use anyhow::Result;

use crate::config::{Config, DispatchRole};
use crate::executor::native::provider::{Provider, create_provider_ext};

/// Routing context inherited from the parent native/nex session.
///
/// The API key is intentionally not `Debug` and never appears in labels.
#[derive(Clone, Default)]
pub struct HelperRouting {
    active_model: Option<String>,
    provider: Option<String>,
    endpoint: Option<String>,
    api_key: Option<String>,
}

/// Resolved helper route with the secret-bearing fields kept private.
pub struct ResolvedHelperRouting {
    pub model: String,
    pub provider: Option<String>,
    pub endpoint: Option<String>,
    api_key: Option<String>,
}

impl HelperRouting {
    pub fn new(
        active_model: Option<&str>,
        provider: Option<&str>,
        endpoint: Option<&str>,
        api_key: Option<&str>,
    ) -> Self {
        Self {
            active_model: non_empty(active_model),
            provider: non_empty(provider),
            endpoint: non_empty(endpoint),
            api_key: non_empty(api_key),
        }
    }

    /// Build a routing context for a parent session when only the active
    /// model is known.
    pub fn from_active_model(model: &str) -> Self {
        Self::new(Some(model), None, None, None)
    }

    /// Resolve the helper route. `configured_model` is an explicit helper
    /// override such as `[native_executor.delegate].delegate_model`; an empty
    /// string means "inherit from the parent session".
    pub fn resolve(&self, workgraph_dir: &Path, configured_model: &str) -> ResolvedHelperRouting {
        let model = non_empty(Some(configured_model))
            .or_else(|| self.active_model.clone())
            .or_else(|| env_non_empty("WG_MODEL"))
            .unwrap_or_else(|| {
                Config::load_or_default(workgraph_dir)
                    .resolve_model_for_role(DispatchRole::TaskAgent)
                    .model
            });

        let provider = self
            .provider
            .clone()
            .or_else(|| env_non_empty("WG_LLM_PROVIDER"));

        let endpoint = self
            .endpoint
            .clone()
            .or_else(|| env_non_empty("WG_ENDPOINT"))
            .or_else(|| env_non_empty("WG_ENDPOINT_NAME"))
            .or_else(|| env_non_empty("WG_ENDPOINT_URL"));

        let api_key = self.api_key.clone().or_else(|| env_non_empty("WG_API_KEY"));

        ResolvedHelperRouting {
            model,
            provider,
            endpoint,
            api_key,
        }
    }
}

impl ResolvedHelperRouting {
    pub fn create_provider(&self, workgraph_dir: &Path) -> Result<Box<dyn Provider>> {
        create_provider_ext(
            workgraph_dir,
            &self.model,
            self.provider.as_deref(),
            self.endpoint.as_deref(),
            self.api_key.as_deref(),
        )
    }

    pub fn label(&self) -> String {
        format!(
            "provider={}, model={}, endpoint={}",
            self.provider.as_deref().unwrap_or("auto"),
            self.model,
            self.endpoint
                .as_deref()
                .map(sanitize_endpoint_label)
                .unwrap_or_else(|| "auto".to_string()),
        )
    }
}

/// User-facing route label for a concrete provider. Includes provider/model
/// and endpoint identity, never API key material.
pub fn provider_route_label(provider: &dyn Provider) -> String {
    format!(
        "provider={}, model={}, endpoint={}",
        provider.name(),
        provider.model(),
        provider
            .endpoint_name()
            .map(sanitize_endpoint_label)
            .unwrap_or_else(|| "default".to_string())
    )
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn sanitize_endpoint_label(value: &str) -> String {
    let no_query = value.split(['?', '#']).next().unwrap_or(value);
    let Some((scheme, rest)) = no_query.split_once("://") else {
        return no_query.to_string();
    };
    let Some((userinfo, host_path)) = rest.split_once('@') else {
        return no_query.to_string();
    };
    if userinfo.is_empty() {
        no_query.to_string()
    } else {
        format!("{}://[redacted]@{}", scheme, host_path)
    }
}
