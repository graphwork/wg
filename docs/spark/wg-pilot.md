<!-- provenance: extracted verbatim from AGENTS.md (sections now stubbed there) by
     task `doc-archive-policy`. Text is unchanged apart from heading level; the
     stub in AGENTS.md/CLAUDE.md links here. See docs/archive/POLICY.md. -->

# WG-Pilot — turnkey family-team deploy (`wg pilot`)

> **RETIRED — historical record only.** Task `retire-wg-pilot` (subtraction plan S10)
> **deleted** the implementation: `src/commands/pilot_cmd.rs`, `pilot.example.toml`, the
> `Commands::Pilot` / `PilotCommands` CLI arms, and `wg pilot`'s entries in the worker-control
> and daemon-IPC admin allowlists. No build (`--features federation` included) has a
> `wg pilot` verb. The `pilot_dry_run` smoke body is archived at
> `tests/smoke/archive/pilot_dry_run.sh` (manifest entry preserved, `retired = true`) and now
> SKIPs loudly. The text below is kept as the design record of what the wrapper did; it is
> **not** a contract, and nothing in the daily path depends on it. Use the substrate verbs
> directly (`wg identity`, `wg fed-node`, `wg peer`, `wg msg`, `wg review`, `wg provider`);
> see `docs/ops/runbook.md` §1–§5.
