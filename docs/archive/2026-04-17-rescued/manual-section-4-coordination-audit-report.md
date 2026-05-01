# Manual Section 4: Coordination - Audit Report

**Date:** 2026-04-12  
**Task:** audit-manual-section-4  
**Files Audited:**
- `docs/manual/04-coordination.md` (373 lines)
- `docs/manual/04-coordination.typ` (large file, exists)

## Executive Summary

Manual section 4 (Coordination) provides comprehensive documentation of workgraph's service daemon, coordinator features, dispatch mechanisms, and execution models. The audit reveals **high accuracy** between documented features and current CLI implementation, with all major command groups and configuration options verified as correct.

## Verification Results

### ✅ Service Commands Verified

**Location in manual:** Lines 9-16 (Service Daemon section)

| Command | Manual Reference | CLI Verified | Status |
|---------|-----------------|--------------|---------|
| `wg service start` | ✓ | ✓ | ✅ Correct |
| `wg service stop` | ✓ | ✓ | ✅ Correct |
| `wg service status` | ✓ | ✓ | ✅ Correct |
| `wg service restart` | ✓ | ✓ | ✅ Correct |
| `wg service pause` | ✓ | ✓ | ✅ Correct |
| `wg service resume` | ✓ | ✓ | ✅ Correct |
| `wg service freeze` | ✓ | ✓ | ✅ Correct |
| `wg service thaw` | ✓ | ✓ | ✅ Correct |
| `wg service tick` | ✓ | ✓ | ✅ Correct |
| `wg service reload` | ✓ | ✓ | ✅ Correct |

**Service start options verified:**
- `--max-agents` ✅
- `--executor` ✅  
- `--interval` ✅
- `--model` ✅
- `--force` ✅
- `--socket` ✅
- `--port` ✅
- `--no-coordinator-agent` ✅

### ✅ Multi-Coordinator Session Commands Verified

**Location in manual:** Lines 246-257 (Multi-Coordinator Sessions section)

| Command | Manual Reference | CLI Verified | Status |
|---------|-----------------|--------------|---------|
| `wg service create-coordinator` | ✓ | ✓ | ✅ Correct |
| `wg service stop-coordinator` | ✓ | ✓ | ✅ Correct |
| `wg service archive-coordinator` | ✓ | ✓ | ✅ Correct |
| `wg service delete-coordinator` | ✓ | ✓ | ✅ Correct |
| `wg service interrupt-coordinator` | ✓ | ✓ | ✅ Correct |

**Chat targeting verified:**
- `wg chat --coordinator <ID>` ✅ (Lines 257)

### ✅ Coordinator Configuration Options Verified

**Location in manual:** Lines 19, 167, 244 (Coordinator Tick, Parallelism, Reconfigure sections)

| Configuration Option | Manual Reference | CLI Verified | Status |
|---------------------|-----------------|--------------|---------|
| `max_agents` | ✓ | ✓ | ✅ Correct |
| `executor` | ✓ | ✓ | ✅ Correct |
| `poll_interval` | ✓ | ✓ | ✅ Correct |
| `model` | ✓ | ✓ | ✅ Correct |
| `coordinator.model` | ✓ | ✓ | ✅ Correct |
| `coordinator.executor` | ✓ | ✓ | ✅ Correct |
| `auto_assign` | ✓ | ✓ | ✅ Correct |
| `auto_evaluate` | ✓ | ✓ | ✅ Correct |
| `auto_triage` | ✓ | ✓ | ✅ Correct |
| `flip_verification_threshold` | ✓ | ✓ | ✅ Correct |

### ✅ IPC Protocol Commands Verified

**Location in manual:** Lines 214-243 (IPC Protocol section)

The manual documents comprehensive IPC commands table. All major IPC commands are verified as accurate:

| IPC Command | Manual Reference | CLI Behavior | Status |
|-------------|-----------------|---------------|---------|
| `graph_changed` | Line 221 | Automatic trigger | ✅ Correct |
| `spawn` | Line 223 | `wg spawn` command | ✅ Correct |
| `agents` | Line 224 | `wg agents` command | ✅ Correct |
| `kill` | Line 225 | Agent termination | ✅ Correct |
| `reconfigure` | Line 229 | `wg service reload` | ✅ Correct |
| `pause`/`resume` | Line 227-228 | Service commands | ✅ Correct |
| `freeze`/`thaw` | Line 231-232 | Service commands | ✅ Correct |

### ✅ Agent Monitoring Commands Verified

**Location in manual:** Lines 162-173 (Parallelism Control section)

| Command | Manual Reference | CLI Verified | Status |
|---------|-----------------|--------------|---------|
| `wg agents` | ✓ | ✓ | ✅ Correct |
| `wg agents --alive` | ✓ | ✓ | ✅ Correct |
| `wg agents --dead` | Not mentioned | ✓ | ℹ️ Additional option |
| `wg agents --working` | Not mentioned | ✓ | ℹ️ Additional option |
| `wg agents --idle` | Not mentioned | ✓ | ℹ️ Additional option |

### ✅ Peer Communication Commands Verified

**Location in manual:** Lines 260-264 (Peer Communication section)

| Command | Manual Reference | CLI Verified | Status |
|---------|-----------------|--------------|---------|
| `wg peer add <name> <path>` | Line 261 | ✓ | ✅ Correct |
| `wg peer list` | Line 263 | ✓ | ✅ Correct |
| `wg peer status` | Line 263 | ✓ | ✅ Correct |
| `wg peer show` | Not mentioned | ✓ | ℹ️ Additional command |
| `wg peer remove` | Not mentioned | ✓ | ℹ️ Additional command |

### ✅ Maintenance Commands Verified

**Location in manual:** Lines 267-274 (Compaction, Sweep, and Checkpoint section)

| Command | Manual Reference | CLI Verified | Status |
|---------|-----------------|--------------|---------|
| `wg compact` | Line 269 | ✓ | ✅ Correct |
| `wg sweep` | Line 271 | ✓ | ✅ Correct |
| `wg checkpoint` | Line 273 | ✓ | ✅ Correct |
| `wg requeue` | Lines 303-307 | ✓ | ✅ Correct |

**Sweep options verified:**
- `--dry-run` ✅

**Checkpoint options verified:**
- `--summary` ✅
- `--agent` ✅
- `--file` ✅
- All metadata options ✅

### ✅ User Board Commands Verified

**Location in manual:** Lines 282-288 (User Boards section)

| Command | Manual Reference | CLI Verified | Status |
|---------|-----------------|--------------|---------|
| `wg user init` | Line 283 | ✓ | ✅ Correct |
| `wg user list` | Line 287 | ✓ | ✅ Correct |
| `wg user archive` | Line 287 | ✓ | ✅ Correct |

### ✅ Provider Profiles and Cost Tracking Verified

**Location in manual:** Lines 290-301 (Provider Profiles and Cost Tracking section)

| Command | Manual Reference | CLI Verified | Status |
|---------|-----------------|--------------|---------|
| `wg profile set <name>` | Line 292 | ✓ | ✅ Correct |
| `wg profile show` | Line 293 | ✓ | ✅ Correct |
| `wg profile list` | Line 293 | ✓ | ✅ Correct |
| `wg profile refresh` | Line 293 | ✓ | ✅ Correct |
| `wg spend` | Line 298 | ✓ | ✅ Correct |
| `wg spend --today` | Line 299 | ✓ | ✅ Correct |
| `wg spend --json` | Line 299 | ✓ | ✅ Correct |
| `wg openrouter status` | Line 301 | ✓ | ✅ Correct |
| `wg openrouter session` | Line 301 | ✓ | ✅ Correct |
| `wg openrouter set-limit` | Line 301 | ✓ | ✅ Correct |

### ✅ Event Stream and Trace Commands Verified

**Location in manual:** Lines 312-340 (Observing the System, Operations Log and Trace sections)

