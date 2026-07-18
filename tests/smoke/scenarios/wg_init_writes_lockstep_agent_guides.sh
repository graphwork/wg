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
mkdir -p "$fake_home/.wg"
: >"$fake_home/.wg/config.toml"

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
    grep -q '<!-- worksgood-managed-guide:v1:start -->' "$project/CLAUDE.md" || \
        loud_fail "$project CLAUDE.md lacks the versioned WorksGood start marker"
    grep -q '<!-- worksgood-managed-guide:v1:end -->' "$project/CLAUDE.md" || \
        loud_fail "$project CLAUDE.md lacks the versioned WorksGood end marker"
    grep -q 'WorksGood' "$project/CLAUDE.md" || \
        loud_fail "$project CLAUDE.md does not lead with the WorksGood identity"
    if grep -qi 'workgraph' "$project/CLAUDE.md"; then
        loud_fail "$project CLAUDE.md leaked stale WorkGraph branding"
    fi
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

# Upgrade an unversioned managed region in place. User-authored bytes on both
# sides must survive, both project guides must remain identical, and a second
# repair must be a byte-level no-op. The same command repairs the global Claude
# guide, which is the explicit migration surface for existing installations.
legacy_project="$scratch/legacy-project"
mkdir -p "$legacy_project" "$fake_home/.claude"
legacy_body='<!-- wg-managed -->
# WorkGraph (project-specific guide)

Use workgraph for task management.

This guide is written to both `CLAUDE.md` and `AGENTS.md` and kept in
lock-step. The two files exist because Claude Code and Codex CLI look for
different filenames, but they should never drift in content. Any divergence is
a bug. Update both together.'
for guide in "$legacy_project/CLAUDE.md" "$legacy_project/AGENTS.md"; do
    {
        printf '%s\n' '# user preface' ''
        printf '%s\n' "$legacy_body"
        printf '%s\n' '' '<!-- user appendix -->' 'keep exactly'
    } >"$guide"
done
{
    printf '%s\n' '# global user preface' ''
    printf '%s\n' "$legacy_body"
    printf '%s\n' '' '<!-- global user appendix -->' 'keep globally'
} >"$fake_home/.claude/CLAUDE.md"

cd "$legacy_project" || loud_fail "could not cd to legacy fixture"
repair_out=$(run_wg setup --repair-guides 2>&1) || \
    loud_fail "wg setup --repair-guides failed: $repair_out"
echo "$repair_out" | grep -q 'Migrated WorksGood guide' || \
    loud_fail "repair output did not report the managed migration: $repair_out"
assert_guides_lockstep "$legacy_project"
head -1 "$legacy_project/CLAUDE.md" | grep -qx '# user preface' || \
    loud_fail "repair changed project text before the managed block"
tail -2 "$legacy_project/CLAUDE.md" | grep -q 'keep exactly' || \
    loud_fail "repair changed project text after the managed block"
head -1 "$fake_home/.claude/CLAUDE.md" | grep -qx '# global user preface' || \
    loud_fail "repair changed global text before the managed block"
tail -2 "$fake_home/.claude/CLAUDE.md" | grep -q 'keep globally' || \
    loud_fail "repair changed global text after the managed block"
if grep -qi 'workgraph' "$legacy_project/CLAUDE.md" "$fake_home/.claude/CLAUDE.md"; then
    loud_fail "repair left stale WorkGraph branding in managed guides"
fi

before_project=$(sha256sum "$legacy_project/CLAUDE.md" "$legacy_project/AGENTS.md")
before_global=$(sha256sum "$fake_home/.claude/CLAUDE.md")
run_wg setup --repair-guides >/dev/null 2>&1 || loud_fail "idempotent guide repair failed"
after_project=$(sha256sum "$legacy_project/CLAUDE.md" "$legacy_project/AGENTS.md")
after_global=$(sha256sum "$fake_home/.claude/CLAUDE.md")
[[ "$before_project" == "$after_project" ]] || loud_fail "second repair changed project guides"
[[ "$before_global" == "$after_global" ]] || loud_fail "second repair changed global guide"

# Legacy configuration locations remain readable, but every fallback must name
# both the old and canonical targets so migration is actionable.
legacy_notify_home="$scratch/legacy-notify-home"
legacy_notify_project="$scratch/legacy-notify-project"
mkdir -p "$legacy_notify_home/.config/workgraph" "$legacy_notify_project"
cat >"$legacy_notify_home/.config/workgraph/notify.toml" <<'EOF'
[telegram]
bot_token = "123456:legacy-test-token"
chat_id = "123"
EOF
legacy_notify_out=$(cd "$legacy_notify_project" && \
    env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID HOME="$legacy_notify_home" \
        XDG_CONFIG_HOME="$legacy_notify_home/.config" wg telegram list-bots 2>&1) || \
    loud_fail "legacy notification config did not load: $legacy_notify_out"
echo "$legacy_notify_out" | grep -q 'using legacy notification config' || \
    loud_fail "legacy notification config lacked a precise diagnostic: $legacy_notify_out"
echo "$legacy_notify_out" | grep -q '.config/worksgood/notify.toml' || \
    loud_fail "legacy notification diagnostic lacked the canonical target: $legacy_notify_out"

legacy_global_home="$scratch/legacy-global-home"
legacy_global_project="$scratch/legacy-global-project"
mkdir -p "$legacy_global_home/.workgraph" "$legacy_global_project"
: >"$legacy_global_home/.workgraph/config.toml"
legacy_global_out=$(cd "$legacy_global_project" && \
    env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID HOME="$legacy_global_home" \
        XDG_CONFIG_HOME="$legacy_global_home/.config" wg config --show 2>&1) || \
    loud_fail "legacy global directory did not load: $legacy_global_out"
echo "$legacy_global_out" | grep -q 'reading legacy WorksGood global config' || \
    loud_fail "legacy global config lacked a precise diagnostic: $legacy_global_out"
echo "$legacy_global_out" | grep -q "$legacy_global_home/.wg" || \
    loud_fail "legacy global diagnostic lacked the canonical ~/.wg target: $legacy_global_out"

echo "PASS: fresh and migrated WorksGood guides are lockstep, bounded, and idempotent"
exit 0
