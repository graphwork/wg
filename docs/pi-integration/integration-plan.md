# Pi.dev Integration Plan — Executor Handler + Chat/TUI Layer + Light-Touch Patch

**Task:** `pi-design-integration` · **Date:** 2026-06-22
**Status:** **Design only — no production code changed. Reviewable before any implementation starts.**

This plan synthesizes the two upstream research findings into one actionable
integration design covering **both** surfaces — pi as a WG *executor handler*
and pi as a *chat/TUI* layer — plus the minimal upstreamable pi patch, an
assessment of the "pi as WG's default foundation" idea, and a phased,
dependency-aware task breakdown for the next phase.

**Inputs synthesized:**
- [`executor-research.md`](executor-research.md) — `pi-research-executor`: blockers B1–B8, terminal-takeover root cause, `pi:` handler interface sketch, WRAPPER-vs-patch verdict.
- [`model-mgmt-research.md`](model-mgmt-research.md) — `pi-research-models`: pi warm in-process `set_model`/`cycle_model` vs WG cold `SetChatExecutor` respawn, Pi→WG concept map, `wg chat model` + TUI `/model` recommendation, pi-as-front-end feasibility.

**Codebase anchors verified for this plan (this branch):**
- `src/dispatch/plan.rs:48` `enum ExecutorKind`; `:90` `EXTERNAL_CLIS`; `:109` `WORKER_ONLY_EXTERNALS`; `:118` `as_str`; `:136` `from_str`; `:164` `is_external_cli`; `:176` `is_worker_only_external`; `:221` `parse_executor_model_route` (the `openrouter/<vendor>/<model>` normalizer).
- `src/dispatch/handler_for_model.rs:71` `handler_for_model` — external-CLI prefix interception (`is_external_cli()` check at `:83`) means a new `is_external_cli()` kind routes **with no new match arm**.
- `src/commands/opencode_handler.rs:40` `run`; `:273` `opencode_model_arg` (delegates to `worksgood::chat_command::opencode_model_arg`).
- `src/cli.rs:2429` `codex-handler`; `:2451` `opencode-handler` subcommands; `:4917` `wg chat set-executor` (alias `switch`).
- `src/executor_discovery.rs:15` `STABLE_EXTERNAL_EXECUTORS`; `:17` `EXPERIMENTAL_EXTERNAL_EXECUTORS`.
- `src/profile/templates/{claude,codex,nex,opencode}.toml` — starter profile templates.
- `src/commands/service/ipc.rs:602` `SetChatExecutor`; `:1909` `handle_set_coordinator_executor` (persist override → SIGTERM → supervisor respawn).
- `src/nex_runtime.rs` — nex binds model **once** at construction (`resolve_model_for_role(TaskAgent)`).

---

## 0. Executive summary

1. **Integrate pi via a WG-side wrapper/handler — do not patch pi to make it
   work.** The terminal takeover is a launch-flag property on WG's side
   (`resolveAppMode` → `interactive` only when *both* fds are TTYs and no
   `-p`/`--mode`); launching with `--mode rpc` (chat) or `-p`/`--mode json`
   (worker) and piped stdio defeats it entirely (executor-research §1, §4).
2. **Add `ExecutorKind::Pi` as a chat-capable external CLI** — in
   `EXTERNAL_CLIS` but **not** `WORKER_ONLY_EXTERNALS`, exactly like `OpenCode`.
   Routing then comes for free through `handler_for_model`'s external-CLI
   interception; no new match arm (§1).
3. **Ship a `wg pi-handler` peer of `opencode_handler.rs`** with two shapes:
   long-lived `--mode rpc` for live chat, one-shot `-p`/`--mode json` for
   worker dispatch. Reuse the opencode model-normalization, inbox-cursor loop,
   and stdout-is-protocol discipline (§1).
4. **Distinctive value pi unlocks for WG: a warm, mid-chat `set_model` swap for
   a CLI-class handler.** Neither the `claude` nor `codex` CLI can change model
   without a respawn; pi's long-lived RPC process can (`set_model`/`cycle_model`),
   so a pi chat handler gives WG a warm `/model` swap that today only the
   in-process `nex` path could ever offer (§2).
5. **For WG's own chat agent, ship the thin `wg chat model <id> <spec>` verb +
   TUI `/model` picker over the existing `SetChatExecutor` IPC** (no new switch
   engine). Treat the warm in-process `nex` per-turn re-resolve as an optional
   later layer (§2).
