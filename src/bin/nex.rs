use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use workgraph::nex_cli::NexArgs;

#[derive(Parser, Debug)]
#[command(name = "nex")]
#[command(about = "Interactive agentic REPL powered by WG's native executor")]
#[command(version)]
struct NexCli {
    /// Path to the WG directory (default: .wg in current dir; legacy .workgraph accepted)
    #[arg(long, global = true)]
    dir: Option<PathBuf>,

    #[command(flatten)]
    args: NexArgs,
}

fn main() -> Result<()> {
    init_logging();

    let cli = NexCli::parse();
    let workgraph_dir = workgraph::workgraph_dir::resolve_workgraph_dir(
        cli.dir.clone(),
        std::env::var_os("WG_DIR").map(PathBuf::from),
        std::env::current_dir().ok(),
        dirs::home_dir(),
    );

    if !workgraph_dir.exists()
        && let Some(home) = dirs::home_dir()
        && workgraph_dir == home.join(".wg")
    {
        if let Err(e) = std::fs::create_dir_all(&workgraph_dir) {
            eprintln!(
                "warning: failed to create global WG dir {}: {}",
                workgraph_dir.display(),
                e
            );
        } else {
            eprintln!(
                "\x1b[2m[nex] created global WG directory: {}\x1b[0m",
                workgraph_dir.display()
            );
        }
    }

    let workgraph_dir = workgraph_dir.canonicalize().unwrap_or(workgraph_dir);
    workgraph::usage::append_usage_log(&workgraph_dir, "nex");
    workgraph::nex::run_args(&workgraph_dir, &cli.args, "nex")
}

fn init_logging() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,html5ever=error,selectors=error"),
    )
    .format_timestamp(None)
    .init();
}
