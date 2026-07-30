# Migration note: attended chats use their normal tools

**Behavior change (July 2026):** WG no longer treats an attended chat as a
graph-only task intake bot.

> You are the human's attended repository assistant. Follow the human's request
> using your normal tools. Use WorksGood/`wg` to create, delegate, publish,
> inspect, and monitor tracked work when task management is requested or useful.
> Do not force every request into a task, and do not refuse repository
> inspection or implementation merely because you are a chat agent.

WorksGood is the project's task graph and worker coordinator; `wg` is its
expert CLI. At the WorksGood layer there is no attended-chat operation
denylist. An explicit, unambiguous human request authorizes any operation
provided by the normal tool surface. Actual tools, OS/platform permissions,
sandboxing, and project instructions still apply; if blocked, the chat must
name that real constraint. Discussion alone is not a mutation request, and
actual ambiguity should be clarified.

This intentionally supersedes the old `STOP` / `A chat agent NEVER reads source
files` prompt and similar duplicates. It does not change unattended dispatcher,
worker, bounded-evaluator, or deep-FLIP contracts.

## Existing installations

1. Install the accepted WG binary, then **restart every existing attended chat**
   so its first-turn/session prompt receives the new contract. User wording
   cannot override an old contract already retained in a live model session.
2. Neutral `.wg/agency/coordinator-prompt/*.md` guidance remains and is followed
   by the authoritative attended contract. If a composed legacy prompt contains
   a known retired denylist marker (including the production `A chat agent
   NEVER reads source files` wording), WG omits that stale body rather than
   sending contradictory system instructions. Remove or rewrite it to restore
   any still-useful custom graph guidance.
3. Pi chats load the version-locked WG extension with `-e` while retaining Pi's
   normal built-in tools and discovery (no `-ne`). Claude chats no longer
   receive a graph-only `--allowedTools Bash(wg:*)` restriction.
4. New and migrated chat tasks persist repository-root `working_dir`,
   `context_scope=full`, and `exec_mode=full` explicitly.
