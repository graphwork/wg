# Federation Study 2/6 — Current-State Re-Baseline

**Task:** `fed-baseline` (wave 1, task 2 of 6 — gather phase)
**Date:** 2026-06-24
**Baseline ref:** current `main` (this worktree branched from `f258d5f5`)
**Re-baselines:** Luca Pinello's gap analysis (`poietic-pbc/poietic-family-team`
`docs/02-workgraph-gap-analysis.md`, R1–R38, dated **2026-04-30**, ~8 weeks stale)

> **Purpose.** Honest "where we are *today*" for the key-based-P2P social-network
> vision (see the [wg-social-network-vision] north star). Every capability below
> is cited to a real `file:line` on current `main`. Where the April snapshot and
> today disagree, the delta is called out explicitly.

---

## 0. TL;DR — the one-paragraph truth

WG today has **no key-based federation primitives whatsoever**. What it *does*
have is (a) a **filesystem-local agency-primitive transfer** tool
(`src/federation.rs` `transfer()`), (b) **cross-repo task-reference resolution**
over a Unix-domain socket or a direct `graph.jsonl` read between peers that share
a filesystem (`resolve_remote_task_status`), (c) a **content-hash identity model**
for agents (SHA-256 of *content*, **not** a signing keypair), (d) a **single-graph,
task-keyed message queue** (`src/messages.rs`), and (e) a **credential store**
that holds API keys (`src/secret.rs`) — but holds **no identity keypairs and signs
nothing**. The four pillars of the key-based-P2P vision — *identity keys, signed
messages, cross-WG addressing, portable signed state* — are **all absent**,
confirmed below. Critically, the **federation/identity/messaging substrate has
barely moved since 2026-04-30**: the only material delta is `impl-agency-schema-fields`
(2026-05-04) firming up the human-as-agent data fields. So Luca's federation/identity
R-rows are **largely still accurate**; this document records the few deltas.

---

## 1. Method & scope

Audited subsystems (with the modules that actually implement them):

| Subsystem | Primary module(s) | LoC |
|---|---|---|
| Federation / peers | `src/federation.rs`, `src/commands/peer.rs` | 2049 + ~340 |
| Identity (agents) | `src/agency/types.rs`, `src/agency/hash.rs`, `src/commands/agent_crud.rs` | 1108 + 85 + ~800 |
| Messaging | `src/messages.rs` | 1550 |
| Keys / secrets | `src/secret.rs` | ~600 |
| Channels (notify) | `src/notify/*.rs` (telegram, matrix, email, discord, slack, sms, voice, webhook, push) | — |

Two **design documents** also exist and are easy to mistake for shipped code —
treated here as *aspiration, not capability*:

- `docs/design/federation-architecture.md` (written 2026-03-25) — discovery,
  layered transport, an identity model, and §5.3 "SSH Keys as Identity (Future)".
  Most of §2–§9 is **unimplemented**; §1.1 accurately describes what exists.
- `docs/ADR-actor-vs-agent-identity.md` ("Accepted", 2026-02-13) — the decision
  that collapsed the old `Actor` node into the agency `Agent`. This *is* shipped
  and is the foundation of the current identity model.

---

## 2. Subsystem audit (cited to current `main`)

### 2.1 Federation — `src/federation.rs`

**What it actually is: a local-filesystem agency-primitive sync + cross-repo
task-status reader.** Not a network protocol, not identity/message transport.

**(a) Named remotes & peers — `.wg/federation.yaml`.**
- `FederationConfig { remotes: BTreeMap<String, Remote>, peers: BTreeMap<String, PeerConfig> }`
  — `src/federation.rs:39`. A `Remote` is `{ path, description, last_sync }`
  (`:21`); a `PeerConfig` is `{ path, description }` (`:31`). **Both address peers
  by filesystem path only** — there is no URL, host, or key field anywhere.
- Config load/save are plain YAML file I/O: `load_federation_config` (`:49`),
  `save_federation_config` (`:60`), `touch_remote_sync` (`:84`).

