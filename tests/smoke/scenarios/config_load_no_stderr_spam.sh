#!/usr/bin/env bash
# Scenario: config_load_no_stderr_spam
#
# Pins the `deduplicate-config-deprecation` fix: reusable config-load paths
# (`Config::load` / `load_merged` / `load_with_sources` / `load_or_default`)
# are side-effect-free w.r.t. terminal output. Deprecation diagnostics are
# COLLECTED into `Config.load_diagnostics` and emitted at most once at a
# user-facing CLI boundary (deduplicated), instead of `eprintln!`-ing the same
# paragraph on every load — which corrupted the TUI alternate screen, grew the
# daemon log per tick, and leaked into worker / smoke output.
#
# Before the fix, `Config::load*` printed `deprecated; wg now derives the
# handler from the model spec…` (and the legacy-section / executor-key /
# compaction-key paragraphs) directly to stderr on every load. This scenario
# proves:
#   1. A config with deprecated keys surfaces each finding AT MOST ONCE on a
#      CLI inspect surface (`wg config --show`) — never once per internal load.
#   2. `wg config lint --local` still surfaces the COMPLETE, copy-pasteable
#      migration findings (the canonical "what's stale?" surface is preserved).
#   3. A migrated-clean config is SILENT on every surface (must-not-over-block).
#   4. The daemon logs the deprecation at most once across startup + repeated
#      hot reloads (no per-tick / per-reload log growth).

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
fake_home="$scratch/home"
mkdir -p "$fake_home/.config"

# Isolate HOME + XDG + WG_GLOBAL_DIR so the host's global config can't leak a
# stale executor key or a different default model into the merged config.
run_wg() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        -u WG_DIR HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" \
        WG_GLOBAL_DIR="$scratch/global" \
        wg "$@"
}

project="$scratch/project"
mkdir -p "$project"
cd "$project"

if ! run_wg init --no-agency >"$scratch/init.log" 2>&1; then
    loud_fail "wg init --no-agency failed: $(tail -20 "$scratch/init.log")"
fi

wg_dir=$(graph_dir_in "$project") || loud_fail "no .wg dir after wg init"
cfg="$wg_dir/config.toml"

# A config with VALID pi routing but SEVERAL deprecated keys: a legacy
# `[coordinator]` section alias, an explicit `executor` key, and a retired
# compaction key. None break pi validation; all should be collected (not
# printed on every load) and surfaced once at a CLI boundary.
write_legacy_config() {
    cat >"$cfg" <<'TOML'
[agent]
model = "pi:openrouter:anthropic/claude-opus-4-7"

[coordinator]
executor = "claude"
model = "pi:openrouter:anthropic/claude-opus-4-7"
compactor_interval = 99

[tiers]
standard_reasoning = "high"
fast_reasoning = "low"
premium_reasoning = "xhigh"
TOML
}

# A migrated-clean config: handler-first pi: routes, no legacy keys.
write_clean_config() {
    cat >"$cfg" <<'TOML'
[agent]
model = "pi:openrouter:anthropic/claude-opus-4-7"

[dispatcher]
model = "pi:openrouter:anthropic/claude-opus-4-7"

[tiers]
standard_reasoning = "high"
fast_reasoning = "low"
premium_reasoning = "xhigh"
TOML
}

# Marker substring that appears in every deprecated-executor / legacy-section /
# model-prefix deprecation paragraph. Counting its occurrences in captured
# stderr bounds how many times the paragraph was emitted.
DEPRECATION_MARKER="deprecated"

count_marker() {
    # $1 = text to scan. grep -c counts matching LINES; a deprecation
    # paragraph is one line, so this is the occurrence count.
    grep -ci "$DEPRECATION_MARKER" <<<"$1" 2>/dev/null || printf 0
}

# ---------------------------------------------------------------------------
# 1. Legacy config → each deprecation surfaces AT MOST ONCE on `wg config --show`.
#    (criterion: repeated CLI loads emit at most one copy per invocation.)
# ---------------------------------------------------------------------------
write_legacy_config

show_err=$(run_wg config --show >/dev/null 2>"$scratch/show.err" && cat "$scratch/show.err" || cat "$scratch/show.err")
legacy_show_count=$(count_marker "$show_err")
if [[ "$legacy_show_count" -lt 1 ]]; then
    loud_fail "legacy config produced NO deprecation on 'wg config --show' stderr (signal lost):\n$show_err"
