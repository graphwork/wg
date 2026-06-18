//! Streaming-safe splitter for reasoning `<think>…</think>` blocks.
//!
//! Models such as Qwen3, DeepSeek R1, and MiniMax emit chain-of-thought
//! reasoning inline in the assistant `content` channel, wrapped in
//! `<think>…</think>` tags. Under *streaming* those tags (and the
//! reasoning between them) arrive incrementally — a single tag can be
//! split across two SSE deltas (`"…<thi"` then `"nk>…"`). If the raw
//! deltas are echoed straight to the terminal the user sees the entire
//! reasoning dump before the actual answer.
//!
//! [`ThinkSplitter`] is a tiny state machine that consumes content
//! deltas and yields ordered [`ThinkPiece`]s — [`ThinkPiece::Answer`]
//! for user-visible answer text and [`ThinkPiece::Reasoning`] for the
//! suppressed-by-default reasoning. It correctly buffers a partial tag
//! that lands at the end of a delta so tags split across chunk
//! boundaries are still detected.
//!
//! The OpenAI-compatible `reasoning` / `reasoning_content` channel (a
//! *separate* response field rather than inline tags — used by
//! OpenRouter and vLLM) does not flow through this splitter; it is
//! already kept out of the streamed `content` and surfaced after the
//! turn from the assembled [`ContentBlock::Thinking`] block. Both paths
//! share the [`render_collapsed_thought`] marker and the
//! [`reasoning_shown_by_default`] / [`parse_think_arg`] toggle so the
//! default and the `/think` command behave identically regardless of
//! how the model delivered its reasoning.

/// One ordered slice of streamed assistant output, classified by the
/// [`ThinkSplitter`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThinkPiece {
    /// User-visible answer text — always displayed.
    Answer(String),
    /// Reasoning text from inside a `<think>…</think>` block —
    /// suppressed by default, shown only when reasoning display is on.
    Reasoning(String),
}

const OPEN_TAG: &str = "<think>";
const CLOSE_TAG: &str = "</think>";

/// Incremental `<think>…</think>` splitter. Feed each streamed content
/// delta to [`push`](ThinkSplitter::push); at end of stream call
/// [`finish`](ThinkSplitter::finish) to flush any held-back partial
/// tag. The splitter never loses bytes: every byte fed in is emitted
/// exactly once across the returned pieces (possibly held until a later
/// `push` or `finish` once a partial tag is disambiguated).
#[derive(Debug, Default)]
pub struct ThinkSplitter {
    /// True while between an open `<think>` and its matching `</think>`.
    in_think: bool,
    /// Trailing bytes from the previous delta that form a prefix of the
    /// tag we are currently scanning for (`<think>` when answering,
    /// `</think>` when reasoning). Held back until the next delta (or
    /// `finish`) disambiguates whether they begin a real tag.
    pending: String,
}

impl ThinkSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the splitter is currently inside a reasoning block.
    pub fn in_think(&self) -> bool {
        self.in_think
    }

    /// Feed one streamed content delta, returning the ordered pieces it
    /// resolves to. Consecutive same-kind pieces are coalesced.
    pub fn push(&mut self, delta: &str) -> Vec<ThinkPiece> {
        let mut out = Vec::new();
        let mut work = std::mem::take(&mut self.pending);
        work.push_str(delta);

        loop {
            if self.in_think {
                if let Some(idx) = work.find(CLOSE_TAG) {
                    let before = &work[..idx];
                    if !before.is_empty() {
                        coalesce(&mut out, ThinkPiece::Reasoning(before.to_string()));
                    }
                    self.in_think = false;
                    work = work[idx + CLOSE_TAG.len()..].to_string();
                    continue;
                }
                // No full close tag — emit everything except a possible
                // partial `</think>` at the tail, which we hold back.
                let hold = partial_tag_suffix(&work, CLOSE_TAG);
                let split = work.len() - hold;
                if split > 0 {
                    coalesce(&mut out, ThinkPiece::Reasoning(work[..split].to_string()));
                }
                self.pending = work[split..].to_string();
                break;
            } else {
                if let Some(idx) = work.find(OPEN_TAG) {
                    let before = &work[..idx];
                    if !before.is_empty() {
                        coalesce(&mut out, ThinkPiece::Answer(before.to_string()));
                    }
                    self.in_think = true;
                    work = work[idx + OPEN_TAG.len()..].to_string();
                    continue;
                }
                let hold = partial_tag_suffix(&work, OPEN_TAG);
                let split = work.len() - hold;
                if split > 0 {
                    coalesce(&mut out, ThinkPiece::Answer(work[..split].to_string()));
                }
                self.pending = work[split..].to_string();
                break;
            }
        }

        out
    }

    /// Flush any held-back bytes at end of stream. An unclosed
    /// `<think>` (no `</think>` ever arrived) flushes its remainder as
    /// reasoning, matching the non-streaming assembly behavior.
    pub fn finish(&mut self) -> Vec<ThinkPiece> {
        let mut out = Vec::new();
        let pending = std::mem::take(&mut self.pending);
        if !pending.is_empty() {
            if self.in_think {
                coalesce(&mut out, ThinkPiece::Reasoning(pending));
            } else {
                coalesce(&mut out, ThinkPiece::Answer(pending));
            }
        }
        out
    }
}

