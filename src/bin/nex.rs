use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use worksgood::nex_cli::NexArgs;

#[derive(Parser, Debug)]
#[command(name = "nex")]
#[command(about = "Interactive agentic REPL powered by WG's native executor")]
#[command(version)]
struct NexCli {
    /// Path to a WG directory. Legacy compatibility mode; prefer `wg nex`
    /// for WG sessions or `--nex-dir` for standalone sessions.
    #[arg(long, global = true)]
    dir: Option<PathBuf>,

    /// Force WG compatibility mode, resolving a WG directory from cwd/WG_DIR.
    #[arg(long, global = true)]
    wg: bool,

    #[command(flatten)]
    args: NexArgs,
}

fn main() -> Result<()> {
    init_logging();

    let cli = NexCli::parse();
    let runtime = if cli.dir.is_some() || cli.wg {
        let workgraph_dir = worksgood::workgraph_dir::resolve_workgraph_dir(
            cli.dir.clone(),
            std::env::var_os("WG_DIR").map(PathBuf::from),
            std::env::current_dir().ok(),
            dirs::home_dir(),
        );
        worksgood::nex_runtime::resolve_legacy_wg_compat(
            workgraph_dir.canonicalize().unwrap_or(workgraph_dir),
            dirs::home_dir(),
        )
    } else if cli.args.eval_mode {
        resolve_standalone_eval(&standalone_input(&cli.args))
    } else {
        worksgood::nex_runtime::resolve_standalone(&standalone_input(&cli.args))
    };

    if runtime.state_root.exists() {
        worksgood::usage::append_usage_log(&runtime.state_root, "nex");
    }
    worksgood::nex::run_args_with_runtime(&runtime, &cli.args, "nex")
}

fn standalone_input(args: &NexArgs) -> worksgood::nex_runtime::NexRuntimeResolveInput {
    worksgood::nex_runtime::NexRuntimeResolveInput {
        cwd: std::env::current_dir().ok(),
        home_dir: dirs::home_dir(),
        cli_nex_dir: args.nex_dir.clone(),
        env_nex_dir: std::env::var_os("NEX_DIR").map(PathBuf::from),
        env_nex_home: std::env::var_os("NEX_HOME").map(PathBuf::from),
        explicit_config: args
            .config
            .clone()
            .or_else(|| std::env::var_os("NEX_CONFIG").map(PathBuf::from)),
    }
}

fn resolve_standalone_eval(
    input: &worksgood::nex_runtime::NexRuntimeResolveInput,
) -> worksgood::nex_runtime::NexRuntime {
    let mut runtime = worksgood::nex_runtime::resolve_eval(input);
    let standalone = worksgood::nex_runtime::resolve_standalone(input);

    runtime.config_paths = merge_unique_paths(standalone.config_paths, runtime.config_paths);
    runtime.model_registry_paths = merge_unique_paths(
        standalone.model_registry_paths,
        runtime.model_registry_paths,
    );
    runtime.legacy_session_roots = standalone.legacy_session_roots;
    runtime
}

fn merge_unique_paths(mut low: Vec<PathBuf>, high: Vec<PathBuf>) -> Vec<PathBuf> {
    for path in high {
        if !low.contains(&path) {
            low.push(path);
        }
    }
    low
}

fn init_logging() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,html5ever=error,selectors=error"),
    )
    .format_timestamp(None)
    .init();
}
