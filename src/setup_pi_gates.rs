//! The concierge Pi-readiness gates in front of attended interactive `wg setup`.
//!
//! WG is embedded in the Pi model plane: Pi owns login, providers, and
//! models, and WG only ever stores the exact routes a user types. A
//! brand-new user running attended interactive `wg setup` must therefore be
//! walked through Pi readiness **first**, in three fail-clean gates, before
//! any prompt is shown and before any byte is written:
//!
//! 1. **PI DETECTED?** — no `pi` executable on PATH ⇒ print the exact
//!    install command and exit cleanly. No partial state.
//! 2. **PROVIDER AUTHENTICATED?** — run Pi's own credential readiness check
//!    (`pi auth check --provider <P> --json --no-refresh`) across the
//!    providers Pi exposes; none ready ⇒ print the guided `/login`
//!    instruction and exit cleanly for the user to return. Pi owns the
//!    OAuth flow; WG never sees keys.
//! 3. **MODEL RESOLVABLE?** — once authenticated, verify at least one model
//!    resolves via Pi's offline registry (the existing bounded catalog
//!    preflight; never a live provider call).
//!
//! Every gate is fail-clean: the exact next command is printed and `wg
//! setup` exits with no config write, no package install, and no WG-side
//! credential storage. The pure gate matrix lives here so it can be pinned
//! by unit tests with **injectable probe results** (also bridged to the
//! smoke harness via [`PI_GATE_PROBES_ENV`]); the live probe collection is a
//! thin, bounded wrapper around existing discovery + catalog preflight.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::PathBuf;

/// Environment variable that fully injects the gate probe results.
///
/// Mirrors the existing `WORKSGOOD_PI_MODELS_JSON` mock convention: when set,
/// no `pi` process is spawned and the JSON is used verbatim as the probe
/// results. This is the sanctioned harness bridge the smoke scenarios use to
/// stay credential-free and hermetic, and it keeps already-green hosts from
/// leaking live auth state into fixtures that isolate `$HOME`.
pub const PI_GATE_PROBES_ENV: &str = "WORKSGOOD_PI_GATE_JSON";

/// The exact install command printed by the pi-missing gate.
pub const PI_INSTALL_COMMAND: &str = "npm install -g @earendil-works/pi-coding-agent";

/// The Node prerequisite printed alongside the install command.
pub const PI_NODE_PREREQUISITE: &str = "Node 22.19+ is required.";

/// What a user types inside Pi to authenticate a provider (the guided hint).
pub const PI_LOGIN_COMMAND: &str = "/login <provider>";

/// An example provider for the guided login hint.
pub const PI_LOGIN_EXAMPLE: &str = "openrouter";

/// The clean-exit instruction every stop gate ends with.
pub const RERUN_INSTRUCTION: &str = "Rerun `worksgood setup` after installing Pi.";

/// Injectable probe results — the complete input to the gate matrix.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiGateProbes {
    /// `pi` executable found on PATH (gate 1).
    #[serde(default)]
    pub pi_present: bool,
    /// Providers Pi exposes (derived from Pi's offline model registry).
    #[serde(default)]
    pub providers: Vec<String>,
    /// Providers whose `pi auth check` reported ready (gate 2).
    #[serde(default)]
    pub authenticated: Vec<String>,
    /// Model ids resolvable via Pi's offline registry (gate 3).
    #[serde(default)]
    pub models: Vec<String>,
}

impl PiGateProbes {
    /// An all-green probe set (used by tests and the smoke harness bridge).
    pub fn all_green() -> Self {
        Self {
            pi_present: true,
            providers: vec![PI_LOGIN_EXAMPLE.to_string()],
            authenticated: vec![PI_LOGIN_EXAMPLE.to_string()],
            models: vec![format!("pi:{PI_LOGIN_EXAMPLE}:z-ai/glm-5.2")],
        }
    }
}

