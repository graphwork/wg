//! Immutable Cargo-target baselines with private copy-on-write layers.
//!
//! Cargo must never have two divergent worktrees writing one target directory.
//! This module instead keeps a content-keyed, read-only baseline and gives each
//! attempt its own directory tree. Regular files are cloned with a verified
//! filesystem reflink when available and otherwise copied to private inodes.
//! Mutable build artifacts are never hard-linked. If a baseline cannot be
//! cloned safely, the attempt starts cold rather than sharing uncertain bytes.
//! Incremental compilation is disabled by the spawn path.

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const CACHE_SCHEMA: u32 = 4;
const LAYER_MANIFEST: &str = ".wg-target-layer.json";
const LAYER_OWNED: &str = ".wg-owned-layer";
const BASELINE_MANIFEST: &str = ".wg-target-baseline.json";
const BASELINE_OWNED: &str = ".wg-owned-baseline";
const READY: &str = "READY";
/// Transient rollback marker installed by spawn before durable lease publish.
/// It must never be promoted as a Cargo artifact.
const UNPUBLISHED_OWNER_MARKER: &str = ".wg-unpublished-cache-owner";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetCacheKey {
    pub schema: u32,
    pub source_baseline: String,
    pub cargo_lock: String,
    /// Hash of workspace manifests, toolchain files, and Cargo configuration.
    pub cargo_inputs: String,
    pub rustc: String,
    pub target_triple: String,
    /// Logical Cargo working directory. Managed worktree roots normalize to
    /// `.` so sibling worktrees can share an otherwise identical key.
    pub working_directory: String,
    /// Effective Cargo home, including an accepted inline `CARGO_HOME=...`.
    pub cargo_home: String,
    /// Effective rustup/toolchain selector.
    pub toolchain: String,
    /// Every accepted leading environment assignment, in source order.
    pub accepted_environment: String,
    pub features: String,
    pub profile: String,
    pub flags: String,
    /// The complete WG-controlled shell command, when it is known exactly.
    /// This intentionally over-invalidates: two byte-different commands never
    /// claim an exact shared baseline even if Cargo would treat them alike.
    pub command_identity: String,
    /// False when WG launches an interactive agent or the shell command falls
    /// outside WG's deliberately tiny attested grammar. Such a layer is
    /// attempt-isolated and can neither consume nor publish a shared baseline.
    pub baseline_reusable: bool,
}

