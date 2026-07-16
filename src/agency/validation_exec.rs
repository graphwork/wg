//! Evaluator-executes-validation: run a task's machine-checkable validation
//! commands instead of trusting the agent's narrative.
//!
//! ## Why this exists — the three phantom completions
//!
//! Shipping one flake fix (`session_lock.rs`, PR #53) took four dispatch
//! attempts because **three agents in a row reported the task done without ever
//! pushing a branch — and the evaluator passed their narratives** (Diary §19;
//! `06-hybrid-team-review.md` §2.4). The root cause is a *design* inversion: a
//! cheap `claude:haiku` evaluator scored an expensive `claude:opus` agent's
//! confident prose. The cheapest model was judging the most expensive model's
//! narrative — exactly the setup that produces phantom passes.
//!
//! The `task-authoring-standard` (`docs/13-task-validation-standard.md`) fixed
//! the authoring half: every external-deliverable task must carry a
//! machine-checkable command in its `## Validation` block — a command that
//! exits non-zero when the deliverable is absent. This module is the enforcement
//! half: the evaluator **executes** those commands and a non-zero exit forces
//! the task to FAIL, regardless of how confident the agent's write-up is.
//! Artifact existence, not confident narrative, becomes the pass condition.
//!
//! ## Security boundary (the sandbox)
//!
//! Executing strings pulled from a task description is inherently dangerous, so
//! the executor is conservative by construction:
//!
//! * **Allowlist only.** A line is executed only if *every* command in it is on
//!   [`ALLOWLIST`] — `git ls-remote`, `gh pr view`, `gh pr list`, `test`,
//!   `grep`. These are the read-only "does the deliverable exist?" checks from
//!   `docs/13` §3. Anything else (`cargo`, `rm`, `curl`, a bare prose bullet…)
//!   is **skipped, never run**.
//! * **Argument-aware parsing, not head-matching.** A line is tokenized with a
//!   quote-aware parser ([`parse_line`]) that recognizes exactly four control
//!   operators — `&&`, `||`, `|`, `;` — and REJECTS every other shell
//!   metacharacter: a lone `&` (background), `<`/`>` (redirection), `(`/`)`
//!   (subshell), a backtick or `$` (command / variable substitution), a `\`
//!   escape, and newlines. Because we spawn the parsed `argv` *directly* (never
//!   through `sh -c`), there is no shell re-parse that could resurrect a
//!   rejected operator — closing the "`test -f x & <cmd>`" background escape.
//! * **Per-command option validation.** Each command's head AND its options are
//!   checked ([`validate_argv`]): `git ls-remote`'s `--upload-pack=<program>` /
//!   `-u` (which make git *execute* a program) and other protocol-escape
//!   options are refused, and only a small read-only option set is accepted.
//!   Head allowlisting alone is not enough — `git ls-remote` is not intrinsically
//!   read-only.
//! * **Direct spawning + process-group timeout.** Every command is spawned in
//!   its OWN process group (Unix `setsid`); a per-line deadline kills and reaps
//!   the WHOLE group — leader plus any pipeline members / descendants — so a
//!   hung check cannot leak a surviving child that keeps inherited pipes open.
//! * **Skip is safe; only run-and-fail fails a task.** A line we refuse to run
//!   never forces a failure — it simply is not machine-verified. A task is
//!   forced to FAIL only when an allowlisted command actually *ran* and exited
//!   non-zero (a timeout counts as a run-and-fail). This bias means the
//!   evaluator can never fail a task because it declined to execute something.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Command heads the evaluator is permitted to execute. These are all
/// read-only existence checks — none of them mutate the repo or the remote.
/// The two-word entries (`git ls-remote`, `gh pr view`, `gh pr list`) pin the
/// *subcommand* so a bare `git` / `gh` (which could push, delete, merge…) is
/// never allowed.
pub const ALLOWLIST: &[&str] = &["git ls-remote", "gh pr view", "gh pr list", "test", "grep"];

/// Default per-command validation timeout, in seconds. A single existence check
/// (`git ls-remote`, `gh pr view/list`, `test`, `grep`) should finish in well
/// under a minute; anything longer is a hang. Without this bound a wedged
/// `gh`/`git` network call (or a `grep` over a pathological path) blocks the
/// evaluator indefinitely — the failure mode Erik flagged on PR #57.
pub const DEFAULT_VALIDATION_TIMEOUT_SECS: u64 = 60;

/// Environment variable that overrides [`DEFAULT_VALIDATION_TIMEOUT_SECS`].
pub const VALIDATION_TIMEOUT_ENV: &str = "WG_VALIDATION_TIMEOUT_SECS";

