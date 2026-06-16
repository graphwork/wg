#![cfg(unix)]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;
use worksgood::graph::WorkGraph;
use worksgood::parser::save_graph;

const OLD_MARKER: &[u8] = b"\nWG_UPGRADE_TEST_OLD_BINARY\n";
const NEW_MARKER: &[u8] = b"\nWG_UPGRADE_TEST_NEW_BINARY\n";

#[derive(Debug)]
struct RunEnv {
    home: PathBuf,
    cargo_home: PathBuf,
    path: String,
    cargo_log: PathBuf,
    fail_install: bool,
    fail_clean: bool,
}

#[derive(Debug)]
struct Fixture {
    home: PathBuf,
    wg_dir: PathBuf,
    cargo_home: PathBuf,
    installed: PathBuf,
    cargo_log: PathBuf,
    path: String,
}

impl Fixture {
    fn run_env(&self) -> RunEnv {
        RunEnv {
            home: self.home.clone(),
            cargo_home: self.cargo_home.clone(),
            path: self.path.clone(),
            cargo_log: self.cargo_log.clone(),
            fail_install: false,
            fail_clean: false,
        }
    }
}

fn wg_binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("current_exe");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("wg");
    assert!(path.exists(), "wg binary not found at {:?}", path);
    path
}

fn make_executable(path: &Path) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn fresh_workgraph(root: &Path) -> PathBuf {
    let wg_dir = root.join("project").join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();
    save_graph(&WorkGraph::new(), &wg_dir.join("graph.jsonl")).unwrap();
    wg_dir
}

fn install_old_wg(cargo_home: &Path) -> PathBuf {
    let install_dir = cargo_home.join("bin");
    fs::create_dir_all(&install_dir).unwrap();
    let installed = install_dir.join("wg");
    fs::copy(wg_binary(), &installed).unwrap();
    OpenOptions::new()
        .append(true)
        .open(&installed)
        .unwrap()
        .write_all(OLD_MARKER)
        .unwrap();
    make_executable(&installed);

    let nex = install_dir.join("nex");
    fs::write(&nex, "#!/bin/sh\necho old nex 0.0.1\n").unwrap();
    make_executable(&nex);
    installed
}

fn write_fake_cargo(root: &Path) -> (PathBuf, PathBuf) {
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let cargo = fake_bin.join("cargo");
    let script = r#"#!/bin/sh
set -eu
cmd="${1:-}"
printf '%s\n' "$cmd" >> "${WG_UPGRADE_TEST_CARGO_LOG:?}"
case "$cmd" in
  --version)
    echo "cargo 1.85.0 (fake)"
    ;;
  clean)
    if [ "${WG_UPGRADE_TEST_FAIL_CLEAN:-}" = "1" ]; then
      echo "fake cargo clean failure" >&2
      exit 43
    fi
    rm -rf target
    ;;
  install)
    if [ "${WG_UPGRADE_TEST_FAIL_INSTALL:-}" = "1" ]; then
      echo "fake cargo install failure" >&2
      exit 42
    fi
    dest="${CARGO_HOME:?}/bin"
    mkdir -p "$dest"
    tmp="$dest/wg.upgrade-test-tmp"
    cp "${WG_UPGRADE_TEST_WG_BINARY:?}" "$tmp"
    printf '\nWG_UPGRADE_TEST_NEW_BINARY\n' >> "$tmp"
    chmod 755 "$tmp"
    mv "$tmp" "$dest/wg"
    cat > "$dest/nex.upgrade-test-tmp" <<'NEX'
#!/bin/sh
echo nex fake 0.2.0
NEX
    chmod 755 "$dest/nex.upgrade-test-tmp"
    mv "$dest/nex.upgrade-test-tmp" "$dest/nex"
    ;;
  *)
    echo "unexpected fake cargo invocation: $*" >&2
    exit 64
    ;;
