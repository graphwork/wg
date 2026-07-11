# Investigate Tasks Disappearing After WG Reinstall

Task: `investigate-tasks-disappearing`

Date: 2026-07-11

## Executive Summary

The reported symptom, "most recent/long-term tasks disappeared from the TUI, while a seemingly random subset from roughly 25 days ago remained," is more consistent with a visibility or graph-selection problem than with task deletion.

The highest-probability causes are:

1. **The service/TUI is pointed at a different WG graph after reinstall.** WG graph resolution depends on `--dir`, `WG_DIR`, cwd walk-up, and `HOME`; a reinstall can change the executable path, systemd user, `HOME`, working directory, or environment. An old `~/.wg`, legacy `~/.workgraph`, copied project checkout, or stale worktree can easily present as "random older tasks remain."
2. **A creation-era cohort is being selected by representation, tag/filter state, or schema mix.** The added clue that recent/new tasks disappear while sparse older tasks survive makes this a first-class hypothesis. Recent changes made tag routing inert, but tag filters still exist and old/new tasks may differ in tags, dot-prefixes, statuses, `archived` tags, chat-loop tags, `created_at`/`last_interaction_at`, or fields added by newer code.
3. **The user is not actually looking at the same inventory surface.** The dedicated `wg tui` command sets `VizOptions { all: true, ... }`, so it should include ordinary non-system tasks from the active graph. However, `wg viz --tui` without `--all`, dot-prefixed/internal tasks, archive browser state, search state, and sort/focus state can still hide or reorder what the user expects. This is especially easy to confuse after a restart because the TUI is visual, not an authoritative all-graph audit.
4. **Tasks were archived, not deleted.** Archived terminal tasks move to `.wg/archive.jsonl` and are recoverable, but they leave the active graph view.

No code path inspected suggests reinstall itself deletes tasks. Task-moving operations are explicit (`wg archive`, `wg gc`, cleanup/recovery commands, migrations), and the safe procedure below treats all hidden data as recoverable evidence until proven otherwise.

## Code Findings

Graph resolution is path-sensitive:

- `src/cli.rs:12` defines global `--dir <PATH>`.
- `src/main.rs:58` resolves the workgraph directory from `--dir`, `WG_DIR`, cwd walk-up, `HOME`, then `./.wg`.
- `src/workgraph_dir.rs:9` documents the same precedence and accepts both modern `.wg` and legacy `.workgraph`.
- `src/main.rs:92` falls back to `~/.wg`, then `~/.workgraph`.
- `src/main.rs:107` makes `--dir <project-root>` or `WG_DIR=<project-root>` descend into `.wg`/`.workgraph`.

CLI inventory is less filtered than the TUI/viz view:

- `src/commands/list.rs:57` loads all tasks and only filters by dot-prefixed system tasks unless `--all`, status, paused, tag, priority, or cron filters are supplied.
- `src/commands/status.rs:130` supports `--all`, but status is still a summary, not an inventory.
- Installed CLI help confirms `wg list --all` and `wg status --all` exist; `wg graph-export`, not `wg graph`, is the graph export command.

TUI/viz has real visibility filters:

- `src/main.rs:1318` routes `wg viz` options into the TUI when `--tui` is used.
- `src/main.rs:3336` handles the dedicated `wg tui` command.
- `src/main.rs:3346` constructs the TUI `VizOptions`.
- `src/main.rs:3347` sets `all: true` for dedicated `wg tui`, so ordinary active-graph tasks should not be hidden by active-component filtering in that command path.
- `src/commands/viz/mod.rs:477` computes active roots from tasks whose status is not `Done` or `Abandoned` and that are not internal tasks.
- `src/commands/viz/mod.rs:502` includes tasks by focus, `--all`, status filter, or active connected component.
- `src/commands/viz/mod.rs:523` is the key default for `wg viz` without `--all`: without explicit `all`, status, or focus, it shows tasks in active weakly connected components and excludes abandoned tasks.
- `src/commands/viz/mod.rs:530` applies tag filters with AND semantics when tags are supplied.
- `src/commands/viz/mod.rs:580` hides internal dot/system tasks unless `show_internal` or `show_internal_running_only` is active.
- `src/tui/viz_viewer/state.rs:6918` loads config on startup.
- `src/tui/viz_viewer/state.rs:6949` uses `tui.show_system_tasks`.
- `src/tui/viz_viewer/state.rs:6950` uses `tui.show_running_system_tasks`.
- `src/config.rs:837` defines `tui.show_system_tasks`, default false.
- `src/config.rs:840` defines `tui.show_running_system_tasks`, default false.
- `src/tui/viz_viewer/state.rs:7461` passes those toggles into `generate_viz_output`.
- `src/tui/viz_viewer/state.rs:9576` applies TUI sort mode after rendering. Status-grouped sorting can make the visible subset look non-chronological, but it does not delete tasks.

