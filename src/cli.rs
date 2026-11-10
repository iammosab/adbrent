//! The `adbrent` management commands.

use crate::adb::{self, Device};
use crate::desk::{self, Claim};
use crate::store::{self, Lease};
use crate::{hook, install};
use std::time::{Duration, Instant};

const HELP: &str = "\
adbrent - a rental desk for Android devices shared between AI agents

USAGE
  adbrent status [--json]          Devices and who rents each one
  adbrent rent [SERIAL] [--wait[=SECONDS]] [--force]
                                   Rent a device (a free one if none is named)
                                   and print its serial; --wait queues for one;
                                   --force takes a device that looks in use
                                   outside adbrent
  adbrent release [SERIAL...]      Hand back your devices (all of them by default)
  adbrent release --force SERIAL   Take a device back from whoever holds it
  adbrent release --force --all    Clear every rental
  adbrent env                      Shell line that puts the adb stand-in on PATH
  adbrent install [--shim-only]    Set up ~/.adbrent and the Claude Code plugin
  adbrent uninstall                Remove both

Agents do not need these to work: with the stand-in on PATH, plain `adb`
rents a free device on first use and keeps it until the session ends.
";

pub fn run(args: Vec<String>) -> i32 {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("status" | "ls") => status(rest),
        Some("rent") => rent(rest),
        Some("release") => release(rest),
        Some("env") => {
            println!("{}", install::env_lines(None));
            0
        }
        Some("hook") => hook::run(rest.first().map(String::as_str)),
        Some("install") => install::install(rest.iter().any(|a| a == "--shim-only")),
        Some("uninstall") => install::uninstall(),
        Some("version" | "--version" | "-V") => {
            println!("adbrent {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Some("help" | "--help" | "-h") | None => {
            print!("{HELP}");
            0
        }
        Some(other) => {
            eprintln!("adbrent: unknown command `{other}`\n\n{HELP}");
            2
        }
    }
}

fn connected() -> Vec<Device> {
    adb::real_adb()
        .map(|real| adb::devices(&real, &[]))
        .unwrap_or_default()
}

