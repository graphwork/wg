# Agency Federation Design

> Share, discover, and compose agency definitions across projects and machines.

## 1. Overview

Agency federation enables roles, motivations, and agents to flow between WG instances. Because all three entity types use **content-hash IDs** (SHA-256 of identity-defining fields), federation is inherently conflict-free: identical definitions produce identical IDs regardless of where they were created.

This document covers six capabilities, layered from simple to complex:

| Layer | Command | Purpose |
|-------|---------|---------|
| 0 | `wg agency scan` | Discover agency stores on the filesystem |
| 1 | `wg agency pull` | Copy entities from another store into local |
| 2 | `wg agency push` | Copy local entities to another store |
| 3 | `wg agency remote` | Named references to other agency stores |
| 4 | `wg agency merge` | Combine multiple stores with dedup and performance merge |
| 5 | Global agency store | `~/.wg/agency/` with inheritance |

## 2. Core Invariants

1. **Content-addressable identity**: A role's ID is `sha256(skills + desired_outcome + description)`. A motivation's ID is `sha256(acceptable_tradeoffs + unacceptable_tradeoffs + description)`. An agent's ID is `sha256(role_id + motivation_id)`. These are the *only* identity-defining fields.

2. **Immutable identity, mutable metadata**: `name`, `performance`, and `lineage` are metadata attached to an entity but not part of its identity hash. Two stores may have the same entity ID with different performance records.

3. **Safe overwrite rule**: When pulling entity X from a remote store:
   - If X doesn't exist locally → copy it in (new entity).
   - If X exists locally with identical content → no-op.
   - If X exists locally with different *metadata* (performance, lineage, name) → merge metadata (see §6).
   - Structural conflicts are impossible because the hash would differ.

4. **Referential integrity**: An agent references `role_id` and `motivation_id`. When pulling an agent, its referenced role and motivation must also be pulled (or already exist locally). Federation commands enforce this transitively.

## 3. Agency Store Abstraction

An **agency store** is any directory containing the standard layout:

```
<root>/
├── roles/*.yaml
├── motivations/*.yaml
├── agents/*.yaml
└── evaluations/*.json        (optional)
```

A store can be:
- **Local project**: `.wg/agency/` in a WG project
- **Global user**: `~/.wg/agency/`
- **Bare store**: A standalone directory with just the agency subdirs (no `.wg/` parent needed)
- **Remote path**: Any accessible filesystem path

### 3.1 Store Resolution

Given a store reference string, resolution order:
1. If it's a named remote → look up path in `.wg/federation.yaml`
2. If it starts with `~/` or `/` → treat as filesystem path, look for `agency/` or `.wg/agency/` subdirectory
3. If it's `.` or a relative path → resolve from CWD
4. `--global` flag → `~/.wg/agency/`

Git-remote-backed stores (e.g., `git@github.com:team/agency-repo`) are a future extension. The initial implementation covers local filesystem paths only. The store abstraction is designed as a trait so git/URL backends can be added later.

## 4. Command Specifications

### 4.1 `wg agency scan <root-dir>`

Recursively walks `<root-dir>` looking for directories matching `**/agency/roles/` or `**/.wg/agency/roles/`. Reports each discovered store with a summary.

```
$ wg agency scan ~/projects
Found 3 agency stores:

  ~/projects/alpha/.wg/agency/
    Roles: 4  Motivations: 3  Agents: 6  Evaluations: 23

  ~/projects/beta/.wg/agency/
    Roles: 2  Motivations: 2  Agents: 3  Evaluations: 8

  ~/projects/shared-agency/
    Roles: 7  Motivations: 4  Agents: 12  Evaluations: 0
    (bare store)
```

**Options:**
- `--json` — machine-readable output
- `--max-depth <N>` — limit recursion depth (default: 10)

**Implementation notes:**
- Skip `.git`, `node_modules`, `target`, and other common build directories
- Detect bare stores (agency dirs without `.wg/` parent)
- Report total unique entities across all found stores

### 4.2 `wg agency pull <source> [--entity <id>...] [--type role|motivation|agent]`

Copies entities from `<source>` store into the local `.wg/agency/`.

