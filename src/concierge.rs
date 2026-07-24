//! Reversible, profile-first `worksgood` concierge trial.
//!
//! This module is intentionally a narrow orchestration layer. It composes the
//! existing graph initializer and service/TUI entrypoints through one verified
//! absolute WorksGood executable, and uses the existing project-profile plan /
//! apply API directly. It is not a second config, trust, process, or TUI owner.

use crate::atomic_file::write_atomic;
use crate::config::{Config, ReasoningLevel};
use crate::profile::{named, project};
use crate::service_identity::{
    ServiceHealth, ServiceIdentity, ServiceObservation, executable_sha256, expected_identity,
    observe_service,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const ATTENDED_TTY_REQUIRED: &str = "ATTENDED_TTY_REQUIRED";
pub const STATE_FILE: &str = "concierge.json";
pub const PENDING_FILE: &str = "concierge-pending.json";
const STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryTarget {
    pub repository: PathBuf,
    pub graph: PathBuf,
    pub graph_exists: bool,
    pub worktree: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutableAuthority {
    pub executable: PathBuf,
    pub sha256: String,
    pub build_id: String,
    pub authority: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConciergeMode {
    ContinueWithoutAi,
    Profile,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConciergeState {
    pub version: u32,
    pub mode: ConciergeMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_fingerprint: Option<String>,
    pub plan_digest: String,
    pub executable_sha256: String,
}

/// Durable post-confirmation recovery marker. It is written only after graph
/// init (never during planning/dry-run), contains no credential or raw profile
/// content, and gives rollback enough preimage identity to clear only this
/// transaction's project selection/service start.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingTransaction {
    pub version: u32,
    pub plan_digest: String,
    pub mode: ConciergeMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_association: Option<project::ProjectProfileAssociation>,
    pub graph_created: bool,
    pub service_was_down: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReconcileAction {
    None,
    Start,
    Reuse,
    Reload,
    RepairAndStart,
    ControlledRestart,
    StopForNoAi,
    /// Fail closed without signalling a foreign or unverifiable service.
    Refuse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConciergePlan {
    pub version: u32,
    pub repository: String,
    pub graph: String,
    pub graph_exists: bool,
    pub executable: String,
    pub executable_sha256: String,
    pub build_id: String,
    pub executable_authority: String,
    pub mode: ConciergeMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<project::ProjectSelectionPlan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clear_selection: Option<project::ProjectClearPlan>,
    pub service_observation: ServiceObservation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_service_identity: Option<ServiceIdentity>,
    pub service_action: ReconcileAction,
    pub service_reason: String,
    pub actions: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct LifecycleOptions {
    pub project: Option<PathBuf>,
    pub dry_run: bool,
    pub requested_profile: Option<String>,
    pub continue_without_ai: bool,
    pub strong_model: Option<String>,
    pub weak_model: Option<String>,
    pub strong_reasoning: Option<ReasoningLevel>,
    pub weak_reasoning: Option<ReasoningLevel>,
    pub yes: bool,
    pub open_tui: bool,
}

#[derive(Debug, Clone)]
struct PreparedPlan {
    public: ConciergePlan,
    generated_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InstallReceipt {
    product: String,
    executable: String,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PiModel {
    pub provider: String,
    pub id: String,
    pub name: String,
    pub reasoning: bool,
}

fn state_path(graph: &Path) -> PathBuf {
    graph.join(STATE_FILE)
}

fn pending_path(graph: &Path) -> PathBuf {
    graph.join(PENDING_FILE)
}

pub fn resolve_repository(start: Option<&Path>) -> Result<RepositoryTarget> {
    let physical = match start {
        Some(path) => path
            .canonicalize()
            .with_context(|| format!("Cannot canonicalize project {}", path.display()))?,
        None => std::env::current_dir()?.canonicalize()?,
    };
    let start_dir = if physical.is_file() {
        physical
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Project path has no parent"))?
            .to_path_buf()
    } else {
        physical
    };
    let repository = start_dir
        .ancestors()
        .find(|candidate| candidate.join(".git").exists())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No enclosing Git repository/worktree. Pass --project <repo>; global ~/.wg fallback is disabled."
            )
        })?;
    let graph = repository.join(".wg");
    if !graph.exists() && repository.join(".workgraph").exists() {
        anyhow::bail!(
            "Legacy graph {} exists; refusing to create a competing .wg. Use the existing migration policy first.",
            repository.join(".workgraph").display()
        );
    }
    let graph_exists = graph.is_dir();
    let graph = if graph_exists {
        graph.canonicalize()?
    } else {
        graph
    };
    Ok(RepositoryTarget {
        worktree: repository.join(".git").is_file(),
        repository,
        graph,
        graph_exists,
    })
}

fn same_bundle_sibling(current: &Path, candidate: &Path) -> bool {
    current.parent() == candidate.parent()
        && candidate.file_name().and_then(|v| v.to_str())
            == Some(if cfg!(windows) { "wg.exe" } else { "wg" })
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    true
}

/// Resolve `W` without PATH lookup or executing a candidate. The normal trial
/// shape is the sibling `wg` produced by the same isolated Cargo target. A
/// separately located build requires an absolute receipt whose recorded hash
/// matches before a byte is executed.
pub fn resolve_authoritative_executable() -> Result<ExecutableAuthority> {
    let current_raw = std::env::current_exe()?;
    let current = current_raw.canonicalize()?;
    let requested = std::env::var_os("WORKSGOOD_W_EXECUTABLE")
        .map(PathBuf::from)
        .unwrap_or_else(|| current.with_file_name(if cfg!(windows) { "wg.exe" } else { "wg" }));
    if !requested.is_absolute() {
        anyhow::bail!(
            "WORKSGOOD_W_EXECUTABLE must be absolute; PATH candidates are never executed"
        );
    }
    if fs::symlink_metadata(&requested)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        anyhow::bail!(
            "Refusing symlinked WorksGood candidate {}",
            requested.display()
        );
    }
    let candidate = requested.canonicalize().with_context(|| {
        format!(
            "WorksGood executable is unavailable: {}",
            requested.display()
        )
    })?;
    if !is_executable_file(&candidate) {
        anyhow::bail!(
            "WorksGood candidate is not an executable file: {}",
            candidate.display()
        );
    }
    let hash = executable_sha256(&candidate)?;
    let authority = if same_bundle_sibling(&current, &candidate) {
        "same isolated Cargo bundle".to_string()
    } else {
        let receipt_path = std::env::var_os("WORKSGOOD_W_RECEIPT")
            .map(PathBuf::from)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Candidate {} is outside the isolated worksgood bundle and has no WORKSGOOD_W_RECEIPT",
                    candidate.display()
                )
            })?;
        if !receipt_path.is_absolute() {
            anyhow::bail!("WORKSGOOD_W_RECEIPT must be absolute");
        }
        let receipt: InstallReceipt = serde_json::from_slice(&fs::read(&receipt_path)?)?;
        if receipt.product != "WorksGood"
            || Path::new(&receipt.executable).canonicalize()? != candidate
            || receipt.sha256 != hash
        {
            anyhow::bail!("WorksGood receipt does not authenticate the requested executable");
        }
        format!("verified receipt {}", receipt_path.display())
    };
    Ok(ExecutableAuthority {
        build_id: crate::service_identity::build_id(&hash),
        executable: candidate,
        sha256: hash,
        authority,
    })
}

fn run_w(w: &Path, graph: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new(w)
        .arg("--dir")
        .arg(graph)
        .args(args)
        .status()
        .with_context(|| format!("Failed to execute authenticated {}", w.display()))?;
    if !status.success() {
        anyhow::bail!("{} {} exited with {}", w.display(), args.join(" "), status);
    }
    Ok(())
}

fn run_w_quiet(w: &Path, graph: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new(w)
        .arg("--dir")
        .arg(graph)
        .args(args)
        .output()
        .with_context(|| format!("Failed to execute authenticated {}", w.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "{} {} exited with {}: {}{}",
            w.display(),
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn digest_json<T: Serialize>(value: &T) -> Result<String> {
    let bytes = serde_json::to_vec(value)?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn read_state(graph: &Path) -> Result<Option<ConciergeState>> {
    match fs::read(state_path(graph)) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).context("Invalid concierge state")?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn write_state(graph: &Path, state: &ConciergeState) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(state)?;
    bytes.push(b'\n');
    write_atomic(&state_path(graph), &bytes).context("Failed to commit concierge state")
}

fn write_pending(graph: &Path, pending: &PendingTransaction) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(pending)?;
    bytes.push(b'\n');
    write_atomic(&pending_path(graph), &bytes).context("Failed to write recovery marker")
}

fn read_pending(graph: &Path) -> Result<Option<PendingTransaction>> {
    match fs::read(pending_path(graph)) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).context("Invalid concierge recovery marker")?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn profile_content(name: &str) -> Result<String> {
    let path = named::profile_path(name)?;
    if path.is_file() {
        return fs::read_to_string(&path)
            .with_context(|| format!("Failed to read profile {}", path.display()));
    }
    named::starter_template(name)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("Profile '{}' is unavailable", name))
}

fn patch_two_tier_content(
    mut content: String,
    strong: &str,
    weak: &str,
    strong_reasoning: ReasoningLevel,
    weak_reasoning: ReasoningLevel,
) -> Result<String> {
    crate::config::parse_model_spec_strict(strong)?;
    crate::config::parse_model_spec_strict(weak)?;
    for key in Config::PI_STRONG_TOML_KEYS {
        content = named::set_toml_string_value(&content, key, strong);
    }
    for key in Config::PI_WEAK_TOML_KEYS {
        content = named::set_toml_string_value(&content, key, weak);
    }
    for key in Config::PI_STRONG_REASONING_TOML_KEYS {
        content = named::set_toml_string_value(&content, key, &strong_reasoning.to_string());
    }
    // Concierge configures Worker *and Chat* together and always configures
    // the weak roles in the same transaction, so make the default/chat effort
    // explicit without creating a partial-update inheritance surprise.
    content = named::set_toml_string_value(
        &content,
        "models.default.reasoning",
        &strong_reasoning.to_string(),
    );
    for key in Config::PI_WEAK_REASONING_TOML_KEYS {
        content = named::set_toml_string_value(&content, key, &weak_reasoning.to_string());
    }
    let parsed: Config = toml::from_str(&content)?;
    parsed.validate_model_format()?;
    Ok(content)
}

fn generated_profile_name(base: &str, content: &str) -> Result<String> {
    let fp = project::profile_content_fingerprint(content)?;
    let short = fp
        .trim_start_matches("b3:")
        .chars()
        .take(10)
        .collect::<String>();
    Ok(format!("concierge-{base}-{short}"))
}

fn existing_or_generated_plan(
    graph: &Path,
    base: &str,
    content: String,
) -> Result<(project::ProjectSelectionPlan, Option<String>)> {
    let name = generated_profile_name(base, &content)?;
    let path = named::profile_path(&name)?;
    if path.is_file() {
        let existing = fs::read_to_string(&path)?;
        if project::profile_content_fingerprint(&existing)?
            != project::profile_content_fingerprint(&content)?
        {
            anyhow::bail!("Generated profile hash collision at {}", path.display());
        }
        return Ok((project::plan_project_selection(graph, &name)?, None));
    }
    Ok((
        project::plan_generated_project_selection(graph, &name, &content)?,
        Some(content),
    ))
}

fn route_for_pi_model(model: &PiModel) -> String {
    format!("pi:{}:{}", model.provider, model.id)
}

fn parse_mock_pi_models(raw: &str) -> Result<Vec<PiModel>> {
    let value: serde_json::Value = serde_json::from_str(raw)?;
    let values = value
        .get("models")
        .and_then(|v| v.as_array())
        .or_else(|| value.as_array())
        .ok_or_else(|| anyhow::anyhow!("Mock Pi catalog must be an array or {{models:[...]}}"))?;
    values
        .iter()
        .map(|v| {
            Ok(PiModel {
                provider: v
                    .get("provider")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Pi model omitted provider"))?
                    .to_string(),
                id: v
                    .get("id")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Pi model omitted id"))?
                    .to_string(),
                name: v
                    .get("name")
                    .and_then(|x| x.as_str())
                    .unwrap_or_else(|| v.get("id").and_then(|x| x.as_str()).unwrap_or("model"))
                    .to_string(),
                reasoning: v
                    .get("reasoning")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false),
            })
        })
        .collect()
}

/// Query Pi's own authenticated model registry. Tests can supply the exact RPC
/// response data via `WORKSGOOD_PI_MODELS_JSON`; dry-run never starts Pi because
/// even a no-session external process may refresh its own cache.
pub fn pi_available_models(allow_process: bool) -> Result<Vec<PiModel>> {
    if let Ok(mock) = std::env::var("WORKSGOOD_PI_MODELS_JSON") {
        return parse_mock_pi_models(&mock);
    }
    if !allow_process {
        anyhow::bail!("Pi catalog probe suppressed in strict dry-run; use manual exact IDs");
    }
    let pi = crate::executor_discovery::discover()
        .into_iter()
        .find(|entry| entry.name == "pi" && entry.available)
        .and_then(|entry| entry.binary_path)
        .ok_or_else(|| {
            anyhow::anyhow!("Pi is unavailable; install/login with Pi or enter manual exact IDs")
        })?;
    let mut child = Command::new(&pi)
        .args(["--mode", "rpc", "--no-session", "-ne"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("Failed to start Pi at {}", pi.display()))?;
    writeln!(
        child.stdin.as_mut().context("Pi stdin missing")?,
        r#"{{"id":"worksgood-catalog","type":"get_available_models"}}"#
    )?;
    child.stdin.take();
    let stdout = child.stdout.take().context("Pi stdout missing")?;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut result = Err(anyhow::anyhow!("Pi ended without a model catalog"));
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if value.get("id").and_then(|v| v.as_str()) == Some("worksgood-catalog") {
                result = (|| {
                    if value.get("success").and_then(|v| v.as_bool()) != Some(true) {
                        anyhow::bail!("Pi model catalog request failed");
                    }
                    parse_mock_pi_models(&serde_json::to_string(
                        value
                            .get("data")
                            .context("Pi catalog response omitted data")?,
                    )?)
                })();
                break;
            }
        }
        let _ = tx.send(result);
    });
    let result = rx.recv_timeout(Duration::from_secs(5));
    let _ = child.kill();
    let _ = child.wait();
    match result {
        Ok(models) => models,
        Err(_) => anyhow::bail!("Pi model catalog timed out; use manual exact IDs"),
    }
}

