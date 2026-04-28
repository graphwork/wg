# `wg done` silently drops staged-uncommitted worktree changes

**Status:** open bug, surfaced 2026-04-28 by the `verify-agents-md` cycle (iter 1/3).
**Originally suspected as:** codex handler not running the same git commit + merge lifecycle as claude.
**Actual root cause:** `wg done`'s worktree-merge codepath returns `NoCommits` and proceeds to mark the task done when the worktree branch has 0 commits ahead of `main` but **non-zero uncommitted/staged files**. Not codex-specific.

## Repro (from `create-agents-md` / agent-933, 2026-04-28T21:09:12+00:00)

1. Task description told the agent: *"Stage and let `wg done` handle the commit + merge."*
2. Codex agent (agent-933, model `gpt-5.5`) faithfully followed the instruction:
   - `cp CLAUDE.md AGENTS.md`
   - `git add AGENTS.md` (stages, but does not commit)
   - `wg done create-agents-md`
3. Output of the final command (from `.wg/agents/agent-933/output.log`):
   ```
   Marked 'create-agents-md' as done
   Agent archived to /home/erik/workgraph/.wg/log/agents/create-agents-md/2026-04-28T21:09:12Z
   Output captured to /home/erik/workgraph/.wg/output/create-agents-md
   ```
   No `[merge]` line. No warning about uncommitted files. No error.
4. Mid-run `wg show create-agents-md` snapshot:
   ```
   Worktree:
     Path:              /home/erik/workgraph/.wg-worktrees/agent-908
     Branch:            wg/agent-908/create-agents-md
     Commits ahead:     0
     Uncommitted files: 1
     Cleanup pending:   false
     Merged to main:    true        ← FALSE LABEL — nothing was merged
   ```
5. Result: `git ls-tree main -- AGENTS.md` shows nothing on `main`. The agent's branch is gone. The worktree directory has been cleaned up. The file is lost.

## Code paths

`src/commands/done.rs:280-294` — the gate:

```rust
let commits_output = Command::new("git")
    .args(["log", "--oneline"])
    .arg(format!("HEAD..{}", wt.branch))
    .current_dir(&wt.project_root)
    .output()?;

let commit_count = String::from_utf8_lossy(&commits_output.stdout)
    .lines()
    .filter(|l| !l.is_empty())
    .count();

if commit_count == 0 {
    return Ok(WorktreeMergeResult::NoCommits);
}
```

`src/commands/done.rs:2113-2117` — the silent drop:

```rust
match attempt_worktree_merge(&wt, id)? {
    WorktreeMergeResult::NotInWorktree | WorktreeMergeResult::NoCommits => {
        // Nothing to merge — proceed to mark done
        mark_worktree_for_cleanup(&wt);
    }
    ...
}
```

**Two bugs collapse into one observable symptom:**

1. `attempt_worktree_merge` does not check whether the worktree has uncommitted/staged tracked changes. A worktree branch with `commits_ahead == 0 && uncommitted_files > 0` is treated identically to a clean worktree.
2. Whatever code populates the `Merged to main: true` field in `wg show` is not gated on an actual successful merge — it gets set even when `NoCommits` short-circuited.

## Why this is not codex-specific

The same path executes for every executor (`claude`, `codex`, `amplifier`, `native`). Claude tasks usually escape the trap because the claude agent guide explicitly tells the agent to `git add && git commit && git push` *before* `wg done`. Tasks whose description tells the agent "stage and let wg done handle the commit" — regardless of executor — hit this trap.

The original suspicion (codex handler doesn't commit/merge like claude) was a wrong lead. There is no commit step in either handler — workgraph relies on the *agent* to commit on its branch, then `wg done` squash-merges that branch to `main`.

## Suggested fixes

Mutually independent, can land separately. Listed in priority order.

1. **Loud failure for staged-uncommitted on `wg done`.** Before the `commit_count == 0` gate, run `git status --porcelain` in the worktree. If there are staged or modified tracked files, either:
   - **(a)** Refuse `wg done` with an actionable error: *"Worktree has uncommitted changes (X.md, Y.rs). Run `git commit` in the worktree before `wg done`, or pass `--abandon-uncommitted` to discard them explicitly."*
   - **(b)** Auto-commit on the agent's behalf with a default message like `"WIP: <task-id>"`. Less safe but more forgiving.

   Option (a) is the conservative pick — it preserves the invariant that `wg done` never silently mutates the agent's git state, and it surfaces the problem at the moment it happens (vs. discovering hours later that the file isn't on `main`).

2. **Stop lying in `wg show`.** The `Merged to main: true` flag in the worktree-status block must reflect actual `Merged` outcome, not just "we ran `attempt_worktree_merge` once." Trace where this field is set and gate it on `WorktreeMergeResult::Merged`.

3. **Surface `NoCommits` in `wg done` output.** Currently the only outputs from a successful `wg done` in a worktree are either silence (NoCommits) or `[merge] Squash-merged ...`. Add a `[merge] No commits on branch <X> — nothing to merge` log line for the NoCommits branch so the agent at least sees that nothing got merged.

4. **Companion task-template fix.** The phrase "Stage and let `wg done` handle the commit + merge" is wrong and appears in some onboarding task descriptions (it appeared in `create-agents-md` here). Replace with the canonical step: *"Run `git add <files> && git commit -m '...' && wg done <task>` — `wg done` will squash-merge your branch to main, but it does not commit on your behalf."*

## Iteration log (verify-agents-md cycle)

### Iter 1/3 — 2026-04-28 (this run)

- Verified AGENTS.md absent from main and no `feat: create-agents-md` commit exists.
- Diagnosed root cause as `wg done` silent-NoCommits path (above).
- Filed wg-side fix task (link below once created).
- Edited `create-agents-md` description: removed "let `wg done` handle the commit + merge" line, added explicit `git commit` step.
- Called `wg retry create-agents-md` to trigger iter 2.
- Did **not** converge — AGENTS.md is still not on main.
