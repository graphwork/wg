#!/usr/bin/env bash
# Scenario: setup_scope_flag — ARCHIVED (retired). The `--scope global` / `--scope
# both` contract it pins was removed by task `project-local-pi-cli` (commit
# 27e802d9, `docs/design-project-local-pi-config.md` + `docs/config-precedence.md`):
# machine-global routing is no longer writable, so `wg setup --scope global|both`
# fails closed with `error[WG-GLOBAL-CONFIG-WRITE-REFUSED]` before any mutation, and
# `--scope local` is the only accepted value — it writes the project document
# `worksgood.toml`, not `./.wg/config.toml`. Both halves of the fixture's file-path
# assertions therefore have no subject. The script and its grow-only `[[scenario]]`
# manifest entry are preserved for history; the scenario SKIPs loudly (exit 77)
# instead of FAILing. The replacement contract (setup writes only the project
# document and refuses machine-global writes) is pinned by the live
# `project_local_pi_cli` scenario. Original header follows.
#
# Scenario: setup_scope_flag
#
# Locks in the `wg setup --scope <global|local|both>` contract introduced by
# improve-wg-setup (per docs/config-ux-design.md §4.1). Pre-existing setup
# behavior was scope-ambiguous: it always wrote ~/.wg/config.toml
# regardless of where the user was, and there was no way to ask for a
# project-local config or for both at once.
#
# This scenario verifies, against the real `wg` binary, that:
#
#   1. `wg setup --scope global --route pi --model pi:test:fake --yes` writes ONLY the
#      global config (~/.wg/config.toml) and leaves the cwd's .wg
#      untouched.
#   2. `wg setup --scope local --route pi --model pi:test:fake --yes` writes ONLY the
#      local config (./.wg/config.toml) and leaves ~/.wg untouched.
#   3. `wg setup --scope both --route pi --model pi:test:fake --yes` writes BOTH paths.
#   4. `wg setup --scope garbage --yes` refuses with an error mentioning
#      the valid values.
#   5. The summary screen prints "Will write N keys" + "Will NOT write
#      (built-in defaults): M more" (per §4.3).

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
fake_home="$scratch/home"
mkdir -p "$fake_home"
export HOME="$fake_home"
unset WG_DIR ANTHROPIC_API_KEY OPENROUTER_API_KEY OPENAI_API_KEY 2>/dev/null || true

# ── Retirement guard: machine-global setup writes are refused ───────────────
# `wg setup --scope global|both` now fails closed (`WG-GLOBAL-CONFIG-WRITE-REFUSED`)
# before any mutation — task `project-local-pi-cli`, commit 27e802d9. The scenario's
# subject (global/both writes, and the `.wg/config.toml` paths they land in) has no
# producer, so prove the refusal behaviourally in a throwaway dir and SKIP loudly;
# never FAIL a scenario whose subject was removed on purpose.
probe="$scratch/retire-probe"
mkdir -p "$probe/home"
(
    cd "$probe" || exit 1
    env -u WG_DIR -u WG_PROJECT_ROOT -u WG_TASK_ID -u WG_AGENT_ID \
        HOME="$probe/home" \
        wg setup --scope global --route pi --model pi:test:fake --yes 2>&1 || true
) >"$probe/probe.log" 2>&1
if grep -q 'WG-GLOBAL-CONFIG-WRITE-REFUSED' "$probe/probe.log"; then
    loud_skip "SETUP --scope GLOBAL|BOTH RETIRED" \
        'machine-global routing writes were removed by task project-local-pi-cli (commit 27e802d9, docs/design-project-local-pi-config.md): `wg setup --scope global|both` fails closed with error[WG-GLOBAL-CONFIG-WRITE-REFUSED] and `--scope local` writes the project document worksgood.toml rather than ./.wg/config.toml, so the per-scope file-path assertions in this scenario have no subject. The replacement contract is pinned by project_local_pi_cli.'
fi

