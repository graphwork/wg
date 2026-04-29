//! Secure credential storage for API keys.
//!
//! Resolution order for `api_key_ref` URIs (first hit wins):
//! 1. `literal:<value>` — inline value; warns loudly, test use only
//! 2. `op://<path>` / `pass:<path>` — delegates to external tool (1Password, pass)
//! 3. `keyring:<name>` — secure file keystore at `~/.wg/keystore/<name>` (0600, always on)
//! 4. `env:<VAR>` — explicit, opt-in env forwarding
//! 5. `plain:<name>` — plaintext file at `~/.wg/secrets/<name>` (requires allow_plaintext=true)
//!
//! The "keyring" backend is a dedicated secure file store separate from the
//! plaintext backend. On headless Linux (no D-Bus / secret-service), file-based
//! storage with strict 0600 permissions is the standard secure approach and is
//! equivalent to what the keyring crate does as its file-credentials fallback.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ── Backend selection ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    Keyring,
    Plaintext,
}

impl Default for Backend {
    fn default() -> Self {
        Self::Keyring
    }
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Keyring => write!(f, "keyring"),
            Self::Plaintext => write!(f, "plaintext"),
        }
    }
}

impl std::str::FromStr for Backend {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "keyring" => Ok(Self::Keyring),
            "plaintext" | "plain" => Ok(Self::Plaintext),
            other => bail!("Unknown backend '{}'. Choose: keyring, plaintext", other),
        }
    }
}

// ── Secrets config section ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SecretsConfig {
    /// Enable the plaintext file backend. Off by default for safety.
    #[serde(default)]
    pub allow_plaintext: bool,

    /// Default backend for `wg secret set` when no --backend is given.
    #[serde(default)]
    pub default_backend: Backend,
}

impl SecretsConfig {
    /// Load the global secrets config from `~/.wg/config.toml`.
    /// Returns defaults if the file doesn't exist or can't be read.
    pub fn load_global() -> Self {
        let path = match dirs::home_dir() {
            Some(h) => h.join(".wg").join("config.toml"),
            None => return Self::default(),
        };
        if !path.exists() {
            return Self::default();
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Self::default(),
        };
        #[derive(serde::Deserialize, Default)]
        struct Partial {
            #[serde(default)]
            secrets: SecretsConfig,
        }
        toml::from_str::<Partial>(&content)
            .map(|p| p.secrets)
            .unwrap_or_default()
    }
}

// ── Path helpers ──────────────────────────────────────────────────────────────

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("Secret name cannot be empty");
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") || name.contains('\0') {
        bail!("Secret name '{}' contains invalid characters", name);
    }
    Ok(())
}

fn keystore_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Cannot determine home directory")?;
    Ok(home.join(".wg").join("keystore"))
}

fn keystore_file(name: &str) -> Result<PathBuf> {
    validate_name(name)?;
    Ok(keystore_dir()?.join(name))
}

fn secrets_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Cannot determine home directory")?;
    Ok(home.join(".wg").join("secrets"))
}

fn secrets_file(name: &str) -> Result<PathBuf> {
    validate_name(name)?;
    Ok(secrets_dir()?.join(name))
}

// ── Shared file I/O ───────────────────────────────────────────────────────────

#[cfg(unix)]
fn write_secret_file(path: &std::path::Path, value: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    std::fs::write(path, value)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_secret_file(path: &std::path::Path, value: &str) -> Result<()> {
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir)?;
    std::fs::write(path, value)?;
    Ok(())
}

fn read_secret_file(path: &std::path::Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    let value = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read secret file {}", path.display()))?;
    Ok(Some(value.trim().to_string()))
}

fn delete_secret_file(path: &std::path::Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(path)?;
    Ok(true)
}

fn list_secret_files(dir: &std::path::Path) -> Result<Vec<String>> {
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut names = vec![];
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
}

// ── Keyring backend (secure file store at ~/.wg/keystore/) ───────────────────