impl TargetCacheKey {
    pub fn digest(&self) -> String {
        let encoded = serde_json::to_vec(self).expect("target cache key serializes");
        blake3::hash(&encoded).to_hex().to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LayerManifest {
    schema: u32,
    key: TargetCacheKey,
    source_root: String,
    baseline_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TargetLayer {
    pub path: PathBuf,
    pub key: TargetCacheKey,
    pub baseline_path: Option<PathBuf>,
}

struct KeyLock {
    _file: File,
}

impl KeyLock {
    fn acquire(root: &Path, key: &str) -> Result<Self> {
        let locks = root.join("locks");
        fs::create_dir_all(&locks)?;
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(locks.join(format!("{key}.lock")))?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
                return Err(std::io::Error::last_os_error()).context("lock target cache key");
            }
        }
        Ok(Self { _file: file })
    }
}

fn command_stdout(root: &Path, program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn hash_file(path: &Path) -> String {
    fs::read(path)
        .map(|bytes| blake3::hash(&bytes).to_hex().to_string())
        .unwrap_or_else(|_| "missing".to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AttestedCargoCommand {
    original: String,
    words: Vec<String>,
    environment: Vec<(String, String)>,
}

fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

/// Tokenize one simple shell command. Expansion, redirection, grouping,
/// pipelines, comments and command substitution are intentionally unsupported:
/// accepting more shell is much easier than attesting its state transitions.
fn simple_shell_words(input: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;
    for ch in input.chars() {
        if escaped {
            word.push(ch);
            escaped = false;
            started = true;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            started = true;
            continue;
        }
        if matches!(ch, '\'' | '"') {
            if quote == Some(ch) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(ch);
            } else {
                word.push(ch);
            }
            started = true;
            continue;
        }
        if quote.is_none() && ch.is_ascii_whitespace() {
            if started {
                words.push(std::mem::take(&mut word));
                started = false;
            }
            continue;
        }
        if matches!(
            ch,
            '$' | '`' | '<' | '>' | '|' | ';' | '&' | '(' | ')' | '{' | '}' | '\n' | '\r'
        ) {
            return None;
        }
        word.push(ch);
        started = true;
    }
    if escaped || quote.is_some() {
        return None;
    }
    if started {
        words.push(word);
    }
    Some(words)
}

fn parse_attested_cargo_command(shell_command: &str) -> Option<AttestedCargoCommand> {
    let original = shell_command.trim();
    if original.is_empty() {
        return None;
    }

    // The sole accepted compound form is an inert bounded delay used by the
    // overlap smoke. It cannot prepare or alter Cargo state. Every other shell
    // operator fails closed to an attempt-isolated namespace.
    let pieces = original.split("&&").map(str::trim).collect::<Vec<_>>();
    if pieces.is_empty() || pieces.len() > 3 {
        return None;
    }
    let cargo_part = pieces[0];
    if let Some(sleep) = pieces.get(1) {
        let suffix_words = simple_shell_words(sleep)?;
        if suffix_words.len() != 2
            || suffix_words[0] != "sleep"
            || suffix_words[1]
                .parse::<u32>()
                .ok()
                .filter(|n| *n <= 300)
                .is_none()
        {
            return None;
        }
    }
    if pieces.get(2).copied()
        != Some("wg wait \"$WG_TASK_ID\" --until message --checkpoint 'storage fixture complete'")
    {
        if pieces.len() == 3 {
            return None;
        }
    }

    let words = simple_shell_words(cargo_part)?;
    let mut cursor = 0;
    let mut environment = Vec::new();
    while let Some(word) = words.get(cursor) {
        let Some((name, value)) = word.split_once('=') else {
            break;
        };
        if !valid_env_name(name) || name == "CARGO_TARGET_DIR" {
            return None;
        }
        environment.push((name.to_string(), value.to_string()));
        cursor += 1;
    }
    if words.get(cursor).map(String::as_str) != Some("cargo") {
        return None;
    }
    let cargo_words = &words[cursor + 1..];
    let mut cargo_cursor = 0;
    if cargo_words
        .get(cargo_cursor)
        .is_some_and(|word| word.starts_with('+') && word.len() > 1)
    {
        cargo_cursor += 1;
    }
    while let Some(word) = cargo_words.get(cargo_cursor) {
        if word.starts_with("--config=") {
            cargo_cursor += 1;
        } else if matches!(word.as_str(), "--config" | "-Z")
            && cargo_words.get(cargo_cursor + 1).is_some()
        {
            cargo_cursor += 2;
        } else {
            break;
        }
    }
    if !cargo_words
        .get(cargo_cursor)
        .is_some_and(|word| matches!(word.as_str(), "build" | "check" | "test" | "clippy" | "doc"))
        || cargo_words[cargo_cursor + 1..]
            .iter()
            .any(|word| word == "cargo")
    {
        return None;
    }
    Some(AttestedCargoCommand {
        original: original.to_string(),
        words,
        environment,
    })
}

/// Return exact command bytes only for WG's deliberately tiny attested Cargo
/// grammar. Stateful setup (`export`, `cd`, functions/subshells), redirections,
/// arbitrary compound commands and dynamic expansion all return `None`; their
/// callers receive an attempt-isolated, non-reusable layer.
pub fn controlled_cargo_command(shell_command: &str) -> Option<String> {
    parse_attested_cargo_command(shell_command).map(|command| command.original)
}

fn hash_controlled_command_files(
    source_root: &Path,
    controlled_command: Option<&AttestedCargoCommand>,
    base_inputs: &str,
) -> String {
    let Some(command) = controlled_command else {
        return base_inputs.to_string();
    };
    let words = &command.words;
    let mut referenced = Vec::new();
    let mut index = 0;
    while index < words.len() {
        let word = words[index].as_str();
        let inline = ["--config=", "--manifest-path=", "--target="]
            .into_iter()
            .find_map(|prefix| word.strip_prefix(prefix));
        let separate = matches!(word, "--config" | "--manifest-path" | "--target")
            .then(|| words.get(index + 1).map(String::as_str))
            .flatten();
        if let Some(value) = inline.or(separate) {
            let path = PathBuf::from(value);
            let path = if path.is_absolute() {
                path
            } else {
                source_root.join(path)
            };
            // `--config key=value` and named target triples are fully captured
            // by the command bytes. Existing paths additionally bind content,
            // closing same-path config/manifest/JSON-target mutation.
            if path.is_file() {
                referenced.push(path);
            }
        }
        if separate.is_some() {
            index += 1;
        }
        index += 1;
    }
    referenced.sort();
    referenced.dedup();
    let mut hasher = blake3::Hasher::new();
    hasher.update(base_inputs.as_bytes());
    for path in referenced {
        hasher.update(b"\0command-file\0");
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update(b"\0");
        match fs::read(path) {
            Ok(bytes) => hasher.update(&bytes),
            Err(_) => hasher.update(b"<unreadable>"),
        };
    }
    hasher.finalize().to_hex().to_string()
}

fn hash_cargo_inputs(source_root: &Path, cargo_home: Option<&Path>) -> String {
    let mut paths = command_stdout(source_root, "git", &["ls-files", "*Cargo.toml"])
        .unwrap_or_default()
        .lines()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    paths.extend(
        [
            "Cargo.toml",
            "rust-toolchain",
            "rust-toolchain.toml",
            ".cargo/config",
            ".cargo/config.toml",
        ]
        .into_iter()
        .map(PathBuf::from),
    );
    paths.sort();
    paths.dedup();
    let mut hasher = blake3::Hasher::new();
    for relative in paths {
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update(b"\0");
        match fs::read(source_root.join(&relative)) {
            Ok(bytes) => hasher.update(&bytes),
            Err(_) => hasher.update(b"<missing>"),
        };
        hasher.update(b"\0");
    }
    if let Some(cargo_home) = cargo_home {
        for name in ["config", "config.toml"] {
            let path = cargo_home.join(name);
            hasher.update(path.to_string_lossy().as_bytes());
            hasher.update(b"\0");
            match fs::read(path) {
                Ok(bytes) => hasher.update(&bytes),
                Err(_) => hasher.update(b"<missing>"),
            };
            hasher.update(b"\0");
        }
    }
    hasher.finalize().to_hex().to_string()
}

/// Compute a target namespace under WG's honest command-identity contract.
///
/// `controlled_command` is present only when WG will execute those exact shell
/// bytes (currently an explicit shell task). The complete command captures
/// profile/release, target, feature switches, inline rustflags, and `--config`
/// arguments without pretending that a partial parser is Cargo. Workspace,
/// Cargo-home configuration and ambient rustflags are hashed separately below.
/// When the command is unavailable, `isolation_id` makes a private namespace
/// and `baseline_reusable=false` prevents both baseline consumption and
/// promotion. Unknown command identity therefore fails closed, never "exact".
pub fn compute_key(
    source_root: &Path,
    controlled_command: Option<&str>,
    isolation_id: &str,
) -> TargetCacheKey {
    // Re-parse at the authority boundary even when the caller already used
    // `controlled_cargo_command`. A future call site cannot accidentally mark
    // arbitrary shell bytes reusable by passing `Some` directly.
    let attested = controlled_command.and_then(parse_attested_cargo_command);
    let inline_env = |name: &str| {
        attested.as_ref().and_then(|command| {
            command
                .environment
                .iter()
                .rev()
                .find(|(candidate, _)| candidate == name)
                .map(|(_, value)| value.clone())
        })
    };
    let toolchain = inline_env("RUSTUP_TOOLCHAIN")
        .or_else(|| std::env::var("RUSTUP_TOOLCHAIN").ok())
        .unwrap_or_else(|| "rustup-default".to_string());
    let rustc = {
        let mut command = Command::new(inline_env("RUSTC").as_deref().unwrap_or("rustc"));
        command
            .args(["--version", "--verbose"])
            .current_dir(source_root);
        if toolchain != "rustup-default" {
            command.env("RUSTUP_TOOLCHAIN", &toolchain);
        }
        command
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .unwrap_or_else(|| "rustc-unavailable".to_string())
    };
    let target_triple = inline_env("CARGO_BUILD_TARGET")
        .or_else(|| std::env::var("CARGO_BUILD_TARGET").ok())
        .filter(|v| !v.is_empty())
        .or_else(|| {
            rustc
                .lines()
                .find_map(|line| line.strip_prefix("host: ").map(str::to_string))
        })
        .unwrap_or_else(|| "host-unknown".to_string());
    let source_baseline = command_stdout(source_root, "git", &["rev-parse", "HEAD^{tree}"])
        .unwrap_or_else(|| hash_file(&source_root.join("Cargo.toml")));
    let working_directory = command_stdout(source_root, "git", &["rev-parse", "--show-prefix"])
        .filter(|prefix| !prefix.is_empty())
        .unwrap_or_else(|| {
            command_stdout(source_root, "git", &["rev-parse", "--show-toplevel"])
                .map(|_| ".".to_string())
                .unwrap_or_else(|| {
                    source_root
                        .canonicalize()
                        .unwrap_or_else(|_| source_root.to_path_buf())
                        .to_string_lossy()
                        .to_string()
                })
        });
    let cargo_home_path = inline_env("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CARGO_HOME").map(PathBuf::from))
        .or_else(|| dirs::home_dir().map(|home| home.join(".cargo")));
    let resolved_cargo_home = cargo_home_path.as_deref().map(|path| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            source_root.join(path)
        }
    });
    let cargo_home = resolved_cargo_home
        .as_deref()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| "cargo-home-unavailable".to_string());
    let accepted_environment = attested
        .as_ref()
        .map(|command| {
            command
                .environment
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let features = std::env::var("WG_CARGO_FEATURES").unwrap_or_else(|_| "default".to_string());
    let profile = std::env::var("WG_CARGO_PROFILE").unwrap_or_else(|_| "test".to_string());
    let mut flag_values = std::env::vars()
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "RUSTFLAGS" | "RUSTDOCFLAGS" | "CARGO_ENCODED_RUSTFLAGS" | "RUSTUP_TOOLCHAIN"
            ) || name.starts_with("CARGO_BUILD_")
                || (name.starts_with("CARGO_PROFILE_")
                    && name != "CARGO_PROFILE_DEV_DEBUG"
                    && name != "CARGO_PROFILE_TEST_DEBUG")
                || (name.starts_with("CARGO_TARGET_") && name != "CARGO_TARGET_DIR")
        })
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>();
    for (name, default) in [
        ("CARGO_INCREMENTAL", "0"),
        ("CARGO_PROFILE_DEV_DEBUG", "line-tables-only"),
        ("CARGO_PROFILE_TEST_DEBUG", "line-tables-only"),
    ] {
        if !flag_values
            .iter()
            .any(|value| value.starts_with(&format!("{name}=")))
        {
            flag_values.push(format!("{name}={default}"));
        }
    }
    flag_values.sort();
    let flags = flag_values.join("\n");
    let (command_identity, baseline_reusable) = match attested.as_ref() {
        Some(command) => (command.original.clone(), true),
        None => (format!("unknown-isolated:{isolation_id}"), false),
    };
    let cargo_inputs = hash_controlled_command_files(
        source_root,
        attested.as_ref(),
        &hash_cargo_inputs(source_root, resolved_cargo_home.as_deref()),
    );
    TargetCacheKey {
        schema: CACHE_SCHEMA,
        source_baseline,
        cargo_lock: hash_file(&source_root.join("Cargo.lock")),
        cargo_inputs,
        rustc,
        target_triple,
        working_directory,
        cargo_home,
        toolchain,
        accepted_environment,
        features,
        profile,
        flags,
        command_identity,
        baseline_reusable,
    }
}

