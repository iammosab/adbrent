//! The stand-in `adb`: check the rental, then become the real adb.

use crate::adb::{self, Device, Kind};
use crate::desk::{self, Claim};
use crate::store::{self, Me};
use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{exit, Command, Stdio};

fn exec(real: &PathBuf, args: Vec<OsString>) -> ! {
    let err = Command::new(real).args(args).exec();
    eprintln!("adbrent: cannot run {}: {err}", real.display());
    exit(127)
}

fn die(msg: String) -> ! {
    eprintln!("{msg}");
    exit(1)
}

fn rent_or_die(real: &Path, serial: &str, me: &Me, devs: &[Device]) {
    match desk::claim(real, serial, me, devs, false) {
        Claim::Rented { new: true } => announce(
            devs.iter()
                .find(|d| d.serial == serial)
                .map_or(serial.to_string(), Device::name),
        ),
        Claim::Rented { .. } => {}
        Claim::Refused(msg) => die(msg),
        // Never let a state problem take adb away: warn and carry on.
        Claim::Unrecorded(e) => eprintln!("adbrent: warning: rental not recorded ({e})"),
    }
}

fn announce(name: String) {
    eprintln!(
        "adbrent: rented {name} to this session. Run `adbrent release` when you no longer need it."
    );
}

/// `adb devices`, minus the devices other agents hold.
fn list(real: &PathBuf, args: Vec<OsString>, me: &Me) -> ! {
    let out = match Command::new(real)
        .args(&args)
        .stderr(Stdio::inherit())
        .output()
    {
        Ok(o) => o,
        Err(_) => exec(real, args),
    };
    let hidden: Vec<String> = store::leases()
        .into_iter()
        .filter(|l| l.owner != me.owner)
        .map(|l| l.serial)
        .collect();
    let mut dropped = 0;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if line
            .split_whitespace()
            .next()
            .is_some_and(|s| hidden.iter().any(|h| h == s))
        {
            dropped += 1;
        } else {
            println!("{line}");
        }
    }
    if dropped > 0 {
        eprintln!("adbrent: {dropped} device(s) not listed: rented by other agents. `adbrent status` shows them.");
    }
    exit(out.status.code().unwrap_or(1))
}

pub fn run(mut args: Vec<OsString>) -> ! {
    let Some(real) = adb::real_adb() else {
        eprintln!("adbrent: cannot find the real adb on PATH (set ADBRENT_REAL_ADB to its path)");
        exit(127)
    };
    let text: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let p = adb::parse(&text);
    let Some(cmd) = p.cmd.map(|i| text[i].as_str()) else {
        exec(&real, args)
    };

    let me = store::me();
    if cmd == "devices" {
        list(&real, args, &me);
    }
    if cmd == "kill-server" {
        let others = store::leases()
            .iter()
            .filter(|l| l.owner != me.owner)
            .count();
        if others > 0 {
            die(format!("adbrent: kill-server refused: it would disconnect {others} device(s) rented by other agents."));
        }
    }
    // `forward --list` reports on every device and needs none of its own.
    let lists_forwards = cmd == "forward"
        && p.cmd
            .and_then(|i| text.get(i + 1))
            .is_some_and(|a| a == "--list");
    if adb::is_global(cmd) || lists_forwards {
        exec(&real, args);
    }

    let explicit = p.serial.clone().or_else(|| {
        std::env::var("ANDROID_SERIAL")
            .ok()
            .filter(|s| !s.is_empty())
    });
    // The common case: a device this session already holds. No adb round trip.
    if explicit
        .as_deref()
        .is_some_and(|s| store::held_by_me(s, &me))
    {
        exec(&real, args);
    }

    let devs = adb::devices(&real, &p.conn);
    let by_transport = || {
        let t = p.transport.as_deref()?;
        devs.iter()
            .find(|d| d.transport_id == t)
            .map(|d| d.serial.clone())
    };
    if let Some(serial) = explicit.or_else(by_transport) {
        if devs.iter().any(|d| d.serial == serial) {
            rent_or_die(&real, &serial, &me, &devs);
        } else if let Some(holder) = store::held_by_other(&serial, &me) {
            die(desk::refusal(&serial, &holder, &devs));
        }
        exec(&real, args);
    }

    // No device named: give this session its own.
    let pool: Vec<&Device> = devs
        .iter()
        .filter(|d| match p.kind {
            Kind::Any => true,
            Kind::Usb => !d.emulator(),
            Kind::Emulator => d.emulator(),
        })
        .collect();
    let leases = store::leases();
    let mine: Vec<&Device> = pool
        .iter()
        .filter(|d| {
            leases
                .iter()
                .any(|l| l.serial == d.serial && l.owner == me.owner)
        })
        .copied()
        .collect();
    let chosen = match mine.len() {
        1 => mine[0].serial.clone(),
        0 => {
            if !pool.iter().any(|d| d.ready()) {
                exec(&real, args); // nothing usable: let adb report it
            }
            match desk::pick(&real, &me, &pool, &devs) {
                Ok(d) => {
                    announce(d.name());
                    d.serial.clone()
                }
                Err(msg) => die(msg),
            }
        }
        n => die(format!(
            "adbrent: this session holds {n} devices ({}). Name one with `adb -s <serial>`.",
            mine.iter()
                .map(|d| d.serial.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    };
    for i in p.strip.iter().rev() {
        args.remove(*i);
    }
    args.splice(0..0, ["-s".into(), chosen.into()]);
    exec(&real, args)
}
