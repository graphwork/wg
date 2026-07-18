//! Real-process regressions for early-closing CLI pipeline consumers.

#[cfg(unix)]
mod unix {
    use std::fs;
    use std::os::unix::process::ExitStatusExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use tempfile::TempDir;
    use worksgood::graph::{Node, Task, WorkGraph};
    use worksgood::parser::save_graph;

    fn wg_binary() -> PathBuf {
        let mut path = std::env::current_exe().expect("could not get current exe path");
        path.pop();
        if path.ends_with("deps") {
            path.pop();
        }
        path.push("wg");
        assert!(
            path.exists(),
            "wg binary not found at {:?}. Run `cargo build` first.",
            path
        );
        path
    }

    fn run(wg_dir: &Path, args: &[&str]) -> std::process::Output {
        Command::new(wg_binary())
            .arg("--dir")
            .arg(wg_dir)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap_or_else(|error| panic!("failed to run wg {args:?}: {error}"))
    }

    #[test]
    fn show_to_head_exits_quietly_and_preserves_normal_output() {
        let tmp = TempDir::new().unwrap();
        let wg_dir = tmp.path().join(".wg");
        fs::create_dir_all(&wg_dir).unwrap();

        let description = (0..3_000)
            .map(|index| format!("pipeline regression line {index:05}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(Task {
            id: "pipe-task".to_string(),
            title: "Pipeline regression task".to_string(),
            description: Some(description),
            ..Task::default()
        }));
        save_graph(&graph, &wg_dir.join("graph.jsonl")).unwrap();

        let normal = run(&wg_dir, &["show", "pipe-task"]);
        assert!(
            normal.status.success(),
            "normal wg show failed: {}",
            String::from_utf8_lossy(&normal.stderr)
        );
        assert!(normal.stderr.is_empty(), "normal wg show wrote to stderr");
        let normal_stdout = String::from_utf8(normal.stdout).unwrap();
        assert!(normal_stdout.starts_with("Task: pipe-task\n"));
        assert!(normal_stdout.contains("pipeline regression line 02999"));

        // Exercise the actual human terminal flow: the real wg process writes
        // directly into the real `head -n 1` process, which closes the pipe as
        // soon as it has consumed the first line.
        let mut wg = Command::new(wg_binary())
            .arg("--dir")
            .arg(&wg_dir)
            .args(["show", "pipe-task"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn wg show");
        let wg_stdout = wg.stdout.take().expect("wg stdout was not piped");
        let head = Command::new("head")
            .args(["-n", "1"])
            .stdin(Stdio::from(wg_stdout))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn head -n 1")
            .wait_with_output()
            .expect("failed to wait for head");
        let wg_output = wg.wait_with_output().expect("failed to wait for wg show");

        assert!(head.status.success(), "head failed: {head:?}");
        assert_eq!(head.stdout, b"Task: pipe-task\n");
        assert!(head.stderr.is_empty(), "head wrote to stderr: {head:?}");
        assert_eq!(
            wg_output.status.signal(),
            Some(libc::SIGPIPE),
            "wg show should terminate by the conventional SIGPIPE without a panic; status={:?}, stderr={}",
            wg_output.status,
            String::from_utf8_lossy(&wg_output.stderr)
        );
        assert!(
            wg_output.stderr.is_empty(),
            "wg show emitted a panic/backtrace: {}",
            String::from_utf8_lossy(&wg_output.stderr)
        );
    }
}
