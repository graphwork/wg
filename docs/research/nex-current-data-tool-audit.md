# Nex Current-Data Tool Audit

Task: `fix-nex-agent`

## Findings

- Normal `wg nex` sessions build `ToolRegistry::default_all_with_config`, which registers `bash`, `web_search`, and `web_fetch` before the agent loop starts. The agent has a direct web-fetch affordance in the default/full tool surface.
- `wg nex --minimal-tools` intentionally strips the tool surface down to local development tools: `read_file`, `edit_file`, `write_file`, `bash`, `grep`, and `glob`. In that mode, `web_search` and `web_fetch` are not exposed, so HTTP requests must go through `bash` with `curl` or `wget`.
- Read-only `wg nex --read-only` applies the read-only tool filter after registry construction. Web search/fetch remain available; arbitrary `bash` is removed because it is not classified as read-only.

## Prompt Fix

The bundled `wg nex` prompt now tells the agent that current real-world data
requires fetching before answering. It directs the agent to:

- use web search/fetch when available,
- use `bash` with `curl` or `wget` when bash is the available HTTP path,
- state the limitation and ask for data or placeholder confirmation when no live-fetch path is available,
- avoid writing code or prose that fabricates current data.

For `--minimal-tools`, the prompt explicitly says `web_search` and `web_fetch`
are unavailable and names bash + curl/wget as the HTTP fallback.

## Recommendation

Keep the existing `web_fetch` tool for full/research surfaces. A future
smaller, more obvious `fetch_url` tool would still be useful for coding-biased
or local models because the name directly matches the desired action. It should
default to HTTP GET only, reject non-HTTP(S) schemes, and respect
user-configurable allowlists or denylists before making network requests.
