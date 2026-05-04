#!/usr/bin/env bash
# Scenario: agency_import_dedup_collision
#
# Pins the dedup decision documented in docs/manual/03-agency.md
# "Import Dedup Rule": one row per content_hash = sha256(description).
# When upstream rows collide on description, default mode warns + skips
# subsequent rows (first-write-wins) and `--strict` errors.
#
# Regression cover for investigate-agency-import: the previous behavior
# silently overwrote earlier rows (last-write-wins), masking semantic
# drift like per-scope variants and same-description name collisions.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
wg_dir="$scratch/.wg"
mkdir -p "$wg_dir"

csv="$scratch/dedup-collision.csv"
cat > "$csv" <<'EOF'
type,name,description,quality,domain_specificity,domain,origin_instance_id,parent_content_hash,scope
role_component,forward-compatible-deferral-spec,Defer the choice when forward compatibility matters,80,2,management,inst-001,,task
role_component,forward-compatible-deferral-spec,Defer the choice when forward compatibility matters,80,2,management,inst-001,,meta:assigner
role_component,identify-write-up-audience,Adapt the synthesis to non-domain audience members,75,2,research,inst-002,,task
role_component,adapt-research-synthesis,Adapt the synthesis to non-domain audience members,90,2,research,inst-003,,task
EOF

# --- Default mode: should succeed and warn-and-skip on each collision.
if ! wg --dir "$wg_dir" agency import "$csv" >"$scratch/import.log" 2>"$scratch/import.err"; then
    loud_fail "default-mode import failed unexpectedly:\nstdout:\n$(cat "$scratch/import.log")\nstderr:\n$(cat "$scratch/import.err")"
fi

if ! grep -q "agency import collision (row 2)" "$scratch/import.err"; then
    loud_fail "expected per-scope variant collision warning on row 2:\n$(cat "$scratch/import.err")"
fi
if ! grep -q "agency import collision (row 4)" "$scratch/import.err"; then
    loud_fail "expected same-description name collision warning on row 4:\n$(cat "$scratch/import.err")"
fi

# Exactly two component files: one per distinct description.
comp_count=$(find "$wg_dir/agency/primitives/components" -maxdepth 1 -name '*.yaml' | wc -l | tr -d ' ')
if [ "$comp_count" != "2" ]; then
    loud_fail "expected 2 components after warn-and-skip dedup; got $comp_count"
fi

# First-write-wins: the saved description-collision row should be the seeded one,
# not the upstream-style one.
if ! grep -lq "name: identify-write-up-audience" "$wg_dir/agency/primitives/components/"*.yaml; then
    loud_fail "first-write-wins violated — earlier row 'identify-write-up-audience' was overwritten"
fi
if grep -lq "name: adapt-research-synthesis" "$wg_dir/agency/primitives/components/"*.yaml; then
    loud_fail "first-write-wins violated — later row 'adapt-research-synthesis' should have been skipped"
fi

# --- Strict mode: should fail on the first collision in a fresh scratch.
strict_scratch=$(make_scratch)
strict_wg="$strict_scratch/.wg"
mkdir -p "$strict_wg"
if wg --dir "$strict_wg" agency import "$csv" --strict >"$strict_scratch/strict.log" 2>"$strict_scratch/strict.err"; then
    loud_fail "--strict should have failed on collision but exited 0:\nstdout:\n$(cat "$strict_scratch/strict.log")\nstderr:\n$(cat "$strict_scratch/strict.err")"
fi
if ! grep -q "agency import --strict" "$strict_scratch/strict.err"; then
    loud_fail "--strict error did not mention --strict in its message:\n$(cat "$strict_scratch/strict.err")"
fi

echo "PASS: agency import dedup collisions are surfaced (warn-and-skip default, --strict errors)"
exit 0
