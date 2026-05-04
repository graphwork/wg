#!/usr/bin/env bash
# Scenario: wg_init_writes_lockstep_agent_guides
#
# Regression: `wg init` created CLAUDE.md only for Claude-flavored projects.
# Fresh Codex / nex projects had no AGENTS.md, so codex sessions missed the
# project-specific layer-2 guide. Every supported init route must create both
# files from the same template.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
fake_home="$scratch/home"
mkdir -p "$fake_home/.config/workgraph"
: >"$fake_home/.config/workgraph/config.toml"

run_wg() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" \
        wg "$@"
}

assert_guides_lockstep() {
    local project="$1"
    [[ -f "$project/CLAUDE.md" ]] || loud_fail "$project missing CLAUDE.md"
    [[ -f "$project/AGENTS.md" ]] || loud_fail "$project missing AGENTS.md"

    if ! cmp -s "$project/CLAUDE.md" "$project/AGENTS.md"; then
        loud_fail "$project CLAUDE.md and AGENTS.md differ"
    fi

    grep -q 'wg agent-guide' "$project/CLAUDE.md" || \
        loud_fail "$project CLAUDE.md does not delegate to wg agent-guide"
    grep -q 'layer-2' "$project/CLAUDE.md" || \
        loud_fail "$project CLAUDE.md does not identify itself as layer-2"
}

init_and_check() {
    local name="$1"
    shift

    local project="$scratch/$name"
    mkdir -p "$project"
    cd "$project" || loud_fail "could not cd to $project"

    local out
    if ! out=$(run_wg init "$@" --no-agency 2>&1); then
        loud_fail "wg init $* failed for $name: $out"
    fi

    assert_guides_lockstep "$project"
}

init_and_check claude-cli --route claude-cli
init_and_check codex-cli --route codex-cli
init_and_check openrouter --route openrouter
init_and_check local --route local -e http://127.0.0.1:11434 -m nex:qwen3-coder
init_and_check nex-custom --route nex-custom -e http://127.0.0.1:8088 -m nex:qwen3-coder

echo "PASS: wg init writes byte-identical CLAUDE.md and AGENTS.md for all routes"
exit 0
