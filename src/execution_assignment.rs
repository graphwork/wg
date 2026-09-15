//! Opt-in immutable execution assignment experiment.
//!
//! The stable dispatcher still uses `SpawnPlan`.  When
//! `WG_EXPERIMENTAL_OPAQUE_ASSIGNMENT=1`, the service resolves authoring policy
//! once into this type and the runtime consumes these bytes.  The Pi envelope
//! is recognized exactly once; the suffix is opaque and is never split into a
//! WG provider/model pair.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::{Config, DispatchRole, ReasoningLevel};
use crate::graph::Task;

pub const EXPERIMENT_ENV: &str = "WG_EXPERIMENTAL_OPAQUE_ASSIGNMENT";
pub const ASSIGNMENT_COMPONENT: &str = "execution-assignment";
pub const ASSIGNMENT_FILE: &str = "assignment.json";
const PREFLIGHT_BACKOFF_FILE: &str = "opaque-preflight-backoff.json";
const TRANSIENT_BACKOFF_BASE_SECS: u64 = 5;
const PREFLIGHT_BACKOFF_CAP_SECS: u64 = 60;
pub const PI_PROCESSES_PACKAGE: &str = "@mjakl/pi-processes";
pub const PI_PROCESSES_VERSION: &str = "2.0.0";

pub fn experiment_enabled() -> bool {
    std::env::var(EXPERIMENT_ENV)
        .ok()
        .is_some_and(|value| matches!(value.trim(), "1" | "true" | "yes" | "on"))
}

/// Non-secret identity of one path consumed by a Pi invocation. The digest
/// authenticates bytes without serializing settings, extension source, or
/// credentials into an assignment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinnedPathIdentity {
    pub path: PathBuf,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PiInvocationKind {
    WorkerJson,
    HermeticReview,
    ManagedProcessRpc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PiWorkingDirectoryPolicy {
    /// Capability query runs in `capability_cwd`; execution is bound to the
    /// exact attempt workspace when the assignment gains attempt authority.
    AttemptWorkspace,
    Exact,
}

/// Complete non-secret Pi invocation policy. Capability and execution argv
/// are separate fields so `--offline --list-models` is an explicit phase
/// delta rather than a hidden second resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiLaunchPlan {
    pub invocation_kind: PiInvocationKind,
    pub executable: PinnedPathIdentity,
    pub fixed_argv: Vec<String>,
    pub capability_argv: Vec<String>,
    pub config_root: PathBuf,
    pub config_identity: String,
    pub executor_config: PinnedPathIdentity,
    pub extension_policy: String,
    pub extension_identities: Vec<PinnedPathIdentity>,
    pub tool_policy: String,
    pub capability_cwd: PathBuf,
    pub working_directory_policy: PiWorkingDirectoryPolicy,
    /// Exact review cwd, or the resolved worker attempt workspace once bound.
    pub execution_cwd: Option<PathBuf>,
    pub prompt_policy: String,
    pub session_policy: String,
    pub timeout_secs: Option<u64>,
    pub cancellation_grace_secs: u64,
    pub network_policy: String,
    /// Pi owns credential lookup and token refresh. This descriptive boundary
    /// is intentionally stable while auth bytes remain mutable and unrecorded.
    pub authentication_boundary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_process_extension: Option<PinnedPathIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wg_extension: Option<PinnedPathIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wg_plugin_root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wg_plugin_compat: Option<String>,
}