/// Resolve the per-command timeout: `WG_VALIDATION_TIMEOUT_SECS` when it is set
/// to a positive integer, otherwise [`DEFAULT_VALIDATION_TIMEOUT_SECS`]. A blank
/// / unparseable / zero value falls back to the default (zero would disable the
/// safety net, which is never what an operator means).
pub fn validation_timeout_secs() -> u64 {
    std::env::var(VALIDATION_TIMEOUT_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .unwrap_or(DEFAULT_VALIDATION_TIMEOUT_SECS)
}

/// The result of considering (and possibly executing) one validation command.
#[derive(Debug, Clone)]
pub struct CommandResult {
    /// The cleaned command string (list marker stripped).
    pub command: String,
    /// Whether the command was actually executed. `false` means it was skipped
    /// because it is not on the allowlist (see [`skip_reason`](Self::skip_reason)).
    pub ran: bool,
    /// If the command was skipped, why.
    pub skip_reason: Option<String>,
    /// Process exit code, if the command ran.
    pub exit_code: Option<i32>,
    /// Whether the command ran AND exited 0.
    pub passed: bool,
    /// Whether the command was killed for exceeding the per-command timeout.
    /// A timed-out command is a failure (`ran == true`, `passed == false`) —
    /// a hung validation check can never be allowed to pass silently.
    pub timed_out: bool,
}

/// The outcome of executing all machine-checkable validation commands in a
/// task's `## Validation` block.
#[derive(Debug, Clone, Default)]
pub struct ValidationOutcome {
    /// Per-command results, in the order they appeared.
    pub results: Vec<CommandResult>,
    /// The per-command timeout (seconds) that was applied to this run, for
    /// reporting. Zero on the default-constructed (no-command) outcome.
    pub timeout_secs: u64,
}

impl ValidationOutcome {
    /// Did at least one allowlisted command actually run?
    pub fn any_ran(&self) -> bool {
        self.results.iter().any(|r| r.ran)
    }

    /// Did any allowlisted command run and fail (non-zero exit)? This is the
    /// trust-inversion trigger: a single failed artifact check fails the task.
    pub fn has_failure(&self) -> bool {
        self.results.iter().any(|r| r.ran && !r.passed)
    }

    /// The commands that ran and failed — used to build the failure message.
    pub fn failures(&self) -> Vec<&CommandResult> {
        self.results.iter().filter(|r| r.ran && !r.passed).collect()
    }

    /// Did any command exceed the per-command timeout?
    pub fn any_timed_out(&self) -> bool {
        self.results.iter().any(|r| r.timed_out)
    }

    /// A one-line human-readable summary of what ran and what failed.
    pub fn summary(&self) -> String {
        let ran = self.results.iter().filter(|r| r.ran).count();
        let failed = self.failures().len();
        let timed_out = self.results.iter().filter(|r| r.timed_out).count();
        let skipped = self.results.len() - ran;
        format!(
            "validation commands: {} ran, {} failed ({} timed out), {} skipped (not allowlisted)",
            ran, failed, timed_out, skipped
        )
    }
}

/// Extract the body of the `## Validation` section from a task description, if
/// present. Returns everything between the `## Validation` header and the next
/// `## ` header (or end of string). Matching is case-insensitive on the header
/// text and tolerant of extra `#`.
pub fn validation_section(description: &str) -> Option<&str> {
    let mut start: Option<usize> = None;
    let mut end: Option<usize> = None;
    for (offset, line) in line_offsets(description) {
        let trimmed = line.trim();
        let is_header = trimmed.starts_with('#');
        if start.is_none() {
            if is_header && header_text(trimmed).eq_ignore_ascii_case("validation") {
                // Section body begins after this header line.
                start = Some(offset + line.len());
            }
        } else if is_header {
            // Next header closes the section.
            end = Some(offset);
            break;
        }
    }
    let start = start?;
    let end = end.unwrap_or(description.len());
    Some(description[start..end].trim_matches('\n'))
}

/// Strip leading `#` and whitespace from a header line, returning the header text.
fn header_text(header_line: &str) -> &str {
    header_line.trim_start_matches('#').trim()
}

/// Iterate `(byte_offset, line_without_newline)` over `s`.
fn line_offsets(s: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    s.split_inclusive('\n').map(move |raw| {
        let start = offset;
        offset += raw.len();
        (start, raw.trim_end_matches('\n'))
    })
}

/// Extract candidate validation command strings from a task description.
///
/// Prefers the `## Validation` section; if there is none, the whole description
/// is scanned (so freeform descriptions still get their commands checked). List
/// markers (`- [ ]`, `- [x]`, `- `, `* `, `1.`) and fenced-code markers are
/// stripped. This returns *candidates* — allowlist filtering happens at
/// execution time so callers can see everything that was considered.
pub fn extract_validation_commands(description: &str) -> Vec<String> {
    let scan = validation_section(description).unwrap_or(description);
    let mut out = Vec::new();
    for line in scan.lines() {
        if let Some(cmd) = clean_command_line(line) {
            out.push(cmd);
        }
    }
    out
}

/// Turn one markdown line into a candidate command, or `None` if it is not a
/// plausible command line (blank, a header, a code fence, or pure prose with no
/// allowlisted head).
fn clean_command_line(line: &str) -> Option<String> {
    let mut s = line.trim();
    if s.is_empty() || s.starts_with("```") || s.starts_with('#') {
        return None;
    }
    // Strip an unordered list marker: `- `, `* `, `+ `.
    if let Some(rest) = s
        .strip_prefix("- ")
        .or_else(|| s.strip_prefix("* "))
        .or_else(|| s.strip_prefix("+ "))
    {
        s = rest.trim_start();
    }
    // Strip a checkbox: `[ ] ` / `[x] ` / `[X] `.
    if let Some(rest) = s
        .strip_prefix("[ ] ")
        .or_else(|| s.strip_prefix("[x] ").or_else(|| s.strip_prefix("[X] ")))
    {
        s = rest.trim_start();
    }
    // Strip an ordered list marker: `1. `, `12) `, etc.
    s = strip_ordered_marker(s);
    // Drop a trailing inline comment / parenthetical note that follows the
    // command (docs/13 writes e.g. `... -q .state  (prints OPEN or MERGED)`).
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Only keep lines whose first segment reads as a real allowlisted command.
    // This drops prose bullets ("Branch pushed", "cargo build ... pass") AND
    // prose that merely starts with a command word ("test the endpoint",
    // "grep the logs to confirm") — the signature check requires a flag or a
    // comparison, which prose lacks. Command-shaped-but-unsafe lines (e.g. a
    // pipe into a non-allowlisted tool) still survive here and are recorded as
    // skipped by `is_executable` at execution time, preserving visibility.
    let first_segment = split_segments(s).into_iter().next().unwrap_or_default();
    if !passes_segment_signature(first_segment.trim()) {
        return None;
    }
    Some(s.to_string())
}

/// Strip a leading ordered-list marker like `1. ` or `3) `.
fn strip_ordered_marker(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0 && i < bytes.len() && (bytes[i] == b'.' || bytes[i] == b')') {
        let rest = &s[i + 1..];
        if rest.starts_with(' ') {
            return rest.trim_start();
        }
    }
    s
}

/// Does a command string start with one of the allowlisted heads?
fn starts_with_allowlisted(cmd: &str) -> bool {
    let normalized = normalize_ws(cmd);
    ALLOWLIST
        .iter()
        .any(|head| normalized == *head || normalized.starts_with(&format!("{} ", head)))
}

/// Collapse runs of whitespace to single spaces and trim.
fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Is this whole command line safe to execute under the sandbox rules?
///
/// The line is tokenized with an argument-aware, quote-aware parser
/// ([`parse_line`]) that understands exactly four control operators — `&&`,
/// `||`, `|`, `;` — and REJECTS every other shell metacharacter: a lone `&`
/// (background), `<`/`>` (redirection), `(`/`)` (subshell), a backtick / `$`
/// (command & variable substitution), a `\` escape, and newlines. Each
/// resulting `argv` must then pass [`validate_argv`], which pins the command
/// head to the allowlist AND enforces a per-command option policy (e.g. the
/// `git ls-remote --upload-pack=<program>` exec escape is refused). Because the
/// vetted argv is exactly what [`run_sandboxed`] spawns — directly, never
/// through `sh -c` — there is no shell re-parse that could resurrect a rejected
/// operator or option.
pub fn is_executable(cmd: &str) -> Result<(), String> {
    let stages = parse_line(cmd)?;
    for stage in &stages {
        for argv in &stage.pipeline {
            validate_argv(argv)?;
        }
    }
    Ok(())
}

/// One control operator connecting a pipeline to the PREVIOUS pipeline in a
/// line. `First` marks the leading pipeline (no predecessor).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Conn {
    First,
    And,  // &&
    Or,   // ||
    Semi, // ;
}

/// A pipeline (`cmd | cmd | …`) plus the operator that joins it to the previous
/// stage. Each inner `Vec<String>` is one command's fully-tokenized argv.
#[derive(Debug, Clone)]
struct Stage {
    pipeline: Vec<Vec<String>>,
    conn: Conn,
}

/// Lexical token: a shell word (quotes resolved) or a supported operator.
#[derive(Debug, PartialEq)]
enum Tok {
    Word(String),
    And,
    Or,
    Pipe,
    Semi,
}

