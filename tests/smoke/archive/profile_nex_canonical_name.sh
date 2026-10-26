#!/usr/bin/env bash
# Smoke: the in-process LLM handler starter profile is named `nex` (matches the
# `wg nex` subcommand). Pins rename-wgnext-profile: previously the same thing
# was spelled three ways — `wgnext` (filename), `wg-next` (description), and
# `wg nex` (the actual subcommand). Asserts that:
#   1. `wg profile init-starters` writes `nex.toml` (not `wgnext.toml`).
#   2. `wg profile list` shows the starter as `nex` with a `wg nex` description.
#   3. No `wgnext` or `wg-next` strings leak into init-starters output or
#      the rendered profile file (only canonical `nex` / `wg nex`).
#   4. Backward compat: an existing `wgnext.toml` on disk is auto-migrated to
#      `nex.toml` on the next `init-starters`, with a visible notice.
#   5. Loading the legacy `wgnext` name still works but emits a deprecation
#      hint pointing the user at `nex`.
set -eu
source "$(dirname "$0")/_helpers.sh"
require_wg

scratch=$(make_scratch)

# ── Case 1: fresh init-starters writes nex.toml, NOT wgnext.toml ──
export HOME="$scratch/fresh"
mkdir -p "$HOME/.wg"

init_out=$(wg profile init-starters 2>&1)

if [[ ! -f "$HOME/.wg/profiles/nex.toml" ]]; then
    loud_fail "init-starters should write nex.toml; not found.\nfiles: $(ls "$HOME/.wg/profiles/" 2>&1)\nout: $init_out"
fi
if [[ -f "$HOME/.wg/profiles/wgnext.toml" ]]; then
    loud_fail "init-starters must NOT write the legacy wgnext.toml; found it.\nout: $init_out"
fi
if echo "$init_out" | grep -qE 'wgnext|wg-next'; then
    loud_fail "init-starters output must not surface 'wgnext' or 'wg-next' on a fresh install: $init_out"
fi
if ! echo "$init_out" | grep -q "wg profile use claude|codex|nex"; then
    loud_fail "init-starters activation hint should list 'claude|codex|nex': $init_out"
fi

# The rendered nex.toml description must say `wg nex` (not `wg-next`).
if ! grep -q 'wg nex' "$HOME/.wg/profiles/nex.toml"; then
    loud_fail "nex.toml description should reference 'wg nex': $(cat "$HOME/.wg/profiles/nex.toml")"
fi
if grep -qE 'wg-next|wgnext' "$HOME/.wg/profiles/nex.toml"; then
    loud_fail "nex.toml must not contain legacy 'wg-next' or 'wgnext' strings: $(cat "$HOME/.wg/profiles/nex.toml")"
fi

# wg profile list shows the canonical name.
list_out=$(wg profile list 2>&1)
if ! echo "$list_out" | grep -qE '\[user\][[:space:]]+nex[[:space:]]+wg nex:'; then
    loud_fail "wg profile list should show '[user] nex   wg nex: ...': $list_out"
fi
if echo "$list_out" | grep -qE 'wgnext|wg-next'; then
    loud_fail "wg profile list output must not surface 'wgnext' or 'wg-next': $list_out"
fi

# ── Case 2: legacy wgnext.toml on disk is auto-migrated to nex.toml ──
export HOME="$scratch/legacy"
mkdir -p "$HOME/.wg/profiles"
cat > "$HOME/.wg/profiles/wgnext.toml" <<'EOF'
description = "user-edited legacy profile"

[agent]
model = "local:qwen3-coder-30b"

[[llm_endpoints.endpoints]]
name = "default"
provider = "oai-compat"
url = "http://desktop.local:30000"
api_key_env = ""
is_default = true
EOF

migrate_out=$(wg profile init-starters 2>&1)

if [[ -f "$HOME/.wg/profiles/wgnext.toml" ]]; then
    loud_fail "init-starters should rename wgnext.toml -> nex.toml; legacy file still present.\nout: $migrate_out"
fi
if [[ ! -f "$HOME/.wg/profiles/nex.toml" ]]; then
    loud_fail "init-starters should create nex.toml from the legacy wgnext.toml.\nout: $migrate_out"
fi
if ! echo "$migrate_out" | grep -q "migrated"; then
    loud_fail "init-starters should announce the migration with a 'migrated' line: $migrate_out"
fi
# The migrated profile must preserve the user's edits (different URL).
if ! grep -q 'http://desktop.local:30000' "$HOME/.wg/profiles/nex.toml"; then
    loud_fail "migrated nex.toml lost user's endpoint edit: $(cat "$HOME/.wg/profiles/nex.toml")"
fi

# ── Case 3: loading the legacy `wgnext` name still works, with a deprecation hint ──
export HOME="$scratch/deprecation"
mkdir -p "$HOME/.wg/profiles"
cat > "$HOME/.wg/profiles/wgnext.toml" <<'EOF'
description = "still here"

[agent]
model = "local:qwen3-coder-30b"
EOF

show_out=$(wg profile show wgnext 2>&1)
if ! echo "$show_out" | grep -qE 'deprecated|canonical'; then
    loud_fail "loading legacy 'wgnext' should emit a deprecation hint: $show_out"
fi
if ! echo "$show_out" | grep -q 'still here'; then
    loud_fail "loading legacy 'wgnext' should still surface profile contents (backward compat): $show_out"
fi

echo "PASS: profile_nex_canonical_name"
