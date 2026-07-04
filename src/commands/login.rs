//! `wg login` — one-command provider credential setup.

use crate::cli::LoginCommands;
use anyhow::{Context, Result, bail};
use dialoguer::Password;
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, HeaderValue};
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

use worksgood::config::{Config, EndpointConfig};
use worksgood::secret::{self, Backend, SecretsConfig};

const OPENROUTER_ENDPOINT_NAME: &str = "openrouter";
const OPENROUTER_PROVIDER: &str = "openrouter";
const OPENROUTER_ENV_VAR: &str = "OPENROUTER_API_KEY";

pub fn run(workgraph_dir: &Path, command: &LoginCommands) -> Result<()> {
    match command {
        LoginCommands::Openrouter {
            check,
            from_stdin,
            env,
            backend,
            global,
            local,
            set_default,
            reset_endpoint,
        } => {
            let scope = ConfigScope::from_flags(*global, *local)?;
            let options = OpenRouterLoginOptions {
                check: *check,
                from_stdin: *from_stdin,
                env_var: env.clone(),
                backend: backend.clone(),
                scope,
                set_default: *set_default,
                reset_endpoint: *reset_endpoint,
            };
            run_openrouter(workgraph_dir, &options)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigScope {
    Global,
    Local,
}

impl ConfigScope {
    fn from_flags(global: bool, local: bool) -> Result<Self> {
        if global && local {
            bail!("Choose only one of --global or --local");
        }
        if local {
            Ok(Self::Local)
        } else {
            Ok(Self::Global)
        }
    }
}

#[derive(Debug, Clone)]
struct OpenRouterLoginOptions {
    check: bool,
    from_stdin: bool,
    env_var: Option<String>,
    backend: Option<String>,
    scope: ConfigScope,
    set_default: bool,
    reset_endpoint: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum CredentialMode {
    StoredSecret {
        api_key_ref: String,
        backend: Backend,
        secret_name: String,
    },
    EnvRef {
        api_key_ref: String,
        var_name: String,
    },
}

#[derive(Debug, Clone)]
struct CheckReport {
    wg_ref_label: String,
    wg_secret_present: bool,
    endpoint_name: String,
    endpoint_url: String,
    endpoint_default: bool,
    auth_ok: Option<bool>,
    auth_detail: String,
    pi_auth_present: bool,
    pi_auth_path: PathBuf,
}

fn run_openrouter(workgraph_dir: &Path, options: &OpenRouterLoginOptions) -> Result<()> {
    if options.check {
        let report = build_openrouter_check_report(workgraph_dir)?;
        print_check_report(&report);
        if !report.wg_secret_present {
            bail!("WG OpenRouter credential is not configured. Run `wg login openrouter` first.");
        }
        if matches!(report.auth_ok, Some(false)) {
            bail!("WG OpenRouter endpoint failed credential check.");
        }
        return Ok(());
    }

    let cfg = SecretsConfig::load_global();
    let credential_mode = resolve_credential_mode(options, &cfg)?;
    let mut config = load_target_config(workgraph_dir, options.scope)?;
    upsert_openrouter_endpoint(
        &mut config,
        &credential_mode,
        options.set_default,
        options.reset_endpoint,
    )?;
    save_target_config(&config, workgraph_dir, options.scope)?;

    let report = build_openrouter_check_report(workgraph_dir)?;
    print_check_report(&report);
    if !matches!(report.auth_ok, Some(true)) {
        bail!("OpenRouter login saved config, but the credential check did not succeed.");
    }
    Ok(())
}

fn resolve_credential_mode(
    options: &OpenRouterLoginOptions,
    secrets_cfg: &SecretsConfig,
) -> Result<CredentialMode> {
    if let Some(var_name) = options.env_var.as_deref() {
        let value = std::env::var(var_name).with_context(|| {
            format!(
                "Environment variable '{}' is not set. Export it first, then rerun `wg login openrouter --env {}`.",
                var_name, var_name
            )
        })?;
        if value.trim().is_empty() {
            bail!(
                "Environment variable '{}' is set but empty. Refusing to configure an unusable OpenRouter credential.",
                var_name
            );
        }
        return Ok(CredentialMode::EnvRef {
            api_key_ref: format!("env:{var_name}"),
            var_name: var_name.to_string(),
        });
    }

    let backend = if let Some(backend) = options.backend.as_deref() {
        backend.parse::<Backend>()?
    } else {
        secrets_cfg.default_backend.clone()
    };
    let secret_value = read_secret_value(options.from_stdin)?;
    if secret_value.trim().is_empty() {
        bail!("OpenRouter API key cannot be empty");
    }
    secret::set(
        OPENROUTER_ENDPOINT_NAME,
        &secret_value,
        &backend,
        secrets_cfg,
    )?;
    Ok(CredentialMode::StoredSecret {
        api_key_ref: api_key_ref_for_backend(&backend, OPENROUTER_ENDPOINT_NAME),
        backend,
        secret_name: OPENROUTER_ENDPOINT_NAME.to_string(),
    })
}

fn api_key_ref_for_backend(backend: &Backend, name: &str) -> String {
    match backend {
        Backend::Keyring => format!("keyring:{name}"),
        Backend::Keystore => format!("keystore:{name}"),
        Backend::Plaintext => format!("plain:{name}"),
    }
}

fn read_secret_value(from_stdin: bool) -> Result<String> {
    if from_stdin {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("failed to read OpenRouter API key from stdin")?;
        if buf.ends_with('\n') {
            buf.pop();
            if buf.ends_with('\r') {
                buf.pop();
            }
        }
        return Ok(buf);
    }

    if !std::io::stdin().is_terminal() {
        bail!(
            "stdin is not a terminal; use `wg login openrouter --from-stdin` or `wg login openrouter --env {}` for automation.",
            OPENROUTER_ENV_VAR
        );
    }

    Ok(Password::new()
        .with_prompt("OpenRouter API key")
        .interact()
        .unwrap_or_default())
}

fn load_target_config(workgraph_dir: &Path, scope: ConfigScope) -> Result<Config> {
    match scope {
        ConfigScope::Global => Ok(Config::load_global()?.unwrap_or_default()),
        ConfigScope::Local => Config::load(workgraph_dir),
    }
}

fn save_target_config(config: &Config, workgraph_dir: &Path, scope: ConfigScope) -> Result<()> {
    match scope {
        ConfigScope::Global => config.save_global(),
        ConfigScope::Local => config.save(workgraph_dir),
    }
}

fn upsert_openrouter_endpoint(
    config: &mut Config,
    credential_mode: &CredentialMode,
    set_default: bool,
    reset_endpoint: bool,
) -> Result<()> {
    let desired_ref = match credential_mode {
        CredentialMode::StoredSecret { api_key_ref, .. } => api_key_ref,
        CredentialMode::EnvRef { api_key_ref, .. } => api_key_ref,
    };

    let any_default = config
        .llm_endpoints
        .endpoints
        .iter()
        .any(|ep| ep.is_default);
    let endpoint_idx = config
        .llm_endpoints
        .endpoints
        .iter()
        .position(|ep| ep.name == OPENROUTER_ENDPOINT_NAME);

    if let Some(idx) = endpoint_idx {
        let ep = &config.llm_endpoints.endpoints[idx];
        if ep.provider != OPENROUTER_PROVIDER && !reset_endpoint {
            bail!(
                "Existing endpoint '{}' uses provider '{}'. Re-run with --reset-endpoint to repurpose it for OpenRouter.",
                OPENROUTER_ENDPOINT_NAME,
                ep.provider
            );
        }
    }

    if let Some(idx) = endpoint_idx {
        if set_default || !any_default {
            for other in &mut config.llm_endpoints.endpoints {
                other.is_default = false;
            }
        }
        let ep = &mut config.llm_endpoints.endpoints[idx];
        ep.provider = OPENROUTER_PROVIDER.to_string();
        if reset_endpoint || ep.url.is_none() {
            ep.url =
                Some(EndpointConfig::default_url_for_provider(OPENROUTER_PROVIDER).to_string());
        }
        ep.api_key = None;
        ep.api_key_file = None;
        ep.api_key_env = None;
        ep.api_key_ref = Some(desired_ref.clone());
        if set_default || !any_default {
            ep.is_default = true;
        }
    } else {
        if set_default || !any_default {
            for other in &mut config.llm_endpoints.endpoints {
                other.is_default = false;
            }
        }
        config.llm_endpoints.endpoints.push(EndpointConfig {
            name: OPENROUTER_ENDPOINT_NAME.to_string(),
            provider: OPENROUTER_PROVIDER.to_string(),
            url: Some(EndpointConfig::default_url_for_provider(OPENROUTER_PROVIDER).to_string()),
            model: None,
            api_key: None,
            api_key_file: None,
            api_key_env: None,
            api_key_ref: Some(desired_ref.clone()),
            is_default: set_default || !any_default,
            context_window: None,
        });
    }

    Ok(())
}

fn build_openrouter_check_report(workgraph_dir: &Path) -> Result<CheckReport> {
    let config = Config::load_merged(workgraph_dir)?;
    let endpoint = config
        .llm_endpoints
        .find_by_name(OPENROUTER_ENDPOINT_NAME)
        .or_else(|| config.llm_endpoints.find_for_provider(OPENROUTER_PROVIDER))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No OpenRouter endpoint configured. Run `wg login openrouter` to create one."
            )
        })?;

    let wg_ref_label = endpoint
        .api_key_ref
        .clone()
        .or_else(|| endpoint.api_key_env.as_ref().map(|v| format!("env:{v}")))
        .unwrap_or_else(|| endpoint.key_source());

    let resolved_key = endpoint.resolve_api_key(Some(workgraph_dir));
    let (wg_secret_present, auth_ok, auth_detail) = match resolved_key {
        Ok(Some(key)) => {
            let auth = probe_openrouter_endpoint(endpoint, key.as_str())?;
            (
                true,
                Some(auth),
                if auth { "ok" } else { "failed" }.to_string(),
            )
        }
        Ok(None) => (false, None, "missing credential".to_string()),
        Err(err) => (
            false,
            None,
            format!(
                "unreachable credential ({})",
                redact_error(&err.to_string())
            ),
        ),
    };

    let home = dirs::home_dir().context("Cannot determine home directory for Pi auth check")?;
    let pi_auth_path = home.join(".pi").join("agent").join("auth.json");
    let pi_auth_present = pi_openrouter_auth_present(&pi_auth_path);

    Ok(CheckReport {
        wg_ref_label,
        wg_secret_present,
        endpoint_name: endpoint.name.clone(),
        endpoint_url: endpoint.url.clone().unwrap_or_else(|| {
            EndpointConfig::default_url_for_provider(&endpoint.provider).to_string()
        }),
        endpoint_default: endpoint.is_default,
        auth_ok,
        auth_detail,
        pi_auth_present,
        pi_auth_path,
    })
}

fn probe_openrouter_endpoint(endpoint: &EndpointConfig, api_key: &str) -> Result<bool> {
    let base_url = endpoint
        .url
        .as_deref()
        .unwrap_or(EndpointConfig::default_url_for_provider(
            OPENROUTER_PROVIDER,
        ));
    let models_url = format!("{}/models", base_url.trim_end_matches('/'));
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {api_key}"))
            .context("failed to build auth header")?,
    );
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let response = client
        .get(&models_url)
        .headers(headers)
        .send()
        .with_context(|| format!("OpenRouter endpoint check failed for {}", endpoint.name))?;
    Ok(response.status().is_success())
}