# ── 1. --scope global writes only global ──────────────────────────────
proj1="$scratch/proj1"
mkdir -p "$proj1"
(
    cd "$proj1"
    if ! wg setup --scope global --route pi --model pi:test:fake --yes >"$scratch/scope1.log" 2>&1; then
        cat "$scratch/scope1.log" 1>&2
        exit 1
    fi
) || loud_fail "wg setup --scope global --route pi --model pi:test:fake --yes failed: $(tail -10 "$scratch/scope1.log")"

if [[ ! -f "$fake_home/.wg/config.toml" ]]; then
    loud_fail "expected $fake_home/.wg/config.toml after --scope global, got: $(ls "$fake_home")"
fi
if [[ -f "$proj1/.wg/config.toml" ]]; then
    loud_fail "--scope global must NOT write a local config; found $proj1/.wg/config.toml"
fi
# Summary screen contract.
if ! grep -qE "Will write [0-9]+ keys" "$scratch/scope1.log"; then
    loud_fail "summary screen missing 'Will write N keys' line. Log:\n$(cat "$scratch/scope1.log")"
fi
if ! grep -qE "Will NOT write \(built-in defaults\): [0-9]+ more" "$scratch/scope1.log"; then
    loud_fail "summary screen missing 'Will NOT write (built-in defaults): N more' line. Log:\n$(cat "$scratch/scope1.log")"
fi

# Reset for the next case.
rm -rf "$fake_home/.wg" "$fake_home/.wg" "$proj1"

# ── 2. --scope local writes only local ────────────────────────────────
proj2="$scratch/proj2"
mkdir -p "$proj2"
(
    cd "$proj2"
    if ! wg setup --scope local --route pi --model pi:test:fake --yes >"$scratch/scope2.log" 2>&1; then
        cat "$scratch/scope2.log" 1>&2
        exit 1
    fi
) || loud_fail "wg setup --scope local --route pi --model pi:test:fake --yes failed: $(tail -10 "$scratch/scope2.log")"

if [[ ! -f "$proj2/.wg/config.toml" ]]; then
    loud_fail "expected $proj2/.wg/config.toml after --scope local"
fi
if [[ -f "$fake_home/.wg/config.toml" ]] || [[ -f "$fake_home/.wg/config.toml" ]]; then
    loud_fail "--scope local must NOT write a global config; found global config under $fake_home"
fi

# Reset for the next case.
rm -rf "$fake_home/.wg" "$fake_home/.wg" "$proj2"

# ── 3. --scope both writes both ──────────────────────────────────────
proj3="$scratch/proj3"
mkdir -p "$proj3"
(
    cd "$proj3"
    if ! wg setup --scope both --route pi --model pi:test:fake --yes >"$scratch/scope3.log" 2>&1; then
        cat "$scratch/scope3.log" 1>&2
        exit 1
    fi
) || loud_fail "wg setup --scope both --route pi --model pi:test:fake --yes failed: $(tail -10 "$scratch/scope3.log")"

if [[ ! -f "$fake_home/.wg/config.toml" ]]; then
    loud_fail "--scope both did NOT write global config at $fake_home/.wg/config.toml"
fi
if [[ ! -f "$proj3/.wg/config.toml" ]]; then
    loud_fail "--scope both did NOT write local config at $proj3/.wg/config.toml"
fi

# ── 4. --scope garbage refuses with helpful message ──────────────────
if wg setup --scope garbage --route pi --model pi:test:fake --yes >"$scratch/scope4.log" 2>&1; then
    loud_fail "wg setup --scope garbage --yes succeeded; should have refused. Log:\n$(cat "$scratch/scope4.log")"
fi
if ! grep -qE "global.*local.*both|--scope" "$scratch/scope4.log"; then
    loud_fail "invalid --scope error did not mention valid values. Log:\n$(cat "$scratch/scope4.log")"
fi

echo "PASS: --scope global writes only global, --scope local writes only local, --scope both writes both, invalid --scope refused with helpful message, summary prints 'Will write N keys' + 'Will NOT write (built-in defaults): M more'"
exit 0
