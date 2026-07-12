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
# fix-strong-tier: the STRONG tier is normalized to a `pi:` route on write (so
# strong-tier work runs through the self-authenticating pi handler, NOT the
# in-process nex OpenRouter client that would need a wg-side key). The user
# typed a raw `openrouter:` spec; what gets echoed/persisted is the `pi:` form.
# The WEAK/agency tier keeps its native `openrouter:` route.
set_out=$(wg profile pi openrouter:qwen/qwen3-max openrouter:deepseek/deepseek-v3.1 2>&1) \
    || loud_fail "wg profile pi <strong> <weak> failed:
$set_out"
grep -q "glm-5.2 → pi:openrouter/qwen/qwen3-max" <<<"$set_out" \
    || loud_fail "set echo missing strong old → new transition (expected pi: route):
$set_out"
grep -q "deepseek-chat → openrouter:deepseek/deepseek-v3.1" <<<"$set_out" \
    || loud_fail "set echo missing weak old → new transition:
$set_out"
[ -f "$PI" ] || loud_fail "profile file was not written at $PI"

# The surgical patch updated the §4.1 key-set AND preserved the comment block.
# Strong → pi: route (pi handler auths itself); weak → native openrouter: route.
grep -q 'standard = "pi:openrouter/qwen/qwen3-max"' "$PI" \
    || loud_fail "tiers.standard not patched to the pi: strong route:
$(cat "$PI")"
grep -q 'premium = "pi:openrouter/qwen/qwen3-max"' "$PI" \
    || loud_fail "tiers.premium not patched to the pi: strong route:
$(cat "$PI")"
grep -q 'fast = "openrouter:deepseek/deepseek-v3.1"' "$PI" \
    || loud_fail "tiers.fast not patched to weak:
$(cat "$PI")"
# The strong tier must NOT persist as a raw openrouter: spec (the bug that made
# wg the OpenRouter client and required a wg-side key → exit-1-at-spawn).
if grep -q 'standard = "openrouter:qwen/qwen3-max"' "$PI"; then
    loud_fail "strong tier persisted as a raw openrouter: spec (routes to nex, needs a wg key):
$(cat "$PI")"
fi
grep -q "PLUGIN INSTALL" "$PI" \
    || loud_fail "comment block was lost by the patch (should be a line patch):
$(cat "$PI")"

# ── 3. PERSISTS: --show reflects the applied tiers on a fresh process ────────
show_out=$(wg profile pi --show 2>&1) || loud_fail "wg profile pi --show failed:
$show_out"
grep -q "strong = pi:openrouter/qwen/qwen3-max" <<<"$show_out" \
    || loud_fail "--show did not reflect the persisted pi: strong tier:
$show_out"
grep -q "weak   = openrouter:deepseek/deepseek-v3.1" <<<"$show_out" \
    || loud_fail "--show did not reflect the persisted weak tier:
$show_out"

# ── 4. PARTIAL: --weak alone leaves strong untouched ────────────────────────
partial_out=$(wg profile pi --weak openrouter:deepseek/deepseek-chat 2>&1) \
    || loud_fail "partial --weak update failed:
$partial_out"
grep -q "strong = pi:openrouter/qwen/qwen3-max .* (unchanged)" <<<"$partial_out" \
    || loud_fail "partial weak update should leave strong unchanged:
$partial_out"

# ── 5. ACTIVE: when pi is active, the set re-applies as global config so the ─
#       next turn/worker picks it up (reflected next turn). --dir → no daemon,
#       so the reload note degrades gracefully (no live daemon is touched).
wg profile use pi --no-reload >/dev/null 2>&1 \
    || loud_fail "wg profile use pi failed"
wg profile pi --strong openrouter:z-ai/glm-5.2 >/dev/null 2>&1 \
    || loud_fail "set on active pi profile failed"
# fix-strong-tier: the active re-apply must write the pi: strong route into the
# global config so the NEXT worker dispatches through pi (handler_for_model →
# ExecutorKind::Pi), needing no wg-side OpenRouter key.
grep -q 'standard = "pi:openrouter/z-ai/glm-5.2"' "$HOME/.wg/config.toml" \
    || loud_fail "active set did not re-apply the pi: strong route to the global config.toml (next worker would route through nex and need a wg key):