esac
"#;
    fs::write(&cargo, script).unwrap();
    make_executable(&cargo);

    let cargo_log = root.join("cargo.log");
    (fake_bin, cargo_log)
}

fn prepend_path(bin: &Path) -> String {
    let original = std::env::var_os("PATH").unwrap_or_default();
    format!("{}:{}", bin.display(), original.to_string_lossy())
}

fn setup_fixture(root: &Path) -> Fixture {
    let home = root.join("home");
    let cargo_home = root.join("cargo-home");
    let wg_dir = fresh_workgraph(root);
    let installed = install_old_wg(&cargo_home);
    let (fake_bin, cargo_log) = write_fake_cargo(root);
    Fixture {
        home,
        wg_dir,
        cargo_home,
        installed,
        cargo_log,
        path: prepend_path(&fake_bin),
    }
}

fn init_source_repo(root: &Path, version: &str) -> PathBuf {
    let repo = root.join("upstream");
    fs::create_dir_all(&repo).unwrap();
    let output = Command::new("git")
        .arg("init")
        .arg(&repo)
        .output()
        .expect("git init");
    assert!(
        output.status.success(),
        "git init failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    git(&repo, &["checkout", "-B", "main"]);
    git(
        &repo,
        &["config", "user.email", "wg-upgrade-test@example.invalid"],
    );
    git(&repo, &["config", "user.name", "WG Upgrade Test"]);
    commit_source_version(&repo, version);
    repo
}

fn commit_source_version(repo: &Path, version: &str) {
    fs::create_dir_all(repo.join("src/bin")).unwrap();
    fs::write(
        repo.join("Cargo.toml"),
        format!(
            r#"[package]
name = "workgraph"
version = "{version}"
edition = "2024"

[[bin]]
name = "wg"
path = "src/main.rs"

[[bin]]
name = "nex"
path = "src/bin/nex.rs"
"#
        ),
    )
    .unwrap();
    fs::write(repo.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(repo.join("src/bin/nex.rs"), "fn main() {}\n").unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-m", &format!("version {version}")]);
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap_or_else(|err| panic!("failed to run git {:?}: {}", args, err));
    assert!(
        output.status.success(),
        "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn which_program(name: &str) -> PathBuf {
    let output = Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {name}"))
        .output()
        .unwrap_or_else(|err| panic!("failed to locate {name}: {err}"));
    assert!(
        output.status.success(),
        "could not locate {name}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
}

fn git_only_path(root: &Path) -> String {
    let bin = root.join("git-only-bin");
    fs::create_dir_all(&bin).unwrap();
    symlink(which_program("git"), bin.join("git")).unwrap();
    bin.display().to_string()
}

fn run_wg(wg_path: &Path, wg_dir: &Path, env: &RunEnv, args: &[&str]) -> Output {
    let mut command = Command::new(wg_path);
    command
        .arg("--dir")
        .arg(wg_dir)
        .args(args)
        .env("HOME", &env.home)
        .env("CARGO_HOME", &env.cargo_home)
        .env("PATH", &env.path)
        .env("WG_UPGRADE_TEST_WG_BINARY", wg_binary())
        .env("WG_UPGRADE_TEST_CARGO_LOG", &env.cargo_log)
        .env_remove("WG_DIR")
        .env_remove("WG_UPGRADE_SOURCE_URL")
        .env_remove("WG_UPGRADE_REF")
        .env_remove("WG_UPGRADE_SOURCE_DIR")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if env.fail_install {
        command.env("WG_UPGRADE_TEST_FAIL_INSTALL", "1");
    } else {
        command.env_remove("WG_UPGRADE_TEST_FAIL_INSTALL");
    }
    if env.fail_clean {
        command.env("WG_UPGRADE_TEST_FAIL_CLEAN", "1");
    } else {
        command.env_remove("WG_UPGRADE_TEST_FAIL_CLEAN");
    }
    command
        .output()
        .unwrap_or_else(|err| panic!("failed to run wg {:?}: {}", args, err))
}

fn assert_success(output: Output, args: &[&str]) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        output.status.success(),
        "wg {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        stdout,
        stderr
    );
    stdout
}

fn assert_failure(output: Output, args: &[&str]) -> (String, String) {
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        !output.status.success(),
        "wg {:?} unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        args,
        stdout,
        stderr
    );
    (stdout, stderr)
}

fn assert_contains_marker(path: &Path, marker: &[u8]) {
    let bytes = fs::read(path).unwrap();
    assert!(
        bytes.windows(marker.len()).any(|window| window == marker),
        "{} did not contain marker {:?}",
        path.display(),
        String::from_utf8_lossy(marker)
    );
}

fn assert_lacks_marker(path: &Path, marker: &[u8]) {
    let bytes = fs::read(path).unwrap();
    assert!(
        !bytes.windows(marker.len()).any(|window| window == marker),
        "{} unexpectedly contained marker {:?}",
        path.display(),
        String::from_utf8_lossy(marker)
    );
}

#[test]
fn dry_run_reports_source_plan_without_writes() {
    let tmp = TempDir::new().unwrap();
    let fixture = setup_fixture(tmp.path());
    let upstream = init_source_repo(tmp.path(), "0.2.0");
    let source_dir = fixture.home.join(".wg/source/wg");

    let upstream_arg = upstream.display().to_string();
    let source_dir_arg = source_dir.display().to_string();
    let args = [
        "upgrade",
        "--dry-run",
        "--source",
        &upstream_arg,
        "--source-dir",
        &source_dir_arg,
    ];
    let stdout = assert_success(
        run_wg(
            &fixture.installed,
            &fixture.wg_dir,
            &fixture.run_env(),
            &args,
        ),
        &args,
    );

    assert!(stdout.contains("WG upgrade (dry run)"));
    assert!(stdout.contains("current version: 0.1.0"));
    assert!(stdout.contains("install source: Cargo install"));
    assert!(stdout.contains(&format!("source path: {}", source_dir.display())));
    assert!(stdout.contains(&format!("source upstream: {}", upstream.display())));
    assert!(stdout.contains("target ref: origin/main"));
    assert!(stdout.contains("target channel: source/origin/main"));
    assert!(stdout.contains("target version: unknown until source checkout is cloned/fetched"));
    assert!(stdout.contains("daemon: not running"));
    assert!(stdout.contains("cargo: cargo 1.85.0 (fake)"));
    assert!(stdout.contains("planned action: git clone"));
    assert!(stdout.contains("cargo install --path . --locked"));
    assert!(stdout.contains("wg migrate config --dry-run"));
    assert!(stdout.contains("wg migrate secrets --dry-run"));
    assert!(stdout.contains("graph-layout/profile/default checks"));
    assert!(stdout.contains("Disk usage:"));
    assert!(stdout.contains("managed source:"));
    assert!(stdout.contains("cargo target/cache:"));
    assert!(stdout.contains("Dry run complete. No files were changed."));

    assert!(!source_dir.exists(), "dry-run created the source checkout");
    assert!(
        !fixture.home.join(".wg/backups").exists(),
        "dry-run created backups"
    );
    assert_contains_marker(&fixture.installed, OLD_MARKER);
}

#[test]
fn source_managed_upgrade_clones_backs_up_binary_and_rollback_restores_it() {
    let tmp = TempDir::new().unwrap();
    let fixture = setup_fixture(tmp.path());
    let upstream = init_source_repo(tmp.path(), "0.2.0");
    let source_dir = fixture.home.join(".wg/source/wg");

    let upstream_arg = upstream.display().to_string();
    let source_dir_arg = source_dir.display().to_string();
    let args = [
        "upgrade",
        "--source",
        &upstream_arg,
        "--source-dir",
        &source_dir_arg,
        "--yes",
    ];
    let stdout = assert_success(
        run_wg(
            &fixture.installed,
            &fixture.wg_dir,
            &fixture.run_env(),
            &args,
        ),
        &args,
    );
    assert!(stdout.contains("Syncing source checkout:"));
    assert!(stdout.contains("Running cargo install:"));
    assert!(stdout.contains("Upgrade complete: 0.1.0 -> 0.2.0"));
    assert!(
        source_dir.join(".git").exists(),
        "source checkout was not cloned"
    );
    assert_contains_marker(&fixture.installed, NEW_MARKER);
    assert_lacks_marker(&fixture.installed, OLD_MARKER);

    let backup_root = fixture.home.join(".wg/backups/bin");
    let mut backups: Vec<_> = fs::read_dir(&backup_root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    backups.sort();
    let backup_wg = backups.last().unwrap().join("wg");
    assert_contains_marker(&backup_wg, OLD_MARKER);
    assert!(fixture.home.join(".wg/upgrade-state.toml").exists());

    let rollback_args = ["upgrade", "--rollback", "--yes"];
    let rollback_stdout = assert_success(
        run_wg(
            &fixture.installed,
            &fixture.wg_dir,
            &fixture.run_env(),
            &rollback_args,
        ),
        &rollback_args,
    );
    assert!(rollback_stdout.contains("Rollback complete."));
    assert_contains_marker(&fixture.installed, OLD_MARKER);
}

#[test]
fn source_managed_upgrade_fetches_and_resets_existing_checkout() {
    let tmp = TempDir::new().unwrap();
    let fixture = setup_fixture(tmp.path());
    let upstream = init_source_repo(tmp.path(), "0.2.0");
    let source_dir = fixture.home.join(".wg/source/wg");
    fs::create_dir_all(source_dir.parent().unwrap()).unwrap();
    let output = Command::new("git")
        .arg("clone")
        .arg(&upstream)
        .arg(&source_dir)
        .output()
        .expect("git clone fixture");
    assert!(
        output.status.success(),
        "git clone failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    commit_source_version(&upstream, "0.3.0");

    let upstream_arg = upstream.display().to_string();
    let source_dir_arg = source_dir.display().to_string();
    let args = [
        "upgrade",
        "--source",
        &upstream_arg,
        "--source-dir",
        &source_dir_arg,
        "--yes",
    ];
    let stdout = assert_success(
        run_wg(
            &fixture.installed,
            &fixture.wg_dir,
            &fixture.run_env(),
            &args,
        ),
        &args,
    );

    assert!(stdout.contains("package version: 0.3.0"));
    assert!(stdout.contains("Upgrade complete: 0.1.0 -> 0.3.0"));
    let cargo_toml = fs::read_to_string(source_dir.join("Cargo.toml")).unwrap();
    assert!(cargo_toml.contains("version = \"0.3.0\""));
    assert_contains_marker(&fixture.installed, NEW_MARKER);
}

#[test]
fn package_manager_install_is_refused_with_owner_command() {
    let tmp = TempDir::new().unwrap();
    let fixture = setup_fixture(tmp.path());
    let brew_dir = tmp.path().join("homebrew/Cellar/wg/0.1.0/bin");
    fs::create_dir_all(&brew_dir).unwrap();
    let brew_wg = brew_dir.join("wg");
    fs::copy(wg_binary(), &brew_wg).unwrap();
    make_executable(&brew_wg);

    let args = ["upgrade", "--dry-run"];
    let (stdout, stderr) = assert_failure(
        run_wg(&brew_wg, &fixture.wg_dir, &fixture.run_env(), &args),
        &args,
    );

    assert!(stdout.contains("install source: Homebrew"));
    assert!(stdout.contains("Use: brew upgrade graphwork/tap/wg"));
    assert!(stderr.contains("wg upgrade source-managed mode refuses Homebrew"));
}

#[test]
fn missing_cargo_fails_with_prerequisite_hint() {
    let tmp = TempDir::new().unwrap();
    let fixture = setup_fixture(tmp.path());
    let upstream = init_source_repo(tmp.path(), "0.2.0");
    let source_dir = fixture.home.join(".wg/source/wg");
    let env = RunEnv {
        path: git_only_path(tmp.path()),
        ..fixture.run_env()
    };

    let upstream_arg = upstream.display().to_string();
    let source_dir_arg = source_dir.display().to_string();
    let args = [
        "upgrade",
        "--source",
        &upstream_arg,
        "--source-dir",
        &source_dir_arg,
        "--yes",
    ];
    let (_stdout, stderr) = assert_failure(
        run_wg(&fixture.installed, &fixture.wg_dir, &env, &args),
        &args,
    );

    assert!(stderr.contains("preflight phase failed: required tool `cargo` is unavailable"));
    assert!(stderr.contains("install Rust/Cargo"));
    assert!(
        !source_dir.exists(),
        "upgrade synced source after cargo preflight failed"
    );
}

#[test]
fn cargo_install_failure_reports_phase_and_bug_context() {
    let tmp = TempDir::new().unwrap();
    let fixture = setup_fixture(tmp.path());
    let upstream = init_source_repo(tmp.path(), "0.2.0");
    let source_dir = fixture.home.join(".wg/source/wg");
    let env = RunEnv {
        fail_install: true,
        ..fixture.run_env()
    };

    let upstream_arg = upstream.display().to_string();
    let source_dir_arg = source_dir.display().to_string();
    let args = [
        "upgrade",
        "--source",
        &upstream_arg,
        "--source-dir",
        &source_dir_arg,
        "--yes",
    ];
    let (_stdout, stderr) = assert_failure(
        run_wg(&fixture.installed, &fixture.wg_dir, &env, &args),
        &args,
    );

    assert!(stderr.contains("cargo install phase failed"));
    assert!(stderr.contains("file a bug with this phase name"));
    assert!(stderr.contains("fake cargo install failure"));
    assert_contains_marker(&fixture.installed, OLD_MARKER);
}

#[test]
fn disk_usage_reported_and_clean_mode_runs_before_install() {
    let tmp = TempDir::new().unwrap();
    let fixture = setup_fixture(tmp.path());
    let upstream = init_source_repo(tmp.path(), "0.2.0");
    let source_dir = fixture.home.join(".wg/source/wg");
    fs::create_dir_all(source_dir.parent().unwrap()).unwrap();
    let output = Command::new("git")
        .arg("clone")
        .arg(&upstream)
        .arg(&source_dir)
        .output()
        .expect("git clone fixture");
    assert!(
        output.status.success(),
        "git clone failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    fs::create_dir_all(source_dir.join("target/cache")).unwrap();
    fs::write(source_dir.join("target/cache/blob"), vec![b'x'; 8192]).unwrap();

    let upstream_arg = upstream.display().to_string();
    let source_dir_arg = source_dir.display().to_string();
    let args = [
        "upgrade",
        "--source",
        &upstream_arg,
        "--source-dir",
        &source_dir_arg,
        "--clean",
        "--yes",
    ];
    let stdout = assert_success(
        run_wg(
            &fixture.installed,
            &fixture.wg_dir,
            &fixture.run_env(),
            &args,
        ),
        &args,
    );

    assert!(stdout.contains("Disk usage:"));
    assert!(stdout.contains("managed source:"));
    assert!(stdout.contains("cargo target/cache:"));
    assert!(stdout.contains("Running cargo clean before build:"));
    assert!(stdout.contains("Disk usage after upgrade:"));
    assert!(
        !source_dir.join("target/cache/blob").exists(),
        "cargo clean did not remove target build output"
    );

    let log = fs::read_to_string(&fixture.cargo_log).unwrap();
    let clean_pos = log.find("clean").expect("fake cargo did not record clean");
    let install_pos = log
        .find("install")
        .expect("fake cargo did not record install");
    assert!(
        clean_pos < install_pos,
        "cargo clean should run before cargo install; log:\n{}",
        log
    );
}