Tag semantics are relevant but not currently a deletion mechanism:

- `src/config.rs:115` retains legacy `tag_routing` config entries.
- `src/config.rs:1760` documents the compatibility shim: freeform task tags are labels only, so tag routing never returns a runtime route.
- `src/config.rs:5879` tests that tag routing entries are inert legacy config.
- `src/commands/viz/mod.rs:1131` tests internal task filtering, including the guard that normal label tags do not make a task internal.
- `src/commands/add.rs:137` still gives `urgent`/`triage` tags a priority boost; this affects priority, not visibility.

Archive is explicit and recoverable:

- `src/commands/archive.rs:81` archives only `Done` and `Abandoned` tasks.
- `src/commands/archive.rs:117` appends archived tasks to `.wg/archive.jsonl`.
- `src/commands/archive.rs:246` implements restore from archive.

Recent relevant commits:

- `5ca7636b feat: make-tags-inert-labels` touched `src/commands/viz/*`, `src/config.rs`, and `src/main.rs`. It makes tags labels rather than routing semantics. I did not find evidence that it would hide ordinary non-dot tasks by itself, but it is relevant if the user relied on tag-based routing/filter expectations.
- `1124d678 feat: implement-structured-pi` touched `src/commands/status.rs`, `src/config.rs`, `src/main.rs`, and one line in TUI state. It is mainly model/reasoning plumbing, not task enumeration.

## Ranked Hypotheses And Discriminating Checks

### 1. Wrong graph path after reinstall or service restart

Why it fits: a graph from 25 days ago could be an old `~/.wg`, legacy `~/.workgraph`, a stale checkout, a backup copy, or a graph under a different Unix user. If service and TUI use different cwd/HOME/env, one can show a stale subset while the authoritative project graph still contains recent work.

Checks:

- Compare `wg which` from the project shell, from the service unit environment, and from any TUI launch wrapper.
- Compare inode, mtime, and line counts of every candidate `graph.jsonl`.
- Compare `wg --dir <candidate> --json list --all` counts for each candidate.
- Check `systemctl --user show wg.service` for `User`, `WorkingDirectory`, `ExecStart`, and sanitized environment.

### 2. Creation-era cohort selected by tags, schema fields, or persisted filters

Why it fits: the specific split "recent/new missing, sparse older surviving" can happen if a view/filter matches only old tag representations, old chat-loop/coordinator-loop tags, old statuses, or tasks created before/after a serialization change. It can also happen if missing new tasks are not in the same graph, while a stale older graph contains only an old subset.

Checks:

- Compare common fields among surviving old task IDs versus missing new task IDs on a copied graph.
- Split all tasks by `created_at` before/after 2026-06-16 (25 days before this report) and compare tags, statuses, dot-prefix, presence/absence of newer fields, and archive membership.
- Inspect persisted TUI state and config without mutating them: `.wg/tui-state.json`, `tui.show_system_tasks`, `tui.show_running_system_tasks`, and any active aliases/wrappers.
- Check whether any command uses `--tag`; both `wg list --tag` and `wg viz --tags/--tag` style filters are AND semantics when present.

### 3. TUI command path, TUI state, or hidden internal/system tasks

Why it fits: `wg tui` should pass `all=true`, but `wg viz --tui` without `--all` does not. Also, dot-prefixed/internal tasks remain hidden by default, TUI search/sort/focus state can make the visible list look selective, and status-grouped sorting can make old tasks appear before newer terminal tasks.

