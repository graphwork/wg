# Web Search API Comparison for Self-Hosted Agent Tool

**Date:** 2026-03-04
**Context:** The native executor needs a `web_search` tool for research tasks (~188 calls observed, ~10% of task types). This report compares API options.

## Comparison Table

| Feature | **Serper** | **Brave Search** | **Tavily** | **SearXNG** | **Exa** | **Google CSE** |
|---------|-----------|-----------------|-----------|-------------|---------|---------------|
| **Cost/1k queries** | $1.00 (50k tier) — $0.30 (12.5M tier) | $5.00 (Base AI) — $9.00 (Pro AI) | $1.00 (basic) — $2.00 (advanced) | Free (self-hosted) | $5.00–$7.00 | $5.00 |
| **Free tier** | 2,500 queries (one-time) | ~$5/mo credit (~1k queries) | 1,000 credits/mo | Unlimited (self-hosted) | $10 credit (one-time) | 100/day |
| **Rate limits** | 300 req/s | 20 req/s (Base), 50 req/s (Pro) | 100 RPM (free), 1k RPM (paid) | No limit (self-hosted) | Not published | 10k/day |
| **Result source** | Google SERP | Brave's own index | Multiple (AI-curated) | Meta-search (70+ engines) | Neural search (own index) | Google |
| **Result quality (technical)** | Excellent (Google results) | Good | Good (AI-summarized) | Variable (depends on engines) | Good for semantic queries | Excellent |
| **API simplicity** | REST, single endpoint, JSON | REST, JSON, well-documented | REST, JSON, simple | REST, JSON (needs config) | REST, JSON | REST, JSON |
| **Self-host option** | No | No | No | **Yes** (Docker) | No | No |
| **Privacy** | Queries sent to Serper | Queries sent to Brave | Queries sent to Tavily | **Full privacy** | Queries sent to Exa | Queries sent to Google |
| **Credits expire** | 6 months | Monthly | Monthly (no rollover) | N/A | No expiration noted | N/A |
| **Snippet/content** | Snippets + optional scrape | Snippets + summaries | Full answer extraction | Snippets only | Content extraction | Snippets |

## Detailed Notes

### Serper (Recommended for primary API)
- **Cheapest per-query cost** at scale: $0.30–$1.00 per 1k queries
- Returns actual Google SERP results — best quality for technical/code queries
- Simple REST API: single POST to `/search` with JSON body
- Credits last 6 months (no monthly subscription pressure)
- 300 req/s rate limit is extremely generous
- 2,500 free queries to start
- No subscription lock-in — pure pay-as-you-go

### Brave Search API
- Independent index (not Google) — good for diversity but slightly lower quality for niche technical queries
- $5/1k queries is 5–16x more expensive than Serper
- Free tier recently reduced to ~1k queries/month
- Good documentation and MCP server already exists
- Rate limits (20–50 req/s) are adequate for agent workloads

### Tavily
- Designed specifically for AI/agent use cases
- "Advanced" search (2 credits) provides AI-curated summaries
- Credit system with monthly expiration is annoying for bursty workloads
- $1/1k basic queries is competitive, but advanced mode doubles the cost
- 100 RPM free-tier limit could bottleneck parallel agents

### SearXNG (Self-hosted)
- **Best for privacy** — zero data leaves your infrastructure
- **Zero marginal cost** — only hosting costs (~$5–10/mo VPS)
- Aggregates 70+ search engines (Google, Bing, DuckDuckGo, etc.)
- Quality is variable — depends on upstream engine availability and rate limiting
- Requires Docker setup and maintenance
- No API key management needed
- Upstream engines may rate-limit or block the instance over time
- JSON API needs manual enabling in settings

### Exa
- Neural/semantic search — different paradigm from keyword search
- Best for "find pages similar to X" rather than "answer this question"
- $5–7/1k queries is expensive for general search
- Interesting for specific research patterns but not a general replacement

### Google Custom Search Engine
- Official Google results but limited to 10k queries/day
- $5/1k queries — same as Brave
- Requires creating a Custom Search Engine (extra setup)
- The 100 free queries/day are useful for testing only

## Recommendation

### Primary: **Serper**

**Why:**
1. **Cheapest** — $0.30–$1.00/1k vs $5–$9/1k for alternatives
2. **Best result quality** — actual Google SERP results, which are strongest for code/technical queries
3. **Simplest integration** — single REST endpoint, JSON in/out, no SDK needed
4. **No subscription** — pay-as-you-go with 6-month credit expiration
5. **Highest rate limit** — 300 req/s handles any number of parallel agents
6. **Low risk** — 2,500 free queries to validate before spending

**Integration sketch:**
```rust
// POST https://google.serper.dev/search
// Headers: X-API-KEY: <key>, Content-Type: application/json
// Body: {"q": "rust async trait", "num": 10}
// Returns: { organic: [{ title, link, snippet, position }], ... }
```

### Fallback consideration: **SearXNG**

For users who want zero API costs and full privacy, SearXNG is the right choice. However, it adds operational complexity (Docker, maintenance, upstream rate limits) that makes it a poor default. Consider supporting it as an alternative backend behind the same tool interface.

### Suggested architecture

```
web_search tool
  ├── SerperBackend (default, API key required)
  ├── BraveBackend (alternative, API key required)
  ├── TavilyBackend (alternative, API key required)
  └── SearXNGBackend (self-hosted, URL required)
```

Configure via environment variable or wg config:
```
WG_SEARCH_BACKEND=serper
WG_SEARCH_API_KEY=<key>
# or
WG_SEARCH_BACKEND=searxng
WG_SEARXNG_URL=http://localhost:8080
```

### Cost projection

Based on observed usage (188 web_search calls across 1,363 agents, ~0.14 searches/agent):
- At 100 agents/day: ~14 searches/day → **$0.014/day** with Serper ($1/1k tier)
- At 1,000 agents/day: ~140 searches/day → **$0.14/day** with Serper
- Even at 10,000 agents/day: ~1,400 searches/day → **$1.40/day** — negligible

## APIs NOT Recommended

| API | Reason |
|-----|--------|
| **Bing Web Search API** | Retired August 2025. Replacement ("Grounding with Bing") costs $35/1k — absurd. |
| **Google CSE** | 10k/day hard limit + requires Custom Search Engine setup. Not worth the hassle when Serper provides the same Google results cheaper. |
| **Exa** | Semantic search is a different paradigm. Not a drop-in for general web search. Consider as a separate tool if needed. |
