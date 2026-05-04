# Agency Primitive Sync Model: Research & Recommendations

**Task:** research-agency-primitive  
**Date:** 2026-03-28  
**Prior art:** [primitive-pool-location.md](primitive-pool-location.md), [primitive-pool-sync.md](primitive-pool-sync.md)  
**User constraint:** NO automatic pulling from remote repos — supply chain attack vector. Primitives must be vendored in the binary at build time. Updates flow only when user upgrades `wg`.

---

## 1. Where the ~700 Primitives Live in the Source Tree

### 1.1 Vendored CSV (primary source)

**File:** `agency/starter.csv` (180 KB, 638 lines including header)

| Type | Count |
|------|-------|
| `role_component` | 338 |
| `desired_outcome` | 98 |
| `trade_off_config` | 201 |
| **Total** | **637** |

9-column format: `type, name, description, quality, domain_specificity, domain, origin_instance_id, parent_content_hash, scope`. Scope values include `task` (~579), `meta` (26), and meta-role scopes (`meta:assigner`, `meta:evaluator`, `meta:evolver`, `meta:agent_creator`).

Git-tracked since commit `64bcf83`. This is the distribution artifact.

### 1.2 Embedded in Binary

`src/commands/agency_init.rs:297`:
```rust
const EMBEDDED_STARTER_CSV: &[u8] = include_bytes!("../../agency/starter.csv");
```

The CSV is compiled into the `wg` binary at build time. This ensures `wg agency init` works even when the on-disk CSV is absent (e.g., installed via `cargo install`).

### 1.3 Hardcoded Starters (fallback)

`src/agency/starters.rs` defines a minimal set as Rust literals:
- 8 role components (code-writing, testing, debugging, code-review, security-audit, technical-writing, system-design, dependency-analysis)
- 4 desired outcomes
- 4 starter tradeoffs (Careful, Fast, Thorough, Balanced)
- 4 starter roles (Programmer, Reviewer, Documenter, Architect)
- Plus special-agent components/roles/tradeoffs for Assigner, Evaluator, Evolver, and Agent Creator

These are always seeded by `seed_starters()`, regardless of CSV import status.

### 1.4 Per-Project Materialized Store

`wg agency init` materializes primitives into `.wg/agency/`:
```
.wg/agency/
├── primitives/
│   ├── components/*.yaml    (one YAML file per role component)
│   ├── outcomes/*.yaml      (one YAML file per desired outcome)
│   └── tradeoffs/*.yaml     (one YAML file per trade-off config)
├── cache/
│   ├── roles/*.yaml         (composed roles = components + outcome)
│   └── agents/*.yaml        (agent = role + tradeoff)
├── evaluations/*.json
├── assignments/*.yaml
└── import-manifest.yaml     (provenance record of CSV import)
```

Each primitive is stored as `{content-hash-id}.yaml`. Content-hash addressing means identical content always produces the same filename, making deduplication automatic.

---

## 2. How `wg agency init` Currently Seeds Primitives

The seeding pipeline runs three layers in sequence:

1. **`seed_starters()`** — writes hardcoded components, outcomes, tradeoffs, and composed roles to YAML. Skip-if-exists logic (idempotent). Always runs.

2. **`try_csv_import()`** — imports the full CSV pool:
   - Checks for `import-manifest.yaml`; if present, skips (idempotent guard)
   - Resolution order: on-disk `agency/starter.csv` → embedded `EMBEDDED_STARTER_CSV`
   - Calls `agency_import::run_from_bytes()` which parses each row, content-hashes it, and writes `{id}.yaml` to the appropriate primitives subdirectory
   - Writes `import-manifest.yaml` with source, version, timestamp, counts, and SHA-256 of the CSV

3. **`try_upstream_pull()`** — if `agency.upstream_url` is set in config, fetches a remote CSV via HTTP and imports it. Non-blocking: failures print a warning but don't fail init.

After seeding, `agency_init` creates default + special agents and configures `auto_assign` and `auto_evaluate`.

**Current gap in this project:** The local project (`.wg/agency/`) has 110 components, 32 outcomes, 46 tradeoffs — substantially fewer than the full CSV pool (338 + 98 + 201). The `import-manifest.yaml` is absent, meaning the CSV import was never run. This project was initialized before the CSV bundling feature was implemented.

---