Checks:

- Run `wg --json list --all` and compare the task count to the TUI count.
- Run `wg viz --all --show-internal --no-tui` and search for missing task IDs.
- Confirm the user starts `wg tui`, not `wg viz --tui` or a shell alias/wrapper.
- Run `wg viz --status done --no-tui` for done tasks and `wg viz --status in-progress --no-tui` for active tasks.
- If CLI contains the tasks but TUI does not, do not recover or restore; diagnose visibility settings and view mode.

### 4. `wg viz --tui` active-component filtering hides terminal-only subgraphs

Why it fits: if the launch path is `wg viz --tui` rather than `wg tui`, completed long-running work can become terminal-only and disappear from the default viz view. Older tasks can remain visible if they are still connected to an active cycle/chat/root. That looks random by age because graph connectivity, not timestamp, decides visibility.

Checks:

- Compare `wg tui --help` and shell aliases/wrappers.
- Run `type wg` and `alias | grep '^alias wg='`.
- Run `wg viz --all --tui` or `wg tui` explicitly.

### 5. Archive moved terminal tasks into `.wg/archive.jsonl`

Why it fits: if recent/long-term tasks were completed and archived, the TUI main graph would no longer show them. Archive can appear age-correlated because `wg archive --older` accepts age filters and config has archive retention defaults.

Checks:

- `wg archive --list` is read-only.
- `wg graph-export --archive` includes archive support.
- Count `.wg/archive.jsonl` lines on a copied graph or with `wc -l`.

Stop condition: if tasks are in archive, do not restore live yet. Copy graph first and test `wg --dir <copy> archive restore <id>` on the copy.

### 6. TUI persisted search/sort/filter state or hidden system-task config

Why it fits: a TUI restart can preserve focus/search/sort state. A search term or hidden dot/system task setting can suppress expected entries.

Checks:

- In the TUI, clear search/filter state and toggle system task visibility.
- Compare with `wg viz --all --show-internal --no-tui`.
- Inspect sanitized config for `tui.show_system_tasks` and `tui.show_running_system_tasks`.

### 7. Stale daemon/socket bound to a different graph

Why it fits: service status is stored under the resolved WG dir and IPC socket paths are per graph. A daemon launched before reinstall or under a different cwd/user may still serve a graph different from the TUI's resolved graph.

Checks:

- `wg service status` and `wg --json status --all` from the same shell as TUI.
- Compare socket path from status output with the `wg which` result.
- Do not restart service during evidence collection; record state first.

### 8. Parser/index/cache corruption or partial graph write

Why it fits: less likely, but a damaged `graph.jsonl` or a partial append could cause a truncated load or parse failure. The CLI would usually error rather than show a coherent older subset, so this is lower probability.

Checks:

- Copy graph first, then validate JSONL line counts and parse behavior on the copy.
- Compare `tail -50 graph.jsonl`, mtime, and file size across candidates.

## Safe Decision Tree

1. Freeze evidence mentally: do not run `wg migrate`, `wg archive`, `wg gc`, `wg cleanup`, `wg recover`, `wg retry`, `wg sweep`, profile changes, or service restarts.
2. Identify the binary and launch context: `command -v wg`, `wg --version`, `id`, `pwd`, `HOME`, `WG_DIR`, `WG_GLOBAL_DIR`.
3. Identify the graph: `wg which`, `wg --json status --all`, `wg --json list --all`.
4. Compare candidate graphs: find `.wg` and `.workgraph` directories, then run read-only count commands against each via `wg --dir <candidate>`.
5. Run the cohort comparison on a copied graph. Use the user's known "surviving old" and "missing new" task IDs if available; otherwise split at 2026-06-16.
6. If the expected tasks exist in any candidate graph, stop investigating deletion. This is graph selection or view filtering.
7. If expected tasks exist in `wg list --all` for the current graph but not the TUI, stop investigating storage. This is command-path mismatch, TUI/viz filtering, sort/search/focus, or hidden system/archive behavior.
8. If expected tasks exist only in `archive.jsonl`, stop before restore. Copy the graph and test restore on the copy.
9. If expected tasks exist nowhere in active graph, archive, backups, or candidate dirs, preserve the evidence bundle and inspect shell history/system logs for prior destructive commands.
10. Only after a copied-graph reproduction identifies a code defect should an implementation task be created.

