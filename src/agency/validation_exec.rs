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
//! * **Segments are vetted individually.** A line is split on `&&`, `||`, `|`,
//!   and `;`; each segment's head must be allowlisted. One non-allowlisted
//!   segment disqualifies the whole line.
//! * **No filesystem-mutating shell metacharacters.** Any line containing a
//!   redirection (`>`, `<`, `>>`) or a backtick is skipped outright — the
//!   allowlisted commands never need them, and they are the classic write /
//!   command-substitution escape hatches.
//! * **Command substitution (`$(...)`) is skipped.** We only execute lines we
//!   can fully vet; `$(...)` can smuggle a non-allowlisted command past a
//!   segment-head check, so any line containing it is not run.
//! * **Skip is safe; only run-and-fail fails a task.** A line we refuse to run
//!   never forces a failure — it simply is not machine-verified. A task is
//!   forced to FAIL only when an allowlisted command actually *ran* and exited
//!   non-zero. This bias means the evaluator can never fail a task because it
//!   declined to execute something.

use std::path::Path;
use std::process::Command;

/// Command heads the evaluator is permitted to execute. These are all
/// read-only existence checks — none of them mutate the repo or the remote.
/// The two-word entries (`git ls-remote`, `gh pr view`, `gh pr list`) pin the
/// *subcommand* so a bare `git` / `gh` (which could push, delete, merge…) is
/// never allowed.
pub const ALLOWLIST: &[&str] = &["git ls-remote", "gh pr view", "gh pr list", "test", "grep"];

/// Characters that, if present anywhere in a candidate line, disqualify it from
/// execution. Redirections can write files; backticks are command substitution.
const FORBIDDEN_CHARS: &[char] = &['>', '<', '`'];

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
}

/// The outcome of executing all machine-checkable validation commands in a
/// task's `## Validation` block.
#[derive(Debug, Clone, Default)]
pub struct ValidationOutcome {
    /// Per-command results, in the order they appeared.
    pub results: Vec<CommandResult>,
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

    /// A one-line human-readable summary of what ran and what failed.
    pub fn summary(&self) -> String {
        let ran = self.results.iter().filter(|r| r.ran).count();
        let failed = self.failures().len();
        let skipped = self.results.len() - ran;
        format!(
            "validation commands: {} ran, {} failed, {} skipped (not allowlisted)",
            ran, failed, skipped
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

/// Is this whole command line safe to execute under the sandbox rules? Every
/// pipeline/`&&`/`;` segment head must be allowlisted, there must be no
/// forbidden character, and no `$(...)` command substitution.
pub fn is_executable(cmd: &str) -> Result<(), String> {
    if cmd.chars().any(|c| FORBIDDEN_CHARS.contains(&c)) {
        return Err("contains a redirection or backtick".to_string());
    }
    if cmd.contains("$(") {
        return Err("contains command substitution $(...)".to_string());
    }
    // Split on shell control operators and vet each segment head.
    for segment in split_segments(cmd) {
        let seg = segment.trim();
        if seg.is_empty() {
            continue;
        }
        if !passes_segment_signature(seg) {
            return Err(format!("segment not allowlisted: `{}`", seg));
        }
    }
    Ok(())
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
    let mut outcome = ValidationOutcome::default();
    for cmd in extract_validation_commands(description) {
        match is_executable(&cmd) {
            Err(reason) => {
                outcome.results.push(CommandResult {
                    command: cmd,
                    ran: false,
                    skip_reason: Some(reason),
                    exit_code: None,
                    passed: false,
                });
            }
            Ok(()) => {
                let (exit_code, passed) = run_sandboxed(&cmd, cwd);
                outcome.results.push(CommandResult {
                    command: cmd,
                    ran: true,
                    skip_reason: None,
                    exit_code,
                    passed,
                });
            }
        }
    }
    outcome
}

/// Run one vetted command via `sh -c` in `cwd`. Returns `(exit_code, passed)`.
/// The line has already been proven to contain only allowlisted commands and no
/// forbidden metacharacters, so `sh -c` only ever sees read-only checks.
fn run_sandboxed(cmd: &str, cwd: &Path) -> (Option<i32>, bool) {
    let output = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .output();
    match output {
        Ok(out) => {
            let code = out.status.code();
            (code, out.status.success())
        }
        // If we cannot even spawn the check, treat it as not-run rather than a
        // failure so infrastructure gaps don't masquerade as phantom-catches.
        Err(_) => (None, false),
    }
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
        parts.push(format!(
            "  `{}` exited {}",
            f.command,
            f.exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "non-zero".to_string())
        ));
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
        // Allowlisted single and chained commands are accepted.
        assert!(is_executable("git ls-remote --exit-code origin refs/heads/x").is_ok());
        assert!(is_executable("test -f a && grep -q \"z\" a").is_ok());
        assert!(is_executable("gh pr view 42 --repo o/r --json state").is_ok());
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