/// Tokenize `line` into words and the four supported operators, honoring single
/// and double quotes. ANY other shell metacharacter is a hard error — this is
/// the parity guarantee that makes direct spawning safe: the tokens we vet are
/// exactly what will be spawned, so nothing can be smuggled past into `sh`.
fn tokenize(line: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut i = 0usize;
    let mut toks: Vec<Tok> = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;

    while i < n {
        let c = chars[i];
        match c {
            ' ' | '\t' => {
                if in_word {
                    toks.push(Tok::Word(std::mem::take(&mut cur)));
                    in_word = false;
                }
                i += 1;
            }
            '\n' | '\r' => return Err("line contains a newline".to_string()),
            '\'' => {
                // Single quotes: everything literal until the closing quote.
                in_word = true;
                i += 1;
                let mut closed = false;
                while i < n {
                    if chars[i] == '\'' {
                        closed = true;
                        i += 1;
                        break;
                    }
                    cur.push(chars[i]);
                    i += 1;
                }
                if !closed {
                    return Err("unterminated single quote".to_string());
                }
            }
            '"' => {
                // Double quotes: literal, but `$`/backtick/`\` would be special
                // to a shell, so refuse them even here (defense in depth).
                in_word = true;
                i += 1;
                let mut closed = false;
                while i < n {
                    match chars[i] {
                        '"' => {
                            closed = true;
                            i += 1;
                            break;
                        }
                        '`' => {
                            return Err("contains a backtick (command substitution)".to_string());
                        }
                        '$' => {
                            return Err("contains `$` (variable/command substitution)".to_string());
                        }
                        '\\' => return Err("contains a backslash escape".to_string()),
                        d => {
                            cur.push(d);
                            i += 1;
                        }
                    }
                }
                if !closed {
                    return Err("unterminated double quote".to_string());
                }
            }
            '&' => {
                if in_word {
                    toks.push(Tok::Word(std::mem::take(&mut cur)));
                    in_word = false;
                }
                if i + 1 < n && chars[i + 1] == '&' {
                    toks.push(Tok::And);
                    i += 2;
                } else {
                    return Err(
                        "contains a lone `&` (background execution is not supported)".to_string(),
                    );
                }
            }
            '|' => {
                if in_word {
                    toks.push(Tok::Word(std::mem::take(&mut cur)));
                    in_word = false;
                }
                if i + 1 < n && chars[i + 1] == '|' {
                    toks.push(Tok::Or);
                    i += 2;
                } else {
                    toks.push(Tok::Pipe);
                    i += 1;
                }
            }
            ';' => {
                if in_word {
                    toks.push(Tok::Word(std::mem::take(&mut cur)));
                    in_word = false;
                }
                toks.push(Tok::Semi);
                i += 1;
            }
            '<' | '>' => return Err("contains a redirection (`<`/`>`)".to_string()),
            '(' | ')' => return Err("contains a subshell parenthesis".to_string()),
            '`' => return Err("contains a backtick (command substitution)".to_string()),
            '$' => return Err("contains `$` (variable/command substitution)".to_string()),
            '\\' => return Err("contains a backslash escape".to_string()),
            '{' | '}' => return Err("contains a brace (expansion)".to_string()),
            _ => {
                in_word = true;
                cur.push(c);
                i += 1;
            }
        }
    }
    if in_word {
        toks.push(Tok::Word(cur));
    }
    Ok(toks)
}

/// Parse a command line into ordered [`Stage`]s (pipelines joined by `&&` /
/// `||` / `;`). Rejects empty segments and leading/trailing operators.
fn parse_line(line: &str) -> Result<Vec<Stage>, String> {
    let toks = tokenize(line)?;
    let mut stages: Vec<Stage> = Vec::new();
    let mut pipeline: Vec<Vec<String>> = Vec::new();
    let mut simple: Vec<String> = Vec::new();
    let mut conn = Conn::First;

    for t in toks {
        match t {
            Tok::Word(w) => simple.push(w),
            Tok::Pipe => {
                if simple.is_empty() {
                    return Err("`|` with no preceding command".to_string());
                }
                pipeline.push(std::mem::take(&mut simple));
            }
            Tok::And | Tok::Or | Tok::Semi => {
                if simple.is_empty() {
                    return Err("control operator with no preceding command".to_string());
                }
                pipeline.push(std::mem::take(&mut simple));
                stages.push(Stage {
                    pipeline: std::mem::take(&mut pipeline),
                    conn,
                });
                conn = match t {
                    Tok::And => Conn::And,
                    Tok::Or => Conn::Or,
                    _ => Conn::Semi,
                };
            }
        }
    }
    if simple.is_empty() {
        if stages.is_empty() && pipeline.is_empty() {
            return Err("no command".to_string());
        }
        return Err("trailing control or pipe operator".to_string());
    }
    pipeline.push(simple);
    stages.push(Stage { pipeline, conn });
    Ok(stages)
}

/// Validate one command's argv: the head must be allowlisted, and its options
/// must satisfy that command's per-command policy. This is where the
/// `git ls-remote --upload-pack=<program>` exec escape is refused.
fn validate_argv(argv: &[String]) -> Result<(), String> {
    let head = argv.first().map(|s| s.as_str()).unwrap_or("");
    match head {
        "git" => {
            if argv.get(1).map(|s| s.as_str()) != Some("ls-remote") {
                return Err(format!(
                    "git subcommand not allowlisted: `{}`",
                    argv.get(1).cloned().unwrap_or_default()
                ));
            }
            validate_git_ls_remote(&argv[2..])
        }
        "gh" => {
            if argv.get(1).map(|s| s.as_str()) != Some("pr") {
                return Err("only `gh pr view` / `gh pr list` are allowlisted".to_string());
            }
            match argv.get(2).map(|s| s.as_str()) {
                Some("view") | Some("list") => validate_gh_pr(&argv[3..]),
                other => Err(format!(
                    "gh pr subcommand not allowlisted: `{}`",
                    other.unwrap_or_default()
                )),
            }
        }
        "test" => validate_test(&argv[1..]),
        "grep" => validate_grep(&argv[1..]),
        other => Err(format!("command not allowlisted: `{}`", other)),
    }
}