fn pi_openrouter_auth_present(path: &Path) -> bool {
    let Ok(body) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) else {
        return false;
    };
    value
        .as_object()
        .is_some_and(|map| map.contains_key(OPENROUTER_PROVIDER))
}

fn redact_error(message: &str) -> String {
    let message = message.replace(OPENROUTER_ENV_VAR, "[redacted-env]");
    if message.contains("sk-") {
        "[redacted-secret]".to_string()
    } else {
        message
    }
}

fn print_check_report(report: &CheckReport) {
    println!("OpenRouter (WG)");
    let secret_status = if report.wg_secret_present {
        "present"
    } else {
        "missing"
    };
    println!("  secret: {} ({})", secret_status, report.wg_ref_label);
    println!(
        "  endpoint: {} -> {}",
        report.endpoint_name, report.endpoint_url
    );
    println!(
        "  default: {}",
        if report.endpoint_default { "yes" } else { "no" }
    );
    match report.auth_ok {
        Some(true) => println!("  auth: ok"),
        Some(false) => println!("  auth: failed"),
        None => println!("  auth: {}", report.auth_detail),
    }
    println!();
    println!("OpenRouter (Pi)");
    if report.pi_auth_present {
        println!("  auth: present in {}", report.pi_auth_path.display());
    } else {
        println!("  auth: not detected");
        println!("  note: `pi:` routes can still work later if you run `/login` inside pi");
    }
    println!();
    println!("Next:");
    println!("  wg login openrouter --check");
    println!("  wg models fetch --no-cache");
    println!("  wg model-scout --no-cache");
    println!("  wg profile pi");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    static HOME_MUTEX: Mutex<()> = Mutex::new(());

    fn with_home<T>(f: impl FnOnce(&Path) -> T) -> T {
        let _guard = HOME_MUTEX.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().to_path_buf();
        std::fs::create_dir_all(home.join(".wg")).unwrap();
        unsafe { std::env::set_var("HOME", &home) };
        f(&home)
    }

    #[test]
    fn backend_selection_uses_default_backend() {
        let cfg = SecretsConfig {
            allow_plaintext: false,
            default_backend: Backend::Keystore,
        };
        let options = OpenRouterLoginOptions {
            check: false,
            from_stdin: false,
            env_var: Some("WG_TEST_OPENROUTER_KEY".into()),
            backend: None,
            scope: ConfigScope::Global,
            set_default: false,
            reset_endpoint: false,
        };
        unsafe { std::env::set_var("WG_TEST_OPENROUTER_KEY", "sk-or-test") };
        let env_mode = resolve_credential_mode(&options, &cfg).unwrap();
        assert_eq!(
            env_mode,
            CredentialMode::EnvRef {
                api_key_ref: "env:WG_TEST_OPENROUTER_KEY".into(),
                var_name: "WG_TEST_OPENROUTER_KEY".into()
            }
        );
        unsafe { std::env::remove_var("WG_TEST_OPENROUTER_KEY") };
        assert_eq!(
            api_key_ref_for_backend(&Backend::Keystore, "openrouter"),
            "keystore:openrouter"
        );
    }

    #[test]
    fn upsert_openrouter_endpoint_is_idempotent() {
        let mut cfg = Config::default();
        let mode = CredentialMode::StoredSecret {
            api_key_ref: "keystore:openrouter".into(),
            backend: Backend::Keystore,
            secret_name: "openrouter".into(),
        };

        upsert_openrouter_endpoint(&mut cfg, &mode, false, false).unwrap();
        upsert_openrouter_endpoint(&mut cfg, &mode, false, false).unwrap();

        assert_eq!(cfg.llm_endpoints.endpoints.len(), 1);
        let ep = &cfg.llm_endpoints.endpoints[0];
        assert_eq!(ep.name, OPENROUTER_ENDPOINT_NAME);
        assert_eq!(ep.provider, OPENROUTER_PROVIDER);
        assert_eq!(ep.api_key_ref.as_deref(), Some("keystore:openrouter"));
        assert!(ep.api_key.is_none());
        assert!(ep.api_key_file.is_none());
        assert!(ep.api_key_env.is_none());
    }

    #[test]
    fn upsert_openrouter_endpoint_patches_existing_url_only_when_needed() {
        let mut cfg = Config::default();
        cfg.llm_endpoints.endpoints.push(EndpointConfig {
            name: OPENROUTER_ENDPOINT_NAME.into(),
            provider: OPENROUTER_PROVIDER.into(),
            url: Some("http://127.0.0.1:9999/v1".into()),
            model: Some("x".into()),
            api_key: Some("sk-inline".into()),
            api_key_file: None,
            api_key_env: Some("OLD".into()),
            api_key_ref: None,
            is_default: false,
            context_window: None,
        });

        let mode = CredentialMode::StoredSecret {
            api_key_ref: "keyring:openrouter".into(),
            backend: Backend::Keyring,
            secret_name: "openrouter".into(),
        };
        upsert_openrouter_endpoint(&mut cfg, &mode, false, false).unwrap();
        let ep = &cfg.llm_endpoints.endpoints[0];
        assert_eq!(ep.url.as_deref(), Some("http://127.0.0.1:9999/v1"));
        assert_eq!(ep.api_key_ref.as_deref(), Some("keyring:openrouter"));
        assert!(ep.api_key.is_none());
        assert!(ep.api_key_env.is_none());

        upsert_openrouter_endpoint(&mut cfg, &mode, false, true).unwrap();
        let ep = &cfg.llm_endpoints.endpoints[0];
        assert_eq!(
            ep.url.as_deref(),
            Some(EndpointConfig::default_url_for_provider(
                OPENROUTER_PROVIDER
            ))
        );
    }

    #[test]
    fn redact_error_never_echoes_key_material() {
        let redacted = redact_error("request failed with Authorization: Bearer sk-or-secret-123");
        assert!(!redacted.contains("sk-or-secret-123"));
    }

    #[test]
    fn pi_auth_detection_checks_redacted_file_presence_only() {
        with_home(|home| {
            let auth_dir = home.join(".pi").join("agent");
            std::fs::create_dir_all(&auth_dir).unwrap();
            std::fs::write(
                auth_dir.join("auth.json"),
                r#"{"openrouter":{"type":"apiKey","apiKey":"redacted"}}"#,
            )
            .unwrap();
            assert!(pi_openrouter_auth_present(&auth_dir.join("auth.json")));
        });
    }
}