fn reasoning_label(reasoning: Option<ReasoningLevel>) -> &'static str {
    reasoning
        .map(ReasoningLevel::as_str)
        .unwrap_or("handler default")
}

fn print_catalog(graph: &Path) -> Result<Vec<project::ProfileCatalogEntry>> {
    let all = project::catalog(graph)?;
    let (catalog, advanced): (Vec<_>, Vec<_>) = all.into_iter().partition(|entry| {
        entry.readiness.handlers.iter().all(|handler| {
            matches!(
                handler.handler.as_str(),
                "pi" | "codex" | "claude" | "native" | "opencode"
            )
        })
    });
    println!("\nChoose how this repository should run (nothing is selected automatically):");
    println!("  0. Continue without AI — no LLM service");
    for (index, entry) in catalog.iter().enumerate() {
        let current = if entry.selected_for_project {
            " [current project]"
        } else {
            ""
        };
        let frequency = entry
            .usage_label
            .as_deref()
            .map(|label| format!("; {label}"))
            .unwrap_or_default();
        println!(
            "  {}. {}{} — worker/chat {} (effort {} [{}]); agency {} (effort {} [{}]){}\n       {}",
            index + 1,
            entry.name,
            current,
            entry.readiness.strong_route,
            reasoning_label(entry.readiness.strong_reasoning),
            entry.readiness.strong_reasoning_provenance,
            entry.readiness.weak_route,
            reasoning_label(entry.readiness.weak_reasoning),
            entry.readiness.weak_reasoning_provenance,
            frequency,
            entry.readiness.annotation
        );
    }
    if !advanced.is_empty() {
        println!(
            "  Advanced: {} specialized worker/chat adapter profile(s); use the complete `wg profile list` surface",
            advanced.len()
        );
    }
    println!("  q. Cancel [default]");
    Ok(catalog)
}

fn read_answer(prompt: &str) -> Result<String> {
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(answer.trim().to_string())
}

fn choose_reasoning(label: &str, default: ReasoningLevel) -> Result<ReasoningLevel> {
    let answer = read_answer(&format!(
        "{label} reasoning [off|minimal|low|medium|high|xhigh|max] (default {default}): "
    ))?;
    if answer.is_empty() {
        return Ok(default);
    }
    answer.parse()
}

fn resolve_effort_choice(
    label: &str,
    explicit: Option<ReasoningLevel>,
    configured: Option<ReasoningLevel>,
    recommended: ReasoningLevel,
    yes: bool,
    flag: &str,
) -> Result<ReasoningLevel> {
    if let Some(level) = explicit {
        return Ok(level);
    }
    if yes {
        return configured.ok_or_else(|| {
            anyhow::anyhow!(
                "--yes does not choose a consequential {label} effort. The selected profile has no effective value; pass {flag} <off|minimal|low|medium|high|xhigh|max> explicitly"
            )
        });
    }
    choose_reasoning(label, configured.unwrap_or(recommended))
}

