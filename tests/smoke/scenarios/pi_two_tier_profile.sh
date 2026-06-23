#!/usr/bin/env bash
# Smoke: the two-tier Pi profile setter (`wg profile pi`) drives the full
# list → select → apply → persist human flow on the real binary.
#
# Isolation: every invocation sets an isolated HOME (profile files live under
# Config::global_dir() = $HOME/.wg) AND passes --dir to a scratch graph dir, so
# the scenario never reads or pokes the developer's real ~/.wg or live daemon.
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
export HOME="$scratch/home"
mkdir -p "$HOME/.wg"
WG_DIR_FLAG="--dir $scratch/.wg"
mkdir -p "$scratch/.wg"
cd "$scratch"

# shellcheck disable=SC2086
wg() { command wg $WG_DIR_FLAG "$@"; }

PI="$HOME/.wg/profiles/pi.toml"

# ── 1. LIST: the picker lists models from the profile, not hardcoded ─────────
list_out=$(wg profile pi --list 2>&1) || loud_fail "wg profile pi --list failed:
$list_out"
grep -q "pi:openrouter/z-ai/glm-5.2" <<<"$list_out" \
    || loud_fail "--list did not surface the configured strong model:
$list_out"
grep -q "openrouter:deepseek/deepseek-chat" <<<"$list_out" \
    || loud_fail "--list did not surface the configured weak model:
$list_out"
grep -q "\[strong\]" <<<"$list_out" \
    || loud_fail "--list did not tag the strong tier:
$list_out"

# ── 2. SELECT + APPLY: set both tiers positionally; echo shows old → new ─────
set_out=$(wg profile pi openrouter:qwen/qwen3-max openrouter:deepseek/deepseek-v3.1 2>&1) \
    || loud_fail "wg profile pi <strong> <weak> failed:
$set_out"
grep -q "glm-5.2 → openrouter:qwen/qwen3-max" <<<"$set_out" \
    || loud_fail "set echo missing strong old → new transition:
$set_out"
grep -q "deepseek-chat → openrouter:deepseek/deepseek-v3.1" <<<"$set_out" \
    || loud_fail "set echo missing weak old → new transition:
$set_out"
[ -f "$PI" ] || loud_fail "profile file was not written at $PI"

# The surgical patch updated the §4.1 key-set AND preserved the comment block.
grep -q 'standard = "openrouter:qwen/qwen3-max"' "$PI" \
    || loud_fail "tiers.standard not patched to strong:
$(cat "$PI")"
grep -q 'fast = "openrouter:deepseek/deepseek-v3.1"' "$PI" \
    || loud_fail "tiers.fast not patched to weak:
$(cat "$PI")"
grep -q "PLUGIN INSTALL" "$PI" \
    || loud_fail "comment block was lost by the patch (should be a line patch):
$(cat "$PI")"

# ── 3. PERSISTS: --show reflects the applied tiers on a fresh process ────────
show_out=$(wg profile pi --show 2>&1) || loud_fail "wg profile pi --show failed:
$show_out"
grep -q "strong = openrouter:qwen/qwen3-max" <<<"$show_out" \
    || loud_fail "--show did not reflect the persisted strong tier:
$show_out"
grep -q "weak   = openrouter:deepseek/deepseek-v3.1" <<<"$show_out" \
    || loud_fail "--show did not reflect the persisted weak tier:
$show_out"

# ── 4. PARTIAL: --weak alone leaves strong untouched ────────────────────────
partial_out=$(wg profile pi --weak openrouter:deepseek/deepseek-chat 2>&1) \
    || loud_fail "partial --weak update failed:
$partial_out"
grep -q "strong = openrouter:qwen/qwen3-max .* (unchanged)" <<<"$partial_out" \
    || loud_fail "partial weak update should leave strong unchanged:
$partial_out"

# ── 5. ACTIVE: when pi is active, the set re-applies as global config so the ─
#       next turn/worker picks it up (reflected next turn). --dir → no daemon,
#       so the reload note degrades gracefully (no live daemon is touched).
wg profile use pi --no-reload >/dev/null 2>&1 \
    || loud_fail "wg profile use pi failed"
wg profile pi --strong openrouter:z-ai/glm-5.2 >/dev/null 2>&1 \
    || loud_fail "set on active pi profile failed"
grep -q 'standard = "openrouter:z-ai/glm-5.2"' "$HOME/.wg/config.toml" \
    || loud_fail "active set did not re-apply strong to the global config.toml (next turn would not see it):
$(cat "$HOME/.wg/config.toml" 2>/dev/null || echo '(no config.toml)')"

# ── 6. GRAMMAR: a lone positional is rejected as ambiguous ──────────────────
if wg profile pi openrouter:z-ai/glm-5.2 >/dev/null 2>&1; then
    loud_fail "a single positional tier must be rejected as ambiguous"
fi

echo "PASS: wg profile pi list/select/apply/persist/active/grammar"
