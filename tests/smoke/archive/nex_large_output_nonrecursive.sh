#!/usr/bin/env bash
# Scenario: nex_large_output_nonrecursive
#
# Pins fix-nex-large without requiring a live model endpoint:
# - bash returns complete large output to the native channeler
# - context-aware channeling stores full output and returns metadata + preview
# - `cat <tool-output-artifact>` produces a bounded preview without recursively
#   creating another routed artifact

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

repo_root=""
candidate="$HERE"
for _ in 1 2 3 4 5 6; do
    if [[ -f "$candidate/Cargo.toml" ]]; then
        repo_root="$candidate"
        break
    fi
    candidate="$(dirname "$candidate")"
done
if [[ -z "$repo_root" ]]; then
    loud_fail "could not find Cargo.toml above $HERE"
fi

cd "$repo_root"

scratch=$(make_scratch)
log="$scratch/cargo.log"

tests=(
    test_large_output_is_channeled
    test_cat_of_artifact_is_non_recursive_preview
    test_small_output_passes_through
    test_default_threshold_spends_more_than_4kb
    test_threshold_scales_with_context_window
    bash_returns_large_output_untruncated_for_channeler
)

for test_name in "${tests[@]}"; do
    if ! cargo test --lib "$test_name" >"$log" 2>&1; then
        loud_fail "nex large-output regression test $test_name failed:
$(tail -80 "$log")"
    fi
    if ! grep -qE "::${test_name} \.\.\. ok" "$log"; then
        loud_fail "regression test $test_name did not report ok:
$(cat "$log")"
    fi
done

echo "PASS: nex large-output channeling is context-aware and artifact reads are non-recursive"
exit 0