/// `git ls-remote` option policy. `--upload-pack` / `-u` hand git a program to
/// EXECUTE (the escape Erik reproduced); `-o` / `--server-option` and
/// `--receive-pack` are likewise protocol/exec vectors. Everything is refused
/// unless it is on a small read-only allowlist, so an unknown option is skipped
/// (safe) rather than forwarded to git. Non-`-` tokens are positional
/// (repository URL / ref patterns); they pass through UNLESS they name a
/// remote-helper transport (`ext::`, `fd::`, or the generic `<transport>::`
/// form), which is refused because `ext::` in particular makes git *execute*
/// the supplied program — the protocol escape Erik named alongside
/// `--upload-pack`.
fn validate_git_ls_remote(rest: &[String]) -> Result<(), String> {
    const EXEC_OPTS: &[&str] = &[
        "-u",
        "--upload-pack",
        "--exec",
        "-o",
        "--server-option",
        "--receive-pack",
    ];
    const ALLOWED: &[&str] = &[
        "--exit-code",
        "-q",
        "--quiet",
        "-h",
        "--heads",
        "-t",
        "--tags",
        "--refs",
        "--get-url",
        "--symref",
        "--sort",
        "--",
    ];
    for arg in rest {
        if !arg.starts_with('-') {
            // Positional (repository URL / ref pattern). A legitimate remote —
            // `origin`, `.`, `https://…`, `git@host:path`, `file://…` — never
            // uses the `scheme::address` remote-helper syntax, so any positional
            // whose leading `scheme::` marks a helper transport is refused. This
            // closes `git ls-remote 'ext::sh -c <payload>' .`, which would make
            // git execute the supplied program. (An IPv6 URL such as
            // `ssh://[::1]/r` is unaffected: its `::` sits after `://[`, so the
            // leading token is not a bare scheme.)
            if let Some(idx) = arg.find("::") {
                let scheme = &arg[..idx];
                let is_helper_transport = !scheme.is_empty()
                    && scheme
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'));
                if is_helper_transport {
                    return Err(format!(
                        "git ls-remote repository `{}` uses a remote-helper transport (`{}::`) that can execute a program",
                        arg, scheme
                    ));
                }
            }
            continue;
        }
        // Normalize `--flag=value` to `--flag` for matching.
        let flag = arg.split('=').next().unwrap_or(arg.as_str());
        if EXEC_OPTS.contains(&flag) {
            return Err(format!(
                "git ls-remote option `{}` can execute a program / escape the read-only contract",
                flag
            ));
        }
        if !ALLOWED.contains(&flag) {
            return Err(format!(
                "git ls-remote option `{}` is not on the read-only allowlist",
                flag
            ));
        }
    }
    Ok(())
}

/// `gh pr view` / `gh pr list` option policy. These read-only subcommands take
/// no program-executing options; we still hard-deny any `--upload-pack`-style
/// exec flag defensively and otherwise accept the subcommand's own flags.
fn validate_gh_pr(rest: &[String]) -> Result<(), String> {
    for arg in rest {
        if !arg.starts_with('-') {
            continue;
        }
        let flag = arg.split('=').next().unwrap_or(arg.as_str());
        if matches!(flag, "--upload-pack" | "--exec" | "--receive-pack") {
            return Err(format!("gh option `{}` is not permitted", flag));
        }
    }
    Ok(())
}

/// `test` needs a `-f`/`-e`/… file-test flag or a `=` / `!=` comparison; bare
/// prose ("test the endpoint") never matches.
fn validate_test(rest: &[String]) -> Result<(), String> {
    let has_flag = rest.first().map(|a| a.starts_with('-')).unwrap_or(false);
    let has_cmp = rest.iter().any(|a| a == "=" || a == "!=");
    if has_flag || has_cmp {
        Ok(())
    } else {
        Err("`test` needs a `-f`/`-e` flag or a `=`/`!=` comparison".to_string())
    }
}

/// `grep` needs a leading flag (`-q`, `-r`, …); "grep the logs" prose does not.
fn validate_grep(rest: &[String]) -> Result<(), String> {
    if rest.first().map(|a| a.starts_with('-')).unwrap_or(false) {
        Ok(())
    } else {
        Err("`grep` needs a flag (real invocations pass `-q`/`-r`…)".to_string())
    }
}

/// Split a command line into segments on `&&`, `||`, `|`, `;`.
fn split_segments(cmd: &str) -> Vec<String> {
    // Replace the two-char operators with a sentinel, then split on it and `;`/`|`.
    let replaced = cmd
        .replace("&&", "\u{0}")
        .replace("||", "\u{0}")
        .replace('|', "\u{0}")
        .replace(';', "\u{0}");
    replaced.split('\u{0}').map(|s| s.to_string()).collect()
}

/// A single segment must (a) start with an allowlisted head and (b) match that
/// head's stricter signature, so a prose sentence like "grep the logs" or
/// "test the endpoint" cannot be mistaken for a real command.
fn passes_segment_signature(seg: &str) -> bool {
    if !starts_with_allowlisted(seg) {
        return false;
    }
    let norm = normalize_ws(seg);
    if norm == "grep" || norm.starts_with("grep ") {
        // Real grep invocations from docs/13 always carry a flag first
        // (`grep -q "pat" file`). Requiring a leading flag rejects prose like
        // "grep the logs to confirm".
        return first_arg(&norm)
            .map(|a| a.starts_with('-'))
            .unwrap_or(false);
    }
    if norm == "test" || norm.starts_with("test ") {
        // `test -f X`, `test -e X`, or a comparison `test "a" = "b"`.
        return first_arg(&norm)
            .map(|a| a.starts_with('-'))
            .unwrap_or(false)
            || norm.contains(" = ")
            || norm.contains(" != ");
    }
    // git ls-remote / gh pr view / gh pr list — the two-word head already
    // pins a read-only subcommand; accept as-is.
    true
}

/// The first whitespace-delimited argument after the command head, if any.
/// Handles both one-word heads (`grep`, `test`) and two-word heads
/// (`git ls-remote`, `gh pr view`).
fn first_arg(norm: &str) -> Option<&str> {
    let head = ALLOWLIST
        .iter()
        .find(|h| norm == **h || norm.starts_with(&format!("{} ", h)))?;
    let rest = norm[head.len()..].trim_start();
    rest.split_whitespace().next()
}

/// Execute every allowlisted machine-checkable validation command found in
/// `description`, running each in `cwd`. Commands that are not allowlisted are
/// recorded as skipped, never executed.
pub fn execute_validation_commands(description: &str, cwd: &Path) -> ValidationOutcome {
    execute_validation_commands_with_timeout(description, cwd, validation_timeout_secs())
}

/// Like [`execute_validation_commands`], but with an explicit per-command
/// timeout (seconds). Exposed so callers (and tests) can pin a deadline instead
/// of relying on the `WG_VALIDATION_TIMEOUT_SECS` environment resolver.
pub fn execute_validation_commands_with_timeout(
    description: &str,
    cwd: &Path,
    timeout_secs: u64,
) -> ValidationOutcome {
    let mut outcome = ValidationOutcome {
        timeout_secs,
        ..Default::default()
    };
    for cmd in extract_validation_commands(description) {
        match is_executable(&cmd) {
            Err(reason) => {
                outcome.results.push(CommandResult {
                    command: cmd,
                    ran: false,
                    skip_reason: Some(reason),
                    exit_code: None,
                    passed: false,
                    timed_out: false,
                });
            }
            Ok(()) => {
                let run = run_sandboxed(&cmd, cwd, timeout_secs);
                outcome.results.push(CommandResult {
                    command: cmd,
                    ran: true,
                    skip_reason: None,
                    exit_code: run.exit_code,
                    passed: run.passed,
                    timed_out: run.timed_out,
                });
            }
        }
    }
    outcome
}

