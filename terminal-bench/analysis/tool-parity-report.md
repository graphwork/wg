# Tool Parity Report: Claude Code Agents vs Terminal Bench Adapter Agents

**Date:** 2026-04-04
**Task:** research-tool-parity
**Purpose:** Full tool inventory comparison, gap analysis, and recommendations for achieving parity.

---

## 1. Complete Tool Inventory

### 1.1 Claude Code Agent Built-in Tools

Claude Code agents (the ones running in this workgraph repo) have access to the following tools:

| # | Tool | Category | Description |
|---|------|----------|-------------|
| 1 | **Bash** | System | Execute shell commands (git, cargo, npm, etc.) with configurable timeout |
| 2 | **Read** | File I/O | Read file contents with offset/limit, supports images, PDFs, notebooks |
| 3 | **Write** | File I/O | Create or overwrite files |
| 4 | **Edit** | File I/O | Targeted string-replacement edits to existing files |
| 5 | **Glob** | Search | Fast file pattern matching (e.g., `**/*.rs`) |
| 6 | **Grep** | Search | Regex content search via ripgrep, with context lines, file type filters |
| 7 | **WebSearch** | Web | Search the internet, returns structured results with URLs |
| 8 | **WebFetch** | Web | Fetch URL content, convert HTML→markdown, process with AI prompt |
| 9 | **Agent** (Task) | Delegation | Spawn sub-agents for parallel/independent work |
| 10 | **NotebookEdit** | Specialized | Edit Jupyter notebook cells |
| 11 | **LSP** | Specialized | Language Server Protocol integration (go-to-definition, references, diagnostics) |
| 12 | **TodoWrite** | Planning | Create/update task checklists for tracking progress |
| 13 | **AskUserQuestion** | Interaction | Ask the user a clarifying question |
| 14 | **Skill** | Meta | Invoke registered skills (e.g., `/commit`, `/wg`) |
| 15 | **ToolSearch** | Meta | Discover and load deferred tools dynamically |

**Additional capabilities via settings/plugins:**
- **rust-analyzer LSP plugin** (enabled in settings.json)
- **clangd LSP plugin** (enabled in settings.json)
- **MCP tools** (marketplace plugins available but not project-configured): GitHub, Slack, Linear, Playwright, etc.
- **Custom skills**: `/wg` skill (SKILL.md in `.claude/skills/wg/`)

**System-level capabilities via Bash:**
- git (version control)
- cargo/rustc (Rust toolchain)
- npm/node (JavaScript)
- python3/pip (Python)
- curl/wget (HTTP)
- Any system binary available on the host

### 1.2 Terminal Bench Adapter Tools

From `terminal-bench/wg/adapter.py`:

#### Condition A Tools (6 tools — bare control)

| # | Tool | Maps to Claude Code | Notes |
|---|------|-------------------|-------|
| 1 | `bash` | Bash | Shell execution with timeout |
| 2 | `read_file` | Read | File reading with offset/limit |
| 3 | `write_file` | Write | Create/overwrite files |
| 4 | `edit_file` | Edit | String replacement editing |
| 5 | `glob` | Glob | File pattern matching (via `find` + `ls`) |
| 6 | `grep` | Grep | Regex search (via system `grep -rn`) |

#### Condition B–E Tools (15 tools — adds workgraph)

All Condition A tools, plus:

| # | Tool | Category | Description |
|---|------|----------|-------------|
| 7 | `wg_show` | Workgraph | Show task details |
| 8 | `wg_list` | Workgraph | List tasks by status |
| 9 | `wg_add` | Workgraph | Create new tasks |
| 10 | `wg_done` | Workgraph | Mark task done (with --converged) |
| 11 | `wg_fail` | Workgraph | Mark task failed |
| 12 | `wg_log` | Workgraph | Append log entry |
| 13 | `wg_artifact` | Workgraph | Record file artifact |
| 14 | `wg_msg_send` | Workgraph | Send message to task |
| 15 | `wg_msg_read` | Workgraph | Read task messages |

