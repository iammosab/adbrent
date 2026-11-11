//! Commands the user wants run when a device is rented or released: waking the
//! screen for an agent, say, or switching it off again afterwards.

use crate::{adb, store};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The longest a rent command may hold up the agent that just got the device.
const LIMIT: Duration = Duration::from_secs(10);

pub fn path(event: &str) -> PathBuf {
    store::home().join(format!("on-{event}"))
}

/// The command set for `event` ("rent" or "release"), if there is one.
pub fn command(event: &str) -> Option<String> {
    std::fs::read_to_string(path(event))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Run the user's command for `event` on `serial`. A rent command finishes
/// (or is stopped after ten seconds) before the agent's own command starts; a
/// release command is left to run on its own.
pub fn run(event: &str, serial: &str, renter: &str) {
    let Some(line) = command(event) else {
        return;
    };
    let failed = |why: String| {
        store::log_as(
            "error",
            renter,
            "",
            serial,
            &format!("the on-{event} command {why}: {line}"),
            None,
        );
    };
    let mut c = Command::new("sh");
    c.arg("-c")
        .arg(&line)
        .env("ADBRENT_EVENT", event)
        .env("ADBRENT_SERIAL", serial)
        .env("ADBRENT_RENTER", renter)
        .env("ANDROID_SERIAL", serial)
        // The command must use the real adb ($ADB): the stand-in would rent
        // the device for it, and refuses to run when it sees this.
        .env(adb::HANDOFF, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(real) = adb::real_adb() {
        c.env("ADB", real);
    }
    let mut child = match c.spawn() {
        Ok(child) => child,
        Err(e) => return failed(format!("could not start ({e})")),
    };
    if event != "rent" {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        return;
    }
    let started = Instant::now();
    let mut nap = Duration::from_millis(1);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return,
            Ok(Some(status)) => return failed(format!("failed ({status})")),
            Ok(None) if started.elapsed() < LIMIT => {
                std::thread::sleep(nap);
                nap = (nap * 2).min(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return failed("was stopped after ten seconds".to_string());
            }
        }
    }
}
