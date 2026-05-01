# Archival Behavior + Cross-Graph Overlay Design

**Status:** Design proposal
**Date:** 2026-04-28
**Source task:** `research-archival-behavior`
**Out of scope:** code changes, format changes to `graph.jsonl`, implementation.

---

## 1. Motivating question

> "Do we have archive graphs that work back... and can these just be overlayed to get a big graph of past stuff?"

We have **five separate archival mechanisms** today, each with its own format, location, and read path. None of them are "the past graph" in a unified way. Some are graph-shaped (archived tasks), some are file-shaped (chat scrollback, agent logs), and some are git-shaped (preserved branch tips). This doc maps what exists, picks a target shape for a unified overlay, and lists the forks the user needs to weigh in on before we build it.

---

## 2. Current state map

### 2.1 `.wg/archive.jsonl` — task archive *(graph-shaped, partial overlay exists)*

**What it is.** A separate JSONL file written by `wg archive`. Each line is a serialized `Node::Task` — same schema as `graph.jsonl`, but living in a sibling file. Done/Abandoned tasks with no active downstream dependents are eligible.

**Who writes it.**
- `wg archive [ids...]` — explicit user invocation. Validates `Done | Abandoned` and `!has_active_dependents`. Bulk operations require `--yes`. (`src/commands/archive.rs:372` `run`)
- `wg archive --undo` — restore last batch. Sister file `archive-last-batch.json` holds the most recent set of ids for one-shot rollback. (`src/commands/archive.rs:303` `undo`)
- `archive::run_automatic(dir, retention_days)` — daemon-callable; archives tasks older than `retention_days` whose ids do NOT start with `.` (system tasks excluded). Provenance `{"automatic": true, ...}`. (`src/commands/archive.rs:530`)

**Who reads it.**
- `wg archive --list` and `wg archive search <query>` (`src/commands/archive.rs:186` `search`).
- `wg archive restore <id> [--reopen]` (`src/commands/archive.rs:247`).
- `wg graph-export --archive` — the **only existing overlay**. Reads both `graph.jsonl` and `archive.jsonl`, renders archived nodes as `lightgray, filled,dashed` in DOT. `--since`/`--until` add date filtering by `completed_at`. (`src/commands/graph.rs:112-194`)
- TUI `viz_viewer` — pulls counts from `archive.jsonl` for the dashboard; reads it line-by-line in two places (`src/tui/viz_viewer/state.rs:2614, 6445`).

**Gaps.**
- `wg list`, `wg show`, `wg viz` (the ASCII / interactive tree) do **not** read it. Only `wg graph-export --archive` and TUI counts touch it.
- No deduplication invariant: `restore` checks the live graph first, but two separate `wg archive` calls could in principle preserve a task that was re-added with the same id — there is nothing globally enforcing "id is unique across {live, archive}".
- No time-bounding: it grows monotonically until `wg archive --undo` or manual edit.

### 2.2 `refs/archive/wg/agent-*` — preserved branch tips *(git-shaped, no wg integration)*

**What it is.** Plain git refs created by the 2026-04-26 unmerged-branches audit. After squash-merging an agent branch, the tip is pushed to `refs/archive/wg/agent-N/<task>` so the original commit DAG stays reachable forever.

**Who writes it.** A one-shot script captured in `docs/audit-unmerged-branches-2026-04-26.md` and run during that audit. **Nothing in `wg` writes these today.** They are pure git operations.

**Who reads it.** Humans, via `git for-each-ref refs/archive` (130+ refs currently exist on this repo). The `cherry-pick-valuable` task referenced these refs to selectively land work that had been archived rather than merged.

**Recovery.** `git push origin refs/archive/wg/agent-N/<task>:refs/heads/wg/agent-N/<task>` resurrects the branch on `origin`.

**Gaps.**
- No automation. Future agent-branch cleanups have to redo the script by hand.
- No link from the archived task in `archive.jsonl` to the archived ref. A user looking at a 6-month-old `archive search` hit cannot get to the original branch tip without git-spelunking.
- Not in scope of any wg query surface.

### 2.3 `.wg/log/` — agent logs + operations *(file-shaped, write-once, no archival)*

