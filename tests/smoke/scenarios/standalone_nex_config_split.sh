#!/usr/bin/env bash
# Scenario: standalone_nex_config_split
#
# Drives the installed standalone `nex` binary and `wg nex` subcommand
# through real command-line flows with WG_FAKE_LLM. This pins the
# standalone config/state split without requiring network access:
#
#   * bare standalone `nex` discovers project .nex from cwd and starts a
#     fresh session each invocation instead of auto-resuming;
#   * CLI model and NEX_MODEL beat configured project/user/WG defaults;
#   * NEX_DIR beats discovered project .nex for standalone state/config;
#   * autonomous `wg nex` uses WG routing and ignores human ~/.nex state.

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v nex >/dev/null 2>&1; then
    loud_skip "MISSING NEX BINARY" "nex not found on PATH; run 'cargo install --path . --locked' from this checkout"
fi

scratch=$(make_scratch)
home="$scratch/home"
project="$scratch/project"
nested="$project/src/deep"
project_nex="$project/.nex"
env_nex="$scratch/env-nex"
fake_llm="$scratch/fake-llm.txt"

mkdir -p "$home/.nex" "$home/.wg" "$project/.wg" "$project_nex" "$env_nex" "$nested"
printf 'STANDALONE_NEX_SMOKE_RESPONSE\n' >"$fake_llm"

write_model_config() {
    local path="$1"
    local model_id="$2"
    mkdir -p "$(dirname "$path")"
    cat >"$path" <<EOF
[models.task_agent]
model = "nex:${model_id}"
EOF
}

list_standalone_journals() {
    local state_root="$1"
    if [[ -d "$state_root/sessions" ]]; then
        find "$state_root/sessions" -mindepth 2 -maxdepth 2 -name conversation.jsonl | sort
    fi
}

list_wg_journals() {
    local wg_dir="$1"
    if [[ -d "$wg_dir/chat" ]]; then
        find "$wg_dir/chat" -mindepth 2 -maxdepth 2 -name conversation.jsonl | sort
    fi
}

new_journal_after() {
    local label="$1"
    local list_fn="$2"
    local root="$3"
    local before="$scratch/${label}.before"
    local after="$scratch/${label}.after"
    local diff="$scratch/${label}.diff"
    shift 3

    "$list_fn" "$root" >"$before"
    local log="$scratch/${label}.log"
    if ! "$@" >"$log" 2>&1; then
        loud_fail "$label failed: $(cat "$log")"
    fi
    "$list_fn" "$root" >"$after"
    comm -13 "$before" "$after" >"$diff"
    local count
    count=$(wc -l <"$diff" | tr -d ' ')
    if [[ "$count" != "1" ]]; then
        loud_fail "$label should create exactly one new journal under $root. before=$(cat "$before") after=$(cat "$after") log=$(cat "$log")"
    fi
    cat "$diff"
}

assert_log_contains() {
    local label="$1"
    local expected="$2"
    local log="$scratch/${label}.log"
    if ! grep -q "$expected" "$log"; then
        loud_fail "$label log should contain $expected. log=$(cat "$log")"
    fi
}

assert_journal_model() {
    local journal="$1"
    local expected="$2"
    if ! grep -q "\"entry_type\":\"init\"" "$journal"; then
        loud_fail "journal has no init entry: $journal contents=$(cat "$journal")"
    fi
    if ! grep -q "\"model\":\"$expected\"" "$journal"; then
        loud_fail "journal $journal should use model=$expected. contents=$(cat "$journal")"
    fi
}

assert_journal_contains() {
    local journal="$1"
    local expected="$2"
    if ! grep -q "$expected" "$journal"; then
        loud_fail "journal $journal should contain $expected. contents=$(cat "$journal")"
    fi
}

clean_env=(env -i "HOME=$home" "PATH=$PATH" "TERM=${TERM:-xterm-256color}" "USER=wg-smoke")
base_nex=(nex --no-mcp --minimal-tools --max-turns 4)
pipe_quit=(bash -c 'printf "/quit\n" | "$@"' bash)

