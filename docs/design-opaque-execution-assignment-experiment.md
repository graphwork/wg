# Immutable opaque execution assignment experiment

Status: **opt-in implementation experiment** (not the default runtime)

Enable a candidate daemon with:

```sh
WG_EXPERIMENTAL_OPAQUE_ASSIGNMENT=1 wg service start
```

## Contract

Authoring remains WG's responsibility. Project-local selection, imported reusable profiles, role/tier overrides, task overrides, agent identity, structured reasoning, and the project configuration revision resolve once into `execution_assignment::ExecutionAssignment`. Unset weak and strong tiers retain the existing equal-by-default rule; explicit tier and role splits remain explicit.

Runtime receives only one of:

- `Pi { program, opaque_route, reasoning, session_id }` (`None` explicitly means fresh); or
- `Shell { argv, environment, working_directory }`.

The Pi program is resolved to an absolute path at authoring when present; an unavailable configured path is retained only so preflight can classify it. Shell environment means the immutable explicit overlay (task identity/title); WG's attempt-control variables remain wrapper-owned.

WG recognizes the outer `pi:` envelope once. Every byte after it is stored and passed as one `--model` argument to the pinned Pi executable. The experiment runtime does not split that value into provider/model fields, normalize it, inspect the model registry, infer an endpoint/backend, merge a profile/tier, or choose a fallback. Reasoning is a separate pinned field. The bound assignment is create-once evidence under the exact attempt-runtime namespace (`execution-assignment/assignment.json`) and includes task, authoring-agent, runtime-agent, generation, attempt ID, fence, and configuration revision.

The worker and hermetic one-shot/review paths author and consume the same typed assignment and use the same opaque argument convention. Preflight invokes the same pinned Pi program with the same route; hermetic review adds the same `-ne` isolation flag as execution. Capability is ready only when Pi's own fuzzy resolver returns exactly one data row; zero (including Pi's successful `No models matching` result) or multiple rows are deterministic missing/ambiguous capability. Pi exit 2 is likewise deterministic. Other non-zero exits are transient/indeterminate. All defer admission without claiming the task or consuming task/cycle retries. Failed probes use assignment-keyed persisted exponential backoff (5s base, 60s cap), which survives daemon restart; a new config/task assignment gets a new key and can probe immediately. WG does not maintain a provider allowlist or split the query for this decision. Experimental reviewer/evaluator and persisted-plan calls execute the single authored primary assignment once; configured fallbacks are not expanded.

## Compatibility and unsupported boundaries

The default path is unchanged and historical executor/provider/model fields remain readable. The experiment accepts active `pi:<opaque-route>` selections and explicit shell tasks only. Active Claude, Codex, Nex/native, bare-provider, or ambiguous legacy routes are refused with `WG-OPAQUE-LEGACY-ACTIVE`; there is deliberately no guessed native-to-Pi translation. Migrate by explicitly selecting the intended Pi route after checking the original handler's authentication and model spelling.

Remote-provider placement remains on the WG-Exec provider plane and is refused by this local experiment (`WG-OPAQUE-REMOTE-UNSUPPORTED`), never covertly converted to Shell. The optional managed-process wake extension currently needs the stable split-route adapter. If explicitly requested with this experiment, admission reports `WG-OPAQUE-OPTIONAL-CAPABILITY-UNISOLATED` before an attempt is created; an unrelated absent extension is not a startup prerequisite.

Supervisor restart does not reinterpret a running attempt: the process and bound assignment remain attempt-owned. A changed project configuration applies when authoring a new assignment. Replacing a persisted assignment for an existing attempt fails closed and requires explicit new attempt authority. A digest over all task fields that influence authoring (agent/model/tier/reasoning/profile/exec/mode/remote placement/session identity, plus shell title metadata) is rechecked after automatic intent preparation and again inside the atomic claim transaction; mutation requires a fresh authoring pass rather than running bytes the evidence does not describe.

## Validation evidence boundary

Controlled fake-Pi fixtures test deterministic rejection, successful-empty rejection, transient backoff across restart, argv preservation, restart/config-mutation pinning, cancellation, and exact Shell fields. They are labelled fixtures, not proof of provider support. The accepted real-process wake evidence remains in `docs/reports/pi-managed-process-wakeup.md` and is reused rather than duplicated. Real-provider acceptance belongs to the downstream `verify-pi-execution-boundary` comparison task.