**What it is.**
- `.wg/log/agents/<agent-id>/` — per-agent log directories. 110 such dirs currently. Written by spawn paths, never trimmed.
- `.wg/log/operations.jsonl` — append-only provenance: `{op: "archive"|"add_task"|...|, task_id, timestamp, user, detail}`. Rotation threshold is configurable in `Config::log.rotation_threshold` but no one calls "rotate".

**Who writes it.** `workgraph::provenance::record(...)` — called by `wg archive`, `wg gc`, `wg done`, etc.

**Who reads it.** Audit / debugging only — there is no first-class "show me the operations log" command. `wg trace` reads from agent log dirs.

**Gaps.**
- Not graph-shaped. Cannot be overlaid.
- Effectively unmanaged: agent log dirs accumulate forever.
- This is the right place to *find* "what was archived when, by whom" — but only by grep.

### 2.4 `.wg/chat/coordinator-N/archive/` — chat scrollback *(file-shaped, retention-managed)*

**What it is.** Rotated chat history. The active inbox/outbox is `.wg/chat/coordinator-N/{inbox,outbox}.jsonl`; rotation moves them under `archive/` with a `YYYYMMDD-HHMMSS` timestamp suffix. (`src/chat.rs:1200-1310`)

**Who writes it.** `chat::rotate_to_archive(...)` triggered by size thresholds in the chat module.

**Who reads it.** `chat::read_all_history_for(...)`, `chat::search_all_history_for(...)` — both already implement an overlay-style read across active + archived files, sorted by timestamp (`src/chat.rs:1233`). TUI scrollback uses these.

**Retention.** `chat.retention_days` (0 = forever); `cleanup_archives_for` deletes archives older than the cutoff (`src/chat.rs:1271`).

**Why it matters for this design.** The chat archive is the **closest existing precedent** for what the user is asking for: read-only, transparent overlay across "live" and "archived" data, with date filtering and search. The pattern is small and contained — we should copy it for tasks.

### 2.5 `.compact-N` / `.archive-N` cycle scaffolding — **retired** *(historical, do not use)*

**What it was.** An earlier design where each coordinator had companion `.compact-N` and `.archive-N` cycle members for introspection and cleanup loops. This was removed in `retire-compact-archive`.

**What remains.** The migration `wg migrate retire-compact-archive` (`src/commands/migrate.rs:215`):
- Abandons any surviving `.compact-N` / `.archive-N` task with a migration log entry.
- Strips `after`-edges that reference them.
- Idempotent.
- The integration test `tests/integration_retire_compact_archive.rs` enforces that creating a new chat does NOT auto-create these tasks.

**The English noun "archive" in this code path is unrelated to `.wg/archive.jsonl`.** Calling them out separately because the names collide and confuse search.

### 2.6 Adjacent commands that are NOT archival

