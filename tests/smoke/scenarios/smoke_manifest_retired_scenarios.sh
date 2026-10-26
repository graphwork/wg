#!/usr/bin/env bash
# Scenario: smoke_manifest_retired_scenarios (task archive-retired-smokes)
#
# Pins the smoke-corpus retirement contract (reduction plan C1.2):
#
#   1. PRESERVED, NOT DELETED — every `retired = true` manifest entry keeps its
#      owners, carries a `retired_reason`, and points at a script that exists
#      under `tests/smoke/archive/` and is TRACKED BY GIT. Nothing is removed.
#   2. DEFAULT GATE IGNORES RETIRED — `wg done <task>` never RUNS a retired
#      scenario even when the task id is still in its `owners`, and it says so
#      loudly (naming the skipped scenario + the opt-in command) instead of
#      silently narrowing coverage.
#   3. THE GATE STILL GATES — a non-retired owned failing scenario still blocks
#      `wg done`, and the retired sibling is still not run.
#   4. `--full-smoke` OPTS THEM BACK IN — `wg done <task> --full-smoke` runs the
#      retired scenario and fails on it.
#   5. ARCHIVE LAYOUT IS LIVE — the retired scenario that `--full-smoke` runs
#      sources its same-directory `_helpers.sh` shim successfully, proving the
#      moved corpus is still executable rather than a broken path reference.
#
# Credential-free: no endpoint and no model call. Flip review / auto-eval are
# disabled in the scratch config, so the only authority under test is the gate.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

repo_root="$(cd "$HERE/../../.." && pwd)"
manifest="$repo_root/tests/smoke/manifest.toml"
archive_dir="$repo_root/tests/smoke/archive"
scenarios_dir="$repo_root/tests/smoke/scenarios"

[ -f "$manifest" ] || loud_fail "smoke manifest missing: $manifest"
[ -d "$archive_dir" ] || loud_fail "smoke archive directory missing: $archive_dir"

# ── Candidate binary (never cargo-install; use the running candidate) ──
if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" ]]; then
    WG_BIN="$WG_SMOKE_CANDIDATE_BIN"
elif [[ -x "$repo_root/target/debug/wg" ]]; then
    WG_BIN="$repo_root/target/debug/wg"
else
    WG_BIN="$(command -v wg)"
fi
[ -x "$WG_BIN" ] || loud_fail "candidate wg binary not found"
export PATH="$(dirname "$WG_BIN"):$PATH"

scratch=$(make_scratch)

# Isolate HOME/global config so no host route leaks into the fixture.
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
export WG_GLOBAL_DIR="$HOME/.wg"
mkdir -p "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR"
logs="$scratch/logs"
mkdir -p "$logs"

