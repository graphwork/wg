#!/usr/bin/env bash
# Scenario: handler_first_bare_provider_model
#
# Regression for the 14h-401 incident (docs/design-handler-first-model-spec.md):
# a coordinator launched
#     wg service start --model openrouter:z-ai/glm-5.2
# silently routed to the keyless in-process `native` handler, so every
# non-pinned task 401'd invisibly for ~14 hours. Handler-first enforcement
# turns that silent mis-route into a LOUD warning at every strict entry point:
#   - the daemon-launch `--model` arg (the exact path that bit us),
#   - CLI `wg config --model`,
#   - `wg config lint`,
#   - `wg migrate config`.
# The canonical handler-first form (`nex:openrouter:...`) must stay SILENT at
# the same paths, and `wg config --models` must render the canonical route +
# the resolved `handler=native` so a mis-route is visible at a glance.

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

scratch=$(make_scratch)
fake_home="$scratch/home"
mkdir -p "$fake_home/.config"

run_wg() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID -u WG_DIR \
        HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" \
        wg "$@"
}

project="$scratch/project"
mkdir -p "$project"
cd "$project"

run_wg init --no-agency >"$scratch/init.log" 2>&1 ||
    loud_fail "wg init --no-agency failed: $(tail -20 "$scratch/init.log")"
wg_dir=$(graph_dir_in "$project") || loud_fail "no .wg dir after wg init"

BARE="openrouter:z-ai/glm-5.2"
CANON="nex:openrouter:z-ai/glm-5.2"
ALT="pi:openrouter:z-ai/glm-5.2"

# --- 1. THE INCIDENT PATH: daemon-launch --model bare provider WARNs loudly ---
# run_start prints the handler-first warning synchronously, before forking the
# daemon, so the warning is captured regardless of whether the daemon (with a
# keyless model) actually comes up. Stop any daemon that did.
start_log="$scratch/start_bare.log"
run_wg --dir "$wg_dir" service start --model "$BARE" --force >"$start_log" 2>&1 || true
run_wg --dir "$wg_dir" service stop --force >/dev/null 2>&1 || true
for needle in "not a handler" "$CANON" "$ALT"; do
    grep -qF "$needle" "$start_log" ||
        loud_fail "daemon-launch '--model $BARE' did not warn with '$needle'. Output:
$(cat "$start_log")"
done

# --- 2. The canonical handler-first form is SILENT at the same path ---
start2_log="$scratch/start_canon.log"
run_wg --dir "$wg_dir" service start --model "$CANON" --force >"$start2_log" 2>&1 || true
run_wg --dir "$wg_dir" service stop --force >/dev/null 2>&1 || true
if grep -qF "not a handler" "$start2_log"; then
    loud_fail "canonical handler-first '--model $CANON' wrongly warned. Output:
$(cat "$start2_log")"
fi

# --- 3. CLI `wg config --model` bare provider warns + names nex:/pi: ---
cfg_log="$scratch/config_set.log"
run_wg --dir "$wg_dir" config --local \
    --dispatcher-model "$BARE" \
    --set-model default "$BARE" \
    --no-reload >"$cfg_log" 2>&1 ||
    loud_fail "wg config set failed:
$(cat "$cfg_log")"
grep -qF "not a handler" "$cfg_log" ||
    loud_fail "wg config --dispatcher-model $BARE did not warn. Output:
$(cat "$cfg_log")"
grep -qF "$CANON" "$cfg_log" ||
    loud_fail "config-set warning did not name the canonical $CANON. Output:
$(cat "$cfg_log")"

# --- 4. `wg config lint` flags the bare provider (now in config) ---
lint_log="$scratch/lint.log"
run_wg --dir "$wg_dir" config lint >"$lint_log" 2>&1 || true
grep -qF "$BARE" "$lint_log" ||
    loud_fail "wg config lint did not flag $BARE. Output:
$(cat "$lint_log")"
grep -qF "$CANON" "$lint_log" ||
    loud_fail "wg config lint did not name the rewrite target $CANON. Output:
$(cat "$lint_log")"

# --- 5. `wg migrate config --dry-run` shows the handler-first rewrite ---
mig_log="$scratch/migrate.log"
run_wg --dir "$wg_dir" migrate config --dry-run >"$mig_log" 2>&1 || true
grep -qF "$CANON" "$mig_log" ||
    loud_fail "wg migrate config --dry-run did not show rewrite to $CANON. Output:
$(cat "$mig_log")"

# --- 6. `wg config --models` renders the canonical route + handler=native ---
models_log="$scratch/models.log"
run_wg --dir "$wg_dir" config --models >"$models_log" 2>/dev/null ||
    loud_fail "wg config --models failed"
grep -qF "$CANON" "$models_log" ||
    loud_fail "wg config --models did not render the canonical $CANON. Output:
$(cat "$models_log")"
grep -Eq "${CANON//./\\.}[[:space:]]+native" "$models_log" ||
    loud_fail "wg config --models did not echo handler=native for the bare-provider route. Output:
$(cat "$models_log")"

echo "PASS: bare provider model spec ($BARE) warns loudly at the daemon-launch path, CLI config, lint, and migrate; the canonical handler-first form ($CANON) is silent and renders with handler=native"
exit 0
