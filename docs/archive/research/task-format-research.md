# Task and Workflow Management Format Research

This document analyzes existing lightweight workflow and task management file formats to inform the design of workgraph's format.

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Format Analysis](#format-analysis)
   - [Todo.txt](#todotxt)
   - [Taskwarrior](#taskwarrior)
   - [GitHub Issues/Projects](#github-issuesprojects)
   - [BPMN](#bpmn)
   - [Plain Markdown Task Lists](#plain-markdown-task-lists)
   - [JSONL/NDJSON](#jsonlndjson)
3. [Comparison Table](#comparison-table)
4. [Recommendations for workgraph](#recommendations-for-workgraph)
5. [Sources](#sources)

---

## Executive Summary

After analyzing six major task/workflow management formats, clear patterns emerge:

- **Human-readable formats** (Todo.txt, Markdown) excel at simplicity but struggle with complex relationships like dependencies
- **Machine-first formats** (BPMN XML, GitHub GraphQL) support rich semantics but sacrifice human editability
- **Hybrid approaches** (Taskwarrior JSON, JSONL) balance both concerns with one-record-per-line designs

For workgraph, a **JSONL-based format with carefully designed schema** offers the best tradeoffs: append-only semantics for event sourcing, git-friendly line-based diffs, machine parseability, and reasonable human readability when needed.

---

## Format Analysis

### Todo.txt

**Overview**: Created by Gina Trapani in 2006, todo.txt is a plain-text format designed to be human-readable and editable in any text editor. It follows a simple one-task-per-line structure.

**Syntax**:
```
(A) 2024-01-15 Call mom @phone +family due:2024-01-20
x 2024-01-14 2024-01-10 Buy groceries @errands +household
```

Format: `[completion] [priority] [completion-date] [creation-date] description [+project] [@context] [key:value]`

**Primitives Supported**:
| Primitive | Support | Notes |
|-----------|---------|-------|
| Dependencies | No native support | Proposed via `dep:taskid` key:value extension |
| Actors | Partial | @contexts can represent people/teams |
| Resources | No | Would need custom key:value pairs |
| Weights/Priority | Yes | (A)-(Z) priority levels |
| Due dates | Yes | `due:YYYY-MM-DD` extension |
| Projects | Yes | +ProjectName tags |
| Subtasks | No | Workaround: shared +project tags |

**Git-Friendliness**: Excellent
- One task per line enables clean line-based diffs
- Alphabetical sorting produces predictable order
- Text-based, no binary data
- Merge conflicts are straightforward to resolve

**Human Readability**: Excellent
- Designed for plain text editors
- Minimal syntax to learn
- Self-documenting format

**Machine Parseability**: Good
- Simple regex-based parsing
- Unambiguous syntax
- Many parsers available (Python, Ruby, JavaScript, etc.)

**Extensibility**: Limited
- key:value pairs allow custom metadata
- No schema validation
- No nested structures
- Adding complex features "makes files look uglier and less readable"

**Limitations**:
- No task dependencies without extensions
- No subtask hierarchy
- No multi-line task descriptions
- Limited metadata types (all values are strings)
- No unique task identifiers

**Tooling Ecosystem**: Extensive
- CLI: todo.txt-cli, TTDL
- Desktop: sleek, Todotxt.net, TodoTxtMac
- Mobile: Official Android/iOS apps, Markor
- Editor extensions: VS Code, Atom, gedit

---

### Taskwarrior

**Overview**: Taskwarrior is a command-line task management tool that stores tasks as JSON objects, one per line. It models dependencies as a directed acyclic graph (DAG).

**Data Format**:
```json
{"description":"Buy groceries","uuid":"a1b2c3d4-...","status":"pending","entry":"20240115T120000Z","depends":"e5f6g7h8-...,i9j0k1l2-...","tags":["shopping"],"project":"household","priority":"H"}
```

**Primitives Supported**:
| Primitive | Support | Notes |
|-----------|---------|-------|
| Dependencies | Yes | `depends` field with comma-separated UUIDs |
| Actors | Partial | Can be modeled via UDAs or tags |
| Resources | Via UDAs | User Defined Attributes allow custom fields |
| Weights/Priority | Yes | H/M/L priority + urgency coefficients |
| Due dates | Yes | `due` field with ISO timestamps |
| Projects | Yes | Hierarchical via dot notation (project.subproject) |
| Subtasks | Indirect | Via dependencies and project hierarchy |
| Blocking/Blocked | Yes | Automatic calculation from depends graph |

**UDA (User Defined Attributes)**:
Taskwarrior's extensibility mechanism allows custom fields with types:
- `string` (with optional allowed values list)
- `numeric`
- `date`
- `duration`

Example configuration:
```
uda.estimate.type=numeric
uda.estimate.label=Est
uda.size.type=string
uda.size.values=large,medium,small
```

**Git-Friendliness**: Good
- One JSON object per line
- UUIDs ensure stable identity across edits
- Line-based diffs work reasonably well
- Reordering tasks changes many lines
- JSON formatting must be consistent (no pretty-printing)

**Human Readability**: Moderate
- JSON is verbose but parseable by humans
- UUIDs are unwieldy for manual editing
- Requires tooling for comfortable interaction

**Machine Parseability**: Excellent
- Well-defined JSON format
- RFC specifies all field semantics
- UDAs must be preserved even if unknown

**Extensibility**: Excellent
- UDAs allow arbitrary custom fields
- Urgency coefficients customizable
- Hooks system for workflow automation
- Color rules for UDA values

**Dependency Model**:
- Directed acyclic graph (DAG)
- `depends` field contains blocking task UUIDs
- System calculates `is_blocked` and `is_blocking` status
- Circular dependencies prevented
- Dependency chains tracked on completion/deletion

---

### GitHub Issues/Projects

**Overview**: GitHub's issue tracking uses a GraphQL API with a rich data model. Projects V2 adds custom fields and views.

**Data Model** (simplified):
```graphql
type Issue {
  id: ID!
  title: String!
  body: String
  state: IssueState!  # OPEN, CLOSED
  assignees: [User!]
  labels: [Label!]
  milestone: Milestone
  projectItems: [ProjectV2Item!]
}

type ProjectV2Item {
  id: ID!
  content: Issue | PullRequest | DraftIssue
  fieldValues: [ProjectV2ItemFieldValue!]
}

type ProjectV2SingleSelectField {
  id: ID!
  name: String!
  options: [ProjectV2SingleSelectFieldOption!]
}
```

**Primitives Supported**:
| Primitive | Support | Notes |
|-----------|---------|-------|
| Dependencies | Limited | Via "blocked by #123" in body text, or task lists |
| Actors | Yes | Assignees (multiple users) |
| Resources | Indirect | Labels, custom fields |
| Weights/Priority | Via fields | Custom single-select or number fields |
| Due dates | Yes | Custom date fields |
| Projects | Yes | Issues can belong to multiple projects |
| Subtasks | Yes | Task lists in issue body, linked issues |

**Custom Field Types**:
- Text
- Number
- Date
- Single Select (with colored options)
- Iteration (for sprint planning)

**Git-Friendliness**: Poor (for local use)
- Data lives in GitHub's database, not files
- No direct file format for version control
- API-driven, not file-driven
- Export formats (JSON) are complex and verbose

**Human Readability**: Good (in UI)
- Web interface designed for humans
- Markdown body content
- API responses are complex nested JSON

**Machine Parseability**: Excellent
- Well-documented GraphQL schema
- Strongly typed
- Rich query capabilities
- Schema versioned with deprecation notices

**Extensibility**: Good
- Custom fields per project
- Labels for categorization
- Milestones for grouping
- Automation via Actions and API

---

### BPMN

**Overview**: Business Process Model and Notation 2.0 is an OMG standard for workflow modeling with XML serialization. It captures complex business processes with execution semantics.

**XML Structure** (simplified):
```xml
<?xml version="1.0" encoding="UTF-8"?>
<definitions xmlns="http://www.omg.org/spec/BPMN/20100524/MODEL">
  <process id="orderProcess" isExecutable="true">
    <startEvent id="start"/>
    <sequenceFlow id="flow1" sourceRef="start" targetRef="task1"/>
    <userTask id="task1" name="Review Order">
      <performer>sales_team</performer>
    </userTask>
    <parallelGateway id="gateway1"/>
    <sequenceFlow id="flow2" sourceRef="task1" targetRef="gateway1"/>
    <serviceTask id="task2" name="Check Inventory"/>
    <serviceTask id="task3" name="Validate Payment"/>
    <sequenceFlow id="flow3" sourceRef="gateway1" targetRef="task2"/>
    <sequenceFlow id="flow4" sourceRef="gateway1" targetRef="task3"/>
    <endEvent id="end"/>
  </process>
</definitions>
```

**Core Concepts**:
- **Flow Objects**: Events (start/end/intermediate), Activities (tasks), Gateways (decisions)
- **Connecting Objects**: Sequence flows, message flows, associations
- **Swimlanes**: Pools (organizations), Lanes (roles/actors)
- **Artifacts**: Data objects, groups, annotations

**Primitives Supported**:
| Primitive | Support | Notes |
|-----------|---------|-------|
| Dependencies | Yes | Sequence flows define execution order |
| Actors | Yes | Lanes, performers, user tasks |
| Resources | Yes | Data objects, data stores |
| Weights/Priority | Limited | Not core, requires extensions |
| Due dates | Limited | Timer events, not task deadlines |
| Projects | No | Single process per file |
| Parallel execution | Yes | Parallel/inclusive gateways |
| Conditional branching | Yes | Exclusive/event-based gateways |

**Gateway Types**:
- **Exclusive (XOR)**: One path based on condition
- **Parallel (AND)**: All paths simultaneously
- **Inclusive (OR)**: One or more paths
- **Event-based**: Wait for events

**Git-Friendliness**: Poor
- Verbose XML with deep nesting
- Graphical coordinates stored inline
- ID references scattered throughout
- Merge conflicts common and hard to resolve
- Tools often reformat entire file on save

**Human Readability**: Poor
- XML verbosity
- Designed for graphical tools
- Element IDs don't convey meaning
- Requires schema knowledge

**Machine Parseability**: Excellent
- XSD schema validation
- Well-defined semantics
- Execution engines available (Camunda, Flowable)
- Standardized by OMG

**Extensibility**: Good
- Extension elements supported
- Custom attributes via BPMN extensions
- Execution-specific additions (Camunda, etc.)

---

### Plain Markdown Task Lists

**Overview**: Markdown checkboxes (`- [ ]`, `- [x]`) have become a de facto standard for task lists in tools like Obsidian, Logseq, GitHub, and many others.

**Basic Syntax**:
```markdown
## Project Alpha

- [ ] Design system architecture
  - [ ] Review requirements
  - [x] Create initial diagrams
- [x] Set up development environment
- [ ] Implement core features #priority/high due::2024-01-20

### Notes
Some context about the project...
```

**Tool Variations**:

| Tool | Task Syntax | Metadata | Query Language |
|------|-------------|----------|----------------|
| GitHub | `- [ ]` | None | None |
| Obsidian | `- [ ]` | Inline `[key:: value]` or YAML | Dataview DQL |
| Logseq | `TODO/DOING/DONE` | Properties `key:: value` | Advanced queries |
| Notion | `- [ ]` | Database properties | Filters |

**Obsidian/Dataview Approach**:
```markdown
---
status: active
priority: high
---

- [ ] Complete report [due:: 2024-01-20] [assignee:: @john]
- [x] Review draft [completed:: 2024-01-15]
```

Dataview queries:
```dataview
TASK
WHERE !completed AND contains(tags, "#work")
SORT due ASC
```

**Logseq Approach**:
```markdown
- TODO Buy groceries
  priority:: high
  scheduled:: [[2024-01-20]]
- DOING Write documentation
- DONE Review PR
```

**Primitives Supported**:
| Primitive | Support | Notes |
|-----------|---------|-------|
| Dependencies | Plugin-dependent | Some tools support via properties |
| Actors | Via metadata | assignee:: @person |
| Resources | Via tags/links | [[Resource Name]] or #tag |
| Weights/Priority | Via metadata | priority:: high or #priority/high |
| Due dates | Via metadata | due:: 2024-01-20 |
| Projects | Via folders/tags | File hierarchy or +project tags |
| Subtasks | Yes | Indented nested lists |

**Git-Friendliness**: Good
- Text-based, line-oriented
- Hierarchy via indentation
- Metadata inline or in frontmatter
- YAML frontmatter can cause multi-line diffs

**Human Readability**: Excellent
- Designed for human consumption
- Renders nicely in many tools
- Natural prose mixed with structure

**Machine Parseability**: Moderate
- Markdown parsing well-understood
- Metadata syntax varies by tool
- No standard for task-specific features
- YAML frontmatter parsing is standard

**Extensibility**: Tool-dependent
- YAML frontmatter allows arbitrary fields
- Inline fields via Dataview syntax
- Plugin ecosystem (Obsidian has 1000+ plugins)
- No cross-tool standardization

---

### JSONL/NDJSON

**Overview**: JSON Lines (JSONL) or Newline Delimited JSON (NDJSON) stores one JSON object per line. While not a task format itself, it's an excellent foundation for event-sourced task systems.

**Specification** (NDJSON 1.0):
- Each line is a valid JSON object (RFC 8259)
- Lines separated by `\n` (0x0A)
- UTF-8 encoding
- Media type: `application/x-ndjson`
- File extension: `.ndjson` or `.jsonl`

**Example Task Log**:
```jsonl
{"type":"task.created","id":"task-001","timestamp":"2024-01-15T10:00:00Z","data":{"title":"Design API","project":"backend"}}
{"type":"task.assigned","id":"task-001","timestamp":"2024-01-15T10:05:00Z","data":{"assignee":"alice"}}
{"type":"task.dependency.added","id":"task-001","timestamp":"2024-01-15T10:10:00Z","data":{"depends_on":"task-002"}}
{"type":"task.started","id":"task-001","timestamp":"2024-01-16T09:00:00Z","data":{}}
{"type":"task.completed","id":"task-001","timestamp":"2024-01-17T15:30:00Z","data":{}}
```

**Primitives Supported** (schema-dependent):
| Primitive | Support | Notes |
|-----------|---------|-------|
| Dependencies | Schema-defined | Any relationship model possible |
| Actors | Schema-defined | Assignees, watchers, etc. |
| Resources | Schema-defined | Arbitrary resource linking |
| Weights/Priority | Schema-defined | Numbers, enums, whatever needed |
| Due dates | Schema-defined | ISO timestamps |
| Event history | Native | Append-only captures all changes |
| Audit trail | Native | Timestamp on every event |

**Event Sourcing Benefits**:
- Complete history of all changes
- Reconstruct state at any point in time
- Natural audit trail
- Supports undo/replay
- Conflict resolution via event ordering

**Git-Friendliness**: Excellent
- One event per line
- Append-only means no rewriting history
- Line-based diffs show exactly what changed
- Merge = concatenate and sort by timestamp
- No formatting inconsistencies

**Human Readability**: Moderate
- JSON is readable but verbose
- One-line-per-record helps scanning
- Event types provide context
- Current state requires reconstruction

**Machine Parseability**: Excellent
- Standard JSON parsing
- Stream processing friendly
- No need to load entire file
- Partial reads possible

**Extensibility**: Excellent
- Schema is application-defined
- New event types added freely
- Old events remain valid
- Forward/backward compatibility easy

---

## Comparison Table

| Feature | Todo.txt | Taskwarrior | GitHub | BPMN | Markdown | JSONL |
|---------|----------|-------------|--------|------|----------|-------|
| **Dependencies** | None | DAG | Limited | Flows | Plugin | Schema |
| **Actors** | @context | UDA/tags | Assignees | Lanes | Metadata | Schema |
| **Resources** | None | UDA | Labels | Data objects | Tags/links | Schema |
| **Priority/Weights** | A-Z | H/M/L + urgency | Custom fields | Extensions | Metadata | Schema |
| **Subtasks** | None | Indirect | Task lists | Subprocesses | Indentation | Schema |
| **Event history** | None | Limited | Full | None | Plugin | Native |
| | | | | | | |
| **Git-friendliness** | Excellent | Good | Poor | Poor | Good | Excellent |
| **Human readability** | Excellent | Moderate | Good (UI) | Poor | Excellent | Moderate |
| **Machine parseability** | Good | Excellent | Excellent | Excellent | Moderate | Excellent |
| **Extensibility** | Limited | Excellent | Good | Good | Tool-dependent | Excellent |
| | | | | | | |
| **File format** | Plain text | JSON lines | API/JSON | XML | Markdown | JSON lines |
| **Tooling** | Extensive | CLI-focused | Web/API | Enterprise | Growing | Libraries |
| **Learning curve** | Low | Medium | Low (UI) | High | Low | Medium |

---

## Recommendations for workgraph

Based on this analysis, here are recommendations for workgraph's format design:

### 1. Primary Format: JSONL Event Log

**Rationale**: JSONL combines machine parseability with git-friendliness. The event-sourcing pattern provides:
- Complete audit trail
- Natural conflict resolution (merge by timestamp)
- Append-only simplicity
- Streaming/incremental processing

**Proposed structure**:
```jsonl
{"v":1,"type":"node.created","id":"n1","ts":"2024-01-15T10:00:00Z","data":{"type":"task","title":"Design API"}}
{"v":1,"type":"node.updated","id":"n1","ts":"2024-01-15T10:05:00Z","data":{"assignee":"alice"}}
{"v":1,"type":"edge.created","id":"e1","ts":"2024-01-15T10:10:00Z","data":{"from":"n1","to":"n2","type":"depends_on"}}
```

### 2. Graph-Based Data Model

**Rationale**: Taskwarrior's DAG approach works well for dependencies. Extending to a full graph allows:
- Tasks as nodes
- Dependencies as edges
- Resources as nodes with relationships
- Actors as nodes with assignment edges
- Weighted edges for priority/effort

**Core primitives**:
```
Nodes: task, milestone, resource, actor, note
Edges: depends_on, blocks, assigned_to, uses_resource, subtask_of
Properties: Any key-value on nodes/edges
```

### 3. Human-Readable View Format

**Rationale**: For quick edits, a Todo.txt-inspired format could be generated:
```
[n1] (A) Design API @alice +backend depends:n2 due:2024-01-20
[n2] (B) Set up database @bob +backend
```

This would be a projection of the graph, not the source of truth.

### 4. Schema Validation

**Rationale**: Unlike Todo.txt's implicit schema, define explicit types:
```json
{
  "node_types": {
    "task": {
      "required": ["title"],
      "optional": ["description", "assignee", "due", "priority"],
      "priority_values": ["critical", "high", "medium", "low"]
    }
  },
  "edge_types": {
    "depends_on": {
      "valid_from": ["task"],
      "valid_to": ["task", "milestone"]
    }
  }
}
```

### 5. ID Strategy

**Recommendation**: Short human-readable IDs with optional auto-generation
- Auto-generated: `t1`, `t2`, `m1` (type prefix + sequence)
- User-specified: `api-design`, `db-setup`
- UUIDs only for sync/conflict resolution

### 6. Key Design Principles

1. **Append-only primary storage**: Events never modified, only appended
2. **Materialized views**: Compute current state from events
3. **Line-based diffs**: One logical change per line
4. **Schema versioning**: `"v":1` in every event for evolution
5. **Timestamps everywhere**: Enable merge by temporal ordering
6. **ID stability**: Nodes/edges keep IDs forever
7. **Optional compression**: For long histories, periodic snapshots

### 7. What to Avoid

- **XML**: Too verbose, merge-conflict-prone
- **Pretty-printed JSON**: Wastes diff space
- **Binary formats**: Not git-friendly
- **Single-file state**: Concurrent edit conflicts
- **Implicit relationships**: Dependencies should be explicit edges

---

## Sources

### Todo.txt
- [Todo.txt Format Specification](https://github.com/todotxt/todo.txt)
- [Todo.txt Official Site](http://todotxt.org/)
- [SwiftoDo Syntax Overview](https://swiftodoapp.com/todotxt-syntax/syntax-overview/)
- [Todo.txt Dependencies Discussion](https://github.com/todotxt/todo.txt/issues/96)
- [sleek - Todo.txt Manager](https://github.com/ransome1/sleek)

### Taskwarrior
- [Taskwarrior Documentation](https://taskwarrior.org/docs/)
- [Task Representation RFC](https://github.com/GothenburgBitFactory/taskwarrior/blob/develop/doc/devel/rfcs/task.md)
- [Taskwarrior UDAs](https://taskwarrior.org/docs/udas/)
- [Dependency Management](https://deepwiki.com/GothenburgBitFactory/taskwarrior/3.5-dependency-management)
- [3rd-Party Guidelines](https://taskwarrior.org/docs/3rd-party/)

### GitHub Issues/Projects
- [GitHub GraphQL API Docs](https://docs.github.com/en/graphql)
- [Using the API to Manage Projects](https://docs.github.com/en/issues/planning-and-tracking-with-projects/automating-your-project/using-the-api-to-manage-projects)
- [Understanding Fields](https://docs.github.com/en/issues/planning-and-tracking-with-projects/understanding-fields)
- [GraphQL Schema Repository](https://github.com/octokit/graphql-schema)

### BPMN
- [BPMN 2.0 Specification (OMG)](http://www.omg.org/spec/BPMN/2.0/)
- [BPMN.org](https://www.bpmn.org/)
- [Flowable BPMN Introduction](https://www.flowable.com/open-source/docs/bpmn/ch07a-BPMN-Introduction)
- [BPMN Symbols Explained (Lucidchart)](https://www.lucidchart.com/pages/tutorial/bpmn-symbols-explained)
- [Camunda BPMN Primer](https://docs.camunda.io/docs/components/modeler/bpmn/bpmn-primer/)

### Markdown Task Lists
- [Obsidian Dataview](https://blacksmithgu.github.io/obsidian-dataview/)
- [Logseq Markdown Cheat Sheet](https://facedragons.com/foss/logseq-markdown-cheat-sheet/)
- [Obsidian vs Logseq Comparison](https://www.glukhov.org/post/2025/11/obsidian-vs-logseq-comparison/)
- [Logseq and Obsidian Interoperability](https://hub.logseq.com/integrations/aV9AgETypcPcf8avYcHXQT/how-to-use-obsidian-and-logseq-together-and-why-markdown-matters/1rqp92wgow7wGXS37Ckz1U)

### JSONL/NDJSON
- [NDJSON Specification](https://github.com/ndjson/ndjson-spec)
- [JSON Lines Definition](https://ndjson.com/definition/)
- [Event Sourcing Pattern (Microsoft)](https://learn.microsoft.com/en-us/azure/architecture/patterns/event-sourcing)
- [JSONL for Log Processing](https://jsonl.help/use-cases/log-processing/)
- [JSON Streaming (Wikipedia)](https://en.wikipedia.org/wiki/JSON_streaming)

### Git and Merge Conflicts
- [Git Advanced Merging](https://git-scm.com/book/en/v2/Git-Tools-Advanced-Merging)
- [Atlassian Git Merge Conflicts](https://www.atlassian.com/git/tutorials/using-branches/merge-conflicts)
