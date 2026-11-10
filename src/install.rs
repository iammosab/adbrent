//! Setup: a stable copy of the binary, the `adb` stand-in, and the Claude Code plugin.

use crate::store;
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

fn claude(args: &[&str]) -> Option<bool> {
    Command::new("claude")
        .args(args)
        .output()
        .ok()
        .map(|o| o.status.success())
}

/// Where Claude Code gets the plugin: this project's repository, which is
/// also a plugin marketplace. `ADBRENT_MARKETPLACE` points it elsewhere, such
/// as at a local checkout.
fn marketplace() -> String {
    std::env::var("ADBRENT_MARKETPLACE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "iammosab/adbrent".into())
}

fn setup() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    fs::create_dir_all(store::home().join("bin"))?;
    if exe.canonicalize().ok() != bin().canonicalize().ok() {
        // Copy then rename, so a running copy is replaced safely.
        let tmp = bin().with_extension("new");
        fs::copy(&exe, &tmp)?;
        fs::rename(&tmp, bin())?;
    }
    let _ = fs::remove_file(shim_dir().join("adb"));
    ensure_shim()
}

pub fn install(shim_only: bool) -> i32 {
    if let Err(e) = setup() {
        eprintln!("adbrent: install failed: {e}");
        return 1;
    }
    println!("Installed adbrent in {}", store::home().display());
    if !shim_only {
        let source = marketplace();
        // An "adbrent" marketplace may already be registered, possibly an older
        // one from another place: replace it, so the plugin comes from `source`.
        let mut added = claude(&["plugin", "marketplace", "add", &source]);
        if added == Some(false) {
            claude(&["plugin", "marketplace", "remove", "adbrent"]);
            added = claude(&["plugin", "marketplace", "add", &source]);
        }
        let installed = added.and_then(|_| claude(&["plugin", "install", "adbrent@adbrent"]));
        // Installing is a no-op when an older copy of the plugin is already there.
        claude(&["plugin", "update", "adbrent@adbrent"]);
        match installed {
            Some(true) => println!("Claude Code plugin installed. New sessions share devices automatically."),
            Some(false) => println!(
                "Could not install the Claude Code plugin. Inside Claude Code, run:\n  /plugin marketplace add {source}\n  /plugin install adbrent@adbrent"
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