/// Outcome of actually executing one vetted command line.
struct RunResult {
    exit_code: Option<i32>,
    passed: bool,
    timed_out: bool,
    /// Process-group ids (Unix) / root child pids (Windows) that were spawned
    /// for this line. Exposed so tests can prove that, after a timeout, the
    /// whole spawned tree has been reaped (no lingering pid). Only read by the
    /// descendant-cleanup regression, so it is dead outside `cfg(test)`.
    #[cfg_attr(not(test), allow(dead_code))]
    group_ids: Vec<i32>,
}

impl RunResult {
    /// A line we could not spawn / vet: not a pass, not a timeout, no group.
    fn not_run() -> Self {
        RunResult {
            exit_code: None,
            passed: false,
            timed_out: false,
            group_ids: Vec::new(),
        }
    }
}

/// Execute one already-extracted validation command line in `cwd`, under a
/// `timeout_secs` deadline.
///
/// The line is re-validated here (defense in depth) and then run by SPAWNING
/// EACH command directly — never through `sh -c`, so no shell can re-parse a
/// rejected operator. Every spawned command runs in its OWN process group
/// (Unix `setsid`), so a hang is terminated by signalling the ENTIRE group
/// (leader plus any pipeline members / descendants), not just the immediate
/// child — the descendant-cleanup guarantee Erik required. Timeout completion
/// is explicit: a dedicated watchdog sets a flag when — and only when — it
/// actually kills the tree, so `timed_out` is deterministic rather than
/// inferred from elapsed time.
fn run_sandboxed(cmd: &str, cwd: &Path, timeout_secs: u64) -> RunResult {
    let stages = match parse_line(cmd) {
        Ok(s) => s,
        Err(_) => return RunResult::not_run(),
    };
    for stage in &stages {
        for argv in &stage.pipeline {
            if validate_argv(argv).is_err() {
                return RunResult::not_run();
            }
        }
    }
    run_stages(&stages, cwd, timeout_secs.max(1))
}

/// Run the parsed stages with short-circuit `&&` / `||` / `;` semantics under a
/// single wall-clock deadline shared across the whole line.
fn run_stages(stages: &[Stage], cwd: &Path, timeout_secs: u64) -> RunResult {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut last_ok = true;
    let mut last_code: Option<i32> = None;
    let mut group_ids: Vec<i32> = Vec::new();
    let mut any_ran = false;

    for stage in stages {
        let should_run = match stage.conn {
            Conn::First | Conn::Semi => true,
            Conn::And => last_ok,
            Conn::Or => !last_ok,
        };
        if !should_run {
            continue;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return RunResult {
                exit_code: Some(124),
                passed: false,
                timed_out: true,
                group_ids,
            };
        }
        let pr = run_pipeline(&stage.pipeline, cwd, remaining);
        group_ids.extend(pr.group_ids.iter().copied());
        any_ran = true;
        if pr.timed_out {
            return RunResult {
                exit_code: Some(124),
                passed: false,
                timed_out: true,
                group_ids,
            };
        }
        last_code = pr.exit_code;
        last_ok = pr.exit_code == Some(0);
    }

    if !any_ran {
        return RunResult::not_run();
    }
    RunResult {
        exit_code: last_code,
        passed: last_ok,
        timed_out: false,
        group_ids,
    }
}

/// Outcome of running one pipeline (`a | b | …`).
struct PipeResult {
    exit_code: Option<i32>,
    timed_out: bool,
    group_ids: Vec<i32>,
}

/// Spawn one pipeline directly (no shell), each command in its own process
/// group, wired stdout→stdin. A watchdog thread SIGKILLs every group at the
/// deadline; all direct children are reaped, and on timeout we wait for each
/// group to fully disappear so no descendant lingers.
fn run_pipeline(argvs: &[Vec<String>], cwd: &Path, budget: Duration) -> PipeResult {
    let n = argvs.len();
    let mut children: Vec<Child> = Vec::new();
    let mut group_ids: Vec<i32> = Vec::new();
    let mut prev_stdout: Option<std::process::ChildStdout> = None;

    for (idx, argv) in argvs.iter().enumerate() {
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]);
        cmd.current_dir(cwd);
        match prev_stdout.take() {
            Some(out) => {
                cmd.stdin(Stdio::from(out));
            }
            None => {
                cmd.stdin(Stdio::null());
            }
        }
        if idx + 1 < n {
            cmd.stdout(Stdio::piped());
        } else {
            // We only care about the exit status of a validation check, not its
            // output. Discarding stdout also keeps the evaluator log clean.
            cmd.stdout(Stdio::null());
        }
        cmd.stderr(Stdio::null());
        set_own_process_group(&mut cmd);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(_) => {
                // Kill/reap whatever already started, then report not-run.
                for g in &group_ids {
                    kill_group(*g);
                }
                for mut c in children {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                return PipeResult {
                    exit_code: None,
                    timed_out: false,
                    group_ids: Vec::new(),
                };
            }
        };
        prev_stdout = child.stdout.take();
        group_ids.push(child.id() as i32);
        children.push(child);
    }

    // Arm the watchdog. It fires exactly once, only if the pipeline is still
    // running at the deadline, and records that fact so timeout is deterministic.
    let done = Arc::new(AtomicBool::new(false));
    let fired = Arc::new(AtomicBool::new(false));
    let watch = {
        let done = Arc::clone(&done);
        let fired = Arc::clone(&fired);
        let groups = group_ids.clone();
        std::thread::spawn(move || {
            let step = Duration::from_millis(25);
            let mut waited = Duration::ZERO;
            while waited < budget {
                if done.load(Ordering::Acquire) {
                    return;
                }
                std::thread::sleep(step);
                waited += step;
            }
            if done.load(Ordering::Acquire) {
                return;
            }
            fired.store(true, Ordering::Release);
            for g in &groups {
                kill_group(*g);
            }
        })
    };

    // Reap every direct child (waiting on all avoids zombies). The pipeline's
    // exit status is the LAST command's.
    let mut last_code: Option<i32> = None;
    for (i, mut child) in children.into_iter().enumerate() {
        if let Ok(status) = child.wait() {
            if i + 1 == n {
                last_code = status.code();
            }
        }
    }
    done.store(true, Ordering::Release);
    let _ = watch.join();
    let timed_out = fired.load(Ordering::Acquire);

    if timed_out {
        // Grandchildren killed via SIGKILL reparent to init and are reaped
        // asynchronously; wait briefly to confirm each group is truly gone.
        for g in &group_ids {
            wait_group_gone(*g, Duration::from_secs(2));
        }
    }

    PipeResult {
        exit_code: if timed_out { Some(124) } else { last_code },
        timed_out,
        group_ids,
    }
}