/// Runtime selection. There are deliberately no provider, endpoint, registry,
/// handler, tier, or fallback fields in the Pi variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeExecution {
    Pi {
        /// Exact configured Pi executable retained for historical readers.
        program: PathBuf,
        /// Every byte after the single outer `pi:` envelope.
        opaque_route: String,
        reasoning: ReasoningLevel,
        /// `None` is an explicitly fresh session; `Some` pins exact resume id.
        session_id: Option<String>,
        /// Historical assignment files omitted the complete plan. They remain
        /// readable, but cannot be launched by the repaired experiment.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_plan: Option<Box<PiLaunchPlan>>,
    },
    Shell {
        /// Exact argv, including argv[0]. No shell parsing occurs after assignment.
        argv: Vec<String>,
        /// Exact explicit environment overlay. Ambient worker-control variables are
        /// added by the wrapper, but execution policy cannot add or rewrite entries.
        environment: std::collections::BTreeMap<String, String>,
        /// Exact directory in which the shell process is launched.
        working_directory: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionAssignment {
    /// Assignment envelope schema. The launch plan is an optional additive
    /// field within v1 so pre-plan historical records remain readable.
    pub schema: u32,
    pub task_id: String,
    /// Agency identity selected during authoring, or an explicit direct marker.
    pub agent_identity: String,
    pub role: DispatchRole,
    pub config_revision: String,
    /// Digest of every mutable task field consumed while authoring execution.
    pub authoring_fingerprint: String,
    pub authored_route: String,
    pub execution: RuntimeExecution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundExecutionAssignment {
    #[serde(flatten)]
    pub assignment: ExecutionAssignment,
    pub runtime_agent_id: String,
    pub generation: u64,
    pub attempt_id: String,
    pub attempt_fence: u64,
    /// Exact cwd resolved from the authored AttemptWorkspace policy. Historical
    /// files omit it and remain readable, but repaired launches always persist it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PiPreflightOutcome {
    Ready,
    MissingRequiredCapability {
        exit_code: Option<i32>,
        diagnostic: String,
    },
    TransientFailure {
        exit_code: Option<i32>,
        diagnostic: String,
    },
}

impl PiPreflightOutcome {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }

    pub fn diagnostic(&self) -> Option<&str> {
        match self {
            Self::Ready => None,
            Self::MissingRequiredCapability { diagnostic, .. }
            | Self::TransientFailure { diagnostic, .. } => Some(diagnostic),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PreflightBackoffState {
    schema: u32,
    entries: BTreeMap<String, PreflightBackoffEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PreflightBackoffEntry {
    failures: u32,
    next_probe_unix_ms: u64,
}

fn assignment_preflight_key(assignment: &ExecutionAssignment) -> String {
    let bytes = serde_json::to_vec(assignment).expect("assignment serializes");
    blake3::hash(&bytes).to_hex().to_string()
}

/// Stable coalescing identity for admission evidence about this exact plan.
/// It contains no route or credential bytes.
#[must_use]
pub fn preflight_notification_key(assignment: &ExecutionAssignment) -> String {
    assignment_preflight_key(assignment)
}

fn unix_ms(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn backoff_path(dir: &Path) -> PathBuf {
    dir.join("service").join(PREFLIGHT_BACKOFF_FILE)
}

fn load_backoff(dir: &Path) -> PreflightBackoffState {
    std::fs::read(backoff_path(dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| PreflightBackoffState {
            schema: 1,
            entries: BTreeMap::new(),
        })
}

fn save_backoff(dir: &Path, state: &PreflightBackoffState) -> Result<()> {
    let path = backoff_path(dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::atomic_file::write_atomic(&path, &serde_json::to_vec_pretty(state)?)
        .with_context(|| format!("persist opaque preflight backoff at {}", path.display()))
}

/// Return the remaining persisted admission delay for this exact assignment.
pub fn preflight_backoff_remaining(
    dir: &Path,
    assignment: &ExecutionAssignment,
) -> Option<Duration> {
    preflight_backoff_remaining_at(dir, assignment, SystemTime::now())
}

fn preflight_backoff_remaining_at(
    dir: &Path,
    assignment: &ExecutionAssignment,
    now: SystemTime,
) -> Option<Duration> {
    let state = load_backoff(dir);
    let entry = state.entries.get(&assignment_preflight_key(assignment))?;
    let now = unix_ms(now);
    (entry.next_probe_unix_ms > now).then(|| Duration::from_millis(entry.next_probe_unix_ms - now))
}

/// Persist bounded exponential retry authority for a failed Pi probe. Success
/// clears the exact assignment key. Configuration or task changes produce a
/// different key and are therefore immediately eligible.
pub fn record_preflight_outcome(
    dir: &Path,
    assignment: &ExecutionAssignment,
    outcome: &PiPreflightOutcome,
) -> Result<()> {
    record_preflight_outcome_at(dir, assignment, outcome, SystemTime::now())
}

fn record_preflight_outcome_at(
    dir: &Path,
    assignment: &ExecutionAssignment,
    outcome: &PiPreflightOutcome,
    now: SystemTime,
) -> Result<()> {
    let mut state = load_backoff(dir);
    state.schema = 1;
    let key = assignment_preflight_key(assignment);
    if outcome.is_ready() {
        if state.entries.remove(&key).is_some() {
            save_backoff(dir, &state)?;
        }
        return Ok(());
    }
    let prior = state
        .entries
        .get(&key)
        .map(|entry| entry.failures)
        .unwrap_or(0);
    let failures = prior.saturating_add(1);
    let delay = match outcome {
        PiPreflightOutcome::TransientFailure { .. } => TRANSIENT_BACKOFF_BASE_SECS
            .saturating_mul(1_u64.checked_shl(prior.min(16)).unwrap_or(u64::MAX))
            .min(PREFLIGHT_BACKOFF_CAP_SECS),
        PiPreflightOutcome::MissingRequiredCapability { .. } => PREFLIGHT_BACKOFF_CAP_SECS,
        PiPreflightOutcome::Ready => 0,
    };
    state.entries.insert(
        key,
        PreflightBackoffEntry {
            failures,
            next_probe_unix_ms: unix_ms(now).saturating_add(delay.saturating_mul(1000)),
        },
    );
    save_backoff(dir, &state)
}

fn digest_bytes(bytes: &[u8]) -> String {
    format!("b3:{}", blake3::hash(bytes).to_hex())
}

fn pinned_path(path: impl AsRef<Path>) -> PinnedPathIdentity {
    let path = pin_executable(path.as_ref());
    let digest = std::fs::read(&path)
        .map(|bytes| digest_bytes(&bytes))
        .unwrap_or_else(|_| "missing".to_string());
    PinnedPathIdentity { path, digest }
}

/// Validate the optional managed-process extension at the selection boundary.
///
/// This check intentionally happens while authoring and again during pinned-plan
/// verification, before any attempt or Pi invocation receives authority.
pub fn verify_managed_process_extension(entry: &Path) -> Result<()> {
    let entry = entry
        .canonicalize()
        .with_context(|| format!("canonicalize process extension {}", entry.display()))?;
    let package_json = entry
        .ancestors()
        .take(5)
        .map(|dir| dir.join("package.json"))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "WG-PI-PROCESS-EXTENSION-INVALID: {} has no enclosing package.json",
                entry.display()
            )
        })?;
    let package: serde_json::Value = serde_json::from_slice(&std::fs::read(&package_json)?)?;
    let name = package
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let version = package
        .get("version")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if name != PI_PROCESSES_PACKAGE || version != PI_PROCESSES_VERSION {
        bail!(
            "WG-PI-PROCESS-EXTENSION-MISMATCH: expected {}@{}, found {:?}@{:?} at {}",
            PI_PROCESSES_PACKAGE,
            PI_PROCESSES_VERSION,
            name,
            version,
            package_json.display()
        );
    }
    Ok(())
}

fn pi_config_root() -> PathBuf {
    let root = std::env::var_os("PI_CODING_AGENT_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".pi/agent")))
        .or_else(|| dirs::home_dir().map(|home| home.join(".pi/agent")))
        .unwrap_or_else(|| PathBuf::from(".pi/agent"));
    if root.is_absolute() {
        root
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(root)
    }
}

/// Digest only non-secret configuration surfaces which may change model
/// registration or invocation behavior. Authentication stores are deliberately
/// excluded so Pi can refresh OAuth tokens while an assignment is live.
fn pi_config_identity(root: &Path) -> String {
    let mut material = Vec::new();
    material.extend_from_slice(root.to_string_lossy().as_bytes());
    for relative in ["settings.json", "models.json", "models-store.json"] {
        material.extend_from_slice(relative.as_bytes());
        match std::fs::read(root.join(relative)) {
            Ok(bytes) => material.extend_from_slice(blake3::hash(&bytes).as_bytes()),
            Err(_) => material.extend_from_slice(b"missing"),
        }
    }
    digest_bytes(&material)
}

fn executor_config_identity(
    project_root: &Path,
    settings: &crate::service::executor::ExecutorSettings,
) -> PinnedPathIdentity {
    let path = project_root.join(".wg/executors/pi.toml");
    if path.is_file() {
        return pinned_path(path);
    }
    PinnedPathIdentity {
        path: PathBuf::from("<built-in-pi-executor>"),
        digest: digest_bytes(&serde_json::to_vec(settings).expect("executor settings serialize")),
    }
}

fn reserved_pi_flag(arg: &str) -> bool {
    let name = arg.split_once('=').map_or(arg, |(name, _)| name);
    matches!(
        name,
        "--provider"
            | "--model"
            | "-m"
            | "--thinking"
            | "--session"
            | "--session-id"
            | "--session-dir"
            | "--resume"
            | "-r"
            | "--continue"
            | "-c"
            | "--prompt"
            | "-p"
            | "--extension"
            | "-e"
            | "--no-extensions"
            | "-ne"
            | "--tools"
            | "-t"
            | "--no-tools"
            | "-nt"
            | "--no-builtin-tools"
            | "-nbt"
            | "--exclude-tools"
            | "-xt"
            | "--offline"
            | "--api-key"
            | "--no-session"
            | "--mode"
            | "--no-context-files"
            | "-nc"
            | "--skill"
            | "--no-skills"
            | "-ns"
            | "--prompt-template"
            | "--no-prompt-templates"
            | "-np"
    )
}

fn worker_launch_plan(
    task: &Task,
    config: &Config,
    program: &Path,
    project_root: &Path,
) -> Result<PiLaunchPlan> {
    let executor = crate::service::executor::ExecutorRegistry::new(&project_root.join(".wg"))
        .load_config("pi")?
        .executor;
    if executor.executor_type != "pi" {
        bail!(
            "error[WG-OPAQUE-ASSIGNMENT-CONFIG]: Pi executor config changed type to {:?}",
            executor.executor_type
        );
    }
    let pinned_program = pin_executable(program);
    let custom_executor_path = project_root.join(".wg/executors/pi.toml");
    if custom_executor_path.is_file()
        && pin_executable(Path::new(&executor.command)) != pinned_program
    {
        bail!(
            "error[WG-OPAQUE-ASSIGNMENT-CONFIG]: configured Pi executable changed while authoring (selected={} configured={})",
            pinned_program.display(),
            executor.command
        );
    }
    if executor
        .env
        .iter()
        .any(|(key, value)| key != "WG_TASK_ID" || value != "{{task_id}}")
    {
        bail!(
            "error[WG-OPAQUE-ASSIGNMENT-CONFIG]: experimental Pi executor env must remain wrapper-owned; custom env (which may contain secrets) is not serializable"
        );
    }
    let mut fixed_argv = Vec::new();
    let mut args = executor.args.iter();
    while let Some(arg) = args.next() {
        if matches!(arg.as_str(), "--prompt" | "-p") {
            let _ = args.next().ok_or_else(|| {
                anyhow::anyhow!(
                    "error[WG-OPAQUE-ASSIGNMENT-ARGV-CONFLICT]: configured prompt flag has no value"
                )
            })?;
            continue;
        }
        if arg == "--mode" {
            let value = args.next().ok_or_else(|| {
                anyhow::anyhow!(
                    "error[WG-OPAQUE-ASSIGNMENT-ARGV-CONFLICT]: configured --mode has no value"
                )
            })?;
            if value != "json" {
                bail!(
                    "error[WG-OPAQUE-ASSIGNMENT-ARGV-CONFLICT]: worker Pi mode must be json, got {value:?}"
                );
            }
            continue;
        }
        if reserved_pi_flag(arg) {
            bail!(
                "error[WG-OPAQUE-ASSIGNMENT-ARGV-CONFLICT]: configured Pi argv contains WG-owned selector/policy flag {arg:?}"
            );
        }
        fixed_argv.push(arg.clone());
    }
    let managed_process_extension = std::env::var_os("WG_PI_PROCESS_WAKE_EXTENSION")
        .map(PathBuf::from)
        .map(|path| {
            if !path.is_absolute() {
                bail!("error[WG-OPAQUE-ASSIGNMENT-CONFIG]: WG_PI_PROCESS_WAKE_EXTENSION must be an absolute path");
            }
            verify_managed_process_extension(&path)?;
            let identity = pinned_path(path);
            if identity.digest == "missing" {
                bail!("error[WG-OPAQUE-ASSIGNMENT-CONFIG]: managed process extension is unavailable at {}", identity.path.display());
            }
            Ok(identity)
        })
        .transpose()?;
    if managed_process_extension.is_some() {
        if !fixed_argv.is_empty() {
            bail!(
                "error[WG-OPAQUE-ASSIGNMENT-ARGV-CONFLICT]: managed-process Pi does not accept custom executor argv because the RPC adapter must own its complete invocation"
            );
        }
        fixed_argv.extend([
            "--mode".to_string(),
            "rpc".to_string(),
            "--no-approve".to_string(),
            "-ne".to_string(),
        ]);
    } else {
        fixed_argv.extend([
            "--mode".to_string(),
            "json".to_string(),
            "-ne".to_string(),
            "--no-skills".to_string(),
            "--no-prompt-templates".to_string(),
            "--no-context-files".to_string(),
        ]);
    }
    let config_root = pi_config_root();
    let task_timeout = task
        .timeout
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(|value| {
            crate::graph::parse_delay(value).ok_or_else(|| {
                anyhow::anyhow!(
                    "error[WG-OPAQUE-ASSIGNMENT-CONFIG]: invalid task timeout {value:?}"
                )
            })
        })
        .transpose()?;
    let coordinator_timeout = (!config.coordinator.agent_timeout.is_empty())
        .then(|| crate::graph::parse_delay(&config.coordinator.agent_timeout))
        .flatten();
    let timeout_secs = task_timeout.or(executor.timeout).or(coordinator_timeout);
    let wg_plugin = crate::pi_plugin::ensure_pi_plugin(crate::pi_plugin::EnsureMode::Hermetic)
        .context("prepare exact WG extension for opaque Pi worker assignment")?;
    let wg_extension = Some(pinned_path(&wg_plugin.dist_entry));
    let mut extension_identities: Vec<_> = managed_process_extension.clone().into_iter().collect();
    extension_identities.extend(wg_extension.clone());
    Ok(PiLaunchPlan {
        invocation_kind: if managed_process_extension.is_some() {
            PiInvocationKind::ManagedProcessRpc
        } else {
            PiInvocationKind::WorkerJson
        },
        executable: pinned_path(&pinned_program),
        fixed_argv,
        capability_argv: vec![
            "--offline".into(), "-ne".into(), "--no-skills".into(),
            "--no-prompt-templates".into(), "--no-context-files".into(),
        ],
        config_root: config_root.clone(),
        config_identity: pi_config_identity(&config_root),
        executor_config: executor_config_identity(project_root, &executor),
        extension_policy: if managed_process_extension.is_some() {
            "discovery disabled; exact managed-process and WG extensions are adapter-owned".into()
        } else {
            "discovery disabled; exact version-locked WG extension only".into()
        },
        extension_identities,
        tool_policy: if managed_process_extension.is_some() {
            "Pi built-ins plus exact process and WG graph tools; graph authority remains wrapper-scoped".into()
        } else {
            "Pi built-ins plus exact version-locked WG graph tools".into()
        },
        capability_cwd: project_root.to_path_buf(),
        working_directory_policy: PiWorkingDirectoryPolicy::AttemptWorkspace,
        execution_cwd: None,
        prompt_policy: "assembled WG task prompt on stdin with fixed -p instruction".into(),
        session_policy: task.session_id.as_ref().map_or_else(
            || "fresh exact generated session".into(),
            |id| format!("resume exact session:{id}"),
        ),
        timeout_secs,
        cancellation_grace_secs: 5,
        network_policy: "capability=offline; execution=Pi-configured network".into(),
        authentication_boundary: "Pi-owned mutable credential/token refresh; auth bytes excluded from assignment identity".into(),
        managed_process_extension,
        wg_extension,
        wg_plugin_root: Some(wg_plugin.root.clone()),
        wg_plugin_compat: Some(wg_plugin.compat),
    })
}

fn review_launch_plan(program: &Path, timeout_secs: Option<u64>) -> PiLaunchPlan {
    let cwd = std::env::current_dir().unwrap_or_default();
    let config_root = pi_config_root();
    let settings = crate::service::executor::ExecutorSettings {
        executor_type: "pi".into(),
        command: program.to_string_lossy().into_owned(),
        args: Vec::new(),
        env: std::collections::HashMap::new(),
        prompt_template: None,
        working_dir: None,
        timeout: timeout_secs,
        model: None,
    };
    PiLaunchPlan {
        invocation_kind: PiInvocationKind::HermeticReview,
        executable: pinned_path(program),
        fixed_argv: vec![
            "--mode".into(), "json".into(), "--print".into(), "-ne".into(),
            "--no-tools".into(), "--no-context-files".into(), "--no-skills".into(),
            "--no-prompt-templates".into(), "--no-session".into(),
        ],
        capability_argv: vec![
            "--offline".into(), "-ne".into(), "--no-tools".into(),
            "--no-context-files".into(), "--no-skills".into(),
            "--no-prompt-templates".into(), "--no-session".into(),
        ],
        config_root: config_root.clone(),
        config_identity: pi_config_identity(&config_root),
        executor_config: PinnedPathIdentity {
            path: PathBuf::from("<hermetic-review-built-in>"),
            digest: digest_bytes(&serde_json::to_vec(&settings).expect("review settings serialize")),
        },
        extension_policy: "discovery disabled; no extensions".into(),
        extension_identities: Vec::new(),
        tool_policy: "all tools disabled".into(),
        capability_cwd: cwd.clone(),
        working_directory_policy: PiWorkingDirectoryPolicy::Exact,
        execution_cwd: Some(cwd),
        prompt_policy: "one prompt on stdin; print/json response".into(),
        session_policy: "ephemeral; no session".into(),
        timeout_secs,
        cancellation_grace_secs: 5,
        network_policy: "capability=offline; execution=Pi-configured network".into(),
        authentication_boundary: "Pi-owned mutable credential/token refresh; auth bytes excluded from assignment identity".into(),
        managed_process_extension: None,
        wg_extension: None,
        wg_plugin_root: None,
        wg_plugin_compat: None,
    }
}

/// Resolve authoring policy once. This is the only experiment function which
/// may consult role/tier/profile policy. Runtime code receives the result.
pub fn resolve(
    task: &Task,
    config: &Config,
    role: DispatchRole,
    agent_identity: Option<&str>,
    pi_program: impl AsRef<Path>,
    shell_working_directory: impl AsRef<Path>,
) -> Result<ExecutionAssignment> {
    if task
        .remote_provider
        .as_deref()
        .is_some_and(|p| !p.trim().is_empty())
    {
        bail!(
            "error[WG-OPAQUE-REMOTE-UNSUPPORTED]: remote provider dispatch is outside the immutable Pi-or-shell experiment; use the WG-Exec provider plane explicitly"
        );
    }

    if task
        .exec
        .as_deref()
        .is_some_and(|command| !command.trim().is_empty())
        || task.exec_mode.as_deref() == Some("shell")
    {
        let command = task.exec.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "error[WG-OPAQUE-SHELL-ARGV-MISSING]: shell assignment requires task.exec"
            )
        })?;
        return Ok(ExecutionAssignment {
            schema: 1,
            task_id: task.id.clone(),
            agent_identity: agent_identity.unwrap_or("direct").to_string(),
            role,
            config_revision: config
                .authority_revision
                .clone()
                .unwrap_or_else(|| "unversioned".to_string()),
            authoring_fingerprint: task_authoring_fingerprint(
                task,
                config
                    .authority_revision
                    .as_deref()
                    .unwrap_or("unversioned"),
            ),
            authored_route: "shell".to_string(),
            execution: RuntimeExecution::Shell {
                argv: vec!["bash".to_string(), "-c".to_string(), command.to_string()],
                environment: std::collections::BTreeMap::from([
                    ("TASK_ID".to_string(), task.id.clone()),
                    ("TASK_TITLE".to_string(), task.title.clone()),
                ]),
                working_directory: shell_working_directory.as_ref().to_path_buf(),
            },
        });
    }

    let resolved = if let Some(route) = task
        .model
        .as_deref()
        .filter(|route| !route.trim().is_empty())
    {
        let reasoning = task
            .reasoning
            .or_else(|| config.resolve_reasoning_for_role(role))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "error[WG-EXEC-REASONING-MISSING]: task route {route:?} has no pinned reasoning"
                )
            })?;
        (route.to_string(), reasoning)
    } else if let Some(tier) = task.tier.as_deref().filter(|tier| !tier.trim().is_empty()) {
        let tier = tier.parse::<crate::config::Tier>()?;
        let route = config.resolve_tier_route(tier)?;
        let reasoning = task
            .reasoning
            .or_else(|| config.resolve_reasoning_for_tier(tier))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "error[WG-EXEC-REASONING-MISSING]: tier={tier} route={:?} has no pinned reasoning",
                    route.route
                )
            })?;
        (route.route, reasoning)
    } else {
        let route = config.resolve_opaque_pi_route_for_role(role).map_err(|error| {
            let diagnostic = format!("{error:#}");
            if diagnostic.contains("WG-EXEC-ROUTE-MISSING") {
                anyhow::anyhow!(diagnostic)
            } else {
                anyhow::anyhow!(
                    "error[WG-OPAQUE-LEGACY-ACTIVE]: role selection is not a safe outer Pi envelope and was not migrated: {diagnostic}"
                )
            }
        })?;
        let reasoning = task.reasoning.unwrap_or(route.reasoning);
        (route.route, reasoning)
    };

    let mut assignment = resolved_pi_assignment(
        &task.id,
        agent_identity.unwrap_or("direct"),
        role,
        config
            .authority_revision
            .as_deref()
            .unwrap_or("unversioned"),
        &resolved.0,
        resolved.1,
        pi_program,
    )?;
    if let RuntimeExecution::Pi {
        session_id,
        launch_plan,
        program,
        ..
    } = &mut assignment.execution
    {
        *session_id = task.session_id.clone();
        *launch_plan = Some(Box::new(worker_launch_plan(
            task,
            config,
            program,
            shell_working_directory.as_ref(),
        )?));
    }
    assignment.authoring_fingerprint = task_authoring_fingerprint(
        task,
        config
            .authority_revision
            .as_deref()
            .unwrap_or("unversioned"),
    );
    Ok(assignment)
}