## 3. Sync Model Options

### Option A: Vendored Snapshot (Current Model)

**How it works:**
- A copy of `agency/starter.csv` is checked into the workgraph repo
- It's embedded into the binary at compile time via `include_bytes!`
- New projects get the full pool on `wg init` / `wg agency init`
- To update: copy the latest CSV from the upstream Agency repo, commit, release a new `wg` binary
- Existing projects: re-run `wg agency init` or `wg agency import` (content-hash dedup makes this safe)

**Advantages:**
- Zero runtime dependencies — works offline, no network calls at init time
- Fully reproducible — the binary contains exactly the primitives it was built with
- Git-tracked changes are reviewable (diff `agency/starter.csv` across commits)
- Already implemented and tested
- **Security:** No supply chain attack surface — primitives are reviewed in source control before being baked into the binary

**Disadvantages:**
- Manual update process: developer must copy CSV from upstream, commit, rebuild
- Existing projects don't automatically get new primitives (must re-run init or import)
- The binary size grows with the CSV (~180 KB currently, negligible)

**Migration concerns:** None. This is the status quo.

### Option B: Registry Pull at Runtime — REJECTED (security)

**How it works:**
- Configure `agency.upstream_url` pointing to a hosted CSV (or API endpoint)
- `wg agency init` fetches the latest primitives from the URL
- The embedded CSV serves as offline fallback

**Why this is rejected:**
- **Supply chain attack vector:** Primitives are behavioral instructions injected into agent prompts. A compromised upstream could inject malicious instructions that agents execute automatically. This is not a theoretical concern — primitives like "Evaluate sources by reputation level" directly shape agent behavior.
- **Trust boundary violation:** Fetching unreviewed content at runtime bypasses the code review process
- **Non-deterministic builds:** Two `wg agency init` runs at different times could produce different agent behavior

**Note:** The existing `try_upstream_pull()` and `--upstream` flag in `agency_import.rs` should be **removed or gated behind an explicit opt-in flag** (e.g., `--allow-remote`) with a warning. The current implementation fetches silently during init if `upstream_url` is configured.

**Current code of concern:** `src/commands/agency_init.rs:252-292` — `try_upstream_pull()` runs automatically during `wg agency init` if `upstream_url` is set. This should require explicit invocation.

### Option C: Git Submodule / Subtree

**How it works:**
- The upstream Agency repo (or a dedicated primitives repo) is linked as a git submodule at `agency/` or `primitives/`
- `git submodule update` pulls the latest CSV
- Build embeds from the submodule's file

**Advantages:**
- Git-native update mechanism (`git submodule update --remote`)
- Clear versioning via commit pinning
- Works with existing CI/CD
- Changes are reviewable via standard git diff

