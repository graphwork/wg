# Local Model Integration: Ollama/vLLM with Native Executor

**Date:** 2026-03-04
**Dependency:** [native-executor-tool-gaps.md](native-executor-tool-gaps.md)

## Executive Summary

The workgraph native executor's `OpenAiClient` is well-positioned to work with local models served by **Ollama** and **vLLM**, both of which expose OpenAI-compatible `/v1/chat/completions` endpoints. Tool calling works with capable models, but quality varies dramatically by model family and size. **Qwen3 30B+ and Llama 3.1/3.3 70B are the most viable local options for agentic workgraph tasks.**

---

## 1. Compatibility Assessment

### 1.1 Ollama

**API compatibility:** Ollama's OpenAI-compatible endpoint (`/v1/chat/completions`) supports tool calling as of late 2024. The workgraph `OpenAiClient` sends `stream: false` requests, which avoids Ollama's historical streaming+tools issues.

**Configuration for workgraph:**
```toml
# .wg/config.toml
[native_executor]
provider = "openai"
api_base = "http://localhost:11434"
# No API key needed for local Ollama
```

Set `OPENAI_API_KEY=ollama` (any non-empty string) as Ollama ignores it but the client requires it.

**Known quirks:**
- `tool_choice` parameter is **not supported** — the model decides whether to use tools. This is acceptable since the agent loop already handles both tool-use and text-only responses.
- Tool call IDs: Ollama generates UUIDs in the `call_XXXXXX` format that OpenAI uses. Compatible with the `OpenAiClient`'s translation layer.
- Response format matches OpenAI spec: `finish_reason: "tool_calls"` when tools are invoked, `finish_reason: "stop"` otherwise. The `translate_response` function handles both correctly.
- **No `logprobs`** — not needed for workgraph.

### 1.2 vLLM

**API compatibility:** vLLM provides a full OpenAI-compatible server with tool calling via `--enable-auto-tool-choice` and `--tool-call-parser <parser>`. Streaming and non-streaming both supported.

**Configuration for workgraph:**
```toml
# .wg/config.toml
[native_executor]
provider = "openai"
api_base = "http://localhost:8000"
```

**vLLM server launch:**
```bash
vllm serve Qwen/Qwen3-32B \
  --enable-auto-tool-choice \
  --tool-call-parser hermes \
  --max-model-len 32768
```

**Known quirks:**
- Requires explicit `--tool-call-parser` flag matching the model family (hermes for Qwen/Hermes, llama3_json for Llama, mistral for Mistral).
- `tool_choice: "required"` supported since vLLM 0.8.3+.
- Uses guided decoding to enforce valid JSON in tool arguments — more reliable than Ollama's unguided generation.
- Better throughput than Ollama for concurrent agent workloads (tensor parallelism, continuous batching).

---

## 2. Tool-Use Capability Matrix

Models rated on three dimensions:
- **Tool detection:** Does the model correctly decide when to use a tool vs. respond with text?
- **Argument quality:** Does it produce valid JSON arguments with correct parameter names/types?
- **Multi-turn coherence:** Can it sustain a tool-use conversation across many turns (typical agent loop: 10-50 turns)?

| Model | Size | Tool Detection | Arg Quality | Multi-Turn | BFCL Score | Ollama | vLLM | Notes |
|-------|------|:-:|:-:|:-:|:-:|:-:|:-:|-------|
| **Qwen3-235B-A22B** | 235B (22B active) | Excellent | Excellent | Excellent | 70.8% | Yes | Yes | MoE; best open-source option. Requires ~48GB VRAM (A22B active). |
| **Llama 3.1 405B** | 405B | Excellent | Excellent | Good | 81.1% | Partial | Yes | Needs multi-GPU; impractical for most local setups. |
| **Llama 3.3 70B** | 70B | Good | Good | Good | 77.3% | Yes (Q4) | Yes | Best Llama for local. Q4 fits in 48GB VRAM. |
| **Qwen3-32B** | 32B | Good | Good | Good | ~65%* | Yes | Yes | Strong reasoning, native Hermes-style tool calling. |
| **DeepSeek V3** | 671B (37B active) | Good | Good | Fair | 58.6% | No | Yes | MoE; primarily a reasoning model, tool use is secondary. |
| **Qwen3-8B** | 8B | Fair | Fair | Fair | ~55%* | Yes | Yes | Usable for simple tasks. Struggles with complex tool schemas. |
| **Llama 3.1 8B** | 8B | Fair | Poor | Poor | ~50%* | Yes | Yes | Frequently hallucinates tool names or argument formats. |
| **Mistral 7B** | 7B | Fair | Poor | Poor | ~45%* | Yes | Yes | Early tool support; inconsistent argument formatting. |
| **Qwen3-4B** | 4B | Poor | Poor | Poor | — | Yes | Yes | Too small for reliable tool use. |
| **Llama 3.2 3B** | 3B | Poor | Poor | Poor | — | Yes | Yes | Cannot reliably follow tool-use format. |

