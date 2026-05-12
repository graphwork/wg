# CLI Command Documentation Coverage Analysis

## Overview

This analysis compares the CLI command output from `wg --help-all` against the documentation in `docs/COMMANDS.md` to identify gaps and inconsistencies.

## CLI Commands (Total: 82)

Commands extracted from `wg --help-all`:

```
show, status, log, viz, msg, done, evaluate, abandon, artifact, list, fail, add, edit, chat, agent, service, assign, profile, role, publish, archive, agents, retry, unclaim, kill, ready, tui, quickstart, config, agency, resume, check, func, why-blocked, context, requeue, blocked, tradeoff, impact, telegram, trace, cycles, models, gc, model, evolve, claim, endpoints, dead-agents, pause, peer, spawn, critical-path, key, rm-dep, compact, metrics, add-dep, velocity, analyze, bottlenecks, discover, coordinate, exec, replay, server, setup, aging, forecast, notify, stats, workload, cost, reschedule, skill, structure, sweep, reclaim, runs, tui-dump, cleanup, init, match, plan, resource, resources, screencast, spend, approve, checkpoint, heartbeat, matrix, next, openrouter, reject, trajectory, user, wait, watch
```

## Documented Commands (Total: 76)

Commands documented in `docs/COMMANDS.md`:

```
add, edit, done, fail, abandon, retry, requeue, claim, unclaim, reclaim, log, assign, show, pause, resume, approve, reject, publish, add-dep, rm-dep, wait, list, ready, blocked, why-blocked, impact, context, status, discover, bottlenecks, critical-path, forecast, velocity, aging, structure, cycles, workload, analyze, cost, plan, coordinate, resources, skill, match, matrix, notify, role, tradeoff, evaluate, evolve, spawn, next, exec, trajectory, heartbeat, agents, kill, dead-agents, checkpoint, watch, stats, metrics, msg, chat, model, key, models, endpoints, profile, spend, openrouter, init, cleanup, check, viz, archive, reschedule, artifact, config, quickstart, tui, setup, replay, runs, gc, compact, sweep, telegram, screencast, server, user, tui-dump
```

## Gap Analysis

### 1. Commands in CLI but NOT documented in COMMANDS.md (6 commands)

- **`agency`** - "Manage the agency (roles + tradeoffs)" - Missing documentation
- **`func`** - "Function management: extract, apply, list, show, bootstrap" - Missing documentation  
- **`peer`** - "Manage peer wg instances for cross-repo communication" - Missing documentation
- **`resource`** - "Manage resources" - Missing documentation (note: `resources` IS documented)
- **`service`** - "Manage the agent service daemon" - Missing documentation
- **`trace`** - "Trace commands: execution history, export, import" - Missing documentation

### 2. Commands documented but NOT in CLI (0 commands)

All documented commands appear in the CLI help output.

### 3. Command Description Consistency Analysis

Comparing CLI help descriptions with COMMANDS.md documentation:

#### Consistent descriptions:
- Most commands have consistent basic descriptions between CLI help and COMMANDS.md

#### Potential inconsistencies to verify:
- **`resource` vs `resources`**: CLI has both `resource` and `resources` commands, but COMMANDS.md only documents `resources`

## Detailed Gap Analysis for Missing Commands

### `agency` Command
- **CLI Description**: "Manage the agency (roles + tradeoffs)"
- **Status**: No documentation in COMMANDS.md
- **Impact**: High - This appears to be a key command for agency management

### `func` Command  
- **CLI Description**: "Function management: extract, apply, list, show, bootstrap"
- **Status**: No documentation in COMMANDS.md
- **Impact**: High - Function management is a significant feature

### `peer` Command
- **CLI Description**: "Manage peer wg instances for cross-repo communication"  
- **Status**: No documentation in COMMANDS.md
- **Impact**: Medium - Cross-repo communication functionality

### `resource` Command
- **CLI Description**: "Manage resources"
- **Status**: No documentation in COMMANDS.md (but `resources` is documented)
- **Impact**: Medium - May be related to but distinct from `resources`

### `service` Command
- **CLI Description**: "Manage the agent service daemon"
- **Status**: No documentation in COMMANDS.md  
- **Impact**: High - Service management is core functionality

### `trace` Command
- **CLI Description**: "Trace commands: execution history, export, import"
- **Status**: No documentation in COMMANDS.md
- **Impact**: Medium - Tracing/debugging functionality

## Coverage Assessment

- **Total CLI Commands**: 82
- **Documented Commands**: 76  
- **Documentation Coverage**: 92.7% (76/82)
- **Undocumented Commands**: 6 (7.3%)

## Recommendations

### High Priority (Missing Documentation)

1. **Document `agency` command** - Core agency management functionality
2. **Document `service` command** - Essential service daemon management  
3. **Document `func` command** - Function management capabilities

### Medium Priority

4. **Document `peer` command** - Cross-repo communication features
5. **Document `trace` command** - Execution history and debugging
6. **Clarify `resource` vs `resources`** - Verify if these are distinct commands or if documentation is missing

### Documentation Quality Improvements

7. **Add usage examples** for complex commands missing them
8. **Verify command descriptions** match between CLI help and documentation
9. **Check for any commands with changed behavior** since documentation was written

## Additional Findings

### Commands with Complex Functionality (May Need Usage Examples)
Based on CLI descriptions, these commands appear complex and should have comprehensive examples:
- `evaluate` - "Evaluate tasks: auto-evaluate, record external scores, view history"
- `evolve` - "Trigger an evolution cycle, or review deferred operations"  
- `replay` - "Replay tasks: snapshot graph, selectively reset tasks, re-execute with a different model"
- `trajectory` - "Show context-efficient task trajectory (claim order for minimal context switching)"