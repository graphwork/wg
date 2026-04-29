#!/usr/bin/env bash
# Scenario: wg_html_chat
#
# Regression for wg-html-chat: `wg html --chat` must
#   1. Render chat transcripts on `.chat-N` task pages (Conversation section),
#      respecting `visibility = public` by default.
#   2. With `--all`, include non-public chat transcripts as well.
#   3. With `--public-only`, drop non-public chat tasks entirely (the page
#      doesn't even exist).
#   4. Default `wg html` (no --chat) must NOT include any transcript content,
#      but the chat task node still appears in the task list.
#   5. Best-effort sanitization redacts api-key shapes / OPENAI_API_KEY=...
#      / ~/.wg/secrets paths.
#   6. The index header reports the transcript counts when --chat is active.
#
# No daemon, no LLM — pure graph + chat-fixture manipulation + `wg html`.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

if ! wg init -m claude:opus >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -5 init.log)"
fi

# ── Build a graph with two chat tasks (one public, one internal) ──────────────
# We add the chat tasks via direct graph manipulation so we don't have to spin
# up the dispatcher / chat handler. The chat fixtures (sessions registry +
# inbox/outbox JSONL) we also create directly.
graph_file=""
chat_root=""
for cand in ".workgraph" ".wg"; do
    if [[ -f "$cand/graph.jsonl" ]]; then
        graph_file="$cand/graph.jsonl"
        chat_root="$cand/chat"
        break
    fi
done
if [[ -z "$graph_file" ]]; then
    loud_fail "neither .workgraph/graph.jsonl nor .wg/graph.jsonl exists after wg init"
fi
mkdir -p "$chat_root"

# Append two chat tasks with distinct visibilities to the graph.
cat >> "$graph_file" <<'JSONL'
{"kind":"task","id":".chat-pub","title":"Chat Public","description":"public chat","status":"open","priority":50,"tags":["chat-loop"],"visibility":"public","created_at":"2026-04-29T12:00:00+00:00"}
{"kind":"task","id":".chat-int","title":"Chat Internal","description":"internal chat","status":"open","priority":50,"tags":["chat-loop"],"visibility":"internal","created_at":"2026-04-29T12:00:00+00:00"}
JSONL

# Build sessions.json mapping aliases -> uuid dirs.
pub_uuid="00000000-0000-7000-8000-00000000aaaa"
int_uuid="00000000-0000-7000-8000-00000000bbbb"
mkdir -p "$chat_root/$pub_uuid" "$chat_root/$int_uuid"

cat > "$chat_root/sessions.json" <<JSON
{
  "version": 0,
  "sessions": {
    "$pub_uuid": {
      "kind": "coordinator",
      "created": "2026-04-29T12:00:00+00:00",
      "aliases": ["chat-pub"],
      "label": "smoke pub"
    },
    "$int_uuid": {
      "kind": "coordinator",
      "created": "2026-04-29T12:00:00+00:00",
      "aliases": ["chat-int"],
      "label": "smoke int"
    }
  }
}
JSON

# Seed inbox/outbox messages. We embed three secret patterns to verify
# the sanitizer fires on rendering.
cat > "$chat_root/$pub_uuid/inbox.jsonl" <<'JSONL'
{"id":1,"timestamp":"2026-04-29T12:00:01+00:00","role":"user","content":"please use sk-ABCDEFGHIJKLMNOPQRSTUVWXYZ12345 with OPENAI_API_KEY=hunter2","request_id":"r1"}
{"id":2,"timestamp":"2026-04-29T12:00:03+00:00","role":"user","content":"see ~/.wg/secrets/openai.key for the credential","request_id":"r2"}
JSONL
cat > "$chat_root/$pub_uuid/outbox.jsonl" <<'JSONL'
{"id":1,"timestamp":"2026-04-29T12:00:02+00:00","role":"coordinator","content":"acknowledged, will not echo the key","request_id":"r1"}
JSONL

cat > "$chat_root/$int_uuid/inbox.jsonl" <<'JSONL'
{"id":1,"timestamp":"2026-04-29T12:00:01+00:00","role":"user","content":"internal-only conversation snippet","request_id":"r1"}
JSONL
cat > "$chat_root/$int_uuid/outbox.jsonl" <<'JSONL'
{"id":1,"timestamp":"2026-04-29T12:00:02+00:00","role":"coordinator","content":"got it","request_id":"r1"}
JSONL

# ── Test 1: default `wg html` (no --chat) — task node yes, transcript no ─────
default_dir=$(mktemp -d "$scratch/html-default.XXXXXX")
if ! wg html --out "$default_dir" >default.log 2>&1; then
    loud_fail "wg html (default) failed: $(cat default.log)"