- **`wg sweep`** (`src/commands/sweep.rs`) — does not touch the archive. It detects orphaned `InProgress` tasks (assigned to dead agents) and resets them to `Open` so the dispatcher re-claims. Reaping subsystem ("targets reaped, bytes freed") is about agent worktrees, not graph state.
- **`wg gc`** (`src/commands/gc.rs`) — deletes terminal tasks from `graph.jsonl` outright. Done tasks require `--include-done`. SCC-aware (won't delete a partially-complete cycle). Internal `.assign-*` / `.evaluate-*` companions are gc'd alongside their parent. **Crucially: `gc` discards the task; `archive` preserves it.** They are not redundant; they are different policies.

---

## 3. What "overlay" should mean

The user's question — *"can these just be overlayed to get a big graph of past stuff?"* — is the right framing. Below are the four concrete decisions an overlay design must answer.

### 3.1 Storage model — *recommendation: keep `archive.jsonl` separate*

Three options:

| Option | Pros | Cons |
|--------|------|------|
| **A. Separate `archive.jsonl`** *(current)* | Active reads stay fast. Format is identical so re-import is trivial. Already implemented + tested. | Two read paths. Need overlay layer. |
| **B. Single `graph.jsonl` with `archived: bool`** | One file, one read path. Simple overlay (just a filter). | Active reads degrade as the file grows — `load_graph` rescans the whole thing. Today's hot path is "list ready tasks", which would re-scan thousands of done entries. |
| **C. SQLite single source of truth** | Indexes, time queries, joins across live + archived for free. | Big migration. Loses the human-readable, git-friendly invariant — the explicit selling point in CLAUDE.md ("`.wg/graph.jsonl` ... human-readable, git-friendly"). |

**Recommendation: A (status quo) plus a thin overlay reader.** The hot path stays fast. The overlay is opt-in, like `--archive` already is for `graph-export`. C is a much bigger conversation and would erase a value-prop the project chose deliberately. Revisit C only if archive grows beyond ~100k tasks and we measure a real problem.

### 3.2 Time-bounding — *recommendation: keep single growing file, add date-windowed reads*

Two options:

| Option | Pros | Cons |
|--------|------|------|
| **A. Per-month bundles** (`archive-2026-04.jsonl`, ...) | Smaller working set per query. Easy to drop a year by deleting files. | Cross-bundle queries need union. More moving parts. |
| **B. Single `archive.jsonl` with `--since/--until` filters** *(close to current)* | Already half-built — `graph-export` does it. Simpler invariant. | One file grows unbounded. |

**Recommendation: B.** `graph-export` already filters by `completed_at`. Promote that filter to a shared library function and reuse it across all read paths. Per-month bundles are an optimization for "archive too big" — not a problem we have. If we later hit it, splitting an existing `archive.jsonl` by month is mechanical (lines are independent).

### 3.3 Read surface — *recommendation: a single `with_archive` flag, propagated*

The current state is incoherent:
- `wg graph-export --archive` works.
- TUI counts work.
- `wg list`, `wg show <id>`, `wg viz` (ASCII), `wg ready` — all silently ignore the archive.
- `wg archive --list`, `wg archive search` — only see the archive.

**Recommendation:**
1. Add a shared loader `parser::load_graph_with_archive(dir, since, until) -> WorkGraph` that returns a single `WorkGraph` with archived tasks tagged on a per-node `archived: bool` *projection field* (in-memory only — disk format unchanged).
2. Add a uniform `--include-archived [--since DATE] [--until DATE]` flag to `wg list`, `wg show`, `wg viz`, `wg ready`. They all pass through to the new loader.
3. The TUI gains a toggle (e.g., `A`) that flips the same flag on the active panel.
4. `wg archive --list` / `wg archive search` keep their current archive-only meaning — they're the "drill into the archive" surface, distinct from "merge into my view".

This matches the chat module's existing pattern (`read_all_history_for` ≈ "with archive") and avoids inventing a parallel idiom.

### 3.4 Export / import — *recommendation: single bundle = `archive.jsonl` + git refs*

The user asked about both "big graph of past stuff" (read) and implicitly "share a graph with someone else" (transport).

- **Read overlay:** `archive.jsonl` already round-trips through `Node::Task`. Nothing more is needed for in-process overlay.
- **Inter-project transport:** A workgraph "archive bundle" should be a tarball of:
  - `archive.jsonl` (tasks)
  - `log/operations.jsonl` (provenance, optionally filtered by date)
  - A manifest of `refs/archive/*` git tip SHAs that the archived tasks reference (so a sister project can `git fetch` them if they share the upstream).
- A new `wg archive export <path>` / `wg archive import <path>` pair is the right shape, but is an implementation task — out of scope for this doc.

---

## 4. Recommended target shape (concrete enough to implement)

### 4.1 On-disk
- **No format change.** `graph.jsonl` and `archive.jsonl` keep their current schemas.
- `archive-last-batch.json` keeps its role (one-shot undo).
- Optional follow-up: link archived tasks to their git ref by storing `archived_branch_ref: "refs/archive/wg/agent-141/nex-ux-600s"` on the task before archive (only when the agent that produced it had a worktree branch). Enables "from this archived task, get me back to the diff."

### 4.2 In-process
- New: `parser::load_graph_with_archive(dir, since: Option<DateTime>, until: Option<DateTime>) -> Result<WorkGraph>`.
  - Reads `graph.jsonl`, then merges archive entries that fall in `[since, until]`.
  - Tags merged-in nodes via a transient `archived: true` projection (NOT serialized).
  - Where ids collide, live wins; archive entry is dropped with a warning.
- New helper: `Task::is_archived(&self) -> bool` (reads the projection field).

### 4.3 CLI surface
- Add `--include-archived`, `--since`, `--until` to: `wg list`, `wg show`, `wg viz`, `wg ready`, `wg viz --tui`.
- TUI: `A` (or `Shift-A`) toggles archived overlay in the active pane. Archived nodes render greyed-out + dashed (mirroring `graph-export`).
- `wg archive` subcommands stay as they are — they are the archive-focused surface.

### 4.4 Performance contract
- The hot path `load_graph` is unchanged (does NOT read archive). Cost is paid only when `--include-archived` is set.
- `archive.jsonl` is read in a single linear pass (it is already). Date filtering happens in-memory after parse.
- If/when `archive.jsonl` exceeds ~50 MB, revisit per-year sharding (`archive-2025.jsonl`, ...) before per-month — coarser is enough.

---

## 5. Migration path

1. **Land the loader** (`load_graph_with_archive`) and unit-test that ids overlap is handled. No CLI change.
2. **Wire `wg viz --include-archived`** as the first consumer (lowest blast radius — read-only, easy to A/B with `wg graph-export --archive`).
3. **Wire `wg list --include-archived`** + `--since`/`--until`.
4. **Wire `wg show <id>` to fall through to archive** when the id is missing live (currently `wg show` errors; with the flag, it should find the archived task and label it as such).
5. **TUI toggle.**
6. **Optional: archive bundle export/import** — separate design once the read overlay is shipped and used.

Each step is a single PR. No format change at any step.

---

## 6. Open questions for the user

These are real forks I cannot make alone. Each one can be answered yes/no in one line.

1. **Is overlay opt-in or opt-out?** I assumed opt-in (`--include-archived`). The alternative is "always include archive in `wg list` / `wg show`" with `--no-archive` to hide. **Default-on changes meaning of historic command outputs in scripts.** Recommendation: opt-in.

2. **Should `wg show <archived-id>` work without a flag?** Today it errors — the id isn't in `graph.jsonl`. A natural improvement is to fall through to `archive.jsonl` and label the result as archived. Slightly different from "overlay on `list`". Recommendation: yes — the discovery cost of "wait, where did my task go" is high, and the result is unambiguous.

3. **Do we want git-ref linking now or later?** Storing `archived_branch_ref` on archived tasks lets us walk from "old archived task" → "git diff that produced it". It costs one optional field on the Task struct and a hook in `wg archive`. Could also be a follow-up.

4. **Time filter semantic — `completed_at` or `archived_at`?** Today `graph-export` filters by `completed_at`. But "show me everything that was archived in March" wants `archived_at`. We do not currently store the archive timestamp on the task — it's only in `archive-last-batch.json` and `operations.jsonl`. **Are we OK adding `archived_at: Option<String>` to the Task struct on archive write?** Recommendation: yes; it's additive and round-trip-safe.

5. **Should `wg gc` know about the archive?** Right now `gc` just deletes. Should it instead `archive`-then-delete (so nothing is ever silently lost)? This is a policy change and could double the size of `archive.jsonl` for users who use `gc` aggressively. Recommendation: leave `gc` alone for now — it has explicit `--include-done` opt-in and `--older` filtering, so the user has already made the "I want this gone" decision.

6. **Cross-project / federation overlay?** The federation system already shares agency entities by content hash. Should "archived task" be shareable cross-project the same way (e.g. mount another project's `archive.jsonl` under a remote prefix)? Recommendation: defer. The single-project overlay is concrete; cross-project is a separate design once we have a real use case.

---

## 7. Out of scope (intentionally)

- Implementing the overlay loader.
- Changing `graph.jsonl` format.
- A SQLite migration.
- An archive UI in the TUI.
- Archive bundle export/import.
- Cross-project overlay / federation.

These are good follow-ups, not part of this design.