# ════════════════════════════════════════════════════════════════════════
# 1. Static archive invariants over the real checked-in manifest
# ════════════════════════════════════════════════════════════════════════
rows="$scratch/retired-rows.tsv"
awk '
    /^\[\[scenario\]\]$/ { if (retired) print name "\t" script; name=""; script=""; retired=0 }
    /^name = "/          { if (name=="") { v=$0; sub(/^name = "/,"",v); sub(/"$/,"",v); name=v } }
    /^script = "/        { v=$0; sub(/^script = "/,"",v); sub(/"$/,"",v); script=v }
    /^retired = true$/   { retired=1 }
    END                  { if (retired) print name "\t" script }
' "$manifest" >"$rows"

retired_count=$(wc -l <"$rows" | tr -d ' ')
total_count=$(grep -c '^\[\[scenario\]\]$' "$manifest")
active_count=$((total_count - retired_count))

[ "$retired_count" -ge 50 ] \
    || loud_fail "expected a substantial retired corpus, found ${retired_count} (total ${total_count})"
[ "$active_count" -ge 1 ] \
    || loud_fail "retirement must not empty the default gate: total=${total_count} retired=${retired_count}"

# The reduction plan's named spark planes must be in the retired set.
for anchored in federation_spark_two_graphs exec_spark_borrowed_box \
    content_safety_spark e2e_family_team pilot_dry_run; do
    awk -F'\t' -v n="$anchored" '$1==n { found=1 } END { exit(found ? 0 : 1) }' "$rows" \
        || loud_fail "spark plane scenario '${anchored}' is not retired"
done

bad_reason=0
bad_script=0
still_active=0
untracked=0
while IFS=$'\t' read -r name script; do
    [ -n "$name" ] || continue
    awk -v want="$name" '
        /^\[\[scenario\]\]$/ { target=0 }
        index($0, "name = \"" want "\"") == 1 { target=1 }
        target && /^retired_reason = ".+"/ { found=1 }
        END { exit(found ? 0 : 1) }
    ' "$manifest" || {
        bad_reason=$((bad_reason + 1))
        echo "missing retired_reason: $name" >&2
    }

    case "$script" in
        archive/*)
            # Exists AND is git-tracked: preserved, never deleted.
            if [ ! -f "$repo_root/tests/smoke/$script" ]; then
                bad_script=$((bad_script + 1))
                echo "archived script missing for '${name}': $script" >&2
            elif ! git -C "$repo_root" ls-files --error-unmatch "tests/smoke/$script" \
                >/dev/null 2>&1; then
                untracked=$((untracked + 1))
                echo "archived script not git-tracked for '${name}': $script" >&2
            fi
            if [ -f "$scenarios_dir/$(basename "$script")" ]; then
                still_active=$((still_active + 1))
                echo "'${name}' script still present in scenarios/ as well as the archive" >&2
            fi
            bash -n "$repo_root/tests/smoke/$script" \
                || loud_fail "archived script has a shell syntax error: $script"
            ;;
        *)
            # The one documented exception: a script owned by another harness
            # (tests/install). It must still exist at its manifest path.
            [ -f "$repo_root/tests/smoke/$script" ] \
                || loud_fail "non-archived retired script missing for '${name}': $script"
            ;;
    esac
done <"$rows"

[ "$bad_reason" -eq 0 ] || loud_fail "${bad_reason} retired scenario(s) missing retired_reason"
[ "$bad_script" -eq 0 ] || loud_fail "${bad_script} retired scenario(s) lost their archived script"
[ "$untracked" -eq 0 ] || loud_fail "${untracked} archived script(s) not git-tracked (deleted?)"
[ "$still_active" -eq 0 ] || loud_fail "${still_active} retired script(s) still present in scenarios/"

# The archive helper shim must forward to the single live helper implementation.
[ -f "$archive_dir/_helpers.sh" ] || loud_fail "archive/_helpers.sh shim missing"
grep -q 'scenarios" && pwd)/_helpers.sh' "$archive_dir/_helpers.sh" \
    || loud_fail "archive/_helpers.sh does not forward to scenarios/_helpers.sh"

echo "STATIC OK: ${retired_count}/${total_count} scenarios retired (${active_count} active), archived + git-tracked"

# ════════════════════════════════════════════════════════════════════════
# 2-5. Behavioural proofs through the real `wg done` gate
# ════════════════════════════════════════════════════════════════════════
project="$scratch/project"
mkdir -p "$project"
cd "$project" || loud_fail "cannot enter scratch project"

git init -q -b main . || loud_fail "git init failed"
git config user.email "smoke@test"
git config user.name "Smoke"
echo "fixture" >README.md
git add README.md >/dev/null
git commit -qm "fixture base" >/dev/null || loud_fail "git initial commit failed"

wg init --route pi >"$logs/init.log" 2>&1 \
    || loud_fail "wg init failed: $(tail -5 "$logs/init.log")"
# No model authority in this fixture: the smoke gate is the only authority
# under test, so disable flip review / auto-eval.
wg config set agency.auto_evaluate false >/dev/null 2>&1 || true
wg config set agency.auto_assign false >/dev/null 2>&1 || true
wg config set agency.flip_enabled false >/dev/null 2>&1 || true
wg config set agency.completion_review_strict false >/dev/null 2>&1 || true
git add -A >/dev/null
git commit -qm "wg init" >/dev/null || loud_fail "git commit (wg init) failed"

mkdir -p tests/smoke/scenarios tests/smoke/archive
# The archived fixture sources its same-directory helper shim exactly like a
# real moved scenario — this is what proves the archive layout is executable.
cp "$archive_dir/_helpers.sh" tests/smoke/archive/_helpers.sh \
    || loud_fail "cannot copy archive helper shim into fixture"
cp "$scenarios_dir/_helpers.sh" tests/smoke/scenarios/_helpers.sh \
    || loud_fail "cannot copy live helper into fixture"

retired_marker="$scratch/retired-ran.marker"
live_marker="$scratch/live-ran.marker"
rm -f "$retired_marker" "$live_marker"

cat >tests/smoke/archive/retired_fixture.sh <<EOF
#!/usr/bin/env bash
HERE="\$(cd "\$(dirname "\$0")" && pwd)"
. "\$HERE/_helpers.sh"
touch "$retired_marker"
loud_fail "retired fixture scenario must only run under --full-smoke"
EOF
cat >tests/smoke/scenarios/live_fixture.sh <<EOF
#!/usr/bin/env bash
HERE="\$(cd "\$(dirname "\$0")" && pwd)"
. "\$HERE/_helpers.sh"
touch "$live_marker"
loud_fail "live fixture scenario must block the default gate"
EOF

retired_block() { # $1=owner
    cat <<EOF
[[scenario]]
name = "retired_fixture"
script = "archive/retired_fixture.sh"
owners = ["$1"]
description = "archived fixture proving retired scenarios are preserved and opt-in only"
retired = true
retired_reason = "fixture: retired scenario, archived under tests/smoke/archive/"
timeout_seconds = 60
EOF
}
live_block() { # $1=owner
    cat <<EOF
[[scenario]]
name = "live_fixture"
script = "scenarios/live_fixture.sh"
owners = ["$1"]
description = "live fixture proving the default gate still blocks a non-retired failure"
timeout_seconds = 60
EOF
}

write_manifest() { # $1=retired owner, $2=live owner ("" omits the live entry)
    retired_block "$1" >tests/smoke/manifest.toml
    if [ -n "${2:-}" ]; then
        echo "" >>tests/smoke/manifest.toml
        live_block "$2" >>tests/smoke/manifest.toml
    fi
    git add -A >/dev/null
    git commit -qm "fixture manifest" >/dev/null
}

add_task() { # $1=task-id
    wg add "fixture task $1" --id "$1" >"$logs/add-$1.log" 2>&1 \
        || loud_fail "wg add $1 failed: $(tail -5 "$logs/add-$1.log")"
    wg resume "$1" >/dev/null 2>&1 || true
    wg claim "$1" >"$logs/claim-$1.log" 2>&1 \
        || loud_fail "wg claim $1 failed: $(tail -5 "$logs/claim-$1.log")"
}

# ── Proof 2: default gate skips an owned retired scenario and says so ──
write_manifest "t-skip" ""
add_task t-skip
wg done t-skip >"$logs/done-t-skip.log" 2>&1 || true

grep -q "Smoke gate: 1 retired scenario(s) owned by 't-skip' skipped" \
    "$logs/done-t-skip.log" \
    || loud_fail "default gate did not report the retired-scenario skip:\n$(tail -30 "$logs/done-t-skip.log")"
grep -q "retired_fixture" "$logs/done-t-skip.log" \
    || loud_fail "retired skip note did not name the skipped scenario"
grep -q "wg done t-skip --full-smoke" "$logs/done-t-skip.log" \
    || loud_fail "retired skip note did not offer the --full-smoke opt-in"
grep -q "SMOKE FAILED" "$logs/done-t-skip.log" \
    && loud_fail "the retired scenario was RUN by the default gate:\n$(tail -40 "$logs/done-t-skip.log")"
[ -f "$retired_marker" ] \
    && loud_fail "retired scenario executed under the default gate (marker present)"
echo "PROOF 2 OK: default gate skipped and named the owned retired scenario"

# ── Proof 3: the gate still blocks a live owned failure ────────────────
write_manifest "t-block" "t-block"
add_task t-block
wg done t-block >"$logs/done-t-block.log" 2>&1 || true

grep -q "Smoke gate refused 'wg done t-block': 1 scenario(s) broken — live_fixture" \
    "$logs/done-t-block.log" \
    || loud_fail "default gate did not block the live failing scenario:\n$(tail -40 "$logs/done-t-block.log")"
grep -q "retired_fixture (exit" "$logs/done-t-block.log" \
    && loud_fail "the retired scenario was reported as an executed failure"
[ -f "$retired_marker" ] \
    && loud_fail "retired scenario executed while the default gate blocked on the live one"
[ -f "$live_marker" ] \
    || loud_fail "live fixture did not execute; the block was not the gate's"
echo "PROOF 3 OK: default gate still blocks a non-retired failure; the retired sibling stayed unrun"

# ── Proof 4+5: --full-smoke opts the archived scenario back in ─────────
write_manifest "t-full" ""
add_task t-full
rm -f "$retired_marker" "$live_marker"
wg done t-full --full-smoke >"$logs/done-t-full.log" 2>&1 || true

grep -q -- "--full-smoke opted 1 retired scenario(s) back in" \
    "$logs/done-t-full.log" \
    || loud_fail "--full-smoke did not announce the retired opt-in:\n$(tail -40 "$logs/done-t-full.log")"
grep -q "Smoke gate refused 'wg done t-full'" "$logs/done-t-full.log" \
    || loud_fail "--full-smoke did not fail on the retired scenario:\n$(tail -40 "$logs/done-t-full.log")"
grep -q "retired_fixture" "$logs/done-t-full.log" \
    || loud_fail "--full-smoke did not name the retired scenario in its report"
[ -f "$retired_marker" ] \
    || loud_fail "--full-smoke did not execute the archived script (no marker)"
[ ! -f "$live_marker" ] \
    || loud_fail "the un-owned live fixture ran under --full-smoke"
echo "PROOF 4+5 OK: --full-smoke ran the archived retired scenario through its helper shim"

echo "PASS: retired scenarios are preserved, ignored by the default gate, and opt-in via --full-smoke"
