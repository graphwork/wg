#!/usr/bin/env bash
# Project-scoped reusable profiles: two live projects stay route-isolated.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

scratch=$(make_scratch)
export HOME="$scratch/home"
export WG_GLOBAL_DIR="$HOME/.wg"
mkdir -p "$WG_GLOBAL_DIR" "$scratch/a" "$scratch/b"

( cd "$scratch/a" && wg init --no-agency >/dev/null )
( cd "$scratch/b" && wg init --no-agency >/dev/null )
a="$scratch/a/.wg"
b="$scratch/b/.wg"

# Strict dry-run is byte-for-byte non-mutating: no definition, association,
# history, or lock is created.
before=$(find "$scratch" -type f -printf '%p %s %T@\n' | sort)
plan=$(wg --dir "$a" --json profile select claude --dry-run)
after=$(find "$scratch" -type f -printf '%p %s %T@\n' | sort)
[[ "$before" = "$after" ]] || loud_fail "profile select --dry-run mutated files"
grep -q '"global_config_changed"' <<<"$plan" && loud_fail "dry-run unexpectedly emitted apply result"
grep -q '"materializes_global_profile_definition": true' <<<"$plan" || \
  loud_fail "dry-run did not disclose built-in definition materialization"
[[ ! -e "$a/profile-selection.json" ]] || loud_fail "dry-run wrote association"
[[ ! -e "$WG_GLOBAL_DIR/profile-usage.jsonl" ]] || loud_fail "dry-run wrote history"

# Definition mutations are atomic and never silently retarget an association.
wg --dir "$a" profile create mutable --model claude:sonnet >/dev/null
wg --dir "$a" profile select mutable --no-reload >/dev/null
wg --dir "$a" profile rename mutable moved >/dev/null
if moved_err=$(wg --dir "$a" config --merged 2>&1); then
  loud_fail "renaming selected definition did not fail closed"
fi
grep -qi 'unavailable' <<<"$moved_err" || loud_fail "rename recovery was not explained: $moved_err"
wg --dir "$a" profile select moved --no-reload >/dev/null
wg --dir "$a" profile delete moved --force >/dev/null
if deleted_err=$(wg --dir "$a" config --merged 2>&1); then
  loud_fail "deleting selected definition did not fail closed"
fi
grep -qi 'unavailable' <<<"$deleted_err" || loud_fail "delete recovery was not explained: $deleted_err"

# An invalid editor result never replaces the live reusable definition.
wg --dir "$a" profile create editable --model claude:haiku >/dev/null
editable="$WG_GLOBAL_DIR/profiles/editable.toml"
editable_before=$(sha256sum "$editable")
editor="$scratch/invalid-editor.sh"
printf '%s\n' '#!/usr/bin/env bash' 'printf "[agent]\\nmodel = [\\n" > "$1"' >"$editor"
chmod +x "$editor"
if EDITOR="$editor" wg --dir "$a" profile edit editable --no-reload >/dev/null 2>&1; then
  loud_fail "invalid profile edit unexpectedly succeeded"
fi
[[ "$editable_before" = "$(sha256sum "$editable")" ]] || loud_fail "invalid edit changed live profile bytes"
wg --dir "$a" profile delete editable >/dev/null

wg --dir "$a" profile select claude --no-reload >/dev/null
wg --dir "$b" profile select codex --no-reload >/dev/null
[[ ! -e "$WG_GLOBAL_DIR/config.toml" ]] || loud_fail "project selection rewrote global config"
[[ ! -e "$WG_GLOBAL_DIR/active-profile" ]] || loud_fail "project selection rewrote global active pointer"

a_cfg=$(wg --dir "$a" config --merged)
b_cfg=$(wg --dir "$b" config --merged)
grep -q 'task_agent.model = "claude:opus"' <<<"$a_cfg" || loud_fail "project A did not resolve claude"
grep -q 'task_agent.model = "codex:gpt-5.6-sol"' <<<"$b_cfg" || loud_fail "project B did not resolve codex"

# Catalog inspection is also strictly read-only (including legacy usage.log).
list_before=$(find "$scratch" -type f -exec sha256sum {} + | sort)
wg --dir "$a" --json profile list >/dev/null
list_after=$(find "$scratch" -type f -exec sha256sum {} + | sort)
[[ "$list_before" = "$list_after" ]] || loud_fail "profile list mutated files"

# Run both services concurrently without launching an LLM. Then switch/reload
# only B and prove A's persisted service route does not move.
start_wg_daemon "$scratch/a" --max-agents 0 --no-coordinator-agent --interval 60
a_pid="$WG_SMOKE_DAEMON_PID"
a_service="$WG_SMOKE_DAEMON_DIR"
start_wg_daemon "$scratch/b" --max-agents 0 --no-coordinator-agent --interval 60
b_pid="$WG_SMOKE_DAEMON_PID"
b_service="$WG_SMOKE_DAEMON_DIR"
kill -0 "$a_pid" 2>/dev/null || loud_fail "project A daemon not alive"
kill -0 "$b_pid" 2>/dev/null || loud_fail "project B daemon not alive"
grep -q '"model": "claude:opus"' "$a_service/service/coordinator-state-0.json" || \
  loud_fail "project A service did not start on claude"
grep -q '"model": "codex:gpt-5.6-sol"' "$b_service/service/coordinator-state-0.json" || \
  loud_fail "project B service did not start on codex"

wg --dir "$b" profile select nex >/dev/null
for _ in $(seq 1 50); do
  grep -q '"model": "nex:qwen3-coder-30b"' "$b_service/service/coordinator-state-0.json" && break
  sleep 0.1
done
grep -q '"model": "nex:qwen3-coder-30b"' "$b_service/service/coordinator-state-0.json" || \
  loud_fail "project B daemon did not reload nex"
grep -q '"model": "claude:opus"' "$a_service/service/coordinator-state-0.json" || \
  loud_fail "switching project B silently rerouted project A"

# Current project is pinned first; history is redacted and clearable.
first=$(wg --dir "$a" --json profile list | python3 -c 'import json,sys; print(json.load(sys.stdin)[0]["name"])')
[[ "$first" = claude ]] || loud_fail "current project profile was not pinned first: $first"
history=$(wg --dir "$a" --json profile history)
grep -q '"profile"' <<<"$history" || loud_fail "successful-event history missing"
if grep -Fq "$scratch" <<<"$history"; then loud_fail "history leaked canonical project path"; fi
if grep -Eqi 'prompt|api_key|credential|command_argv' <<<"$history"; then
  loud_fail "history leaked forbidden fields"
fi
wg --dir "$a" profile history --clear >/dev/null
[[ $(wg --dir "$a" --json profile history) = "[]" ]] || loud_fail "history clear failed"

echo "PASS: project profile selections, live service routes, read-only plan, and redacted usage stay isolated"
