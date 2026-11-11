//! adbrent: a rental desk for Android devices shared between AI agents.
//!
//! One binary, two faces. Invoked as `adb` (through a symlink placed first on
//! PATH) it checks the rental for the targeted device and then hands off to
//! the real adb. Invoked as `adbrent` it is the management CLI and the hook
//! entry point for agent harnesses.

mod adb;
mod cli;
mod desk;
mod hook;
mod install;
mod on;
mod proc;
mod serve;
mod shim;
mod store;

use std::path::Path;

fn main() {
    let mut args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let arg0 = if args.is_empty() {
        Default::default()
    } else {
        args.remove(0)
    };
    let invoked_as_adb = Path::new(&arg0).file_name().is_some_and(|n| n == "adb");
    if invoked_as_adb {
        shim::run(args);
    }
    let args = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    std::process::exit(cli::run(args));
}
