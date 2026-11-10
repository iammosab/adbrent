//! The `adbrent` management commands.

use crate::adb::{self, Device};
use crate::desk::{self, Claim};
use crate::store::{self, Lease};
use crate::{hook, install, serve};
use std::path::Path;
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
  adbrent share [SERIAL]           Print a code that lets another session use your device
  adbrent join CODE                Use the device shared under CODE alongside its renter
  adbrent exclude [SERIAL]         Put a device off limits to agents (no SERIAL: list;
                                   --remove SERIAL)
  adbrent ignore [TEXT]            Stop counting on-device commands containing TEXT
                                   as outside use (no TEXT: list; --remove TEXT)
  adbrent serve [--port N]         Live status page on http://127.0.0.1:7878
  adbrent log [-n N] [--json]      Recent refusals, forced rentals and feedback
  adbrent feedback [TEXT]          Record a problem or a suggestion (no TEXT: list them)
  adbrent feedback --issue         Print a GitHub issue link filled in from the latest one
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
        Some("share") => share(rest),
        Some("join") => join(rest),
        Some("exclude") => edit_list("exclude", rest, "No device is excluded."),
        Some("ignore") => edit_list("ignore", rest, "Nothing is ignored."),
        Some("serve") => serve::run(rest),
        Some("log") => log(rest),
        Some("feedback") => feedback(rest),
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

/// A device as the user sees it: who holds it, and what else is using it.
#[derive(serde::Serialize)]
pub struct Row {
    pub serial: String,
    pub model: String,
    pub state: String,
    pub rented_by: Option<String>,
    pub since: Option<u64>,
    /// Rented by whoever is asking.
    pub mine: bool,
    /// Shared with whoever is asking by its renter.
    pub guest: bool,
    /// How many other sessions the renter has let in.
    pub shared_with: usize,
    pub in_use_outside_adbrent: Vec<String>,
    /// Put off limits to agents by the user.
    pub excluded: bool,
}

/// Every connected device and every rental. `probe` reports what is using an
/// unrented device outside adbrent; it is asked about all of them at once.
pub fn rows(probe: &(dyn Fn(&Path, &str) -> Vec<String> + Sync)) -> Vec<Row> {
    let _ = store::release(|_| false); // sweep rentals whose owner is gone
    let me = store::me();
    let leases = store::leases();
    let excluded = store::list("exclude");
    let real = adb::real_adb();
    let devices = real
        .as_ref()
        .map(|real| adb::devices(real, &[]))
        .unwrap_or_default();
    let row = |serial: &str, model: String, state: String| {
        let lease: Option<&Lease> = leases.iter().find(|l| l.serial == serial);
        Row {
            serial: serial.to_string(),
            model,
            state,
            rented_by: lease.map(|l| l.label.clone()),
            since: lease.map(|l| l.since),
            mine: lease.is_some_and(|l| l.owner == me.owner),
            guest: lease.is_some_and(|l| l.guests.contains(&me.owner)),
            shared_with: lease.map_or(0, |l| l.guests.len()),
            in_use_outside_adbrent: Vec::new(),
            excluded: excluded.iter().any(|s| s == serial),
        }
    };
    let mut rows: Vec<Row> = devices
        .iter()
        .map(|d| row(&d.serial, d.model.replace('_', " "), d.state.clone()))
        .collect();
    for l in &leases {
        if !rows.iter().any(|r| r.serial == l.serial) {
            rows.push(row(&l.serial, String::new(), "absent".into()));
        }
    }
    if let Some(real) = &real {
        std::thread::scope(|scope| {
            let probes: Vec<_> = rows
                .iter_mut()
                .filter(|r| r.rented_by.is_none() && !r.excluded && r.state == "device")
                .map(|r| {
                    let serial = r.serial.clone();
                    (r, scope.spawn(move || probe(real, &serial)))
                })
                .collect();
            for (r, running) in probes {
                r.in_use_outside_adbrent = running.join().unwrap_or_default();
            }
        });
    }
    rows
}