fi
if [[ ! -f "$default_dir/tasks/.chat-pub.html" ]]; then
    loud_fail "default wg html should still render chat task page"
fi
if grep -q "Conversation" "$default_dir/tasks/.chat-pub.html"; then
    loud_fail "default wg html (no --chat) leaked Conversation section: $(grep -A 2 Conversation "$default_dir/tasks/.chat-pub.html" | head -10)"
fi
if grep -q "acknowledged, will not echo" "$default_dir/tasks/.chat-pub.html"; then
    loud_fail "default wg html leaked transcript content"
fi
echo "PASS (1/5): default wg html omits chat transcripts"

# ── Test 2: --chat — public transcripts in, internal hidden behind notice ────
chat_dir=$(mktemp -d "$scratch/html-chat.XXXXXX")
if ! wg html --chat --out "$chat_dir" >chat.log 2>&1; then
    loud_fail "wg html --chat failed: $(cat chat.log)"
fi
if ! grep -q "Conversation" "$chat_dir/tasks/.chat-pub.html"; then
    loud_fail "wg html --chat: public chat page missing Conversation section"
fi
if ! grep -q "acknowledged, will not echo" "$chat_dir/tasks/.chat-pub.html"; then
    loud_fail "wg html --chat: public transcript content missing"
fi
# Internal page exists but transcript is hidden.
if [[ ! -f "$chat_dir/tasks/.chat-int.html" ]]; then
    loud_fail "wg html --chat: internal chat task page should still be rendered"
fi
if ! grep -q "Chat transcript hidden" "$chat_dir/tasks/.chat-int.html"; then
    loud_fail "wg html --chat: internal chat page missing 'Chat transcript hidden' notice"
fi
if grep -q "internal-only conversation snippet" "$chat_dir/tasks/.chat-int.html"; then
    loud_fail "wg html --chat: internal transcript content leaked"
fi
# Index header banner.
if ! grep -qE "Showing 1 chat transcript" "$chat_dir/index.html"; then
    loud_fail "wg html --chat: index header banner missing transcript count"
fi
echo "PASS (2/5): --chat includes public transcripts only and shows hidden marker for internal"

# ── Test 3: --chat --all — internal transcript content rendered ──────────────
all_dir=$(mktemp -d "$scratch/html-all.XXXXXX")
if ! wg html --chat --all --out "$all_dir" >all.log 2>&1; then
    loud_fail "wg html --chat --all failed: $(cat all.log)"
fi
if ! grep -q "internal-only conversation snippet" "$all_dir/tasks/.chat-int.html"; then
    loud_fail "wg html --chat --all: internal transcript content missing"
fi
if grep -q "Chat transcript hidden" "$all_dir/tasks/.chat-int.html"; then
    loud_fail "wg html --chat --all: hidden marker should not appear"
fi
echo "PASS (3/5): --chat --all includes non-public transcripts"

# ── Test 4: --chat --public-only — internal chat task page absent ────────────
po_dir=$(mktemp -d "$scratch/html-po.XXXXXX")
if ! wg html --chat --public-only --out "$po_dir" >po.log 2>&1; then
    loud_fail "wg html --chat --public-only failed: $(cat po.log)"
fi
if [[ -f "$po_dir/tasks/.chat-int.html" ]]; then
    loud_fail "wg html --chat --public-only: internal chat page should NOT be rendered"
fi
if ! grep -q "Conversation" "$po_dir/tasks/.chat-pub.html"; then
    loud_fail "wg html --chat --public-only: public chat page missing Conversation"
fi
echo "PASS (4/5): --chat --public-only filters internal chat tasks"

# ── Test 5: secret sanitization in the rendered transcript ───────────────────
if grep -q "sk-ABCDEFGHIJKLMNOPQRSTUVWXYZ12345" "$chat_dir/tasks/.chat-pub.html"; then
    loud_fail "sanitizer did not redact sk- key"
fi
if grep -qE "OPENAI_API_KEY=[^[]" "$chat_dir/tasks/.chat-pub.html"; then
    loud_fail "sanitizer did not redact OPENAI_API_KEY=..."
fi
if grep -q "secrets/openai.key" "$chat_dir/tasks/.chat-pub.html"; then
    loud_fail "sanitizer did not redact ~/.wg/secrets/... path"
fi
if ! grep -q '\[redacted\]' "$chat_dir/tasks/.chat-pub.html"; then
    loud_fail "expected [redacted] marker in rendered transcript"
fi
echo "PASS (5/5): sanitizer redacts api-key, env-var assignment, and secrets path"

echo "PASS: wg_html_chat — all assertions passed"
exit 0
