#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
PI=$(command -v pi || true)
if [[ -z "$PI" ]]; then
  echo "SKIP: MISSING PI — install the pinned patched runtime with 'make install-patched-pi'" >&2
  exit 77
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# `wg doctor` may return 1/2 for unrelated optional host checks. Its structured
# Pi check must nevertheless identify the actual PATH runtime as fixed.
set +e
wg --json --dir "$ROOT/.wg" doctor >"$TMP/doctor.json"
set -e
jq -e '
  any(.checks[];
    .name == "Pi output guard" and
    .status == "ok" and
    (.detail | contains("fixed closed-consumer EPIPE handling and bounded retries")))
' "$TMP/doctor.json" >/dev/null || {
  echo "FAIL: wg doctor did not identify the PATH Pi runtime as fixed" >&2
  jq '.checks[] | select(.name == "Pi output guard")' "$TMP/doctor.json" >&2
  exit 1
}

# Real human-facing Pi terminal flow: consume one NDJSON record, close the
# reader, and require Pi itself (not a source snapshot) to finish cleanly.
set +e
timeout 45s "$PI" --mode json -p "Reply with exactly: ok" \
  2>"$TMP/early.stderr" | head -n1 >"$TMP/first.json"
statuses=("${PIPESTATUS[@]}")
set -e
[[ "${statuses[0]}" == 0 && "${statuses[1]}" == 0 ]] || {
  echo "FAIL: installed Pi closed-reader pipeline statuses: ${statuses[*]}" >&2
  cat "$TMP/early.stderr" >&2
  exit 1
}
jq -e '.type == "session"' "$TMP/first.json" >/dev/null || {
  echo "FAIL: installed Pi did not deliver its first session event" >&2
  exit 1
}
if grep -Eqi 'EPIPE|Unhandled|uncaught' "$TMP/early.stderr"; then
  echo "FAIL: installed Pi emitted an unhandled closed-pipe error" >&2
  cat "$TMP/early.stderr" >&2
  exit 1
fi

# A normal reader must still receive Pi's complete event stream and the
# turn-level usage object consumed by WG accounting.
timeout 60s "$PI" --mode json -p "Reply with exactly: ok" \
  >"$TMP/full.jsonl" 2>"$TMP/full.stderr" || {
  echo "FAIL: installed Pi full-consumer run failed" >&2
  cat "$TMP/full.stderr" >&2
  exit 1
}
[[ ! -s "$TMP/full.stderr" ]] || {
  echo "FAIL: installed Pi full-consumer run emitted stderr" >&2
  cat "$TMP/full.stderr" >&2
  exit 1
}
jq -e -s '
  (map(.type) | index("session") < index("turn_start")) and
  (map(.type) | index("turn_start") < index("turn_end")) and
  any(.[]; .type == "turn_end" and (.message.usage.totalTokens // 0) > 0)
' "$TMP/full.jsonl" >/dev/null || {
  echo "FAIL: installed Pi full consumer missed ordered events or non-zero usage" >&2
  exit 1
}

# The WG worker and RPC transports resolve this same executable through PATH.
# Pin both production call sites so a future hard-coded alternate runtime
# cannot silently bypass the doctor-inspected package.
grep -q 'Command::new("pi")' "$ROOT/src/commands/spawn_task.rs" || {
  echo "FAIL: human Pi chat path no longer resolves the doctor-inspected PATH runtime" >&2
  exit 1
}
grep -q 'pi_binary.*avail.*pi_binary\|\.pi_binary' "$ROOT/src/commands/pi_handler.rs" || {
  echo "FAIL: hermetic wg pi-handler no longer uses discovered PATH Pi" >&2
  exit 1
}
grep -q 'command: "pi".to_string()' "$ROOT/src/service/executor.rs" || {
  echo "FAIL: WG Pi JSON worker no longer uses the PATH Pi runtime" >&2
  exit 1
}

echo "PASS: installed Pi is EPIPE-safe, full NDJSON carries usage, and every WG Pi launch surface resolves that runtime"
