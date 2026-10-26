# Thinking effort as an explicit configuration dimension

Task: `expose-thinking-effort`

## Result

WorksGood now treats model identity and thinking effort as independently inspectable values.
The supported effort vocabulary remains:

`off | minimal | low | medium | high | xhigh | max`

No effort is encoded in a model string. Pi receives a separate `--thinking <level>` only
when the resolved effort is set; omission leaves Pi's handler default authoritative. Direct
Codex keeps its existing independent `model_reasoning_effort` adapter (`off → none`,
`minimal → low`) and does not conflate effort with response verbosity.

## Resolution and provenance

`Config::resolve_reasoning_detail(role)` returns the effective level, its logical source,
and one of three user-facing provenance states:

- **explicit** — `[models.<role>].reasoning` owns the role;
- **inherited** — the role inherits a configured tier or `models.default` value;
- **unset/omitted** — WG emits no effort flag.

Task-level explicit reasoning still wins at the existing spawn edge. Persisted task/chat
identity remains the authority for in-flight work; profile reloads affect future spawns only.

The two-tier setter preserves partial-update semantics. Model and effort edits use separate
TOML key sets. A reasoning-only edit never changes a `.model`, tier model, handler, provider,
or endpoint byte. A strong-only edit does not create `models.default.reasoning` when that key
was absent, preventing an omitted weak tier from accidentally inheriting the new strong
value. An already-explicit chat/default effort is updated with the strong tier.

## User surfaces

- `wg profile pi [--profile NAME]` always displays strong and weak models with exact effort,
  provenance, and source. `--list` does the same.
- Setter and dry-run output report model and effort transitions independently. JSON retains
  the legacy top-level model values and adds structured effort plus old/new transitions.
- `wg profile show`, `wg profile list`, and project `profile select` readiness show compact
  Worker/chat and Agency/FLIP/Eval effort. `wg config --models` remains the per-role source
  of truth.
- `wg status` shows compact chat/worker/agency effort.
- The TUI Settings panel exposes editable fast/standard/premium reasoning rows and appends
  compact `W:<level> A:<level>` information to actionable profile rows.
- `worksgood` attended setup prompts separately for Worker/chat and Agency/FLIP/evaluation
  effort, recommending `high` and `low`. Confirmation plans, returning lifecycle guidance,
  and `worksgood status` show the resolved choices.
- `worksgood --yes` is confirmation only. It preserves an already-configured effective
  value, but refuses an unset effort unless the matching `--strong-reasoning` or
  `--weak-reasoning` value is supplied. It never silently chooses the recommendation.

Global reusable profile definitions and fingerprint-pinned project selections remain
separate. Editing an inactive profile does not rewrite global routing. Editing a definition
selected by a project produces the existing fail-closed content-drift state until that
project explicitly reselects it; another project's pinned profile remains unchanged.

## Validation coverage

Automated coverage includes:

- omitted, inherited, and explicit provenance;
- partial reasoning/model updates and dry-run immutability;
- every supported effort level;
- model-byte stability during reasoning-only edits;
- active/inactive and two-project fingerprint-pinned isolation;
- actual fake-Pi worker argv with `high`, agency argv with `low`, interactive chat argv with
  `xhigh`, and an unset worker invocation with no `--thinking`;
- attended `worksgood` high/low recommendations, persisted/revisitable choices, and `--yes`
  refusal for omitted values;
- a real tmux TUI Settings flow showing editable effort rows and compact profile effort.

The task-owned smoke scenarios are registered in `tests/smoke/manifest.toml` under
`expose-thinking-effort`.