## Read-Only Server Evidence Commands

Run these from the directory where the user normally starts `wg tui`. They avoid secrets by printing only paths, counts, command help, and sanitized config lines.

```bash
set -u
mkdir -p /tmp/wg-disappearing-evidence

{
  date -Is
  uname -a
  id
  printf 'pwd=%s\n' "$PWD"
  printf 'HOME=%s\n' "${HOME-}"
  printf 'WG_DIR=%s\n' "${WG_DIR-}"
  printf 'WG_GLOBAL_DIR=%s\n' "${WG_GLOBAL_DIR-}"
  command -v wg || true
  wg --version || true
  wg --help | sed -n '1,80p' || true
  wg which || true
} > /tmp/wg-disappearing-evidence/00-context.txt 2>&1

{
  wg list --help || true
  wg status --help || true
  wg tui --help || true
  wg graph-export --help || true
  wg archive --help || true
} > /tmp/wg-disappearing-evidence/01-cli-surfaces.txt 2>&1

{
  wg --json status --all || true
  wg --json list --all || true
} > /tmp/wg-disappearing-evidence/02-current-graph-json.txt 2>&1

{
  wg status --all || true
  wg list --all || true
  wg service status || true
} > /tmp/wg-disappearing-evidence/03-current-graph-human.txt 2>&1

{
  wg viz --all --show-internal --no-tui || true
  wg viz --status in-progress --no-tui || true
  wg viz --status open --no-tui || true
  wg viz --status done --no-tui || true
} > /tmp/wg-disappearing-evidence/04-viz-views.txt 2>&1

{
  wg archive --list || true
  wg graph-export --archive || true
} > /tmp/wg-disappearing-evidence/05-archive.txt 2>&1

{
  wg config --list || true
  wg config --show || true
  wg config lint || true
} 2>&1 \
  | sed -E 's/(api[_-]?key|token|password|secret)([[:space:]]*[:=][[:space:]]*)[^[:space:]]+/\1\2<redacted>/Ig' \
  > /tmp/wg-disappearing-evidence/06-config-sanitized.txt

{
  systemctl --user status wg.service --no-pager || true
  systemctl --user show wg.service \
    -p Id -p FragmentPath -p LoadState -p ActiveState -p SubState -p MainPID \
    -p User -p WorkingDirectory -p ExecStart -p Environment \
    --no-pager || true
} 2>&1 \
  | sed -E 's/(api[_-]?key|token|password|secret|OPENROUTER_API_KEY|OPENAI_API_KEY|ANTHROPIC_API_KEY)=([^[:space:]]+)/\1=<redacted>/Ig' \
  > /tmp/wg-disappearing-evidence/07-systemd-sanitized.txt

{
  printf 'Candidate WG directories under HOME/project/tmp paths:\n'
  for root in "$PWD" "${HOME:-/nonexistent}" /srv /opt /var/lib /tmp; do
    [ -d "$root" ] || continue
    find "$root" -maxdepth 5 -type f \
      \( -path '*/.wg/*' -o -path '*/.workgraph/*' \) \
      \( -name graph.jsonl -o -name archive.jsonl \) 2>/dev/null
  done | sort -u
} > /tmp/wg-disappearing-evidence/08-candidate-files.txt 2>&1

tar -C /tmp -czf /tmp/wg-disappearing-evidence.tgz wg-disappearing-evidence
printf 'Evidence bundle: /tmp/wg-disappearing-evidence.tgz\n'
```

If `jq` is installed, add these read-only summaries:

```bash
wg --json list --all \
  | jq -r 'group_by(.status)[] | "\(.[0].status)\t\(length)"'

wg --json list --all \
  | jq -r '.[] | (.tags // [])[]' \
  | sort | uniq -c | sort -nr | head -50

wg --json list --all \
  | jq -r '.[] | [.id, .status, ((.tags // [])|join(",")), (.title // "")] | @tsv' \
  > /tmp/wg-disappearing-evidence/tasks.tsv
```