/// The verdict of the gate matrix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PiGateVerdict {
    /// All three gates passed; attended setup may proceed to the prompts.
    Proceed,
    /// Gate 1: no `pi` executable on PATH.
    PiMissing,
    /// Gate 2: Pi is present but no provider is authenticated.
    NotAuthenticated { providers: Vec<String> },
    /// Gate 3: authenticated, but no model resolves in Pi's registry.
    NoResolvableModel,
}

impl PiGateVerdict {
    /// `true` when the gate matrix stops setup cleanly.
    pub fn is_stop(&self) -> bool {
        !matches!(self, PiGateVerdict::Proceed)
    }
}

/// Evaluate the three concierge gates against injectable probe results.
///
/// The order is load-bearing: detection before auth before model
/// resolvability, each fail-clean.
pub fn evaluate_pi_gates(probes: &PiGateProbes) -> PiGateVerdict {
    if !probes.pi_present {
        return PiGateVerdict::PiMissing;
    }
    if probes.authenticated.is_empty() {
        return PiGateVerdict::NotAuthenticated {
            providers: probes.providers.clone(),
        };
    }
    if probes.models.is_empty() {
        return PiGateVerdict::NoResolvableModel;
    }
    PiGateVerdict::Proceed
}

/// Render the gate's user-facing message (the exact next command included).
///
/// Every stop message ends with the rerun instruction and an explicit
/// nothing-was-changed line, matching the ATTENDED_TTY fail-clean style.
pub fn render_gate_message(verdict: &PiGateVerdict) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(String::new());
    match verdict {
        PiGateVerdict::Proceed => {
            // All gates passed; there is no stop message to render.
            return Vec::new();
        }
        PiGateVerdict::PiMissing => {
            lines.push("Pi is required for guided setup: WG configures orchestration;".to_string());
            lines.push("Pi owns login, providers, and models.".to_string());
            lines.push(String::new());
            lines.push("The `pi` executable was not found on PATH. Install it with:".to_string());
            lines.push(format!("  {PI_INSTALL_COMMAND}"));
            lines.push(format!("({PI_NODE_PREREQUISITE})"));
            lines.push(String::new());
            lines.push("No configuration was written and nothing was changed.".to_string());
            lines.push(RERUN_INSTRUCTION.to_string());
        }
        PiGateVerdict::NotAuthenticated { providers } => {
            lines.push("Pi is installed, but no provider is authenticated yet.".to_string());
            if providers.is_empty() {
                lines.push(format!(
                    "To authenticate: run `pi`, then type `{PI_LOGIN_COMMAND}` \
                     (for example `{PI_LOGIN_EXAMPLE}`)."
                ));
            } else {
                lines.push(format!("Providers Pi exposes: {}.", providers.join(", ")));
                lines.push(format!(
                    "To authenticate one: run `pi`, then type `{PI_LOGIN_COMMAND}` \
                     (for example `/login {}`).",
                    providers
                        .first()
                        .map(String::as_str)
                        .unwrap_or(PI_LOGIN_EXAMPLE)
                ));
            }
            lines.push("Pi owns the OAuth flow; WG never sees or stores your keys.".to_string());
            lines.push(String::new());
            lines.push("No configuration was written and nothing was changed.".to_string());
            lines.push("Rerun `worksgood setup` after logging in.".to_string());
        }
        PiGateVerdict::NoResolvableModel => {
            lines.push(
                "Pi is installed and a provider is authenticated, but no model resolves in \
                 Pi's model registry."
                    .to_string(),
            );
            lines.push(
                "Check `pi --list-models` and refresh the catalog with `pi update`.".to_string(),
            );
            lines.push(String::new());
            lines.push("No configuration was written and nothing was changed.".to_string());
            lines.push("Rerun `worksgood setup` once at least one model resolves.".to_string());
        }
    }
    lines
}

/// Render the one-line readiness summary printed when all gates pass.
pub fn render_gate_summary(probes: &PiGateProbes) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Pi readiness: OK ({} authenticated provider(s): {}; {} model(s) resolvable).",
        probes.authenticated.len(),
        probes.authenticated.join(", "),
        probes.models.len()
    );
    out
}