fn baseline_dir(root: &Path, digest: &str) -> PathBuf {
    root.join("baselines").join(digest)
}

fn baseline_is_ready(path: &Path, key: &TargetCacheKey) -> bool {
    if !path.join(READY).is_file() || !baseline_is_owned(path) {
        return false;
    }
    fs::read(path.join(BASELINE_MANIFEST))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<TargetCacheKey>(&bytes).ok())
        .is_some_and(|found| &found == key)
}

fn target_has_artifacts(target: &Path) -> bool {
    walkdir::WalkDir::new(target)
        .follow_links(false)
        .min_depth(1)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            let name = entry.file_name().to_string_lossy();
            entry.file_type().is_file()
                && ![
                    LAYER_MANIFEST,
                    LAYER_OWNED,
                    BASELINE_MANIFEST,
                    BASELINE_OWNED,
                    READY,
                    ".cargo-lock",
                    ".rustc_info.json",
                    UNPUBLISHED_OWNER_MARKER,
                ]
                .contains(&name.as_ref())
        })
}

/// Whether the exact current build key already has a published immutable
/// baseline. `READY` is written last, so this lock-free admission read can only
/// return false during an in-progress promotion, never observe a partial true.
pub fn has_ready_baseline(
    cache_root: &Path,
    source_root: &Path,
    controlled_command: Option<&str>,
) -> bool {
    let Some(command) = controlled_command.filter(|command| !command.trim().is_empty()) else {
        return false;
    };
    let key = compute_key(source_root, Some(command), "admission");
    key.baseline_reusable && baseline_is_ready(&baseline_dir(cache_root, &key.digest()), &key)
}

/// Prepare one private writable target. No mutable directory is shared.
pub fn prepare_layer(
    cache_root: &Path,
    source_root: &Path,
    agent_id: &str,
    controlled_command: Option<&str>,
) -> Result<TargetLayer> {
    let key = compute_key(source_root, controlled_command, agent_id);
    prepare_layer_with_key(cache_root, source_root, agent_id, key)
}

