#!/usr/bin/env bash
# Scenario: agency_csv_roundtrip
#
# Pins Agency starter CSV import/export as a byte-equal 12-column round trip.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

repo_root="$(cd "$HERE/../../.." && pwd)"
fixture="$repo_root/tests/fixtures/agency-starter-sample.csv"
scratch=$(make_scratch)
wg_dir="$scratch/.wg"
mkdir -p "$wg_dir"

if ! wg --dir "$wg_dir" agency import --format agency-csv "$fixture" >import.log 2>&1; then
    loud_fail "wg agency import --format agency-csv failed:\n$(cat import.log)"
fi

if ! wg --dir "$wg_dir" agency export --format agency-csv "$scratch/export.csv" >export.log 2>&1; then
    loud_fail "wg agency export --format agency-csv failed:\n$(cat export.log)"
fi

if ! diff -u "$fixture" "$scratch/export.csv" >diff.log 2>&1; then
    loud_fail "agency CSV roundtrip was not byte-equal:\n$(cat diff.log)"
fi

echo "PASS: agency CSV import/export roundtrip is byte-equal"
exit 0