| Command | Manual Reference | CLI Verified | Status |
|---------|-----------------|--------------|---------|
| `wg watch` | Line 312 | ✓ | ✅ Correct |
| `wg watch --event` | Line 315 | ✓ | ✅ Correct |
| `wg watch --task` | Line 315 | ✓ | ✅ Correct |
| `wg watch --replay` | Line 316 | ✓ | ✅ Correct |
| `wg trace show` | Line 333 | ✓ | ✅ Correct |
| `wg trace export` | Line 337 | ✓ | ✅ Correct |
| `wg trace import` | Line 337 | ✓ | ✅ Correct |

### ✅ Model and Routing Commands Verified

**Location in manual:** Lines 90-91 (Dispatch Cycle - model resolution section)

| Command | Manual Reference | CLI Verified | Status |
|---------|-----------------|--------------|---------|
| `wg model routing` | Line 90 | ✓ | ✅ Correct |
| `wg model set` | Line 90 | ✓ | ✅ Correct |
| Model routing per role | ✓ | ✓ | ✅ Correct |

### ✅ Manual Dispatch Commands Verified

**Location in manual:** Line 355 (Manual Control section)

| Command | Manual Reference | CLI Verified | Status |
|---------|-----------------|--------------|---------|
| `wg spawn <task-id> --executor claude` | Line 355 | ✓ | ✅ Correct |

**Spawn options verified:**
- `--executor` ✅
- `--timeout` ✅  
- `--model` ✅

## Documentation Accuracy Assessment

### Strengths
1. **Comprehensive Coverage**: Manual covers all major coordination features
2. **Technical Accuracy**: Command syntax, options, and behavior descriptions are correct
3. **Architecture Documentation**: Detailed explanation of coordinator tick phases, dispatch cycle, and IPC protocol
4. **Feature Completeness**: All documented features have corresponding CLI implementations

### Minor Gaps (Enhancement Opportunities)
1. **Additional Agent Filtering**: CLI has `--dead`, `--working`, `--idle` options for `wg agents` not mentioned in manual (lines around 162-173)
2. **Extended Peer Commands**: CLI has `wg peer show` and `wg peer remove` not documented
3. **Chat Command Features**: Some advanced chat options like `--compact`, `--share-from` not covered

### Technical Concepts Accurately Documented
1. **Coordinator Tick Phases**: 8-phase tick loop correctly documented (lines 21-76)
2. **Dispatch Cycle**: Multi-step dispatch process accurately described (lines 82-149)
3. **IPC Protocol**: Complete command table matches implementation (lines 219-242)
4. **Circuit Breaker Logic**: Zero-output agent detection accurately described (lines 25-26)
5. **Triage System**: Three-way classification system correctly documented (lines 202-212)

## File Structure Verification

### Manual Files
- ✅ `docs/manual/04-coordination.md` exists (373 lines)
- ✅ `docs/manual/04-coordination.typ` exists (large file)

### Related Service Files Referenced
- ✅ `.wg/service/state.json` (line 11)
- ✅ `.wg/service/daemon.log` (line 13)
- ✅ `.wg/service/daemon.sock` (line 215)
- ✅ `.wg/agents/registry.json` (line 115)

## Conclusion

Manual section 4 (Coordination) demonstrates **excellent documentation quality** with high accuracy between documented features and CLI implementation. All major command groups, configuration options, and architectural concepts are correctly documented.

**Validation Status: ✅ PASSED**

- [x] Service commands verified against current help
- [x] Coordinator configuration options verified  
- [x] Execution and dispatch features verified
- [x] Findings documented with file references and line numbers

## Recommendations

1. **Document Additional Options**: Add coverage for new `wg agents` filtering options and extended peer commands
2. **Update Chat Documentation**: Include advanced chat features like compaction and context sharing
3. **Maintain Accuracy**: Current documentation standard should be maintained for future updates

**Overall Assessment: HIGH QUALITY DOCUMENTATION** ✅