6. **The minimal upstreamable pi patch is a default-off `PI_NO_TUI` env switch**
   (~3 lines at the top of `resolveAppMode`). It is **optional / belt-and-
   suspenders** — only worth upstreaming if WG later embeds pi's real TUI
   through a both-TTY PTY and needs an external kill-switch. WG must not block
   on it (§3).
7. **Do NOT rebase WG's default stack around pi.** Adopt pi *additively* for the
   two things it adds that WG lacks — a warm `/model` CLI-handler swap and an
   attachable human TUI front-end — while keeping `claude`/`codex`/`nex` as the
   defaults and `claude:haiku` for agency tasks (§4).

---

## 1. Executor-handler architecture for `pi:`

### 1.1 Where it plugs into WG (type + routing)

`pi` is an **executor name, not a provider prefix** (like `opencode`/`octomind`
— see the `is_external_cli` design note at `plan.rs:80`). It is chat-capable
(RPC), so it mirrors `OpenCode`: an external CLI that is *also* a live chat
handler.

Concrete edits (all in **`src/dispatch/plan.rs`**, the canonical
`ExecutorKind` home):

| Edit | Location | Value |
|---|---|---|
| Add enum variant | `enum ExecutorKind` (`:48`) | `Pi,` with a doc line "Pi Coding Agent CLI. Chat-capable external CLI (`pi --mode rpc`) + one-shot worker (`pi -p`)." |
| Add to `EXTERNAL_CLIS` | `:90` | append `ExecutorKind::Pi` |
| Do **NOT** add to `WORKER_ONLY_EXTERNALS` | `:109` | (pi has a live chat handler, like OpenCode) |
| `as_str` arm | `:118` | `ExecutorKind::Pi => "pi"` |
| `from_str` arm | `:136` | `"pi" => Some(ExecutorKind::Pi)` |

Routing then needs **no change** to `handler_for_model`: the external-CLI
interception at `handler_for_model.rs:82-92` already returns any
`is_external_cli()` kind for a `prefix:rest` spec, so `pi:openrouter/anthropic/
claude-3.5-haiku` resolves to `ExecutorKind::Pi` automatically. Likewise
`parse_executor_model_route` (`plan.rs:221`) already normalizes the
`openrouter/<vendor>/<model>` nested shorthand to `openrouter:<vendor>/<model>`,
so pi inherits OpenRouter route parsing for free.

Keep `pi` **out of `KNOWN_PROVIDERS`** (an executor is not a provider) — same
treatment as `opencode`. The existing `test_opencode_prefix_routes_to_opencode_handler`
pattern in `handler_for_model.rs` is the template for a `test_pi_prefix_routes_to_pi_handler`.

### 1.2 Discovery

pi is an **npm package, not a static binary** (executor-research B6). Treat it
exactly like the other external CLIs in `src/executor_discovery.rs`:

- Add `pi` to `EXPERIMENTAL_EXTERNAL_EXECUTORS` (`executor_discovery.rs:17`)
  initially (promote to `STABLE_EXTERNAL_EXECUTORS` only after the credentialed
  smoke pass in §5). `binary_candidates: &["pi"]`, description
  `"Pi Coding Agent (npm @earendil-works/pi-coding-agent; \`pi --mode rpc\`)"`.
- `wg config lint` should reject a `pi:` route when no `pi` binary is
  discoverable (executor-research §5 item 1) — same mechanism other external
  CLIs already get from discovery.

### 1.3 The `wg pi-handler` subcommand (invocation contract)

Add a `pi-handler` subcommand in `src/cli.rs` (peer of `codex-handler` `:2429`
and `opencode-handler` `:2451`), backed by a new `src/commands/pi_handler.rs`
modeled on `opencode_handler.rs`.

#### Shape A — live chat handler (preferred): `wg pi-handler --chat`

One long-lived pi process per chat, driven over the RPC protocol.

**Spawn (argv):**
```
pi --mode rpc \
   --provider <prov> --model <model> \
   --session-id   <wg-chat-id> \
   --session-dir  <wg-state>/pi-sessions \
   --append-system-prompt <wg-system-prompt-file-or-text> \
   --no-approve
```
**stdio:** `stdin = piped`, `stdout = piped`, `stderr = handler.log`. Piped fds
alone defeat the takeover (neither is a TTY); `--mode rpc` makes it explicit and
robust (executor-research §1.4, B1).