/// Convert an already-resolved role policy into the same immutable Pi type used
/// by workers. This is the reviewer/evaluator/preflight authoring seam: policy
/// resolves before entry, and the runtime receives only this value.
pub fn resolved_pi_assignment(
    task_id: &str,
    agent_identity: &str,
    role: DispatchRole,
    config_revision: &str,
    authored_route: &str,
    reasoning: ReasoningLevel,
    pi_program: impl AsRef<Path>,
) -> Result<ExecutionAssignment> {
    resolved_pi_assignment_with_timeout(
        task_id,
        agent_identity,
        role,
        config_revision,
        authored_route,
        reasoning,
        pi_program,
        None,
    )
}

pub fn resolved_pi_assignment_with_timeout(
    task_id: &str,
    agent_identity: &str,
    role: DispatchRole,
    config_revision: &str,
    authored_route: &str,
    reasoning: ReasoningLevel,
    pi_program: impl AsRef<Path>,
    timeout_secs: Option<u64>,
) -> Result<ExecutionAssignment> {
    let opaque_route = crate::config::parse_opaque_pi_route(authored_route).map_err(|error| {
        anyhow::anyhow!(
            "error[WG-OPAQUE-LEGACY-ACTIVE]: experimental execution accepts only a safe exact outer `pi:` envelope; active route {authored_route:?} needs an explicit operator-declared migration and was not translated: {error}"
        )
    })?;
    Ok(ExecutionAssignment {
        schema: 1,
        task_id: task_id.to_string(),
        agent_identity: agent_identity.to_string(),
        role,
        config_revision: config_revision.to_string(),
        authoring_fingerprint: direct_authoring_fingerprint(
            task_id,
            agent_identity,
            role,
            config_revision,
            authored_route,
            reasoning,
        ),
        authored_route: authored_route.to_string(),
        execution: RuntimeExecution::Pi {
            program: pin_executable(pi_program.as_ref()),
            opaque_route: opaque_route.to_string(),
            reasoning,
            session_id: None,
            launch_plan: Some(Box::new(review_launch_plan(
                pi_program.as_ref(),
                timeout_secs,
            ))),
        },
    })
}

