# nex fuzzy / robust file editing — design & chosen-approach rationale

**Task:** `add-fuzzy-robust` — *Add fuzzy/robust file editing to nex (kill full-file rewrite thrash).*
**Date:** 2026-06-18
**Builds on:** `audit-nex-tool` (`docs/research/nex-tool-framework-compat-audit.md`),
which established that `edit_file` is the canonical, snake_case str-replace
tool in nex's surface (not a Claude-Code `Edit` clone), and that the tool
schema is re-sent in prefill every turn (so adding a *second* edit tool is not
free).

---

## 1. The problem (observed)

Watching nex (qwen3.6-35b-a3b) build a Game-of-Life TUI: after each
`cargo build` error the model **rewrote the whole file** with `write_file`
repeatedly — `main.rs` went 2.3KB → 17KB → 19.5KB → about to rewrite again —
instead of making targeted edits.

Root cause is the classic failure mode of a strict **exact-match** edit tool:

- `edit_file` required `old_string` to appear **byte-for-byte**. Its only
  leniency was two **opt-in** flags (`normalize_whitespace`,
  `normalize_line_endings`) that weak/local models essentially never set.
- When the model's `old_string` was slightly off — indentation it eyeballed
  wrong, a stray trailing space, a CRLF/LF mismatch, a missing trailing
  newline — the edit failed with a bare `old_string not found ... Make sure the
  string matches exactly.`
- With no path forward, the model fell back to `write_file` and regenerated the
  entire file. This is wasteful (tokens + latency), error-prone (re-introduces
  bugs it had already fixed), and disproportionately hurts local/weaker models,
  which are the least able to reproduce an exact long string.

---

## 2. Approaches considered

| Approach | What it is | Verdict |
|---|---|---|
| **A. Fuzzy str-replace** (chosen) | Keep the single `edit_file` tool; make its matching whitespace/indentation/line-ending tolerant with near-miss diagnostics. | **Chosen.** Highest leverage, lowest risk. |
| **B. OpenAI `apply_patch` (V4A envelope)** | A separate tool taking a `*** Begin Patch / *** Update File / @@ context / -old / +new` envelope. Used by Codex. | Deferred. Adds a second wire format + parser; the V4A context-matching is itself fuzzy, so it solves the *same* problem A does but at the cost of a new tool schema in every prefill and a new failure surface (malformed envelopes). |
| **C. Aider-style search/replace blocks** | `<<<<<<< SEARCH / ======= / >>>>>>> REPLACE` fenced blocks with fuzzy context matching. | Deferred. Same trade-off as B. Its key insight — *fuzzy context + re-indentation of the replacement* — is exactly what A adopts internally, without a new block grammar the model must emit correctly. |
| **D. Fast-apply / Morph-style second model** | A small dedicated "apply" model rewrites the file from a loose edit description. | Out of scope as a dependency. Powerful but requires a second model + endpoint; noted here as a *future option*, not a requirement. The task explicitly says "do not require a second model." |

### Why A over B/C

