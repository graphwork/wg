# Autonomous actor identity and graph presentation

## Decision

Graph visibility is a typed property of a task, not a property of its ID.
Every task has:

- `presentation`: `primary`, `autonomous`, or `plumbing`;
- `origin.kind`: `user`, `autonomous-actor`, or `agency-plumbing`;
- optional `origin.parent_task` and `origin.goal` lineage.

`primary` and `autonomous` tasks are visible in the default graph.
Autonomous tasks carry the centered-dot `·` identity glyph. `plumbing` tasks
are collapsed by default, summarized on their typed parent while active, and
remain reachable through the annotation/inspector and the unified plumbing
control.

A task name is opaque to renderers. In particular, `.quality-pass-*` may be
`autonomous`, and a non-dot task may be `plumbing`. The reserved dot namespace
continues to serve scheduler and compatibility checks, but it is not a
presentation API.

## Actors versus work

The daemon and deterministic reconciliation transitions are ledger actors:
`System` / `Reconciler`. They are not graph tasks. A reconciliation action that
only repairs a projection, advances a lifecycle transition, or reclaims stale
state therefore records the actor in the lifecycle ledger and creates no node.

If reconciliation discovers genuinely new LLM/source work, the controller
creates a normal graph task. That task must carry:

1. `presentation = autonomous`;
2. `origin.kind = autonomous-actor`;
3. the causal `origin.parent_task` when one exists;
4. a stable human-readable `origin.goal`.

Coordinator-created creation/evolution tasks follow this rule. Placement,
assignment, evaluation, FLIP, and verification satellites instead use
`presentation = plumbing` and name their owning task as `origin.parent_task`.

## Unified control

The TUI exposes one labeled, clickable control:

```text
· plumbing: hidden · N hidden
```

It cycles `hidden → running only → all`. `.` and the historical `<` key are
aliases for that same cycle. `running only` means genuinely active plumbing,
not queued `Open` satellites. The control always reports the current mode and
how many plumbing nodes remain hidden.

## Compatibility migration

Rows written before typed metadata are classified once during deserialization:

- known assign/FLIP/evaluate/place/verification satellite namespaces become
  `plumbing`, with a migrated parent;
- known chat/coordinator/user-board identities remain `primary`;
- other validated dot namespaces become visible `autonomous` work.

Explicit metadata always wins, even when an ID resembles a legacy satellite.
Serialization writes the migrated typed fields, so normal graph rewrites make
the migration durable. All ASCII, spatial graph, DOT, Mermaid, list, and TUI
visibility decisions consume the typed presentation.