fn status(args: &[String]) -> i32 {
    let rows = rows(&|real, serial| desk::outside(real, serial).streams);
    if args.iter().any(|a| a == "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).unwrap_or_default()
        );
        return 0;
    }
    if rows.is_empty() {
        println!("No devices connected.");
        return 0;
    }
    let w0 = rows
        .iter()
        .map(|r| r.serial.len())
        .max()
        .unwrap_or(0)
        .max(6);
    let w1 = rows.iter().map(|r| r.model.len()).max().unwrap_or(0).max(5);
    let w2 = rows.iter().map(|r| r.state.len()).max().unwrap_or(0).max(5);
    println!(
        "{:w0$}  {:w1$}  {:w2$}  RENTED BY",
        "SERIAL", "MODEL", "STATE"
    );
    for r in &rows {
        let renter = match (&r.rented_by, r.since) {
            (Some(who), Some(since)) => {
                let held = format!("{who}, {}", age(store::now().saturating_sub(since)));
                let shared = match r.shared_with {
                    0 => String::new(),
                    n => format!(
                        ", shared with {n} other session{}",
                        if n == 1 { "" } else { "s" }
                    ),
                };
                match (r.mine, r.guest) {
                    (true, _) => format!("you ({held}{shared})"),
                    (_, true) => format!("{held}{shared}, you included"),
                    _ => format!("{held}{shared}"),
                }
            }
            _ if r.excluded => "nobody: off limits to agents (excluded)".to_string(),
            _ if !r.in_use_outside_adbrent.is_empty() => {
                format!(
                    "nobody, but in use outside adbrent ({})",
                    r.in_use_outside_adbrent.join("; ")
                )
            }
            _ => "-".to_string(),
        };
        println!(
            "{:w0$}  {:w1$}  {:w2$}  {renter}",
            r.serial, r.model, r.state
        );
    }
    0
}

fn age(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        _ => format!("{}h{}m", secs / 3600, secs % 3600 / 60),
    }
}

fn share(args: &[String]) -> i32 {
    let me = store::me();
    let mine: Vec<String> = store::leases()
        .into_iter()
        .filter(|l| l.owner == me.owner)
        .map(|l| l.serial)
        .collect();
    let serial = match (args.first(), mine.as_slice()) {
        (Some(serial), _) => serial.clone(),
        (None, [only]) => only.clone(),
        (None, []) => {
            eprintln!("adbrent: this session rents no device to share. Rent one first (run `adb`, or `adbrent rent`).");
            return 1;
        }
        (None, _) => {
            eprintln!(
                "adbrent: this session rents {}. Name the one to share: `adbrent share <serial>`.",
                mine.join(", ")
            );
            return 1;
        }
    };
    match store::share(&serial, &me) {
        Ok(Some(code)) => {
            println!("{code}");
            eprintln!("adbrent: a session that runs `adbrent join {code}` can use {serial} alongside this one, for as long as this rental lasts.");
            0
        }
        Ok(None) => {
            eprintln!("adbrent: this session does not rent {serial}, so it cannot share it.");
            1
        }
        Err(e) => {
            eprintln!("adbrent: cannot share {serial}: {e}");
            1
        }
    }
}

fn join(args: &[String]) -> i32 {
    let Some(code) = args.first() else {
        eprintln!("adbrent: join needs the code from `adbrent share`");
        return 2;
    };
    let me = store::me();
    match store::join(code, &me) {
        Ok(Some(serial)) => {
            println!("{serial}");
            eprintln!("adbrent: this session may now use {serial} alongside its renter. Plain `adb` goes to it.");
            0
        }
        Ok(None) => {
            eprintln!(
                "adbrent: no device is shared under the code `{code}` (the rental may have ended)."
            );
            1
        }
        Err(e) => {
            eprintln!("adbrent: cannot join: {e}");
            1
        }
    }
}