*Scores marked with \* are estimates based on published benchmarks for related model versions; exact BFCL v4 scores not publicly available for all variants.*

---

## 3. Quality Assessment for Workgraph Tasks

### What workgraph agents actually do (from tool gap analysis)

The typical workgraph agent session involves:
1. Read task description and orient (Bash: `wg show`, `wg context`)
2. Search/read codebase (Grep, Read, Glob)
3. Edit/write files (Edit, Write)
4. Build/test (Bash: `cargo build`, `cargo test`)
5. Log progress and mark done (Bash: `wg log`, `wg done`)

This requires: reliable tool detection, valid JSON arguments, ability to interpret tool output and decide next action across 10-50 turns.

### Quality tiers for workgraph tasks

**Tier 1 — Production-viable (can replace Claude for most tasks):**
- Qwen3-235B-A22B (MoE, ~48GB VRAM)
- Llama 3.3 70B (dense, ~48GB VRAM at Q4)

These models can: follow complex system prompts, correctly choose between 8+ tools, produce valid arguments, sustain 20+ turn conversations, and interpret error output to self-correct.

**Tier 2 — Viable for simple/structured tasks:**
- Qwen3-32B (~20GB VRAM at Q4)
- DeepSeek V3 (via vLLM with multi-GPU)

These models can handle: file reading/editing, simple bash commands, `wg` commands. They struggle with: complex multi-step debugging, large codebase navigation, ambiguous task descriptions.

**Tier 3 — Research/experimentation only:**
- Qwen3-8B, Llama 3.1 8B, Mistral 7B

These models frequently: hallucinate tool names, produce malformed JSON arguments, lose context in multi-turn conversations, fail to self-correct after tool errors. Not recommended for unattended agent work.

**Tier 4 — Not viable:**
- Models under 8B parameters. Cannot reliably follow the tool-use format.

### Specific failure modes observed with local models

1. **Tool name hallucination:** Small models invent tool names not in the provided list (e.g., calling `execute_command` instead of `bash`).
2. **Argument format errors:** JSON arguments with unquoted strings, missing required fields, or wrong types. vLLM's guided decoding mitigates this.
3. **Premature completion:** Model emits `stop` instead of `tool_calls` when it should invoke a tool, ending the agent loop early.
4. **Context window exhaustion:** Local models typically have 8K-32K context. A 20-turn agent session with tool results can exceed this. Qwen3 (32K-128K depending on variant) handles this best.
5. **Instruction following degradation:** After ~10 turns, smaller models "forget" the system prompt instructions about using `wg done` when finished.

---

## 4. Streaming Behavior

The workgraph `OpenAiClient` uses **non-streaming** requests (`stream: false` in `OaiRequest`). This is the safest path for local model compatibility:

| Server | Non-streaming + tools | Streaming + tools | Notes |
|--------|:-----:|:-----:|-------|
| Ollama | Works | Historically buggy | Ollama's OpenAI compat layer had issues with streaming tool calls; native `/api/chat` was preferred. Non-streaming is reliable. |
| vLLM | Works | Works | Both modes supported. Non-streaming simpler. |

**Recommendation:** Keep `stream: false` for local models. The latency cost is minimal (responses are typically <30s for agent turns), and it avoids streaming compatibility issues across different server versions.

---

## 5. Configuration Guidance

### Recommended setup for Ollama

```bash
# Install Ollama
curl -fsSL https://ollama.com/install.sh | sh

# Pull recommended model
ollama pull qwen3:32b

# Start server (default port 11434)
ollama serve
```

```toml
# .wg/config.toml
[native_executor]
provider = "openai"
api_base = "http://localhost:11434"
max_tokens = 8192
```

```bash
# Set env vars
export OPENAI_API_KEY=ollama  # Required but ignored
export WG_MODEL=qwen3:32b
```

### Recommended setup for vLLM

