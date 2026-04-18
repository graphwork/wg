//! Transport layer for MCP servers.
//!
//! Only stdio is implemented in v1. A server runs as a child
//! subprocess; we write newline-delimited JSON to its stdin and
//! read newline-delimited JSON from its stdout. Stderr is tee'd to
//! a log file for debugging.

use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;

/// A running MCP-server subprocess with duplex JSON-line I/O.
///
/// `incoming` receives each complete line parsed from the server's
/// stdout. `outgoing` is the stdin writer. Stderr lines are emitted
/// to `stderr_log` if configured, otherwise discarded.
pub struct StdioTransport {
    pub child: Child,
    pub stdin: ChildStdin,
    pub incoming: mpsc::UnboundedReceiver<String>,
    pub _stderr_task: tokio::task::JoinHandle<()>,
    pub _stdout_task: tokio::task::JoinHandle<()>,
}

impl StdioTransport {
    /// Spawn `command` with `args` + `env`. Set up the stdin/stdout
    /// pipes and background readers. `stderr_log` is appended to if
    /// provided; a None discards stderr.
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &[(String, String)],
        stderr_log: Option<PathBuf>,
    ) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn MCP server {:?}", command))?;

        let stdin = child
            .stdin
            .take()
            .context("take stdin on spawned MCP server")?;
        let stdout = child
            .stdout
            .take()
            .context("take stdout on spawned MCP server")?;
        let stderr = child
            .stderr
            .take()
            .context("take stderr on spawned MCP server")?;

        let (tx, rx) = mpsc::unbounded_channel::<String>();
        let stdout_task = spawn_stdout_reader(stdout, tx);
        let stderr_task = spawn_stderr_logger(stderr, stderr_log);

        Ok(Self {
            child,
            stdin,
            incoming: rx,
            _stderr_task: stderr_task,
            _stdout_task: stdout_task,
        })
    }

    /// Write one JSON-encoded message, terminated by `\n`, to the
    /// server's stdin. The MCP spec mandates one message per line.
    pub async fn send_line(&mut self, json: &str) -> Result<()> {
        self.stdin
            .write_all(json.as_bytes())
            .await
            .context("write MCP message body")?;
        self.stdin
            .write_all(b"\n")
            .await
            .context("write MCP message terminator")?;
        self.stdin.flush().await.context("flush MCP stdin")?;
        Ok(())
    }
}

fn spawn_stdout_reader(
    stdout: ChildStdout,
    tx: mpsc::UnboundedSender<String>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if tx.send(line).is_err() {
                break;
            }
        }
    })
}

fn spawn_stderr_logger(
    stderr: tokio::process::ChildStderr,
    stderr_log: Option<PathBuf>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        // If there's a log file, append lines to it. Otherwise drop.
        let mut file_handle = stderr_log.as_ref().and_then(|p| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .ok()
        });
        while let Ok(Some(line)) = reader.next_line().await {
            if let Some(f) = file_handle.as_mut() {
                use std::io::Write;
                let _ = writeln!(f, "{}", line);
            }
        }
    })
}
