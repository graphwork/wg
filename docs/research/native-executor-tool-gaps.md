# Native Executor Tool Gap Analysis

**Date:** 2026-03-04
**Method:** Parsed stream-json output from 1,363 agents (agent-5000 through agent-6361), totaling 23,129 tool calls.

## Tool Usage Frequency

| Tool | Calls | Pct | Native Executor Has? |
|------|------:|----:|:--------------------:|
| Bash | 10,352 | 44.8% | Yes |
| Read | 5,856 | 25.3% | Yes (`read_file`) |
| Grep | 2,971 | 12.8% | Yes (`grep`) |
| Edit | 2,521 | 10.9% | Yes (`edit_file`) |
| TodoWrite | 777 | 3.4% | No |
| Glob | 190 | 0.8% | Yes (`glob`) |
| WebSearch | 188 | 0.8% | No |
| Write | 185 | 0.8% | Yes (`write_file`) |
| WebFetch | 83 | 0.4% | No |
| TaskOutput | 3 | 0.0% | No |
| Skill | 3 | 0.0% | No |

**Coverage with current native tools: 93.8%** (Bash + Read + Grep + Edit + Glob + Write)

## Bash Command Breakdown

The Bash tool (44.8% of all calls) is used for:

| Category | Calls | Pct of Bash | Notes |
|----------|------:|------------:|-------|
| `wg log` | 1,877 | 18.1% | Already native (`wg_log`) |
| `cargo test` | 1,009 | 9.7% | Needs Bash |
| `cargo build` | 747 | 7.2% | Needs Bash |
| `wg done` | 554 | 5.3% | Already native (`wg_done`) |
| `git diff` | 533 | 5.1% | Needs Bash |
| `grep/rg` | 434 | 4.2% | Partially native (`grep` tool) |
| `wg artifact` | 402 | 3.9% | Already native (`wg_artifact`) |
| `wg agent` | 391 | 3.8% | Agency commands — not native |
| `wg show` | 312 | 3.0% | Already native (`wg_show`) |
| `cargo install` | 261 | 2.5% | Needs Bash |
| `cat/head/tail` | 258 | 2.5% | Covered by native `read_file` |
| `ls` | 255 | 2.5% | Not native (could add `list_dir`) |
| `piped commands` | 252 | 2.4% | Needs Bash |
| `wg msg` | 239 | 2.3% | Not native |
| `git status` | 142 | 1.4% | Needs Bash |
| `wg role/assign/motivation` | 350 | 3.4% | Agency commands — not native |
| `cargo clippy/fmt/check` | 251 | 2.4% | Needs Bash |
| `git add/commit/push/stash` | 236 | 2.3% | Needs Bash |
| `wg add` | 83 | 0.8% | Already native (`wg_add`) |
| `wg list` | 61 | 0.6% | Already native (`wg_list`) |
| Other | ~550 | 5.3% | Mixed (python, find, sed, etc.) |

## Gap Analysis

### Already covered (native tools exist)
1. **File I/O:** `read_file`, `write_file`, `edit_file`, `glob`, `grep` — covers Read, Write, Edit, Glob, Grep
2. **workgraph core:** `wg_show`, `wg_list`, `wg_add`, `wg_done`, `wg_fail`, `wg_log`, `wg_artifact`
3. **Shell execution:** `bash` — covers all cargo, git, and misc commands

### Missing tools (prioritized by impact)