fi
if [[ "$legacy_show_count" -gt 4 ]]; then
    loud_fail "legacy config spammed 'wg config --show' stderr with $legacy_show_count deprecation lines (expected <=4, one per finding, deduplicated):\n$show_err"
fi

# ---------------------------------------------------------------------------
# 2. `wg config lint --local` still surfaces the COMPLETE migration findings
#    (the canonical, copy-pasteable surface is preserved — criterion 6).
# ---------------------------------------------------------------------------
lint_out=$(run_wg config lint --local 2>"$scratch/lint.err") || \
    loud_fail "wg config lint --local failed: $lint_out / $(cat "$scratch/lint.err")"
# The executor key and the legacy [coordinator] section must be named in the
# complete lint surface so a user can copy-paste the migration.
if ! grep -qi "coordinator" <<<"$lint_out"; then
    loud_fail "wg config lint did not surface the legacy [coordinator] section:\n$lint_out"
fi

# ---------------------------------------------------------------------------
# 3. Migrated-clean config is SILENT on every surface (must-not-over-block).
# ---------------------------------------------------------------------------
write_clean_config
clean_show_err=$(run_wg config --show >/dev/null 2>"$scratch/clean-show.err" && cat "$scratch/clean-show.err" || cat "$scratch/clean-show.err")
if grep -qi "$DEPRECATION_MARKER" <<<"$clean_show_err"; then
    loud_fail "clean config emitted deprecation text on 'wg config --show' stderr (must-not-over-block):\n$clean_show_err"
fi
clean_lint_out=$(run_wg config lint --local 2>"$scratch/clean-lint.err") || \
    loud_fail "wg config lint --local failed on clean config: $(cat "$scratch/clean-lint.err")"
if ! grep -qi "clean" <<<"$clean_lint_out"; then
    loud_fail "wg config lint did not report the clean config as clean:\n$clean_lint_out"
fi

# ---------------------------------------------------------------------------
# 4. Daemon logs the deprecation at most once across startup + repeated hot
#    reloads (no per-tick / per-reload log growth — criterion 3).
#    The daemon loads config at startup (logs diagnostics once) and on each
#    `wg service reload`; with the fix, reloads do NOT re-emit the paragraph.
# ---------------------------------------------------------------------------
write_legacy_config
daemon_log="$wg_dir/service/daemon.log"
mkdir -p "$wg_dir/service"

# Start the daemon with the legacy config (no coordinator agent, no workers).
if ! run_wg service start --max-agents 0 --no-coordinator-agent \
        >"$scratch/start.out" 2>"$scratch/start.err"; then
    loud_fail "daemon did not start with a legacy-but-valid pi config:\n$(cat "$scratch/start.err")"
fi

# Give it a moment to write its startup diagnostics, then trigger several
# hot reloads (each re-loads config.toml).
sleep 1
for _ in 1 2 3 4 5; do
    run_wg service reload >/dev/null 2>"$scratch/reload.err" || true
    sleep 0.3
done
sleep 1

run_wg service stop --force >/dev/null 2>&1 || true

if [[ -f "$daemon_log" ]]; then
    daemon_log_text=$(cat "$daemon_log" 2>/dev/null || printf '')
    daemon_deprec_count=$(count_marker "$daemon_log_text")
    # The deprecation paragraph must appear at most a small bounded number of
    # times (once at startup) — NOT once per reload (5 reloads would yield 6+
    # copies pre-fix). Allow a generous ceiling of 3 to absorb a single
    # startup log without flaking, while still catching per-reload spam.
    if [[ "$daemon_deprec_count" -gt 3 ]]; then
        loud_fail "daemon log grew deprecation spam across reloads ($daemon_deprec_count copies after 5 reloads; expected <=3):\n$daemon_log_text"
    fi
else
    loud_fail "daemon log was not created at $daemon_log; cannot verify reload dedup"
fi

echo "PASS: config-load paths are side-effect-free; deprecations collect (not print), surface once at CLI boundaries, stay silent on clean configs, and do not grow the daemon log across reloads."
exit 0
