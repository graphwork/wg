# Design Docs (Contributor Only)

> **This directory is for people hacking on wg itself.**
>
> The behavior these documents describe is **already implemented**. You do
> not need to read anything here to USE wg. If you're a user looking
> for the agent / chat-agent role contract, run:
>
> ```
> wg agent-guide
> ```
>
> The bundled output of `wg agent-guide` — not these docs — is the
> authoritative source of agent behavior in any wg project.

## What lives here

Design rationale and architecture decision records (ADRs) for wg
features. Each document explains *why* a system was built the way it was,
the alternatives considered, and the trade-offs accepted. They are
historical / didactic — useful when you want to change the system, not
when you want to use it.

## Three documentation layers

wg documentation is organized into three layers; this directory is
layer 3:

| Layer | Audience | Where it lives |
|-------|----------|----------------|
| **1. Universal role contract** | Every chat agent and worker agent in any project | Bundled in the `wg` binary; `wg agent-guide` |
| **2. Project-specific context** | Agents working on a specific project | `CLAUDE.md` / `AGENTS.md` at each project root |
| **3. wg contributor docs** | People hacking on wg itself | `docs/designs/` (this directory), `docs/research/` |

When in doubt: **users read `wg agent-guide` and project `CLAUDE.md`.
Contributors additionally read `docs/`.**