fn choose_pi_route(
    options: &LifecycleOptions,
    configured: &Config,
) -> Result<(String, String, ReasoningLevel, ReasoningLevel)> {
    let (configured_strong, configured_weak) = configured.pi_tiers();
    let (strong, weak) = match (&options.strong_model, &options.weak_model) {
        (Some(strong), Some(weak)) => (strong.clone(), weak.clone()),
        (None, None) if options.yes => (
            configured_strong.context(
                "--yes cannot choose a Pi Worker/chat model; pass --strong-model explicitly",
            )?,
            configured_weak.context(
                "--yes cannot choose a Pi Agency/FLIP/evaluation model; pass --weak-model explicitly",
            )?,
        ),
        (None, None) => {
            let models = pi_available_models(!options.dry_run).unwrap_or_default();
    if !models.is_empty() {
        println!("\nModels available through Pi's own authenticated registry:");
        for (index, model) in models.iter().enumerate() {
            println!(
                "  {}. {}/{} — {}",
                index + 1,
                model.provider,
                model.id,
                model.name
            );
        }
    } else {
        println!("Pi catalog/auth unavailable. Manual exact handler-first IDs remain available.");
    }
    println!("  m. Enter a manual exact handler-first model ID");
    println!("  q. Cancel");
    let worker = read_answer("Worker/chat model: ")?;
    if worker.is_empty() || worker == "q" {
        anyhow::bail!("Cancelled before planning; nothing was written");
    }
    let strong = if worker == "m" {
        read_answer("Exact Worker/chat route (pi:<provider>:<model>): ")?
    } else {
        let index: usize = worker.parse().context("Invalid Pi model selection")?;
        route_for_pi_model(
            models
                .get(index - 1)
                .context("Pi model selection out of range")?,
        )
    };
    println!("Agency/FLIP/evaluation model must be explicit:");
    println!("  s. Same as worker");
    println!("  m. Manual exact handler-first model ID");
    for (index, model) in models.iter().enumerate() {
        println!("  {}. {}/{}", index + 1, model.provider, model.id);
    }
    let agency = read_answer("Agency model: ")?;
    if agency.is_empty() || agency == "q" {
        anyhow::bail!("Cancelled before planning; nothing was written");
    }
    let weak = if agency == "s" {
        strong.clone()
    } else if agency == "m" {
        read_answer("Exact Agency route (pi:<provider>:<model>): ")?
    } else {
        let index: usize = agency.parse().context("Invalid Pi model selection")?;
        route_for_pi_model(
            models
                .get(index - 1)
                .context("Pi model selection out of range")?,
        )
    };
            (strong, weak)
        }
        _ => anyhow::bail!(
            "Manual Pi configuration requires both --strong-model and --weak-model; no model route was inferred"
        ),
    };
    let strong_reasoning = resolve_effort_choice(
        "Worker/chat",
        options.strong_reasoning,
        configured.resolve_reasoning_for_role(crate::config::DispatchRole::TaskAgent),
        ReasoningLevel::High,
        options.yes,
        "--strong-reasoning",
    )?;
    let weak_reasoning = resolve_effort_choice(
        "Agency/FLIP/evaluation",
        options.weak_reasoning,
        configured.resolve_reasoning_for_role(crate::config::DispatchRole::Evaluator),
        ReasoningLevel::Low,
        options.yes,
        "--weak-reasoning",
    )?;
    Ok((strong, weak, strong_reasoning, weak_reasoning))
}

fn customize_core_profile(
    base: &str,
    content: String,
    options: &LifecycleOptions,
) -> Result<Option<String>> {
    let config: Config = toml::from_str(&content)?;
    let (configured_strong, configured_weak) = config.pi_tiers();
    let configured_strong = configured_strong.unwrap_or_else(|| config.agent.model.clone());
    let configured_weak = configured_weak.unwrap_or_else(|| configured_strong.clone());
    let noninteractive_choice = options.yes;
    let (strong, weak) = match (&options.strong_model, &options.weak_model) {
        (None, None) if noninteractive_choice => {
            (configured_strong.clone(), configured_weak.clone())
        }
        (None, None) => {
            println!("\nHandler-owned profile picker for '{base}':");
            println!("  Worker/chat: {configured_strong}");
            println!("  Agency/FLIP/evaluation: {configured_weak}");
            println!("  u. Use these exact configured routes");
            println!("  m. Enter exact routes for this same handler");
            println!("  q. Cancel [default]");
            match read_answer("Route choice: ")?.as_str() {
                "u" | "U" => (configured_strong.clone(), configured_weak.clone()),
                "m" | "M" => (
                    read_answer("Exact Worker/chat handler-first route: ")?,
                    read_answer("Exact Agency/FLIP/evaluation handler-first route: ")?,
                ),
                _ => anyhow::bail!("Cancelled before planning; nothing was written"),
            }
        }
        (Some(strong), Some(weak)) => (strong.clone(), weak.clone()),
        _ => anyhow::bail!(
            "Manual core-profile configuration requires both --strong-model and --weak-model; no agency route was inferred"
        ),
    };
    crate::config::parse_model_spec_strict(&strong)?;
    crate::config::parse_model_spec_strict(&weak)?;
    let configured_handler = crate::dispatch::handler_for_model(&configured_strong);
    if crate::dispatch::handler_for_model(&strong) != configured_handler
        || crate::dispatch::handler_for_model(&weak) != configured_handler
    {
        anyhow::bail!(
            "Profile '{}' is owned by handler '{}'; cross-system fallback/routes were refused",
            base,
            configured_handler.as_str()
        );
    }
    let strong_reasoning = resolve_effort_choice(
        "Worker/chat",
        options.strong_reasoning,
        config.resolve_reasoning_for_role(crate::config::DispatchRole::TaskAgent),
        ReasoningLevel::High,
        noninteractive_choice,
        "--strong-reasoning",
    )?;
    let weak_reasoning = resolve_effort_choice(
        "Agency/FLIP/evaluation",
        options.weak_reasoning,
        config.resolve_reasoning_for_role(crate::config::DispatchRole::Evaluator),
        ReasoningLevel::Low,
        noninteractive_choice,
        "--weak-reasoning",
    )?;
    patch_two_tier_content(content, &strong, &weak, strong_reasoning, weak_reasoning).map(Some)
}

fn choose_mode_and_profile(
    target: &RepositoryTarget,
    options: &LifecycleOptions,
) -> Result<(ConciergeMode, Option<String>)> {
    if options.continue_without_ai {
        return Ok((ConciergeMode::ContinueWithoutAi, None));
    }
    if let Some(profile) = &options.requested_profile {
        return Ok((ConciergeMode::Profile, Some(profile.clone())));
    }
    let catalog = print_catalog(&target.graph)?;
    let answer = read_answer("Selection: ")?;
    if answer.is_empty() || answer.eq_ignore_ascii_case("q") {
        anyhow::bail!("Cancelled before planning; nothing was written");
    }
    if answer == "0" {
        return Ok((ConciergeMode::ContinueWithoutAi, None));
    }
    let index: usize = answer.parse().context("Invalid selection")?;
    let profile = catalog.get(index - 1).context("Selection out of range")?;
    if profile.source == project::ProfileSource::Unavailable {
        anyhow::bail!("Selected profile '{}' is unavailable", profile.name);
    }
    Ok((ConciergeMode::Profile, Some(profile.name.clone())))
}

#[derive(Debug, Clone)]
struct ReconcileDecision {
    action: ReconcileAction,
    reason: String,
}

fn service_identity_equivalent(actual: &ServiceIdentity, expected: &ServiceIdentity) -> bool {
    actual.canonical_graph == expected.canonical_graph
        && actual.graph_digest == expected.graph_digest
        && actual.executable_sha256 == expected.executable_sha256
        && actual.build_id == expected.build_id
        && actual.protocol == expected.protocol
        && actual.config_fingerprint == expected.config_fingerprint
        && actual.selected_profile == expected.selected_profile
        && actual.selected_profile_fingerprint == expected.selected_profile_fingerprint
}

