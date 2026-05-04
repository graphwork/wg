use std::process::Command;
use tempfile::TempDir;

fn git(dir: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "wg test")
        .env("GIT_AUTHOR_EMAIL", "wg-test@example.com")
        .env("GIT_COMMITTER_NAME", "wg test")
        .env("GIT_COMMITTER_EMAIL", "wg-test@example.com")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

#[test]
fn dev_check_warns_on_non_main_branch() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    git(repo, &["init", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "test\n").unwrap();
    git(repo, &["add", "README.md"]);
    git(repo, &["commit", "-m", "initial"]);
    git(repo, &["checkout", "-b", "wg/agent-1398/fix-tui-perf"]);

    let output = Command::new(env!("CARGO_BIN_EXE_wg"))
        .arg("dev-check")
        .current_dir(repo)
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("branch: wg/agent-1398/fix-tui-perf"));
    assert!(stdout.contains("status: WARN"));
    assert!(stdout.contains("not 'main'"));
}