fn prepare_layer_with_key(
    cache_root: &Path,
    source_root: &Path,
    agent_id: &str,
    key: TargetCacheKey,
) -> Result<TargetLayer> {
    let digest = key.digest();
    let _lock = KeyLock::acquire(cache_root, &digest)?;
    let layer_parent = cache_root.join("layers").join(&digest).join(agent_id);
    let target = layer_parent.join("target");
    if layer_parent.exists() {
        let owned_marker = target.join(LAYER_OWNED);
        if fs::read(&owned_marker).ok().as_deref() == Some(b"wg-owned Cargo layer\n")
            && !target.join(LAYER_MANIFEST).exists()
        {
            // A marker without a finalized manifest is a crash during clone;
            // no process can have received this target yet.
            fs::remove_dir_all(&layer_parent)?;
        } else {
            bail!(
                "target cache layer already exists and is not a recoverable partial clone: {}",
                layer_parent.display()
            );
        }
    }
    fs::create_dir_all(&target)?;
    write_new(&target.join(LAYER_OWNED), b"wg-owned Cargo layer\n")?;
    let prepared = (|| -> Result<TargetLayer> {
        let baseline = baseline_dir(cache_root, &digest);
        let baseline_target = baseline.join("target");
        let baseline_path = if key.baseline_reusable && baseline_is_ready(&baseline, &key) {
            match clone_tree_private(&baseline_target, &target) {
                Ok(()) => Some(baseline_target),
                Err(error) => {
                    // A filesystem without a usable reflink is allowed to make
                    // byte copies. If even private copying fails, discard every
                    // partial seed and start cold; never keep uncertain sharing.
                    eprintln!(
                        "[target-cache] baseline seed failed safely for {}: {error:#}; starting with an empty private layer",
                        target.display()
                    );
                    clear_partial_seed(&target)?;
                    None
                }
            }
        } else {
            None
        };
        let manifest = LayerManifest {
            schema: CACHE_SCHEMA,
            key: key.clone(),
            source_root: source_root.to_string_lossy().to_string(),
            baseline_path: baseline_path
                .as_ref()
                .map(|path| path.to_string_lossy().to_string()),
        };
        write_new(
            &target.join(LAYER_MANIFEST),
            &serde_json::to_vec_pretty(&manifest)?,
        )?;
        Ok(TargetLayer {
            path: target.clone(),
            key,
            baseline_path,
        })
    })();
    if prepared.is_err() {
        let _ = fs::remove_dir_all(&layer_parent);
    }
    prepared
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn clone_excluded(relative: &Path, file_name: &std::ffi::OsStr) -> bool {
    relative == Path::new(LAYER_MANIFEST)
        || relative == Path::new(LAYER_OWNED)
        || relative == Path::new(BASELINE_MANIFEST)
        || relative == Path::new(BASELINE_OWNED)
        || relative == Path::new(READY)
        || file_name == ".cargo-lock"
        || file_name == ".rustc_info.json"
        || file_name == UNPUBLISHED_OWNER_MARKER
}

fn relative_link_stays_inside(relative: &Path, link: &Path) -> bool {
    if link.is_absolute() {
        return false;
    }
    let mut depth = relative
        .parent()
        .map_or(0, |parent| parent.components().count());
    for component in link.components() {
        match component {
            std::path::Component::Normal(_) => depth += 1,
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir if depth > 0 => depth -= 1,
            std::path::Component::ParentDir => return false,
            std::path::Component::RootDir | std::path::Component::Prefix(_) => return false,
        }
    }
    true
}

#[cfg(target_os = "linux")]
fn try_reflink(source: &Path, destination: &Path) -> Result<bool> {
    use std::os::fd::AsRawFd;
    const FICLONE: libc::c_ulong = 0x4004_9409;
    let input = File::open(source)?;
    let output = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(destination)?;
    let cloned = unsafe { libc::ioctl(output.as_raw_fd(), FICLONE, input.as_raw_fd()) } == 0;
    drop(output);
    if !cloned {
        let _ = fs::remove_file(destination);
    }
    Ok(cloned)
}

#[cfg(not(target_os = "linux"))]
fn try_reflink(_source: &Path, _destination: &Path) -> Result<bool> {
    Ok(false)
}

fn files_are_identical(source: &Path, destination: &Path) -> bool {
    let (Ok(left), Ok(right)) = (fs::read(source), fs::read(destination)) else {
        return false;
    };
    left == right
}

fn make_private_file_writable(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let source_mode = fs::metadata(source)?.permissions().mode();
        let mode = if source_mode & 0o111 == 0 {
            0o644
        } else {
            0o755
        };
        fs::set_permissions(destination, fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(destination)?.permissions();
        permissions.set_readonly(false);
        fs::set_permissions(destination, permissions)?;
    }
    Ok(())
}

fn clone_regular_file_private_with(
    source: &Path,
    destination: &Path,
    try_native_reflink: bool,
) -> Result<()> {
    let reflinked = try_native_reflink && try_reflink(source, destination)?;
    if !reflinked {
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(destination)
            .and_then(|mut output| {
                let mut input = File::open(source)?;
                std::io::copy(&mut input, &mut output)?;
                output.sync_all()
            })
            .with_context(|| {
                format!(
                    "private-copy Cargo artifact {} -> {}",
                    source.display(),
                    destination.display()
                )
            })?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let source_meta = fs::metadata(source)?;
        let destination_meta = fs::metadata(destination)?;
        if source_meta.dev() == destination_meta.dev()
            && source_meta.ino() == destination_meta.ino()
        {
            let _ = fs::remove_file(destination);
            bail!("mutable Cargo artifact clone unexpectedly reused an inode");
        }
    }
    if !files_are_identical(source, destination) {
        let _ = fs::remove_file(destination);
        bail!(
            "Cargo artifact clone verification failed: {} -> {}",
            source.display(),
            destination.display()
        );
    }
    make_private_file_writable(source, destination)
}

fn clone_regular_file_private(source: &Path, destination: &Path) -> Result<()> {
    clone_regular_file_private_with(source, destination, true)
}

fn clone_tree_private(source: &Path, destination: &Path) -> Result<()> {
    if !source.is_dir() {
        return Ok(());
    }
    for entry in walkdir::WalkDir::new(source)
        .follow_links(false)
        .min_depth(1)
    {
        let entry = entry?;
        let relative = entry.path().strip_prefix(source)?;
        if clone_excluded(relative, entry.file_name()) {
            continue;
        }
        let output = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&output)?;
        } else if entry.file_type().is_symlink() {
            let link = fs::read_link(entry.path())?;
            if !relative_link_stays_inside(relative, &link) {
                // Absolute/escaping links could retain a mutable path into the
                // baseline or another layer. Omit them and let Cargo rebuild.
                continue;
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(link, &output)?;
            #[cfg(windows)]
            {
                let followed = fs::metadata(entry.path())?;
                if followed.is_dir() {
                    std::os::windows::fs::symlink_dir(link, &output)?;
                } else {
                    std::os::windows::fs::symlink_file(link, &output)?;
                }
            }
        } else if entry.file_type().is_file() {
            clone_regular_file_private(entry.path(), &output)?;
        }
    }
    Ok(())
}