fn verify_running_executable(
    actual: &ServiceIdentity,
    expected: &ServiceIdentity,
) -> Result<String, String> {
    let path = Path::new(&actual.executable);
    if !path.is_absolute() {
        return Err("live service executable identity is not absolute".to_string());
    }
    // Replacing the authoritative path while its prior image is still running
    // is the normal same-version/different-build upgrade case. The expected
    // fingerprint already authenticated the new bytes; restart is safe.
    if actual.executable == expected.executable
        && actual.executable_sha256 != expected.executable_sha256
    {
        return Ok(
            "authoritative executable path now contains a different authenticated build"
                .to_string(),
        );
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("cannot authenticate running executable path: {error}"))?;
    if canonical.display().to_string() != actual.executable {
        return Err("running executable canonical identity changed after startup".to_string());
    }
    let on_disk = executable_sha256(&canonical)
        .map_err(|error| format!("cannot authenticate running executable bytes: {error}"))?;
    if on_disk != actual.executable_sha256 {
        return Err(
            "running executable was replaced or deleted and its startup build is unverifiable"
                .to_string(),
        );
    }
    Ok("running executable path and startup content fingerprint agree".to_string())
}

fn reconcile_decision(
    observation: &ServiceObservation,
    expected: Option<&ServiceIdentity>,
    mode: &ConciergeMode,
) -> ReconcileDecision {
    if *mode == ConciergeMode::ContinueWithoutAi {
        return match observation.health {
            ServiceHealth::Down | ServiceHealth::StalePid => ReconcileDecision {
                action: ReconcileAction::None,
                reason: "no live service is present; Continue without AI remains service-free"
                    .to_string(),
            },
            ServiceHealth::Healthy => ReconcileDecision {
                action: ReconcileAction::StopForNoAi,
                reason:
                    "a verified project service is running and must stop for Continue without AI"
                        .to_string(),
            },
            ServiceHealth::Foreign | ServiceHealth::Unverified | ServiceHealth::Unresponsive => {
                ReconcileDecision {
                    action: ReconcileAction::Refuse,
                    reason: format!(
                        "refuse to signal an unverifiable service: {}",
                        observation.detail
                    ),
                }
            }
        };
    }
    match observation.health {
        ServiceHealth::Down => ReconcileDecision {
            action: ReconcileAction::Start,
            reason: "service is down; start the authenticated paired build".to_string(),
        },
        ServiceHealth::StalePid => ReconcileDecision {
            action: ReconcileAction::RepairAndStart,
            reason: "recorded PID birth is proven dead/stale; repair state and start".to_string(),
        },
        ServiceHealth::Foreign | ServiceHealth::Unverified | ServiceHealth::Unresponsive => {
            ReconcileDecision {
                action: ReconcileAction::Refuse,
                reason: format!(
                    "foreign or unverifiable handshake; no process will be signalled: {}",
                    observation.detail
                ),
            }
        }
        ServiceHealth::Healthy => {
            let (Some(actual), Some(expected)) =
                (observation.handshake_identity.as_ref(), expected)
            else {
                return ReconcileDecision {
                    action: ReconcileAction::Refuse,
                    reason: "healthy state omitted an actual or intended service identity"
                        .to_string(),
                };
            };
            let executable_evidence = match verify_running_executable(actual, expected) {
                Ok(evidence) => evidence,
                Err(reason) => {
                    return ReconcileDecision {
                        action: ReconcileAction::Refuse,
                        reason: format!("{reason}; no process will be signalled"),
                    };
                }
            };
            let compatible_build = actual.executable_sha256 == expected.executable_sha256
                && actual.build_id == expected.build_id
                && actual.protocol == expected.protocol;
            if !compatible_build {
                return ReconcileDecision {
                    action: ReconcileAction::ControlledRestart,
                    reason: format!(
                        "binary/build/protocol mismatch (actual build={} protocol={}, intended build={} protocol={}); {executable_evidence}",
                        actual.build_id, actual.protocol, expected.build_id, expected.protocol
                    ),
                };
            }
            let same_generation = actual.config_fingerprint == expected.config_fingerprint
                && actual.selected_profile == expected.selected_profile
                && actual.selected_profile_fingerprint == expected.selected_profile_fingerprint;
            if !same_generation {
                return ReconcileDecision {
                    action: ReconcileAction::Reload,
                    reason: format!(
                        "compatible build with config/profile/reasoning generation drift (actual config={} profile={:?}/{:?}, intended config={} profile={:?}/{:?})",
                        actual.config_fingerprint,
                        actual.selected_profile,
                        actual.selected_profile_fingerprint,
                        expected.config_fingerprint,
                        expected.selected_profile,
                        expected.selected_profile_fingerprint
                    ),
                };
            }
            ReconcileDecision {
                action: ReconcileAction::Reuse,
                reason: format!(
                    "canonical graph, protocol, profile/config generation, and content build fingerprint all match; {executable_evidence}"
                ),
            }
        }
    }
}

fn prepare_plan(
    target: &RepositoryTarget,
    executable: &ExecutableAuthority,
    options: &LifecycleOptions,
    mode: ConciergeMode,
    base_profile: Option<String>,
) -> Result<PreparedPlan> {
    let observation = observe_service(&target.graph);
    let mut generated_content = None;
    let mut planned_profile_toml = None;
    let selection = if mode == ConciergeMode::Profile {
        let base = base_profile.context("Profile mode omitted profile")?;
        if base == "pi" || base.starts_with("pi-") {
            let content = profile_content(&base)?;
            let configured: Config = toml::from_str(&content)?;
            let (strong, weak, strong_reasoning, weak_reasoning) =
                choose_pi_route(options, &configured)?;
            let content =
                patch_two_tier_content(content, &strong, &weak, strong_reasoning, weak_reasoning)?;
            planned_profile_toml = Some(content.parse::<toml::Value>()?);
            let (plan, generated) = existing_or_generated_plan(&target.graph, &base, content)?;
            generated_content = generated;
            Some(plan)
        } else {
            let content = profile_content(&base)?;
            if let Some(content) = customize_core_profile(&base, content, options)? {
                planned_profile_toml = Some(content.parse::<toml::Value>()?);
                let (plan, generated) = existing_or_generated_plan(&target.graph, &base, content)?;
                generated_content = generated;
                Some(plan)
            } else {
                let content = profile_content(&base)?;
                planned_profile_toml = Some(content.parse::<toml::Value>()?);
                Some(project::plan_project_selection(&target.graph, &base)?)
            }
        }
    } else {
        None
    };
    let mut expected = if target.graph_exists {
        match (mode.clone(), planned_profile_toml) {
            (ConciergeMode::Profile, Some(profile)) => {
                let config = Config::load_merged_for_planned_profile(&target.graph, profile)?;
                Some(expected_identity(
                    &target.graph,
                    &executable.executable,
                    &config,
                )?)
            }
            _ => None,
        }
    } else {
        None
    };
    // Planning may describe a not-yet-applied project generation. Bind the
    // intended handshake to the immutable selection rather than the currently
    // committed association read by expected_identity().
    if let (Some(expected), Some(selection)) = (expected.as_mut(), selection.as_ref()) {
        expected.selected_profile = Some(selection.profile.clone());
        expected.selected_profile_fingerprint = Some(selection.profile_fingerprint.clone());
    }
    let clear_selection = if mode == ConciergeMode::ContinueWithoutAi
        && target.graph_exists
        && project::read_association(&target.graph)?.is_some()
    {
        Some(project::plan_clear_project_selection(&target.graph)?)
    } else {
        None
    };
    let service_decision = reconcile_decision(&observation, expected.as_ref(), &mode);
    let mut actions = Vec::new();
    if !target.graph_exists {
        actions.push(format!("Initialize graph at {}", target.graph.display()));
    }
    if let Some(selection) = &selection {
        if selection.materializes_global_profile_definition {
            actions.push(format!(
                "Materialize reusable profile '{}' (fingerprint {})",
                selection.profile, selection.profile_fingerprint
            ));
        }
        actions.push(format!(
            "Select profile '{}' for this project only; Worker/chat {} (effort {} [{}]); Agency/FLIP/evaluation {} (effort {} [{}])",
            selection.profile,
            selection.readiness.strong_route,
            reasoning_label(selection.readiness.strong_reasoning),
            selection.readiness.strong_reasoning_provenance,
            selection.readiness.weak_route,
            reasoning_label(selection.readiness.weak_reasoning),
            selection.readiness.weak_reasoning_provenance,
        ));
        if selection
            .readiness
            .handlers
            .iter()
            .any(|handler| handler.handler == "pi")
        {
            actions.push(
                "Prepare the compatible Pi plugin; Pi retains authentication ownership".into(),
            );
        }
    } else {
        actions.push("Continue without AI; do not select a route or run an LLM service".into());
        if clear_selection.is_some() {
            actions.push(
                "Clear the exact current-project profile association; reusable definition remains"
                    .into(),
            );
        }
    }
    actions.push(format!(
        "Service reconcile: {:?} — {}",
        service_decision.action, service_decision.reason
    ));
    if options.open_tui {
        actions.push(format!(
            "Open setup-neutral TUI with {} --dir {} tui",
            executable.executable.display(),
            target.graph.display()
        ));
    }
    Ok(PreparedPlan {
        public: ConciergePlan {
            version: 1,
            repository: target.repository.display().to_string(),
            graph: target.graph.display().to_string(),
            graph_exists: target.graph_exists,
            executable: executable.executable.display().to_string(),
            executable_sha256: executable.sha256.clone(),
            build_id: executable.build_id.clone(),
            executable_authority: executable.authority.clone(),
            mode,
            profile: selection.as_ref().map(|plan| plan.profile.clone()),
            selection,
            clear_selection,
            service_observation: observation,
            expected_service_identity: expected,
            service_action: service_decision.action,
            service_reason: service_decision.reason,
            actions,
        },
        generated_content,
    })
}