/// Parse the injectable probe results from the harness-bridge JSON.
pub fn probes_from_json(raw: &str) -> Result<PiGateProbes> {
    let probes: PiGateProbes = serde_json::from_str(raw).map_err(|error| {
        anyhow::anyhow!("{PI_GATE_PROBES_ENV} is not valid gate probe JSON: {error}")
    })?;
    Ok(probes)
}

/// Collect live probe results for the gate matrix (the thin impure wrapper).
///
/// Bounded and offline by construction: provider/model discovery reuses the
/// existing bounded catalog preflight ([`crate::concierge::pi_available_models`],
/// never a live provider call) and the auth probe is Pi's own
/// `pi auth check --provider <P> --json --no-refresh`, which only reads
/// locally stored credentials — refresh (and therefore any OAuth flow) is
/// explicitly disabled. A probe failure degrades to "not ready" so the gate
/// fails closed.
pub fn collect_pi_gate_probes(allow_process: bool) -> Result<PiGateProbes> {
    if let Ok(raw) = std::env::var(PI_GATE_PROBES_ENV) {
        return probes_from_json(&raw);
    }
    let pi_binary = crate::executor_discovery::discover()
        .into_iter()
        .find(|executor| executor.name == "pi" && executor.available)
        .and_then(|executor| executor.binary_path);
    let Some(pi_binary) = pi_binary else {
        return Ok(PiGateProbes::default());
    };
    let models = crate::concierge::pi_available_models(allow_process).unwrap_or_default();
    let providers: Vec<String> = models
        .iter()
        .map(|model| model.provider.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let authenticated = providers
        .iter()
        .filter(|provider| auth_check_ready(&pi_binary, provider))
        .cloned()
        .collect();
    let model_ids = models
        .iter()
        .map(|model| route_for(&model.provider, &model.id))
        .collect();
    Ok(PiGateProbes {
        pi_present: true,
        providers,
        authenticated,
        models: model_ids,
    })
}

fn route_for(provider: &str, model: &str) -> String {
    format!("pi:{provider}:{model}")
}

/// Run Pi's own credential readiness check for one provider.
///
/// `--no-refresh` keeps the probe non-interactive: an expired OAuth
/// credential is reported not-ready instead of opening a browser mid-setup.
fn auth_check_ready(pi_binary: &PathBuf, provider: &str) -> bool {
    let Ok(output) = std::process::Command::new(pi_binary)
        .args([
            "auth",
            "check",
            "--provider",
            provider,
            "--json",
            "--no-refresh",
        ])
        .stdin(std::process::Stdio::null())
        .output()
    else {
        return false;
    };
    serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .ok()
        .and_then(|value| {
            value
                .get("status")
                .and_then(|status| status.as_str())
                .map(|status| status == "ready")
        })
        .unwrap_or(false)
}

/// Run the concierge gates for attended interactive setup.
///
/// Returns `Ok(None)` when all gates pass (setup continues); `Ok(Some(lines))`
/// is the fail-clean stop message to print before exiting with no state
/// change. When the probes are injected via [`PI_GATE_PROBES_ENV`] no `pi`
/// process is spawned at all.
pub fn run_pi_readiness_gates(allow_process: bool) -> Result<Option<Vec<String>>> {
    let probes = collect_pi_gate_probes(allow_process)?;
    Ok(match evaluate_pi_gates(&probes) {
        PiGateVerdict::Proceed => {
            println!("{}", render_gate_summary(&probes).trim_end());
            None
        }
        verdict => Some(render_gate_message(&verdict)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_missing_is_the_first_gate_and_names_the_install_command() {
        let probes = PiGateProbes {
            pi_present: false,
            ..PiGateProbes::all_green()
        };
        let verdict = evaluate_pi_gates(&probes);
        assert_eq!(verdict, PiGateVerdict::PiMissing);
        assert!(verdict.is_stop());

        let text = render_gate_message(&verdict).join("\n");
        assert!(text.contains(PI_INSTALL_COMMAND), "{text}");
        assert_eq!(
            PI_INSTALL_COMMAND,
            "npm install -g @earendil-works/pi-coding-agent"
        );
        assert!(text.contains(PI_NODE_PREREQUISITE), "{text}");
        assert!(text.contains("Node 22.19+"), "{text}");
        assert!(
            text.contains("Rerun `worksgood setup` after installing Pi"),
            "{text}"
        );
        assert!(text.contains("No configuration was written"), "{text}");
    }

    #[test]
    fn pi_present_but_no_auth_stops_with_the_guided_login_instruction() {
        let probes = PiGateProbes {
            pi_present: true,
            providers: vec!["openrouter".into(), "openai-codex".into()],
            authenticated: vec![],
            models: vec!["pi:openrouter:z-ai/glm-5.2".into()],
        };
        let verdict = evaluate_pi_gates(&probes);
        assert_eq!(
            verdict,
            PiGateVerdict::NotAuthenticated {
                providers: vec!["openrouter".into(), "openai-codex".into()]
            }
        );

        let text = render_gate_message(&verdict).join("\n");
        assert!(text.contains("no provider is authenticated"), "{text}");
        assert!(text.contains("/login <provider>"), "{text}");
        assert!(text.contains("/login openrouter"), "{text}");
        assert!(text.contains("Pi owns the OAuth flow"), "{text}");
        assert!(text.contains("WG never sees or stores your keys"), "{text}");
        assert!(
            text.contains("Rerun `worksgood setup` after logging in"),
            "{text}"
        );
        assert!(text.contains("No configuration was written"), "{text}");
    }

    #[test]
    fn no_auth_with_no_catalog_still_falls_to_the_auth_gate() {
        // A broken/empty catalog must not skip the auth gate: detection →
        // auth → model, each fail-closed.
        let probes = PiGateProbes {
            pi_present: true,
            providers: vec![],
            authenticated: vec![],
            models: vec![],
        };
        let verdict = evaluate_pi_gates(&probes);
        assert!(matches!(verdict, PiGateVerdict::NotAuthenticated { .. }));
    }

    #[test]
    fn authenticated_but_no_resolvable_model_is_the_third_gate() {
        let probes = PiGateProbes {
            pi_present: true,
            providers: vec!["openrouter".into()],
            authenticated: vec!["openrouter".into()],
            models: vec![],
        };
        let verdict = evaluate_pi_gates(&probes);
        assert_eq!(verdict, PiGateVerdict::NoResolvableModel);

        let text = render_gate_message(&verdict).join("\n");
        assert!(text.contains("no model resolves"), "{text}");
        assert!(text.contains("pi --list-models"), "{text}");
        assert!(
            text.contains("Rerun `worksgood setup` once at least one model resolves"),
            "{text}"
        );
        assert!(text.contains("No configuration was written"), "{text}");
    }

    #[test]
    fn all_green_proceeds_to_the_prompts() {
        let probes = PiGateProbes::all_green();
        assert_eq!(evaluate_pi_gates(&probes), PiGateVerdict::Proceed);
        assert!(!evaluate_pi_gates(&probes).is_stop());

        let summary = render_gate_summary(&probes);
        assert!(summary.contains("Pi readiness: OK"), "{summary}");
        assert!(summary.contains("openrouter"), "{summary}");
        assert!(summary.contains("1 model(s) resolvable"), "{summary}");
    }

    #[test]
    fn injectable_probe_results_bridge_parses_the_harness_json() {
        let raw = r#"{"pi_present":true,"providers":["openrouter"],"authenticated":["openrouter"],"models":["pi:openrouter:z-ai/glm-5.2"]}"#;
        let probes = probes_from_json(raw).unwrap();
        assert_eq!(probes, PiGateProbes::all_green());
        assert_eq!(evaluate_pi_gates(&probes), PiGateVerdict::Proceed);

        let raw = r#"{"pi_present":true,"providers":[],"authenticated":[],"models":[]}"#;
        let probes = probes_from_json(raw).unwrap();
        assert!(matches!(
            evaluate_pi_gates(&probes),
            PiGateVerdict::NotAuthenticated { .. }
        ));

        let error = probes_from_json("not json").unwrap_err();
        assert!(error.to_string().contains(PI_GATE_PROBES_ENV), "{error}");
    }
}