/// Resolve a configured executable to an absolute path when it is available.
/// A missing path is retained verbatim so preflight can classify it without
/// consuming attempt authority.
fn direct_authoring_fingerprint(
    task_id: &str,
    agent_identity: &str,
    role: DispatchRole,
    config_revision: &str,
    route: &str,
    reasoning: ReasoningLevel,
) -> String {
    let value = serde_json::json!({
        "task_id": task_id,
        "agent_identity": agent_identity,
        "role": role,
        "config_revision": config_revision,
        "route": route,
        "reasoning": reasoning,
    });
    format!(
        "b3:{}",
        blake3::hash(&serde_json::to_vec(&value).expect("fingerprint serializes")).to_hex()
    )
}

pub fn task_authoring_fingerprint(task: &Task, config_revision: &str) -> String {
    let value = serde_json::json!({
        "task_id": task.id,
        "title": task.title,
        "agent": task.agent,
        "model": task.model,
        "tier": task.tier,
        "reasoning": task.reasoning,
        "profile": task.profile,
        "exec": task.exec,
        "exec_mode": task.exec_mode,
        "remote_provider": task.remote_provider,
        "session_id": task.session_id,
        "config_revision": config_revision,
    });
    format!(
        "b3:{}",
        blake3::hash(&serde_json::to_vec(&value).expect("fingerprint serializes")).to_hex()
    )
}