fn clear_partial_seed(target: &Path) -> Result<()> {
    let mut entries = walkdir::WalkDir::new(target)
        .follow_links(false)
        .min_depth(1)
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.depth()));
    for entry in entries {
        if entry.depth() == 1 && entry.file_name() == LAYER_OWNED {
            continue;
        }
        if entry.file_type().is_dir() {
            fs::remove_dir(entry.path())?;
        } else {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn make_baseline_read_only(root: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut entries = walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.depth()));
    for entry in entries {
        if entry.file_type().is_symlink() {
            continue;
        }
        let metadata = entry.metadata()?;
        let mode = metadata.permissions().mode();
        let readonly = if entry.file_type().is_dir() {
            0o555
        } else if mode & 0o111 != 0 {
            0o555
        } else {
            0o444
        };
        fs::set_permissions(entry.path(), fs::Permissions::from_mode(readonly))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn make_baseline_read_only(root: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_symlink() {
            let mut permissions = entry.metadata()?.permissions();
            permissions.set_readonly(true);
            fs::set_permissions(entry.path(), permissions)?;
        }
    }
    Ok(())
}

/// Promote a clean completed attempt to the immutable baseline. A layer built
/// from changed/uncommitted source is never promoted into its old namespace.
pub fn promote_layer(target: &Path) -> Result<bool> {
    let manifest = validated_layer_manifest(target)
        .ok_or_else(|| anyhow!("invalid target layer manifest/layout: {}", target.display()))?;
    let source_root = PathBuf::from(&manifest.source_root);
    if !manifest.key.baseline_reusable || source_is_dirty(&source_root) {
        return Ok(false);
    }
    // Publish only the exact key used to prepare the layer. A commit after the
    // last successful build may make the source clean while changing its tree;
    // promoting those old outputs under the new key would poison the baseline.
    let current_key = compute_key(
        &source_root,
        Some(&manifest.key.command_identity),
        "promotion",
    );
    if current_key != manifest.key {
        return Ok(false);
    }
    promote_layer_validated(target, &manifest.key)
}

fn source_is_dirty(source_root: &Path) -> bool {
    Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=normal"])
        .current_dir(source_root)
        .output()
        .map(|output| {
            !output.status.success()
                || String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .any(|line| line != "?? .wg-cleanup-pending")
        })
        .unwrap_or(true)
}

fn promote_layer_validated(target: &Path, key: &TargetCacheKey) -> Result<bool> {
    if !key.baseline_reusable || !target_has_artifacts(target) {
        return Ok(false);
    }
    let digest = key.digest();
    let cache_root = target
        .ancestors()
        .nth(4)
        .ok_or_else(|| anyhow!("invalid target layer layout: {}", target.display()))?;
    let _lock = KeyLock::acquire(cache_root, &digest)?;
    let baseline = baseline_dir(cache_root, &digest);
    if baseline_is_ready(&baseline, key) {
        return Ok(false);
    }
    fs::create_dir_all(baseline.parent().expect("baseline parent"))?;
    if baseline.exists() {
        // A missing READY marker is repairable only with exact WG ownership
        // evidence. Never infer ownership from a digest-shaped directory name.
        if !baseline_is_owned(&baseline) {
            bail!(
                "refusing to replace unowned baseline collision: {}",
                baseline.display()
            );
        }
        make_tree_writable(&baseline)?;
        fs::remove_dir_all(&baseline)?;
    }
    fs::create_dir_all(&baseline)?;
    write_new(&baseline.join(BASELINE_OWNED), b"wg-owned Cargo baseline\n")?;
    let baseline_target = baseline.join("target");
    fs::create_dir_all(&baseline_target)?;
    clone_tree_private(target, &baseline_target)?;
    write_new(
        &baseline.join(BASELINE_MANIFEST),
        &serde_json::to_vec_pretty(key)?,
    )?;
    // READY is the publication boundary. Readers hold the same key lock while
    // cloning and never observe a partially materialized baseline.
    write_new(&baseline.join(READY), b"ready\n")?;
    make_baseline_read_only(&baseline)?;
    Ok(true)
}

#[cfg(unix)]
fn make_tree_writable(root: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        // Removing a read-only tree requires writable directories, not writable
        // files. Keeping file modes unchanged also minimizes the authority of
        // this repair path.
        if entry.file_type().is_dir() {
            fs::set_permissions(entry.path(), fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn make_tree_writable(root: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_dir() {
            let mut permissions = entry.metadata()?.permissions();
            permissions.set_readonly(false);
            fs::set_permissions(entry.path(), permissions)?;
        }
    }
    Ok(())
}

/// Extract the key digest from a canonical layer path.
pub fn layer_key_from_path(target: &Path) -> Option<String> {
    validated_layer_manifest(target).map(|manifest| manifest.key.digest())
}

pub fn layer_was_seeded_from_baseline(target: &Path) -> bool {
    validated_layer_manifest(target)
        .and_then(|manifest| manifest.baseline_path)
        .is_some_and(|path| Path::new(&path).is_dir())
}

fn validated_layer_manifest(target: &Path) -> Option<LayerManifest> {
    let manifest = fs::read(target.join(LAYER_MANIFEST)).ok()?;
    let manifest: LayerManifest = serde_json::from_slice(&manifest).ok()?;
    let digest = manifest.key.digest();
    let mut ancestors = target.ancestors();
    let target_dir = ancestors.next()?;
    let agent_dir = ancestors.next()?;
    let digest_dir = ancestors.next()?;
    let layers_dir = ancestors.next()?;
    if manifest.schema != CACHE_SCHEMA
        || fs::read(target.join(LAYER_OWNED)).ok().as_deref() != Some(b"wg-owned Cargo layer\n")
        || target_dir.file_name()?.to_str()? != "target"
        || agent_dir.file_name()?.is_empty()
        || digest_dir.file_name()?.to_str()? != digest
        || layers_dir.file_name()?.to_str()? != "layers"
    {
        return None;
    }
    Some(manifest)
}

/// Remove empty per-agent/key directory shells after the exact owned `target`
/// path has been reaped. This never traverses above the cache's `layers` root.
pub fn prune_empty_layer_parents(cache_root: &Path, target: &Path) {
    let layers = cache_root.join("layers");
    let mut current = target.parent().map(Path::to_path_buf);
    while let Some(path) = current {
        if path == layers || !path.starts_with(&layers) {
            break;
        }
        if fs::remove_dir(&path).is_err() {
            break;
        }
        current = path.parent().map(Path::to_path_buf);
    }
}

/// Logical bytes in `path` and a conservative physical charge. Reflink extent
/// sharing is not safely inferable from inode link counts, so cloned files are
/// charged to each layer even though the filesystem stores shared extents once.
/// Cargo-created hard links wholly inside one tree are charged once. Discover
/// valid layer keys, including the short prepare→registry publication
/// window. This closes the race where baseline GC could unlink a lower after a
/// clone completed but before its ownership row was committed.
pub fn existing_layer_keys(cache_root: &Path, limit: usize) -> HashSet<String> {
    let root = cache_root.join("layers");
    walkdir::WalkDir::new(root)
        .follow_links(false)
        .min_depth(3)
        .max_depth(3)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_dir() && entry.file_name() == "target")
        .take(limit)
        .filter_map(|entry| layer_key_from_path(entry.path()))
        .collect()
}

/// Remove superseded immutable baselines while protecting every key referenced
/// by a live layer. One newest inactive baseline is retained as a warm fallback.
/// Incomplete crash remnants are always eligible once their key is inactive.
pub fn gc_superseded_baselines(
    cache_root: &Path,
    active_keys: &HashSet<String>,
) -> Result<Vec<(PathBuf, u64)>> {
    let root = cache_root.join("baselines");
    let mut candidates = fs::read_dir(&root)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            entry.path().is_dir()
                && name.len() == 64
                && name.bytes().all(|byte| byte.is_ascii_hexdigit())
                && baseline_is_owned(&entry.path())
        })
        .filter(|entry| !active_keys.contains(&entry.file_name().to_string_lossy().to_string()))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|entry| {
        std::cmp::Reverse(
            entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
        )
    });
    let mut removed = Vec::new();
    let mut retained_ready = false;
    for entry in candidates {
        let path = entry.path();
        let ready = path.join(READY).is_file() && path.join(BASELINE_MANIFEST).is_file();
        if ready && !retained_ready {
            retained_ready = true;
            continue;
        }
        let digest = entry.file_name().to_string_lossy().to_string();
        let _lock = KeyLock::acquire(cache_root, &digest)?;
        if active_keys.contains(&digest) {
            continue;
        }
        let bytes = layer_bytes(&path).1;
        make_tree_writable(&path)?;
        fs::remove_dir_all(&path)?;
        removed.push((path, bytes));
    }
    Ok(removed)
}

