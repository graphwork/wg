# Progressive streaming markdown in the nex CLI

`wg nex` (and the standalone `nex` binary) render the assistant's answer as
**styled markdown** while it streams — headings, bold/italic, bullet and
numbered lists, inline code, fenced code blocks, links, blockquotes, and
tables — without waiting for the whole response to finish. Formatting is
applied incrementally as tokens arrive, so the output keeps the same live
feel as plain streaming text.

## Why streaming markdown is hard

Markdown needs lookahead. You cannot bold `**France**` until the closing
`**` has arrived, and a fenced code block is not a code block until its
closing ```` ``` ```` lands. The naive options are both bad:

- **Buffer the whole turn, render at the end** — loses the live feel; the
  user stares at a blank screen, then the answer pops in all at once.
- **Echo raw markdown** — live, but the user reads `**bold**` and
  ```` ```rust ```` punctuation instead of formatted output.

## How nex does it

The stream is split into two regions (`src/executor/native/streaming_markdown.rs`):

- **Committed blocks** — markdown blocks (paragraphs, headings, lists,
  fenced code, …) that are *definitely complete* because a blank line
  terminated them at top level. A committed block is rendered once with
  [`crate::markdown::markdown_to_ansi`] and printed permanently. It never
  moves or redraws, so there is **zero flicker** on text already read.
- **The live tail** — the in-progress block after the last blank line. It
  is re-rendered and redrawn in place on every delta, so the current
  paragraph/list/heading updates token-by-token. Partial markdown (an
  unclosed `**bold`, a half-open code fence, an in-progress list) renders
  best-effort and is corrected the instant the closing token arrives.

`BlockSplitter` is the pure, fence-aware state machine that performs the
split. It is **lossless** and **chunking-invariant**: the sequence of
committed blocks does not depend on how the byte stream was chopped into
SSE deltas, so the final rendered output is identical to a one-shot render
of the finished text regardless of how the model fragmented its tokens.
`CursorRenderer` drives the splitter for a VT100 terminal, emitting the
cursor-control + ANSI byte sequence for each delta (commit finished blocks,
move up and erase the previous live tail with `ESC[…A` / `ESC[0J`, redraw).

Because a block becomes the live region only between blank lines, the
worst case for redraw cost is a single long unbroken block; typical chat
replies are a sequence of short blocks that commit and scroll away as the
model emits the next blank line.

## When it is active vs the plain fallback

| Context | Behavior |
|---------|----------|
| Interactive TTY, no rustyline live-input layer, color on | **Progressive styled markdown** (`CursorRenderer`) |
| `--eval-mode` / autonomous task agents | Raw text to the stream sinks (unchanged) |
| Piped / redirected / non-TTY stderr | **Plain passthrough** — raw markdown, no ANSI |
| `NO_COLOR` / `TERM=dumb` | Plain passthrough |
| rustyline live-input layer present (`WG_NEX_LIVE_INPUT=1`, the default in a live terminal) | Plain live stream — its append-only `ExternalPrinter` cannot host an in-place redraw, so the answer streams as live plain text |

The progressive renderer owns the cursor, so it only runs when no other
component (rustyline's live prompt) is drawing to the same screen. Set
`WG_NEX_LIVE_INPUT=0` to take the direct-stderr path and get progressive
styled markdown in a live terminal.

`markdown_to_plain` renders the same layout (bullets, heading text,
code-block bars, tables) with every color/style escape stripped, for
callers that want structured-but-uncolored text.

## Relationship to reasoning display

Reasoning `<think>…</think>` handling (see
[nex-reasoning-display.md](nex-reasoning-display.md)) runs *upstream* of
the markdown renderer: `ThinkSplitter` classifies each delta as answer vs
reasoning, and only the **answer** pieces flow into the markdown renderer.
Suppressed reasoning and the `✓ thought for N tokens` collapse marker are
never passed through markdown rendering.

## Tests

- Unit + virtual-terminal coverage in
  `src/executor/native/streaming_markdown.rs`: `BlockSplitter`
  chunking-invariance (byte-by-byte and random splits produce the same
  block sequence), fence-awareness, losslessness, and a `CursorRenderer`
  "final screen == one-shot render" no-garble check driven through a tiny
  VT100 model over single-character deltas.
- Human-flow PTY smoke: `tests/smoke/scenarios/nex_progressive_markdown_pty.sh`
  (owned by `progressive-streaming-markdown`) drives real `nex` through a
  PTY against a mock server that fragments every markdown token across SSE
  deltas, reconstructs the final screen, and asserts formatted output on
  the TTY path + raw passthrough on the piped path.