1. **Single tool, no new wire format.** `edit_file` already exists, is audited,
   and is in the minimal-tools allowlist. Making it tolerant changes behavior
   the model already knows how to call. B and C each add a tool whose *schema is
   re-sent every turn* (the audit's prefill-cost finding) and whose envelope
   grammar is one more thing a weak model can get wrong — trading an
   exact-`old_string` failure for an exact-envelope failure.
2. **The hard part is matching, not framing.** Aider/Codex get their robustness
   from fuzzy *context matching* and *re-indentation*, not from the envelope
   syntax. A implements those directly (see §3), so we capture the benefit
   without the grammar.
3. **Backward compatible.** Existing callers, tests, and the
   `read_file → edit_file` loop keep working; the two old flags still parse.
4. **Streaming/latency friendly, no second model.** Pure local string
   algorithm; no extra round-trip.

B/C remain reasonable *future* additions if a harness needs a multi-file patch
envelope, but they are not needed to kill the rewrite thrash.

---

## 3. What was implemented

Code: `src/executor/native/tools/fuzzy_match.rs` (matcher + unit tests),
wired into `EditFileTool` in `src/executor/native/tools/file.rs`.

### 3.1 Strictness cascade (strict → loose; first level with exactly one match wins)

1. **Exact substring** — byte-for-byte. Preserves all historical within-line
   edit semantics and is the fast path.
2. **Line-ending tolerant** — `\n` ≡ `\r\n`; a final line with/without a
   trailing newline matches. (Line-based from here down.)
3. **+ Trailing-whitespace tolerant** — ignore trailing spaces/tabs per line.
4. **+ Indentation tolerant** — ignore *leading* whitespace per line. The
   replacement is **re-indented** to the file's actual indentation (see §3.2),
   so a block the model supplied at the wrong indent still lands correctly
   formatted.
5. **+ Interior-whitespace collapse** — **opt-in only** via the existing
   `normalize_whitespace` flag. Off by default because collapsing interior
   whitespace (`a  b` ≡ `a b`) can match semantically different code; kept as a
   power-user escape hatch.

If a level yields **more than one** match, the tool reports an
*ambiguous* error (asking for more context) rather than guessing. Uniqueness is
required at whatever level first matches.

### 3.2 Re-indentation

When a match is found only after ignoring leading whitespace, the replacement
is re-anchored: the leading-whitespace delta between `old_string`'s first
non-blank line and the matched file line is applied to every non-blank line of
`new_string`. This means "model wrote the block at the wrong indent, and wrote
`new_string` at that same wrong indent" still produces correctly-indented file
content. Blank lines are left untouched; divergent indent kinds (tabs vs
spaces) degrade gracefully.

### 3.3 Near-miss diagnostics

On no match at any enabled level, instead of a bare "not found" the tool scans
the file for the **closest line window** (most trim-equal lines, tie-broken by
common-prefix length) and returns a side-by-side comparison of the requested
`old_string` vs the file's actual lines, with the line numbers and a `✗` marker
on differing lines. The message explicitly steers the model to **fix
`old_string` and retry a targeted edit rather than rewrite the file with
`write_file`**. Output is capped (30 lines, 120 cols/line, char-boundary safe).

### 3.4 Steering toward targeted edits

- `edit_file` description now opens with "PREFERRED way to change an existing
  file: make a small, targeted string replacement instead of rewriting the
  whole file," documents the tolerant matching, and says "After a failed build,
  fix each error with a focused edit_file call."
- `write_file` description now says it is "Best for CREATING a new file or a
  full rewrite. To CHANGE an existing file, prefer edit_file ... repeatedly
  rewriting a file to fix small errors is wasteful and reintroduces bugs."
- The default system prompt (`build_default_system_prompt` in
  `src/commands/nex.rs`) gained an "Editing files" paragraph telling the model
  to prefer targeted `edit_file` edits, that matching is tolerant, and to fix
  each build error with a focused edit instead of regenerating the file.

---

## 4. Tests

- `src/executor/native/tools/fuzzy_match.rs` `#[cfg(test)]`: exact /
  trailing-ws / line-ending / indentation (+ re-indent) / collapse-opt-in /
  ambiguity (exact + fuzzy) / near-miss / deletion / unicode. 22 cases.
- `tests/test_edit_file_edge_cases.rs`: updated from the old strict contract to
  the tolerant contract (the `*_fails_*` cases that documented the *bug* are now
  `*_tolerates_*` cases asserting the fix + correct resulting content), plus the
  deliberate strict boundary (`interior_whitespace_not_collapsed_by_default`)
  and its opt-in counterpart.
- `tests/test_edit_file_fix_build_repro.rs`: the headline repro — a "fix build
  errors" flow where each fix is a targeted `edit_file` with an imperfect
  `old_string` (wrong indent / trailing space / CRLF), all of which now land
  without a single `write_file` rewrite; plus a near-miss assertion.

### Live-model repro (executed)

Ran the real flow against **qwen via OpenRouter** (`openrouter:qwen/qwen3-coder`,
which OpenRouter resolved to `qwen3-coder-480b-a35b`) on a throwaway cargo
project whose `src/main.rs` had a deliberate build error (missing `;`):

```
wg nex -m openrouter:qwen/qwen3-coder --autonomous \
  "Run cargo build. It fails. Fix the build error with a SMALL TARGETED edit_file
   change to src/main.rs — do NOT rewrite the whole file. Then run cargo build again."
```

Tool-call tally from the session trace (`.wg/chat/<id>/trace.ndjson`):

| tool | calls |
|---|---|
| `bash` (cargo build) | 4 |
| `read_file` | 2 |
| **`edit_file`** | **2** |
| **`write_file`** | **0** |

The model fixed the error with a targeted `edit_file`
(`old_string: "... sum_all(&nums)\n    println!..."` →
`new_string: "... sum_all(&nums);\n    println!..."`) — result
`Successfully edited /tmp/.../src/main.rs` — and `cargo build` then passed.
**Zero full-file rewrites**, which is the behavior this task set out to produce.
(The local `http://127.0.0.1:8088` endpoint was down in this environment, so the
OpenRouter route was used.) The deterministic registry-level repro in
`tests/test_edit_file_fix_build_repro.rs` additionally exercises the *fuzzy*
matching paths (imperfect indentation / trailing space / CRLF) that a clean
small-file live run does not always trigger.

---

## 5. Future options (not dependencies)

- **Fast-apply / Morph** (approach D): a small apply-model for very loose edit
  intents. Layer *on top of* the fuzzy matcher if ever needed.
- **apply_patch / search-replace envelope** (B/C): add as a *multi-file* patch
  tool if a harness needs it; the matcher here is reusable as its context
  engine.