pub fn keyring_set(name: &str, value: &str) -> Result<()> {
    let path = keystore_file(name)?;
    write_secret_file(&path, value)
        .with_context(|| format!("Failed to write to keystore for '{}'", name))
}

pub fn keyring_get(name: &str) -> Result<Option<String>> {
    read_secret_file(&keystore_file(name)?)
}

pub fn keyring_delete(name: &str) -> Result<bool> {
    delete_secret_file(&keystore_file(name)?)
}

pub fn keyring_list() -> Result<Vec<String>> {
    list_secret_files(&keystore_dir()?)
}

// ── Plaintext backend (opt-in, requires allow_plaintext = true) ───────────────

fn plaintext_set(name: &str, value: &str) -> Result<()> {
    let path = secrets_file(name)?;
    write_secret_file(&path, value)
        .with_context(|| format!("Failed to write plaintext secret for '{}'", name))
}

fn plaintext_get(name: &str) -> Result<Option<String>> {
    read_secret_file(&secrets_file(name)?)
}

fn plaintext_delete(name: &str) -> Result<bool> {
    delete_secret_file(&secrets_file(name)?)
}

fn plaintext_list() -> Result<Vec<String>> {
    list_secret_files(&secrets_dir()?)
}

// ── Pass-through resolver ─────────────────────────────────────────────────────

fn resolve_passthrough(uri: &str) -> Result<Option<String>> {
    if let Some(op_path) = uri.strip_prefix("op://") {
        let output = std::process::Command::new("op")
            .arg("read")
            .arg(format!("op://{}", op_path))
            .output()
            .context("Failed to run `op` (1Password CLI). Is it installed and authenticated?")?;
        if output.status.success() {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(Some(value))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("1Password CLI error for '{}': {}", uri, stderr.trim())
        }
    } else if let Some(pass_path) = uri.strip_prefix("pass:") {
        let output = std::process::Command::new("pass")
            .arg("show")
            .arg(pass_path)
            .output()
            .context("Failed to run `pass`. Is it installed?")?;
        if output.status.success() {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let first = value.lines().next().unwrap_or("").trim().to_string();
            Ok(Some(first))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("pass error for '{}': {}", uri, stderr.trim())
        }
    } else {
        Ok(None)
    }
}

// ── Public CRUD API ───────────────────────────────────────────────────────────

/// Store a secret using the chosen backend.
pub fn set(name: &str, value: &str, backend: &Backend, cfg: &SecretsConfig) -> Result<()> {
    match backend {
        Backend::Keyring => keyring_set(name, value),
        Backend::Plaintext => {
            if !cfg.allow_plaintext {
                bail!(
                    "Plaintext backend is disabled. Set `secrets.allow_plaintext = true` in \
                     ~/.wg/config.toml to enable it."
                );
            }
            plaintext_set(name, value)
        }
    }
}

/// Retrieve a secret from the specified backend.
pub fn get(name: &str, backend: &Backend, cfg: &SecretsConfig) -> Result<Option<String>> {
    match backend {
        Backend::Keyring => keyring_get(name),
        Backend::Plaintext => {
            if !cfg.allow_plaintext {
                bail!(
                    "Plaintext backend is disabled. Set `secrets.allow_plaintext = true` in \
                     ~/.wg/config.toml to enable it."
                );
            }
            plaintext_get(name)
        }
    }
}

/// Delete a secret from the specified backend.
pub fn delete(name: &str, backend: &Backend, cfg: &SecretsConfig) -> Result<bool> {
    match backend {
        Backend::Keyring => keyring_delete(name),
        Backend::Plaintext => {
            if !cfg.allow_plaintext {
                bail!("Plaintext backend is disabled.");
            }
            plaintext_delete(name)
        }
    }
}

/// List all secret names across both active backends (names only, never values).
pub fn list(cfg: &SecretsConfig) -> Result<Vec<String>> {
    let mut names = std::collections::BTreeSet::new();
    for n in keyring_list()? {
        names.insert(format!("keyring:{}", n));
    }
    if cfg.allow_plaintext {
        for n in plaintext_list()? {
            names.insert(format!("plain:{}", n));
        }
    }
    Ok(names.into_iter().collect())
}

