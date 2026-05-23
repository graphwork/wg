use std::path::Path;
use std::process::{Command, Output};

fn wg_binary() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_wg") {
        return p.into();
    }
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    Path::new(&manifest).join("target/debug/wg")
}

fn nex_binary() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_nex") {
        return p.into();
    }
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    Path::new(&manifest).join("target/debug/nex")
}

fn output_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn standalone_nex_help_exposes_shared_options() {
    let output = Command::new(nex_binary())
        .arg("--help")
        .output()
        .expect("spawn nex --help");
    let text = output_text(&output);

    assert!(output.status.success(), "nex --help failed:\n{text}");
    assert!(
        text.contains("Usage: nex"),
        "standalone help should render as nex, got:\n{text}"
    );
    for flag in [
        "--model",
        "--endpoint",
        "--resume",
        "--chat",
        "--read-only",
        "--minimal-tools",
        "--eval-mode",
    ] {
        assert!(
            text.contains(flag),
            "standalone nex help missing {flag}:\n{text}"
        );
    }
}

#[test]
fn wg_nex_help_keeps_compatibility_options() {
    let output = Command::new(wg_binary())
        .args(["nex", "--help"])
        .output()
        .expect("spawn wg nex --help");
    let text = output_text(&output);

    assert!(output.status.success(), "wg nex --help failed:\n{text}");
    assert!(
        text.contains("Usage: wg nex") || text.contains("Usage: wg [OPTIONS] nex"),
        "wg nex help should render as a wg subcommand, got:\n{text}"
    );
    for flag in [
        "--model",
        "--endpoint",
        "--resume",
        "--chat",
        "--read-only",
        "--minimal-tools",
        "--eval-mode",
    ] {
        assert!(text.contains(flag), "wg nex help missing {flag}:\n{text}");
    }
}