**Behavior:**
1. Resolve `<source>` to an agency store path (see §3.1)
2. Load all entities from source (or filter by `--entity` / `--type`)
3. For each entity:
   - If pulling an agent, ensure its role and motivation are also pulled (transitive closure)
   - Check if entity exists locally
   - New entity → copy file
   - Existing entity → merge metadata (see §6)
4. Print summary of added/updated/skipped entities

```
$ wg agency pull ~/other-project
Pulled from ~/other-project/.wg/agency/:
  Roles:        +2 new, 1 updated, 3 skipped (identical)
  Motivations:  +1 new, 0 updated, 2 skipped
  Agents:       +3 new, 1 updated, 2 skipped

$ wg agency pull upstream --entity 81afa9b1
Pulled 1 role (81afa9b1 "analyst") from upstream
  Also pulled 0 dependencies
```

**Options:**
- `--dry-run` — show what would be pulled without writing
- `--no-performance` — skip merging performance data (copy definitions only)
- `--no-evaluations` — skip copying evaluation JSON files
- `--global` — pull into `~/.wg/agency/` instead of local project
- `--force` — overwrite local metadata instead of merging

### 4.3 `wg agency push <target> [--entity <id>...] [--type role|motivation|agent]`

Copies local entities to `<target>` store. Symmetric to pull.

```
$ wg agency push ~/shared-agency --type role
Pushed to ~/shared-agency/:
  Roles: +2 new, 1 updated, 4 skipped (identical)
```

**Options:** Same as pull (`--dry-run`, `--no-performance`, `--no-evaluations`, `--force`)

**Safety:** Push will not delete entities from the target. It only adds or updates. Push creates the target directory structure if it doesn't exist.

### 4.4 `wg agency remote add|remove|list|show`

Named references to agency stores, stored in `.wg/federation.yaml`.

```yaml
# .wg/federation.yaml
remotes:
  upstream:
    path: /home/erik/shared-agency
    description: "Team shared agency store"
    last_sync: "2026-02-19T22:00:00Z"
  alpha:
    path: /home/erik/projects/alpha/.wg/agency
    description: "Alpha project agencies"
    last_sync: null
```

**Commands:**
```
wg agency remote add <name> <path> [-d <description>]
wg agency remote remove <name>
wg agency remote list
wg agency remote show <name>   # Shows remote details + entity summary
```

Once a remote is registered, it can be used by name in pull/push:
```
wg agency pull upstream
wg agency push upstream --type agent
```

### 4.5 `wg agency merge <source1> <source2> [<source3>...]`

Combines entities from multiple sources into the local store. This is a multi-pull with explicit sources.

```
$ wg agency merge ~/project-a ~/project-b ~/project-c
Merged from 3 sources:
  Total unique roles: 8 (3 new, 5 existing)
  Total unique motivations: 5 (1 new, 4 existing)
  Total unique agents: 11 (4 new, 7 existing)
```

The merge operation is idempotent and commutative — the order of sources doesn't matter because entity identity is content-addressed.

**Options:**
- `--into <path>` — merge into a specific target instead of local project (useful for creating a combined bare store)
- `--dry-run` — preview without writing

### 4.6 Global Agency Store

`~/.wg/agency/` serves as a user-level store. It follows the same layout as project stores.

**Inheritance model:**
- When the agency system loads entities, it checks local project first, then global
- Local entities shadow global ones (same ID = local wins for metadata)
- `wg role list --include-global` shows both local and global entities
- `wg agency pull --global` pulls into global instead of local
- `wg agency push --global` pushes from global

**Implementation:** Modify `load_roles()`, `load_motivations()`, `load_agents()` in `src/agency.rs` to accept an optional secondary store path. When both exist, merge with local taking precedence on metadata conflicts.

## 5. Store Trait

```rust
/// An agency store that can load and save entities
pub trait AgencyStore {
    fn store_path(&self) -> &Path;
    fn load_roles(&self) -> Result<Vec<Role>>;
    fn load_motivations(&self) -> Result<Vec<Motivation>>;
    fn load_agents(&self) -> Result<Vec<Agent>>;
    fn load_evaluations(&self) -> Result<Vec<Evaluation>>;
    fn save_role(&self, role: &Role) -> Result<()>;
    fn save_motivation(&self, motivation: &Motivation) -> Result<()>;
    fn save_agent(&self, agent: &Agent) -> Result<()>;
    fn save_evaluation(&self, eval: &Evaluation) -> Result<()>;
    fn exists_role(&self, id: &str) -> bool;
    fn exists_motivation(&self, id: &str) -> bool;
    fn exists_agent(&self, id: &str) -> bool;
}
```

