#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
GUARD="$ROOT/docs/pi-integration/upstream-patch/output-guard-epipe/output-guard.ts"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

cat >"$TMP/producer.mjs" <<'JS'
import { pathToFileURL } from "node:url";

const guard = await import(pathToFileURL(process.argv[2]).href);
const mode = process.argv[3];
guard.takeOverStdout();
if (mode === "early") {
  guard.writeRawStdout('{"type":"session","version":3}\n');
  for (let i = 0; i < 10000; i += 1) {
    guard.writeRawStdout(`${JSON.stringify({ type: "message_update", index: i })}\n`);
  }
} else {
  guard.writeRawStdout('{"type":"session","version":3}\n');
  guard.writeRawStdout('{"type":"turn_start"}\n');
  guard.writeRawStdout(
    '{"type":"turn_end","message":{"usage":{"input":7,"output":3,"totalTokens":10,"cost":{"total":0.01}}}}\n',
  );
}
await guard.waitForRawStdoutBackpressure();
guard.restoreStdout();
JS

set +e
node --no-warnings --experimental-strip-types "$TMP/producer.mjs" "$GUARD" early \
  2>"$TMP/early.stderr" | head -n 1 >"$TMP/first.json"
statuses=("${PIPESTATUS[@]}")
set -e

[[ "${statuses[0]}" == 0 && "${statuses[1]}" == 0 ]] || {
  echo "FAIL: closed-reader pipeline statuses: ${statuses[*]}" >&2
  cat "$TMP/early.stderr" >&2
  exit 1
}
[[ "$(jq -r .type "$TMP/first.json")" == session ]] || {
  echo "FAIL: first NDJSON event was not delivered" >&2
  exit 1
}
if grep -Eq 'EPIPE|Unhandled|uncaught' "$TMP/early.stderr"; then
  echo "FAIL: closed reader produced an unhandled broken-pipe error" >&2
  cat "$TMP/early.stderr" >&2
  exit 1
fi

node --no-warnings --experimental-strip-types "$TMP/producer.mjs" "$GUARD" full \
  >"$TMP/full.jsonl" 2>"$TMP/full.stderr"
[[ ! -s "$TMP/full.stderr" ]] || {
  echo "FAIL: full consumer emitted stderr" >&2
  cat "$TMP/full.stderr" >&2
  exit 1
}
jq -e -s '
  map(.type) == ["session", "turn_start", "turn_end"] and
  .[2].message.usage == {
    input: 7,
    output: 3,
    totalTokens: 10,
    cost: {total: 0.01}
  }
' "$TMP/full.jsonl" >/dev/null || {
  echo "FAIL: full consumer did not receive ordered NDJSON with usage" >&2
  exit 1
}

echo "PASS: Pi output guard tolerates a closed NDJSON reader and preserves full-stream usage"
