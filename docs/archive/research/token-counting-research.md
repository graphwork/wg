# Token Counting Research for Role Weight Calculation

This document summarizes research on token counting approaches for calculating role weights in workgraph.

## 1. Rust Tokenizer Options

### tiktoken-rs

**Repository:** [github.com/zurawiki/tiktoken-rs](https://github.com/zurawiki/tiktoken-rs)
**Version:** 0.9.1 (as of May 2025)
**Downloads:** ~314k/month
**License:** MIT

The most mature Rust tokenizer for OpenAI models. Provides:

- Multiple encodings: `o200k_base` (GPT-4o, o1, o3), `cl100k_base` (ChatGPT), `p50k_base` (code models)
- Singleton and instance-based initialization
- Chat completion token calculation helpers
- Optional `async-openai` integration

```rust
use tiktoken_rs::o200k_base;

fn count_tokens(text: &str) -> usize {
    let bpe = o200k_base().unwrap();
    bpe.encode_with_special_tokens(text).len()
}
```

**Considerations:**
- Downloads vocabulary files on first use (~2MB for o200k_base)
- Tight coupling to OpenAI model families
- Does not support Claude's tokenizer

### HuggingFace tokenizers

**Repository:** [github.com/huggingface/tokenizers](https://github.com/huggingface/tokenizers)
**Crate:** [crates.io/crates/tokenizers](https://crates.io/crates/tokenizers)

The HuggingFace tokenizers library is written in Rust with Python bindings. Features:

- Native Rust implementation (the Python version wraps Rust)
- Support for BPE, WordPiece, Unigram tokenizers
- Can load any HuggingFace tokenizer config
- Claimed performance: <20 seconds to tokenize 1GB of text

```rust
use tokenizers::Tokenizer;

let tokenizer = Tokenizer::from_pretrained("bert-base-uncased", None)?;
let encoding = tokenizer.encode("Hello world", false)?;
println!("Token count: {}", encoding.get_ids().len());
```

**Considerations:**
- Heavier dependency than tiktoken-rs
- More flexible (can load arbitrary tokenizers)
- No direct Claude tokenizer support without loading the model config

### bpe crate

**Crate:** [crates.io/crates/bpe](https://crates.io/crates/bpe)

A pure Rust BPE implementation claiming ~10x speedup over HuggingFace for encoding tasks. Lower-level API focused on the BPE algorithm itself rather than specific model tokenizers.

## 2. Byte-Based Approximations

### Common Heuristics

| Method | Formula | Typical Use Case |
|--------|---------|------------------|
| Bytes/4 | `text.len() / 4` | English text |
| Chars/4 | `text.chars().count() / 4` | Unicode-safe variant |
| Words × 1.3 | `word_count * 1.3` | Prose-heavy content |
| Chars/3.5 | `text.chars().count() / 3.5` | Claude-specific estimate |

### Accuracy Analysis

**English prose:**
- Bytes/4 approximation: typically within 5-10% for clean English text
- OpenAI's documentation states BPE averages ~4 bytes per token

**Code:**
- Code tokenizes differently due to operators, brackets, variable names
- Error rates of 15-25% are common with simple heuristics
- Indentation and whitespace patterns affect accuracy

**Non-English text:**
- Japanese/Chinese: much higher token counts (up to 2-3× more tokens per character)
- Accented languages: ~10% deviation reported

### Research Finding: Linear Relationship for Claude

Research on Claude tokenization found that the relationship between bytes/runes and input_tokens appears to be **linear**. This suggests a simple formula like:

```rust
fn estimate_claude_tokens(text: &str) -> usize {
    // Empirically, ~3.5-4 chars per token for English
    (text.chars().count() as f64 / 3.7).ceil() as usize
}
```

Could be calibrated against Claude's token counting API for specific content types (markdown role definitions).

## 3. Model-Specific Concerns

### Tokenizer Differences by Model Family

| Model Family | Tokenizer | Notes |
|--------------|-----------|-------|
| Claude (Anthropic) | Proprietary | API-only counting, ~3.5-4 chars/token for English |
| GPT-4/4o (OpenAI) | o200k_base | tiktoken-rs supports |
| GPT-3.5 (OpenAI) | cl100k_base | tiktoken-rs supports |
| LLaMA/Mistral | SentencePiece | Different token boundaries |
| Gemini (Google) | Proprietary | API-based counting |

### Cross-Model Variance

Testing the same text across tokenizers shows:
- GPT-4o vs Claude: typically within 10-15% of each other for English
- GPT-4o vs older GPT-3.5: can differ by 15-20% (different vocab sizes)
- Same text, different tokenizers: variations of 5-25% depending on content type

### Does Accuracy Matter?

For role weight calculation in workgraph, consider what the token count is used for:

1. **Context budget decisions:** "Can this role fit in the context window?"
   - 10% error: Unlikely to cause problems (200K context, 10% = 20K tokens of buffer)
   - 20% error: Still workable with conservative limits
   - 30%+ error: Could cause unexpected context overflow

2. **Cost estimation:** Not billing-critical, just for planning
   - Any approximation is fine for rough estimates

3. **Model selection:** Choosing between models based on required context
   - 10-15% variance is acceptable with appropriate safety margins

**Recommendation:** For role weight, ~10-15% accuracy is sufficient. The role definitions are markdown files of moderate size (typically <10K tokens), so even 20% error means ±2K tokens—well within the margins of modern context windows.

## 4. Recommendation for workgraph

### Pragmatic Choice: Simple Heuristic with Optional Calibration

Given:
- Role definitions are markdown files (mostly English prose with some code examples)
- Accuracy requirements are modest (context budget decisions, not billing)
- Minimizing dependencies is valuable for a CLI tool
- Multiple model families may be used

**Recommended approach:**

```rust
/// Estimate token count for a role definition.
/// Uses a simple heuristic calibrated for English markdown content.
///
/// Accuracy: ~85-90% for English prose/markdown, ~75-85% for code-heavy content.
pub fn estimate_tokens(text: &str) -> usize {
    // Count Unicode characters (handles non-ASCII properly)
    let char_count = text.chars().count();

    // Use 3.8 chars/token as middle ground between:
    // - OpenAI's ~4 bytes/token
    // - Claude's ~3.5 chars/token
    // This slightly overestimates, providing a safety margin
    (char_count as f64 / 3.8).ceil() as usize
}

/// More accurate estimate that considers content type.
pub fn estimate_tokens_with_hints(text: &str, code_heavy: bool) -> usize {
    let char_count = text.chars().count();

    // Code typically has more tokens per character due to:
    // - Short variable names
    // - Punctuation-heavy syntax
    // - Whitespace patterns
    let chars_per_token = if code_heavy { 3.2 } else { 3.8 };

    (char_count as f64 / chars_per_token).ceil() as usize
}
```

### Why Not a Full Tokenizer?

1. **Dependencies:** tiktoken-rs adds ~2MB of vocabulary data and non-trivial compile time
2. **Model lock-in:** Choosing GPT's tokenizer when users might use Claude feels wrong
3. **Marginal benefit:** For context budget decisions, a heuristic that's 90% accurate is sufficient
4. **Simplicity:** A pure Rust function with no external dependencies is easier to maintain

### Optional Enhancement: Calibration Mode

For users who need higher accuracy, offer an optional calibration feature:

```rust
// In config.toml
[role_weights]
chars_per_token = 3.8  # User-adjustable based on their model

// Or via CLI
wg config set role_weights.chars_per_token 3.5  # For Claude users
```

### When to Use a Full Tokenizer

Consider adding tiktoken-rs as an optional feature if:
- Users request billing-grade accuracy
- Role definitions become very large (>50K chars)
- workgraph adds support for precise context management

```toml
# Cargo.toml
[features]
default = []
precise-tokens = ["tiktoken-rs"]
```

## Summary

| Approach | Accuracy | Dependencies | Complexity | Recommendation |
|----------|----------|--------------|------------|----------------|
| chars/3.8 heuristic | ~85-90% | None | Trivial | **Use this** |
| tiktoken-rs | ~100% (GPT) | tiktoken-rs | Low | Optional feature |
| HuggingFace tokenizers | ~100% | tokenizers | Medium | Overkill |
| Claude API counting | 100% | HTTP client | Medium | Not practical offline |

**Bottom line:** Use `chars / 3.8` as the default, document the expected variance (±10-15%), and let users adjust the ratio in config if needed. This gives good-enough accuracy for context budget decisions without adding dependencies or complexity.

## References

- [tiktoken-rs crate](https://crates.io/crates/tiktoken-rs)
- [HuggingFace tokenizers](https://github.com/huggingface/tokenizers)
- [Anthropic Token Counting API](https://docs.anthropic.com/en/api/messages-count-tokens)
- [OpenAI tiktoken](https://github.com/openai/tiktoken)
- [Token counting guide (Propel)](https://www.propelcode.ai/blog/token-counting-tiktoken-anthropic-gemini-guide-2025)
- [Practical guide to token counting](https://winder.ai/calculating-token-counts-llm-context-windows-practical-guide/)