**env (never `--api-key`, per B5):** `PI_CODING_AGENT_DIR`,
`PI_CODING_AGENT_SESSION_DIR`, and the provider key
(`OPENROUTER_API_KEY` / `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `GEMINI_API_KEY`)
resolved through `wg secret`.

**Per WG inbox message** (poll loop identical in shape to `opencode_handler::run`,
`opencode_handler.rs:40`):
1. Write one LF-delimited JSONL command to pi stdin (never `\r\n`-only):
   `{"id":"req-<n>","type":"prompt","message":"<user text>"}`.
2. Read pi stdout JSONL events:
   - `{"type":"response","id":"req-<n>","success":true}` ⇒ accepted.
   - `message_update` with `assistantMessageEvent.type == "text_delta"` ⇒ append
     the delta to WG `streaming_path` (`chat::streaming_path_ref`) for live render.
   - `{"type":"agent_end", ...}` ⇒ **turn complete / idle**.
3. Capture the final reply (either from `agent_end` messages or by sending
   `{"type":"get_last_assistant_text"}` and reading `response.data.text`); write
   to the WG outbox (`chat::append_outbox_ref`); clear `streaming_path`.
4. Cancel = `{"type":"abort"}`. Shutdown = SIGTERM (pi exits 143; registers
   SIGTERM/SIGHUP teardown in rpc/print mode).

**RPC framing caveat (executor-research §3.2):** split records on `\n` **only**.
Use Rust `BufRead::read_until(b'\n')` — a reader that also breaks on
`U+2028`/`U+2029` (Node `readline`) is non-compliant because those bytes occur
inside JSON strings.

#### Shape B — one-shot worker (print/json)

Mirrors the external-CLI worker spawn in `src/commands/spawn/execution.rs`.

```
pi -p \
   --provider <prov> --model <model> \
   --session-id <task-id> --session-dir <wg-state>/pi-sessions \
   --append-system-prompt <wg-system-prompt> --no-approve \
   "<assembled prompt>"          # or feed prompt on piped stdin
```
Use `--mode json` instead of `-p` for a richer JSONL event capture. **stdio:**
stdin piped (prompt or empty), `stdout` = the result captured to
`raw_stream.jsonl` / `output.log` (the claude/codex stdout split,
`execution.rs:~1756`), `stderr` = diagnostics. **Exit code:** `0` ok, nonzero on
error (e.g. `1` = `No API key found for <prov>`) ⇒ WG task failure. Always add a
supervisor timeout (executor-research B2).

### 1.4 Model normalization (`pi:<spec>` → `--provider`/`--model`)

Add `pi_model_arg(model) -> Option<(provider, model)>` in `pi_handler.rs`,
mirroring `opencode_model_arg` (`opencode_handler.rs:273`). Logic:

- `pi:openrouter/anthropic/claude-3.5-haiku` → after the `pi:` prefix the rest
  is `openrouter/anthropic/claude-3.5-haiku`; first `/`-segment = `--provider
  openrouter`, remainder = `--model anthropic/claude-3.5-haiku`. (OpenRouter ids
  verified working in the prior eval report.)
- `pi:anthropic/claude-sonnet-4` → `--provider anthropic --model
  claude-sonnet-4`.
- Custom `baseUrl`/headers/non-env key ⇒ write a temp `~/.pi/agent/models.json`
  (or `--session-dir`-local) provider block (executor-research B4) and pass
  `--provider <name>`; env-key + built-in providers ride CLI flags only (no file).

Keep this in lock-step with `parse_executor_model_route` (`plan.rs:221`) the same
way opencode keeps `opencode_handler::opencode_model_arg` and
`chat_command::opencode_model_arg` in agreement (see the comment at
`opencode_handler.rs:293`). A `test_pi_model_arg_shapes` should assert the three
shapes above.

### 1.5 Config + the `pi` starter profile

Add **`src/profile/templates/pi.toml`** (and register it alongside the other
templates wherever `claude.toml`/`codex.toml`/`nex.toml`/`opencode.toml` are
embedded — grep for `templates/opencode.toml` to find the include site). Model
the file on `opencode.toml`, with agency roles pinned to `claude:haiku` because
pi (like opencode) is a worker/chat handler and does **not** serve the agency
one-shot path (CLAUDE.md "Agency tasks run on claude CLI"):

```toml
description = "Pi Coding Agent: openrouter/anthropic/claude-3.5-haiku workers; claude:haiku for agency meta-tasks"

[agent]
model = "pi:openrouter/anthropic/claude-3.5-haiku"

[dispatcher]
model = "pi:openrouter/anthropic/claude-3.5-haiku"
max_agents = 8

[tiers]
fast     = "pi:openrouter/anthropic/claude-3.5-haiku"
standard = "pi:openrouter/anthropic/claude-3.5-haiku"
premium  = "pi:openrouter/anthropic/claude-sonnet-4"

[models.default]
model = "pi:openrouter/anthropic/claude-3.5-haiku"

[models.task_agent]
model = "pi:openrouter/anthropic/claude-3.5-haiku"

# Agency tasks (.evaluate-*, .flip-*, .assign-*) are short one-shot LLM calls
# pinned to claude:haiku by design (CLAUDE.md). Pi is a worker/chat handler and
# does not serve the agency one-shot path, so these roles stay on claude CLI.
[models.evaluator]
model = "claude:haiku"
[models.assigner]
model = "claude:haiku"
[models.flip_inference]
model = "claude:haiku"
[models.flip_comparison]
model = "claude:haiku"
```

Activation: `wg profile use pi` then runs pi-backed workers; `wg profile use
nex` reverts (the documented round-trip). Because the suffix in
`wg profile use <name>:<route>` is a model spec, `wg profile use pi:openrouter/
anthropic/claude-sonnet-4` pins that exact route in one step. (`pi` is a profile
name *and* an executor prefix here; that is consistent with how `claude`/`codex`
already double as profile name + executor.)

### 1.5.1 Reusable WG plumbing this mirrors
- Reply extraction → `extract_export_reply`/`extract_reply` (`opencode_handler.rs`).
- Model normalization → `opencode_model_arg` (`opencode_handler.rs:273`).
- Session lock + inbox cursor loop → `opencode_handler::run` (`:40`).
- Stdout-is-protocol (diagnostics to stderr/`handler.log` only) → `opencode_handler.rs:20-25`.

---

## 2. Chat/TUI layer + mid-chat model switching

There are **two distinct integration shapes — keep them separate** (model-mgmt
§6.1):

| Shape | Who renders | Terminal takeover | Use |
|---|---|---|---|
| **A. Pi as RPC chat handler** | WG draws transcript in its ratatui pane | a **bug** here (reintroduces alt-screen/tmux breakage) → must use `--mode rpc`/`print`, never TUI | the default, terminal-safe embed (§1.3 Shape A) |
| **B. Pi TUI as attached human front-end** | Pi renders full-screen | the **product** (the human wants pi's `/model`, steering, fork/clone) | explicit, advanced, user-launched only — never WG's unattended worker contract |

### 2.1 WG chat-agent mid-chat model switch (Layer 1 — ship now)

WG already has the switch engine: `wg chat set-executor <id> [--executor] [--model]`
(alias `switch`, `cli.rs:4917`) → `SetChatExecutor` IPC (`ipc.rs:602`) →
`handle_set_coordinator_executor` (`ipc.rs:1909`): persist override → SIGTERM
live handler → supervisor respawns → new handler re-reads the shared
`chat/<ref>/{inbox,outbox}.jsonl` (history survives on disk). **No new switch
engine is needed.** Add only:

1. **`wg chat model <id> <spec>`** — an ergonomic alias over `SetChatExecutor`.
   It derives the handler from the spec via `handler_for_model(spec)`, so one
   model spec sets both `model_override` and the implied `executor_override`
   (e.g. `wg chat model 3 codex:gpt-5.5` switches model *and* handler;
   `wg chat model 3 pi:openrouter/anthropic/claude-3.5-haiku` flips to the pi
   handler). This mirrors pi's `set_model {provider, modelId}` where provider
   rides with the model. Files: `src/cli.rs` (verb) + `src/commands/chat_cmd.rs`
   (handler that builds the existing IPC).
2. **TUI `/model` picker** (pi's `/model` UX) in the chat pane: list models from
   the model registry / active profile (`wg models`, `src/models.rs:363`);
   selection issues the same IPC. This is the one missing affordance — WG's TUI
   exposes a model field only at chat *create* time today
   (`src/tui/viz_viewer/chat_*`). **This is a user-visible TUI behavior**, so its
   validation must drive the real keystroke flow (tmux/PTY), not a CLI substitute
   (CLAUDE.md "User-visible behavior fixes require live human-flow validation").
3. **Optional `wg chat model <id> --cycle`** = pi's `cycle_model`, rotating the
   active profile's model list or the fast→standard→premium tiers
   (`Tier::escalate`).
4. **Surface the respawn honestly** for `claude`/`codex`/`pi`-as-worker handlers
   ("restarting handler on <model>…"); history is preserved, so it is a brief
   reconnect, not data loss.

### 2.2 Two warm-swap fast paths (Layer 2 — optional, later)

WG's `claude`/`codex` CLI handlers *cannot* swap model warm — their model is
fixed at CLI launch — so a respawn is correct for them. Two handlers *can* be
warm, and both are worth a later layer:

- **`nex` (in-process):** teach `nex_runtime` to **re-resolve its model per turn**
  from `CoordinatorState.model_override` instead of binding once at construction
  (`nex_runtime.rs`). On a `SetChatExecutor` where old and new handler kinds are
  both `nex`, **skip the SIGTERM** and let the running loop pick up the override
  on the next turn — a true warm swap. Cold respawn still applies when the
  handler *kind* changes (`nex → claude`). This is also the natural home for a
  `set_thinking_level` analogue (nex already has thinking support), matching pi's
  live reasoning-depth control. Files: `src/nex_runtime.rs` + a small branch in
  `ipc.rs:1909`.
- **`pi` (long-lived RPC):** **this is pi's distinctive contribution.** When the
  chat handler is already a live `pi --mode rpc` process and the new model stays
  on the pi handler, WG can issue pi's `set_model {provider, modelId}` RPC over
  the existing stdin instead of respawning — a warm swap for a *CLI-class*
  handler, which no other external CLI offers. The `wg chat model` verb should,
  when `old_kind == new_kind == Pi`, route to a "warm" branch in `pi_handler` (send
  `set_model`) and skip the SIGTERM; fall back to the cold path on a kind change.

### 2.3 Identity bridge (front-end enabler, §6.2 of model-mgmt)

Pi's session model maps onto WG chat identity nearly 1:1, which is what makes a
pi front-end coherent and lets a model choice round-trip back into WG:

- WG chat id (`chat-<N>`) ↔ pi `--session-id <wg-chat-id>` with
  `--session-dir <wg-state>/pi-sessions`.
- WG `chat/<ref>/{inbox,outbox}.jsonl` ↔ pi session JSONL (both append-only).
- WG `wg chat resume` (respawn from saved metadata) ↔ pi `--continue`/`--resume`/
  `switch_session`.
- WG per-chat `model_override` ↔ pi `set_model` — the front-end's `/model`
  selection writes the WG override so the choice survives a WG-side respawn.

**Shape B is gated** behind a spike: interactive pi does **not** exit on
credential failure (it sits in the TUI), so it can never be an unattended worker
handler — only an explicitly-attached human surface with a supervisor timeout.

---

## 3. Light-touch upstreamable pi patch (`PI_NO_TUI`) — optional

**Need: optional / belt-and-suspenders only.** The takeover is already fully
avoidable from WG's side with two flags (§1.3); WG must **not block** on any
upstream change. The patch matters only if WG later wants to embed pi's *real
TUI* through a both-TTY PTY **and** force-disable the grab from outside (e.g. a
shared launcher that cannot guarantee the flags).

**Exact spec** — a default-off env switch at the top of `resolveAppMode` in
`packages/coding-agent/src/main.ts` (executor-research §4.2):

```ts
function resolveAppMode(parsed, stdinIsTTY, stdoutIsTTY) {
  // Default-off escape hatch: let a supervising harness force non-interactive
  // even when launched under a PTY (both fds are TTYs). No effect unless set,
  // and explicit --mode rpc/json still wins.
  if (process.env.PI_NO_TUI && parsed.mode === undefined && !parsed.print) {
    return "print";
  }
  if (parsed.mode === "rpc")  return "rpc";
  if (parsed.mode === "json") return "json";
  if (parsed.print || !stdinIsTTY || !stdoutIsTTY) return "print";
  return "interactive";
}
```

**Upstream-friendliness properties (why a pi maintainer can pull it):**
- **Default-off:** active only when `PI_NO_TUI` is set in the environment.
- **Minimal surface:** ~3 lines, one new env var, one function.
- **No behavior change for existing users:** the existing branch order is
  preserved; the new guard sits *above* it and is inert without the env var.
- **Composes, does not override:** an explicit `--mode rpc`/`json` still wins
  (the guard only fires when `parsed.mode === undefined && !parsed.print`).
- **Strictly a convenience over `-p`:** documents intent ("a harness is driving
  me") without changing what `-p` already does.

**Recommendation:** prepare it as a small, well-described PR to
`earendil-works/pi` **only if** the Shape-B attached-TUI path is pursued and a
launcher that can't pass flags is in play. Otherwise skip it — the `--mode`/`-p`
+ piped-stdio wrapper carries zero upstream dependency, which is the preferred
posture (executor-research §4.1).

---

## 4. Assessment (NOT a commitment): rebase WG's default around pi

**The idea:** make pi the foundation of WG's default chat/worker stack rather
than an additive handler — i.e. pi becomes the default handler that
`claude`/`codex`/`nex` are today.

### 4.1 Scope
This is a **large, identity-level** change, not an integration. It would mean:
the default profile points at `pi:*`; the default chat agent and default workers
run pi; WG's in-process `nex` engine and the direct `claude`/`codex` CLI paths
become secondary; and WG's runtime contract gains a hard **Node.js + npm**
dependency for every user.

### 4.2 Risks
- **Runtime dependency inversion.** WG ships as a self-contained Rust binary
  (`cargo install`, `native` is "always available" — `executor_discovery.rs`).
  pi is an **npm ESM package** requiring Node on PATH (executor-research B6).
  Making it the default makes a non-Rust runtime a hard install prerequisite.
- **Version coupling to upstream.** A default built on pi couples WG's core UX to
  pi's release cadence and RPC/`models.json` schema. Today the dependency surface
  is **zero** (wrapper-only); a default rebase makes upstream pi a load-bearing
  dependency (executor-research §4.1).
- **Redundancy / loss of WG-native control.** WG already has `claude`, `codex`,
  and in-process `nex` handlers; pi *overlaps* all three (model-mgmt §6.3). WG-native
  capabilities — the warm `nex` per-turn swap, nex thinking control, agency
  one-shots on `claude:haiku`, the model registry/benchmarks — are not improved by
  pi and some (in-process control) are *weakened* by routing through an external
  process.
- **Agency pinning unchanged anyway.** `.evaluate-*`/`.flip-*`/`.assign-*` stay on
  `claude:haiku` by design (CLAUDE.md). A pi default doesn't simplify that; it just
  adds a second default engine beside the agency one.
- **Unvalidated at scale.** The three gating unknowns (credentialed RPC streaming
  end-to-end, session resume after kill, large tool-output truncation) are still
  open with no credentials on the eval machine (model-mgmt §6.3). Defaulting to pi
  bets the whole product on paths not yet exercised live.

### 4.3 Recommendation
**Do not rebase WG's default around pi.** Adopt pi **additively**, for the two
things it adds that WG genuinely lacks:
1. a **warm, mid-chat `/model` swap for a CLI-class handler** (§2.2), and
2. an **attachable human TUI front-end** (§2, Shape B).

Keep `claude`/`codex`/`nex` as the defaults and `claude:haiku` for agency. Revisit
only if (a) the §5 credentialed smoke pass is green, (b) pi demonstrably
out-performs the existing handlers on cost/quality for WG's default workload, and
(c) the Node runtime dependency is acceptable for WG's distribution story — at
which point this becomes its own design task, not a rider on this integration.

---

## 5. Phased implementation breakdown (next-phase tasks)

These are the concrete tasks to create **after this plan is human-reviewed**
(the `.flip-pi-design-integration` downstream gates implementation). They are
**documented here, not spawned**, so a human can adjust scope/sequencing first.

**Shared-file sequencing rule (golden rule: same file ⇒ sequential edge):**
- `src/dispatch/plan.rs` — touched only by **P0**. Everything else depends on P0.
- `src/cli.rs` — touched by **P1a** (`pi-handler` subcommand) and **P2a**
  (`wg chat model` verb). These **must be sequential** (P2a `--after` P1a) to
  avoid a same-file conflict, even though they are logically independent.
- `src/commands/service/ipc.rs` — touched only by **P3** (warm `nex` swap). P2a
  reuses the *existing* `SetChatExecutor` IPC unchanged, so it does not edit
  ipc.rs.
- `tests/smoke/manifest.toml` — grow-only; **P4** owns all pi smoke scenarios in
  one task to avoid concurrent appends.
- `src/profile/templates/pi.toml` is a **new file** (P1b) — no conflict; but the
  *include/registration* site it edits may overlap P1a's cli wiring file, so keep
  P1b's registration edit minimal and `--after P0`.

### Phase 0 — Foundation (prereq for everything)
```bash
wg add 'pi-executor-kind: add ExecutorKind::Pi + discovery' --after pi-design-integration -d '## Description
Add `ExecutorKind::Pi` as a chat-capable external CLI. Implement directly — do not decompose further.
File scope: src/dispatch/plan.rs, src/dispatch/handler_for_model.rs (test only), src/executor_discovery.rs.
- enum variant Pi; add to EXTERNAL_CLIS; do NOT add to WORKER_ONLY_EXTERNALS; as_str="pi"; from_str "pi".
- add "pi" to EXPERIMENTAL_EXTERNAL_EXECUTORS (binary_candidates ["pi"]).
## Validation
- [ ] Failing test written first: test_pi_prefix_routes_to_pi_handler in handler_for_model.rs
- [ ] handler_for_model("pi:openrouter/anthropic/claude-3.5-haiku") == ExecutorKind::Pi
- [ ] Pi in EXTERNAL_CLIS, NOT in WORKER_ONLY_EXTERNALS (assert both)
- [ ] cargo build + cargo test pass with no regressions'
```

### Phase 1 — Handler + profile (depends on P0)
```bash
# P1a — the handler (new file + cli wiring)
wg add 'pi-handler: wg pi-handler RPC chat + one-shot worker' --after pi-executor-kind -d '## Description
Add `wg pi-handler` subcommand (peer of opencode-handler/codex-handler) backed by new src/commands/pi_handler.rs. Implement directly — do not decompose further.
File scope: src/commands/pi_handler.rs (new), src/cli.rs (subcommand wiring), src/main.rs (dispatch).
- Shape A: pi --mode rpc, piped stdio, inbox-cursor loop mirroring opencode_handler::run; LF-only framing via read_until(b\x27\\n\x27).
- Shape B: pi -p / --mode json one-shot worker with supervisor timeout.
- pi_model_arg(): pi:openrouter/<vendor>/<model> -> --provider/--model (mirror opencode_model_arg); custom baseUrl -> temp models.json.
- credentials by env only, never --api-key.
## Validation
- [ ] Failing test written first: test_pi_model_arg_shapes (three spec shapes)
- [ ] pi_handler parses an agent_end event and extracts last assistant text (unit test with a canned JSONL fixture)
- [ ] cargo build + cargo test pass with no regressions'

# P1b — starter profile (new file; registration --after P0, runs parallel to P1a logically
# but keep --after pi-executor-kind; its only shared edit is the template-include site)
wg add 'pi-profile: pi.toml starter profile' --after pi-executor-kind -d '## Description
Add src/profile/templates/pi.toml (model opencode.toml after it; agency roles on claude:haiku) and register it at the template-include site. Implement directly — do not decompose further.
File scope: src/profile/templates/pi.toml (new), the template registration module (grep templates/opencode.toml).
## Validation
- [ ] wg profile list shows "pi"; wg profile use pi sets agent/dispatcher/tiers to pi:* and agency roles to claude:haiku
- [ ] cargo build + cargo test pass with no regressions'
```

### Phase 2 — Chat-agent mid-chat switch (Layer 1; P2a after P1a for cli.rs)
```bash
# P2a — wg chat model verb (shares src/cli.rs with P1a -> sequential)
wg add 'chat-model-verb: wg chat model <id> <spec> over SetChatExecutor' --after pi-handler -d '## Description
Add `wg chat model <id> <spec>` (and --cycle) as an ergonomic alias over the existing SetChatExecutor IPC; derive handler via handler_for_model(spec) so model+executor switch together. Reuse the existing IPC — do NOT add a new switch engine. Implement directly — do not decompose further.
File scope: src/cli.rs, src/commands/chat_cmd.rs. (Does NOT touch ipc.rs.)
## Validation
- [ ] Failing test written first: test_chat_model_sets_both_overrides
- [ ] wg chat model <id> codex:gpt-5.5 sets model_override AND executor_override=codex
- [ ] history preserved across the respawn (existing inbox/outbox jsonl)
- [ ] cargo build + cargo test pass with no regressions'

# P2b — TUI /model picker (separate files; user-visible -> human-flow validation)
wg add 'tui-model-picker: TUI /model picker mid-chat' --after chat-model-verb -d '## Description
Add a /model picker to the TUI chat pane (pi /model UX); selection issues the same SetChatExecutor IPC as P2a. Implement directly — do not decompose further.
File scope: src/tui/viz_viewer/chat_* (and the keymap/command dispatch for /model).
## Validation
- [ ] Reproducer is a live PTY simulation of the real human flow (tmux send-keys /model, select, observe view), NOT a CLI substitute
- [ ] Reproducer fails on main and passes after the change
- [ ] Scenario added to tests/smoke/scenarios/ and listed in owners of tests/smoke/manifest.toml
- [ ] cargo build + cargo test pass with no regressions'
```

### Phase 3 — Warm swap fast paths (optional, larger; after P2a)
```bash
wg add 'warm-swap: nex per-turn re-resolve + pi set_model warm path' --after chat-model-verb -d '## Description
Layer 2 warm swaps (optional). Implement directly — do not decompose further.
File scope: src/nex_runtime.rs, src/commands/service/ipc.rs (the SetChatExecutor branch), src/commands/pi_handler.rs (warm set_model branch).
- nex: re-resolve model per turn from CoordinatorState.model_override; skip SIGTERM when old/new kind both nex.
- pi: when old/new kind both Pi, send set_model RPC over existing stdin instead of respawn.
## Validation
- [ ] Failing test written first: test_nex_warm_swap_skips_sigterm_same_kind
- [ ] kind change (nex->claude) still cold-respawns
- [ ] cargo build + cargo test pass with no regressions'
```

### Phase 4 — Smoke + credentialed validation (after P1a; owns manifest.toml)
```bash
wg add 'pi-smoke: pi handler smoke scenarios + config lint' --after pi-handler -d '## Description
Add the pi smoke scenarios (grow-only manifest, this task owns all pi entries). Implement directly — do not decompose further.
File scope: tests/smoke/scenarios/* (new pi_*.sh), tests/smoke/manifest.toml (append, owners=[pi-smoke]).
Scenarios (executor-research §5): config lint rejects pi: when no pi binary; takeover regression guard (spawn pi under PTY with --mode rpc, assert NOT raw mode / exits on SIGTERM); one-shot -p worker prompt->reply->nonzero on cred error.
## Validation
- [ ] At least the 3 SKIP-guarded scenarios above present and listed in manifest owners
- [ ] Takeover guard fails if pi enters interactive mode under PTY
- [ ] cargo build + cargo test pass with no regressions'

wg add 'pi-live-validation: credentialed RPC streaming + resume + large output' --after pi-handler,pi-smoke -d '## Description
Live-provider pass for the three gating unknowns (model-mgmt §6.3): credentialed RPC streaming end-to-end (agent_end + non-empty get_last_assistant_text), session resume after process kill (--session-id), large bash/tool output truncation (fullOutputPath). Needs OpenRouter key. Implement directly — do not decompose further.
File scope: tests/smoke/scenarios/* (credentialed, SKIP without key).
## Validation
- [ ] Scenario streams a real turn and asserts a non-empty final reply (SKIP loudly without credentials)
- [ ] kill+resume continues the same session
- [ ] cargo build + cargo test pass with no regressions'
```

### Phase 5 — Optional upstream patch (independent; only if Shape B pursued)
```bash
wg add 'pi-upstream-patch: prepare PI_NO_TUI default-off PR to earendil-works/pi' --after pi-design-integration -d '## Description
ONLY if the attached-TUI (Shape B) path is pursued. Prepare the ~3-line default-off PI_NO_TUI guard at the top of resolveAppMode (packages/coding-agent/src/main.ts) as an upstream PR. This edits a vendored/forked pi checkout, NOT the WG tree. Implement directly — do not decompose further.
## Validation
- [ ] Patch is default-off (inert without PI_NO_TUI), composes with --mode (explicit mode still wins)
- [ ] PR description explains harness use case + minimal surface'
```

### Phase dependency graph (summary)
```
pi-design-integration (this doc, human-reviewed via .flip)
   └─ P0 pi-executor-kind ─┬─ P1a pi-handler ─┬─ P2a chat-model-verb ─┬─ P2b tui-model-picker
                           │                  │                       └─ P3 warm-swap
                           │                  ├─ P4a pi-smoke ── P4b pi-live-validation
                           │                  └─(cli.rs shared: P2a strictly after P1a)
                           └─ P1b pi-profile
   └─ P5 pi-upstream-patch (independent, optional)
```

---

## 6. Open questions for the human reviewer
1. **Default pi model.** The starter profile uses `pi:openrouter/anthropic/
   claude-3.5-haiku` (the only id verified working in prior evals). Confirm or
   substitute a preferred OpenRouter route before P1b.
2. **Shape B scope.** Is the attached human TUI front-end (and therefore the
   `PI_NO_TUI` upstream patch, P5) in scope now, or deferred? If deferred, P5 is
   dropped and §3 stays documentation-only.
3. **Warm-swap priority.** P3 is optional/larger. Ship Layer 1 (P2) first and
   defer P3, or bundle them?
4. **Promotion gate.** Confirm pi stays in `EXPERIMENTAL_EXTERNAL_EXECUTORS`
   until P4b (credentialed validation) is green before any promotion to stable
   or any default-rebase reconsideration (§4.3).

---

## Sources
- [`docs/pi-integration/executor-research.md`](executor-research.md) — `pi-research-executor`.
- [`docs/pi-integration/model-mgmt-research.md`](model-mgmt-research.md) — `pi-research-models`.
- Prior reports: `docs/reports/evaluate-pi-as-wg-executor.md`, `docs/research/openrouter-cli-executor-scan.md`.
- WG code anchors enumerated in the header above (verified this branch).
- Upstream pi: https://github.com/earendil-works/pi · https://pi.dev/docs/latest · npm `@earendil-works/pi-coding-agent`.
</content>
</invoke>