fn print_plan(plan: &ConciergePlan) -> Result<()> {
    println!("\nImmutable redacted plan:");
    println!("{}", serde_json::to_string_pretty(plan)?);
    println!("Plan digest: {}", digest_json(plan)?);
    Ok(())
}

fn confirm_plan(options: &LifecycleOptions) -> Result<bool> {
    if options.yes {
        return Ok(true);
    }
    Ok(matches!(
        read_answer("Apply this exact plan? [y/N]: ")?.as_str(),
        "y" | "Y" | "yes" | "YES"
    ))
}

struct LifecycleLock {
    file: fs::File,
    path: PathBuf,
}

impl LifecycleLock {
    fn acquire(graph: &Path) -> Result<Self> {
        let path = graph.join("concierge.lock");
        let started = Instant::now();
        loop {
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&path)?;
            #[cfg(unix)]
            {
                use std::os::fd::AsRawFd;
                let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
                if rc == 0 {
                    return Ok(Self { file, path });
                }
            }
            #[cfg(not(unix))]
            return Ok(Self { file, path });
            if started.elapsed() > Duration::from_secs(120) {
                anyhow::bail!("Another worksgood setup/reconcile is still active");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for LifecycleLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
        let _ = &self.path;
    }
}

fn apply_selection(graph: &Path, prepared: &PreparedPlan) -> Result<()> {
    let Some(selection) = &prepared.public.selection else {
        return Ok(());
    };
    if let Some(content) = prepared.generated_content.as_deref() {
        project::apply_generated_project_selection(graph, selection, content)?;
    } else {
        project::apply_project_selection(graph, selection)?;
    }
    Ok(())
}

fn validate_prepared_readiness(selection: &project::ProjectSelectionPlan) -> Result<()> {
    for handler in &selection.readiness.handlers {
        if !handler.installed {
            anyhow::bail!(
                "Selected '{}' profile is not ready: {} handler is unavailable. Authenticate/install with that handler's own setup, then rerun `worksgood setup`; no fallback was selected.",
                selection.profile,
                handler.handler
            );
        }
        if handler.handler == "native" && handler.endpoint_status == "not configured" {
            anyhow::bail!(
                "Selected '{}' profile is not ready: its Nex/local endpoint is not configured. Update that reusable profile with the handler-owned endpoint picker, then rerun `worksgood setup`; no fallback was selected.",
                selection.profile
            );
        }
        if handler.handler == "pi" && !crate::pi_plugin::status().ready {
            anyhow::bail!(
                "Selected '{}' profile is not ready: the compatible Pi plugin could not be prepared",
                selection.profile
            );
        }
    }
    Ok(())
}

fn verify_project_selection(graph: &Path, expected: &project::ProjectSelectionPlan) -> Result<()> {
    let inspection = project::inspect_association(graph);
    if inspection.state != project::AssociationState::Ready
        || inspection
            .association
            .as_ref()
            .map(|a| a.profile_fingerprint.as_str())
            != Some(expected.profile_fingerprint.as_str())
    {
        anyhow::bail!("Project profile validation failed: {}", inspection.message);
    }
    let selection = crate::execution_selection::require(graph, None, "worksgood service")?;
    if selection.route.is_none() {
        anyhow::bail!("Selected profile produced no handler-first route");
    }
    Ok(())
}

fn wait_for_expected_service(
    graph: &Path,
    expected: &ServiceIdentity,
) -> Result<ServiceObservation> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let observed = observe_service(graph);
        if observed.health == ServiceHealth::Healthy
            && observed
                .handshake_identity
                .as_ref()
                .is_some_and(|actual| service_identity_equivalent(actual, expected))
        {
            return Ok(observed);
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "Service identity handshake did not converge: {}",
                observed.detail
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn authenticated_prior_executable(identity: &ServiceIdentity) -> Result<PathBuf> {
    let raw = PathBuf::from(&identity.executable);
    if !raw.is_absolute()
        || fs::symlink_metadata(&raw)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(true)
    {
        anyhow::bail!("Prior service executable is not an absolute regular candidate");
    }
    let canonical = raw.canonicalize()?;
    if canonical != raw || !is_executable_file(&canonical) {
        anyhow::bail!("Prior service executable identity changed on disk");
    }
    if executable_sha256(&canonical)? != identity.executable_sha256 {
        anyhow::bail!("Prior service executable bytes changed after handshake");
    }
    Ok(canonical)
}

fn recover_prior_service(
    graph: &Path,
    prior_identity: &ServiceIdentity,
    failed_start: &anyhow::Error,
) -> Result<()> {
    if prior_identity.protocol != crate::service_identity::SERVICE_IDENTITY_PROTOCOL {
        anyhow::bail!(
            "Intended replacement failed ({failed_start:#}); prior protocol is incompatible, so no prior-build recovery was attempted"
        );
    }
    let prior = authenticated_prior_executable(prior_identity).with_context(|| {
        format!(
            "Intended replacement failed ({failed_start:#}); prior executable is not recoverable"
        )
    })?;
    run_w(&prior, graph, &["service", "start"]).with_context(|| {
        format!(
            "Intended replacement failed ({failed_start:#}); prior build {} also failed to start",
            prior.display()
        )
    })?;
    let config = Config::load_merged(graph)?;
    let expected_prior = expected_identity(graph, &prior, &config)?;
    wait_for_expected_service(graph, &expected_prior)?;
    anyhow::bail!(
        "Intended service replacement failed ({failed_start:#}). Authenticated prior build {} was restored; TUI was not opened.",
        prior.display()
    )
}

fn apply_reconcile(
    graph: &Path,
    executable: &ExecutableAuthority,
    mode: &ConciergeMode,
    already_confirmed: bool,
) -> Result<Option<ServiceObservation>> {
    let observation = observe_service(graph);
    let expected = if *mode == ConciergeMode::Profile {
        let config = Config::load_merged(graph)?;
        Some(expected_identity(graph, &executable.executable, &config)?)
    } else {
        None
    };
    let decision = reconcile_decision(&observation, expected.as_ref(), mode);
    println!(
        "Service reconcile: {:?} — {}",
        decision.action, decision.reason
    );
    match decision.action {
        ReconcileAction::Reuse => {}
        ReconcileAction::Start | ReconcileAction::RepairAndStart => {
            run_w(&executable.executable, graph, &["service", "start"])?;
        }
        ReconcileAction::Reload => {
            run_w(&executable.executable, graph, &["service", "reload"])?;
        }
        ReconcileAction::ControlledRestart => {
            let expected = expected
                .as_ref()
                .context("Controlled restart omitted intended service identity")?;
            if !already_confirmed {
                println!("Service identity mismatch:");
                if let Some(actual) = observation.handshake_identity.as_ref() {
                    println!(
                        "  actual:   executable={} build={} protocol={} config={} profile={:?}/{:?} graph={}",
                        actual.executable,
                        actual.build_id,
                        actual.protocol,
                        actual.config_fingerprint,
                        actual.selected_profile,
                        actual.selected_profile_fingerprint,
                        actual.graph_digest
                    );
                } else {
                    println!("  actual:   {}", observation.detail);
                }
                println!(
                    "  intended: executable={} build={} protocol={} config={} profile={:?}/{:?} graph={}",
                    expected.executable,
                    expected.build_id,
                    expected.protocol,
                    expected.config_fingerprint,
                    expected.selected_profile,
                    expected.selected_profile_fingerprint,
                    expected.graph_digest
                );
                let yes = read_answer(
                    "Controlled restart this graph's daemon? Running agents/evals/chats remain detached. [y/N]: ",
                )?;
                if !matches!(yes.as_str(), "y" | "Y" | "yes" | "YES") {
                    anyhow::bail!("Restart cancelled; service and TUI were left untouched");
                }
            }
            // Split stop/start so a failed intended replacement can restore the
            // exact prior executable proven by the pre-stop socket handshake.
            let prior = observation.handshake_identity.clone();
            run_w(&executable.executable, graph, &["service", "stop"])?;
            if let Err(error) = run_w(&executable.executable, graph, &["service", "start"])
                .and_then(|_| wait_for_expected_service(graph, expected).map(|_| ()))
            {
                if let Some(prior) = prior.as_ref() {
                    return recover_prior_service(graph, prior, &error).map(|_| None);
                }
                return Err(error).context(
                    "Intended service replacement failed and no authenticated prior build was available",
                );
            }
        }
        ReconcileAction::StopForNoAi => {
            run_w(&executable.executable, graph, &["service", "stop"])?;
            return Ok(None);
        }
        ReconcileAction::None => return Ok(None),
        ReconcileAction::Refuse => {
            anyhow::bail!(
                "SERVICE_IDENTITY_REFUSED: {}; service was not signalled and TUI was not opened",
                decision.reason
            );
        }
    }
    let expected = expected
        .as_ref()
        .context("Service action omitted intended service identity")?;
    wait_for_expected_service(graph, expected).map(Some)
}

fn open_tui(executable: &ExecutableAuthority, graph: &Path) -> Result<()> {
    run_w(&executable.executable, graph, &["tui"])
}

fn lifecycle_message(
    mode: &ConciergeMode,
    observation: Option<&ServiceObservation>,
    readiness: Option<&project::ProfileReadiness>,
) {
    println!("\nTUI closed.");
    if *mode == ConciergeMode::ContinueWithoutAi {
        println!("Continue without AI: no LLM service is running.");
    } else {
        if let Some(readiness) = readiness {
            println!(
                "Resolved Worker/chat: {} (effort {} [{}])",
                readiness.strong_route,
                reasoning_label(readiness.strong_reasoning),
                readiness.strong_reasoning_provenance,
            );
            println!(
                "Resolved Agency/FLIP/evaluation: {} (effort {} [{}])",
                readiness.weak_route,
                reasoning_label(readiness.weak_reasoning),
                readiness.weak_reasoning_provenance,
            );
        }
        if let Some(pid) = observation.and_then(|o| o.state.as_ref().map(|s| s.pid)) {
            println!("Service remains detached and running (authenticated PID {pid}).");
            println!("Status:   worksgood status");
            println!("Stop:     worksgood stop       # detached agents/evals/chats continue");
        }
    }
    println!("Re-enter: worksgood");
    println!("Setup:    worksgood setup");
    println!("TUI only: worksgood tui        # no setup or service reconcile");
}

fn configured_fast_path(
    target: &RepositoryTarget,
    executable: &ExecutableAuthority,
    state: ConciergeState,
    options: &LifecycleOptions,
) -> Result<()> {
    match state.mode {
        ConciergeMode::ContinueWithoutAi => {
            let observation = observe_service(&target.graph);
            if !matches!(
                observation.health,
                ServiceHealth::Down | ServiceHealth::StalePid
            ) {
                anyhow::bail!(
                    "This project is committed Continue without AI but a service is present; run `worksgood stop` or `worksgood setup`"
                );
            }
            if options.open_tui {
                open_tui(executable, &target.graph)?;
                lifecycle_message(&ConciergeMode::ContinueWithoutAi, None, None);
            }
        }
        ConciergeMode::Profile => {
            let inspection = project::inspect_association(&target.graph);
            if inspection.state != project::AssociationState::Ready
                || inspection.association.as_ref().map(|a| a.profile.as_str())
                    != state.profile.as_deref()
                || inspection
                    .association
                    .as_ref()
                    .map(|a| a.profile_fingerprint.as_str())
                    != state.profile_fingerprint.as_deref()
            {
                anyhow::bail!(
                    "Committed project profile is not ready: {}. Run `worksgood setup`.",
                    inspection.message
                );
            }
            let profile = state.profile.as_deref().ok_or_else(|| {
                anyhow::anyhow!("Committed profile mode omitted profile identity")
            })?;
            let readiness = project::plan_project_selection(&target.graph, profile)?;
            validate_prepared_readiness(&readiness).with_context(|| {
                format!("Committed profile '{profile}' is no longer ready; run `worksgood setup`")
            })?;
            let _lock = LifecycleLock::acquire(&target.graph)?;
            let service =
                apply_reconcile(&target.graph, executable, &ConciergeMode::Profile, false)?;
            drop(_lock);
            if options.open_tui {
                open_tui(executable, &target.graph)?;
                lifecycle_message(
                    &ConciergeMode::Profile,
                    service.as_ref(),
                    Some(&readiness.readiness),
                );
            }
        }
    }
    Ok(())
}

pub fn run_lifecycle(options: &LifecycleOptions, force_setup: bool) -> Result<()> {
    if !options.dry_run && (!std::io::stdin().is_terminal() || !std::io::stdout().is_terminal()) {
        anyhow::bail!(
            "error[{ATTENDED_TTY_REQUIRED}]: bare/setup worksgood requires an attended TTY; no files or services were changed"
        );
    }
    let target = resolve_repository(options.project.as_deref())?;
    let executable = resolve_authoritative_executable()?;
    if target.graph_exists && !force_setup && !options.dry_run {
        if let Some(state) = read_state(&target.graph)? {
            return configured_fast_path(&target, &executable, state, options);
        }
    }

    println!("WorksGood can open a graph without an AI account.");
    println!(
        "LLM work additionally needs one explicit profile, that handler's own authentication/endpoint, any required integration, and a ready service."
    );
    println!("Repository: {}", target.repository.display());
    println!("Graph:      {}", target.graph.display());
    println!(
        "WorksGood:  {} ({}, {}, {})",
        executable.executable.display(),
        executable.build_id,
        executable.sha256,
        executable.authority
    );
    let (mode, profile) = choose_mode_and_profile(&target, options)?;
    let prepared = prepare_plan(&target, &executable, options, mode.clone(), profile)?;
    print_plan(&prepared.public)?;
    if options.dry_run {
        println!(
            "Dry run: no graph, profile, history, journal, cache, service, or TUI state was written."
        );
        return Ok(());
    }
    if !confirm_plan(options)? {
        println!("Cancelled; nothing was written.");
        return Ok(());
    }

    if !target.graph_exists {
        // Existing init remains the sole graph owner; suppress its advanced-CLI
        // route terminology so the primary concierge phrase stays
        // "Continue without AI" rather than presenting a second choice name.
        run_w_quiet(&executable.executable, &target.graph, &["init"])?;
        println!("Initialized WorksGood graph at {}", target.graph.display());
    }
    let _lock = LifecycleLock::acquire(&target.graph)?;
    let pending = PendingTransaction {
        version: STATE_VERSION,
        plan_digest: digest_json(&prepared.public)?,
        mode: mode.clone(),
        profile: prepared
            .public
            .selection
            .as_ref()
            .map(|p| p.profile.clone()),
        profile_fingerprint: prepared
            .public
            .selection
            .as_ref()
            .map(|p| p.profile_fingerprint.clone()),
        previous_association: project::read_association(&target.graph)?,
        graph_created: !target.graph_exists,
        service_was_down: matches!(
            prepared.public.service_observation.health,
            ServiceHealth::Down | ServiceHealth::StalePid
        ),
    };
    write_pending(&target.graph, &pending)?;
    if let Some(clear) = prepared.public.clear_selection.as_ref() {
        project::apply_clear_project_selection(&target.graph, clear)?;
    }
    if let Some(selection) = prepared.public.selection.as_ref() {
        if selection
            .readiness
            .handlers
            .iter()
            .any(|handler| handler.handler == "pi")
        {
            crate::pi_plugin::ensure_pi_plugin(crate::pi_plugin::EnsureMode::Console).context(
                "Pi plugin preparation failed; graph is initialized and setup may be resumed",
            )?;
        }
        validate_prepared_readiness(selection).context(
            "Selected handler readiness failed; graph is initialized and setup may be resumed",
        )?;
        apply_selection(&target.graph, &prepared)
            .context("Profile apply failed; graph is initialized and setup may be resumed")?;
        verify_project_selection(&target.graph, selection)?;
    }
    let service = apply_reconcile(&target.graph, &executable, &mode, true)?;
    let state = ConciergeState {
        version: STATE_VERSION,
        mode: mode.clone(),
        profile: prepared
            .public
            .selection
            .as_ref()
            .map(|p| p.profile.clone()),
        profile_fingerprint: prepared
            .public
            .selection
            .as_ref()
            .map(|p| p.profile_fingerprint.clone()),
        plan_digest: digest_json(&prepared.public)?,
        executable_sha256: executable.sha256.clone(),
    };
    write_state(&target.graph, &state)?;
    if pending_path(&target.graph).exists() {
        fs::remove_file(pending_path(&target.graph))?;
    }
    if prepared.public.selection.is_some() {
        let _ = project::record_successful_event(
            &target.graph,
            project::UsageEventCategory::ConfigApplied,
        );
    }
    drop(_lock);
    if options.open_tui {
        open_tui(&executable, &target.graph)?;
        lifecycle_message(
            &mode,
            service.as_ref(),
            prepared
                .public
                .selection
                .as_ref()
                .map(|selection| &selection.readiness),
        );
    }
    Ok(())
}

/// Roll back only a confirmed-but-uncommitted concierge transaction. Graph
/// initialization and handler-owned credentials/plugins are intentionally
/// preserved; exact project selection and a daemon started from an initially
/// down state are reversible. A changed association or committed lifecycle
/// fails closed rather than deleting later work.
pub fn run_rollback(project_path: Option<&Path>) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("error[{ATTENDED_TTY_REQUIRED}]: rollback requires an attended TTY");
    }
    let target = resolve_repository(project_path)?;
    if !target.graph_exists {
        println!("Nothing to roll back; the graph is not initialized.");
        return Ok(());
    }
    if read_state(&target.graph)?.is_some() {
        anyhow::bail!(
            "The concierge transaction is already committed. Use `worksgood setup` to choose another profile or Continue without AI."
        );
    }
    let pending = read_pending(&target.graph)?
        .ok_or_else(|| anyhow::anyhow!("No pending concierge transaction to roll back"))?;
    let executable = resolve_authoritative_executable()?;
    println!("Rollback plan:");
    println!("  pending plan: {}", pending.plan_digest);
    println!(
        "  graph: {} (initialization is preserved)",
        target.graph.display()
    );
    println!("  handler-owned authentication/plugin state is preserved");
    if let Some(profile) = pending.profile.as_deref() {
        println!("  clear exact pending project selection: {profile}");
    }
    if let Some(previous) = pending.previous_association.as_ref() {
        println!("  restore previous project selection: {}", previous.profile);
    }
    if pending.service_was_down {
        println!("  stop a daemon started by this pending transaction, if present");
    }
    let answer = read_answer("Apply rollback? [y/N]: ")?;
    if !matches!(answer.as_str(), "y" | "Y" | "yes" | "YES") {
        println!("Cancelled; pending transaction was not changed.");
        return Ok(());
    }
    let _lock = LifecycleLock::acquire(&target.graph)?;
    let current = project::read_association(&target.graph)?;
    let current_is_new = match (current.as_ref(), pending.profile.as_deref()) {
        (Some(current), Some(profile)) => {
            current.profile == profile
                && Some(current.profile_fingerprint.as_str())
                    == pending.profile_fingerprint.as_deref()
        }
        _ => false,
    };
    let current_is_previous = current == pending.previous_association;
    if !current_is_new && !current_is_previous && current.is_some() {
        anyhow::bail!("Project selection changed after the failed transaction; refusing rollback");
    }
    if current_is_new {
        let clear = project::plan_clear_project_selection(&target.graph)?;
        project::apply_clear_project_selection(&target.graph, &clear)?;
    }
    if let Some(previous) = pending.previous_association.as_ref()
        && project::read_association(&target.graph)?.is_none()
    {
        let restore = project::plan_project_selection(&target.graph, &previous.profile)?;
        if restore.profile_fingerprint != previous.profile_fingerprint {
            anyhow::bail!(
                "Previous reusable profile changed; refusing to restore drifted selection"
            );
        }
        project::apply_project_selection(&target.graph, &restore)?;
    }
    if pending.service_was_down {
        let observation = observe_service(&target.graph);
        if !matches!(
            observation.health,
            ServiceHealth::Down | ServiceHealth::StalePid
        ) {
            run_w(&executable.executable, &target.graph, &["service", "stop"])?;
        }
    }
    fs::remove_file(pending_path(&target.graph))?;
    println!(
        "Rolled back project selection/service effects. Initialized graph and handler-owned prerequisites remain; rerun `worksgood setup` to resume."
    );
    Ok(())
}