/// Put a to-be-spawned command in its own process group so the whole subtree
/// can be signalled with a single group kill.
#[cfg(unix)]
fn set_own_process_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: `pre_exec` runs in the forked child before `execvp`. `setsid`
    // (and the `setpgid` fallback) only rearrange process-group membership and
    // report errors via errno; they allocate nothing and touch no shared state,
    // so they are safe to call here. See memory bg-job-pid-macos-setsid — macOS
    // ships no `setsid` binary, so we call `libc::setsid()` directly rather than
    // shelling out.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                // Already a group leader (rare) → make a new group in-session.
                let _ = libc::setpgid(0, 0);
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn set_own_process_group(_cmd: &mut Command) {
    // No cheap per-process job object here; `kill_group` uses `taskkill /T` to
    // walk and terminate the child's whole tree by pid instead.
}

/// Best-effort SIGKILL of an entire process group (Unix) / process tree
/// (Windows). Errors are ignored — the group may already be gone.
#[cfg(unix)]
fn kill_group(group_id: i32) {
    // A negative pid signals the whole process group (leader + descendants).
    // SAFETY: `kill(2)` validates its arguments and reports via errno.
    unsafe {
        libc::kill(-group_id as libc::pid_t, libc::SIGKILL);
    }
}

#[cfg(windows)]
fn kill_group(group_id: i32) {
    // /T terminates the whole tree rooted at the pid, /F forces.
    let _ = Command::new("taskkill")
        .args(["/F", "/T", "/PID", &group_id.to_string()])
        .output();
}