```bash
# Install vLLM
pip install vllm

# Serve with tool calling enabled
vllm serve Qwen/Qwen3-32B \
  --enable-auto-tool-choice \
  --tool-call-parser hermes \
  --max-model-len 32768 \
  --tensor-parallel-size 2  # For multi-GPU
```

```toml
# .wg/config.toml
[native_executor]
provider = "openai"
api_base = "http://localhost:8000"
max_tokens = 8192
```

```bash
export OPENAI_API_KEY=local  # Required but ignored
export WG_MODEL=Qwen/Qwen3-32B
```

### Ollama vs vLLM: when to use which

| Criterion | Ollama | vLLM |
|-----------|--------|------|
| **Ease of setup** | Excellent — single binary | Moderate — Python, CUDA deps |
| **Tool-calling reliability** | Good (unguided generation) | Better (guided decoding) |
| **Throughput** | Single-request at a time | Concurrent batching |
| **Multi-GPU** | Limited | Native tensor parallelism |
| **Resource usage** | Lower overhead | Higher but more efficient at scale |
| **Best for** | Single-agent dev/testing | Multi-agent production with `wg service start --max-agents N` |

**Use Ollama** for development and single-agent testing.
**Use vLLM** when running multiple concurrent agents (the coordinator dispatches N agents in parallel).

---

## 6. Code Compatibility Notes

### What works today (no changes needed)

The existing `OpenAiClient` in `src/executor/native/openai_client.rs` is fully compatible:

1. **API format:** Standard OpenAI `/v1/chat/completions` — both Ollama and vLLM implement this.
2. **Tool definitions:** Sent as `tools: [{ type: "function", function: {...} }]` — standard format.
3. **Tool results:** Sent as `role: "tool"` messages with `tool_call_id` — standard format.
4. **Non-streaming:** `stream: false` — the safest mode for compatibility.
5. **Retry logic:** Handles 429/500/502/503 — local servers may return these under load.
6. **Base URL override:** `with_base_url()` or config `api_base` — works for any local endpoint.

### Potential improvements (not blocking)

1. **API key handling:** The client requires a non-empty API key (`resolve_openai_api_key` fails if all env vars are empty). Local servers don't need authentication. Consider allowing `api_key = "none"` or `api_key = ""` in config to skip auth for local servers.

2. **Timeout:** 300s default may be too short for large local models on slow hardware. Could expose `timeout_secs` in config.

3. **`tool_choice` enforcement:** Neither Ollama nor most local models respect `tool_choice: "required"`. If the agent loop depends on this (currently it doesn't), it would need workaround logic.

4. **Model-specific max_tokens:** Some local models have lower limits (e.g., 4096). The default 16384 may cause errors. Already configurable via config `max_tokens`.

---

## 7. Recommendations

### For immediate use
1. **Qwen3-32B via Ollama** is the sweet spot for development — good tool calling, fits on a single GPU (24GB at Q4), easy setup.
2. **Qwen3-235B-A22B via vLLM** for production quality — MoE means only ~22B active params but near-frontier tool-use quality.
3. **Llama 3.3 70B** as an alternative if you prefer Meta's ecosystem — strong BFCL scores, well-tested.

### For the quality/cost sweet spot
| Use case | Recommended model | VRAM needed | Quality |
|----------|------------------|:-----------:|:-------:|
| Development/testing | Qwen3-32B (Q4) | 20 GB | Good |
| Single-agent production | Qwen3-32B (FP16) or Llama 3.3 70B (Q4) | 48 GB | Good-Excellent |
| Multi-agent production | Qwen3-235B-A22B (FP16) | 48 GB | Excellent |
| Budget/constrained | Qwen3-8B (FP16) | 16 GB | Fair (simple tasks only) |

### What NOT to use
- Models under 8B for any agentic work
- DeepSeek R1 or other reasoning-focused models (great at math, poor at tool use)
- Any model without explicit tool-calling training (e.g., base models, chat-only finetunes without function calling)

### Future considerations
- **MCP (Model Context Protocol)**: Ollama is adding MCP support. If workgraph adopts MCP for tool definitions, this would provide a standardized interface across local and remote models.
- **Speculative decoding**: vLLM supports speculative decoding for faster inference with draft models — could significantly reduce agent turn latency.
- **LoRA adapters**: Fine-tuning a smaller model (8B-14B) specifically on workgraph tool schemas could dramatically improve quality at the small model tier.