pub fn run_status(project_path: Option<&Path>) -> Result<()> {
    let target = resolve_repository(project_path)?;
    let executable = resolve_authoritative_executable()?;
    println!("Repository: {}", target.repository.display());
    println!("Graph: {}", target.graph.display());
    println!(
        "WorksGood executable: {} ({}, {})",
        executable.executable.display(),
        executable.build_id,
        executable.sha256
    );
    if !target.graph_exists {
        println!("Setup: not initialized");
        println!("Service: down");
        return Ok(());
    }
    match read_state(&target.graph)? {
        Some(state) => println!(
            "Project lifecycle: {:?}{}",
            state.mode,
            state
                .profile
                .as_deref()
                .map(|p| format!(" ({p})"))
                .unwrap_or_default()
        ),
        None => println!("Project lifecycle: not configured by the worksgood trial"),
    }
    if let Some(pending) = read_pending(&target.graph)? {
        println!(
            "Recovery: pending plan {} — resume with `worksgood setup` or inspect `worksgood setup --rollback`",
            pending.plan_digest
        );
    }
    let inspection = project::inspect_association(&target.graph);
    println!(
        "Project profile: {:?} — {}",
        inspection.state, inspection.message
    );
    if inspection.state == project::AssociationState::Ready
        && let Some(profile) = inspection
            .association
            .as_ref()
            .map(|association| association.profile.as_str())
    {
        let readiness = project::plan_project_selection(&target.graph, profile)?.readiness;
        println!(
            "Resolved effort: Worker/chat {} [{}] · Agency/FLIP/Eval {} [{}]",
            readiness
                .strong_reasoning
                .map(ReasoningLevel::as_str)
                .unwrap_or("(omit)"),
            readiness.strong_reasoning_provenance,
            readiness
                .weak_reasoning
                .map(ReasoningLevel::as_str)
                .unwrap_or("(omit)"),
            readiness.weak_reasoning_provenance,
        );
    }
    let observation = observe_service(&target.graph);
    println!("Service: {:?} — {}", observation.health, observation.detail);
    if let Some(identity) = observation.handshake_identity {
        println!(
            "Service identity: pid={} graph={} build={} protocol={} config={} profile={:?}/{:?} executable={} socket={}",
            observation.state.as_ref().map(|s| s.pid).unwrap_or(0),
            identity.graph_digest,
            identity.build_id,
            identity.protocol,
            identity.config_fingerprint,
            identity.selected_profile,
            identity.selected_profile_fingerprint,
            identity.executable,
            observation
                .state
                .as_ref()
                .map(|s| s.socket_path.as_str())
                .unwrap_or("unknown")
        );
    }
    Ok(())
}