/// Poll until process group `group_id` has no surviving member, or `budget`
/// elapses. Returns `true` if the group is gone.
#[cfg(unix)]
fn wait_group_gone(group_id: i32, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    loop {
        if !process_group_alive(group_id) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(not(unix))]
fn wait_group_gone(_group_id: i32, _budget: Duration) -> bool {
    // `taskkill /T` above is synchronous, so the tree is already gone.
    true
}

/// Does process group `group_id` still have any member? `kill(-pgid, 0)` sends
/// no signal but performs the existence/permission check: `ESRCH` means the
/// group is empty (fully reaped).
#[cfg(unix)]
fn process_group_alive(group_id: i32) -> bool {
    let rc = unsafe { libc::kill(-group_id as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    // rc == -1: ESRCH ⇒ gone; anything else (e.g. EPERM) ⇒ still exists.
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// The trust inversion, expressed as a scoring rule: given the LLM's
/// narrative-based `narrative_score` and the executed-command `outcome`, return
/// the effective score. A failed artifact check caps the score at `0.0` (below
/// any sane eval-gate threshold) so a confident write-up over an absent
/// deliverable is rejected. If nothing failed, the narrative score stands.
pub fn apply_validation_to_score(narrative_score: f64, outcome: &ValidationOutcome) -> f64 {
    if outcome.has_failure() {
        0.0
    } else {
        narrative_score
    }
}

/// Build a human-readable note prefix describing a validation failure, for
/// inclusion in the evaluation record's `notes`.
pub fn failure_note(outcome: &ValidationOutcome) -> Option<String> {
    if !outcome.has_failure() {
        return None;
    }
    let mut parts = vec!["VALIDATION FAILED (evaluator executed the task's `## Validation` commands; artifact absent):".to_string()];
    for f in outcome.failures() {
        if f.timed_out {
            parts.push(format!(
                "  `{}` timed out after {}s (killed; a hung validation check fails the task)",
                f.command, outcome.timeout_secs
            ));
        } else {
            parts.push(format!(
                "  `{}` exited {}",
                f.command,
                f.exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "non-zero".to_string())
            ));
        }
    }
    Some(parts.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp_dir() -> PathBuf {
        std::env::temp_dir()
    }

    /// THE headline test (failing-test-first for this task): a task whose agent
    /// wrote a confident, narrative-only "done" but never produced the
    /// artifact must FAIL when the evaluator executes its validation command.
    ///
    /// Demonstrates the trust inversion directly: a high narrative score
    /// (0.95, "I completed everything") is forced to FAIL because the
    /// `test -f <absent artifact>` command exits non-zero.
    #[test]
    fn test_evaluator_fails_task_with_absent_artifact() {
        // A unique path guaranteed not to exist — the "phantom" deliverable.
        let absent = "docs/__wg_phantom_deliverable_that_does_not_exist_9f3a.md";
        let description = format!(
            "## Description\n\
             Write {absent} covering the trust layer.\n\n\
             ## Validation\n\
             - [ ] test -f {absent}\n\
             - [ ] grep -q \"trust\" {absent}\n"
        );

        // The agent's narrative said "done" with high confidence.
        let narrative_score = 0.95;

        let outcome = execute_validation_commands(&description, &tmp_dir());

        // At least one allowlisted command actually ran...
        assert!(
            outcome.any_ran(),
            "expected the test/grep commands to be executed, got: {:?}",
            outcome.results
        );
        // ...and it failed because the artifact is absent.
        assert!(
            outcome.has_failure(),
            "expected a validation failure for the absent artifact, got: {}",
            outcome.summary()
        );

        // The trust inversion: the confident narrative score is overridden to a
        // failing score.
        let effective = apply_validation_to_score(narrative_score, &outcome);
        assert_eq!(
            effective, 0.0,
            "a narrative-only 'done' over an absent artifact must score FAIL"
        );

        // And a failure note is produced for the evaluation record.
        assert!(
            failure_note(&outcome)
                .unwrap()
                .contains("VALIDATION FAILED")
        );
    }

    /// The positive control: when the artifact IS present, the command passes
    /// and the narrative score is preserved (the evaluator still scores prose
    /// quality — execution only vetoes phantoms, it doesn't replace scoring).
    #[test]
    fn test_present_artifact_preserves_narrative_score() {
        let dir = std::env::temp_dir().join("wg_eval_exec_present_test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("present.md");
        std::fs::write(&file, "this file mentions trust\n").unwrap();

        let description =
            "## Validation\n- [ ] test -f present.md\n- [ ] grep -q \"trust\" present.md\n";
        let outcome = execute_validation_commands(description, &dir);

        assert!(outcome.any_ran());
        assert!(!outcome.has_failure(), "{}", outcome.summary());
        assert_eq!(apply_validation_to_score(0.8, &outcome), 0.8);

        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn test_extract_from_validation_section() {
        let desc = "## Description\n\
                    Do the thing. Also test the endpoint by hand.\n\n\
                    ## Validation\n\
                    - [ ] git ls-remote --exit-code origin refs/heads/fix/x\n\
                    - [ ] gh pr view fix/x --repo graphwork/wg --json state\n\
                    - [ ] cargo build + cargo test pass with no regressions\n\
                    - [ ] Branch pushed to origin\n";
        let cmds = extract_validation_commands(desc);
        // The two allowlisted commands are kept; cargo/prose lines dropped, and
        // the "test the endpoint" prose in the Description is out of section.
        assert_eq!(cmds.len(), 2, "got: {:?}", cmds);
        assert!(cmds[0].starts_with("git ls-remote"));
        assert!(cmds[1].starts_with("gh pr view"));
    }

    #[test]
    fn test_allowlist_rejects_dangerous_lines() {
        // Non-allowlisted command.
        assert!(is_executable("rm -rf /").is_err());
        assert!(is_executable("cargo test").is_err());
        // Redirection / backtick.
        assert!(is_executable("test -f x > /etc/passwd").is_err());
        assert!(is_executable("grep -q x `whoami`").is_err());
        // Command substitution.
        assert!(is_executable("test \"$(rm -rf /)\" = \"\"").is_err());
        // A pipeline where the second segment is NOT allowlisted.
        assert!(is_executable("git ls-remote origin | sh").is_err());
        assert!(is_executable("grep -q x file | tee out").is_err());
        // BLOCKER 1(a): a lone `&` is a background-execution escape.
        assert!(is_executable("test -f x & touch proof").is_err());
        assert!(is_executable("test -f x & rm -rf /").is_err());
        // BLOCKER 1(b): git ls-remote exec/protocol-escape options are refused
        // in every spelling.
        assert!(is_executable("git ls-remote --upload-pack=\"touch proof\" .").is_err());
        assert!(is_executable("git ls-remote --upload-pack touch .").is_err());
        assert!(is_executable("git ls-remote -u touch .").is_err());
        assert!(is_executable("git ls-remote -o anything origin").is_err());
        assert!(is_executable("git ls-remote --server-option=x origin").is_err());
        // An unknown git ls-remote option is not on the read-only allowlist.
        assert!(is_executable("git ls-remote --frobnicate origin").is_err());
        // A `git` subcommand other than ls-remote is refused.
        assert!(is_executable("git push origin main").is_err());
        // Allowlisted single and chained commands are accepted.
        assert!(is_executable("git ls-remote --exit-code origin refs/heads/x").is_ok());
        assert!(is_executable("git ls-remote --heads --tags origin").is_ok());
        assert!(is_executable("test -f a && grep -q \"z\" a").is_ok());
        assert!(is_executable("gh pr view 42 --repo o/r --json state").is_ok());
        assert!(is_executable("test -f a || test -f b").is_ok());
    }

    #[test]
    fn test_prose_starting_with_command_word_is_not_run() {
        // "test the endpoint" and "grep the logs" read as prose, not commands.
        assert!(clean_command_line("- [ ] test the endpoint manually").is_none());
        assert!(clean_command_line("- [ ] grep the logs to confirm").is_none());
        // But real invocations survive.
        assert_eq!(
            clean_command_line("- [ ] test -f docs/x.md").as_deref(),
            Some("test -f docs/x.md")
        );
    }

    /// Create a writer-less FIFO in the temp dir. An allowlisted `grep` reading
    /// it blocks forever (no writer ever `open`s it for write), giving a
    /// DETERMINISTIC hang driven by a genuinely allowlisted command — unlike the
    /// old tests, which drove `run_sandboxed("sleep 30")` (a head the public
    /// allowlist rejects) and then synthesized the outcome.
    #[cfg(unix)]
    fn make_fifo(tag: &str) -> PathBuf {
        use std::ffi::CString;
        let path =
            std::env::temp_dir().join(format!("wg_eval_fifo_{}_{}", std::process::id(), tag));
        let _ = std::fs::remove_file(&path);
        let c = CString::new(path.to_str().unwrap()).unwrap();
        let rc = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
        assert_eq!(rc, 0, "mkfifo failed for {:?}", path);
        path
    }

    /// BLOCKER 2, public path: a GENUINELY hanging ALLOWLISTED check (a real
    /// `grep` blocked on a writer-less FIFO), driven all the way through the
    /// public `execute_validation_commands_with_timeout`, must be killed at the
    /// deadline and reported as a FAILURE — a hung check can never stall the
    /// evaluator or pass silently.
    #[cfg(unix)]
    #[test]
    fn test_hanging_allowlisted_check_times_out_public_path() {
        let fifo = make_fifo("public_hang");
        let desc = format!("## Validation\n- [ ] grep -q needle {}\n", fifo.display());

        let start = Instant::now();
        let outcome = execute_validation_commands_with_timeout(&desc, &tmp_dir(), 1);
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_secs(15),
            "the 1s deadline was not enforced (took {elapsed:?})"
        );
        assert!(
            outcome.any_ran(),
            "the grep must have actually run: {:?}",
            outcome.results
        );
        assert!(
            outcome.any_timed_out(),
            "a hung allowlisted check must be reported as a timeout: {}",
            outcome.summary()
        );
        assert!(
            outcome.has_failure(),
            "a timed-out check must fail the task: {}",
            outcome.summary()
        );
        // The trust inversion: a timed-out check caps the score at FAIL.
        assert_eq!(apply_validation_to_score(0.95, &outcome), 0.0);
        let note = failure_note(&outcome).expect("timeout should produce a failure note");
        assert!(note.contains("timed out"), "note should name it: {note}");

        let _ = std::fs::remove_file(&fifo);
    }

    /// BLOCKER 2, descendant cleanup: a hanging PIPELINE of two allowlisted
    /// `grep`s (the first blocked on a writer-less FIFO, the second blocked
    /// reading the never-written pipe). Under `sh -c` these would be
    /// grandchildren of the runner and a naive single-pid SIGKILL would orphan
    /// them; here each runs in its own process group and, after the timeout,
    /// EVERY group must be gone — no lingering pid.
    #[cfg(unix)]
    #[test]
    fn test_timeout_reaps_all_descendants() {
        let fifo = make_fifo("reap");
        let cmd = format!("grep -q x {} | grep -q y", fifo.display());
        // Sanity: this pipeline is genuinely allowlisted (the public gate would
        // run it), so the hang is exercised on the real execution path.
        assert!(
            is_executable(&cmd).is_ok(),
            "the hanging pipeline must be allowlisted"
        );

        let run = run_sandboxed(&cmd, &tmp_dir(), 1);
        assert!(
            run.timed_out,
            "the hang must trip the deadline (exit={:?})",
            run.exit_code
        );
        assert!(!run.passed, "a timed-out pipeline must not pass");
        assert_eq!(
            run.group_ids.len(),
            2,
            "the two-stage pipeline should spawn two process groups"
        );
        for g in &run.group_ids {
            assert!(
                !process_group_alive(*g),
                "process group {g} still has a surviving member after timeout — a descendant leaked"
            );
        }

        let _ = std::fs::remove_file(&fifo);
    }

    /// BLOCKER 1(a), no side effect: the lone-`&` background escape
    /// (`test -f x & touch <proof>`) must be REFUSED — recorded as skipped,
    /// never executed — and the injected `touch` must NOT create its proof file.
    #[test]
    fn test_lone_ampersand_escape_refused_no_side_effect() {
        let dir = std::env::temp_dir().join(format!("wg_eval_amp_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let proof = dir.join("amp_proof");
        let _ = std::fs::remove_file(&proof);

        let desc = format!(
            "## Validation\n- [ ] test -f x & touch {}\n",
            proof.display()
        );
        let outcome = execute_validation_commands(&desc, &dir);

        assert_eq!(outcome.results.len(), 1, "line should be considered once");
        assert!(
            outcome.results.iter().all(|r| !r.ran),
            "the escape must be refused, never run: {:?}",
            outcome.results
        );
        assert!(
            outcome.results[0].skip_reason.is_some(),
            "the refusal reason must be recorded for visibility"
        );
        assert!(!outcome.has_failure(), "a refused line never fails a task");
        assert!(
            !proof.exists(),
            "SIDE EFFECT: the injected `touch` executed — the RCE is NOT closed"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// BLOCKER 1(b), no side effect: the `git ls-remote --upload-pack=<program>`
    /// exec escape must be REFUSED and its payload must NOT run.
    #[test]
    fn test_ls_remote_upload_pack_escape_refused_no_side_effect() {
        let dir = std::env::temp_dir().join(format!("wg_eval_up_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let proof = dir.join("up_proof");
        let _ = std::fs::remove_file(&proof);

        let desc = format!(
            "## Validation\n- [ ] git ls-remote --upload-pack=\"touch {}\" .\n",
            proof.display()
        );
        let outcome = execute_validation_commands(&desc, &dir);

        assert_eq!(outcome.results.len(), 1, "line should be considered once");
        assert!(
            outcome.results.iter().all(|r| !r.ran),
            "the escape must be refused, never run: {:?}",
            outcome.results
        );
        assert!(
            outcome.results[0].skip_reason.is_some(),
            "the refusal reason must be recorded for visibility"
        );
        assert!(
            !proof.exists(),
            "SIDE EFFECT: --upload-pack executed the payload — the RCE is NOT closed"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// BLOCKER 1(c), no side effect: the `ext::` remote-helper transport escape
    /// (`git ls-remote 'ext::sh -c <payload>' .`, which Erik named explicitly)
    /// must be REFUSED and its payload must NOT run. Unlike `--upload-pack` the
    /// program rides in a POSITIONAL repository argument, so this proves the
    /// transport-escape guard covers positionals, not just options.
    #[test]
    fn test_ls_remote_ext_transport_escape_refused_no_side_effect() {
        let dir = std::env::temp_dir().join(format!("wg_eval_ext_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let proof = dir.join("ext_proof");
        let _ = std::fs::remove_file(&proof);

        // The whole `ext::sh -c 'touch <proof>'` payload is one quoted repo arg,
        // exactly the shape a task description could carry.
        let desc = format!(
            "## Validation\n- [ ] git ls-remote \"ext::sh -c 'touch {}'\" .\n",
            proof.display()
        );
        let outcome = execute_validation_commands(&desc, &dir);

        assert_eq!(outcome.results.len(), 1, "line should be considered once");
        assert!(
            outcome.results.iter().all(|r| !r.ran),
            "the ext:: transport escape must be refused, never run: {:?}",
            outcome.results
        );
        assert!(
            outcome.results[0]
                .skip_reason
                .as_deref()
                .is_some_and(|r| r.contains("remote-helper transport")),
            "the refusal reason must name the transport escape: {:?}",
            outcome.results[0].skip_reason
        );
        assert!(!outcome.has_failure(), "a refused line never fails a task");
        assert!(
            !proof.exists(),
            "SIDE EFFECT: the ext:: transport executed the payload — the escape is NOT closed"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Unit-level guard: `validate_git_ls_remote` refuses `ext::`/`fd::` helper
    /// transports but still accepts ordinary remotes (including an IPv6 URL,
    /// whose `::` must not be mistaken for a helper scheme).
    #[test]
    fn test_ls_remote_transport_policy() {
        let refuse = |s: &str| is_executable(&format!("git ls-remote {s} .")).is_err();
        let accept = |s: &str| is_executable(&format!("git ls-remote --exit-code {s}")).is_ok();

        // Remote-helper transports that can execute a program → refused.
        assert!(refuse("ext::sh"), "ext:: must be refused");
        assert!(refuse("fd::7"), "fd:: must be refused");
        // Ordinary read-only remotes → accepted (no `scheme::` helper form).
        assert!(accept("origin refs/heads/x"));
        assert!(accept("https://example.com/r.git"));
        assert!(accept("git@github.com:o/r.git"));
        assert!(
            accept("ssh://[::1]/r.git"),
            "IPv6 host `::` is not a helper transport"
        );
    }

    /// A fast allowlisted command well under the deadline is NOT flagged as a
    /// timeout — the deadline only fires on genuine overruns.
    #[test]
    fn test_fast_command_under_timeout_is_not_flagged() {
        let dir = std::env::temp_dir().join("wg_eval_exec_fast_test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("present.md");
        std::fs::write(&file, "trust\n").unwrap();

        let desc = "## Validation\n- [ ] test -f present.md\n";
        // Generous 60s deadline; the check finishes in milliseconds.
        let outcome = execute_validation_commands_with_timeout(desc, &dir, 60);
        assert!(outcome.any_ran());
        assert!(!outcome.any_timed_out(), "{}", outcome.summary());
        assert!(!outcome.has_failure(), "{}", outcome.summary());
        std::fs::remove_file(&file).ok();
    }

    /// The env-var override resolves a positive value and falls back to the
    /// default on blank / zero / garbage, so an operator can never accidentally
    /// disable the safety net with a bad value.
    #[test]
    fn test_validation_timeout_env_resolution() {
        // Snapshot & restore so we don't leak state into sibling tests.
        let saved = std::env::var(VALIDATION_TIMEOUT_ENV).ok();

        unsafe { std::env::set_var(VALIDATION_TIMEOUT_ENV, "5") };
        assert_eq!(validation_timeout_secs(), 5);
        unsafe { std::env::set_var(VALIDATION_TIMEOUT_ENV, "0") };
        assert_eq!(validation_timeout_secs(), DEFAULT_VALIDATION_TIMEOUT_SECS);
        unsafe { std::env::set_var(VALIDATION_TIMEOUT_ENV, "not-a-number") };
        assert_eq!(validation_timeout_secs(), DEFAULT_VALIDATION_TIMEOUT_SECS);
        unsafe { std::env::remove_var(VALIDATION_TIMEOUT_ENV) };
        assert_eq!(validation_timeout_secs(), DEFAULT_VALIDATION_TIMEOUT_SECS);

        match saved {
            Some(v) => unsafe { std::env::set_var(VALIDATION_TIMEOUT_ENV, v) },
            None => unsafe { std::env::remove_var(VALIDATION_TIMEOUT_ENV) },
        }
    }

    #[test]
    fn test_no_validation_commands_is_not_a_failure() {
        // A task with no machine-checkable commands must not be force-failed;
        // it falls back to the narrative score (status quo).
        let desc = "## Validation\n- [ ] Looks good to me\n- [ ] cargo test passes\n";
        let outcome = execute_validation_commands(desc, &tmp_dir());
        assert!(!outcome.any_ran());
        assert!(!outcome.has_failure());
        assert_eq!(apply_validation_to_score(0.7, &outcome), 0.7);
    }
}
