use anyhow::Result;

const QUICKSTART_TEXT: &str = r###"
╔══════════════════════════════════════════════════════════════════════════════╗
║                            WG AGENT QUICKSTART                               ║
╚══════════════════════════════════════════════════════════════════════════════╝

GETTING STARTED
  wg init
  wg setup --route pi --yes --model pi:<provider>:<model>
  wg agency init
  wg service start
  wg add "My first task"
  wg publish my-first-task --only
  wg status

Pi is the recommended LLM model plane. Pi owns provider login, model discovery,
availability, endpoint details, support validation, and reported cost. Native
Claude and Codex CLI workers and live chats are available only by explicit
handler-first selection; each CLI owns its login and native model IDs. WG owns
exact routes plus inherited reasoning. Opening a graph or TUI never creates a
route or requires credentials.

SKILL & BUNDLE SETUP
  wg skill install
  Spawned agents need the WG guide/bundle for task-management commands.

AGENCY SETUP
  `wg agency init` creates Roles, Tradeoffs, and an agent identity.
  wg config --models              # effective Pi route + reasoning for every role
  wg config --set-model <role> pi:<provider>:<model>
  wg config --set-reasoning <role> <level>

DISPATCHER SERVICE REMINDER
  If the service is running, define/publish work; do not manually claim/spawn.

SERVICE MODE
  wg service start --max-agents 5
  wg service status
  wg agents
  wg agents kill <agent-id> --force
  wg screencast
  wg tui-dump
  wg server

MANUAL MODE
  wg ready
  wg claim <task-id>
  wg done <task-id>

DISCOVERING & ADDING WORK
  wg list --status open
  wg show <task-id>
  wg add "X" --after Y
  wg add "X" --model pi:<provider>:<model> --reasoning high
  wg add "X" --timeout 30m --cron "0 0 9 * * *" --independent
  wg add "X" --context-scope clean
  wg add "X" --context-scope task
  wg add "X" --context-scope graph
  wg add "X" --context-scope full
  wg add-dep <task> <dependency>
  wg rm-dep <task> <dependency>

TASK STATE COMMANDS
  wg done <task-id>
  wg done <task-id> --converged  # Complete and STOP the loop
  wg fail <task-id> --reason "..."
  wg unclaim <task-id>
  wg requeue <task-id> --reason "..."
  wg wait <task-id> --until "task:dep-a=done"
  wg wait <task-id> --until "timer:5m"
  wg resume <task-id> --only  # Explicitly satisfy one Waiting task