#### Condition F Tools (15 tools — enhanced wg_add)

Same as B–E but `wg_add` gains `verify` and `id` parameters.

---

## 2. Gap Analysis

### 2.1 Priority Matrix

| Missing Tool | Claude Code Equivalent | Priority | Rationale |
|-------------|----------------------|----------|-----------|
| **Web Search** | `WebSearch` | **CRITICAL** | Many TB tasks require looking up documentation, API references, library versions. Without search, agents must rely solely on pre-training knowledge, which is often stale or wrong for specific library APIs. |
| **Web Fetch** | `WebFetch` | **CRITICAL** | Companion to web search — once you find a URL, you need to read it. Required for fetching docs, examples, error explanations. |
| **LSP** | `LSP` (rust-analyzer, clangd) | **LOW** | TB tasks run inside Docker containers with diverse language environments. Setting up LSP servers per-task is impractical. Agents can use `grep` + `bash` as substitutes. |
| **NotebookEdit** | `NotebookEdit` | **IRRELEVANT** | No Jupyter notebook tasks in TB2. Can be done via `write_file` + JSON manipulation if ever needed. |
| **Agent (subagent spawn)** | `Agent` / `Task` | **IRRELEVANT** | TB adapter already handles decomposition via `wg_add`. The coordinator dispatches subtasks externally. Spawning sub-agents inside a benchmark trial would violate TB's single-agent-per-trial model. |
| **TodoWrite** | `TodoWrite` | **NICE-TO-HAVE** | Lightweight planning aid. The model can use `wg_log` for similar effect. Low implementation cost but marginal benefit for benchmark tasks. |
| **AskUserQuestion** | `AskUserQuestion` | **IRRELEVANT** | No human in the loop during TB trials. Tasks must be completed autonomously. |
| **Skill** | `Skill` | **IRRELEVANT** | Meta-tool for Claude Code's skill system. TB agents don't have a skill registry. |
| **ToolSearch** | `ToolSearch` | **IRRELEVANT** | Meta-tool for deferred tool loading. TB agents have a fixed tool set. |

### 2.2 Implementation Gaps by Priority

#### CRITICAL (blocks task success on information-dependent tasks)

1. **`web_search`** — Search the internet for documentation, examples, error messages
2. **`web_fetch`** — Fetch and read web page content in LLM-friendly format

#### NICE-TO-HAVE (improves agent UX but not blocking)

3. **`todo_write`** — In-context planning checklist (can be simulated via `wg_log`)

#### NOT NEEDED (irrelevant for TB benchmark context)

4. LSP, NotebookEdit, Agent, AskUserQuestion, Skill, ToolSearch

### 2.3 Existing Tool Parity Assessment

The 6 core tools (bash, read_file, write_file, edit_file, glob, grep) have reasonable parity but with implementation quality gaps:

| Tool | Claude Code Quality | TB Adapter Quality | Gap |
|------|-------------------|-------------------|-----|
| **Bash** | Full subprocess with timeout, stdout/stderr separation | Equivalent via Harbor `env.exec` | ≈ Parity |
| **Read** | Line numbers, PDF support, image support, smart truncation | Basic `cat`/`sed`/`head`/`tail` | Minor (no PDF/image, but not needed) |
| **Write** | Direct file write | Base64-encoded pipe (avoids escaping issues) | ≈ Parity |
| **Edit** | Smart diffing, line-number targeting | String replacement with uniqueness check | ≈ Parity |
| **Glob** | ripgrep-backed, fast, modification-time sorted | `find` + `ls` fallback, head -200 | Minor (functional but slower) |
| **Grep** | ripgrep with context, file types, output modes, multiline | System `grep -rn`, head -200 | Moderate (no context lines, no type filter, no multiline) |

The grep gap is notable — Claude Code's Grep supports `-A`/`-B`/`-C` context lines, file type filtering (`--type`), output modes (content/files/count), and multiline matching. The TB adapter's grep is basic `grep -rn`. This could be improved but is not blocking.

---

