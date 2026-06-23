# True E2E smoke: PTY Pi GLM-5.2 chat → GLM-5.2 worker + DeepSeek agency

This is the documentation for the single **true end-to-end** smoke of the
Pi/OpenRouter two-tier profile.

- **Scenario:** `tests/smoke/scenarios/true_e2e_pi_glm_chat_deepseek_agency.sh`
- **Manifest entry:** `tests/smoke/manifest.toml` → `name = "true_e2e_pi_glm_chat_deepseek_agency"`, `owners = ["true-e2e-smoke"]`
- **Design it validates:** [`docs/design-two-tier-pi-profile.md`](../design-two-tier-pi-profile.md) — the GLM-5.2-worker / DeepSeek-agency split wired by `update-pi-starter`.

## What it proves (one run, four assertions)

Driven entirely from a PTY-based Pi GLM-5.2 chat running inside a real
`wg tui` (in tmux):

1. **PTY chat boot, no "Booting agent..." hang.** The Pi GLM-5.2 chat created
   with `wg chat new --executor pi --model openrouter:z-ai/glm-5.2` and rendered
   in `wg tui` must attach its pane to a **live interactive `pi`** PTY — not hang
   on "Booting agent..." or route through `pi --mode rpc` on pipes (the
   `fix-tui-pi` regression). A fake `pi` prints a unique boot marker; if the
   marker never appears the smoke FAILs loudly.
2. **Worker runs on GLM-5.2.** A task created **and dispatched from that chat**
   runs on a Pi worker whose inner argv is
   `--provider openrouter --model z-ai/glm-5.2`. Asserted from the fake-pi
   worker's recorded argv.
3. **Agency routes to DeepSeek, not claude:haiku.** The agency one-shot roles
   (`.evaluate` / `.flip` / `.assign`) resolve to
   `openrouter:deepseek/deepseek-chat` under the Pi profile. Asserted
   credential-free via `wg evaluate run <task> --dry-run` (evaluator role) and
   `--flip --dry-run` (inference + comparison roles), which print the
   role-resolved model without any network call.
4. **End-to-end completion, reflected in the chat.** Typing a trigger into the
   live chat composer makes the (fake) Pi agent run `wg add` + `wg spawn`; the
   GLM-5.2 worker marks the task `done`; and the chat pane echoes a reflection
   line. Asserted from the worker task status AND the reflection in the pane.

## Run it

```bash
# Direct (what the smoke gate runs):
WG_SMOKE_SCENARIO=true_e2e_pi_glm_chat_deepseek_agency \
  bash tests/smoke/scenarios/true_e2e_pi_glm_chat_deepseek_agency.sh

# Via the gate (this scenario is owned by true-e2e-smoke):
wg done true-e2e-smoke            # runs owned scenarios as a hard gate
wg done <task> --full-smoke       # runs every scenario
```

It SKIPs (exit 77) if `tmux` is unavailable.

## Why it is CI-capable / credential-free

A fake `pi` binary stands in for **both** the interactive chat agent and the
one-shot worker — the same convention as
`tests/smoke/scenarios/pi_worker_one_shot_prompt_and_cred_error.sh` and
`tui_pi_chat_pty_embedded.sh`. No OpenRouter / DeepSeek / Anthropic key is
required:

- The **worker route** is proven by the fake-pi **argv**
  (`--provider openrouter --model z-ai/glm-5.2`), exactly as `wg`'s pi handler
  builds it.
- The **DeepSeek agency route** is proven by the **config-resolved** `--dry-run`
  output. A *live* DeepSeek agency call needs `OPENROUTER_API_KEY`; without a
  key `resolve_agency_dispatch` deliberately falls back to `claude:haiku` (the
  `fix-route-agency` credential safety net). So a credential-free smoke asserts
  the **configured** routing, not a forced live DeepSeek call. With a key and a
  real `pi`, the same flow exercises the live models.

## Two implementation gotchas this smoke encodes

- **Private tmux server (`tmux -L <socket>`).** `wg tui` resolves the chat
  pane's executor from the loaded config's coordinator/`[dispatcher]` model
  (`effective_executor` infers `pi` from the `pi:` model prefix). A **shared**
  tmux server reuses the env of whichever client started it first, so a box that
  already has a tmux server running leaks its `HOME` (a *different* `~/.wg/config`)
  and `wg tui` then spawns the wrong executor entirely (observed: a real
  `claude` pane). The smoke starts a dedicated throwaway server so the TUI always
  inherits the scenario's isolated `HOME` + `PATH`.
- **Project-local Pi config (not just the active profile).** `wg init` writes a
  claude-default project config that would shadow the activated `pi` profile, and
  the TUI loads its own `Config`. The smoke activates the real `pi` starter
  profile (`wg profile use pi`) and then copies the resulting config to the
  project so **both** the CLI and the TUI resolve `coordinator.model =
  pi:openrouter/z-ai/glm-5.2` deterministically — no daemon / `CoordinatorState`
  handoff required, and no session-lock takeover race.

> Note: the narrower keystone scenario `tui_pi_chat_pty_embedded.sh` relies on
> the daemon writing per-chat `CoordinatorState` and on a shared tmux server; it
> is sensitive to both gotchas above. This E2E smoke avoids them by construction
> (project-local Pi config + private tmux server).