Run this copy-first cohort comparison. It does not mutate the live graph; it copies the graph and archive into `/tmp` and inspects the copy.

```bash
src="$(wg which | sed -n '1p')"
stamp=$(date +%Y%m%d-%H%M%S)
copy="/tmp/wg-cohort-copy-$stamp"
mkdir -p "$copy"
cp -a "$src"/. "$copy"/

# Optional: put known IDs in these files, one task id per line.
# /tmp/wg-surviving-old-ids.txt
# /tmp/wg-missing-new-ids.txt

python3 - "$copy" <<'PY'
import json, sys
from collections import Counter, defaultdict
from pathlib import Path
from datetime import datetime, timezone

root = Path(sys.argv[1])
cutoff = datetime(2026, 6, 16, tzinfo=timezone.utc)

def parse_ts(s):
    if not s:
        return None
    try:
        return datetime.fromisoformat(str(s).replace("Z", "+00:00"))
    except Exception:
        return None

def load_nodes(path):
    out = []
    if not path.exists():
        return out
    for lineno, line in enumerate(path.read_text(errors="replace").splitlines(), 1):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        try:
            obj = json.loads(line)
        except Exception as e:
            out.append({"_parse_error": str(e), "_line": lineno, "_raw": line[:200]})
            continue
        if obj.get("kind") == "task" or obj.get("type") == "task" or "id" in obj:
            obj["_line"] = lineno
            obj["_source"] = path.name
            out.append(obj)
    return out

tasks = load_nodes(root / "graph.jsonl")
archives = load_nodes(root / "archive.jsonl")
by_id = {t.get("id"): t for t in tasks if t.get("id")}
arch_by_id = {t.get("id"): t for t in archives if t.get("id")}

def read_id_file(path):
    p = Path(path)
    if not p.exists():
        return []
    return [l.strip() for l in p.read_text().splitlines() if l.strip() and not l.startswith("#")]

surviving_ids = read_id_file("/tmp/wg-surviving-old-ids.txt")
missing_ids = read_id_file("/tmp/wg-missing-new-ids.txt")

def cohort_for(t):
    ts = parse_ts(t.get("created_at") or t.get("completed_at") or t.get("last_interaction_at"))
    if ts is None:
        return "no_timestamp"
    return "new_since_2026-06-16" if ts >= cutoff else "old_before_2026-06-16"

def summarize(label, rows):
    print(f"\n## {label}: {len(rows)}")
    print("status", Counter(str(t.get("status")) for t in rows))
    print("dot_prefix", Counter(str(t.get("id", "")).startswith(".") for t in rows))
    print("archived_tag", Counter("archived" in (t.get("tags") or []) for t in rows))
    print("tags", Counter(tag for t in rows for tag in (t.get("tags") or [])).most_common(30))
    print("field_presence", Counter(k for t in rows for k in t.keys()).most_common(40))
    print("missing_fields_vs_Task_newer", Counter(
        tuple(k for k in ("created_at","last_interaction_at","completed_at","tags","status","after","before","profile","exec_mode","priority") if k not in t)
        for t in rows
    ).most_common(20))
    print("sample_ids", [t.get("id") for t in rows[:25]])

all_rows = [t for t in tasks if t.get("id")]
summarize("active graph all", all_rows)
summarize("active old_before_2026-06-16", [t for t in all_rows if cohort_for(t) == "old_before_2026-06-16"])
summarize("active new_since_2026-06-16", [t for t in all_rows if cohort_for(t) == "new_since_2026-06-16"])
summarize("archive all", [t for t in archives if t.get("id")])

if surviving_ids or missing_ids:
    surviving = [by_id.get(i) or arch_by_id.get(i) or {"id": i, "_not_found": True} for i in surviving_ids]
    missing = [by_id.get(i) or arch_by_id.get(i) or {"id": i, "_not_found": True} for i in missing_ids]
    summarize("explicit surviving old ids", surviving)
    summarize("explicit missing new ids", missing)
    print("\n## explicit id locations")
    for i in surviving_ids + missing_ids:
        loc = "active" if i in by_id else "archive" if i in arch_by_id else "not_found"
        print(i, loc)
PY
```