write_model_config "$project_nex/config.toml" "project-smoke-model"
write_model_config "$home/.nex/config.toml" "human-standalone-model"
write_model_config "$home/.wg/config.toml" "legacy-home-model"
write_model_config "$project/.wg/config.toml" "wg-autonomous-model"
write_model_config "$env_nex/config.toml" "env-dir-smoke-model"

cd "$nested"

first=$(new_journal_after "standalone-first" list_standalone_journals "$project_nex" \
    "${clean_env[@]}" WG_FAKE_LLM="$fake_llm" "${pipe_quit[@]}" "${base_nex[@]}" -m cli-smoke-model "first standalone smoke")
assert_journal_model "$first" "cli-smoke-model"
assert_journal_contains "$first" "STANDALONE_NEX_SMOKE_RESPONSE"
assert_log_contains "standalone-first" "STANDALONE_NEX_SMOKE_RESPONSE"

second=$(new_journal_after "standalone-second" list_standalone_journals "$project_nex" \
    "${clean_env[@]}" WG_FAKE_LLM="$fake_llm" "${pipe_quit[@]}" "${base_nex[@]}" -m cli-smoke-model "second standalone smoke")
assert_journal_model "$second" "cli-smoke-model"
assert_journal_contains "$second" "STANDALONE_NEX_SMOKE_RESPONSE"
assert_log_contains "standalone-second" "STANDALONE_NEX_SMOKE_RESPONSE"

if [[ "$first" == "$second" ]]; then
    loud_fail "standalone nex reused the same session journal for two bare invocations: $first"
fi

session_count=$(list_standalone_journals "$project_nex" | wc -l | tr -d ' ')
if [[ "$session_count" -lt 2 ]]; then
    loud_fail "expected at least two standalone project sessions, got $session_count"
fi

env_model_journal=$(new_journal_after "standalone-env-model" list_standalone_journals "$project_nex" \
    "${clean_env[@]}" WG_FAKE_LLM="$fake_llm" NEX_MODEL=env-smoke-model "${pipe_quit[@]}" "${base_nex[@]}" "env model smoke")
assert_journal_model "$env_model_journal" "env-smoke-model"
assert_journal_contains "$env_model_journal" "STANDALONE_NEX_SMOKE_RESPONSE"
assert_log_contains "standalone-env-model" "STANDALONE_NEX_SMOKE_RESPONSE"

env_dir_journal=$(new_journal_after "standalone-env-dir" list_standalone_journals "$env_nex" \
    "${clean_env[@]}" WG_FAKE_LLM="$fake_llm" NEX_DIR="$env_nex" "${pipe_quit[@]}" "${base_nex[@]}" "env dir smoke")
assert_journal_model "$env_dir_journal" "env-dir-smoke-model"
assert_journal_contains "$env_dir_journal" "STANDALONE_NEX_SMOKE_RESPONSE"
assert_log_contains "standalone-env-dir" "STANDALONE_NEX_SMOKE_RESPONSE"

project_journal=$(new_journal_after "standalone-project-config" list_standalone_journals "$project_nex" \
    "${clean_env[@]}" WG_FAKE_LLM="$fake_llm" "${pipe_quit[@]}" "${base_nex[@]}" "project config smoke")
assert_journal_model "$project_journal" "project-smoke-model"
assert_journal_contains "$project_journal" "STANDALONE_NEX_SMOKE_RESPONSE"
assert_log_contains "standalone-project-config" "STANDALONE_NEX_SMOKE_RESPONSE"

wg_journal=$(new_journal_after "wg-nex-autonomous" list_wg_journals "$project/.wg" \
    "${clean_env[@]}" WG_FAKE_LLM="$fake_llm" wg nex --autonomous --no-mcp --minimal-tools --max-turns 4 "wg autonomous smoke")
assert_journal_model "$wg_journal" "wg-autonomous-model"
assert_journal_contains "$wg_journal" "STANDALONE_NEX_SMOKE_RESPONSE"

if [[ -d "$home/.nex/sessions" ]] && find "$home/.nex/sessions" -mindepth 2 -maxdepth 2 -name conversation.jsonl | grep -q .; then
    loud_fail "wg nex autonomous should not create/reuse human ~/.nex sessions"
fi

echo "PASS: standalone nex config/state split command flow"
