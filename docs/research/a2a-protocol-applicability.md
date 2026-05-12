# A2A (Agent-to-Agent) Protocol — Applicability to wg

**Date:** 2026-03-04
**Task:** research-a2a-agent

## A2A Protocol Summary

The Agent2Agent (A2A) protocol is an open standard (now under the Linux Foundation) created by Google in April 2025 for inter-agent communication. Version 0.3 was released in late 2025, adding gRPC support, agent card signing, and expanded SDKs (Python, Go, JS, Java, .NET).

### Core Concepts

| Concept | Description |
|---------|-------------|
| **Agent Card** | JSON metadata at `.well-known/agent.json` declaring identity, capabilities, skills, endpoint, and security requirements. Supports digital signatures for trust chains. |
| **Task** | First-class object with server-generated `taskId`, `contextId` for grouping, status lifecycle, message history, and artifacts. Tasks can be blocking or non-blocking. |
| **Message** | Content exchange between client/server agents. Contains `Part` objects (text, files, structured data). Roles: "user" or "agent". |
| **Artifact** | Generated outputs from agent processing. Composed of multiple `Part` objects. Streamed via `TaskArtifactUpdateEvent`. |
| **Streaming** | Server-Sent Events (SSE) with `TaskStatusUpdateEvent` and `TaskArtifactUpdateEvent`. Multiple concurrent streams per task. |
| **Push Notifications** | Webhook-based delivery of status/artifact events to registered endpoints. Per-task configuration. |

### Task States

A2A tasks go through: submitted → working → `COMPLETED` | `FAILED` | `CANCELED` | `REJECTED`, with intermediate states `INPUT_REQUIRED` and `AUTH_REQUIRED` for human-in-the-loop.

### API Surface

11 JSON-RPC 2.0 operations (also available via gRPC and REST):
1. `sendMessage` / `sendStreamingMessage` — initiate interaction
2. `getTask` / `listTasks` — query state
3. `cancelTask` — request cancellation
4. `subscribeToTask` — streaming connection for existing task
5. Push notification CRUD (create/get/list/delete config)
6. `getExtendedAgentCard` — authenticated detailed card

### Security

Supports API key, HTTP Basic/Bearer, OAuth 2.0, OpenID Connect, and mutual TLS. Agents must reject invalid credentials. Error responses avoid information leakage (no distinction between "not found" and "not authorized").

### Protocol Bindings

Three normative bindings: JSON-RPC 2.0, gRPC, and HTTP/REST — all functionally equivalent.

---

## Mapping to wg Concepts

### Task Model Comparison

| Aspect | A2A Task | wg Task |
|--------|----------|----------------|
| **Identity** | Server-generated UUID (`taskId`) | User-provided slug ID |
| **Grouping** | `contextId` (conversation context) | Graph edges (`after`/`before`/`requires`) |
| **Status states** | submitted, working, completed, failed, canceled, rejected, input_required, auth_required | Open, InProgress, Done, Blocked, Failed, Abandoned |
| **Dependencies** | None — tasks are independent RPC calls | First-class: `after`, `before`, `requires` edges + cycles |
| **Assignment** | Implicit (you send to a specific agent endpoint) | Explicit (`assigned` field, agency system with skill matching) |
| **History** | Message array within task | Log entries + message system (`wg msg`) |
| **Artifacts** | Part-based (text, file, structured data), streamed | File paths (strings), recorded via `wg artifact` |
| **Retries** | Client-side (resend message) | Built-in (`retry_count`, `max_retries`) |
| **Cycles** | Not supported | First-class (`cycle_config`, `--max-iterations`, `--converged`) |
| **Verification** | Not in protocol | Built-in (`verify` field, eval system) |
| **Scheduling** | Not in protocol | `not_before`, `delay_after` |

**Key difference:** A2A tasks are flat RPC interactions between two agents. wg tasks are nodes in a dependency graph with rich lifecycle management. A2A has no concept of task dependencies, cycles, or graph-mediated coordination.