fn baseline_is_owned(path: &Path) -> bool {
    fs::read(path.join(BASELINE_OWNED)).ok().as_deref() == Some(b"wg-owned Cargo baseline\n")
}

/// Return the baseline key containing an artifact path, if it is inside the
/// exact owned cache layout. Callers use this to protect registered artifacts
/// from baseline GC.
pub fn baseline_key_containing(cache_root: &Path, artifact: &Path) -> Option<String> {
    let baselines = cache_root.join("baselines");
    let absolute = if artifact.is_absolute() {
        artifact.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(artifact)
    };
    let relative = absolute.strip_prefix(&baselines).ok()?;
    let key = relative.components().next()?.as_os_str().to_string_lossy();
    (key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit())).then(|| key.to_string())
}

pub fn layer_bytes(path: &Path) -> (u64, u64) {
    let mut logical = 0u64;
    let mut private = 0u64;
    #[cfg(unix)]
    let root_inode_counts = validated_layer_manifest(path)
        .is_none()
        .then(|| inode_counts_under(path));
    #[cfg(unix)]
    let mut charged_inodes = HashSet::new();
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        logical = logical.saturating_add(metadata.len());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let inode = (metadata.dev(), metadata.ino());
            // Cargo may hard-link its own private executable/deps paths. Charge
            // that inode once. A link outside an unmanifested root is excluded;
            // nlink alone is never treated as evidence of reflink sharing.
            let externally_linked = root_inode_counts
                .as_ref()
                .is_some_and(|counts| metadata.nlink() > counts.get(&inode).copied().unwrap_or(0));
            if !externally_linked && charged_inodes.insert(inode) {
                private = private.saturating_add(metadata.blocks().saturating_mul(512));
            }
        }
        #[cfg(not(unix))]
        {
            private = private.saturating_add(metadata.len());
        }
    }
    (logical, private)
}

