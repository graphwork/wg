# Rust Ecosystem Research for workgraph System

This document provides a comprehensive evaluation of the Rust ecosystem for building a workgraph system, covering graph libraries, JSON/JSONL handling, CLI frameworks, SQLite bindings, and file watching capabilities.

---

## Table of Contents

1. [Graph Libraries](#1-graph-libraries)
2. [JSONL/JSON Handling](#2-jsonljson-handling)
3. [CLI Frameworks](#3-cli-frameworks)
4. [SQLite Bindings](#4-sqlite-bindings)
5. [File Watching](#5-file-watching)
6. [Recommendations Summary](#6-recommendations-summary)

---

## 1. Graph Libraries

### 1.1 petgraph (Recommended)

**Crate**: [petgraph](https://crates.io/crates/petgraph) | [Documentation](https://docs.rs/petgraph/latest/petgraph/)

Petgraph is the de facto standard graph library in Rust, providing fast, flexible graph data structures and algorithms.

#### Key Features

- **Multiple graph types**: `Graph`, `StableGraph`, `GraphMap`, `MatrixGraph`
- **Arbitrary node/edge weights**: Generic over node weight `N` and edge weight `E`
- **Directed and undirected**: `DiGraph<N, E>` for directed, `UnGraph<N, E>` for undirected
- **Rich algorithm library**: Dijkstra, A*, Bellman-Ford, topological sort, DFS/BFS, etc.
- **Serde support**: Optional `serde-1` feature for serialization
- **DOT format**: Export/import for Graphviz visualization

#### Space/Time Complexity

- Space: O(|V| + |E|)
- Edge lookup: O(e') where e' is local edge count
- Fast node/edge insertion

#### Code Examples

**Creating a weighted directed graph:**

```rust
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::algo::dijkstra;

// Create a directed graph with string node labels and u32 edge weights
let mut graph: DiGraph<&str, u32> = DiGraph::new();

// Add nodes
let a = graph.add_node("A");
let b = graph.add_node("B");
let c = graph.add_node("C");
let d = graph.add_node("D");

// Add weighted edges
graph.add_edge(a, b, 5);
graph.add_edge(a, c, 3);
graph.add_edge(b, c, 2);
graph.add_edge(b, d, 6);
graph.add_edge(c, d, 7);

// Find shortest paths from node A
let node_map = dijkstra(&graph, a, None, |e| *e.weight());
println!("Distance to D: {:?}", node_map.get(&d)); // Some(11)
```

**Topological sort for task ordering:**

```rust
use petgraph::graph::DiGraph;
use petgraph::algo::toposort;

let mut deps: DiGraph<&str, ()> = DiGraph::new();
let compile = deps.add_node("compile");
let test = deps.add_node("test");
let package = deps.add_node("package");

deps.add_edge(compile, test, ());
deps.add_edge(test, package, ());

match toposort(&deps, None) {
    Ok(order) => {
        for node in order {
            println!("Execute: {}", deps[node]);
        }
    }
    Err(cycle) => println!("Cycle detected at {:?}", cycle),
}
```

**Using StableGraph for stable indices during removals:**

```rust
use petgraph::stable_graph::StableGraph;

let mut graph: StableGraph<&str, u32> = StableGraph::new();
let a = graph.add_node("A");
let b = graph.add_node("B");
let c = graph.add_node("C");

graph.add_edge(a, b, 1);
graph.add_edge(b, c, 2);

// Remove node B - indices a and c remain valid
graph.remove_node(b);
assert!(graph.contains_node(a));
assert!(graph.contains_node(c));
```

#### Cargo.toml

```toml
[dependencies]
petgraph = { version = "0.6", features = ["serde-1"] }
```

#### Trade-offs

| Pros | Cons |
|------|------|
| Mature, well-tested | Memory allocation overhead for dynamic updates |
| Comprehensive algorithm library | Index invalidation on removal (use StableGraph) |
| Excellent documentation | Learning curve for advanced features |
| Active maintenance | |

---

### 1.2 daggy

**Crate**: [daggy](https://crates.io/crates/daggy) | [Documentation](https://docs.rs/daggy)

Daggy is a thin wrapper around petgraph specifically for Directed Acyclic Graphs (DAGs), providing cycle-prevention guarantees.

#### Key Features

- **Built on petgraph**: Familiar API, interoperable
- **Cycle detection**: `add_edge` returns `WouldCycle` error if edge would create cycle
- **Walker trait**: Traverse without borrowing the graph (allows mutation during traversal)
- **StableDag**: Optional feature for stable indices

#### Code Examples

**Creating a task dependency DAG:**

```rust
use daggy::{Dag, Walker};

let mut dag: Dag<&str, &str> = Dag::new();

// Add root task
let build = dag.add_node("build");

// Add children with edges
let (_, compile) = dag.add_child(build, "depends", "compile");
let (_, test) = dag.add_child(build, "depends", "test");
let (_, lint) = dag.add_child(build, "depends", "lint");

// Add edge between siblings (compile must finish before test)
dag.add_edge(compile, test, "after").unwrap();

// This would return Err(WouldCycle) - prevents cycles!
// dag.add_edge(test, compile, "invalid");

// Walk children of build
let children = dag.children(build);
for (edge, node) in children.iter(&dag) {
    println!("Child: {}", dag[node]);
}
```

**Traversal with Walker trait:**

```rust
use daggy::{Dag, Walker};

let mut dag: Dag<i32, ()> = Dag::new();
let n0 = dag.add_node(0);
let (_, n1) = dag.add_child(n0, (), 1);
let (_, n2) = dag.add_child(n0, (), 2);
let (_, n3) = dag.add_child(n1, (), 3);

// Walker doesn't borrow the graph
let mut children_walker = dag.children(n0);
while let Some((_, node)) = children_walker.walk_next(&dag) {
    // Can mutate dag here if needed
    println!("Child value: {}", dag[node]);
}
```

#### Cargo.toml

```toml
[dependencies]
daggy = { version = "0.9", features = ["serde-1", "stable_dag"] }
```

#### Trade-offs

| Pros | Cons |
|------|------|
| Guaranteed acyclic | Limited to DAGs only |
| Cycle detection at edge insertion | Thinner algorithm library than petgraph |
| Walker trait for flexible traversal | Index instability (mitigated with stable_dag) |
| Serde support | |

---

### 1.3 Other Graph Libraries

#### graphalgs
Extends petgraph with additional algorithms (Floyd-Warshall, graph metrics like radius/diameter/eccentricity).

```toml
[dependencies]
graphalgs = "0.5"
```

#### typed_graph
Focuses on type-safety and functionality over raw performance.

#### gryf
A newer library with better error handling (returns `Result` instead of panicking).

---

### 1.4 Graph Library Recommendation

**For a workgraph system**: Use **petgraph** as the foundation with **daggy** if you need strict DAG guarantees.

- If tasks can have cycles (rare but possible): Use `petgraph::stable_graph::StableGraph`
- If tasks must be acyclic: Use `daggy::Dag` or `daggy::StableDag`

---

## 2. JSONL/JSON Handling

### 2.1 serde_json (Foundation)

**Crate**: [serde_json](https://crates.io/crates/serde_json) | [Documentation](https://docs.rs/serde_json)

The standard JSON library for Rust, built on serde's serialization framework.

#### Key Features

- **Type-safe serialization/deserialization**
- **Streaming support**: `from_reader` for parsing from any `io::Read`
- **Streaming deserialization**: `StreamDeserializer` for sequences
- **Dynamic JSON**: `serde_json::Value` for untyped JSON

#### Code Example

```rust
use serde::{Deserialize, Serialize};
use serde_json;

#[derive(Serialize, Deserialize, Debug)]
struct Task {
    id: String,
    name: String,
    dependencies: Vec<String>,
    #[serde(default)]
    metadata: serde_json::Value, // Arbitrary JSON
}

// Serialize
let task = Task {
    id: "task-1".into(),
    name: "Build".into(),
    dependencies: vec!["task-0".into()],
    metadata: serde_json::json!({"priority": "high"}),
};
let json = serde_json::to_string(&task)?;

// Deserialize
let parsed: Task = serde_json::from_str(&json)?;
```

---

### 2.2 serde-jsonlines (Recommended for JSONL)

**Crate**: [serde-jsonlines](https://crates.io/crates/serde-jsonlines) | [Documentation](https://docs.rs/serde-jsonlines)

Purpose-built for JSON Lines format with excellent append-only log support.

#### Key Features

- **Extension traits**: `BufReadExt::json_lines()`, `WriteExt::write_json_lines()`
- **File helpers**: `json_lines()`, `write_json_lines()`, `append_json_lines()`
- **Async support**: With `async` feature for tokio
- **Line-by-line control**: `JsonLinesReader` and `JsonLinesWriter`

#### Code Examples

**Basic JSONL read/write:**

```rust
use serde::{Deserialize, Serialize};
use serde_jsonlines::{json_lines, write_json_lines, append_json_lines};
use std::io::Result;

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct LogEntry {
    timestamp: u64,
    event: String,
    data: serde_json::Value,
}

fn main() -> Result<()> {
    let entries = vec![
        LogEntry {
            timestamp: 1000,
            event: "task_started".into(),
            data: serde_json::json!({"task_id": "a"}),
        },
        LogEntry {
            timestamp: 1001,
            event: "task_completed".into(),
            data: serde_json::json!({"task_id": "a", "result": "success"}),
        },
    ];

    // Write to file
    write_json_lines("events.jsonl", &entries)?;

    // Append more entries (append-only pattern)
    let new_entry = LogEntry {
        timestamp: 1002,
        event: "task_started".into(),
        data: serde_json::json!({"task_id": "b"}),
    };
    append_json_lines("events.jsonl", &[new_entry])?;

    // Read back
    let all_entries: Vec<LogEntry> = json_lines("events.jsonl")?
        .collect::<Result<Vec<_>>>()?;

    Ok(())
}
```

**Streaming read with different types per line:**

```rust
use serde_jsonlines::JsonLinesReader;
use std::fs::File;
use std::io::BufReader;

let file = File::open("mixed.jsonl")?;
let mut reader = JsonLinesReader::new(BufReader::new(file));

// Read lines one at a time, potentially different types
while let Some(result) = reader.read::<serde_json::Value>() {
    let value = result?;
    match value.get("type").and_then(|v| v.as_str()) {
        Some("task") => {
            let task: Task = serde_json::from_value(value)?;
            // handle task
        }
        Some("event") => {
            let event: Event = serde_json::from_value(value)?;
            // handle event
        }
        _ => { /* unknown type */ }
    }
}
```

**Async JSONL handling:**

```rust
use serde_jsonlines::{AsyncBufReadJsonLines, AsyncWriteJsonLines};
use tokio::fs::File;
use tokio::io::{BufReader, BufWriter};

async fn async_example() -> std::io::Result<()> {
    // Async write
    let file = File::create("async.jsonl").await?;
    let mut writer = BufWriter::new(file);
    writer.write_json_lines(&entries).await?;

    // Async read
    let file = File::open("async.jsonl").await?;
    let reader = BufReader::new(file);
    let mut lines = reader.json_lines::<LogEntry>();

    while let Some(entry) = lines.next().await {
        println!("{:?}", entry?);
    }
    Ok(())
}
```

#### Cargo.toml

```toml
[dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
serde-jsonlines = { version = "0.5", features = ["async"] }
```

---

### 2.3 Append-Only Log Pattern

For a workgraph system, the append-only JSONL pattern is ideal:

```rust
use serde::{Deserialize, Serialize};
use serde_jsonlines::append_json_lines;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
enum WorkgraphEvent {
    NodeAdded { id: String, data: serde_json::Value },
    EdgeAdded { from: String, to: String, weight: Option<f64> },
    NodeUpdated { id: String, data: serde_json::Value },
    NodeRemoved { id: String },
}

struct EventLog {
    path: std::path::PathBuf,
}

impl EventLog {
    fn append(&self, event: &WorkgraphEvent) -> std::io::Result<()> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer(&mut writer, event)?;
        writeln!(writer)?;
        writer.flush()?;
        Ok(())
    }

    fn replay(&self) -> std::io::Result<Vec<WorkgraphEvent>> {
        serde_jsonlines::json_lines(&self.path)?
            .collect::<std::io::Result<Vec<_>>>()
    }
}
```

---

### 2.4 Alternative: jsonl crate

Simpler API but fewer features:

```rust
use jsonl::{read, write};

let values: Vec<MyStruct> = read(reader)?;
write(writer, &values)?;
```

---

## 3. CLI Frameworks

### 3.1 clap (Recommended)

**Crate**: [clap](https://crates.io/crates/clap) | [Documentation](https://docs.rs/clap)

The most popular and feature-rich CLI parsing library in Rust.

#### Key Features

- **Derive macro**: Define CLI via struct annotations
- **Builder API**: Programmatic construction
- **Subcommands**: Git-style nested commands
- **Auto-generated help**: Always in sync with code
- **Shell completions**: Bash, Zsh, Fish, PowerShell
- **Validation**: Type checking, value ranges, custom validators
- **Environment variables**: Fallback to env vars

#### Code Examples

**Basic CLI with derive:**

```rust
use clap::{Parser, Subcommand, Args};
use std::path::PathBuf;

/// workgraph CLI - manage task dependency graphs
#[derive(Parser)]
#[command(name = "wg")]
#[command(version, about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    /// Path to the workgraph file
    #[arg(short, long, default_value = "workgraph.jsonl")]
    file: PathBuf,

    /// Enable verbose output
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Add a new task to the graph
    Add(AddArgs),

    /// List tasks in the graph
    List {
        /// Filter by status
        #[arg(short, long)]
        status: Option<String>,

        /// Output format
        #[arg(short, long, default_value = "table")]
        format: OutputFormat,
    },

    /// Run tasks in dependency order
    Run {
        /// Specific task to run (runs all if omitted)
        task: Option<String>,

        /// Dry run - show what would execute
        #[arg(short = 'n', long)]
        dry_run: bool,
    },

    /// Show task dependencies
    Deps {
        /// Task to show dependencies for
        task: String,

        /// Show reverse dependencies (dependents)
        #[arg(short, long)]
        reverse: bool,
    },

    /// Watch for file changes and re-run
    Watch {
        /// Paths to watch
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
}

#[derive(Args)]
struct AddArgs {
    /// Task name
    name: String,

    /// Task dependencies
    #[arg(short, long)]
    depends_on: Vec<String>,

    /// Command to execute
    #[arg(short, long)]
    command: Option<String>,
}

#[derive(Clone, clap::ValueEnum)]
enum OutputFormat {
    Table,
    Json,
    Jsonl,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Add(args) => {
            println!("Adding task: {} (depends on: {:?})", args.name, args.depends_on);
        }
        Commands::List { status, format } => {
            println!("Listing tasks (status: {:?}, format: {:?})", status, format);
        }
        Commands::Run { task, dry_run } => {
            if dry_run {
                println!("Would run: {:?}", task);
            } else {
                println!("Running: {:?}", task);
            }
        }
        Commands::Deps { task, reverse } => {
            println!("Dependencies for {} (reverse: {})", task, reverse);
        }
        Commands::Watch { paths } => {
            println!("Watching: {:?}", paths);
        }
    }
}
```

**Handling global options:**

```rust
#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    global: GlobalOpts,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Args)]
struct GlobalOpts {
    /// Config file path
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    /// Suppress output
    #[arg(short, long, global = true)]
    quiet: bool,
}
```

#### Cargo.toml

```toml
[dependencies]
clap = { version = "4.4", features = ["derive", "env"] }
```

---

### 3.2 argh

**Crate**: [argh](https://crates.io/crates/argh)

Lightweight, derive-based parser from Google (Fuchsia project).

#### Trade-offs

| Pros | Cons |
|------|------|
| Minimal runtime overhead | Follows Fuchsia conventions, not Unix |
| Fast compilation | Fewer features than clap |
| Simple API | Smaller ecosystem |

**Not recommended** for Unix CLI tools due to convention differences.

---

### 3.3 pico-args

**Crate**: [pico-args](https://crates.io/crates/pico-args)

Zero-dependency, minimal parser for simple tools.

```rust
use pico_args::Arguments;

let mut args = Arguments::from_env();
let verbose: bool = args.contains("-v");
let output: Option<String> = args.opt_value_from_str("-o")?;
let input: String = args.free_from_str()?;
```

#### Trade-offs

| Pros | Cons |
|------|------|
| Zero dependencies | No help generation |
| Tiny binary size | No derive support |
| Fast compilation | Manual validation |

---

### 3.4 CLI Framework Recommendation

**Use clap** for the workgraph CLI:
- Subcommand support is essential for a tool like this
- Auto-generated help keeps documentation in sync
- Shell completions improve user experience
- Active maintenance and large community

---

## 4. SQLite Bindings

### 4.1 rusqlite (Recommended for Simplicity)

**Crate**: [rusqlite](https://crates.io/crates/rusqlite) | [Documentation](https://docs.rs/rusqlite)

Synchronous, ergonomic SQLite bindings.

#### Key Features

- **Synchronous API**: Simple, no async runtime needed
- **Prepared statements**: Efficient repeated queries
- **Transactions**: Full ACID support
- **Bundled SQLite**: Optional, avoids system dependency
- **Type mapping**: Automatic Rust <-> SQLite type conversion

#### Code Examples

**Basic usage:**

```rust
use rusqlite::{Connection, Result, params};

fn main() -> Result<()> {
    let conn = Connection::open("workgraph.db")?;

    // Create schema
    conn.execute(
        "CREATE TABLE IF NOT EXISTS tasks (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            status TEXT DEFAULT 'pending',
            data JSON,
            created_at INTEGER DEFAULT (unixepoch())
        )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS edges (
            from_id TEXT NOT NULL,
            to_id TEXT NOT NULL,
            weight REAL,
            PRIMARY KEY (from_id, to_id),
            FOREIGN KEY (from_id) REFERENCES tasks(id),
            FOREIGN KEY (to_id) REFERENCES tasks(id)
        )",
        [],
    )?;

    // Insert a task
    conn.execute(
        "INSERT INTO tasks (id, name, data) VALUES (?1, ?2, ?3)",
        params!["task-1", "Build", r#"{"priority": "high"}"#],
    )?;

    // Query tasks
    let mut stmt = conn.prepare("SELECT id, name, status FROM tasks WHERE status = ?1")?;
    let task_iter = stmt.query_map(["pending"], |row| {
        Ok(Task {
            id: row.get(0)?,
            name: row.get(1)?,
            status: row.get(2)?,
        })
    })?;

    for task in task_iter {
        println!("Found task: {:?}", task?);
    }

    Ok(())
}

#[derive(Debug)]
struct Task {
    id: String,
    name: String,
    status: String,
}
```

**Transactions:**

```rust
use rusqlite::{Connection, Result, Transaction};

fn add_task_with_edges(conn: &mut Connection, task: &Task, edges: &[(String, String)]) -> Result<()> {
    let tx = conn.transaction()?;

    tx.execute(
        "INSERT INTO tasks (id, name) VALUES (?1, ?2)",
        params![task.id, task.name],
    )?;

    for (from, to) in edges {
        tx.execute(
            "INSERT INTO edges (from_id, to_id) VALUES (?1, ?2)",
            params![from, to],
        )?;
    }

    tx.commit()?;
    Ok(())
}
```

**Named parameters:**

```rust
use rusqlite::named_params;

conn.execute(
    "UPDATE tasks SET status = :status WHERE id = :id",
    named_params! {
        ":id": task_id,
        ":status": new_status,
    },
)?;
```

#### Cargo.toml

```toml
[dependencies]
rusqlite = { version = "0.31", features = ["bundled"] }
```

---

### 4.2 sqlx (Recommended for Async)

**Crate**: [sqlx](https://crates.io/crates/sqlx) | [Documentation](https://docs.rs/sqlx)

Async SQL toolkit with compile-time query checking.

#### Key Features

- **Async-native**: Built for async/await
- **Compile-time checking**: `query!` macro validates SQL at compile time
- **Multi-database**: PostgreSQL, MySQL, SQLite
- **Connection pooling**: Built-in pool management
- **Migrations**: Built-in migration system

#### Code Examples

**Basic async usage:**

```rust
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use sqlx::FromRow;

#[derive(FromRow, Debug)]
struct Task {
    id: String,
    name: String,
    status: String,
}

#[tokio::main]
async fn main() -> Result<(), sqlx::Error> {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect("sqlite:workgraph.db?mode=rwc").await?;

    // Run migrations
    sqlx::migrate!("./migrations").run(&pool).await?;

    // Insert with compile-time checked query
    sqlx::query!(
        "INSERT INTO tasks (id, name) VALUES (?, ?)",
        "task-1",
        "Build"
    )
    .execute(&pool)
    .await?;

    // Query with automatic struct mapping
    let tasks = sqlx::query_as::<_, Task>(
        "SELECT id, name, status FROM tasks WHERE status = ?"
    )
    .bind("pending")
    .fetch_all(&pool)
    .await?;

    for task in tasks {
        println!("{:?}", task);
    }

    Ok(())
}
```

**Compile-time checked queries:**

```rust
// Requires DATABASE_URL env var at compile time
let task = sqlx::query_as!(
    Task,
    r#"SELECT id, name, status as "status: _" FROM tasks WHERE id = ?"#,
    task_id
)
.fetch_optional(&pool)
.await?;
```

#### Cargo.toml

```toml
[dependencies]
sqlx = { version = "0.7", features = ["runtime-tokio", "sqlite"] }
tokio = { version = "1", features = ["full"] }
```

---

### 4.3 Comparison

| Feature | rusqlite | sqlx |
|---------|----------|------|
| **Async** | No (sync only) | Yes (native async) |
| **Compile-time checks** | No | Yes (`query!` macro) |
| **Multi-database** | SQLite only | PostgreSQL, MySQL, SQLite |
| **Simplicity** | Higher | Lower (requires async runtime) |
| **Connection pooling** | Manual | Built-in |
| **Best for** | Simple embedded apps | Async services, multiple DBs |

---

### 4.4 SQLite Recommendation

**For workgraph system**:

- **Start with rusqlite**: Simpler, no async complexity, perfect for CLI tools
- **Migrate to sqlx** if: You need async operations or might support other databases

---

## 5. File Watching

### 5.1 notify (Recommended)

**Crate**: [notify](https://crates.io/crates/notify) | [Documentation](https://docs.rs/notify)

Cross-platform filesystem notification library, used by rust-analyzer, cargo-watch, and deno.

#### Key Features

- **Cross-platform**: Linux (inotify), macOS (FSEvents), Windows (ReadDirectoryChanges)
- **Multiple backends**: Native + polling fallback
- **Debouncing**: Built-in debouncer to batch rapid changes
- **Recursive watching**: Watch entire directory trees

#### Code Examples

**Basic file watching:**

```rust
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::mpsc::channel;
use std::time::Duration;

fn main() -> notify::Result<()> {
    let (tx, rx) = channel();

    let mut watcher = RecommendedWatcher::new(tx, Config::default())?;

    // Watch the workgraph file
    watcher.watch(Path::new("workgraph.jsonl"), RecursiveMode::NonRecursive)?;

    // Watch a directory recursively
    watcher.watch(Path::new("./tasks"), RecursiveMode::Recursive)?;

    println!("Watching for changes...");

    for result in rx {
        match result {
            Ok(event) => handle_event(event),
            Err(e) => eprintln!("Watch error: {:?}", e),
        }
    }

    Ok(())
}

fn handle_event(event: Event) {
    use notify::EventKind::*;

    match event.kind {
        Create(_) => {
            println!("Created: {:?}", event.paths);
        }
        Modify(_) => {
            println!("Modified: {:?}", event.paths);
        }
        Remove(_) => {
            println!("Removed: {:?}", event.paths);
        }
        _ => {}
    }
}
```

**With debouncing:**

```rust
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};
use std::path::Path;
use std::time::Duration;

fn main() -> notify::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();

    // Debounce events - wait 500ms after last change
    let mut debouncer = new_debouncer(Duration::from_millis(500), tx)?;

    debouncer.watcher().watch(
        Path::new("workgraph.jsonl"),
        RecursiveMode::NonRecursive
    )?;

    for result in rx {
        match result {
            Ok(events) => {
                for event in events {
                    println!("Debounced event: {:?}", event.path);
                    // Rebuild/reload workgraph here
                }
            }
            Err(e) => eprintln!("Error: {:?}", e),
        }
    }

    Ok(())
}
```

**Async with tokio:**

```rust
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> notify::Result<()> {
    let (tx, mut rx) = mpsc::channel(100);

    let mut watcher = RecommendedWatcher::new(
        move |result: Result<Event, notify::Error>| {
            let tx = tx.clone();
            tokio::spawn(async move {
                let _ = tx.send(result).await;
            });
        },
        Config::default(),
    )?;

    watcher.watch(Path::new("workgraph.jsonl"), RecursiveMode::NonRecursive)?;

    while let Some(result) = rx.recv().await {
        match result {
            Ok(event) => println!("Event: {:?}", event),
            Err(e) => eprintln!("Error: {:?}", e),
        }
    }

    Ok(())
}
```

#### Platform Considerations

| Platform | Backend | Notes |
|----------|---------|-------|
| Linux | inotify | May hit user limits on large trees |
| macOS | FSEvents | Works well, M1 Docker needs PollWatcher |
| Windows | ReadDirectoryChanges | Generally reliable |
| Network FS | PollWatcher | Native watchers don't work |

#### Cargo.toml

```toml
[dependencies]
notify = "8.2"
notify-debouncer-mini = "0.5"  # For debouncing
```

---

## 6. Recommendations Summary

### Core Dependencies

```toml
[dependencies]
# Graph structure
petgraph = { version = "0.6", features = ["serde-1"] }
# Or for strict DAGs:
# daggy = { version = "0.9", features = ["serde-1"] }

# JSON/JSONL handling
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
serde-jsonlines = { version = "0.5", features = ["async"] }

# CLI
clap = { version = "4.4", features = ["derive", "env"] }

# SQLite (choose one)
rusqlite = { version = "0.31", features = ["bundled"] }
# Or for async:
# sqlx = { version = "0.7", features = ["runtime-tokio", "sqlite"] }

# File watching
notify = "8.2"
notify-debouncer-mini = "0.5"

# Error handling
anyhow = "1.0"
thiserror = "1.0"
```

### Architecture Recommendations

1. **Graph Storage**: Use petgraph's `StableGraph` for the in-memory representation
2. **Persistence**: JSONL append-only log as primary storage, SQLite as optional index
3. **CLI**: clap with derive macros for ergonomic command structure
4. **File Watching**: notify with debouncing for watch mode

### References

- [petgraph GitHub](https://github.com/petgraph/petgraph)
- [petgraph Documentation](https://docs.rs/petgraph/latest/petgraph/)
- [daggy GitHub](https://github.com/mitchmindtree/daggy)
- [daggy Documentation](https://docs.rs/daggy)
- [serde-jsonlines GitHub](https://github.com/jwodder/serde-jsonlines)
- [serde-jsonlines Documentation](https://docs.rs/serde-jsonlines)
- [clap Documentation](https://docs.rs/clap/latest/clap/)
- [Rain's Rust CLI Recommendations](https://rust-cli-recommendations.sunshowers.io/)
- [rusqlite Documentation](https://docs.rs/rusqlite/latest/rusqlite/)
- [sqlx GitHub](https://github.com/launchbadge/sqlx)
- [notify GitHub](https://github.com/notify-rs/notify)
- [notify Documentation](https://docs.rs/notify/)