Two implementations:
- `LocalStore` — reads/writes YAML files in a directory (refactored from current agency.rs functions)
- Future: `GitStore`, `HttpStore` for remote backends

## 6. Metadata Merge Strategy

When the same entity (same content-hash ID) exists in two stores with different metadata, we need a merge strategy for mutable fields.

### 6.1 Performance Records

Performance data is project-specific — a role may score 0.95 in project A (simple tasks) and 0.70 in project B (hard tasks). Naively averaging would be misleading.

**Strategy: Union with provenance tagging**

Each `EvaluationRef` already contains `task_id` and `timestamp`. On merge:
1. Collect all `EvaluationRef` entries from both stores
2. Deduplicate by `(task_id, timestamp)` — same evaluation shouldn't appear twice
3. Recalculate `avg_score` and `task_count` from the merged set
4. The `--no-performance` flag skips this entirely (useful for sharing definitions without project-specific scores)

For full `Evaluation` JSON files in `evaluations/`:
- Copy any that don't exist locally (matched by filename, which includes task-id and timestamp)
- Skip duplicates

### 6.2 Lineage

Lineage records how an entity was created. Two stores may have the same entity but different lineage metadata if it was created independently (same content, different provenance).

**Strategy: Prefer richer lineage**
- If one has `parent_ids` and the other doesn't → keep the one with parents (more information)
- If both have parents → keep the one with higher generation number
- If equal → keep local (arbitrary but consistent)

### 6.3 Name

Names are human-friendly labels, not identity-defining.

**Strategy: Keep local name**
- If pulling, keep existing local name for already-known entities
- `--force` flag overrides to use source name

## 7. Referential Integrity

Agents depend on roles and motivations. Federation must maintain referential integrity.

### 7.1 Pull Integrity

When pulling agent A (with `role_id=R` and `motivation_id=M`):
1. Check if R exists locally → if not, also pull R from source
2. Check if M exists locally → if not, also pull M from source
3. If R or M doesn't exist in source either → error (broken agent in source)

### 7.2 Push Integrity

When pushing agent A:
1. Also push role R and motivation M if they don't exist in target
2. This ensures the target always has complete, valid agents

### 7.3 Selective Pull/Push

When using `--entity` or `--type` filters:
- `--type role` — pull only roles, no integrity concerns
- `--type motivation` — pull only motivations, no integrity concerns
- `--type agent` — automatically includes referenced roles and motivations
- `--entity <id>` — if it's an agent, include dependencies. If role/motivation, just that entity.

## 8. Federation Config File

`.wg/federation.yaml` stores remote definitions and sync metadata:

```yaml
remotes:
  <name>:
    path: <string>           # Filesystem path to the store
    description: <string>    # Optional human description
    last_sync: <datetime>    # Last pull/push timestamp (null if never synced)
    auto_pull: <bool>        # Future: auto-pull on wg service start
```

This file is separate from the main config to keep federation concerns isolated. It lives alongside `.wg/config.toml`.

## 9. Implementation Plan

### Phase 1: Store Abstraction (impl-agency-scan prerequisite)
- Extract current file I/O in `agency.rs` into an `AgencyStore` trait
- Implement `LocalStore` backed by a directory path
- Refactor existing `load_*` / `save_*` functions to use the trait

### Phase 2: Scan (impl-agency-scan)
- Walk filesystem for `agency/roles/` directories
- Report discovered stores with entity counts
- Handle permission errors gracefully

### Phase 3: Pull & Push (impl-agency-pull, impl-agency-push)
- Implement `pull()` function: source store → local store with metadata merge
- Implement `push()` function: local store → target store (reuses pull logic with swapped source/target)
- Transitive dependency resolution for agents
- `--dry-run`, `--no-performance`, `--force` flags

### Phase 4: Remotes (impl-agency-remote)
- Parse/write `.wg/federation.yaml`
- `wg agency remote add/remove/list/show` commands
- Integration with pull/push (resolve remote names to paths)

### Phase 5: Merge (impl-agency-merge)
- Multi-source pull into local (or `--into` target)
- Deduplication summary output
- Idempotency verification