## 3. Web Search API Recommendation

### 3.1 Comparison Table

| API | Price per 1K queries | Free tier | Rate limits | Quality | API key required | Model-agnostic |
|-----|---------------------|-----------|-------------|---------|-----------------|----------------|
| **Brave Search** | $5.00 | $5/mo credits (~1K queries) | 1 QPS (free), up to 50 QPS (Pro) | Good, independent index | Yes | Yes |
| **Tavily** | $8.00 (advanced) / $4.00 (basic) | 1,000/mo free | Not documented publicly | Good, AI-optimized responses | Yes | Yes |
| **SerpAPI** | $15.00 | None | Varies by plan | Excellent (real Google results) | Yes | Yes |
| **Serper** | $0.30–$1.00 | 2,500 one-time | 300 QPS | Good (real Google results) | Yes | Yes |
| **DuckDuckGo** | Free | Unlimited (unofficial) | ~1 QPS (unofficial) | Moderate | **No** | Yes |
| **Bing** | $35.00 (Grounding) | None | Varies | Good | Yes (Azure) | Yes |

### 3.2 Recommendation: **DuckDuckGo (primary) + Brave Search (fallback)**

**Primary: DuckDuckGo via `duckduckgo-search` Python library**

Rationale:
- **No API key required** — critical for benchmark reproducibility and ease of setup
- **Free** — no cost concerns for running hundreds of TB trials
- **Good enough quality** for the TB use case (finding docs, API references, error messages)
- **Python library** (`pip install duckduckgo-search`) with simple API
- **No rate limit management** needed for our throughput (one agent at a time, occasional searches)

Weaknesses:
- Unofficial API — could break if DDG changes their frontend
- Lower quality than Google results for niche technical queries
- ~1 QPS rate limit may slow down rapid successive searches

**Fallback: Brave Search API**

For cases where DDG doesn't return useful results, Brave is the best fallback:
- Independent search index (not Google-dependent)
- $5/mo free credits is enough for testing
- Clean API with good documentation
- AI-grounding-friendly response format

**Why not others:**
- **Tavily**: Good AI-native design, but $8/1K for advanced search is expensive at scale, and Nebius acquisition creates uncertainty
- **SerpAPI**: Too expensive ($15/1K) for benchmark usage
- **Serper**: Great price ($0.30–$1.00/1K), but requires API key and paid plan for serious usage
- **Bing**: Deprecated legacy API; replacement is expensive ($35/1K) and Azure-locked

### 3.3 Implementation Sketch

```python
WEB_SEARCH_TOOL = {
    "type": "function",
    "function": {
        "name": "web_search",
        "description": (
            "Search the web for information. Returns titles, URLs, and snippets "
            "for the top results. Use for documentation, API references, error messages."
        ),
        "parameters": {
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query string.",
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum results to return (default: 5, max: 10).",
                },
            },
        },
    },
}

async def _exec_web_search(env: BaseEnvironment, args: dict) -> str:
    """Search using duckduckgo-search inside the container."""
    query = shlex.quote(args["query"])
    max_results = args.get("max_results", 5)
    cmd = (
        f"python3 -c \""
        f"from duckduckgo_search import DDGS;"
        f"results = DDGS().text({query}, max_results={max_results});"
        f"[print(f\\\"{{% raw %}}{{r['title']}}\\n{{r['href']}}\\n{{r['body']}}\\n{{% endraw %}}\\\") for r in results]"
        f"\""
    )
    # Alternatively, run on host if container lacks the library:
    # pip install duckduckgo-search in container setup, or run on host
    result = await env.exec(command=cmd, timeout_sec=30)
    return result.stdout or "(no results)"
```

**Note:** Running search on the **host** (not in the container) is more reliable since containers may not have `duckduckgo-search` installed. Consider a host-side implementation similar to `_exec_wg_cmd_host`.

---

## 4. Web Fetch API Recommendation

### 4.1 Comparison Table