fn status(args: &[String]) -> i32 {
    let _ = store::release(|_| false); // sweep rentals whose owner is gone
    let me = store::me();
    let leases = store::leases();
    let mut rows: Vec<(String, String, String, Option<&Lease>)> = connected()
        .into_iter()
        .map(|d| {
            let lease = leases.iter().find(|l| l.serial == d.serial);
            (d.serial, d.model.replace('_', " "), d.state, lease)
        })
        .collect();
    for l in &leases {
        if !rows.iter().any(|r| r.0 == l.serial) {
            rows.push((l.serial.clone(), String::new(), "absent".into(), Some(l)));
        }
    }

    // Ask each unrented device, all at once, whether something else is using it.
    let outside: Vec<(String, Vec<String>)> = match adb::real_adb() {
        None => Vec::new(),
        Some(real) => std::thread::scope(|scope| {
            let probes: Vec<_> = rows
                .iter()
                .filter(|r| r.3.is_none() && r.2 == "device")
                .map(|r| (r.0.clone(), scope.spawn(|| adb::usage(&real, &r.0).streams)))
                .collect();
            probes
                .into_iter()
                .map(|(serial, probe)| (serial, probe.join().unwrap_or_default()))
                .collect()
        }),
    };
    let outside = |serial: &str| {
        outside
            .iter()
            .find(|o| o.0 == serial)
            .map(|o| o.1.clone())
            .unwrap_or_default()
    };

    if args.iter().any(|a| a == "--json") {
        let json: Vec<_> = rows
            .iter()
            .map(|(serial, model, state, lease)| {
                serde_json::json!({
                    "serial": serial,
                    "model": model,
                    "state": state,
                    "rented_by": lease.map(|l| &l.label),
                    "since": lease.map(|l| l.since),
                    "mine": lease.is_some_and(|l| l.owner == me.owner),
                    "in_use_outside_adbrent": outside(serial),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json).unwrap_or_default()
        );
        return 0;
    }
    if rows.is_empty() {
        println!("No devices connected.");
        return 0;
    }
    let w0 = rows.iter().map(|r| r.0.len()).max().unwrap_or(0).max(6);
    let w1 = rows.iter().map(|r| r.1.len()).max().unwrap_or(0).max(5);
    let w2 = rows.iter().map(|r| r.2.len()).max().unwrap_or(0).max(5);
    println!(
        "{:w0$}  {:w1$}  {:w2$}  RENTED BY",
        "SERIAL", "MODEL", "STATE"
    );
    for (serial, model, state, lease) in &rows {
        let renter = match lease {
            None => match outside(serial) {
                streams if streams.is_empty() => "-".to_string(),
                streams => format!(
                    "nobody, but in use outside adbrent ({})",
                    streams.join(", ")
                ),
            },
            Some(l) if l.owner == me.owner => format!("you ({}, {})", l.label, l.age()),
            Some(l) => format!("{}, {}", l.label, l.age()),
        };
        println!("{serial:w0$}  {model:w1$}  {state:w2$}  {renter}");
    }
    0
}

fn rent(args: &[String]) -> i32 {
    let mut serial = None;
    let mut wait = None;
    let mut force = false;
    for a in args {
        if a == "--wait" {
            wait = Some(u64::MAX);
        } else if let Some(secs) = a.strip_prefix("--wait=") {
            match secs.parse() {
                Ok(s) => wait = Some(s),
                Err(_) => {
                    eprintln!("adbrent: --wait takes a number of seconds");
                    return 2;
                }
            }
        } else if a == "--force" {
            force = true;
        } else if a.starts_with('-') {
            eprintln!("adbrent: unknown option `{a}`");
            return 2;
        } else {
            serial = Some(a.clone());
        }
    }
    let Some(real) = adb::real_adb() else {
        eprintln!("adbrent: cannot find the real adb on PATH (set ADBRENT_REAL_ADB to its path)");
        return 1;
    };
    let me = store::me();
    let started = Instant::now();
    let mut announced = false;
    loop {
        let devs = adb::devices(&real, &[]);
        let outcome = match &serial {
            Some(s) if !devs.iter().any(|d| &d.serial == s) => {
                eprintln!("adbrent: {s} is not connected. `adbrent status` lists devices.");
                return 1;
            }
            Some(s) => match desk::claim(&real, s, &me, &devs, force) {
                Claim::Rented { .. } => Ok(s.clone()),
                Claim::Refused(msg) => Err(msg),
                Claim::Unrecorded(e) => {
                    eprintln!("adbrent: cannot record the rental: {e}");
                    return 1;
                }
            },
            None => {
                // Keep a device this session already holds; otherwise pick a free one.
                let leases = store::leases();
                let held = devs.iter().find(|d| {
                    leases
                        .iter()
                        .any(|l| l.serial == d.serial && l.owner == me.owner)
                });
                let pool: Vec<&Device> = devs.iter().collect();
                match held {
                    Some(d) => Ok(d.serial.clone()),
                    None => desk::pick(&real, &me, &pool, &devs).map(|d| d.serial.clone()),
                }
            }
        };
        let blocker = match outcome {
            Ok(serial) => {
                println!("{serial}");
                return 0;
            }
            Err(msg) => msg,
        };
        let limit = match wait {
            Some(limit) => limit,
            None => {
                eprintln!("{blocker}");
                return 1;
            }
        };
        if started.elapsed().as_secs() >= limit {
            eprintln!(
                "adbrent: gave up waiting after {limit}s. `adbrent status` shows who holds what."
            );
            return 1;
        }
        if !announced {
            eprintln!("adbrent: waiting for a device to become free...");
            announced = true;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn release(args: &[String]) -> i32 {
    let force = args.iter().any(|a| a == "--force");
    let all = args.iter().any(|a| a == "--all");
    let serials: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    if force && !all && serials.is_empty() {
        eprintln!("adbrent: --force needs a serial, or --all to clear every rental");
        return 2;
    }
    let me = store::me();
    let named = |l: &Lease| serials.is_empty() || serials.iter().any(|s| **s == l.serial);
    let result = store::release(|l| named(l) && (force || l.owner == me.owner));
    match result {
        Ok(done) if done.is_empty() => {
            // Naming a device someone else holds is the one case worth an error.
            let others: Vec<String> = serials
                .iter()
                .filter_map(|s| store::held_by_other(s, &me))
                .map(|l| format!("{} ({})", l.serial, l.label))
                .collect();
            if others.is_empty() {
                println!("Nothing to release.");
                0
            } else {
                eprintln!(
                    "adbrent: rented by another agent: {}. Use --force to take it back.",
                    others.join(", ")
                );
                1
            }
        }
        Ok(done) => {
            println!("Released {}.", done.join(", "));
            0
        }
        Err(e) => {
            eprintln!("adbrent: cannot release: {e}");
            1
        }
    }
}