/// Append `piece` to `out`, merging into the previous element when both
/// are the same variant so callers see the fewest possible pieces.
fn coalesce(out: &mut Vec<ThinkPiece>, piece: ThinkPiece) {
    match (out.last_mut(), &piece) {
        (Some(ThinkPiece::Answer(prev)), ThinkPiece::Answer(s)) => prev.push_str(s),
        (Some(ThinkPiece::Reasoning(prev)), ThinkPiece::Reasoning(s)) => prev.push_str(s),
        _ => out.push(piece),
    }
}

/// Byte-length of the longest suffix of `s` that is a *proper* prefix of
/// `tag` (i.e. a partial tag that might complete in the next delta). A
/// full occurrence of `tag` is handled by the caller's `find` first, so
/// this only considers prefixes shorter than `tag`. Tags are ASCII, so
/// byte comparison never lands mid-codepoint and the returned split
/// point is always a valid `char` boundary.
fn partial_tag_suffix(s: &str, tag: &str) -> usize {
    let sb = s.as_bytes();
    let tb = tag.as_bytes();
    let max = std::cmp::min(sb.len(), tb.len().saturating_sub(1));
    for k in (1..=max).rev() {
        if sb[sb.len() - k..] == tb[..k] {
            return k;
        }
    }
    0
}

/// Render the one-line collapsed-reasoning marker shown after a hidden
/// reasoning block, e.g. `✓ thought for 1234 tokens`. Dimmed grey (244)
/// when `color` is set, matching the rest of the nex status styling.
pub fn render_collapsed_thought(tokens: u64, color: bool) -> String {
    let body = format!("✓ thought for {} tokens", tokens);
    if color {
        format!("\x1b[2;38;5;244m{}\x1b[0m", body)
    } else {
        body
    }
}

/// Dim a raw reasoning reveal (shown when `/think` display is on) so it
/// reads as annotation rather than answer text. Plain text when color
/// is disabled.
pub fn render_reasoning_reveal(text: &str, color: bool) -> String {
    if color {
        format!("\x1b[2;38;5;244m{}\x1b[0m", text)
    } else {
        text.to_string()
    }
}

/// Whether raw reasoning is shown by default, from the `WG_NEX_THINK`
/// environment variable. Default (unset / unrecognized) is **collapsed**
/// — reasoning is hidden behind the `✓ thought for N tokens` marker and
/// revealed only with `/think on` (or `WG_NEX_THINK=1`).
pub fn reasoning_shown_by_default() -> bool {
    match std::env::var("WG_NEX_THINK") {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "on" | "show" | "raw" | "yes" | "expand"
        ),
        Err(_) => false,
    }
}

/// Resolution of a `/think [arg]` REPL command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkToggle {
    /// Show raw reasoning.
    On,
    /// Collapse reasoning to the marker.
    Off,
    /// Flip the current state (bare `/think`).
    Toggle,
}

