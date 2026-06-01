#!/usr/bin/env bash
# Scenario: executor_arena_surfaces
#
# Pins the final executor-arena integration: the real CLI surfaces users check
# while configuring workers must all name the core, stable external,
# provider-specific, and experimental executor choices. This is intentionally
# credential-free.

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

cd "$scratch"

if ! run_wg init -m claude:opus --no-agency >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -20 init.log)"
fi

choices=(
    native
    claude
    codex
    shell
    opencode
    aider
    goose
    qwen
    cline
    gemini
    crush
    amplifier
)

assert_choices() {
    local label="$1"
    local file="$2"
    local choice
    for choice in "${choices[@]}"; do
        grep -qF "$choice" "$file" || \
            loud_fail "$label missing executor choice '$choice'. Output:\n$(cat "$file")"
    done
}

if ! run_wg executors --all >executors.out 2>&1; then
    loud_fail "wg executors --all failed: $(cat executors.out)"
fi
assert_choices "wg executors --all" executors.out

if ! run_wg config --show >config-show.out 2>&1; then
    loud_fail "wg config --show failed: $(cat config-show.out)"
fi
grep -qF "[executor choices]" config-show.out || \
    loud_fail "wg config --show missing [executor choices]. Output:\n$(cat config-show.out)"
grep -qF "stable_external" config-show.out || \
    loud_fail "wg config --show missing stable_external group. Output:\n$(cat config-show.out)"
grep -qF "provider_specific" config-show.out || \
    loud_fail "wg config --show missing provider_specific group. Output:\n$(cat config-show.out)"
grep -qF "experimental_external" config-show.out || \
    loud_fail "wg config --show missing experimental_external group. Output:\n$(cat config-show.out)"
assert_choices "wg config --show" config-show.out

if ! run_wg config --list >config-list.out 2>&1; then
    loud_fail "wg config --list failed: $(cat config-list.out)"
fi
grep -qF "[executor choices]" config-list.out || \
    loud_fail "wg config --list missing [executor choices]. Output:\n$(cat config-list.out)"
assert_choices "wg config --list" config-list.out

for template in opencode aider goose qwen cline crush amplifier; do
    [[ -f ".wg/executors/${template}.toml.example" ]] || \
        loud_fail "wg init did not seed .wg/executors/${template}.toml.example"
done

if grep -R -E 'sk-[A-Za-z0-9_-]{16,}|sk-or-[A-Za-z0-9_-]+' .wg init.log executors.out config-show.out config-list.out >/dev/null 2>&1; then
    loud_fail "credential-looking API key material appeared in executor-arena surface output"
fi

echo "PASS: config/list/discovery surfaces expose executor arena choices without credentials"