pub fn pin_executable(program: &Path) -> PathBuf {
    if program.components().count() > 1 {
        return std::fs::canonicalize(program).unwrap_or_else(|_| {
            if program.is_absolute() {
                program.to_path_buf()
            } else {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(program)
            }
        });
    }
    std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .map(|directory| directory.join(program))
                .find(|candidate| candidate.is_file())
        })
        .and_then(|candidate| std::fs::canonicalize(&candidate).ok().or(Some(candidate)))
        .unwrap_or_else(|| program.to_path_buf())
}

/// Ask the exact configured Pi executable with the exact opaque route.  Pi's
/// exit status supplies the classification; WG does not inspect names or a
/// model registry.  Exit 2 is Pi's deterministic configuration/capability
/// rejection contract. Other failures are transient/indeterminate.
pub fn preflight(assignment: &ExecutionAssignment) -> PiPreflightOutcome {
    let RuntimeExecution::Pi {
        opaque_route,
        launch_plan,
        ..
    } = &assignment.execution
    else {
        return PiPreflightOutcome::Ready;
    };
    let Some(plan) = launch_plan.as_deref() else {
        return PiPreflightOutcome::MissingRequiredCapability {
            exit_code: None,
            diagnostic: "WG-OPAQUE-LAUNCH-PLAN-MISSING: historical assignment is readable but cannot be executed without a complete pinned invocation".into(),
        };
    };
    if let Err(error) = verify_launch_plan(plan) {
        return PiPreflightOutcome::MissingRequiredCapability {
            exit_code: None,
            diagnostic: format!("WG-OPAQUE-LAUNCH-IDENTITY-CHANGED: {error:#}"),
        };
    }
    let mut command = Command::new(&plan.executable.path);
    command.args(&plan.capability_argv);
    command.current_dir(&plan.capability_cwd);
    command.env("PI_CODING_AGENT_DIR", &plan.config_root);
    let output = command
        .arg("--list-models")
        .arg(opaque_route)
        .stdin(Stdio::null())
        .output();
    match output {
        Ok(output) if output.status.success() => {
            // Pi's list command exits successfully even when the exact query
            // matched nothing. Treat only a returned data row as capability;
            // the opaque query is never split or interpreted by WG.
            let stdout = String::from_utf8_lossy(&output.stdout);
            let matches = stdout
                .lines()
                .skip(1)
                .filter(|line| !line.trim().is_empty())
                .count();
            if matches == 1 {
                PiPreflightOutcome::Ready
            } else {
                PiPreflightOutcome::MissingRequiredCapability {
                    exit_code: output.status.code(),
                    diagnostic: format!(
                        "Pi capability query returned {matches} rows for opaque route {opaque_route:?}; exactly one Pi-resolved selection is required"
                    ),
                }
            }
        }
        Ok(output) => {
            let diagnostic = bounded_diagnostic(&output.stderr, &output.stdout);
            if output.status.code() == Some(2) {
                PiPreflightOutcome::MissingRequiredCapability {
                    exit_code: output.status.code(),
                    diagnostic,
                }
            } else {
                PiPreflightOutcome::TransientFailure {
                    exit_code: output.status.code(),
                    diagnostic,
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            PiPreflightOutcome::MissingRequiredCapability {
                exit_code: None,
                diagnostic: format!(
                    "Pi executable {} is unavailable: {error}",
                    plan.executable.path.display()
                ),
            }
        }
        Err(error) => PiPreflightOutcome::TransientFailure {
            exit_code: None,
            diagnostic: format!(
                "Pi preflight could not start {}: {error}",
                plan.executable.path.display()
            ),
        },
    }
}

pub fn verify_launch_plan(plan: &PiLaunchPlan) -> Result<()> {
    let executable = pinned_path(&plan.executable.path);
    if executable != plan.executable {
        bail!(
            "pinned Pi executable identity changed (expected={} {} observed={})",
            plan.executable.path.display(),
            plan.executable.digest,
            executable.digest
        );
    }
    let observed_config = pi_config_identity(&plan.config_root);
    if observed_config != plan.config_identity {
        bail!(
            "Pi non-secret configuration identity changed (expected={} observed={}); mutable auth/token refresh files are not part of either identity",
            plan.config_identity,
            observed_config
        );
    }
    if !matches!(
        plan.executor_config.path.to_str(),
        Some("<hermetic-review-built-in>" | "<built-in-pi-executor>")
    ) {
        let observed_digest = if plan.executor_config.path.is_file() {
            pinned_path(&plan.executor_config.path).digest
        } else {
            "missing".into()
        };
        if observed_digest != plan.executor_config.digest {
            bail!(
                "configured Pi invocation changed after pinning at {}",
                plan.executor_config.path.display()
            );
        }
    }
    for expected in &plan.extension_identities {
        let observed = pinned_path(&expected.path);
        if &observed != expected {
            bail!("pinned Pi extension changed at {}", expected.path.display());
        }
    }
    if let Some(extension) = plan.managed_process_extension.as_ref() {
        verify_managed_process_extension(&extension.path)?;
    }
    Ok(())
}

fn bounded_diagnostic(stderr: &[u8], stdout: &[u8]) -> String {
    let bytes = if stderr.is_empty() { stdout } else { stderr };
    String::from_utf8_lossy(bytes).chars().take(1000).collect()
}

pub fn as_spawn_plan(assignment: &ExecutionAssignment) -> crate::dispatch::SpawnPlan {
    let (executor, model, reasoning, executor_source, model_source, endpoint_source) =
        match &assignment.execution {
            RuntimeExecution::Pi {
                opaque_route,
                reasoning,
                ..
            } => (
                crate::dispatch::ExecutorKind::Pi,
                crate::dispatch::ResolvedModelSpec {
                    raw: assignment.authored_route.clone(),
                    provider: None,
                    model_id: opaque_route.clone(),
                },
                Some(*reasoning),
                "immutable execution assignment".to_string(),
                "immutable opaque Pi route".to_string(),
                "absent by Pi assignment contract".to_string(),
            ),
            RuntimeExecution::Shell { .. } => (
                crate::dispatch::ExecutorKind::Shell,
                crate::dispatch::ResolvedModelSpec {
                    raw: String::new(),
                    provider: None,
                    model_id: String::new(),
                },
                None,
                "immutable shell assignment".to_string(),
                "shell assignment (no model)".to_string(),
                "shell assignment (no endpoint)".to_string(),
            ),
        };
    crate::dispatch::SpawnPlan {
        executor,
        model,
        reasoning,
        config_revision: Some(assignment.config_revision.clone()),
        endpoint: None,
        env: std::collections::HashMap::new(),
        argv: Vec::new(),
        placement: crate::dispatch::Placement::Local,
        provenance: crate::dispatch::SpawnProvenance {
            executor_source,
            model_source,
            endpoint_source,
        },
    }
}

pub fn bind(
    mut assignment: ExecutionAssignment,
    runtime_agent_id: impl Into<String>,
    generation: u64,
    attempt_id: impl Into<String>,
    attempt_fence: u64,
    execution_cwd: Option<PathBuf>,
) -> BoundExecutionAssignment {
    if let RuntimeExecution::Pi {
        launch_plan: Some(plan),
        ..
    } = &mut assignment.execution
    {
        if let Some(cwd) = execution_cwd.as_ref() {
            plan.capability_cwd = cwd.clone();
        }
        plan.execution_cwd = execution_cwd.clone();
    }
    BoundExecutionAssignment {
        assignment,
        runtime_agent_id: runtime_agent_id.into(),
        generation,
        attempt_id: attempt_id.into(),
        attempt_fence,
        execution_cwd,
    }
}

/// Persist content-addressed one-shot invocation identity for reviewer/evaluator
/// attribution. The object contains only the non-secret assignment; prompts,
/// credentials and refreshed tokens are never included.
pub fn persist_oneshot_attribution(
    dir: &Path,
    assignment: &ExecutionAssignment,
) -> Result<PathBuf> {
    let bytes = serde_json::to_vec_pretty(assignment)?;
    let object_id = blake3::hash(&bytes).to_hex();
    let root = dir.join("service/opaque-oneshot-attribution");
    std::fs::create_dir_all(&root)?;
    let path = root.join(format!("{object_id}.json"));
    match crate::atomic_file::write_atomic_create_new(&path, &bytes) {
        Ok(()) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if std::fs::read(&path)? != bytes {
                bail!("one-shot attribution object changed at {}", path.display());
            }
            Ok(path)
        }
        Err(error) => Err(error).with_context(|| format!("persist {}", path.display())),
    }
}

pub fn persist(dir: &Path, bound: &BoundExecutionAssignment) -> Result<PathBuf> {
    let key = crate::attempt_runtime::AttemptRuntimeKey::new(
        &bound.assignment.task_id,
        bound.generation,
        &bound.attempt_id,
        bound.attempt_fence,
        bound.attempt_fence,
    );
    let component = crate::attempt_runtime::component_for_write(dir, &key, ASSIGNMENT_COMPONENT)?;
    std::fs::create_dir_all(&component)?;
    let path = component.join(ASSIGNMENT_FILE);
    let bytes = serde_json::to_vec_pretty(bound)?;
    match crate::atomic_file::write_atomic_create_new(&path, &bytes) {
        Ok(()) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = std::fs::read(&path)?;
            if existing != bytes {
                bail!(
                    "immutable execution assignment changed for attempt at {}; explicit new attempt authority is required",
                    path.display()
                );
            }
            Ok(path)
        }
        Err(error) => Err(error).with_context(|| format!("persist {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigSource, Tier};

    fn config(route: &str, revision: &str) -> Config {
        let mut config = Config::default();
        config.agent.model = route.to_string();
        config.tiers.standard = None;
        config.tiers.fast = None;
        config.tiers.premium = None;
        config.tiers.standard_reasoning = Some(ReasoningLevel::High);
        config.tiers.fast_reasoning = None;
        config.authority_revision = Some(revision.to_string());
        config.authority_source = Some(ConfigSource::ProjectFile.to_string());
        config
    }

    #[test]
    fn authoring_keeps_opaque_route_default_equal_tiers_and_revision() {
        let config = config("pi:future+wire:model/with:odd:bytes", "b3:project-a");
        assert_eq!(
            config.resolve_tier_route(Tier::Fast).unwrap().route,
            config.resolve_tier_route(Tier::Standard).unwrap().route
        );
        let task = Task {
            id: "t".into(),
            ..Task::default()
        };
        let assignment = resolve(
            &task,
            &config,
            DispatchRole::TaskAgent,
            Some("agent-key"),
            "/x/pi",
            "/project-a",
        )
        .unwrap();
        assert_eq!(assignment.config_revision, "b3:project-a");
        assert_eq!(assignment.agent_identity, "agent-key");
        assert_eq!(
            assignment.authored_route,
            "pi:future+wire:model/with:odd:bytes"
        );
        assert!(matches!(
            assignment.execution,
            RuntimeExecution::Pi {
                program,
                opaque_route,
                reasoning: ReasoningLevel::High,
                session_id: None,
                launch_plan: Some(_),
            } if program == PathBuf::from("/x/pi")
                && opaque_route == "future+wire:model/with:odd:bytes"
        ));
    }

    #[test]
    fn explicit_role_and_task_overrides_are_pinned_without_cross_project_authority() {
        let mut a = config("pi:a:default", "b3:a");
        a.models.reviewer = Some(crate::config::RoleModelConfig {
            provider: None,
            model: Some("pi:a:review".into()),
            tier: None,
            endpoint: None,
            reasoning: Some(ReasoningLevel::Low),
        });
        let b = config("pi:b:default", "b3:b");
        let task = Task {
            id: "same".into(),
            ..Task::default()
        };
        let review = resolve(&task, &a, DispatchRole::Reviewer, None, "pi", "/a").unwrap();
        let worker_b = resolve(&task, &b, DispatchRole::TaskAgent, None, "pi", "/b").unwrap();
        assert_eq!(review.authored_route, "pi:a:review");
        assert_eq!(review.config_revision, "b3:a");
        assert_eq!(worker_b.authored_route, "pi:b:default");
        assert_eq!(worker_b.config_revision, "b3:b");

        a.tiers.fast = Some("pi:a:fast-distinct".into());
        a.tiers.fast_reasoning = Some(ReasoningLevel::Minimal);
        let tier_task = Task {
            id: "tiered".into(),
            tier: Some("fast".into()),
            ..Task::default()
        };
        let tiered = resolve(&tier_task, &a, DispatchRole::TaskAgent, None, "pi", "/a").unwrap();
        assert_eq!(tiered.authored_route, "pi:a:fast-distinct");
        assert!(matches!(
            tiered.execution,
            RuntimeExecution::Pi {
                reasoning: ReasoningLevel::Minimal,
                ..
            }
        ));

        let mut task_override = task;
        task_override.model = Some("pi:a:task-specific".into());
        task_override.reasoning = Some(ReasoningLevel::Xhigh);
        let pinned = resolve(
            &task_override,
            &a,
            DispatchRole::TaskAgent,
            None,
            "pi",
            "/a",
        )
        .unwrap();
        assert_eq!(pinned.authored_route, "pi:a:task-specific");
        assert!(matches!(
            pinned.execution,
            RuntimeExecution::Pi {
                reasoning: ReasoningLevel::Xhigh,
                ..
            }
        ));
    }

    #[test]
    fn pi_assignment_pins_fresh_or_exact_resume_session() {
        let mut task = Task {
            id: "resume".into(),
            session_id: Some("0199-exact-session".into()),
            ..Task::default()
        };
        let assignment = resolve(
            &task,
            &config("pi:test:resume", "b3:resume"),
            DispatchRole::TaskAgent,
            None,
            "pi",
            "/project",
        )
        .unwrap();
        assert!(matches!(
            &assignment.execution,
            RuntimeExecution::Pi {
                session_id: Some(id),
                ..
            } if id == "0199-exact-session"
        ));
        task.session_id = None;
        assert_ne!(
            assignment.authoring_fingerprint,
            task_authoring_fingerprint(&task, &assignment.config_revision)
        );
    }

    #[test]
    fn active_legacy_route_is_refused_not_guessed_into_pi() {
        let legacy_config = config("codex:gpt-historic", "b3:legacy");
        let task = Task {
            id: "t".into(),
            ..Task::default()
        };
        let error = resolve(
            &task,
            &legacy_config,
            DispatchRole::TaskAgent,
            None,
            "pi",
            "/project",
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("WG-OPAQUE-LEGACY-ACTIVE"));

        let remote = Task {
            id: "remote".into(),
            remote_provider: Some("wgid:provider".into()),
            ..Task::default()
        };
        let error = resolve(
            &remote,
            &config("pi:test:valid", "b3:remote"),
            DispatchRole::TaskAgent,
            None,
            "pi",
            "/project",
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("WG-OPAQUE-REMOTE-UNSUPPORTED"));
    }

    #[test]
    fn missing_route_preserves_actionable_route_code_without_legacy_wrapper() {
        let missing_config = config("", "b3:missing");
        let task = Task {
            id: "missing".into(),
            ..Task::default()
        };
        let diagnostic = format!(
            "{:#}",
            resolve(
                &task,
                &missing_config,
                DispatchRole::TaskAgent,
                None,
                "pi",
                "/project",
            )
            .unwrap_err()
        );
        assert!(diagnostic.contains("WG-EXEC-ROUTE-MISSING"), "{diagnostic}");
        assert!(
            !diagnostic.contains("WG-OPAQUE-LEGACY-ACTIVE"),
            "{diagnostic}"
        );
    }

    #[test]
    fn managed_process_extension_requires_exact_package_name_and_version() {
        let temp = tempfile::tempdir().unwrap();
        let package = temp.path().join("package");
        std::fs::create_dir_all(package.join("src")).unwrap();
        let entry = package.join("src/index.js");
        std::fs::write(&entry, "export default {};").unwrap();
        std::fs::write(
            package.join("package.json"),
            format!(
                r#"{{"name":"{}","version":"{}"}}"#,
                PI_PROCESSES_PACKAGE, PI_PROCESSES_VERSION
            ),
        )
        .unwrap();
        verify_managed_process_extension(&entry).unwrap();

        std::fs::write(
            package.join("package.json"),
            r#"{"name":"wrong-package","version":"9.9.9"}"#,
        )
        .unwrap();
        let diagnostic = verify_managed_process_extension(&entry)
            .unwrap_err()
            .to_string();
        assert!(
            diagnostic.contains("WG-PI-PROCESS-EXTENSION-MISMATCH"),
            "{diagnostic}"
        );
        assert!(diagnostic.contains("@mjakl/pi-processes@2.0.0"));
        assert!(diagnostic.contains("wrong-package"));
        assert!(diagnostic.contains("9.9.9"));
    }

    #[test]
    fn shell_assignment_pins_argv_environment_working_directory_and_task_inputs() {
        let config = config("pi:test:unused", "b3:shell");
        let task = Task {
            id: "shell".into(),
            title: "Exact shell".into(),
            exec: Some("printf exact".into()),
            exec_mode: Some("shell".into()),
            ..Task::default()
        };
        let assignment = resolve(
            &task,
            &config,
            DispatchRole::TaskAgent,
            None,
            "pi",
            "/exact/project",
        )
        .unwrap();
        assert!(matches!(
            &assignment.execution,
            RuntimeExecution::Shell {
                argv,
                environment,
                working_directory,
            } if argv == &vec!["bash".to_string(), "-c".to_string(), "printf exact".to_string()]
                && environment.get("TASK_ID").map(String::as_str) == Some("shell")
                && working_directory == Path::new("/exact/project")
        ));
        let mut changed = task.clone();
        changed.exec = Some("printf mutated".into());
        assert_ne!(
            assignment.authoring_fingerprint,
            task_authoring_fingerprint(&changed, &assignment.config_revision)
        );
    }

    #[test]
    fn bound_attempt_assignment_is_create_once_and_stably_pinned() {
        let temp = tempfile::tempdir().unwrap();
        let base_config = config("pi:test:stable", "b3:revision-1");
        let task = Task {
            id: "stable-task".into(),
            ..Task::default()
        };
        let assignment = resolve(
            &task,
            &base_config,
            DispatchRole::TaskAgent,
            None,
            "pi",
            temp.path(),
        )
        .unwrap();
        let bound = bind(
            assignment,
            "agent-7",
            2,
            "attempt-2-3",
            9,
            Some(temp.path().to_path_buf()),
        );
        let first = persist(temp.path(), &bound).unwrap();
        let second = persist(temp.path(), &bound).unwrap();
        assert_eq!(first, second);
        let recorded: BoundExecutionAssignment =
            serde_json::from_slice(&std::fs::read(&first).unwrap()).unwrap();
        assert_eq!(recorded.attempt_id, "attempt-2-3");
        assert_eq!(recorded.attempt_fence, 9);
        assert_eq!(recorded.assignment.config_revision, "b3:revision-1");
        assert_eq!(recorded.assignment.authored_route, "pi:test:stable");
        assert_eq!(recorded.execution_cwd.as_deref(), Some(temp.path()));
        assert!(matches!(
            &recorded.assignment.execution,
            RuntimeExecution::Pi {
                launch_plan: Some(plan),
                ..
            } if plan.execution_cwd.as_deref() == Some(temp.path())
        ));

        let mut changed_same_attempt = bound.clone();
        changed_same_attempt.assignment.config_revision = "b3:illegal-rewrite".into();
        assert!(persist(temp.path(), &changed_same_attempt).is_err());

        let next_config = config("pi:test:new-attempt", "b3:revision-2");
        let next_assignment = resolve(
            &task,
            &next_config,
            DispatchRole::TaskAgent,
            None,
            "pi",
            temp.path(),
        )
        .unwrap();
        let next = bind(
            next_assignment,
            "agent-8",
            3,
            "attempt-3-1",
            10,
            Some(temp.path().to_path_buf()),
        );
        let next_path = persist(temp.path(), &next).unwrap();
        assert_ne!(first, next_path);
        assert_eq!(
            serde_json::from_slice::<BoundExecutionAssignment>(&std::fs::read(next_path).unwrap())
                .unwrap()
                .assignment
                .authored_route,
            "pi:test:new-attempt"
        );
    }

    #[test]
    fn transient_preflight_backoff_is_persisted_bounded_and_assignment_keyed() {
        let temp = tempfile::tempdir().unwrap();
        let task = Task {
            id: "backoff".into(),
            ..Task::default()
        };
        let first = resolve(
            &task,
            &config("pi:test:one", "b3:one"),
            DispatchRole::TaskAgent,
            None,
            "pi",
            temp.path(),
        )
        .unwrap();
        let transient = PiPreflightOutcome::TransientFailure {
            exit_code: Some(75),
            diagnostic: "busy".into(),
        };
        let t0 = UNIX_EPOCH + Duration::from_secs(100);
        record_preflight_outcome_at(temp.path(), &first, &transient, t0).unwrap();
        assert_eq!(
            preflight_backoff_remaining_at(temp.path(), &first, t0),
            Some(Duration::from_secs(TRANSIENT_BACKOFF_BASE_SECS))
        );
        assert!(
            preflight_backoff_remaining_at(
                temp.path(),
                &first,
                t0 + Duration::from_secs(TRANSIENT_BACKOFF_BASE_SECS + 1)
            )
            .is_none()
        );
        record_preflight_outcome_at(temp.path(), &first, &transient, t0 + Duration::from_secs(6))
            .unwrap();
        assert_eq!(
            preflight_backoff_remaining_at(temp.path(), &first, t0 + Duration::from_secs(6)),
            Some(Duration::from_secs(10))
        );

        let changed = resolve(
            &task,
            &config("pi:test:two", "b3:two"),
            DispatchRole::TaskAgent,
            None,
            "pi",
            temp.path(),
        )
        .unwrap();
        assert!(preflight_backoff_remaining_at(temp.path(), &changed, t0).is_none());
        record_preflight_outcome_at(temp.path(), &first, &PiPreflightOutcome::Ready, t0).unwrap();
        assert!(preflight_backoff_remaining_at(temp.path(), &first, t0).is_none());
    }

    #[test]
    fn historical_assignment_without_launch_plan_is_readable_but_not_executable() {
        let task = Task {
            id: "historical".into(),
            ..Task::default()
        };
        let mut assignment = resolve(
            &task,
            &config("pi:openai-codex/gpt-5.6-sol", "b3:historical"),
            DispatchRole::TaskAgent,
            None,
            "pi",
            "/project",
        )
        .unwrap();
        let RuntimeExecution::Pi { launch_plan, .. } = &mut assignment.execution else {
            unreachable!()
        };
        *launch_plan = None;
        let historical: ExecutionAssignment =
            serde_json::from_slice(&serde_json::to_vec(&assignment).unwrap()).unwrap();
        assert!(matches!(
            preflight(&historical),
            PiPreflightOutcome::MissingRequiredCapability { diagnostic, .. }
                if diagnostic.contains("WG-OPAQUE-LAUNCH-PLAN-MISSING")
        ));
    }

    #[test]
    fn pinned_non_secret_pi_config_drift_is_detected_without_freezing_auth() {
        let temp = tempfile::tempdir().unwrap();
        let config_root = temp.path().join("pi-config");
        std::fs::create_dir_all(&config_root).unwrap();
        std::fs::write(config_root.join("settings.json"), br#"{"theme":"one"}"#).unwrap();
        let mut plan = review_launch_plan(Path::new("pi"), Some(30));
        plan.config_root = config_root.clone();
        plan.config_identity = pi_config_identity(&config_root);
        verify_launch_plan(&plan).unwrap();

        // Settings/model registration are launch behavior and therefore pinned.
        std::fs::write(config_root.join("settings.json"), br#"{"theme":"two"}"#).unwrap();
        assert!(
            verify_launch_plan(&plan)
                .unwrap_err()
                .to_string()
                .contains("non-secret configuration identity changed")
        );
        // Auth/token files are deliberately outside the plan identity.
        std::fs::write(config_root.join("settings.json"), br#"{"theme":"one"}"#).unwrap();
        std::fs::write(config_root.join("auth.json"), br#"{"refresh":"rotated"}"#).unwrap();
        verify_launch_plan(&plan).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn controlled_pi_preflight_preserves_route_and_classifies_pi_status() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("argv.json");
        let fake = temp.path().join("pi");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf 'Provider Model\\nfixture exact\\n'\nexit \"${{PI_EXIT:-0}}\"\n",
                log.display()
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake, permissions).unwrap();
        let base_config = config("pi:odd+provider:model:alpha/beta", "b3:a");
        let task = Task {
            id: "t".into(),
            ..Task::default()
        };
        let assignment = resolve(
            &task,
            &base_config,
            DispatchRole::TaskAgent,
            None,
            &fake,
            temp.path(),
        )
        .unwrap();
        assert_eq!(preflight(&assignment), PiPreflightOutcome::Ready);
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "--offline\n-ne\n--no-skills\n--no-prompt-templates\n--no-context-files\n--list-models\nodd+provider:model:alpha/beta\n"
        );

        std::fs::write(&fake, "#!/bin/sh\nprintf 'Provider Model\\n'\n").unwrap();
        assert!(matches!(
            preflight(&assignment),
            PiPreflightOutcome::MissingRequiredCapability { .. }
        ));
        std::fs::write(
            &fake,
            "#!/bin/sh\nprintf 'Provider Model\\nfixture one\\nfixture two\\n'\n",
        )
        .unwrap();
        assert!(matches!(
            preflight(&assignment),
            PiPreflightOutcome::MissingRequiredCapability { .. }
        ));

        std::fs::write(
            &fake,
            "#!/bin/sh\nroute=''; for route in \"$@\"; do :; done; case \"$route\" in missing:*) echo pi-rejected >&2; exit 2;; transient:*) echo pi-busy >&2; exit 75;; esac\n",
        )
        .unwrap();
        let missing_config = config("pi:missing:exact", "b3:m");
        let missing = resolve(
            &task,
            &missing_config,
            DispatchRole::TaskAgent,
            None,
            &fake,
            temp.path(),
        )
        .unwrap();
        assert!(matches!(
            preflight(&missing),
            PiPreflightOutcome::MissingRequiredCapability {
                exit_code: Some(2),
                ..
            }
        ));
        let transient_config = config("pi:transient:exact", "b3:t");
        let transient = resolve(
            &task,
            &transient_config,
            DispatchRole::TaskAgent,
            None,
            &fake,
            temp.path(),
        )
        .unwrap();
        assert!(matches!(
            preflight(&transient),
            PiPreflightOutcome::TransientFailure {
                exit_code: Some(75),
                ..
            }
        ));
    }
}
