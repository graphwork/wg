<!-- provenance: extracted verbatim from AGENTS.md (sections now stubbed there) by
     task `doc-archive-policy`. Text is unchanged apart from heading level; the
     stub in AGENTS.md/CLAUDE.md links here. See docs/archive/POLICY.md. -->

# WG-Pilot — turnkey family-team deploy (`wg pilot`)

*Extracted from `AGENTS.md` § "WG-Pilot — turnkey family-team deploy (`wg pilot`, the deploy/UX wrapper)".*

WG-Pilot is the **one-command stand-up** of the real family-team federation on real
machines — the deploy/UX wrapper over the verified WG-Fed + WG-Review + WG-Exec substrate
(`docs/prod-audit/01`). It **ships NO new substrate and no new compat const**: it sequences
the existing `wg identity` / `wg fed-node` / `wg peer` / `wg msg` / `wg review` /
`wg provider` verbs and applies the SAFE defaults. It targets the **verified v1 profile** —
configured-peer, non-confidential-remote, block-don't-triage (no DHT, no TEE, no
human-in-loop). Cast: humans **Sara** + **Luca** (home host), agents **Bruno** (chef,
authorizer) + **Nora** (dietitian, disjoint verifier) (chef host), each a `wgid:` identity.

- **`src/commands/pilot_cmd.rs` + `wg pilot` (`Commands::Pilot`)** — `up` / `status` / `down`.
  From a filled config it mints the 4 identities into `wg secret` custody, starts the
  `wg fed-node` inbox, wires the configured cross-host peers (`wg peer add --wgid --endpoint`)
  with **split trust** (family Verified, everyone else Unknown), applies the **fail-closed /
  slack-bounded-leash / confidential-refuse / configured-peer** defaults (an explicitly-unsafe
  knob is **refused loudly before anything is stood up** — `resolve_safe_defaults`), optionally
  wires each agent's Telegram bot (`[telegram.bots.<name>]`, the multi-bot feature), and runs a
  **live end-to-end check** (task crosses the wall → content-review gate blocks an Unknown
  injection while the Verified task is consumable → borrowed-box exec under two scoped UCANs,
  no root/no blanket write → signed result accepted + attributed, wrong-signed rejected →
  confidential-to-non-attested REFUSED). Orchestration is by spawning `wg` itself
  (`current_exe`) per role with an isolated `$HOME` keystore + `--dir` graph (the smoke
  `wgrun` pattern). `down` SIGTERMs the node + clears state and is **idempotent**;
  `--wipe-identities` also wipes the rehearsal keystore (real deploys keep custodied roots).
- **`pilot.example.toml`** — the operator-supplied bits ONLY (host bind/endpoint, OpenRouter
  key path, optional Telegram tokens, trust defaults); everything else defaults SAFE. Kept in
  lock-step with the parser by a unit test (`shipped_example_template_parses_and_is_safe`).
- **`wg pilot up --dry-run`** — the smoke-tested rehearsal: models BOTH hosts locally as two
  FS-isolated dirs sharing one relay node and runs the whole family-team live check
  credential-free. Real multi-host `wg pilot up --config pilot.toml` runs once per host
  (`[pilot].role = home|chef`), wiring pre-exchanged `[[peers]]` wgids + probing peer health.
- **Operator README** — `docs/ops/runbook.md` §6 (what you provide vs automated, the one
  command, verify, teardown).

The end-to-end proof is pinned by `tests/smoke/scenarios/pilot_dry_run.sh`
(`owners = [pilot-deploy]`): one command stands up the pilot and the live family-team check
passes (`check_passed=true`, 4 identities minted, relay `/health` reachable) → the SAFE
defaults are applied (no unsafe knob on by default) → teardown stops the node (recorded pid
gone) and is idempotent → an explicitly-unsafe config is refused before stand-up. **Spark
boundary**: the dry-run worker + Pass-2 reviewer are deterministic/credential-free; the
live-model tier + full cross-host silicon ride the same wiring in a real deploy.