VALIDATION (## Validation section in task description)
  Put required checks under `## Validation`; this prose contract is the default.
  Workers choose relevant checks and report candidate vs baseline/environment failures.
  Exact executable hard gates are operator/repository-authorized exceptions.
  Agents must not invent or broaden them.

MESSAGING
  wg msg send <task-id> "message"
  wg msg read <task-id>
  wg msg poll <task-id>

CONTEXT & ARTIFACTS
  wg context <task-id>
  wg artifact <task-id> path
  wg log <task-id> "progress"

CYCLES
  The task graph supports explicit cycles with --max-iterations.
  IMPORTANT — Signaling convergence:
  wg done <task-id> --converged

DISCOVERY & PUBLISHING
  wg discover --with-artifacts
  wg publish <task-id> --only
  wg reclaim <task-id> --from <actor> --to <actor>

RECOVERY
  Hung worker agent: wg agents kill <agent-id>
  wg retry <task-id> --reason "stalled"
  wg recover --yes

HOUSEKEEPING
  wg archive --older 7d
  wg gc --dry-run
  wg cleanup orphaned
  wg cleanup nightly
  wg metrics

GROWING THE GRAPH
  Grow the graph when a prerequisite, follow-up, or independent investigation appears.

TIPS
  Use `wg show`, log progress, validate, commit, push, check messages, then `wg done`.

PI MODEL PLANE (RECOMMENDED; NATIVE CLI ROUTES EXPLICIT)
  Pi route:       pi:<provider>:<model>
  Native Claude:  claude:<native-model> # explicit workers/tasks + live chat
  Native Codex:   codex:<native-model>  # explicit workers/tasks + live chat
  wg config -m pi:<provider>:<model>
  wg profile select claude
  wg profile select codex
  wg add "Task" --model claude:<native-model> --reasoning high
  Each CLI owns model discovery/validation and authentication; WG never falls
  back across Pi/Claude/Codex execution systems.

REUSABLE FUNCTIONS
  wg func list
  wg func apply <id> --input key=value

VISUALIZATION
  wg viz --show-internal
  wg viz --no-tui
  wg tui

CONFIGURATION
  wg config --list
  wg config --models
  wg config --global --model pi:<provider>:<model>
  wg config --creator-agent <hash>
  wg config --creator-model pi:<provider>:<model>
  wg config lint
  Legacy model-plane fields are migration-only and never Pi dispatch authority.

TRACE, RUNS & REPLAY
  wg trace show <task-id>
  wg runs list
  wg replay --below-score 0.7 --subgraph <task-id>

ANALYSIS
  wg analyze
  wg bottlenecks
  wg critical-path
  wg forecast
  wg velocity
  wg aging
  wg workload
  wg coordinate
  wg impact <task-id>
  wg plan --hours 8
  wg cost <task-id>

DEAD AGENT DETECTION
  wg dead-agents --purge --delete-dirs

PEER WG PROJECTS
  wg peer add <name> <path>
  wg add "Task" --repo <peer>

WG-FED IDENTITY & CROSS-GRAPH FEDERATION
  wg identity list
  wg msg poll --as <identity>

CONTENT-SAFETY REVIEW GATE
  wg review check --class IC1 --content-file <file>

AGENCY FEEDBACK & MIGRATION
  wg assign <task-id> --auto           # next-attempt intent; receipt on claim
  wg reviews list <task-id>            # immutable candidate evidence (read-only)
  wg evaluate run <done-task>          # post-terminal learning score only
  wg learning show <task-id>           # episode + delayed reward/evolver input
  wg migrate evaluation-cutover --dry-run  # retire obsolete graph authority
  Only the completion controller applies candidate receipts to task lifecycle.

MONITORING
  wg watch --task <id>

NOTIFICATION & COMMUNICATION
  wg telegram send "message"
  wg telegram ask "question"

RESOURCE MANAGEMENT
  wg resource add
  wg resources

PROVIDER PROFILES
  wg profile list                  # Pi recommended; direct Claude/Codex selectable
  wg profile select pi
  wg profile select codex
  wg profile pi --show

USER BOARDS
  wg user init

COST & SPENDING
  wg spend                         # Pi-reported usage/cost only
"###;

fn json_output() -> serde_json::Value {
    serde_json::json!({
        "getting_started": [
            "wg init",
            "wg setup --route pi --yes --model pi:<provider>:<model>",
            "wg agency init",
            "wg service start",
            "wg add \"My first task\"",
            "wg status"
        ],
        "skill_bundle_setup": {
            "description": "Spawned agents need the right skill or bundle installed to understand wg commands.",
            "claude": {
                "install": "wg skill install",
                "location": "~/.claude/skills/wg/SKILL.md",
                "note": "Injected into every Claude Code session automatically"
            },
            "custom": "Ensure your executor's agent prompt includes wg CLI instructions"
        },
        "agency": {
            "description": "Agency gives the service agents to assign to tasks.",
            "quick_setup": "wg agency init",
            "concepts": {
                "roles": "What agents do (skills + desired outcome)",
                "tradeoffs": "Constraints on how agents work (acceptable/unacceptable trade-offs)",
                "agents": "A role + tradeoff pairing that gets assigned to tasks"
            },
            "manual_setup": [
                "wg role add \"Name\" --outcome \"...\" --skill name",
                "wg tradeoff add \"Name\" --accept \"...\" --reject \"...\"",
                "wg agent create \"Name\" --role <hash> --tradeoff <hash>",
                "wg config --auto-assign true"
            ],
            "authority": {
                "completion_review": "Exact candidate receipts; completion controller alone applies lifecycle policy",
                "candidate_review": "wg reviews; immutable virtual evidence, never a graph task",
                "scored_outcome": "wg evaluate run/show; post-terminal learning only",
                "external_outcome": "wg evaluate record; outcome score that cannot accept a candidate",
                "legacy_evaluation": "wg migrate evaluation-cutover; preserve history and retire obsolete graph authority"
            },
            "placement": {
                "description": "auto_place is a separate placement subsystem; assignment selection only binds identity to a real attempt receipt.",
                "enable": "wg config --auto-place true"
            },
            "auto_create": {
                "description": "When auto_create is enabled, the dispatcher invokes the creator agent to discover and add new primitives when the store needs expansion.",
                "enable": "wg config --auto-create true"
            },
            "pi_routing": {
                "show_roles": "wg config --models",
                "set_model": "wg config --set-model <role> pi:<provider>:<model>",
                "set_reasoning": "wg config --set-reasoning <role> <level>"
            }
        },
        "modes": {
            "service": {
                "description": "Recommended for parallel work. Dispatcher daemon spawns worker agents automatically.",
                "start": "wg service start --max-agents 5",
                "install_systemd": "wg service install (generate systemd user service file)",
                "workflow": "Add tasks with dependencies → dispatcher spawns worker agents on ready tasks",
                "warning": "Do NOT manually wg spawn or wg claim while the service is running",
                "monitor": ["wg service status", "wg agents", "wg list", "wg tui"],
                "control": {
                    "pause": "wg service pause (no new spawns, running agents continue)",
                    "resume": "wg service resume",
                    "freeze": "wg service freeze (SIGSTOP all agents + pause)",
                    "thaw": "wg service thaw (SIGCONT agents + resume)"
                },
                "kill_agent": "wg kill <agent-id> (pauses task by default)",
                "kill_agent_redispatch": "wg kill <agent-id> --redispatch (leave task open)",
                "kill_tree": "wg kill --tree <agent-id> (cascade-abandon all downstream tasks)",
                "kill_all": "wg kill --all (pauses all tasks)",
                "reap": "wg reap (garbage-collect dead/done/failed agents from registry)"
            },
            "manual": {
                "description": "For when no service is running. You claim and work tasks yourself.",
                "workflow": ["wg ready", "wg claim <task-id>", "wg log <task-id> \"msg\"", "wg done <task-id>"]
            }
        },
        "commands": {
            "discovery": {
                "list": "List all tasks",
                "show": "View task details and context",
                "add": "Add a visible draft task (supports --context-scope, --exec-mode, explicit --model pi:<provider>:<model> or codex:<native-model>, --reasoning, scheduling, placement, skills, and --independent); release with wg publish <task-id> --only",
                "edit": "Edit an existing task (title, description, deps, model, tags, etc.)",
                "ready": "See tasks available to work on (manual mode)",
                "status": "Quick one-screen status overview"
            },
            "work": {
                "claim": "Claim a task for work (manual mode only)",
                "log": "Log progress as you work",
                "context": "See context from dependencies",
                "artifact": "Record output file/artifact"
            },
            "completion": {
                "done": "Mark task complete",
                "done_converged": "Complete task and stop loop (wg done <id> --converged)",
                "fail": "Mark failed (can be retried)",
                "retry": "Retry a failed/incomplete/evaluation-held task (resets to open)",
                "abandon": "Give up permanently",
                "pause": "Pause task (dispatcher skips it until resumed)",
                "wait": "Park task until condition met (wg wait <id> --until \"condition\")",
                "resume": "Resume a paused/waiting task",
                "reschedule": "Set not_before timestamp (wg reschedule <id> --after 24)",
                "unclaim": "Release a claimed task back to open",
                "requeue": "Requeue in-progress task for triage (wg requeue <id> --reason \"...\")"
            },
            "dependencies": {
                "add_dep": "Add a dependency edge (wg add-dep <task> <dependency>)",
                "rm_dep": "Remove a dependency edge (wg rm-dep <task> <dependency>)"
            }
        },
        "validation": {
            "description": "Validation criteria live in a ## Validation section. This prose contract is the default: workers choose relevant checks, report results, and distinguish candidate regressions from baseline or environmental failures.",
            "create": "wg add \"task\" -d \"## Validation\\n- [ ] criteria here\"",
            "note": "Exact executable checks are optional operator/repository-authorized hard gates. Agents must not invent or broaden them. Completion review is candidate-bound lifecycle input consumed by the controller; candidate-review history is read-only; scored Agency outcomes are separate post-terminal learning signals."
        },
        "messaging": {
            "description": "Inter-agent and task-scoped messaging. Agents must check messages before and after working.",
            "send": "wg msg send <task-id> \"message\"",
            "list": "wg msg list <task-id>",
            "read": "wg msg read <task-id>",
            "poll": "wg msg poll <task-id>",
            "agent_filter": "Use --agent <id> with read/poll to filter by agent identity"
        },
        "wait_conditions": {
            "description": "Park a task until a condition is met.",
            "task": "wg wait <id> --until \"task:dep-a=done\"",
            "timer": "wg wait <id> --until \"timer:5m\"",
            "message": "wg wait <id> --until \"message\"",
            "human-input": "wg wait <id> --until \"human-input\"",
            "file": "wg wait <id> --until \"file:path/to/file\""
        },
        "discovery_publishing": {
            "discover": "wg discover",
            "discover_since": "wg discover --since 7d",
            "discover_artifacts": "wg discover --with-artifacts",
            "publish": "wg publish <task-id>",
            "publish_only": "wg publish <task-id> --only",
            "html_publish_add": "wg html publish add <name> --rsync user@host:/path/",
            "html_publish_run": "wg html publish run <name>",
            "html_publish_list": "wg html publish list",
            "reclaim": "wg reclaim <task-id> --from <actor> --to <actor>"
        },
        "exec_modes": {
            "description": "Control agent capabilities per task.",
            "modes": {
                "full": "Default: full agent with all tools",
                "light": "Read-only tools (research/review tasks)",
                "bare": "Only wg CLI (coordination-only tasks)",
                "shell": "Shell command, no LLM (use with wg exec --set)"
            },
            "usage": "wg add \"task\" --exec-mode <mode>"
        },
        "scheduling": {
            "delay": "wg add \"task\" --delay 1h",
            "not_before": "wg add \"task\" --not-before 2026-04-01T09:00:00Z",
            "cron": "wg add \"task\" --cron \"0 0 9 * * *\" (6-field: sec min hour day month dow)",
            "timeout": "wg add \"task\" --timeout 30m (per-task timeout)",
            "independent": "wg add \"task\" --independent (suppress implicit --after)",
            "placement": {
                "place_near": "wg add \"task\" --place-near task-a",
                "place_before": "wg add \"task\" --place-before task-b"
            }
        },
        "cycles": {
            "description": "Structural cycles model repeating workflows via after back-edges with CycleConfig.",
            "create": "wg edit write --add-after review --max-iterations 3",
            "inspect": ["wg show <task-id>", "wg cycles"],
            "convergence": "IMPORTANT: Use 'wg done <task-id> --converged' to stop a cycle when work is complete. Plain 'wg done' causes the cycle to iterate again.",
            "advanced": {
                "no_converge": "wg add \"X\" --after Y --max-iterations 5 --no-converge (force all iterations)",
                "no_restart_on_failure": "wg add \"X\" --after Y --max-iterations 10 --no-restart-on-failure",
                "max_failure_restarts": "wg add \"X\" --after Y --max-iterations 10 --max-failure-restarts 1"
            }
        },
        "growing_the_graph": {
            "ethos": "The graph is a shared medium. You are not isolated — you are part of a living system. Your job is not just to complete your task, but to leave the system better than you found it.",
            "the_loop": "spec → implement → verify → improve → spec. Use 'wg context' to see what came before. Use 'wg add' to create what comes next.",
            "examples": {
                "found_bug": "wg add \"Fix: ...\" --after <task-id> -d \"Found while working on <task-id>\"",
                "docs_wrong": "wg add \"Fix docs for X\" -d \"Spotted while reading ...\"",
                "followup": "wg add \"Verify: ...\" --after <task-id>"
            },
            "guidance": "Add creates a visible draft; the dispatcher acts only after explicit publish. If a fix takes 5 minutes, do it inline. Create and publish tasks for work that benefits from separate focus."
        },
        "tips": [
            "If the dispatcher is running: add visible drafts with dependencies, then publish them explicitly",
            "If no dispatcher: ready → claim → work → done",
            "Run 'wg log' BEFORE starting work to track progress",
            "Use 'wg context' to understand what dependencies produced",
            "Check 'wg blocked <task-id>' if a task isn't appearing in ready list",
            "Use 'wg why-blocked <task-id>' for the full transitive blocking chain",
            "Confused which graph wg is talking to? Run 'wg which'"
        ],
        "named_profiles": {
            "description": "Reusable global profile definitions with explicit fingerprint-pinned per-project selection; legacy global activation remains separate.",
            "select": "wg profile select <name> (current project only; never rewrites global config)",
            "select_clear": "wg profile select --clear (current project only)",
            "history": "wg profile history [--clear] (privacy-bounded local successful-event records)",
            "use": "wg profile use <name> (legacy GLOBAL activation; writes ~/.wg/active-profile and ~/.wg/config.toml)",
            "show": "wg profile show",
            "list": "wg profile list",
            "create": "wg profile create <name>",
            "edit": "wg profile edit <name>",
            "diff": "wg profile diff <a> <b>",
            "init_starters": "wg profile init-starters (writes recommended Pi + explicit Claude/Codex CLI starters)",
            "starters": [
                "pi (recommended model plane)",
                "claude (explicit native CLI workers + live chat)",
                "codex (explicit native CLI workers + live chat)"
            ]
        },
        "pi_model_plane": {
            "owner": "Pi",
            "wg_retains": ["exact per-role route", "effective reasoning", "audit identity", "Pi-reported usage/cost"],
            "pi_retains": ["provider auth", "model discovery", "availability", "endpoint details", "support validation"]
        },
        "shell_execution": {
            "set_command": "wg exec --set <task> \"command\"",
            "run": "wg exec <task>",
            "dry_run": "wg exec --dry-run <task>",
            "clear": "wg exec --clear <task>"
        },
        "compact_sweep_checkpoint": {
            "compact": "wg compact",
            "sweep": "wg sweep",
            "sweep_dry_run": "wg sweep --dry-run",
            "checkpoint": "wg checkpoint <task> -s \"summary\"",
            "checkpoint_list": "wg checkpoint <task> --list",
            "stats": "wg stats"
        },
        "multi_chat": {
            "description": "Each chat agent is a graph entity (.chat-N). The canonical command surface is `wg chat <subcommand>`; wg service aliases below preserve back-compat with the old coordinator-named subcommands.",
            "create": "wg chat create (or: wg service create-chat)",
            "stop": "wg chat stop <id> (or: wg service stop-chat <n>)",
            "archive": "wg chat archive <id> (or: wg service archive-chat <n>)",
            "delete": "wg chat delete <id> (or: wg service delete-chat <n>)",
            "interrupt": "wg service interrupt-chat <n>",
            "set_route": "wg chat model <id> pi:<provider>:<model> (hot-swap; respawn preserves history)",
            "purge": "wg service purge-chats (bulk-purge all chat agents; reversible via wg chat create)"
        },
        "chat": {
            "subcommands": {
                "create": "wg chat create",
                "list": "wg chat list",
                "show": "wg chat show <id>",
                "attach": "wg chat attach <id>",
                "send": "wg chat send <id> \"message\"",
                "stop": "wg chat stop <id>",
                "resume": "wg chat resume <id>",
                "archive": "wg chat archive <id>",
                "delete": "wg chat delete <id>"
            },
            "default_form": "wg chat \"message\" (one-shot send to default chat)",
            "interactive": "wg chat -i",
            "attachment": "wg chat --attachment file.txt",
            "coordinator_legacy": "wg chat --coordinator 1 (legacy flag for targeting a specific chat)",
            "history": "wg chat --history",
            "clear": "wg chat --clear"
        },
        "housekeeping": {
            "archive": "wg archive",
            "archive_older": "wg archive --older 7d",
            "archive_list": "wg archive --list",
            "gc": "wg gc",
            "gc_dry_run": "wg gc --dry-run",
            "gc_include_done": "wg gc --include-done",
            "gc_older": "wg gc --older 7d",
            "cleanup_orphaned": "wg cleanup orphaned",
            "cleanup_branches": "wg cleanup recovery-branches",
            "cleanup_nightly": "wg cleanup nightly",
            "metrics": "wg metrics"
        },
        "functions": {
            "description": "Reusable workflow patterns extracted from completed tasks.",
            "commands": {
                "list": "wg func list",
                "show": "wg func show <id>",
                "apply": "wg func apply <id> --input k=v",
                "extract": "wg func extract a b c"
            }
        },
        "visualization": {
            "viz": "wg viz",
            "viz_all": "wg viz --all",
            "viz_focus": "wg viz <task-id>...",
            "viz_critical_path": "wg viz --critical-path",
            "viz_dot": "wg viz --dot",
            "viz_mermaid": "wg viz --mermaid",
            "viz_show_internal": "wg viz --show-internal",
            "viz_no_tui": "wg viz --no-tui",
            "tui": "wg tui"
        },
        "configuration": {
            "show": "wg config --show",
            "list": "wg config --list (merged config with source annotations)",
            "global": "wg config --global (target ~/.wg/config.toml)",
            "local": "wg config --local (target .wg/config.toml)",
            "creator_agent": "wg config --creator-agent <hash>"
        },
        "context_scopes": {
            "description": "Control how much context the dispatcher injects into agent prompts.",
            "levels": {
                "clean": "Minimal: just the task description",
                "task": "Standard default: task + predecessor context",
                "graph": "Task + transitive dependency chain",
                "full": "Everything: full graph state"
            },
            "usage": "wg add \"task\" --context-scope <level>"
        },
        "trace_runs_replay": {
            "trace": {
                "show": "wg trace show <task-id>",
                "export": "wg trace export --visibility public",
                "import": "wg trace import <file>"
            },
            "runs": {
                "list": "wg runs list",
                "show": "wg runs show <run>",
                "diff": "wg runs diff <run>",
                "restore": "wg runs restore <run>"
            },
            "replay": {
                "failed_only": "wg replay --failed-only",
                "with_model": "wg replay --model <model>",
                "below_score": "wg replay --below-score 0.7",
                "subgraph": "wg replay --subgraph <task-id>",
                "keep_done": "wg replay --keep-done 0.9"
            }
        },
        "analysis": {
            "analyze": "wg analyze (comprehensive health report)",
            "structure": "wg structure (entry points, dead ends, fan-out)",
            "bottlenecks": "wg bottlenecks (tasks blocking the most downstream work)",
            "critical_path": "wg critical-path (longest dependency chain)",
            "forecast": "wg forecast (completion date from velocity)",
            "velocity": "wg velocity (task completion rate per week)",
            "aging": "wg aging (task age distribution)",
            "workload": "wg workload (agent workload balance)",
            "coordinate": "wg coordinate (coordination status: ready, in-progress, parallel opportunities)",
            "impact": "wg impact <task-id> (downstream impact analysis)",
            "plan": "wg plan --hours 8 (plan work within a budget)",
            "cost": "wg cost <task-id> (calculate cost including dependencies)"
        },
        "dead_agents": {
            "detect": "wg dead-agents",
            "cleanup": "wg dead-agents --cleanup",
            "purge": "wg dead-agents --purge",
            "purge_with_dirs": "wg dead-agents --purge --delete-dirs",
            "threshold": "wg dead-agents --threshold <minutes>"
        },
        "peer_graphs": {
            "add": "wg peer add <name> <path>",
            "add_key_based": "wg peer add <name> --wgid <W> --endpoint <U>",
            "list": "wg peer list",
            "status": "wg peer status",
            "cross_repo_task": "wg add \"task\" --repo <peer>"
        },
        "federation": {
            "description": "WG-Fed: self-certifying wgid: identity, signed cross-graph messages, portable/recoverable state.",
            "identity_new": "wg identity new <name>",
            "identity_show": "wg identity show <name> | wg identity list",
            "publish": "wg identity publish <name> --store <L>",
            "fetch": "wg identity fetch <wgid> --store <L> [--save <name>]",
            "node": "wg fed-node serve --addr <H:P>",
            "msg_send": "wg msg send --to <wgid> --from <id> --body \"…\" [--seal]",
            "msg_poll": "wg msg poll --as <id> [--store <url>] [--require-fresh high-value]",
            "recovery": "wg identity rotate|revoke|recover|fork|enroll-signer",
            "delegation": "wg identity delegate|verify-cap|revoke-cap",
            "load_state": "wg identity load-state <name> --store <L>"
        },
        "content_safety_review": {
            "description": "WG-Review: screen inbound task/code/state/msg BEFORE an agent consumes it (accept/quarantine/reject, fail-closed).",
            "check": "wg review check --class IC1 --trust unknown --content-file <f> [--author <wgid>]",
            "depth": "wg review depth --trust <t> [--sensitivity low|high]",
            "reviewer_scope": "wg review reviewer-scope",
            "log": "wg review log",
            "consume": "wg review consume --content-file <f>",
            "revoke": "wg review revoke --cid <b3:…>"
        },
        "evaluation_and_monitoring": {
            "description": "Candidate review, terminal outcome scoring, and legacy migration have separate authority; only the completion controller applies candidate receipts to lifecycle.",
            "assign_auto": "wg assign <task-id> --auto",
            "reviews": "wg reviews list <task-id>",
            "evaluate_run": "wg evaluate run <done-task>",
            "evaluate_show": "wg evaluate show",
            "learning": "wg learning show <task-id>",
            "legacy_migration": "wg migrate evaluation-cutover --dry-run",
            "watch": "wg watch",
            "watch_task": "wg watch --task <id>"
        },
        "notification": {
            "telegram": {
                "send": "wg telegram send \"message\"",
                "ask": "wg telegram ask \"question\"",
                "poll": "wg telegram poll",
                "status": "wg telegram status"
            },
            "matrix": "wg matrix",
            "notify": "wg notify"
        },
        "resources": {
            "add": "wg resource add",
            "list": "wg resource list",
            "utilization": "wg resources"
        },
        "profiles": {
            "list": "wg profile list",
            "show": "wg profile show",
            "select": "wg profile select pi",
            "configure": "wg profile pi --show"
        },
        "user_boards": {
            "init": "wg user init",
            "list": "wg user list",
            "archive": "wg user archive"
        },
        "cost_spending": {
            "spend": "wg spend (Pi-reported cost only)",
            "spend_today": "wg spend --today"
        },
        "advanced_service": {
            "screencast": "wg screencast",
            "tui_dump": "wg tui-dump",
            "server": "wg server"
        }
    })
}

pub fn run(json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&json_output())?);
    } else {
        println!("{}", QUICKSTART_TEXT.trim());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quickstart_text_contains_service_mode() {
        assert!(QUICKSTART_TEXT.contains("SERVICE MODE"));
    }

    #[test]
    fn test_quickstart_text_contains_manual_mode() {
        assert!(QUICKSTART_TEXT.contains("MANUAL MODE"));
    }

    #[test]
    fn test_quickstart_text_contains_discovering_work() {
        assert!(QUICKSTART_TEXT.contains("DISCOVERING & ADDING WORK"));
    }

    #[test]
    fn test_quickstart_text_contains_task_state_commands() {
        assert!(QUICKSTART_TEXT.contains("TASK STATE COMMANDS"));
    }

    #[test]
    fn test_quickstart_text_contains_context_artifacts() {
        assert!(QUICKSTART_TEXT.contains("CONTEXT & ARTIFACTS"));
    }

    #[test]
    fn test_quickstart_text_contains_cycles() {
        assert!(QUICKSTART_TEXT.contains("CYCLES"));
    }

    #[test]
    fn test_quickstart_text_contains_tips() {
        assert!(QUICKSTART_TEXT.contains("TIPS"));
    }

    #[test]
    fn test_quickstart_text_contains_dispatcher_reminder() {
        assert!(QUICKSTART_TEXT.contains("DISPATCHER SERVICE REMINDER"));
    }

    #[test]
    fn test_quickstart_text_contains_getting_started() {
        assert!(QUICKSTART_TEXT.contains("GETTING STARTED"));
        assert!(QUICKSTART_TEXT.contains("wg agency init"));
    }

    #[test]
    fn test_quickstart_text_contains_agency_setup() {
        assert!(QUICKSTART_TEXT.contains("AGENCY SETUP"));
        assert!(QUICKSTART_TEXT.contains("Roles"));
        assert!(QUICKSTART_TEXT.contains("Tradeoffs"));
    }

    #[test]
    fn test_run_text_mode_succeeds() {
        assert!(run(false).is_ok());
    }

    #[test]
    fn test_run_json_mode_succeeds() {
        assert!(run(true).is_ok());
    }

    #[test]
    fn test_json_output_has_expected_fields() {
        let output = json_output();

        // Check top-level keys
        assert!(output.get("getting_started").is_some());
        assert!(output.get("agency").is_some());
        assert!(output.get("modes").is_some());
        assert!(output.get("commands").is_some());
        assert!(output.get("cycles").is_some());
        assert!(output.get("tips").is_some());

        // Check getting_started is an array
        let gs = output.get("getting_started").unwrap().as_array().unwrap();
        assert!(gs.len() >= 3);

        // Check agency fields
        let agency = output.get("agency").unwrap();
        assert!(agency.get("quick_setup").is_some());
        assert!(agency.get("concepts").is_some());

        // Check modes
        let modes = output.get("modes").unwrap();
        assert!(modes.get("service").is_some());
        assert!(modes.get("manual").is_some());

        // Check commands sub-sections
        let commands = output.get("commands").unwrap();
        assert!(commands.get("discovery").is_some());
        assert!(commands.get("work").is_some());
        assert!(commands.get("completion").is_some());

        // Check cycles fields
        let cycles = output.get("cycles").unwrap();
        assert!(cycles.get("description").is_some());
        assert!(cycles.get("create").is_some());
        assert!(cycles.get("inspect").is_some());

        // Check growing_the_graph section
        let gtg = output.get("growing_the_graph").unwrap();
        assert!(gtg.get("ethos").is_some());
        assert!(gtg.get("the_loop").is_some());
        assert!(gtg.get("examples").is_some());

        // Check tips is an array with entries
        let tips = output.get("tips").unwrap().as_array().unwrap();
        assert!(!tips.is_empty());
        assert!(tips.len() >= 5);

        // Check the sole Pi model-plane section.
        let plane = output.get("pi_model_plane").unwrap();
        assert_eq!(plane["owner"], "Pi");

        // Check housekeeping section
        let hk = output.get("housekeeping").unwrap();
        assert!(hk.get("archive").is_some());
        assert!(hk.get("gc").is_some());

        // Check functions section
        let funcs = output.get("functions").unwrap();
        assert!(funcs.get("commands").is_some());

        // Check evaluation_and_monitoring section
        let eval = output.get("evaluation_and_monitoring").unwrap();
        assert!(eval.get("evaluate_run").is_some());
        assert!(eval.get("watch").is_some());
    }

    #[test]
    fn test_quickstart_text_contains_pi_model_plane() {
        assert!(QUICKSTART_TEXT.contains("PI MODEL PLANE"));
        assert!(QUICKSTART_TEXT.contains("wg config -m pi:<provider>:<model>"));
        assert!(QUICKSTART_TEXT.contains("--model"));
        assert!(!QUICKSTART_TEXT.contains("wg model list"));
    }

    #[test]
    fn test_quickstart_text_contains_functions() {
        assert!(QUICKSTART_TEXT.contains("REUSABLE FUNCTIONS"));
        assert!(QUICKSTART_TEXT.contains("wg func list"));
        assert!(QUICKSTART_TEXT.contains("wg func apply"));
    }

    #[test]
    fn test_quickstart_text_contains_housekeeping() {
        assert!(QUICKSTART_TEXT.contains("HOUSEKEEPING"));
        assert!(QUICKSTART_TEXT.contains("wg archive"));
        assert!(QUICKSTART_TEXT.contains("wg gc"));
    }

    #[test]
    fn test_quickstart_text_contains_agency_feedback_and_monitoring() {
        assert!(QUICKSTART_TEXT.contains("AGENCY FEEDBACK & MIGRATION"));
        assert!(QUICKSTART_TEXT.contains("wg reviews list"));
        assert!(QUICKSTART_TEXT.contains("wg evaluate run"));
        assert!(QUICKSTART_TEXT.contains("wg learning show"));
        assert!(QUICKSTART_TEXT.contains("wg migrate evaluation-cutover"));
        assert!(QUICKSTART_TEXT.contains("MONITORING"));
        assert!(QUICKSTART_TEXT.contains("wg watch"));
    }

    #[test]
    fn test_quickstart_text_contains_skill_bundle_setup() {
        assert!(QUICKSTART_TEXT.contains("SKILL & BUNDLE SETUP"));
        assert!(QUICKSTART_TEXT.contains("wg skill install"));
    }

    #[test]
    fn test_json_output_has_skill_bundle_setup() {
        let output = json_output();
        let sbs = output
            .get("skill_bundle_setup")
            .expect("missing skill_bundle_setup");
        assert!(sbs.get("claude").is_some());
        assert!(sbs.get("custom").is_some());
        assert!(
            sbs["claude"]["install"]
                .as_str()
                .unwrap()
                .contains("wg skill install")
        );
    }

    #[test]
    fn test_quickstart_converged_prominent() {
        // The CYCLES section must contain IMPORTANT and --converged prominently
        assert!(
            QUICKSTART_TEXT.contains("IMPORTANT — Signaling convergence:"),
            "Cycles section should have IMPORTANT heading for convergence"
        );
        assert!(
            QUICKSTART_TEXT.contains("wg done <task-id> --converged"),
            "Cycles section should show --converged command"
        );
        // The task state commands should also mention --converged
        assert!(
            QUICKSTART_TEXT.contains("wg done <task-id> --converged  # Complete and STOP the loop"),
            "Task state commands should include --converged variant"
        );
    }

    #[test]
    fn test_quickstart_text_contains_visualization() {
        assert!(QUICKSTART_TEXT.contains("VISUALIZATION"));
        assert!(QUICKSTART_TEXT.contains("wg viz"));
        assert!(QUICKSTART_TEXT.contains("--show-internal"));
        assert!(QUICKSTART_TEXT.contains("--no-tui"));
    }

    #[test]
    fn test_quickstart_text_contains_configuration() {
        assert!(QUICKSTART_TEXT.contains("CONFIGURATION"));
        assert!(QUICKSTART_TEXT.contains("--list"));
        assert!(QUICKSTART_TEXT.contains("--global"));
        assert!(QUICKSTART_TEXT.contains("--creator-agent"));
        assert!(QUICKSTART_TEXT.contains("--creator-model"));
    }

    #[test]
    fn test_quickstart_text_contains_trace_runs_replay() {
        assert!(QUICKSTART_TEXT.contains("TRACE, RUNS & REPLAY"));
        assert!(QUICKSTART_TEXT.contains("wg trace show"));
        assert!(QUICKSTART_TEXT.contains("wg runs list"));
        assert!(QUICKSTART_TEXT.contains("wg replay"));
        assert!(QUICKSTART_TEXT.contains("--below-score"));
        assert!(QUICKSTART_TEXT.contains("--subgraph"));
    }

    #[test]
    fn test_quickstart_text_contains_analysis() {
        assert!(QUICKSTART_TEXT.contains("ANALYSIS"));
        assert!(QUICKSTART_TEXT.contains("wg analyze"));
        assert!(QUICKSTART_TEXT.contains("wg bottlenecks"));
        assert!(QUICKSTART_TEXT.contains("wg critical-path"));
        assert!(QUICKSTART_TEXT.contains("wg forecast"));
    }

    #[test]
    fn test_quickstart_text_contains_dead_agents() {
        assert!(QUICKSTART_TEXT.contains("DEAD AGENT DETECTION"));
        assert!(QUICKSTART_TEXT.contains("wg dead-agents"));
        assert!(QUICKSTART_TEXT.contains("--purge"));
        assert!(QUICKSTART_TEXT.contains("--delete-dirs"));
    }

    #[test]
    fn test_quickstart_text_contains_peer_wg_projects() {
        assert!(QUICKSTART_TEXT.contains("PEER WG PROJECTS"));
        assert!(QUICKSTART_TEXT.contains("wg peer add"));
        assert!(QUICKSTART_TEXT.contains("--repo"));
    }

    #[test]
    fn test_quickstart_text_context_scopes_explained() {
        assert!(QUICKSTART_TEXT.contains("--context-scope clean"));
        assert!(QUICKSTART_TEXT.contains("--context-scope task"));
        assert!(QUICKSTART_TEXT.contains("--context-scope graph"));
        assert!(QUICKSTART_TEXT.contains("--context-scope full"));
    }

    #[test]
    fn test_json_output_has_new_sections() {
        let output = json_output();
        assert!(output.get("visualization").is_some());
        assert!(output.get("configuration").is_some());
        assert!(output.get("context_scopes").is_some());
        assert!(output.get("trace_runs_replay").is_some());
        assert!(output.get("analysis").is_some());
        assert!(output.get("dead_agents").is_some());
        assert!(output.get("peer_graphs").is_some());
    }

    #[test]
    fn test_quickstart_json_convergence_emphasis() {
        let output = json_output();
        let convergence = output["cycles"]["convergence"].as_str().unwrap();
        assert!(
            convergence.contains("IMPORTANT"),
            "JSON convergence note should be emphatic"
        );
        let done_converged = output["commands"]["completion"]["done_converged"]
            .as_str()
            .unwrap();
        assert!(
            done_converged.contains("--converged"),
            "JSON should have done_converged command"
        );
    }

    #[test]
    fn test_quickstart_text_all_sections_present() {
        let text = QUICKSTART_TEXT.trim();
        let required_sections = [
            "WG AGENT QUICKSTART",
            "GETTING STARTED",
            "SKILL & BUNDLE SETUP",
            "AGENCY SETUP",
            "DISPATCHER SERVICE REMINDER",
            "SERVICE MODE",
            "MANUAL MODE",
            "DISCOVERING & ADDING WORK",
            "TASK STATE COMMANDS",
            "VALIDATION (## Validation section in task description)",
            "MESSAGING",
            "CONTEXT & ARTIFACTS",
            "CYCLES",
            "DISCOVERY & PUBLISHING",
            "HOUSEKEEPING",
            "GROWING THE GRAPH",
            "TIPS",
            "PI MODEL PLANE",
            "REUSABLE FUNCTIONS",
            "VISUALIZATION",
            "CONFIGURATION",
            "TRACE, RUNS & REPLAY",
            "ANALYSIS",
            "DEAD AGENT DETECTION",
            "PEER WG PROJECTS",
            "WG-FED IDENTITY & CROSS-GRAPH FEDERATION",
            "CONTENT-SAFETY REVIEW GATE",
            "AGENCY FEEDBACK & MIGRATION",
            "MONITORING",
            "NOTIFICATION & COMMUNICATION",
            "RESOURCE MANAGEMENT",
            "PROVIDER PROFILES",
            "USER BOARDS",
            "COST & SPENDING",
        ];
        for section in &required_sections {
            assert!(text.contains(section), "Missing section: {}", section);
        }
    }

    #[test]
    fn test_quickstart_text_contains_wait_command() {
        assert!(QUICKSTART_TEXT.contains("wg wait"));
        assert!(QUICKSTART_TEXT.contains("--until"));
        assert!(QUICKSTART_TEXT.contains("task:dep-a=done"));
        assert!(QUICKSTART_TEXT.contains("timer:5m"));
    }

    #[test]
    fn test_quickstart_text_contains_messaging() {
        assert!(QUICKSTART_TEXT.contains("MESSAGING"));
        assert!(QUICKSTART_TEXT.contains("wg msg send"));
        assert!(QUICKSTART_TEXT.contains("wg msg read"));
        assert!(QUICKSTART_TEXT.contains("wg msg poll"));
    }

    #[test]
    fn test_quickstart_text_contains_validation() {
        assert!(QUICKSTART_TEXT.contains("VALIDATION"));
        // Quickstart must describe the agency-evaluator path (## Validation
        // section) and must NOT advertise the removed --validation flag.
        assert!(QUICKSTART_TEXT.contains("## Validation"));
        assert!(!QUICKSTART_TEXT.contains("--validation"));
    }

    #[test]
    fn test_quickstart_text_contains_discover_publish() {
        assert!(QUICKSTART_TEXT.contains("DISCOVERY & PUBLISHING"));
        assert!(QUICKSTART_TEXT.contains("wg discover"));
        assert!(QUICKSTART_TEXT.contains("wg publish"));
        assert!(QUICKSTART_TEXT.contains("wg reclaim"));
    }

    #[test]
    fn test_json_output_has_new_command_sections() {
        let output = json_output();
        assert!(output.get("validation").is_some());
        assert!(output.get("messaging").is_some());
        assert!(output.get("wait_conditions").is_some());
        assert!(output.get("discovery_publishing").is_some());
    }

    #[test]
    fn test_quickstart_text_contains_notification() {
        assert!(QUICKSTART_TEXT.contains("NOTIFICATION & COMMUNICATION"));
        assert!(QUICKSTART_TEXT.contains("wg telegram send"));
        assert!(QUICKSTART_TEXT.contains("wg telegram ask"));
    }

    #[test]
    fn test_quickstart_text_contains_resources() {
        assert!(QUICKSTART_TEXT.contains("RESOURCE MANAGEMENT"));
        assert!(QUICKSTART_TEXT.contains("wg resource add"));
        assert!(QUICKSTART_TEXT.contains("wg resources"));
    }

    #[test]
    fn test_quickstart_text_contains_profiles() {
        assert!(QUICKSTART_TEXT.contains("PROVIDER PROFILES"));
        assert!(QUICKSTART_TEXT.contains("wg profile list"));
    }

    #[test]
    fn test_quickstart_text_contains_user_boards() {
        assert!(QUICKSTART_TEXT.contains("USER BOARDS"));
        assert!(QUICKSTART_TEXT.contains("wg user init"));
    }

    #[test]
    fn test_quickstart_text_contains_cost_spending() {
        assert!(QUICKSTART_TEXT.contains("COST & SPENDING"));
        assert!(QUICKSTART_TEXT.contains("wg spend"));
    }

    #[test]
    fn test_quickstart_text_contains_unclaim_requeue() {
        assert!(QUICKSTART_TEXT.contains("wg unclaim"));
        assert!(QUICKSTART_TEXT.contains("wg requeue"));
    }

    #[test]
    fn test_quickstart_text_contains_dep_management() {
        assert!(QUICKSTART_TEXT.contains("wg add-dep"));
        assert!(QUICKSTART_TEXT.contains("wg rm-dep"));
    }

    #[test]
    fn test_quickstart_text_contains_cleanup() {
        assert!(QUICKSTART_TEXT.contains("wg cleanup orphaned"));
        assert!(QUICKSTART_TEXT.contains("wg cleanup nightly"));
        assert!(QUICKSTART_TEXT.contains("wg metrics"));
    }

    #[test]
    fn test_quickstart_text_contains_recovery_section() {
        assert!(QUICKSTART_TEXT.contains("RECOVERY"));
        assert!(QUICKSTART_TEXT.contains("wg agents kill"));
        assert!(QUICKSTART_TEXT.contains("Hung worker agent"));
        assert!(QUICKSTART_TEXT.contains("--reason"));
    }

    #[test]
    fn test_quickstart_text_contains_extended_analysis() {
        assert!(QUICKSTART_TEXT.contains("wg velocity"));
        assert!(QUICKSTART_TEXT.contains("wg aging"));
        assert!(QUICKSTART_TEXT.contains("wg workload"));
        assert!(QUICKSTART_TEXT.contains("wg coordinate"));
        assert!(QUICKSTART_TEXT.contains("wg impact"));
        assert!(QUICKSTART_TEXT.contains("wg plan"));
        assert!(QUICKSTART_TEXT.contains("wg cost"));
    }

    #[test]
    fn test_quickstart_text_contains_advanced_service() {
        assert!(QUICKSTART_TEXT.contains("wg screencast"));
        assert!(QUICKSTART_TEXT.contains("wg tui-dump"));
        assert!(QUICKSTART_TEXT.contains("wg server"));
    }

    #[test]
    fn test_quickstart_text_provider_model_format() {
        assert!(QUICKSTART_TEXT.contains("pi:<provider>:<model>"));
        assert!(QUICKSTART_TEXT.contains("--reasoning high"));
        assert!(!QUICKSTART_TEXT.contains("--provider"));
    }

    #[test]
    fn test_quickstart_text_contains_cron_timeout() {
        assert!(QUICKSTART_TEXT.contains("--cron"));
        assert!(QUICKSTART_TEXT.contains("--timeout"));
        assert!(QUICKSTART_TEXT.contains("--independent"));
    }

    #[test]
    fn test_json_output_has_new_sections_apr12() {
        let output = json_output();
        assert!(output.get("notification").is_some());
        assert!(output.get("resources").is_some());
        assert!(output.get("profiles").is_some());
        assert!(output.get("user_boards").is_some());
        assert!(output.get("cost_spending").is_some());
        assert!(output.get("advanced_service").is_some());

        // Check analysis has new fields
        let analysis = output.get("analysis").unwrap();
        assert!(analysis.get("velocity").is_some());
        assert!(analysis.get("aging").is_some());
        assert!(analysis.get("workload").is_some());
        assert!(analysis.get("coordinate").is_some());
        assert!(analysis.get("impact").is_some());
        assert!(analysis.get("plan").is_some());
        assert!(analysis.get("cost").is_some());

        // Check housekeeping has cleanup
        let hk = output.get("housekeeping").unwrap();
        assert!(hk.get("cleanup_orphaned").is_some());
        assert!(hk.get("metrics").is_some());

        // Check the Pi model-plane owner and retained policy fields.
        let plane = output.get("pi_model_plane").unwrap();
        assert_eq!(plane["owner"], "Pi");
        assert!(plane.get("wg_retains").is_some());

        // Check commands has dependencies section
        let commands = output.get("commands").unwrap();
        assert!(commands.get("dependencies").is_some());
    }
}