**Disadvantages:**
- Submodule UX is notoriously poor (detached HEAD, forgotten init, CI complexity)
- Couples the workgraph release cycle to the upstream repo's structure
- Breaks `cargo install --git` (submodules aren't fetched by default)
- Over-engineered for a single 180 KB file
- Adds a build-time dependency on the submodule being initialized

**Migration concerns:** Would need to restructure the `agency/` directory, update CI, document submodule workflow. High friction for low value.

---

## 4. Recommendation: Enhanced Vendored Snapshot (Option A+)

**The vendored snapshot model is correct and should remain the only sync mechanism.** Primitives are a dependency baked into the binary, not a live feed. Updates flow through `wg` binary upgrades only.

### Enhancements over current state:

#### 4.1 Add `wg agency update` command

A new command that imports primitives from the embedded CSV into an existing project:

```bash
# Check what would change without modifying anything
wg agency update --check

# Import new/changed primitives from the embedded CSV
wg agency update

# Import from a specific local CSV file (for development/testing)
wg agency update --from path/to/starter.csv
```

Internally, this:
1. Reads the current `import-manifest.yaml` (if any)
2. Compares the content hash against the embedded CSV
3. If different (or no manifest): runs `run_from_bytes()` with the embedded CSV
4. Updates the manifest with new counts and hash
5. Reports: "Added 15 new components, 3 outcomes, 7 tradeoffs. Total: 353 components, 101 outcomes, 208 tradeoffs."

This gives existing projects a one-command path to pull in primitives from a newer `wg` binary without understanding the import internals. No network calls.

#### 4.2 Re-import on binary upgrade

Detect when the embedded CSV hash differs from the manifest's `content_hash`. On `wg agency init`, if the manifest exists but the hash has changed, re-run the import (instead of skipping). This makes binary upgrades automatically propagate new primitives to existing projects.

Current behavior: `try_csv_import()` skips if manifest exists (line 308: `if manifest.exists() { return Ok(()); }`).

Proposed behavior: Skip only if manifest exists AND `content_hash` matches the embedded CSV hash. If hash differs, re-import and update manifest.

#### 4.3 Remove or gate `try_upstream_pull()`

Per the user's security constraint:
- **Option 1 (preferred):** Remove `try_upstream_pull()` entirely. Remove `upstream_url` from config. The network fetch during init is a security liability.
- **Option 2:** Gate behind `--allow-remote` flag on `wg agency init` and `wg agency import`. Never auto-fetch. Print a security warning when used.

#### 4.4 Manifest versioning

Add a `binary_version` field to the import manifest so `wg agency update` can report what version a project was last updated from:

```yaml
source: agency/starter.csv (embedded)
version: v0.5.2
binary_version: v0.5.2
imported_at: 2026-03-28T22:30:00Z
counts:
  role_components: 338
  desired_outcomes: 98
  trade_off_configs: 201
content_hash: abc123...
```

---

## 5. Update Propagation Flow

```
┌─────────────────────────────────┐
│ Upstream Agency Repo            │
│ primitives/starter.csv          │
└──────────┬──────────────────────┘
           │ Manual copy + git commit + code review
           ▼
┌─────────────────────────────────┐
│ workgraph Repo                  │
│ agency/starter.csv              │  ← git-tracked, reviewable
│                                 │
│ include_bytes!() in binary      │  ← embedded at compile time
└──────────┬──────────────────────┘
           │ cargo install (binary upgrade)
           ▼
┌─────────────────────────────────┐
│ wg binary                       │
│ (embedded CSV, version-stamped) │
└──────────┬──────────────────────┘
           │ wg agency init  (new projects)
           │ wg agency update (existing projects)
           ▼
┌─────────────────────────────────┐
│ Per-Project Store               │
│ .wg/agency/primitives/   │  ← content-hash YAML files
│ .wg/agency/cache/        │  ← composed roles + agents
│ import-manifest.yaml            │  ← provenance tracking
└─────────────────────────────────┘
```

No network calls at any step. Trust boundary is the `wg` binary.

---

## 6. Breaking Changes & Migration Concerns

| Concern | Risk | Mitigation |
|---------|------|------------|
| Changing `try_csv_import()` to re-import on hash mismatch | Low — content-hash dedup means no data loss. New primitives added, existing untouched | Test with existing project stores |
| Adding `wg agency update` command | None — new command, no existing behavior changed | — |
| Removing `try_upstream_pull()` | **Medium** — breaks workflows that depend on `upstream_url` config | Deprecation warning in one release, remove in next. Or gate behind `--allow-remote` |
| Existing projects missing full pool (like this one) | Data gap, not breakage. 110 components vs 338 available | `wg agency update` (or `wg agency import`) fills the gap |
| Manifest format change (add `binary_version`) | Low — YAML is extensible. Old manifests parsed with `#[serde(default)]` | Add field as `Option<String>` |

---

## 7. Immediate Action for This Project

This project's `.wg/agency/` has 110 components, 32 outcomes, and 46 tradeoffs but no `import-manifest.yaml`. The downstream task (`union-merge-agency`) should:

1. Run `wg agency import` to import the embedded CSV (or the on-disk `agency/starter.csv`)
2. This is safe: content-hash dedup ensures no existing entities are overwritten
3. Verify entity counts increase to ≥338 components, ≥98 outcomes, ≥201 tradeoffs
4. Confirm existing assignments and evaluations are preserved

---

## 8. Summary

| Model | Security | UX | Implementation Cost | Recommendation |
|-------|----------|----|---------------------|----------------|
| **A: Vendored (current)** | Excellent | Good | Zero (exists) | **Baseline** |
| **A+: Enhanced vendored** | Excellent | Better | Low (1 new command + hash check) | **Recommended** |
| B: Registry pull | Poor (supply chain risk) | Good | Exists but should be removed | **Rejected** |
| C: Git submodule | Good | Poor | Medium | Not worth the complexity |