For each candidate graph path shown in `08-candidate-files.txt`, run:

```bash
candidate_dir=/path/to/candidate/.wg
printf '\n== %s ==\n' "$candidate_dir"
ls -ld "$candidate_dir" "$candidate_dir/graph.jsonl" "$candidate_dir/archive.jsonl" 2>/dev/null || true
wc -l "$candidate_dir/graph.jsonl" "$candidate_dir/archive.jsonl" 2>/dev/null || true
wg --dir "$candidate_dir" which || true
wg --dir "$candidate_dir" --json status --all || true
wg --dir "$candidate_dir" --json list --all | jq -r 'group_by(.status)[] | "\(.[0].status)\t\(length)"' 2>/dev/null || wg --dir "$candidate_dir" list --all || true
```

## Copy-First Recovery Procedure

Do not mutate the live graph until these conditions are met: the correct source graph is identified, the task IDs to recover are listed, a backup exists, and the recovery command has been tested on a copy.

1. Create a byte-preserving copy of the candidate graph:

```bash
src=/path/to/candidate/.wg
stamp=$(date +%Y%m%d-%H%M%S)
copy=/tmp/wg-recovery-copy-$stamp
mkdir -p "$copy"
cp -a "$src"/. "$copy"/
wg --dir "$copy" which
wg --dir "$copy" --json status --all
wg --dir "$copy" --json list --all > "$copy/list-all.json"
```

2. If tasks are in archive, test restore on the copy only:

```bash
wg --dir "$copy" archive --list
wg --dir "$copy" archive restore TASK_ID
wg --dir "$copy" show TASK_ID
wg --dir "$copy" --json list --all > "$copy/list-after-restore.json"
```

3. If the issue is wrong graph path, test explicit graph selection without changing service state:

```bash
wg --dir /correct/project/.wg which
wg --dir /correct/project/.wg status --all
wg --dir /correct/project/.wg list --all
wg --dir /correct/project/.wg tui --no-mouse
```

4. Stop before live mutation if any of these are true:

- Multiple candidate graphs contain divergent recent work.
- `graph.jsonl` parse behavior differs between CLI versions.
- Archive restore on the copy does not produce the expected task IDs.
- The service socket points at a different graph than the shell/TUI.
- The commands needed are not supported by the installed CLI.

In those cases, preserve `/tmp/wg-disappearing-evidence.tgz`, the copied graph directory, and the output of `wg --version`, then decide whether to merge graph JSONL content manually in a disposable copy or create a targeted repair task.

## When To Create A Follow-Up Implementation Task

Create a focused implementation task only if a copied/synthetic graph reproduces one of these defects:

- `wg --dir <copy> list --all` shows tasks but `wg --dir <copy> viz --all --show-internal --no-tui` omits non-system tasks unexpectedly.
- TUI starts with an unintended persisted search/filter that cannot be cleared.
- `wg which` differs between service IPC and CLI even when `--dir`/`WG_DIR` are explicit.
- A parser/index/cache path silently truncates or drops valid `graph.jsonl` lines.

Suggested follow-up title:

`Fix: TUI task inventory mismatch after graph resolution/filtering`

Suggested validation:

- Build a synthetic `.wg` graph with active, terminal-only, archived, dot-system, tagged, and legacy `.workgraph` tasks.
- Add a smoke scenario that starts `wg tui` in a PTY/tmux, captures the visible task list, and compares it to `wg viz --all --show-internal --no-tui` and `wg list --all`.
- The reproducer must fail on the implicated commit and pass after the fix.
- `cargo fmt --check`, `cargo clippy`, and `cargo test` pass.

## Current Conclusion

The current code has plausible visibility pitfalls but not enough evidence for a demonstrated destructive regression. The most likely operational fix is to identify the graph path actually used before and after reinstall, then launch service/TUI with explicit `--dir` or `WG_DIR` once the correct graph is confirmed. If CLI inventory is intact, the most likely UI explanations are command-path mismatch (`wg viz --tui` vs `wg tui`), hidden internal/system tasks, archive state, or persisted TUI search/sort/focus behavior rather than deletion.
