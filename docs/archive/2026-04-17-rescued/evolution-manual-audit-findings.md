# Manual Section 5: Evolution - Audit Findings

**Audit Date:** 2026-04-12  
**Files Audited:** docs/manual/05-evolution.md, docs/manual/05-evolution.typ  
**Agent:** audit-manual-section-5

## Executive Summary

The evolution documentation is comprehensive and mostly accurate but contains some CLI syntax errors and missing coverage of newer features. Found 2 syntax errors and 3 missing features that should be documented.

## CLI Command Verification Status

### ✅ Verified Commands (Accurate)
- `wg evolve run` - All main options documented correctly
- `wg evolve apply` - Command exists as documented  
- `wg evolve review` - Command exists as documented
- `wg evaluate run` - All options documented correctly
- `wg evaluate record` - All options documented correctly  
- `wg evaluate show` - Command exists as documented
- `wg agency init` - Command exists as documented
- `wg agency stats` - Command exists as documented
- `wg agency pull` - Command exists as documented
- `wg agency push` - Command exists as documented
- `wg agency remote` - Command exists as documented
- `wg role lineage` - Command exists as documented
- `wg tradeoff lineage` - Command exists as documented
- `wg agent lineage` - Command exists as documented
- `wg func extract` - Command exists as documented
- `wg func apply` - Command exists as documented
- `wg func list` - Command exists as documented
- `wg func show` - Command exists as documented
- `wg retry` - Command exists as documented
- `wg trace export` - Command exists with `--visibility` option as documented

### ❌ Syntax Errors Found

1. **docs/manual/05-evolution.typ:267**
   - **Error:** `wg evolve --dry-run`
   - **Correct:** `wg evolve run --dry-run`
   - **Impact:** Users will get command not found error

2. **docs/manual/05-evolution.md:60-61, 63**
   - **Error:** `wg config --flip-verification-model opus`
   - **Correct:** Should be `wg config --set-model verification opus`
   - **Context:** Lines showing FLIP configuration examples
   - **Impact:** Users will get invalid option error

## Missing Features Not Documented

### 1. Autopoietic Evolution Mode
**Location:** Missing from both .md and .typ files  
**CLI Options Found:**
- `wg evolve run --autopoietic` - Enable autopoietic cycle mode
- `wg evolve run --max-iterations <N>` - Max cycle iterations (default: 3) 
- `wg evolve run --cycle-delay <SECONDS>` - Seconds between iterations (default: 3600)

**Impact:** Users unaware of advanced evolution cycling capabilities.

### 2. Evolution Fan-out Mode Controls  
**Location:** Missing from both files
**CLI Options Found:**
- `wg evolve run --force-fanout` - Force fan-out mode even with <50 evaluations
- `wg evolve run --single-shot` - Force legacy single-shot mode even with ≥50 evaluations

**Impact:** Users unaware of evolution scaling behaviors and control options.

### 3. Autonomous Agent Loops
**Location:** Missing from both files
**CLI Command Found:**
- `wg agent run` - Run autonomous agent loop (wake/check/work/sleep cycle)

**Impact:** Users unaware of autonomous agent capabilities beyond task assignment.

## Configuration Options Verification

### ✅ Verified Config Options
- `--flip-enabled` - Documented and exists
- `--flip-inference-model` - Documented and exists  
- `--flip-comparison-model` - Documented and exists
- `--flip-verification-threshold` - Documented and exists
- `--eval-gate-threshold` - Documented and exists
- `--eval-gate-all` - Documented and exists
- `--evolver-agent` - Documented and exists
- `--creator-agent` - Documented and exists
- `--retention-heuristics` - Documented and exists

### ❌ Config Option Errors
- `--flip-verification-model` (docs) → should be `--set-model verification <model>` (CLI)

## Performance Tracking & Scoring Verification

### ✅ Verified Features
- Four-dimension evaluation (Correctness 40%, Completeness 30%, Efficiency 15%, Style 15%) - Accurate
- Three-level propagation (agent, role, motivation) - Feature exists
- Synergy matrix via `wg agency stats` - Verified working
- Trend indicators - Documented correctly  
- FLIP evaluation system - All features verified
- External evaluation sources via `wg evaluate record --source` - Verified
- Eval gate mechanism - All features verified

## Additional Verification Notes

### Federation Features ✅
- `wg agency pull/push` commands exist and work as documented
- `wg agency remote` management commands verified  
- Visibility zones (`internal`, `public`, `peer`) verified in `wg trace export`

### Functions/Organizational Routines ✅ 
- `wg func extract/apply/list/show` commands all verified
- Documentation of workflow pattern extraction accurate

### Safety Guardrails ✅
- Budget limits (`--budget N`) verified
- Dry run mode (`wg evolve run --dry-run`) verified  
- Self-mutation deferral via `wg evolve review` commands verified

## Recommendations

### High Priority Fixes
1. **Fix syntax error in .typ line 267:** Change `wg evolve --dry-run` to `wg evolve run --dry-run`
2. **Fix FLIP config syntax in .md lines 60-61, 63:** Change `--flip-verification-model opus` to `--set-model verification opus`

### Documentation Additions
1. **Add autopoietic evolution mode section:** Document `--autopoietic`, `--max-iterations`, `--cycle-delay` options
2. **Add evolution scaling section:** Document `--force-fanout` and `--single-shot` modes  
3. **Add autonomous agents section:** Document `wg agent run` capabilities

### Minor Improvements
- Consider adding examples of the newer evolution options in practical guidance section
- Add note about the distinction between `--flip-verification-model` (deprecated/nonexistent) and `--set-model verification`

## Conclusion

The evolution documentation is fundamentally sound and covers the core concepts comprehensively. The syntax errors are minor but critical for user experience. The missing autopoietic and fan-out features represent significant capabilities that should be documented for completeness.

**Overall Assessment:** Good with critical syntax fixes needed + feature additions recommended.