// ── ref URI resolver ──────────────────────────────────────────────────────────

/// Resolve an `api_key_ref` URI to its actual value.
///
/// URI schemes:
/// - `keyring:<name>` — look up in secure file keystore (`~/.wg/keystore/<name>`)
/// - `plain:<name>` — look up in plaintext file (requires allow_plaintext)
/// - `env:<VAR>` — read from environment variable (opt-in, explicit)
/// - `op://<path>` — 1Password CLI
/// - `pass:<path>` — pass CLI
/// - `literal:<value>` — inline value (warns loudly; test use only)
pub fn resolve_ref(api_key_ref: &str, cfg: &SecretsConfig) -> Result<Option<String>> {
    if let Some(name) = api_key_ref.strip_prefix("keyring:") {
        return keyring_get(name);
    }

    if let Some(name) = api_key_ref.strip_prefix("plain:") {
        if !cfg.allow_plaintext {
            bail!(
                "Secret ref '{}' uses plaintext backend but it is disabled. \
                 Set `secrets.allow_plaintext = true` in ~/.wg/config.toml.",
                api_key_ref
            );
        }
        return plaintext_get(name);
    }

    if let Some(var) = api_key_ref.strip_prefix("env:") {
        return Ok(std::env::var(var).ok());
    }

    if api_key_ref.starts_with("op://") || api_key_ref.starts_with("pass:") {
        return resolve_passthrough(api_key_ref);
    }

    if let Some(value) = api_key_ref.strip_prefix("literal:") {
        eprintln!(
            "WARNING: secret ref uses literal: scheme — this is for testing only. \
             Never use literal: in production config."
        );
        return Ok(Some(value.to_string()));
    }

    bail!(
        "Unknown api_key_ref scheme in '{}'. \
         Supported: keyring:<name>, plain:<name>, env:<VAR>, op://<path>, pass:<path>",
        api_key_ref
    )
}