| Priority | Tool | Claude Code Equivalent | Calls Avoided | Effort | Notes |
|:--------:|------|----------------------|:-------------:|:------:|-------|
| 1 | `wg_msg` | Bash `wg msg` | 239 | **S** | Read/send messages. Simple graph operation. Required by task workflow. |
| 2 | `list_dir` | Bash `ls` | 255 | **XS** | Simple `fs::read_dir`. Agents use `ls` constantly. |
| 3 | `wg_context` | Bash `wg context` | 10+ | **S** | Get dependency context for current task. Already exists in CLI. |
| 4 | `wg_quickstart` | Bash `wg quickstart` | 67 | **S** | Orientation at task start. Critical for agent bootstrapping. |
| 5 | `web_search` | WebSearch | 188 | **L** | Requires external API (Google/Brave/Tavily). Research tasks need this. |
| 6 | `web_fetch` | WebFetch | 83 | **M** | HTTP GET + HTML-to-markdown. Useful for docs/API research. |
| 7 | `todo_write` | TodoWrite | 777 | **XS** | In-memory task tracking. Only useful if the model is trained to use it. |
| 8 | `wg_agent` tools | Bash `wg agent/role/...` | 741 | **M** | Agency subsystem. Only assignment tasks use these. |
| 9 | `git` tools | Bash `git *` | ~1,100 | **L** | git status/diff/add/commit. Complex; Bash covers it fine. |
| 10 | `cargo` tools | Bash `cargo *` | ~2,100 | **L** | Build/test/clippy. Complex; Bash covers it fine. |

### Effort Legend
- **XS:** < 1 hour, < 50 lines
- **S:** 1–3 hours, 50–150 lines
- **M:** 3–8 hours, 150–400 lines
- **L:** 8+ hours, 400+ lines, external dependencies

## Recommendations

### The native executor already covers ~94% of tool calls

The current toolset (bash, read_file, write_file, edit_file, glob, grep, wg_*) handles the vast majority of agent work. The Bash tool is a universal fallback that covers cargo, git, and ad-hoc commands.

### Top 5 tools to add for maximum ROI

1. **`list_dir`** (XS effort) — Eliminates ~255 `ls` bash calls. Trivial `fs::read_dir` implementation.

2. **`wg_msg`** (S effort) — Message read/send. Required by the standard agent workflow (check messages before/after work). Currently all agents shell out to `wg msg`.

3. **`wg_context`** (S effort) — Returns task context including dependency artifacts and logs. Agents call `wg context` for orientation. Native version avoids subprocess overhead.

4. **`web_search`** (L effort) — For research tasks. Requires API key and external service integration (Brave Search API, Tavily, or SerpAPI). ~188 calls. Only needed for research-tagged tasks.

5. **`web_fetch`** (M effort) — HTTP fetch with HTML-to-markdown conversion. Pairs with web_search. ~83 calls. Requires an HTML parser (e.g., `scraper` + `html2text` crates).

### Tools NOT worth adding as native

- **`git` tools**: Git operations are complex (staging, diffing, rebasing) and well-served by the Bash tool. The cost of reimplementing git porcelain in Rust far outweighs the subprocess overhead.
- **`cargo` tools**: Same reasoning. `cargo build/test/clippy` are best left to Bash.
- **`todo_write`**: This is a Claude Code UX feature (client-side task tracking). Not meaningful for native executor agents that don't have a TUI.
- **Agency tools** (`wg agent`, `wg role`, etc.): Only used by assignment tasks. These are complex and can stay as Bash calls.

### 80% Coverage Assessment

The native executor can already handle **~94%** of tool calls. Adding `list_dir` and `wg_msg` would push this to ~96%. The remaining ~4% (web search/fetch, piped commands, git/cargo) is well-served by the Bash fallback.

**For 80% of workgraph _tasks_ (not just tool calls):** The current toolset is sufficient for:
- All code implementation tasks (read, edit, write, bash for build/test)
- All code review/evaluation tasks
- All documentation tasks
- All assignment tasks (via bash `wg agent/assign`)

The only task category not well-served is **research tasks requiring web access** (~10% of tasks). Adding `web_search` and `web_fetch` would close this gap.

## Summary Table

| Tool Set | Tool Call Coverage | Task Type Coverage |
|----------|------------------:|-------------------:|
| Current native | 93.8% | ~90% |
| + list_dir, wg_msg | ~96% | ~90% |
| + web_search, web_fetch | ~97% | ~98% |
| Full parity with Claude Code | 100% | 100% |
