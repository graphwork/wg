# Unified Manual Audit Findings
**File:** docs/manual/workgraph-manual.md and workgraph-manual.typ  
**Audit Date:** 2026-04-12  
**Task:** audit-unified-manual  

## Executive Summary

The unified manual is generally well-written and accurate, but has **significant CLI coverage gaps**. While all commands mentioned in the manual exist and are accurately described, **57 CLI commands (57% of total CLI commands)** are completely absent from the manual.

## Key Findings

### ✅ PASSED: Commands in Manual vs CLI
- **All 44 commands mentioned in the manual exist in the CLI**
- **All command descriptions and flags are accurate**
- **No obsolete or incorrect commands found**

### ❌ FAILED: CLI Coverage Completeness  
**Critical Gap:** 57 of 99 CLI commands (57.6%) are not documented in the unified manual.

#### Missing Commands by Category

**Core Workflow (13 missing):**
- `abandon` - Mark task as permanently abandoned
- `add-dep` - Add dependency edge 
- `agents` - List running agent processes
- `archive` - Archive completed tasks
- `blocked` - Show what's blocking a task
- `claim` - Claim task for work
- `context` - Show available context for a task
- `kill` - Kill running agents
- `list` - List all tasks  
- `publish` - Publish draft task
- `ready` - List ready tasks
- `rm-dep` - Remove dependency edge
- `unclaim` - Release claimed task

**Analysis & Metrics (16 missing):**
- `aging` - Task age distribution
- `analyze` - Comprehensive health report
- `coordinate` - Coordination status
- `cost` - Calculate task cost
- `cycles` - Analyze structural cycles
- `discover` - Recently completed tasks
- `forecast` - Project completion forecast
- `gc` - Garbage collection
- `metrics` - Cleanup and monitoring metrics
- `stats` - Time counters and agent statistics
- `structure` - Graph structure analysis
- `trajectory` - Context-efficient task trajectory
- `velocity` - Task completion velocity
- `why-blocked` - Show blocking chain
- `workload` - Agent workload balance
- `bottlenecks` (mentioned only once, under-documented)

**Infrastructure & Setup (11 missing):**
- `cleanup` - Manual cleanup commands
- `dead-agents` - Detect and clean up dead agents
- `endpoints` - Manage LLM endpoints
- `heartbeat` - Agent heartbeat management
- `init` - Initialize new workgraph
- `key` - Manage API keys
- `quickstart` - Agent onboarding cheat sheet
- `reclaim` - Reclaim task from dead agent
- `reschedule` - Reschedule task timing
- `server` - Multi-user server setup
- `setup` - Interactive configuration wizard

**Advanced Features (9 missing):**
- `exec` - Interactive agent session
- `match` - Find capable agents
- `next` - Find best next task for agent
- `plan` - Budget/time constraint planning
- `resource` - Manage resources
- `resources` - Resource utilization
- `runs` - Manage run snapshots
- `skill` - Manage skills
- `tui-dump` - Dump TUI screen contents

**Communication & Integration (8 missing):**
- `matrix` - Matrix integration
- `msg` - Send/receive messages (mentioned but under-documented)
- `notify` - Send notifications
- `screencast` - Render TUI event traces
- `status` - One-screen status overview
- `telegram` - Telegram integration
- `tui` - Interactive TUI dashboard
- `user` - Manage user conversation boards

### ✅ PASSED: Content Consistency
- **Markdown (.md) and Typst (.typ) files have consistent content**
- **Glossary definitions are accurate and match CLI behavior**
- **Command examples and syntax are correct**

### ⚠️ MOSTLY PASSED: Glossary Accuracy
Verified key glossary entries against CLI:
- `wg wait` command matches glossary definition (line 52) ✅
- `wg checkpoint` behavior matches description (line 66) ✅ 
- `wg trace export --visibility` matches documented syntax (line 26) ✅
- Status values and lifecycle described accurately (line 14) ✅
- `--after` flag exists as documented (line 16) ✅
- `--visibility` options (internal, public, peer) are accurate (line 23) ✅
- `--delay` and `--not-before` timing options exist (line 239-240) ✅
- `--max-iterations` and `--no-converge` cycle options exist (line 386) ✅

**Minor Inaccuracies Found:**
- Line 16: Glossary states "`--after` (alias: `--blocked-by`)" but `--blocked-by` alias not found in CLI help. The `--after` flag exists and works correctly, but the alias may be outdated or incorrect.
- Line 60: Glossary mentions `--provider` on `wg add` as current functionality, but CLI help shows it as "[DEPRECATED]" with advice to use "provider:model format in --model instead". The glossary should be updated to reflect this deprecation.

## File-Specific Issues

### docs/manual/workgraph-manual.md
**Lines with command references verified:**
- Line 14: Status definitions - ✅ Accurate
- Line 23: `wg add --visibility` - ✅ Command exists, syntax correct
- Line 25: `wg trace` - ✅ Command exists
- Line 28: `wg replay` - ✅ Command exists
- Line 30: `wg tradeoff` - ✅ Command exists
- Line 49: `wg evolve` - ✅ Command exists
- Line 60-61: `wg add`/`wg edit` with `--provider` and `--exec-mode` - ✅ Options exist
- Line 64: `wg compact` - ✅ Command exists
- Line 65: `wg sweep` - ✅ Command exists
- Line 66: `wg checkpoint` - ✅ Command exists and behavior matches
- Line 67: `wg watch` - ✅ Command exists

**Command frequency in document:**
- `wg done`: 13 mentions ✅
- `wg add`: 10 mentions ✅  
- `wg watch`: 10 mentions ✅
- `wg fail`: 8 mentions ✅
- `wg edit`: 7 mentions ✅

### docs/manual/workgraph-manual.typ
- **Content matches markdown file exactly**
- **Typst formatting preserves all CLI references correctly**
- **Table structure maintains glossary accuracy**

## Recommendations

### Priority 1: Critical Coverage Gaps
1. **Add comprehensive CLI reference section** covering all 57 missing commands
2. **Add workflow examples** showing common command combinations
3. **Add troubleshooting section** covering `cleanup`, `dead-agents`, `reclaim`

### Priority 2: Enhanced Documentation
1. **Expand analysis commands section** (`analyze`, `metrics`, `structure`, etc.)
2. **Add setup and configuration section** (`init`, `setup`, `quickstart`)  
3. **Add communication/integration section** (`matrix`, `telegram`, `notify`)

### Priority 3: Structural Improvements
1. **Add command index/reference appendix**
2. **Cross-reference related commands** throughout sections
3. **Add "See also" references** to connect concepts with CLI commands

## Validation Results

- ✅ **Glossary terms verified** against current CLI help and behavior
- ❌ **CLI command references incomplete** - 57 commands missing (57.6% gap)
- ✅ **Unified manual consistency** between .md and .typ verified
- ✅ **Findings documented** with file references and line numbers

## Impact Assessment

**High Impact:** The 57% CLI coverage gap significantly undermines the manual's utility as a complete reference. Users following the manual will miss critical commands for:
- Daily workflow (`list`, `ready`, `agents`)  
- Project analysis (`analyze`, `metrics`, `structure`)
- Troubleshooting (`dead-agents`, `cleanup`, `reclaim`)
- Initial setup (`init`, `setup`, `quickstart`)

**Medium Impact:** Some commands are mentioned but under-documented:
- `wg msg` appears in glossary but lacks usage examples (CLI shows 4 subcommands: send, list, read, poll)
- `wg bottlenecks` mentioned only once
- `wg tui` mentioned but not explained

**Recommended Action:** Substantial manual expansion needed to achieve comprehensive CLI coverage.