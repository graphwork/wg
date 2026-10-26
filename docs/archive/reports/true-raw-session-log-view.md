# True Raw Session Log view

Task: `add-true-raw-log-view`

## Result

The Session Log now cycles in this exact order:

`Events → HighLevel → Pretty → Raw → WgLog → Events`

The former parsed/formatted `RawPretty` transcript is preserved as the user-facing **Pretty** mode. **Raw** is a separate byte lane and never consumes `AgentStreamEvent`, `stream.jsonl`, or another semantic translation.

## Fidelity and source ownership

- The selected task/attempt uses `.wg/agents/<agent>/raw_stream.jsonl` when it exists.
- Only an exact `output.log` file may be used as fallback. The Raw header labels it `output.log fallback`; `output.txt` and synthesized events are not accepted.
- The auxiliary storage lane retains exact `Vec<u8>` windows. It performs no JSON parsing, whitespace trimming, normalization, deduplication, record filtering, or lossy UTF-8 conversion.
- Header provenance records source kind, exact path, retained byte range, file length, first/last-record partiality, attempt identity, and source generation.
- Printable UTF-8 and LF record separators render unchanged. Other Unicode controls and invalid UTF-8 bytes render as uppercase `\xNN` escapes of their original bytes. ESC/CSI bytes therefore cannot reach the terminal backend, and decoding generated escapes reconstructs the retained byte window.

## Bounded paging and live behavior

- Initial/tail reads are reverse reads capped at 1 MiB and 200 records; they never scan from byte zero.
- Explicit older-history reads use the same cap and a fixed retained ceiling of 4 MiB / 2,000 records.
- Live continuation reads are byte-cursor based and preserve partial records across appends.
- Rename/recreate rotation is detected by file identity; truncation is detected by length versus the retained cursor. Both mint a new source generation and replace the old body.
- Re-entering tail performs a bounded EOF refresh. All stat/open/seek/read work stays in the existing auxiliary lane, outside render and event handling.

## Async and interaction safety

Async completion acceptance now fences task, archive iteration, UI generation, mode, selected attempt, source key, file identity, and Raw source generation. Pretty/Raw changes mint a new generation instead of applying an old completion under a new semantic contract.

Raw mode omits summary and JSON controls entirely, creates no hit rectangles for them, and makes their keyboard methods no-ops. View cycling, attempt navigation, scrolling, tailing, and provenance remain available. Wide and compact controls are produced by the same span-derived hit-map path.

## Regression coverage

Rust tests cover:

- valid NDJSON, malformed JSON, plain text, duplicate and blank records, Unicode, a long line, ANSI/control bytes, tabs, and invalid UTF-8;
- reversible visible output and absence of raw ESC bytes;
- raw-stream preference and explicitly named `output.log` fallback;
- bounded 600 MiB reverse tail, live append, partial completion, older history, rotation, and retained limits;
- stale mode/attempt/file/source-generation rejection;
- two-frame Pretty ↔ Raw and attempt-source repaint on one terminal back buffer;
- hidden/inert transform controls at 120, 70, and 40 columns.

`tests/smoke/scenarios/tui_session_log_header_clicks.sh` is an installed-candidate tmux/SGR flow that traverses all five modes by click and keyboard, verifies the exact source path and selected attempt, appends a live plain raw record, switches attempts, and repeats compact-width controls.