pub fn run_stop(project_path: Option<&Path>) -> Result<()> {
    let target = resolve_repository(project_path)?;
    if !target.graph_exists {
        println!("Service already down; graph is not initialized.");
        return Ok(());
    }
    let executable = resolve_authoritative_executable()?;
    let observation = observe_service(&target.graph);
    if matches!(
        observation.health,
        ServiceHealth::Down | ServiceHealth::StalePid
    ) {
        println!("Service already down ({}).", observation.detail);
        return Ok(());
    }
    run_w(&executable.executable, &target.graph, &["service", "stop"])?;
    println!(
        "Stopped this graph's daemon gracefully. Detached agents/evals/chats were not killed."
    );
    Ok(())
}

pub fn run_restart(project_path: Option<&Path>, yes: bool) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("error[{ATTENDED_TTY_REQUIRED}]: restart requires an attended TTY");
    }
    let target = resolve_repository(project_path)?;
    if !target.graph_exists {
        anyhow::bail!("Graph is not initialized; run worksgood setup");
    }
    let executable = resolve_authoritative_executable()?;
    println!("Explicit restart will replace only this graph's daemon.");
    println!(
        "Running agents, inline evaluators/FLIP, chats, and PTYs remain detached and are not killed."
    );
    if !yes {
        let answer = read_answer("Restart now? [y/N]: ")?;
        if !matches!(answer.as_str(), "y" | "Y" | "yes" | "YES") {
            println!("Cancelled; service was not changed.");
            return Ok(());
        }
    }
    run_w(
        &executable.executable,
        &target.graph,
        &["service", "restart"],
    )?;
    let config = Config::load_merged(&target.graph)?;
    let expected = expected_identity(&target.graph, &executable.executable, &config)?;
    let observed = wait_for_expected_service(&target.graph, &expected)?;
    println!("Restarted and authenticated: {}", observed.detail);
    Ok(())
}