**(b) Primitive transfer — `transfer()` (`src/federation.rs:637`).**
- The core push/pull engine; "both `wg agency pull` and `wg agency push` are thin
  wrappers" (`:1`). Moves agency **primitives** (components, outcomes, tradeoffs),
  **cache** entries (roles, agents) and **evaluations** between two `LocalStore`s
  (`:631`–`:636`).
- `TransferSummary` (`:136`) counts added/updated/skipped/**access_denied** per
  entity type — i.e. transfer is the *only* place AccessPolicy is enforced.
- `EntityFilter` (`:95`) and `TransferOptions` (`:106`) gate which entity types /
  ids move and whether performance data and evaluation JSON come along.

**(c) AccessPolicy — `src/agency/types.rs:49`, enforced at `src/federation.rs:627`.**
- `enum AccessPolicy { Private, Shared, Open }` (`types.rs:49`); a primitive's
  `AccessControl { owner: String, policy }` defaults to `owner="local",
  policy=Open` (`types.rs:62`).
- `access_policy_allows_transfer()` (`federation.rs:627`) is literally
  `!matches!(policy, Private)` — **Private never leaves; Shared and Open both
  transfer identically.** The doc-comment is explicit: the `shared_peers` list is
  "advisory metadata; enforcement of per-peer restrictions is a **future
  extension**" (`:621`–`:626`). **There is no per-peer ACL, no recipient identity,
  no encryption.**

**(d) Cross-repo task references — `peer:task-id`.**
- `parse_remote_ref()` (`:418`) splits `peer:task` on the first colon (local task
  ids are colon-free, so the delimiter is unambiguous).
- `resolve_remote_task_status()` (`:458`) resolves the peer (`resolve_peer`, `:301`
  — name→path→`.wg/`), checks whether the peer's service is alive
  (`check_peer_service` reads `<peer>/service/state.json` + `is_process_alive`,
  `:356`), then:
  1. if alive: **IPC** over the peer's **Unix-domain socket** — `query_task_via_ipc`
     sends `{"QueryTask":{...}}` and reads one JSON line (`:545`, `#[cfg(unix)]`
     only — `:594` bails on non-Unix);
  2. else: **direct read** of the peer's `graph.jsonl` via `load_graph` (`:510`).
- `RemoteResolution { Ipc, DirectFileAccess, Unreachable(String) }` (`:441`)
  records which path served the answer.
- **This is read-only and same-filesystem.** Both the socket and the `graph.jsonl`
  fallback require the peer's `.wg/` to be reachable as a local path. There is **no
  remote host, no auth, no wire protocol beyond a localhost Unix socket.**

**(e) `wg peer` CLI — `src/commands/peer.rs`.** `add`/`remove`/`list`/`show`/`status`
(`run_add` `:8`, `run_remove` `:64`, `run_list` `:78`, `run_show` `:124`,
`run_status` `:212`). Surfaces each peer's path + whether its service process is
running. No identity, no key exchange, no trust.

> **Federation verdict:** a *cooperating-checkouts-on-one-machine* tool — share
> learned agency primitives and peek at a sibling repo's task status. It is not,
> and was never built as, identity/message federation. Matches April's §1.1.

---

### 2.2 Identity — `src/agency/types.rs`, `src/agency/hash.rs`, `src/commands/agent_crud.rs`

**What it is: a content-addressed `Agent` record. The IDs are SHA-256 *content
hashes*, NOT cryptographic keypairs — there is no signing key anywhere in the
identity model.**

**(a) The `Agent` struct — `src/agency/types.rs:505`.** First-class entity, "a role
paired with a trade-off configuration", stored at `cache/agents/{hash}.yaml`
(`:500`–`:503`). Operational fields relevant to federation/identity:
- `id: String` (`:506`) — the content hash (below).
- `name`, `role_id`, `tradeoff_id` (`:507`–`:510`).
- `trust_level: TrustLevel` (`:521`) — `Verified | Provisional | Unknown`
  (`src/graph.rs:1920`, default `Provisional`).
- `contact: Option<String>` (`:523`) — **free-text only.** Consumed solely by
  `wg agent show/list` for display (`agent_crud.rs:286`, `:346`); **not routed to
  any channel** (grep of `src/notify/`, `src/messages.rs`, `src/dispatch/` for
  `agent.contact` / `.contact` returns no routing site). This is the would-be
  multi-channel binding (R16/R19/R22) and today it is an inert string.
- `executor: String` (`:528`) — distinguishes human vs AI operators.
- `preferred_model` / `preferred_provider` (`:531`/`:535`), `capabilities`, `rate`,
  `capacity`, `deployment_history`, `attractor_weight`, `staleness_flags`.

**(b) Content-hash IDs — `src/agency/hash.rs`.**
- `content_hash_agent(role_id, tradeoff_id)` (`hash.rs:70`) = `SHA-256` of a YAML
  envelope of `{role_id, motivation_id}` (`:77`–`:83`). So **agent identity is a
  deterministic function of *what the agent is*, not of a key it holds.** Two
  hosts that independently build the same role+tradeoff produce the *same* id —
  good for content dedup, **useless as a self-certifying credential** (anyone can
  recompute it; nothing is signed).
- Human agents without role/tradeoff get `SHA-256("human-agent:{name}:{executor}")`
  (`agent_crud.rs:94`–`:98`). Same property: a name+executor string, not a key.

**(c) Human-as-agent — `is_human()` / `is_human_executor()`.**
- `const HUMAN_EXECUTORS = ["matrix", "email", "shell"]` (`types.rs:549`);
  `is_human_executor()` (`:552`) and `Agent::is_human()` (`:561`) classify by
  executor string. This is the data model behind R9–R14 and it **did land/firm up
  since April** (see §5).
- The unifying ADR (`docs/ADR-actor-vs-agent-identity.md`) maps the old
  `Actor.matrix_user_id → Agent.contact (generalized)` and `actor_type →
  Agent.executor` — i.e. the human-binding fields exist but, per (a), `contact`
  is not yet wired to delivery.

**(d) `wg agent` CRUD — `src/commands/agent_crud.rs`.** create / list / show /
delete / update over the `cache/agents/*.yaml` files. Identity is **local to one
`.wg/`**; agents move between stores only via `transfer()` (§2.1b), which copies
the YAML — there is no portable, signed, self-verifying identity artifact.

> **Identity verdict:** content-addressed, host-local, unsigned. The pieces the
> vision needs (a keypair, `pubkey == identity == address`, signed state) are
> **entirely absent**.

---

### 2.3 Messaging — `src/messages.rs`

**What it is: a single-graph, task-keyed, append-only JSONL queue with delivery
status + per-agent read cursors. No cross-WG addressing, no signing, no
encryption.**

- **Storage:** `.wg/messages/{task-id}.jsonl`; cursors at
  `.wg/messages/.cursors/{agent-id}.{task-id}` (`:3`–`:4`, `message_file` `:75`,
  `cursor_file` `:85`). **Messages are keyed by *task*, scoped to one `.wg/`** —
  there is no addressing to an agent across graphs, no peer/host component.
- **`Message`** (`:44`): `{ id: u64 (monotonic per task), timestamp, sender:
  String, body, priority, status, read_at }`. `sender` is a **free-form string**
  — `"user" | "coordinator" | agent-id | task-id` (`:49`). **No `from`/`to`
  keypair, no signature field, no nonce.** Anyone who can write the file can forge
  any `sender`.
- **Delivery state machine:** `DeliveryStatus { Sent, Delivered, Read,
  Acknowledged }` (`:19`); transitions via `update_message_statuses` (`:306`),
  ordered by `status_rank` (`:363`).
- **Concurrency:** exclusive `flock(LOCK_EX)` around id-assignment + append
  (`send_message` `:94`, lock at `:115`–`:120`). Per-agent read cursor via
  `read_cursor`/`write_cursor` (`:373`/`:394`); `read_unread`/`poll_messages`
  (`:420`/`:440`).
- **Channel bridge (outbound to a running agent):** `trait MessageAdapter`
  (`:507`) with `deliver()`; `ClaudeMessageAdapter`/`CodexMessageAdapter`/
  `ShellMessageAdapter` (`:570`/`:594`/`:616`); `adapter_for_executor` (`:924`)
  and `deliver_message` (`:941`) store-then-deliver. **These deliver into the
  *local* agent's prompt/stdin — they do not cross a network or a graph boundary.**

> **Messaging verdict:** solid local async mailbox ("speed of email" within one
> graph), but single-graph and unauthenticated. The vision's *signed events,
> store-and-forward by relays, cross-WG addressing* are all absent.

---

### 2.4 Keys / secrets — `src/secret.rs`

**What it is: an API-key credential store. It stores no identity keypairs and
performs no signing or verification.**

- **Backends:** `enum Backend { Keyring, Keystore, Plaintext }` (`:31`) — OS
  keyring (default, with file-keystore fallback when unreachable, `:43`), explicit
  `~/.wg/keystore/<name>` at 0600/0700, or plaintext (gated by
  `allow_plaintext`, `:75`).
- **`api_key_ref` URI resolution** (`:3`–`:13`, first hit wins): `literal:` /
  `op://` / `pass:` / `keyring:` / `keystore:` / `env:` / `plain:`. These resolve
  **secrets used to authenticate to LLM providers** — they are *consumed*, never
  used to *sign* anything.
- **Crypto inventory (whole repo).** `Cargo.toml` pulls only `sha2` (content
  hashing), `rustls-tls` (HTTPS transport inside `reqwest`), and `keyring` (OS
  credential store). **No `ed25519`, `secp256k1`, `ring`, `libsodium`/`nacl`,
  `x25519`, `noise`, `libp2p`, `nostr`, `ssh-key`, or PGP crate.** The only
  `private_key` symbol in the tree is `vapid_private_key` in `src/notify/push.rs:37`
  — a Web-Push VAPID key for browser push, unrelated to identity.

> **Secrets verdict:** there is **no signing/keypair infrastructure of any kind.**
> Confirmed by both the module audit and a tree-wide crypto-crate grep.

---

### 2.5 Channels — `src/notify/*.rs`

**What it is: a fan-out notification layer with one inbound listener per supported
transport. Bidirectional for Telegram/Matrix; single-bot (no per-agent persona)
on `main`.**

- **`trait NotificationChannel`** (`src/notify/mod.rs:120`): `send_text` (`:125`),
  `send_rich` (`:128`), `send_with_actions` (`:131`), and **`listen()` →
  `Receiver<IncomingMessage>`** (`:145`) for inbound.
- **Telegram** (`src/notify/telegram.rs`): outbound send (`:121`) + **inbound
  long-poll** `getUpdates` listener (`:185`–`:296`) yielding `IncomingMessage`s
  with reply-to threading. Config is **a single `{ bot_token, chat_id }`**
  (`TelegramConfig` `:25`) — **one bot for the whole instance.**
- **Matrix** (`src/notify/matrix.rs`): `NotificationChannel` over `matrix_lite`;
  send works, but `listen()` bails ("use `MatrixListener` directly", `:112`–`:116`)
  — inbound is driven elsewhere.
- Also present: email, discord, slack, sms, voice, webhook, push (each a
  `NotificationChannel`). Dispatch/fan-out + escalation in `src/notify/dispatch.rs`.
- **Per-agent persona (R16/R19) is NOT on `main`.** Multi-bot Telegram ("one bot
  per persistent named agent") is **PR #37 by lucapinello, still OPEN / unmerged**
  (`gh pr view 37` → `state: OPEN, mergedAt: null`; triaged "merge-now, serialize
  after #31" in `docs/pr-triage-2026-06-24.md:31`). **This is the most important
  correction to the April baseline:** the gap analysis treated R16/R19 as
  satisfied *by that PR*, but on `main` today they are still **Missing** — they
  live in an unmerged branch.

> **Channels verdict:** a usable inbound/outbound transport for human notification
> and (Telegram) two-way chat, but single-persona on `main`. It is a *notification*
> layer, not an authenticated federation transport.

---

## 3. Present-vs-Missing for key-based-P2P federation

The vision's four load-bearing pillars ([wg-social-network-vision]), checked
against current `main`:

| Pillar | Status | Evidence on `main` |
|---|---|---|
| **Identity keys** (each human/agent has a keypair; `pubkey == identity == self-certifying address`) | **ABSENT** | IDs are SHA-256 *content* hashes (`hash.rs:70`, `agent_crud.rs:96`), not keypairs. No `ed25519`/`secp256k1`/`ssh-key` crate (`Cargo.toml`). `secret.rs` stores API keys, not identity keys. |
| **Signed messages** (messages = signed events, integrity + authenticity) | **ABSENT** | `Message` has no signature/nonce; `sender` is a forgeable free-text string (`messages.rs:49`). No signing code anywhere. |
| **Cross-WG addressing** (route to an identity/message across independently-owned WGs over a network) | **ABSENT** | Peers addressed by **filesystem path** (`PeerConfig.path` `federation.rs:31`); cross-repo resolution is localhost Unix socket **or** direct file read (`:545`/`:510`), same-machine only. Messages are single-graph, task-keyed (`messages.rs:75`). No URL/DID/host/relay concept in code. |
| **Portable signed state** (downloadable identity = public-identity + signed-state artifact, bound to a key, NOT the signing key) | **ABSENT** | Agents move only as plaintext YAML copied by `transfer()` (`federation.rs:637`); nothing is signed or content-bound to a key. (Memory of past sessions / loadable hidden state is also unbuilt — orthogonal R2.) |

**All four pillars confirmed absent.** This matches the task's stated expectation
("all currently absent; confirm") and is unchanged since April.

**Closest existing building blocks** (what a future design can stand on, none of
them cryptographic):
- Content-addressing habit + a real SHA-256 path (`hash.rs`) — a model for
  content-bound (but unsigned) artifacts.
- An `AccessPolicy` enum + a single enforcement point (`federation.rs:627`) — the
  obvious hook for a future per-recipient/encrypted ACL (R24).
- A peer registry + resolution indirection (`federation.yaml`, `resolve_peer`) —
  the obvious hook to swap path-addressing for key/URL-addressing.
- A working inbound/outbound transport with an `IncomingMessage` abstraction
  (`notify/mod.rs:145`) — a candidate carrier for signed events.
- A credential store with pluggable backends + passthrough URIs (`secret.rs`) — a
  natural home for a host-held *signing* key (custody crux) if one is ever added.

---

## 4. Re-baselined R-rows (federation/identity-relevant, vs current `main`)

Status is **TODAY on `main`**, not the April snapshot. "Δ since Apr-30" notes what
moved. (R-row titles paraphrase Luca's gap analysis; full text in the private
`poietic-family-team` repo.)

| R# | Topic | April (per gap analysis) | **TODAY on `main`** | Evidence | Δ since Apr-30 |
|---|---|---|---|---|---|
| **R1** | Identity = cryptographic key (`pubkey` is the identity) | Missing | **Missing** | Content-hash IDs only (`hash.rs:70`, `agent_crud.rs:96`); no keypair crate (`Cargo.toml`) | none |
| **R9** | Human modeled as a first-class agent | Partial | **Supported (data model)** | `HUMAN_EXECUTORS` + `is_human()` (`types.rs:549`,`:561`); ADR collapsed Actor→Agent | **firmed up** via `impl-agency-schema-fields` (2026-05-04) |
| **R10** | Human agent has trust level | Partial | **Supported** | `Agent.trust_level: TrustLevel` (`types.rs:521`; enum `graph.rs:1920`) | schema-fields landed |
| **R11** | Human agent reachable via a contact handle | Partial | **Partial (stored, not routed)** | `Agent.contact` exists (`types.rs:523`) but is display-only (`agent_crud.rs:286/346`); no delivery routing | field solidified; **wiring still missing** |
| **R12** | Human vs AI distinguished operationally | Partial | **Supported** | `executor` field + `is_human_executor` (`types.rs:528,:552`) | firmed up |
| **R13** | Unified identity (no Actor/Agent split) | Partial | **Supported** | ADR "Accepted" (`docs/ADR-actor-vs-agent-identity.md`); single `Agent` | predates Apr (Feb-13); stable |
| **R14** | Human can act/be-assigned like an agent | Partial | **Partial** | Human agents assignable; delivery to humans only via notify channels, not `contact` | no material change |
| **R16** | Multi-channel persona (per-agent bot/persona) | (Luca: "PR #37") | **Missing on `main`** | `TelegramConfig` is single `{bot_token,chat_id}` (`telegram.rs:25`); PR #37 **OPEN/unmerged** (`gh pr view 37`) | **CORRECTION: still not merged** |
| **R19** | Distinct outbound persona identity per agent | (Luca: "PR #37") | **Missing on `main`** | same as R16; single-bot fan-out (`notify/dispatch.rs`) | **CORRECTION: still not merged** |
| **R22** | Identity↔channel bindings (one identity, many transports) | Missing | **Missing** | `contact` is one free-text string, not routed; no binding table | none (PR #37 would only partially address) |
| **R23** | Cross-host identity portability | Missing | **Missing** | Agents copied as plaintext YAML by `transfer()` (`federation.rs:637`); no signed/portable identity artifact | none |
| **R24** | Per-recipient ACL / privacy (encryption layer) | Missing | **Missing** | `AccessPolicy` is 3-state, `shared_peers` "advisory… future" (`federation.rs:621-628`); no encryption | none (hook exists, unimplemented) |
| **R25** | Merge semantics across federated stores | Partial | **Partial** | `transfer()` merges performance / skips identical / merges-vs-overwrites (`force`) (`federation.rs:637`, `TransferSummary:136`) | no change since terminology renames |
| **R26** | Dedup of federated entities | Partial | **Partial (content-hash dedup)** | Identical-content ids collide → `*_skipped` counters (`federation.rs`, tests `transfer_skips_identical`) | stable |
| **R27** | Write authority / who may mutate a remote | Missing | **Missing** | Cross-repo path is **read-only** (`RemoteResolution`, IPC `QueryTask` only, `federation.rs:441,:545`); no remote-write/auth | none (design doc §6 "read-first" is aspirational) |
| **R33** | Agent-to-agent messaging | Partial | **Partial (single-graph)** | `wg msg` task-keyed JSONL + adapters (`messages.rs:44,:507`); no cross-graph addressing or signing | `fix-chat-tasks` (May-3) touched msg plumbing; capability unchanged |
| **R36** | Federation across independently-owned WGs | Partial→Missing | **Missing (for the vision); Partial (local-only)** | Only same-filesystem primitive transfer + read-only task peek (`federation.rs`); no network/identity/auth | none |

**Scorecard delta (federation/identity rows only).** April leaned heavily on
"Partial". Today, the **human-as-agent data model (R9/R10/R12/R13) has crossed
into Supported** thanks to `impl-agency-schema-fields`, while **R16/R19 regressed
relative to the April write-up's optimism** because the PR that would satisfy them
(#37) is **still unmerged**. Everything genuinely key-based (R1, R22, R23, R24,
R27, R36-for-the-vision) remains **Missing**, exactly as in April.

---

## 5. What changed since 2026-04-30 (deltas, with git evidence)

The headline: **the federation/identity/messaging substrate is essentially
frozen since the gap analysis.** Git history on the key files since 2026-04-30:

- `src/federation.rs` — last functional touch **2026-05-14** (`0d9521e0`
  *sync-wg-terminology-prompt-help*) + **2026-06-16** (`664a3ada`
  *rename-cargo-package*). **Both are renames/terminology, not behavior.**
- `src/agency/types.rs` — `impl-agency-schema-fields` (**2026-05-04**, `67a4620b`)
  added/aligned agency schema fields (the human-as-agent trust/contact/executor
  surface among them; +220 lines, new `tests/agency_schema_fields.rs`). Then
  terminology renames (motivation→tradeoff). **This is the only substantive
  delta affecting the R-rows (R9–R13).**
- `src/messages.rs` — last touch `fix-chat-tasks` (**2026-05-03**, `f5f05b61`);
  plumbing only, no new messaging capability.
- `src/notify/*` — no merged multi-bot/persona change; PR #37 remains open.

**Where the project's energy actually went since April** (for context — *not*
federation): pi-plugin integration, handler-first model specs, chat agents, TUI
performance, PR-triage/merge waves. None of it touches the key-based-P2P surface.

**Net:** Luca's 2026-04-30 federation/identity rows can be trusted almost verbatim
today. The two corrections a reader must apply are: **(1)** human-as-agent fields
are now *Supported* not merely *Partial* (R9/R10/R12/R13); **(2)** R16/R19 are
**not** delivered on `main` (PR #37 still open), contrary to the April
write-up's framing.

---

## 6. Handoff notes for the rest of the study

- **For `fed-prior-art` (1/6):** the closest-fit prior art the vision names is
  Nostr (key=identity / relays / signed events). WG has *zero* of those three;
  it has content-addressing (IPFS-adjacent) and a credential store. Map prior art
  onto the empty pillars in §3, not onto `federation.rs` (which solves a different
  problem).
- **For `fed-requirements` (3/6):** the **agent-key-custody crux** is wide open —
  WG has a backend-pluggable secret store (`secret.rs`) that is the natural home
  for a *host-held signing key*, but nothing signs today. R22 (bindings) needs
  `contact` promoted from a display string to a routed binding table.
- **For `fed-architectures` (4/6):** the two real extension hooks are
  (a) `federation.yaml` peer addressing (swap path → key/URL) and
  (b) `AccessPolicy` + `shared_peers` (the per-recipient/encrypted ACL hook). The
  existing `docs/design/federation-architecture.md` is a *path-based read-first*
  design — a useful contrast point, but it is **not** the key-based-P2P direction
  and is mostly unimplemented.
- **For `fed-adversarial` (5/6):** today's trust model is "anyone with filesystem
  access can read/write any graph, forge any `sender`, and recompute any agent
  id." That is the threat baseline to improve on.

---

## 7. Validation checklist (this document)

- [x] Every current capability cited to a real `file:line` on current `main`
      (§2; e.g. `federation.rs:637`, `types.rs:505`, `hash.rs:70`, `messages.rs:44`,
      `secret.rs:31`, `notify/mod.rs:145`).
- [x] Present-vs-missing for key-based-P2P federation — identity keys, signed
      messages, cross-WG addressing, portable signed state — **all confirmed
      absent** (§3) with evidence and a tree-wide crypto-crate grep (§2.4).
- [x] Federation/identity R-rows (R1, R9–R14, R16/R19, R22/R23, R24–R27, R33, R36)
      re-baselined against current `main` with deltas (§4), and the few real
      changes since April pinned to commits (§5).
- [x] `docs/federation-study/02-current-state-baseline.md` written.

---

*Cross-refs:* north star [wg-social-network-vision]; `docs/design/federation-architecture.md`
(path-based read-first design, mostly aspirational); `docs/ADR-actor-vs-agent-identity.md`
(shipped Actor→Agent unification); `docs/pr-triage-2026-06-24.md:31` (PR #37 status).