### Agent Identity Comparison

| Aspect | A2A Agent Card | wg Agency |
|--------|---------------|-----------------|
| **Discovery** | `.well-known/agent.json` on HTTP | `.wg/agency/{roles,tradeoffs,agents}/*.yaml` |
| **Identity** | Name, description, URL, provider | Role + Tradeoff = Agent (content-hash ID) |
| **Capabilities** | Skills list, supported content types | Capabilities list, skills, trust level |
| **Security** | OAuth/OIDC/mTLS/API keys | Executor-level (claude CLI, matrix, etc.) |
| **Performance** | Not tracked | Evaluation scores, lineage, evolution |
| **Human support** | Not explicit | First-class (human executors: matrix, email, shell) |

**Key difference:** A2A Agent Cards are static capability advertisements for HTTP-accessible services. wg agents are composable identities with evolutionary tracking, performance evaluation, and multi-executor support (AI and human).

### Artifact Comparison

| Aspect | A2A Artifact | wg Artifact |
|--------|-------------|-------------------|
| **Content** | Multi-part: text, files, structured JSON | File paths (references to files on disk) |
| **Streaming** | Yes (SSE events) | No (file written, then path recorded) |
| **Schema** | Protocol-defined Part types | Unstructured (any file) |
| **Transport** | Inline in protocol messages | Filesystem + graph metadata |

### Communication Comparison

| Aspect | A2A | wg |
|--------|-----|-----------|
| **Transport** | HTTP(S) + JSON-RPC/gRPC/REST | Filesystem (YAML/JSON) + CLI |
| **Streaming** | SSE with typed events | JSONL stream events (for TUI/monitoring) |
| **Async** | Push notifications (webhooks) | Coordinator polling + service daemon |
| **Discovery** | HTTP `.well-known/agent.json` | Local agency directory |

---

## Relationship to MCP

MCP (Model Context Protocol) and A2A are **complementary, not competing:**

| | MCP | A2A |
|--|-----|-----|
| **Scope** | Tools and context for a single agent | Communication between agents |
| **Metaphor** | "Giving an agent a toolbox" | "Agents talking to each other" |
| **Transport** | stdio / SSE | HTTP(S) |
| **Use case** | Agent accesses databases, APIs, files | Agent delegates to specialist agent |

**wg's position:** wg already has its own tool system (native executor tools) and its own inter-agent coordination (graph edges, messages, coordinator). MCP would add external tool access. A2A would add external agent interop. These are orthogonal concerns.

---

## Evaluation: Implement, Consume, or Ignore?

### Option 1: IMPLEMENT A2A (expose wg agents as A2A endpoints)

**What this means:** Each wg agent (or the coordinator) exposes an HTTP server implementing the A2A protocol, so external A2A clients can send tasks to wg.

**Pros:**
- External systems could submit work to wg
- Standard discovery via Agent Cards

**Cons:**
- Massive engineering effort: HTTP server, JSON-RPC handler, SSE streaming, auth, agent card serving
- Impedance mismatch: A2A tasks are flat RPCs; wg tasks are graph nodes with dependencies. Translating between them loses wg's core value (graph structure, cycles, dependency tracking).
- wg agents don't have stable HTTP endpoints — they're ephemeral processes spawned by the coordinator
- Security surface: exposing agents over HTTP opens authentication, authorization, and DoS concerns
- Maintenance burden: tracking A2A spec evolution (still pre-1.0)

**Verdict: Not recommended now.** The impedance mismatch between A2A's flat task model and wg's graph-based model is fundamental. Wrapping wg in A2A would strip away everything that makes it useful.

### Option 2: CONSUME A2A (call external A2A agents from wg)

**What this means:** wg tasks could delegate to external A2A-compatible agents (e.g., a specialized coding agent, a search agent, an enterprise knowledge base agent).

