#!/usr/bin/env bash
# Direct Codex regression: the built-in profile is a one-command Sol/Luna
# activation and the real spawned worker argv carries WG reasoning as Codex
# model_reasoning_effort without coupling model_verbosity.
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

scratch=$(make_scratch)
fake_home="$scratch/home"
bin="$scratch/bin"
project="$scratch/project"
mkdir -p "$fake_home/.config/workgraph" "$bin" "$project"
: >"$fake_home/.config/workgraph/config.toml"

cat >"$bin/codex" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$@" >"$HOME/codex-argv.txt"
cat >/dev/null
printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"OK"}}'
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":1,"cached_input_tokens":0,"output_tokens":1}}'
SH
chmod +x "$bin/codex"

export HOME="$fake_home"
export XDG_CONFIG_HOME="$fake_home/.config"
export PATH="$bin:$PATH"
unset WG_EXECUTOR_TYPE WG_MODEL WG_TIER WG_AGENT_ID WG_TASK_ID

cd "$project"
wg init -m claude:opus >init.log 2>&1 || loud_fail "wg init failed: $(tail -20 init.log)"
wg config --auto-assign false --no-reload >config.log 2>&1 || loud_fail "config failed: $(tail -20 config.log)"

# No init-starters and no profile TOML: activation must use the embedded starter.
wg profile use codex --no-reload >profile.log 2>&1 || loud_fail "profile activation failed: $(cat profile.log)"
profile_file="$HOME/.wg/profiles/codex.toml"
[[ -s "$profile_file" ]] || loud_fail "built-in codex profile was not materialized"

models=$(wg config --models 2>&1) || loud_fail "config --models failed: $models"
grep -q 'codex:gpt-5.6-sol' <<<"$models" || loud_fail "Sol missing from activated profile: $models"
grep -q 'codex:gpt-5.6-luna' <<<"$models" || loud_fail "Luna missing from activated profile: $models"

# Once present, the file is user-owned. Re-activation must not silently replace
# even a harmless customization with a fresh embedded snapshot.
printf '\n# user-owned sentinel\n' >>"$profile_file"
before=$(sha256sum "$profile_file")
wg profile use codex --no-reload >profile-again.log 2>&1 || loud_fail "profile re-activation failed: $(cat profile-again.log)"
after=$(sha256sum "$profile_file")
[[ "$before" == "$after" ]] || loud_fail "existing user codex profile was silently overwritten"

wg add 'direct codex reasoning probe' --id direct-codex-probe --reasoning high --no-place \
  -d 'Reply OK' >add.log 2>&1 || loud_fail "wg add failed: $(cat add.log)"
start_wg_daemon "$project" --max-agents 1 --no-coordinator-agent --interval 1 \
  || loud_fail "daemon failed to start"

for _ in $(seq 1 80); do
  [[ -s "$HOME/codex-argv.txt" ]] && break
  sleep 0.25
done
[[ -s "$HOME/codex-argv.txt" ]] || loud_fail "fake direct Codex was never executed; daemon: $(tail -30 "$WG_SMOKE_DAEMON_DIR/service/daemon.log" 2>/dev/null)"

argv=$(cat "$HOME/codex-argv.txt")
grep -qx 'gpt-5.6-sol' <<<"$argv" || loud_fail "worker did not execute Sol: $argv"
grep -qx 'model_reasoning_effort="high"' <<<"$argv" || loud_fail "reasoning override missing from actual Codex argv: $argv"
grep -qx 'model_verbosity="high"' <<<"$argv" || loud_fail "independent configured verbosity was lost: $argv"

# The generated artifact is the other side of the real path: config resolution
# -> spawn plan -> shell argv -> executable.
run_sh=$(find "$WG_SMOKE_DAEMON_DIR/agents" -name run.sh -type f | head -1)
[[ -n "$run_sh" ]] || loud_fail "no generated worker run.sh"
grep -q 'model_reasoning_effort="high"' "$run_sh" || loud_fail "generated run.sh omitted effort: $(cat "$run_sh")"

echo "PASS: one-command direct Codex profile selects Sol/Luna and actual worker argv carries reasoning independently of verbosity"