| Approach | Cost | Reliability | JS rendering | Content quality | Setup complexity |
|----------|------|-------------|-------------|----------------|-----------------|
| **httpx + trafilatura** | Free (OSS) | High for static sites, no JS | No | Excellent extraction | Low (pip install) |
| **Jina Reader API** | Free tier available | High (hosted service) | Yes (server-side) | Very good (LLM-optimized) | Zero (HTTP API) |
| **Firecrawl** | $16+/mo or self-host | Very high | Yes | Excellent (markdown output) | Medium (API key or Docker) |
| **Playwright/Puppeteer** | Free (OSS) | Very high | Yes (full browser) | Raw HTML (needs post-processing) | High (browser install) |
| **curl/wget** | Free | Low (no JS, blocked by bots) | No | Raw HTML | Zero |

### 4.2 Recommendation: **Jina Reader API (primary) + httpx+trafilatura (fallback)**

**Primary: Jina Reader API (`r.jina.ai`)**

Rationale:
- **Zero setup** — just prefix any URL with `https://r.jina.ai/`
- **No API key required** for basic usage
- **Handles JavaScript rendering** server-side
- **Returns LLM-optimized markdown** — no post-processing needed
- **Handles anti-bot sites** better than raw HTTP (renders in real browser)
- **Free tier** sufficient for benchmark usage

Implementation is trivial:
```python
# Inside the container:
curl -s "https://r.jina.ai/https://docs.python.org/3/library/asyncio.html"
# Returns clean markdown of the page content
```

**Fallback: httpx + trafilatura (local, no network dependency)**

For cases where Jina is unavailable or the page is simple:
- **trafilatura** is the best-in-class open-source HTML→text extractor
- Outperforms all competitors in benchmarks (precision + recall)
- Used by HuggingFace, IBM, Microsoft Research
- Pure Python, no external services needed
- Handles most static pages well

**Why not others:**
- **Firecrawl**: Excellent but adds cost ($16+/mo) or self-hosting complexity. Overkill for occasional page fetches during benchmark tasks.
- **Playwright**: Full browser rendering is powerful but requires installing Chromium in the container (~400MB), significantly increasing container size and setup time. Overkill for TB tasks.
- **curl/wget**: Too basic — returns raw HTML, gets blocked by many sites, no content extraction.

### 4.3 Implementation Sketch

```python
WEB_FETCH_TOOL = {
    "type": "function",
    "function": {
        "name": "web_fetch",
        "description": (
            "Fetch the content of a web page and return it as clean markdown text. "
            "Use for reading documentation, API references, tutorials, etc."
        ),
        "parameters": {
            "type": "object",
            "required": ["url"],
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch.",
                },
                "prompt": {
                    "type": "string",
                    "description": (
                        "Optional: what information to extract from the page. "
                        "If provided, only relevant content is returned."
                    ),
                },
            },
        },
    },
}

async def _exec_web_fetch(env: BaseEnvironment, args: dict) -> str:
    """Fetch a URL using Jina Reader API, fall back to trafilatura."""
    url = args["url"]
    jina_url = f"https://r.jina.ai/{url}"

    # Try Jina Reader first (handles JS, returns markdown)
    result = await env.exec(
        command=f"curl -sL --max-time 20 {shlex.quote(jina_url)}",
        timeout_sec=30,
    )
    if result.return_code == 0 and result.stdout and len(result.stdout.strip()) > 100:
        content = result.stdout
        # Truncate if too long
        if len(content) > 30000:
            content = content[:30000] + "\n\n[... truncated]"
        return content

    # Fallback: trafilatura (if installed in container)
    cmd = (
        f"python3 -c \""
        f"import trafilatura;"
        f"downloaded = trafilatura.fetch_url({shlex.quote(url)});"
        f"print(trafilatura.extract(downloaded) or '(no content extracted)')"
        f"\""
    )
    result = await env.exec(command=cmd, timeout_sec=30)
    if result.return_code == 0 and result.stdout:
        return result.stdout

    # Last resort: raw curl
    result = await env.exec(
        command=f"curl -sL --max-time 15 {shlex.quote(url)} | head -500",
        timeout_sec=20,
    )
    return result.stdout or f"(failed to fetch {url})"
```