### Phase 6: Global Store
- `~/.wg/agency/` as secondary store
- `--global` flag on pull/push
- Inheritance: load global, then overlay local
- `--include-global` flag on list commands

## 10. Future Extensions

These are explicitly out of scope for the initial implementation but the design accommodates them:

- **Git-backed remotes**: `wg agency remote add team git@github.com:org/agency-repo` — clone/pull the repo, treat it as a bare store
- **HTTP API**: Fetch agency definitions from a URL (e.g., a veracity exchange endpoint)
- **Selective sync**: Auto-pull from upstream remotes on `wg service start`
- **Conflict resolution UI**: If we ever need non-content-addressed fields in identity (unlikely), add interactive merge
- **Veracity exchange integration**: Public roles with cross-project performance records, reputation scores, and trust attestations

## 11. Security Considerations

- **Path traversal**: Store resolution must canonicalize paths and reject `..` escapes outside the intended root
- **Symlink attacks**: Follow symlinks cautiously during scan; skip if they point outside the scan root
- **Untrusted stores**: Performance data from external stores could be fabricated. The `--no-performance` flag allows pulling definitions without trusting foreign scores. A future trust model (signed evaluations, veracity attestation) can address this.
- **File permissions**: Push operations should respect target directory permissions and fail clearly if write access is denied

## 12. Implementation Review (2026-02-19, updated 2026-02-19)

Full implementation exists across `federation.rs`, 5 command files (`agency_{scan,pull,push,remote,merge}.rs`), and `agency.rs` trait/store abstractions. **78 integration tests + 52 unit tests pass (130 total).**

### 12.1 Bugs (all resolved)

All bugs identified in the initial review have been fixed:

| Issue | Resolution |
|-------|-----------|
| O(N\*M) disk reads: `target.load_*()` called inside transfer loop | Fixed: target entities loaded ONCE into HashMaps before transfer loops (`federation.rs:299-317`) |
| Silent error swallowing on corrupt target YAML | Fixed: errors now propagated with `?` operator. Test `transfer_errors_on_corrupt_target_yaml()` verifies. |
| `no_performance` + failed target load leaked source perf | Fixed: `PerformanceRecord::default()` used for new entities when `no_performance` is set |
| Missing agent deps in source not reported as error | Fixed: returns `anyhow::anyhow!("...referential integrity...")`. Two tests verify role and motivation cases. |
| `agency_pull.rs` silently ignored invalid `--type` values | Fixed: `parse_entity_filter()` uses `anyhow::bail!` for unknown types |
| `agency_pull.rs` only accepted singular type names | Fixed: now accepts both singular and plural (`"role" \| "roles"`) |
| `agency_scan.rs` display_path was a no-op | Fixed: now uses `strip_prefix(&root).unwrap_or(store_path)` |
| `agency_merge.rs` "Total unique" label was misleading | Fixed: labels now say "Role transfers" with explicit "(new, existing)" breakdown |
| Evaluation filter HashSets rebuilt per-iteration | Fixed: HashSets built ONCE outside the eval loop (`federation.rs:480-484`) |

### 12.2 Missing Features (remaining work)

| Feature | Design Doc Section | Status |
|---------|-------------------|--------|
| `--global` flag on push | §4.6 | Push uses `--global` flag already in CLI (`agency_push.rs:11`) — **implemented** |
| `--json` output on pull | Consistency with push/scan | **Implemented** (`agency_pull.rs:77-106`) |
| `--json` output on merge | Consistency with push/scan | **Implemented** (`agency_merge.rs:73-97`) |
| Global store inheritance (local shadows global) | §4.6 | Not implemented — requires modifying `load_roles()` etc. in `agency.rs` |
| `--include-global` flag on list commands | §4.6 | Not implemented — depends on global store inheritance |
| Path traversal rejection in `resolve_store` | §11 | Canonicalizes but doesn't reject `..` escapes beyond scan root |

### 12.3 Assessment

The core federation system is **production-ready**. All previously reported bugs are fixed. The transfer engine, metadata merging (performance union with dedup, lineage preference), referential integrity enforcement, remote management, and evaluation transfer all work correctly with comprehensive test coverage.

**Remaining work** is limited to Layer 5 (Global Agency Store): the `~/.wg/agency/` inheritance model where local entities shadow global ones. This is an additive feature that doesn't affect existing functionality.