fn ago(event: &serde_json::Value) -> String {
    let s = store::now().saturating_sub(event["t"].as_u64().unwrap_or(0));
    match s {
        0..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86_399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

fn describe(event: &serde_json::Value) -> String {
    let text = |key: &str| event[key].as_str().unwrap_or_default();
    let device = if text("serial").is_empty() {
        String::new()
    } else {
        format!(" {}", text("serial"))
    };
    format!(
        "{:>8}  {}{device}  [{}]  {}",
        ago(event),
        text("kind"),
        text("who"),
        text("detail")
    )
}

fn log(args: &[String]) -> i32 {
    let count = args
        .iter()
        .position(|a| a == "-n")
        .and_then(|i| args.get(i + 1))
        .and_then(|n| n.parse().ok())
        .unwrap_or(30);
    let events = store::events();
    let recent = &events[events.len().saturating_sub(count)..];
    if args.iter().any(|a| a == "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(recent).unwrap_or_default()
        );
    } else if recent.is_empty() {
        println!("Nothing logged yet.");
    } else {
        recent.iter().for_each(|e| println!("{}", describe(e)));
    }
    0
}

/// Percent-encode for a URL query value.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn feedback(args: &[String]) -> i32 {
    let events = store::events();
    let notes: Vec<(usize, &serde_json::Value)> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| e["kind"] == "feedback")
        .collect();
    if args.iter().any(|a| a == "--issue") {
        let Some(&(at, note)) = notes.last() else {
            eprintln!("adbrent: no feedback recorded yet");
            return 1;
        };
        // The note, plus what adbrent did just before it. Printed for a person to
        // read and submit; nothing is sent from here.
        let before: Vec<String> = events[at.saturating_sub(5)..at]
            .iter()
            .map(describe)
            .collect();
        let body = format!(
            "{}\n\nadbrent {} on {}\n\nWhat adbrent logged just before:\n```\n{}\n```\n",
            note["detail"].as_str().unwrap_or_default(),
            note["version"].as_str().unwrap_or_default(),
            std::env::consts::OS,
            before.join("\n")
        );
        let title: String = note["detail"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .take(70)
            .collect();
        println!(
            "https://github.com/iammosab/adbrent/issues/new?title={}&body={}",
            encode(&title),
            encode(&body)
        );
        return 0;
    }
    let text = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    if text.trim().is_empty() {
        if notes.is_empty() {
            println!("No feedback recorded yet.");
        }
        notes.iter().for_each(|(_, e)| println!("{}", describe(e)));
        return 0;
    }
    store::log_event("feedback", &store::me(), "", text.trim());
    println!("Recorded. The user can read it with `adbrent feedback`.");
    0
}

/// Show, add to, or remove from one of the user's lists (`ignore`, `exclude`).
fn edit_list(name: &str, args: &[String], empty: &str) -> i32 {
    let remove = args.iter().any(|a| a == "--remove");
    let entry: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let mut list = store::list(name);
    match entry.first() {
        None if remove => {
            eprintln!("adbrent: --remove needs the entry to take off the list");
            return 2;
        }
        None => {
            if list.is_empty() {
                println!("{empty}");
            }
            list.iter().for_each(|e| println!("{e}"));
            return 0;
        }
        Some(entry) if remove => list.retain(|e| e != *entry),
        Some(entry) => {
            if !list.contains(entry) {
                list.push((*entry).clone());
            }
        }
    }
    match store::set_list(name, &list) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("adbrent: cannot save the {name} list: {e}");
            1
        }
    }
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
                let held = devs
                    .iter()
                    .find(|d| leases.iter().any(|l| l.serial == d.serial && l.allows(&me)));
                let pool: Vec<&Device> = devs.iter().collect();
                match held {
                    Some(d) => Ok(d.serial.clone()),
                    None => desk::pick(&real, &me, &pool).map(|d| d.serial.clone()),
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
    if serials.is_empty() && !force {
        let _ = store::leave(&me.owner); // step out of devices shared with this session
    }
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
