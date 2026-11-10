//! Setup: a stable copy of the binary, the `adb` stand-in, and the Claude Code plugin.

use crate::store;
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;
use std::{fs, io};

fn bin() -> PathBuf {
    store::home().join("bin").join("adbrent")
}

fn shim_dir() -> PathBuf {
    store::home().join("shim")
}

/// Single-quote a string for the shell.
fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Shell lines that put the stand-in first on PATH, and optionally name the session.
pub fn env_lines(session: Option<&str>) -> String {
    let shim = sq(&shim_dir().to_string_lossy());
    let bin_dir = sq(&store::home().join("bin").to_string_lossy());
    let path = format!(
        "case \":$PATH:\" in *:{shim}:*) ;; *) export PATH={shim}:{bin_dir}:\"$PATH\" ;; esac"
    );
    match session {
        Some(id) => format!("export ADBRENT_SESSION={}\n{path}", sq(id)),
        None => path,
    }
}

/// Make sure `shim/adb` exists and points at a runnable adbrent.
pub fn ensure_shim() -> io::Result<()> {
    let link = shim_dir().join("adb");
    if link.exists() {
        return Ok(());
    }
    fs::create_dir_all(shim_dir())?;
    let _ = fs::remove_file(&link); // a dangling link
    let target = if bin().exists() {
        bin()
    } else {
        std::env::current_exe()?
    };
    std::os::unix::fs::symlink(target, link)
}

fn write_plugin() -> io::Result<PathBuf> {
    let root = store::home().join("plugin");
    let plugin = root.join("adbrent");
    fs::create_dir_all(root.join(".claude-plugin"))?;
    fs::create_dir_all(plugin.join(".claude-plugin"))?;
    fs::create_dir_all(plugin.join("hooks"))?;
    let description = "Shares Android devices between agents: each session rents a device on first adb use and others are refused until it is released.";
    let pretty = |v: serde_json::Value| serde_json::to_string_pretty(&v).unwrap_or_default() + "\n";

    fs::write(
        root.join(".claude-plugin/marketplace.json"),
        pretty(json!({
            "name": "adbrent",
            "description": "adbrent: Android devices shared between AI agents",
            "owner": {"name": "adbrent"},
            "plugins": [{"name": "adbrent", "source": "./adbrent", "description": description}],
        })),
    )?;
    fs::write(
        plugin.join(".claude-plugin/plugin.json"),
        pretty(json!({
            "name": "adbrent",
            "version": env!("CARGO_PKG_VERSION"),
            "description": description,
            "author": {"name": "adbrent"},
            "license": "MIT",
        })),
    )?;
    let hook = |event: &str| json!([{"type": "command", "command": bin(), "args": ["hook", event], "timeout": 10}]);
    fs::write(
        plugin.join("hooks/hooks.json"),
        pretty(json!({"hooks": {
            "SessionStart": [{"hooks": hook("session-start")}],
            "PreToolUse": [{"matcher": "mcp__.*", "hooks": hook("pre-tool-use")}],
            "SessionEnd": [{"hooks": hook("session-end")}],
        }})),
    )?;
    Ok(root)
}

fn claude(args: &[&str]) -> Option<bool> {
    Command::new("claude")
        .args(args)
        .output()
        .ok()
        .map(|o| o.status.success())
}

fn setup() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    fs::create_dir_all(store::home().join("bin"))?;
    if exe.canonicalize().ok() != bin().canonicalize().ok() {
        // Copy then rename, so a running copy is replaced safely.
        let tmp = bin().with_extension("new");
        fs::copy(&exe, &tmp)?;
        fs::rename(&tmp, bin())?;
    }
    let _ = fs::remove_file(shim_dir().join("adb"));
    ensure_shim()?;
    write_plugin()
}

pub fn install(shim_only: bool) -> i32 {
    let plugin = match setup() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("adbrent: install failed: {e}");
            return 1;
        }
    };
    println!("Installed adbrent in {}", store::home().display());
    if !shim_only {
        let path = plugin.to_string_lossy();
        // A marketplace that is already registered just needs refreshing.
        let added = claude(&["plugin", "marketplace", "add", &path]);
        if added == Some(false) {
            claude(&["plugin", "marketplace", "update", "adbrent"]);
        }
        match added.and_then(|_| claude(&["plugin", "install", "adbrent@adbrent"])) {
            Some(true) => println!("Claude Code plugin installed. New sessions share devices automatically."),
            Some(false) => println!(
                "Could not install the Claude Code plugin. Run:\n  claude plugin marketplace add {path}\n  claude plugin install adbrent@adbrent"
            ),
            None => println!("Claude Code not found; skipped its plugin."),
        }
    }
    println!(
        "\nFor any other agent or harness, put the adb stand-in on its PATH:\n  eval \"$({} env)\"",
        bin().display()
    );
    0
}

pub fn uninstall() -> i32 {
    claude(&["plugin", "uninstall", "adbrent@adbrent"]);
    claude(&["plugin", "marketplace", "remove", "adbrent"]);
    match fs::remove_dir_all(store::home()) {
        Ok(()) => println!(
            "Removed {} and the Claude Code plugin.",
            store::home().display()
        ),
        Err(e) if e.kind() == io::ErrorKind::NotFound => println!("Nothing to remove."),
        Err(e) => {
            eprintln!("adbrent: cannot remove {}: {e}", store::home().display());
            return 1;
        }
    }
    0
}