**Pros:**
- Enables wg to orchestrate external specialist agents
- Could add a new executor type (`a2a`) alongside `claude`, `matrix`, `email`, `shell`
- Agent Cards provide natural discovery of capabilities
- Non-blocking A2A tasks map to wg's async model (submit, poll/stream, collect artifact)

**Cons:**
- Medium engineering effort: A2A client implementation, agent card discovery, auth handling
- External agents are opaque — harder to verify, evaluate, and debug
- Dependency on network availability and external service reliability
- Still pre-1.0; API may change

**Verdict: Worth considering for the future.** A new `a2a` executor type is the cleanest integration path. It maps well to wg's existing model: the coordinator discovers an A2A agent via its card, sends a message (the task description), polls/streams for completion, and records the artifact. This is structurally identical to how the `claude` executor works today — spawn process, wait for result, record output.

### Option 3: IGNORE A2A

**Verdict: Recommended for now, with a watching brief.** A2A is still pre-1.0 (v0.3) and the ecosystem is nascent. wg's value is in graph-mediated coordination, not HTTP-based agent interop. The protocol should stabilize and demonstrate real adoption before wg invests in integration.

---

## Recommendation

**Short term (now): Ignore.** Focus on wg's core strengths. A2A is pre-1.0, and the impedance mismatch with wg's graph model means integration would be high-effort, low-value today.

**Medium term (when A2A reaches 1.0 and has ecosystem traction): Consume via `a2a` executor.**

### Integration Architecture (if/when consuming)

```
┌─────────────────────────────────────────────┐
│ wg                                    │
│                                              │
│  Coordinator                                 │
│    │                                         │
│    ├── claude executor  → local claude agent  │
│    ├── matrix executor  → Matrix room        │
│    ├── email executor   → email thread       │
│    └── a2a executor     → external A2A agent │
│         │                                    │
│         ├── discover via Agent Card          │
│         ├── sendMessage (task description)   │
│         ├── poll/stream for completion       │
│         └── record artifact from response    │
│                                              │
└─────────────────────────────────────────────┘
```

**Implementation sketch for `a2a` executor:**

1. **Agent Card registry**: `wg config --a2a-agents` or `.wg/a2a-agents.yaml` mapping agent names to card URLs
2. **Discovery**: Fetch `.well-known/agent.json`, cache capabilities
3. **Task dispatch**: `sendMessage` with task description as text Part, optional file Parts for inputs
4. **Status tracking**: `subscribeToTask` (SSE) or polling `getTask` — map A2A states to wg states
5. **Artifact collection**: Extract Parts from A2A artifacts → write to disk → `wg artifact`
6. **Auth**: Support API key and OAuth from agent card's security schemes
7. **Matching**: Map A2A agent card skills to wg task skills for coordinator matching

**State mapping:**

| A2A State | wg Status |
|-----------|-----------------|
| submitted | InProgress |
| working | InProgress |
| input_required | Blocked (+ notify via HITL) |
| completed | Done |
| failed | Failed |
| canceled | Abandoned |
| rejected | Failed (with reason) |

**Estimated effort:** Medium (M). ~200-400 lines for an A2A client executor, plus ~100 lines for agent card discovery/caching. The `reqwest` crate (already a dependency) handles HTTP; `eventsource-client` or similar for SSE. No new architectural changes — it's just another executor alongside claude/matrix/email/shell.

---

## Summary

| Question | Answer |
|----------|--------|
| Should wg IMPLEMENT A2A? | **No.** Impedance mismatch with graph model; ephemeral agents can't serve HTTP. |
| Should wg CONSUME A2A? | **Not yet, but plan for it.** `a2a` executor is the natural integration path. |
| Relationship to MCP? | Complementary. MCP = tools for agents; A2A = agent-to-agent comms. Both are orthogonal to wg's graph coordination. |
| When to act? | When A2A reaches 1.0+ and at least 3-5 external agents exist that would be useful to orchestrate. |
