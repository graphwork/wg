#!/usr/bin/env bash
# Scenario: config_local_sticks_under_profile
#
# Regression for make-wg-config: a `wg config set <key> <value> --local` write
# must AUTHORITY-stick for the current repo even when a project profile is
# active, and every dispatcher/registry knob must be reachable via `wg config`
# without hand-editing files. Concretely pins:
#
#   1. `wg config set/get <dotted.key>` works for arbitrary TOML paths
#      (coordinator.max_agents, coordinator.registry_refresh_interval,
#      agency.auto_evaluate), project-scoped, with daemon reload plumbing.
#   2. A non-routing knob (`dispatcher.max_agents`) set via `wg config set`
#      PERSISTS across a re-read of the merged config (what `wg service reload`
#      does) even with an active project profile whose template declares a
#      different default (pi => 8). Before the fix, the profile overlay
#      silently reset 2 -> 8 on every reload.
#   3. Routing keys (`agent.model`) stay profile-owned (source: project-profile)
#      so the profile still authority-owns routing.
#   4. Source annotations are accurate: the locally-overridden max_agents is
#      labeled `local`, not `project-profile`.
#   5. Validation rejects bad model specs / non-integer ints before writing.
#   6. No supported `wg config` path invalidates the profile fingerprint or
#      disables execution (the config write touches local config.toml only;
#      the profile association stays Ready).
#
# Credential-free: it never starts the daemon or calls an LLM. "Persists across
# reload" is proven by re-reading the merged config (exactly what the daemon's
# Reconfigure-with-no-flags IPC does).

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
mkdir -p "$HOME/.wg" "$XDG_CONFIG_HOME" "$scratch/proj/.wg"
wg_dir="$scratch/proj/.wg"

run_wg() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        HOME="$HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" \
        wg --dir "$wg_dir" "$@"
}

# Select the Pi project profile (its template pins dispatcher.max_agents = 8).
if ! run_wg profile select pi >/dev/null 2>"$scratch/select.err"; then
    loud_fail "wg profile select pi failed: $(cat "$scratch/select.err")"
fi

# 1+2. Default max_agents comes from the profile (8) BEFORE any local write.
default=$(run_wg config get dispatcher.max_agents 2>/dev/null | head -1)
if [[ "$default" != "dispatcher.max_agents = 8" ]]; then
    loud_fail "expected profile default max_agents = 8, got: $default"
fi

# Write max_agents = 2 locally (the value the profile was silently resetting).
if ! run_wg config set coordinator.max_agents 2 --no-reload >"$scratch/set.log" 2>&1; then
    loud_fail "wg config set coordinator.max_agents 2 failed: $(cat "$scratch/set.log")"
fi

# THE BUG FIX: the effective value must be 2 (local wins), not 8 (profile).
# A re-read of the merged config is exactly what `wg service reload` (no flags)
# does, so this proves the value PERSISTS across reload/restart.
effective=$(run_wg config get dispatcher.max_agents 2>/dev/null | head -1)
if [[ "$effective" != "dispatcher.max_agents = 2" ]]; then
    loud_fail "max_agents did not stick under the active profile (reload-override regression): expected 2, got: $effective"
fi

# 4. Source annotation: a locally-overridden non-routing knob is labeled local.
source_line=$(run_wg config get dispatcher.max_agents 2>/dev/null | grep -i 'source:' | head -1)
if ! grep -qi 'source: local' <<<"$source_line"; then
    loud_fail "max_agents source label should be 'local' after a local override, got: $source_line"
fi

# 3. Routing keys remain profile-owned (the profile still authority-owns routing).
agent_model=$(run_wg config get agent.model 2>/dev/null | head -1)
if [[ "$agent_model" != "agent.model = \"pi:openrouter:z-ai/glm-5.2\"" ]]; then
    loud_fail "agent.model should still come from the pi profile, got: $agent_model"
fi
agent_model_src=$(run_wg config get agent.model 2>/dev/null | grep -i 'source:' | head -1)
if ! grep -qi 'source: project-profile' <<<"$agent_model_src"; then
    loud_fail "agent.model source should be 'project-profile', got: $agent_model_src"
fi

# 1. Generic set/get for an arbitrary non-routing knob (registry disable = 0).
if ! run_wg config set coordinator.registry_refresh_interval 0 --no-reload >>"$scratch/set.log" 2>&1; then
    loud_fail "wg config set registry_refresh_interval failed: $(cat "$scratch/set.log")"
fi
rri=$(run_wg config get coordinator.registry_refresh_interval 2>/dev/null | head -1)
if [[ "$rri" != "dispatcher.registry_refresh_interval = 0" ]]; then
    loud_fail "registry_refresh_interval did not round-trip, got: $rri"
fi

# 1b. Generic set/get for an agency boolean (type inference: "false" -> bool).
if ! run_wg config set agency.auto_evaluate false --no-reload >>"$scratch/set.log" 2>&1; then
    loud_fail "wg config set agency.auto_evaluate failed: $(cat "$scratch/set.log")"
fi
ae=$(run_wg config get agency.auto_evaluate 2>/dev/null | head -1)
if [[ "$ae" != "agency.auto_evaluate = false" ]]; then
    loud_fail "agency.auto_evaluate did not round-trip as bool, got: $ae"
fi

# 5. Validation: a bad model spec is rejected and writes nothing.
if run_wg config set agent.model "not-a-valid-spec" --no-reload >"$scratch/bad.log" 2>&1; then
    loud_fail "wg config set accepted an invalid model spec (should have errored)"
fi
if ! grep -qi 'invalid model' "$scratch/bad.log"; then
    loud_fail "bad model spec error message missing guidance, got: $(cat "$scratch/bad.log")"
fi

# 5b. Validation: a non-integer for an integer field is rejected.
if run_wg config set coordinator.max_agents "abc" --no-reload >"$scratch/bad2.log" 2>&1; then
    loud_fail "wg config set accepted a non-integer for max_agents (should have errored)"
fi

# 6. The config write did NOT invalidate the profile fingerprint / disable exec.
#    `wg profile show` must still report a Ready association.
show_out=$(run_wg profile show 2>&1)
if echo "$show_out" | grep -qi 'changed after selection\|ContentDrift\|execution is disabled'; then
    loud_fail "config write invalidated the profile fingerprint / disabled execution:\n$show_out"
fi

# The local config file must use the canonical [dispatcher] section (no
# [coordinator] deprecation), so subsequent loads stay lint-clean.
if grep -q '^\[coordinator\]' "$wg_dir/config.toml"; then
    loud_fail "config set wrote deprecated [coordinator] section; expected [dispatcher]:\n$(cat "$wg_dir/config.toml")"
fi
if ! grep -q '^\[dispatcher\]' "$wg_dir/config.toml"; then
    loud_fail "config set did not write a [dispatcher] section:\n$(cat "$wg_dir/config.toml")"
fi

echo "PASS: wg config set/get sticks project-locally under an active profile (max_agents 8->2 persists), routing stays profile-owned, sources annotated, no fingerprint footgun."
exit 0