$(cat "$HOME/.wg/config.toml" 2>/dev/null || echo '(no config.toml)')"
if grep -q 'standard = "openrouter:z-ai/glm-5.2"' "$HOME/.wg/config.toml"; then
    loud_fail "strong tier re-applied as a raw openrouter: spec (routes to nex, needs a wg key):
$(cat "$HOME/.wg/config.toml")"
fi

# ── 6. CUSTOM PROFILE: create/select/edit/activate on the real CLI ───────────
# Clone the newly shipped direct Codex Sol/Luna starter. This must use the
# embedded starter without materializing or replacing the user's codex.toml.
wg profile create codex-56 --from codex >/dev/null 2>&1 \
    || loud_fail "custom profile creation from built-in codex failed"
CUSTOM="$HOME/.wg/profiles/codex-56.toml"
grep -q 'standard = "codex:gpt-5.6-sol"' "$CUSTOM" \
    || loud_fail "direct Codex Sol starter was not preserved: $(cat "$CUSTOM")"
grep -q 'fast = "codex:gpt-5.6-luna"' "$CUSTOM" \
    || loud_fail "direct Codex Luna starter was not preserved: $(cat "$CUSTOM")"
printf '\n# user-owned sentinel\n' >>"$CUSTOM"

# One command selects the custom profile, partially updates its strong model,
# and independently updates weak reasoning. Handler-first strings are verbatim.
custom_out=$(wg profile pi --profile codex-56 \
    --strong codex:gpt-5.6-terra --weak-reasoning minimal 2>&1) \
    || loud_fail "custom profile combined update failed:
$custom_out"
grep -q 'profile: codex-56' <<<"$custom_out" \
    || loud_fail "custom profile selection was not reflected in output:
$custom_out"
grep -q 'model = "codex:gpt-5.6-terra"' "$CUSTOM" \
    || loud_fail "handler-first custom strong route was not preserved verbatim:
$(cat "$CUSTOM")"
grep -q 'reasoning = "medium"' "$CUSTOM" \
    || loud_fail "model update erased inherited default reasoning:
$(cat "$CUSTOM")"
grep -q 'fast = "codex:gpt-5.6-luna"' "$CUSTOM" \
    || loud_fail "partial strong/reasoning update changed the weak model:
$(cat "$CUSTOM")"
grep -q 'fast_reasoning = "minimal"' "$CUSTOM" \
    || loud_fail "weak reasoning update was not persisted:
$(cat "$CUSTOM")"
grep -q '# user-owned sentinel' "$CUSTOM" \
    || loud_fail "custom profile patch overwrote user-owned content:
$(cat "$CUSTOM")"

# The inactive edit must not touch resolved global routing. Activation then
# makes the custom handler-first routes and reasoning visible to config resolve.
if grep -q 'codex:gpt-5.6-terra' "$HOME/.wg/config.toml"; then
    loud_fail "inactive custom profile edit leaked into global config"
fi
wg profile use codex-56 --no-reload >/dev/null 2>&1 \
    || loud_fail "custom profile activation failed"
models_out=$(wg config --models 2>&1) \
    || loud_fail "resolved routing query failed: $models_out"
grep -q 'codex:gpt-5.6-terra' <<<"$models_out" \
    || loud_fail "activated custom strong route did not resolve: $models_out"
grep -q 'codex:gpt-5.6-luna' <<<"$models_out" \
    || loud_fail "activated custom weak route did not resolve: $models_out"
grep -Eq 'evaluator +fast +codex:gpt-5.6-luna +codex +codex +minimal' <<<"$models_out" \
    || loud_fail "activated custom weak reasoning did not resolve: $models_out"

# ── 7. GRAMMAR: a lone positional is rejected as ambiguous ──────────────────
if wg profile pi openrouter:z-ai/glm-5.2 >/dev/null 2>&1; then
    loud_fail "a single positional tier must be rejected as ambiguous"
fi

echo "PASS: wg profile pi built-in/custom model+reasoning editing"