/// Check whether a ref is reachable (for pre-flight checks).
/// Returns Ok(true) if the secret exists, Ok(false) if not found, Err on config problems.
pub fn check_ref_reachable(api_key_ref: &str, cfg: &SecretsConfig) -> Result<bool> {
    match resolve_ref(api_key_ref, cfg) {
        Ok(Some(_)) => Ok(true),
        Ok(None) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Return a human-readable description of the default backend state.
pub fn backend_status(cfg: &SecretsConfig) -> String {
    let mut parts = vec![format!(
        "Default backend: {} (secure file store at ~/.wg/keystore/)",
        cfg.default_backend
    )];
    if cfg.allow_plaintext {
        parts.push("Plaintext backend: enabled (allow_plaintext = true)".to_string());
    } else {
        parts.push(
            "Plaintext backend: disabled (set secrets.allow_plaintext = true to enable)"
                .to_string(),
        );
    }
    parts.join("\n")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    static HOME_MUTEX: Mutex<()> = Mutex::new(());

    fn with_home(f: impl FnOnce()) -> TempDir {
        let _guard = HOME_MUTEX.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        f();
        tmp
    }

    #[test]
    fn test_keyring_set_get_delete() {
        let _tmp = with_home(|| {
            let cfg = SecretsConfig::default();

            keyring_set("testkey", "sk-abc123").unwrap();
            let val = keyring_get("testkey").unwrap();
            assert_eq!(val.as_deref(), Some("sk-abc123"));

            let deleted = keyring_delete("testkey").unwrap();
            assert!(deleted);

            let val2 = keyring_get("testkey").unwrap();
            assert!(val2.is_none());

            let deleted2 = keyring_delete("testkey").unwrap();
            assert!(!deleted2);

            // list includes the key while it exists
            keyring_set("listkey", "val").unwrap();
            let names = list(&cfg).unwrap();
            assert!(names.iter().any(|n| n.contains("listkey")));
            keyring_delete("listkey").unwrap();
        });
    }

    #[test]
    fn test_plaintext_set_get_list_delete() {
        let _tmp = with_home(|| {
            let cfg = SecretsConfig {
                allow_plaintext: true,
                default_backend: Backend::Plaintext,
            };

            set("mykey", "sk-test-value", &Backend::Plaintext, &cfg).unwrap();
            let val = get("mykey", &Backend::Plaintext, &cfg).unwrap();
            assert_eq!(val.as_deref(), Some("sk-test-value"));

            let names = list(&cfg).unwrap();
            assert!(names.iter().any(|n| n.contains("mykey")));

            let deleted = delete("mykey", &Backend::Plaintext, &cfg).unwrap();
            assert!(deleted);

            let val2 = get("mykey", &Backend::Plaintext, &cfg).unwrap();
            assert!(val2.is_none());

            let deleted2 = delete("mykey", &Backend::Plaintext, &cfg).unwrap();
            assert!(!deleted2);
        });
    }

    #[test]
    fn test_plaintext_disabled_by_default() {
        let _tmp = with_home(|| {
            let cfg = SecretsConfig::default();
            let result = set("key", "val", &Backend::Plaintext, &cfg);
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("allow_plaintext"));
        });
    }

    #[test]
    fn test_resolve_ref_literal() {
        let cfg = SecretsConfig::default();
        let val = resolve_ref("literal:test-key", &cfg).unwrap();
        assert_eq!(val.as_deref(), Some("test-key"));
    }

    #[test]
    fn test_resolve_ref_env() {
        let cfg = SecretsConfig::default();
        unsafe { std::env::set_var("WG_TEST_SECRET_VAR_XYZ", "env-value") };
        let val = resolve_ref("env:WG_TEST_SECRET_VAR_XYZ", &cfg).unwrap();
        assert_eq!(val.as_deref(), Some("env-value"));
        unsafe { std::env::remove_var("WG_TEST_SECRET_VAR_XYZ") };
    }

    #[test]
    fn test_resolve_ref_env_missing() {
        let cfg = SecretsConfig::default();
        unsafe { std::env::remove_var("WG_NONEXISTENT_VAR_12345") };
        let val = resolve_ref("env:WG_NONEXISTENT_VAR_12345", &cfg).unwrap();
        assert!(val.is_none());
    }

    #[test]
    fn test_resolve_ref_keyring() {
        let _tmp = with_home(|| {
            let cfg = SecretsConfig::default();
            keyring_set("myapikey", "sk-12345").unwrap();
            let val = resolve_ref("keyring:myapikey", &cfg).unwrap();
            assert_eq!(val.as_deref(), Some("sk-12345"));
            keyring_delete("myapikey").unwrap();
        });
    }

    #[test]
    fn test_resolve_ref_plain() {
        let _tmp = with_home(|| {
            let cfg = SecretsConfig {
                allow_plaintext: true,
                default_backend: Backend::Plaintext,
            };
            plaintext_set("myapikey", "sk-plain").unwrap();
            let val = resolve_ref("plain:myapikey", &cfg).unwrap();
            assert_eq!(val.as_deref(), Some("sk-plain"));
            plaintext_delete("myapikey").unwrap();
        });
    }

    #[test]
    fn test_resolve_ref_unknown_scheme() {
        let cfg = SecretsConfig::default();
        let result = resolve_ref("fakescheme:something", &cfg);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown api_key_ref scheme"));
    }

    #[test]
    fn test_check_ref_reachable_missing() {
        let _tmp = with_home(|| {
            let cfg = SecretsConfig {
                allow_plaintext: true,
                default_backend: Backend::Plaintext,
            };
            let reachable = check_ref_reachable("plain:no-such-key", &cfg).unwrap();
            assert!(!reachable);
        });
    }

    #[test]
    fn test_secret_name_rejects_path_traversal() {
        let result = validate_name("../../etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn test_secret_name_rejects_slash() {
        let result = validate_name("subdir/key");
        assert!(result.is_err());
    }
}