pub fn run_tui(project_path: Option<&Path>) -> Result<()> {
    let target = resolve_repository(project_path)?;
    if !target.graph_exists {
        anyhow::bail!(
            "Graph is not initialized. `worksgood tui` is setup-neutral; run `worksgood setup` first."
        );
    }
    let executable = resolve_authoritative_executable()?;
    open_tui(&executable, &target.graph)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn repository_resolution_is_nearest_and_never_global() {
        let temp = TempDir::new().unwrap();
        fs::create_dir(temp.path().join(".git")).unwrap();
        let nested = temp.path().join("a/b");
        fs::create_dir_all(&nested).unwrap();
        let target = resolve_repository(Some(&nested)).unwrap();
        assert_eq!(target.repository, temp.path().canonicalize().unwrap());
        assert_eq!(
            target.graph,
            temp.path().canonicalize().unwrap().join(".wg")
        );
    }

    #[test]
    fn missing_graph_can_build_project_profile_plan_without_writing() {
        let temp = TempDir::new().unwrap();
        fs::create_dir(temp.path().join(".git")).unwrap();
        let graph = temp.path().join(".wg");
        let before = fs::read_dir(temp.path()).unwrap().count();
        let _plan = project::plan_project_selection(&graph, "codex").unwrap();
        assert_eq!(before, fs::read_dir(temp.path()).unwrap().count());
        assert!(!graph.exists());
    }

    #[test]
    fn pi_mock_catalog_is_pi_shaped_and_exact() {
        let parsed = parse_mock_pi_models(
            r#"{"models":[{"provider":"openai-codex","id":"gpt-5.6-sol","name":"GPT","reasoning":true}]}"#,
        )
        .unwrap();
        assert_eq!(
            route_for_pi_model(&parsed[0]),
            "pi:openai-codex:gpt-5.6-sol"
        );
    }

    #[test]
    fn generated_two_tier_content_never_silently_infers_weak() {
        let content = patch_two_tier_content(
            named::STARTER_PI.to_string(),
            "pi:openai-codex:gpt-5.6-sol",
            "pi:openrouter:deepseek/deepseek-chat",
            ReasoningLevel::Xhigh,
            ReasoningLevel::Low,
        )
        .unwrap();
        let config: Config = toml::from_str(&content).unwrap();
        let (strong, weak) = config.pi_tiers();
        assert_eq!(strong.as_deref(), Some("pi:openai-codex:gpt-5.6-sol"));
        assert_eq!(
            weak.as_deref(),
            Some("pi:openrouter:deepseek/deepseek-chat")
        );
    }

    #[test]
    fn core_profile_persists_separate_default_effort_without_model_overrides() {
        let content = customize_core_profile(
            "codex",
            named::STARTER_CODEX.to_string(),
            &LifecycleOptions {
                requested_profile: Some("codex".to_string()),
                yes: true,
                ..LifecycleOptions::default()
            },
        )
        .unwrap()
        .expect("core profile must materialize explicit effort");
        let config: Config = toml::from_str(&content).unwrap();
        assert_eq!(
            config.resolve_reasoning_for_role(crate::config::DispatchRole::TaskAgent),
            Some(ReasoningLevel::High)
        );
        assert_eq!(
            config.resolve_reasoning_for_role(crate::config::DispatchRole::Evaluator),
            Some(ReasoningLevel::Low)
        );
    }

    #[test]
    fn noninteractive_effort_preserves_configured_but_never_chooses_missing_value() {
        assert_eq!(
            resolve_effort_choice(
                "Worker/chat",
                None,
                Some(ReasoningLevel::Xhigh),
                ReasoningLevel::High,
                true,
                "--strong-reasoning",
            )
            .unwrap(),
            ReasoningLevel::Xhigh
        );
        let error = resolve_effort_choice(
            "Agency/FLIP/evaluation",
            None,
            None,
            ReasoningLevel::Low,
            true,
            "--weak-reasoning",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("--yes does not choose"));
        assert!(error.contains("--weak-reasoning"));

        for level in ReasoningLevel::ALL {
            assert_eq!(
                resolve_effort_choice(
                    "Worker/chat",
                    Some(*level),
                    None,
                    ReasoningLevel::High,
                    true,
                    "--strong-reasoning",
                )
                .unwrap(),
                *level
            );
        }
    }

    #[test]
    fn core_profile_yes_requires_missing_effort_instead_of_silent_defaults() {
        let error = customize_core_profile(
            "claude",
            named::STARTER_CLAUDE.to_string(),
            &LifecycleOptions {
                requested_profile: Some("claude".to_string()),
                yes: true,
                ..LifecycleOptions::default()
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("--strong-reasoning"));
    }

    fn healthy_observation(identity: ServiceIdentity) -> ServiceObservation {
        ServiceObservation {
            health: ServiceHealth::Healthy,
            state: None,
            handshake_identity: Some(identity),
            detail: "test identity".to_string(),
        }
    }

    #[test]
    fn reconcile_reuses_identical_bytes_across_absolute_path_aliases() {
        let temp = TempDir::new().unwrap();
        let graph = temp.path().join(".wg");
        fs::create_dir(&graph).unwrap();
        let expected_exe = temp.path().join("wg-current");
        let alias = temp.path().join("wg-hardlink");
        fs::write(&expected_exe, b"same-build").unwrap();
        fs::hard_link(&expected_exe, &alias).unwrap();
        let expected = expected_identity(&graph, &expected_exe, &Config::default()).unwrap();
        let mut actual = expected.clone();
        actual.executable = alias.display().to_string();
        let decision = reconcile_decision(
            &healthy_observation(actual),
            Some(&expected),
            &ConciergeMode::Profile,
        );
        assert_eq!(decision.action, ReconcileAction::Reuse);
    }

    #[test]
    fn reconcile_reloads_only_compatible_config_profile_generation_drift() {
        let temp = TempDir::new().unwrap();
        let graph = temp.path().join(".wg");
        fs::create_dir(&graph).unwrap();
        let exe = temp.path().join("wg");
        fs::write(&exe, b"same-build").unwrap();
        let expected = expected_identity(&graph, &exe, &Config::default()).unwrap();
        let mut actual = expected.clone();
        actual.config_fingerprint = "sha256:old-config".to_string();
        let decision = reconcile_decision(
            &healthy_observation(actual),
            Some(&expected),
            &ConciergeMode::Profile,
        );
        assert_eq!(decision.action, ReconcileAction::Reload);
        assert!(decision.reason.contains("generation drift"));
    }

    #[test]
    fn reconcile_restarts_same_path_with_different_content_build() {
        let temp = TempDir::new().unwrap();
        let graph = temp.path().join(".wg");
        fs::create_dir(&graph).unwrap();
        let exe = temp.path().join("wg");
        fs::write(&exe, b"new-build").unwrap();
        let expected = expected_identity(&graph, &exe, &Config::default()).unwrap();
        let mut actual = expected.clone();
        actual.executable_sha256 = format!("sha256:{}", "1".repeat(64));
        actual.build_id = crate::service_identity::build_id(&actual.executable_sha256);
        let decision = reconcile_decision(
            &healthy_observation(actual),
            Some(&expected),
            &ConciergeMode::Profile,
        );
        assert_eq!(decision.action, ReconcileAction::ControlledRestart);
        assert!(decision.reason.contains("binary/build/protocol mismatch"));
    }

    #[test]
    fn reconcile_refuses_foreign_or_deleted_running_identity_without_signal() {
        let temp = TempDir::new().unwrap();
        let graph = temp.path().join(".wg");
        fs::create_dir(&graph).unwrap();
        let exe = temp.path().join("wg");
        fs::write(&exe, b"intended").unwrap();
        let expected = expected_identity(&graph, &exe, &Config::default()).unwrap();
        let foreign = ServiceObservation {
            health: ServiceHealth::Foreign,
            state: None,
            handshake_identity: Some(expected.clone()),
            detail: "foreign graph".to_string(),
        };
        assert_eq!(
            reconcile_decision(&foreign, Some(&expected), &ConciergeMode::Profile).action,
            ReconcileAction::Refuse
        );

        let missing = temp.path().join("deleted-running-wg");
        let mut actual = expected.clone();
        actual.executable = missing.display().to_string();
        let decision = reconcile_decision(
            &healthy_observation(actual),
            Some(&expected),
            &ConciergeMode::Profile,
        );
        assert_eq!(decision.action, ReconcileAction::Refuse);
        assert!(
            decision
                .reason
                .contains("cannot authenticate running executable path")
        );
    }

    #[test]
    fn down_observation_is_read_only() {
        let temp = TempDir::new().unwrap();
        let before = fs::read_dir(temp.path()).unwrap().count();
        assert_eq!(observe_service(temp.path()).health, ServiceHealth::Down);
        assert_eq!(before, fs::read_dir(temp.path()).unwrap().count());
    }
}
