# Terminal-Bench 2.0 Leaderboard Submission

Prepared submission files for the [Terminal-Bench 2.0 leaderboard](https://huggingface.co/datasets/harborframework/terminal-bench-2-leaderboard).

## Status: NOT YET SUBMITTABLE

The leaderboard requires **minimum 5 trials per task**. Our current experiment has 3 trials per task. Two additional trial runs per condition are needed before submission.

## Conditions

| Directory | Condition | Agent | Pass Rate (3 trials) |
|-----------|-----------|-------|---------------------|
| `wg-condition-a__minimax-m2.7/` | A | Bare agent (control) | 52.3% |
| `wg-condition-b__minimax-m2.7/` | B | wg stigmergic context | 51.4% |
| `wg-condition-c__minimax-m2.7/` | C | Enhanced planning + snapshots | 49.0% |
| *(planned)* | G | Context-only: wg context injection, no surveillance | — |

### Condition naming

- **A** — Bare agent (baseline): task description + verify command only
- **B** — wg stigmergic context
- **C** — Enhanced planning + snapshots
- **F** — Full wg-native with surveillance loops (historical; 0 activations in 95 pilot trials)
- **G** — Context-only: wg context injection (graph context + WG Quick Guide + wg CLI), no surveillance. Formalized from pilot analysis showing surveillance added 0 value. Validated by `tb-smoke-no-surv` (4/4 pass). This is the primary treatment condition for the A vs G full-scale experiment.

## To complete and submit

1. Run 2 additional trials per condition:
   ```bash
   bash terminal-bench/reproduce.sh --trials 2 --condition all
   ```

2. Populate submission directories with all trial data:
   ```bash
   bash terminal-bench/prepare-leaderboard.sh
   ```

3. Fork `harborframework/terminal-bench-2-leaderboard` on HuggingFace

4. Copy each condition directory into `submissions/terminal-bench/2.0/`

5. Open a Pull Request — the validation bot will check format and trial count

## Validation checklist

- [x] `metadata.yaml` present for each condition
- [x] `timeout_multiplier` = 1.0 (default)
- [x] No resource overrides
- [ ] Minimum 5 trials per task (currently: 3)
- [ ] All trial directories populated with result.json
