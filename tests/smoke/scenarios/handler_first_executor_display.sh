#!/usr/bin/env bash
# Scenario: handler_first_executor_display
#
# Pins the `bug-handler-first-executor-display-spam` fix: a migrated-clean
# config with `model = "pi:openrouter:..."` and NO legacy `executor` key must
# surface the effective executor/handler as `pi` in every user-facing status
# surface (`wg config --show`, `wg status`, `wg config --models`), and must
# NOT emit any deprecated-executor-key warnings (no TUI/status spam after
# migration).
#
# Before the fix, `provider_to_executor("pi")` returned the legacy `native`
# default and `CoordinatorConfig::effective_executor` fell through to `claude`
# (because `parse_model_spec` deliberately does not recognize external-CLI
# handler prefixes like `pi`), so `wg service reload` / `wg status` printed
# `executor=claude, model=pi:...` even though `handler=pi` — tempting users
# to restore the deprecated `executor` key to "fix" the label, which then
# spammed the TUI with deprecation warnings on every config load.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
fake_home="$scratch/home"
mkdir -p "$fake_home/.config"

# Isolate HOME + XDG so the host's global ~/.wg/config.toml cannot leak a
# stale `executor` key or a different default model into the merged config.
run_wg() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" \
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

# Write a migrated-clean config: handler-first `pi:` model, NO deprecated
# `executor` key anywhere. This is the post-migration state the bug report
# describes (`wg config lint --local` clean, `wg config --models` → pi).
cat >"$cfg" <<'TOML'
[agent]
model = "pi:openrouter:anthropic/claude-opus-4-7"

[dispatcher]
max_agents = 2
model = "pi:openrouter:anthropic/claude-opus-4-7"
TOML

# --- 1. `wg config lint --local` must be clean (no deprecated keys). ---
lint_out=$(run_wg config lint --local 2>"$scratch/lint.err") || \
    loud_fail "wg config lint --local failed: $lint_out / $(cat "$scratch/lint.err")"
lint_err=$(cat "$scratch/lint.err")
if grep -qi "deprecated" <<<"$lint_err"; then
    loud_fail "wg config lint emitted deprecated-key spam on a migrated-clean config:\n$lint_err"
fi
if ! grep -q "clean — no stale keys found" <<<"$lint_out"; then
    loud_fail "wg config lint --local did not report clean:\n$lint_out"
fi

# --- 2. `wg config --show` must display executor = "pi" (handler-first). ---
show_out=$(run_wg config --show 2>"$scratch/show.err") || \
    loud_fail "wg config --show failed: $(cat "$scratch/show.err")"
show_err=$(cat "$scratch/show.err")
if grep -qi "deprecated" <<<"$show_err"; then
    loud_fail "wg config --show emitted deprecated-key spam on a migrated-clean config:\n$show_err"
fi
# The [dispatcher] executor line must show the pi handler, not the legacy
# claude/native default.
disp_exec=$(grep -E '^\s*executor\s*=' <<<"$show_out" | sed -E 's/.*executor\s*=\s*"([^"]*)".*/\1/' | tail -1)
if [[ -z "$disp_exec" ]]; then
    loud_fail "wg config --show did not print an executor line:\n$show_out"
fi
if [[ "$disp_exec" != "pi" ]]; then
    loud_fail "wg config --show displayed executor=\"$disp_exec\" for a pi: model; expected \"pi\" (handler-first). Output:\n$show_out"
fi

# --- 3. `wg status` (no daemon → config fallback) must display executor=pi. ---
status_out=$(run_wg status 2>"$scratch/status.err") || \
    loud_fail "wg status failed: $(cat "$scratch/status.err")"
status_err=$(cat "$scratch/status.err")
if grep -qi "deprecated" <<<"$status_err"; then
    loud_fail "wg status emitted deprecated-key spam on a migrated-clean config:\n$status_err"
fi
# The Dispatcher line looks like: "Dispatcher: max=..., executor=pi, model=..., poll=...s"
if ! grep -q "executor=pi" <<<"$status_out"; then
    loud_fail "wg status did not show executor=pi for a pi: model:\n$status_out"
fi
# And it must NOT label the legacy default as the active executor.
if grep -q "executor=claude" <<<"$status_out"; then
    loud_fail "wg status labeled the deprecated legacy executor=claude as active for a pi: model:\n$status_out"
fi

# --- 4. `wg config --models` must resolve the HANDLER column to pi. ---
models_out=$(run_wg config --models 2>"$scratch/models.err") || \
    loud_fail "wg config --models failed: $(cat "$scratch/models.err")"
if ! grep -qE '\bpi\b' <<<"$models_out"; then
    loud_fail "wg config --models did not surface the pi handler:\n$models_out"
fi

echo "PASS: handler-first executor display shows pi for a migrated-clean pi: config with no deprecated spam."
exit 0