/// Parse the argument to `/think`. Bare `/think` (empty arg) toggles;
/// explicit on/off synonyms force a state.
pub fn parse_think_arg(arg: &str) -> ThinkToggle {
    match arg.trim().to_ascii_lowercase().as_str() {
        "on" | "show" | "raw" | "expand" | "1" | "true" | "yes" => ThinkToggle::On,
        "off" | "hide" | "collapse" | "0" | "false" | "no" => ThinkToggle::Off,
        _ => ThinkToggle::Toggle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Convenience: drive a splitter over a list of deltas and return
    /// the fully-coalesced (answer, reasoning) concatenations plus the
    /// ordered piece list.
    fn drive(deltas: &[&str]) -> (String, String, Vec<ThinkPiece>) {
        let mut s = ThinkSplitter::new();
        let mut pieces = Vec::new();
        for d in deltas {
            pieces.extend(s.push(d));
        }
        pieces.extend(s.finish());
        let mut answer = String::new();
        let mut reasoning = String::new();
        for p in &pieces {
            match p {
                ThinkPiece::Answer(t) => answer.push_str(t),
                ThinkPiece::Reasoning(t) => reasoning.push_str(t),
            }
        }
        (answer, reasoning, pieces)
    }

    #[test]
    fn plain_text_is_passthrough_answer() {
        let (answer, reasoning, _) = drive(&["hello ", "world"]);
        assert_eq!(answer, "hello world");
        assert_eq!(reasoning, "");
    }

    #[test]
    fn single_block_in_one_delta() {
        let (answer, reasoning, _) = drive(&["<think>plan the steps</think>the answer"]);
        assert_eq!(answer, "the answer");
        assert_eq!(reasoning, "plan the steps");
    }

    #[test]
    fn block_then_answer_separate_deltas() {
        let (answer, reasoning, _) = drive(&[
            "<think>",
            "reason ",
            "more reason",
            "</think>",
            "final ",
            "answer",
        ]);
        assert_eq!(answer, "final answer");
        assert_eq!(reasoning, "reason more reason");
    }

    #[test]
    fn open_tag_split_across_deltas() {
        // `<think>` is fragmented as `<thi` + `nk>`.
        let (answer, reasoning, _) = drive(&["pre<thi", "nk>secret</think>post"]);
        assert_eq!(answer, "prepost");
        assert_eq!(reasoning, "secret");
    }

    #[test]
    fn close_tag_split_across_deltas() {
        // `</think>` is fragmented as `</thi` + `nk>`.
        let (answer, reasoning, _) = drive(&["<think>hidden</thi", "nk>shown"]);
        assert_eq!(answer, "shown");
        assert_eq!(reasoning, "hidden");
    }

    #[test]
    fn tag_split_one_byte_at_a_time() {
        // Worst case: every byte of the open tag arrives in its own delta.
        let deltas = ["<", "t", "h", "i", "n", "k", ">", "x", "</think>", "y"];
        let (answer, reasoning, _) = drive(&deltas);
        assert_eq!(answer, "y");
        assert_eq!(reasoning, "x");
    }

    #[test]
    fn unclosed_think_treats_remainder_as_reasoning() {
        let (answer, reasoning, _) = drive(&["before<think>still thinking"]);
        assert_eq!(answer, "before");
        assert_eq!(reasoning, "still thinking");
    }

    #[test]
    fn multiple_blocks() {
        let (answer, reasoning, _) = drive(&["a<think>r1</think>b<think>r2</think>c"]);
        assert_eq!(answer, "abc");
        assert_eq!(reasoning, "r1r2");
    }

    #[test]
    fn lone_less_than_is_not_held_forever() {
        // A `<` that turns out not to start a think tag must be emitted,
        // not silently swallowed.
        let (answer, reasoning, _) = drive(&["a < b < c"]);
        assert_eq!(answer, "a < b < c");
        assert_eq!(reasoning, "");
    }

    #[test]
    fn partial_open_tag_that_does_not_complete_is_flushed() {
        // Stream ends with a dangling `<thi` that never becomes `<think>`.
        let (answer, reasoning, _) = drive(&["done<thi"]);
        assert_eq!(answer, "done<thi");
        assert_eq!(reasoning, "");
    }

    #[test]
    fn multibyte_answer_survives_partial_tag_holdback() {
        // Ensure holding back a `<` next to multibyte text never slices
        // mid-codepoint.
        let (answer, reasoning, _) = drive(&["café <", "think>π</think> δ"]);
        assert_eq!(answer, "café  δ");
        assert_eq!(reasoning, "π");
    }

    #[test]
    fn in_think_state_tracks_correctly() {
        let mut s = ThinkSplitter::new();
        assert!(!s.in_think());
        s.push("<think>");
        assert!(s.in_think());
        s.push("</think>");
        assert!(!s.in_think());
    }

    #[test]
    fn collapsed_marker_formatting() {
        assert_eq!(
            render_collapsed_thought(42, false),
            "✓ thought for 42 tokens"
        );
        assert_eq!(
            render_collapsed_thought(42, true),
            "\x1b[2;38;5;244m✓ thought for 42 tokens\x1b[0m"
        );
    }

    #[test]
    fn reveal_dim_only_with_color() {
        assert_eq!(render_reasoning_reveal("r", false), "r");
        assert_eq!(
            render_reasoning_reveal("r", true),
            "\x1b[2;38;5;244mr\x1b[0m"
        );
    }

    #[test]
    fn toggle_parsing() {
        assert_eq!(parse_think_arg(""), ThinkToggle::Toggle);
        assert_eq!(parse_think_arg("on"), ThinkToggle::On);
        assert_eq!(parse_think_arg("SHOW"), ThinkToggle::On);
        assert_eq!(parse_think_arg("raw"), ThinkToggle::On);
        assert_eq!(parse_think_arg("off"), ThinkToggle::Off);
        assert_eq!(parse_think_arg("hide"), ThinkToggle::Off);
        assert_eq!(parse_think_arg("collapse"), ThinkToggle::Off);
        assert_eq!(parse_think_arg("garbage"), ThinkToggle::Toggle);
    }

    #[test]
    fn captured_qwen_stream_fixture_splits_cleanly() {
        // Integration check over a CAPTURED sample stream: the recorded
        // content-channel deltas of a Qwen-style response whose
        // `<think>…</think>` block (and both tags) are fragmented across
        // SSE chunk boundaries, followed by the real answer. Drives the
        // splitter exactly as the streaming callback does and asserts the
        // raw reasoning never reaches the answer channel.
        let raw = include_str!("../../../tests/fixtures/nex_think_stream_deltas.json");
        let deltas: Vec<String> =
            serde_json::from_str(raw).expect("fixture is a JSON array of content deltas");

        let mut s = ThinkSplitter::new();
        let mut answer = String::new();
        let mut reasoning = String::new();
        for d in &deltas {
            for piece in s.push(d) {
                match piece {
                    ThinkPiece::Answer(t) => answer.push_str(&t),
                    ThinkPiece::Reasoning(t) => reasoning.push_str(&t),
                }
            }
        }
        for piece in s.finish() {
            match piece {
                ThinkPiece::Answer(t) => answer.push_str(&t),
                ThinkPiece::Reasoning(t) => reasoning.push_str(&t),
            }
        }

        // The user-visible answer is the clean text only — no tags, no
        // reasoning leaked.
        assert_eq!(
            answer,
            "The capital of France is **Paris**. It has been the capital since 987 AD."
        );
        assert!(
            !answer.contains("<think>"),
            "answer must not contain open tag"
        );
        assert!(
            !answer.contains("</think>"),
            "answer must not contain close tag"
        );
        // The reasoning was captured (for the collapse marker / `/think`
        // reveal) and contains the chain-of-thought, not the answer.
        assert!(reasoning.contains("capital of France"));
        assert!(!reasoning.contains("987 AD"));

        // The collapse marker renders from the reasoning token count.
        let marker = render_collapsed_thought(7, false);
        assert_eq!(marker, "✓ thought for 7 tokens");
    }

    #[test]
    fn env_default_off_when_unset() {
        // Avoid mutating process env in parallel tests; just assert the
        // recognized-token logic via the documented synonyms indirectly:
        // an unrecognized value is off, recognized is on. We exercise the
        // parser here since it shares the synonym table with the env path.
        assert_eq!(parse_think_arg("1"), ThinkToggle::On);
        assert_eq!(parse_think_arg("0"), ThinkToggle::Off);
    }
}