**Note on `prompt` parameter:** Claude Code's WebFetch processes content through an AI model with the prompt. For the TB adapter, we can either:
1. Skip this (just return raw markdown — simpler, no extra LLM call)
2. Implement it by sending the extracted content through the same litellm model with the prompt (adds latency + cost)

Recommendation: **Skip the AI processing for now.** Return the full page markdown. The calling agent can extract what it needs from the raw content. This avoids a nested LLM call and keeps the tool fast.

---

## 5. Implementation Plan

### Phase 1: Web Tools (CRITICAL — implement first)

**Task: `impl-tb-web-tools`** (downstream consumer)

Files to modify: `terminal-bench/wg/adapter.py`

1. **Add `WEB_SEARCH_TOOL` schema** (after existing tool definitions, ~line 420)
2. **Add `WEB_FETCH_TOOL` schema** (after web_search)
3. **Implement `_exec_web_search`** — host-side DuckDuckGo search via `duckduckgo-search` library
   - Install on host: `pip install duckduckgo-search`
   - Run search on host (not in container) to avoid dependency issues
   - Return structured results: title, URL, snippet
4. **Implement `_exec_web_fetch`** — Jina Reader primary, trafilatura fallback
   - Use `curl` to Jina Reader API (works from any environment)
   - Fallback to trafilatura if Jina fails
   - Truncate output to ~30K chars to avoid context overflow
5. **Add both tools to all condition tool lists** (A through F)
   - These are information tools, not coordination tools — all conditions should have them
6. **Wire into `execute_tool` dispatcher** (~line 558)
7. **Test** with a sample TB task that requires documentation lookup

Dependencies:
- `pip install duckduckgo-search` on the host machine
- No container modifications needed (Jina Reader is accessed via curl)

### Phase 2: Tool Quality Improvements (NICE-TO-HAVE)

**Task: `impl-tb-tool-parity`** (downstream consumer)

1. **Improve `grep` tool** — add context lines (`-A`/`-B`/`-C`), file type filter, output mode
2. **Add `todo_write` tool** (optional) — simple in-memory checklist for agent planning
3. **Improve `glob` tool** — use `find` with proper glob support instead of path match

### Phase 3: Validation

Run a subset of TB tasks (5–10) with web tools enabled vs disabled:
- Pick tasks where documentation lookup would help (library usage, API tasks)
- Compare pass rates
- Measure token overhead from web content in context

---

## 6. Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| DuckDuckGo blocks automated queries | Medium | Web search unavailable | Brave Search API as fallback ($5/mo credits) |
| Jina Reader rate-limited or down | Low | Web fetch degraded | trafilatura local fallback |
| Web content floods context window | Medium | Agent loses focus | Truncate to 30K chars; consider summarization |
| TB leaderboard rules prohibit web access | Low | Tools unusable | TB2 docs confirm internet access is allowed (Constraint 5 in compliance audit) |
| Search results are stale/wrong | Medium | Agent acts on bad info | Agent already has bash for verification; search is advisory, not authoritative |

---

## 7. Summary

**Current state:** TB adapter agents have 6 core tools (Condition A) or 15 tools (Conditions B–F). They lack web search and web fetch capabilities that Claude Code agents have.

**Critical gaps:** `web_search` and `web_fetch` — these are the only two tools that meaningfully impact task success rates. All other missing Claude Code tools are either irrelevant to the benchmark context or can be simulated with existing tools.

**Recommendation:**
- **Web search:** DuckDuckGo (free, no API key) + Brave Search (paid fallback)
- **Web fetch:** Jina Reader API (free, zero setup, JS rendering) + trafilatura (local fallback)

**Expected impact:** Tasks requiring documentation lookup (API usage, library versions, error resolution) should see improved pass rates. The intervention is low-risk since TB2 allows internet access and the tools add no mandatory overhead — agents use them only when needed.