#[cfg(unix)]
fn inode_counts_under(path: &Path) -> HashMap<(u64, u64), u64> {
    use std::os::unix::fs::MetadataExt;
    let mut counts = HashMap::new();
    for metadata in walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| entry.metadata().ok())
    {
        *counts.entry((metadata.dev(), metadata.ino())).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn key(source: &str) -> TargetCacheKey {
        TargetCacheKey {
            schema: CACHE_SCHEMA,
            source_baseline: source.into(),
            cargo_lock: "lock".into(),
            cargo_inputs: "inputs".into(),
            rustc: "rustc 1.96 host:x86_64-unknown-linux-gnu".into(),
            target_triple: "x86_64-unknown-linux-gnu".into(),
            working_directory: ".".into(),
            cargo_home: "/tmp/cargo-home".into(),
            toolchain: "stable".into(),
            accepted_environment: String::new(),
            features: "default".into(),
            profile: "test".into(),
            flags: "incremental=0".into(),
            command_identity: "cargo test --locked".into(),
            baseline_reusable: true,
        }
    }

    #[test]
    fn metadata_only_layer_is_not_published_as_a_warm_cargo_baseline() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let layer = prepare_layer_with_key(temp.path(), &source, "agent", key("empty")).unwrap();
        assert!(!promote_layer_validated(&layer.path, &layer.key).unwrap());
        assert!(!baseline_dir(temp.path(), &layer.key.digest()).exists());
    }

    #[cfg(unix)]
    #[test]
    fn forced_reflink_failure_falls_back_to_private_writable_bytes() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::write(&source, vec![9u8; 1024 * 1024]).unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o444)).unwrap();
        clone_regular_file_private_with(&source, &destination, false).unwrap();
        assert!(files_are_identical(&source, &destination));
        assert_ne!(
            fs::metadata(&source).unwrap().ino(),
            fs::metadata(&destination).unwrap().ino()
        );
        OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&destination)
            .unwrap()
            .write_all(b"private")
            .unwrap();
        assert_eq!(fs::metadata(&source).unwrap().len(), 1024 * 1024);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reflink_capability_uses_shared_extents_without_shared_inodes() {
        use std::os::unix::fs::MetadataExt;
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("large-source");
        let destination = temp.path().join("large-clone");
        let mut file = File::create(&source).unwrap();
        let block = vec![0x5au8; 1024 * 1024];
        for _ in 0..32 {
            file.write_all(&block).unwrap();
        }
        file.sync_all().unwrap();
        drop(file);
        let mut before: libc::statvfs = unsafe { std::mem::zeroed() };
        let path = std::ffi::CString::new(temp.path().as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::statvfs(path.as_ptr(), &mut before) }, 0);

        if !try_reflink(&source, &destination).unwrap() {
            // Capability absence exercises the byte-copy fallback elsewhere;
            // it is safe but intentionally makes no deduplication claim.
            return;
        }
        let output = OpenOptions::new().write(true).open(&destination).unwrap();
        output.sync_all().unwrap();
        drop(output);
        let mut after: libc::statvfs = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::statvfs(path.as_ptr(), &mut after) }, 0);
        let allocated = before
            .f_bfree
            .saturating_sub(after.f_bfree)
            .saturating_mul(after.f_frsize as _);
        assert!(
            allocated < 8 * 1024 * 1024,
            "verified reflink unexpectedly allocated {allocated} bytes"
        );
        assert_ne!(
            fs::metadata(&source).unwrap().ino(),
            fs::metadata(&destination).unwrap().ino()
        );
        assert!(files_are_identical(&source, &destination));
    }

    #[cfg(unix)]
    #[test]
    fn same_baseline_layers_use_distinct_writable_inodes() {
        use std::os::unix::fs::MetadataExt;
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let first = prepare_layer_with_key(temp.path(), &source, "agent-1", key("a")).unwrap();
        fs::create_dir_all(first.path.join("debug/deps")).unwrap();
        fs::write(first.path.join("debug/deps/libsame.rlib"), vec![7; 4096]).unwrap();
        assert!(promote_layer_validated(&first.path, &first.key).unwrap());

        let second = prepare_layer_with_key(temp.path(), &source, "agent-2", key("a")).unwrap();
        let third = prepare_layer_with_key(temp.path(), &source, "agent-3", key("a")).unwrap();
        let base = second
            .baseline_path
            .unwrap()
            .join("debug/deps/libsame.rlib");
        let upper = second.path.join("debug/deps/libsame.rlib");
        let sibling = third.path.join("debug/deps/libsame.rlib");
        let inodes = [
            fs::metadata(&base).unwrap().ino(),
            fs::metadata(&upper).unwrap().ino(),
            fs::metadata(&sibling).unwrap().ino(),
        ];
        assert_ne!(inodes[0], inodes[1]);
        assert_ne!(inodes[0], inodes[2]);
        assert_ne!(inodes[1], inodes[2]);
        OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&upper)
            .unwrap()
            .write_all(b"private")
            .unwrap();
        assert_eq!(fs::read(&base).unwrap(), vec![7; 4096]);
        assert_eq!(fs::read(&sibling).unwrap(), vec![7; 4096]);
    }

    #[cfg(unix)]
    #[test]
    fn adversarial_mutation_of_every_layer_entry_keeps_baseline_and_sibling_immutable() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

        fn snapshot(root: &Path) -> Vec<(String, String, u64)> {
            let mut values = walkdir::WalkDir::new(root)
                .follow_links(false)
                .min_depth(1)
                .into_iter()
                .filter_map(|entry| entry.ok())
                .filter(|entry| {
                    !clone_excluded(entry.path().strip_prefix(root).unwrap(), entry.file_name())
                })
                .map(|entry| {
                    let relative = entry
                        .path()
                        .strip_prefix(root)
                        .unwrap()
                        .display()
                        .to_string();
                    if entry.file_type().is_symlink() {
                        (
                            relative,
                            format!("link:{}", fs::read_link(entry.path()).unwrap().display()),
                            0,
                        )
                    } else if entry.file_type().is_dir() {
                        (
                            relative,
                            "directory".to_string(),
                            entry.metadata().unwrap().ino(),
                        )
                    } else {
                        (
                            relative,
                            blake3::hash(&fs::read(entry.path()).unwrap())
                                .to_hex()
                                .to_string(),
                            entry.metadata().unwrap().ino(),
                        )
                    }
                })
                .collect::<Vec<_>>();
            values.sort();
            values
        }

        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let warm = prepare_layer_with_key(temp.path(), &source, "warm", key("same")).unwrap();
        fs::create_dir_all(warm.path.join("debug/nested")).unwrap();
        fs::write(warm.path.join("debug/truncate-me"), vec![1; 8192]).unwrap();
        fs::write(warm.path.join("debug/overwrite-me"), vec![2; 4096]).unwrap();
        fs::write(warm.path.join("debug/nested/rename-me"), b"nested bytes").unwrap();
        fs::set_permissions(
            warm.path.join("debug/overwrite-me"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        symlink("truncate-me", warm.path.join("debug/link-me")).unwrap();
        assert!(promote_layer_validated(&warm.path, &warm.key).unwrap());

        let victim = prepare_layer_with_key(temp.path(), &source, "victim", key("same")).unwrap();
        let sibling = prepare_layer_with_key(temp.path(), &source, "sibling", key("same")).unwrap();
        let baseline = victim.baseline_path.clone().unwrap();
        let baseline_before = snapshot(&baseline);
        let sibling_before = snapshot(&sibling.path);

        for relative in [
            "debug/truncate-me",
            "debug/overwrite-me",
            "debug/nested/rename-me",
        ] {
            let baseline_inode = fs::metadata(baseline.join(relative)).unwrap().ino();
            let victim_inode = fs::metadata(victim.path.join(relative)).unwrap().ino();
            let sibling_inode = fs::metadata(sibling.path.join(relative)).unwrap().ino();
            assert_ne!(baseline_inode, victim_inode, "{relative}");
            assert_ne!(baseline_inode, sibling_inode, "{relative}");
            assert_ne!(victim_inode, sibling_inode, "{relative}");
        }

        OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(victim.path.join("debug/truncate-me"))
            .unwrap()
            .write_all(b"short")
            .unwrap();
        fs::write(victim.path.join("debug/overwrite-me"), b"overwritten").unwrap();
        fs::rename(
            victim.path.join("debug/nested/rename-me"),
            victim.path.join("debug/nested/renamed-file"),
        )
        .unwrap();
        fs::rename(
            victim.path.join("debug/link-me"),
            victim.path.join("debug/renamed-link"),
        )
        .unwrap();
        fs::rename(
            victim.path.join("debug/nested"),
            victim.path.join("debug/renamed-directory"),
        )
        .unwrap();

        assert_eq!(snapshot(&baseline), baseline_before);
        assert_eq!(snapshot(&sibling.path), sibling_before);
    }

    #[cfg(unix)]
    #[test]
    fn divergent_layer_replacement_does_not_clobber_immutable_baseline() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let first = prepare_layer_with_key(temp.path(), &source, "agent-1", key("a")).unwrap();
        fs::write(first.path.join("artifact"), "baseline").unwrap();
        promote_layer_validated(&first.path, &first.key).unwrap();
        let second = prepare_layer_with_key(temp.path(), &source, "agent-2", key("a")).unwrap();
        let replacement = second.path.join("replacement");
        fs::write(&replacement, "diverged").unwrap();
        fs::rename(&replacement, second.path.join("artifact")).unwrap();
        assert_eq!(
            fs::read_to_string(second.path.join("artifact")).unwrap(),
            "diverged"
        );
        assert_eq!(
            fs::read_to_string(second.baseline_path.unwrap().join("artifact")).unwrap(),
            "baseline"
        );
    }

    #[test]
    fn clean_source_changed_after_build_is_not_promoted_under_a_different_key() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&source)
            .status()
            .unwrap();
        fs::write(
            source.join("Cargo.toml"),
            "[package]\nname='a'\nversion='0.1.0'\n",
        )
        .unwrap();
        std::process::Command::new("git")
            .args(["add", "Cargo.toml"])
            .current_dir(&source)
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args([
                "-c",
                "user.name=WG Test",
                "-c",
                "user.email=wg@example.invalid",
                "commit",
                "-qm",
                "one",
            ])
            .current_dir(&source)
            .status()
            .unwrap();
        let layer =
            prepare_layer(temp.path(), &source, "agent", Some("cargo test --locked")).unwrap();
        fs::write(layer.path.join("artifact"), "built-before-change").unwrap();
        fs::write(
            source.join("Cargo.toml"),
            "[package]\nname='b'\nversion='0.1.0'\n",
        )
        .unwrap();
        std::process::Command::new("git")
            .args(["add", "Cargo.toml"])
            .current_dir(&source)
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args([
                "-c",
                "user.name=WG Test",
                "-c",
                "user.email=wg@example.invalid",
                "commit",
                "-qm",
                "two",
            ])
            .current_dir(&source)
            .status()
            .unwrap();
        assert!(!promote_layer(&layer.path).unwrap());
    }

    #[test]
    fn digest_shaped_unowned_baseline_collision_is_never_deleted() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let layer =
            prepare_layer_with_key(temp.path(), &source, "agent", key("collision")).unwrap();
        fs::write(layer.path.join("artifact"), "built").unwrap();
        let collision = baseline_dir(temp.path(), &layer.key.digest());
        fs::create_dir_all(&collision).unwrap();
        fs::write(collision.join("valuable"), "not WG-owned").unwrap();
        assert!(promote_layer_validated(&layer.path, &layer.key).is_err());
        assert_eq!(
            fs::read_to_string(collision.join("valuable")).unwrap(),
            "not WG-owned"
        );
    }

    #[test]
    fn crash_remnant_is_repaired_and_superseded_baseline_gc_protects_active_key() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let first_key = key("active");
        let first =
            prepare_layer_with_key(temp.path(), &source, "agent-1", first_key.clone()).unwrap();
        fs::write(first.path.join("artifact"), "active").unwrap();
        // Simulate a crash between baseline directory creation and READY.
        let incomplete = baseline_dir(temp.path(), &first_key.digest());
        fs::create_dir_all(incomplete.join("target")).unwrap();
        fs::write(incomplete.join(BASELINE_OWNED), "wg-owned Cargo baseline\n").unwrap();
        fs::write(incomplete.join("target/partial"), "partial").unwrap();
        assert!(promote_layer_validated(&first.path, &first_key).unwrap());
        assert!(!incomplete.join("target/partial").exists());

        for (idx, source_id) in ["old-a", "old-b"].into_iter().enumerate() {
            let layer =
                prepare_layer_with_key(temp.path(), &source, &format!("old-{idx}"), key(source_id))
                    .unwrap();
            fs::write(layer.path.join("artifact"), source_id).unwrap();
            promote_layer_validated(&layer.path, &layer.key).unwrap();
        }
        let active = HashSet::from([first_key.digest()]);
        let removed = gc_superseded_baselines(temp.path(), &active).unwrap();
        assert_eq!(removed.len(), 1);
        assert!(baseline_is_ready(&incomplete, &first_key));
    }

    #[test]
    fn key_invalidates_every_semantic_dimension() {
        let original = key("a");
        let mut variants = Vec::new();
        macro_rules! changed {
            ($field:ident, $value:expr) => {{
                let mut k = original.clone();
                k.$field = $value.into();
                variants.push(k);
            }};
        }
        changed!(source_baseline, "b");
        changed!(cargo_lock, "other-lock");
        changed!(cargo_inputs, "other-inputs");
        changed!(rustc, "other-rustc");
        changed!(target_triple, "aarch64-unknown-linux-gnu");
        changed!(working_directory, "crates/child");
        changed!(cargo_home, "/other/cargo-home");
        changed!(toolchain, "nightly");
        changed!(accepted_environment, "FOO=bar");
        changed!(features, "telegram");
        changed!(profile, "test-full-debug");
        changed!(flags, "RUSTFLAGS=-Ctarget-cpu=native");
        changed!(command_identity, "cargo test --release");
        assert!(
            variants
                .iter()
                .all(|variant| variant.digest() != original.digest())
        );
    }

    #[test]
    fn controlled_command_accepts_only_tiny_stateless_grammar() {
        assert_eq!(
            controlled_cargo_command(
                "CARGO_HOME=/tmp/ch RUSTUP_TOOLCHAIN=nightly RUSTFLAGS='-C target-cpu=native' cargo build --release --target aarch64-unknown-linux-gnu --features x --config net.offline=true && sleep 10"
            )
            .as_deref(),
            Some(
                "CARGO_HOME=/tmp/ch RUSTUP_TOOLCHAIN=nightly RUSTFLAGS='-C target-cpu=native' cargo build --release --target aarch64-unknown-linux-gnu --features x --config net.offline=true && sleep 10"
            )
        );
        assert!(controlled_cargo_command(
            "cargo build --quiet && sleep 10 && wg wait \"$WG_TASK_ID\" --until message --checkpoint 'storage fixture complete'"
        )
        .is_some());
        for ambiguous in [
            "export RUSTFLAGS=-Copt-level=3; cargo build",
            "cd other-dir; cargo build",
            "(cargo build)",
            "f() { cargo build; }; f",
            "cargo build > build.log",
            "cargo build --features $FEATURES",
            "cargo build | tee build.log",
            "cargo build && touch done",
            "env CARGO_HOME=/tmp/ch cargo build",
            "CARGO_TARGET_DIR=/tmp/escape cargo build",
            "echo no-build",
        ] {
            assert!(
                controlled_cargo_command(ambiguous).is_none(),
                "unexpectedly attested: {ambiguous}"
            );
        }
    }

    #[test]
    fn exact_command_identity_separates_profile_target_features_rustflags_and_config() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("Cargo.toml"), "[workspace]\n").unwrap();
        let commands = [
            "cargo test",
            "cargo test --release",
            "cargo test --target aarch64-unknown-linux-gnu",
            "cargo test --features telegram",
            "cargo test --no-default-features",
            "cargo test --all-features",
            "RUSTFLAGS='-C target-cpu=native' cargo test",
            "CARGO_HOME=/tmp/cargo-home cargo test",
            "RUSTUP_TOOLCHAIN=nightly cargo test",
            "FOO=one cargo test",
            "FOO=two cargo test",
            "cargo --config net.offline=true test",
        ];
        let digests = commands
            .map(|command| compute_key(&source, Some(command), "unused").digest())
            .into_iter()
            .collect::<HashSet<_>>();
        assert_eq!(digests.len(), commands.len());
        assert!(
            commands
                .iter()
                .all(|command| compute_key(&source, Some(command), "unused").baseline_reusable)
        );

        fs::write(source.join("extra-config.toml"), "[net]\noffline=true\n").unwrap();
        let before = compute_key(
            &source,
            Some("cargo --config extra-config.toml test"),
            "unused",
        );
        fs::write(source.join("extra-config.toml"), "[net]\noffline=false\n").unwrap();
        let after = compute_key(
            &source,
            Some("cargo --config extra-config.toml test"),
            "unused",
        );
        assert_ne!(before.digest(), after.digest());
    }

    #[test]
    fn unknown_or_ambiguous_command_identity_is_attempt_isolated_and_never_reusable() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("Cargo.toml"), "[workspace]\n").unwrap();
        let left = compute_key(
            &source,
            Some("export RUSTFLAGS=-Copt-level=3; cargo build"),
            "agent-left",
        );
        let right = compute_key(&source, Some("cd other-dir; cargo build"), "agent-right");
        assert!(!left.baseline_reusable);
        assert!(!right.baseline_reusable);
        assert_ne!(left.digest(), right.digest());

        let layer = prepare_layer_with_key(temp.path(), &source, "agent", left).unwrap();
        fs::write(layer.path.join("artifact"), "private").unwrap();
        assert!(!promote_layer_validated(&layer.path, &layer.key).unwrap());
    }
}
