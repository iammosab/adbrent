//! Rental state: one small JSON file per rented device under `~/.adbrent`.
//! There is no daemon. A rental whose owner process is gone is simply ignored
//! and overwritten by the next renter.

use crate::proc;
use serde::{Deserialize, Serialize};
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::{env, fs, io};

pub fn home() -> PathBuf {
    if let Some(h) = env::var_os("ADBRENT_HOME").filter(|h| !h.is_empty()) {
        return h.into();
    }
    PathBuf::from(env::var_os("HOME").unwrap_or_else(|| "/tmp".into())).join(".adbrent")
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Lease {
    pub serial: String,
    pub owner: String,
    pub pid: i32,
    pub start: u64,
    pub label: String,
    pub since: u64,
    /// Other sessions the holder has let in with `adbrent share`.
    #[serde(default)]
    pub guests: Vec<String>,
    /// The code those sessions joined with, once one has been made.
    #[serde(default)]
    pub code: String,
}

impl Lease {
    pub fn alive(&self) -> bool {
        proc::alive(self.pid, self.start)
    }
    /// May `me` use this device: its renter, or a session the renter let in.
    pub fn allows(&self, me: &Me) -> bool {
        self.owner == me.owner || self.guests.contains(&me.owner)
    }
    pub fn age(&self) -> String {
        let s = now().saturating_sub(self.since);
        match s {
            0..=59 => format!("{s}s"),
            60..=3599 => format!("{}m", s / 60),
            _ => format!("{}h{}m", s / 3600, s % 3600 / 60),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Session {
    pub pid: i32,
    pub start: u64,
    pub name: String,
    pub cwd: String,
}

/// The identity of whoever is running this command.
pub struct Me {
    pub owner: String,
    pub pid: i32,
    pub start: u64,
    pub label: String,
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn safe(name: &str) -> String {
    name.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'_' | b'-' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn lease_path(serial: &str) -> PathBuf {
    home().join("leases").join(format!("{}.json", safe(serial)))
}

fn session_path(id: &str) -> PathBuf {
    home().join("sessions").join(format!("{}.json", safe(id)))
}

fn write_json<T: Serialize>(path: &PathBuf, value: &T) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    fs::write(&tmp, serde_json::to_vec(value)?)?;
    fs::rename(&tmp, path)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &PathBuf) -> Option<T> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

/// Exclusive lock over all rental changes; released when dropped.
pub struct Lock(#[allow(dead_code)] fs::File);

pub fn lock() -> io::Result<Lock> {
    fs::create_dir_all(home())?;
    let f = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(home().join("lock"))?;
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Lock(f))
}

pub fn read_lease(serial: &str) -> Option<Lease> {
    read_json(&lease_path(serial))
}

fn all() -> Vec<Lease> {
    let Ok(dir) = fs::read_dir(home().join("leases")) else {
        return Vec::new();
    };
    dir.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| read_json::<Lease>(&e.path()))
        .collect()
}

/// All rentals whose owner is still alive.
pub fn leases() -> Vec<Lease> {
    all().into_iter().filter(Lease::alive).collect()
}

/// Move rentals this same process took under an earlier session id over to `me`.
pub fn adopt(me: &Me) -> io::Result<()> {
    let _lock = lock()?;
    for mut l in all() {
        if l.pid == me.pid && l.start == me.start && l.owner != me.owner {
            l.owner = me.owner.clone();
            l.label = me.label.clone();
            write_json(&lease_path(&l.serial), &l)?;
        }
    }
    Ok(())
}

/// The live rental on `serial` held by someone other than `me`, if any.
pub fn held_by_other(serial: &str, me: &Me) -> Option<Lease> {
    read_lease(serial).filter(|l| !l.allows(me) && l.alive())
}

/// True when `me` already holds `serial` and nothing needs writing.
pub fn held_by_me(serial: &str, me: &Me) -> bool {
    read_lease(serial).is_some_and(|l| l.allows(me) && l.alive())
}

/// Rent `serial` for `me`. `Ok(Ok(true))` is a new rental, `Ok(Ok(false))` one
/// already held, `Ok(Err(lease))` means another live owner holds it.
pub fn acquire(serial: &str, me: &Me) -> io::Result<Result<bool, Lease>> {
    let _lock = lock()?;
    let mut lease = Lease {
        serial: serial.to_string(),
        owner: me.owner.clone(),
        pid: me.pid,
        start: me.start,
        label: me.label.clone(),
        since: now(),
        guests: Vec::new(),
        code: String::new(),
    };
    let mut new = true;
    if let Some(l) = read_lease(serial) {
        if l.alive() {
            // Held under a live process. Ours (as renter or guest): leave it alone,
            // whichever of the session's processes is asking. Otherwise: refused.
            return Ok(if l.allows(me) { Ok(false) } else { Err(l) });
        }
        if l.owner == me.owner {
            // Same session under a new process (a resumed session): keep the rental.
            lease.since = l.since;
            lease.guests = l.guests;
            lease.code = l.code;
            new = false;
        } else {
            record_span(&l);
            log_as(
                "released",
                &l.label,
                &l.owner,
                serial,
                "its session was gone",
                Some(now().saturating_sub(l.since)),
            );
        }
    }
    write_json(&lease_path(serial), &lease)?;
    Ok(Ok(new))
}

/// Make a code that lets other sessions use `serial` alongside `me`, its renter.
pub fn share(serial: &str, me: &Me) -> io::Result<Option<String>> {
    let _lock = lock()?;
    let Some(mut l) = read_lease(serial).filter(|l| l.owner == me.owner && l.alive()) else {
        return Ok(None);
    };
    if l.code.is_empty() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        l.code = format!(
            "{:06x}",
            (nanos ^ std::process::id().wrapping_mul(2_654_435_761)) & 0xff_ffff
        );
        write_json(&lease_path(serial), &l)?;
    }
    Ok(Some(l.code))
}

/// Let `me` in on the device whose renter made `code`. Returns its serial.
pub fn join(code: &str, me: &Me) -> io::Result<Option<String>> {
    let _lock = lock()?;
    let Some(mut l) = all()
        .into_iter()
        .find(|l| !l.code.is_empty() && l.code == code && l.alive())
    else {
        return Ok(None);
    };
    if !l.allows(me) {
        l.guests.push(me.owner.clone());
        write_json(&lease_path(&l.serial), &l)?;
    }
    Ok(Some(l.serial))
}

/// Take `owner` off every device it was let in on.
pub fn leave(owner: &str) -> io::Result<()> {
    let _lock = lock()?;
    for mut l in all() {
        if l.guests.iter().any(|g| g == owner) {
            l.guests.retain(|g| g != owner);
            write_json(&lease_path(&l.serial), &l)?;
        }
    }
    Ok(())
}

/// Remove rentals matching `pick`, plus any whose owner has died. Returns the
/// serials that `pick` released. `why` goes in the log beside each of them.
pub fn release(pick: impl Fn(&Lease) -> bool, why: Option<&str>) -> io::Result<Vec<String>> {
    let _lock = lock()?;
    let mut released = Vec::new();
    let Ok(dir) = fs::read_dir(home().join("leases")) else {
        return Ok(released);
    };
    let ended = |lease: &Lease, why: &str| {
        record_span(lease);
        log_as(
            "released",
            &lease.label,
            &lease.owner,
            &lease.serial,
            why,
            Some(now().saturating_sub(lease.since)),
        );
    };
    for entry in dir.flatten() {
        let path = entry.path();
        let Some(lease) = read_json::<Lease>(&path) else {
            continue;
        };
        if pick(&lease) {
            fs::remove_file(&path)?;
            match why {
                Some(why) => ended(&lease, why),
                None => record_span(&lease),
            }
            released.push(lease.serial);
        } else if !lease.alive() {
            let _ = fs::remove_file(&path);
            ended(&lease, "its session was gone");
        }
    }
    Ok(released)
}

fn span_path(serial: &str) -> PathBuf {
    home()
        .join("history")
        .join(format!("{}.json", safe(serial)))
}

/// Note that `lease` has ended, so that whatever its renter left running on
/// the device is later recognised as a leftover and not as someone else's use.
fn record_span(lease: &Lease) {
    let path = span_path(&lease.serial);
    let mut spans: Vec<(u64, u64)> = read_json(&path).unwrap_or_default();
    spans.push((lease.since, now()));
    let keep = spans.len().saturating_sub(20);
    let _ = write_json(&path, &spans[keep..].to_vec());
}

/// The periods (start, end) during which `serial` was rented: past rentals,
/// and the present one if there is one.
pub fn spans(serial: &str) -> Vec<(u64, u64)> {
    let mut spans: Vec<(u64, u64)> = read_json(&span_path(serial)).unwrap_or_default();
    if let Some(l) = read_lease(serial) {
        spans.push((l.since, now()));
    }
    spans
}

/// A user-kept list, one entry per line: `ignore` (pieces of text that mark
/// an on-device command as a helper to disregard) or `exclude` (serials of
/// devices agents must leave alone).
pub fn list(name: &str) -> Vec<String> {
    fs::read_to_string(home().join(name))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

pub fn set_list(name: &str, entries: &[String]) -> io::Result<()> {
    fs::create_dir_all(home())?;
    fs::write(
        home().join(name),
        entries.iter().map(|e| format!("{e}\n")).collect::<String>(),
    )
}

pub fn write_session(id: &str, session: &Session) -> io::Result<()> {
    write_json(&session_path(id), session)
}

/// Forget sessions whose process is gone (a crashed session never says goodbye).
pub fn sweep_sessions() {
    let Ok(dir) = fs::read_dir(home().join("sessions")) else {
        return;
    };
    for entry in dir.flatten() {
        let path = entry.path();
        if read_json::<Session>(&path).is_none_or(|s| !proc::alive(s.pid, s.start)) {
            let _ = fs::remove_file(path);
        }
    }
}

pub fn remove_session(id: &str) {
    let _ = fs::remove_file(session_path(id));
}

pub fn me() -> Me {
    // A session id travels in the environment, so it also reaches background
    // jobs that have been cut loose from the agent's process tree. Ours is set
    // by the plugin; Claude Code sets its own in every command it runs.
    let named = |var: &str| env::var(var).ok().filter(|s| !s.is_empty());
    match named("ADBRENT_SESSION").or_else(|| named("CLAUDE_CODE_SESSION_ID")) {
        Some(id) => me_for_session(&id),
        None => {
            let a = anchor();
            Me {
                owner: format!("p:{}:{}", a.pid, a.start),
                pid: a.pid,
                start: a.start,
                label: format!("{} (pid {})", a.name, a.pid),
            }
        }
    }
}

/// Identity for a harness session that registered itself (or was named through
/// `ADBRENT_SESSION`). The rental follows the session id, and the registered
/// process tells us when the session is gone.
pub fn me_for_session(id: &str) -> Me {
    let short: String = id.chars().take(8).collect();
    let owner = format!("s:{id}");
    if let Some(s) = read_json::<Session>(&session_path(id)).filter(|s| proc::alive(s.pid, s.start))
    {
        let dir = s.cwd.rsplit('/').next().unwrap_or_default();
        let place = if dir.is_empty() {
            String::new()
        } else {
            format!(" in {dir}")
        };
        return Me {
            owner,
            pid: s.pid,
            start: s.start,
            label: format!("{} session {short}{place}", s.name),
        };
    }
    // Claude Code names its own process to the commands it runs.
    let claude = env::var("CLAUDE_PID")
        .ok()
        .and_then(|p| proc::info(p.parse().ok()?));
    if let Some(p) = claude {
        return Me {
            owner,
            pid: p.pid,
            start: p.start,
            label: format!("claude session {short}"),
        };
    }
    let a = anchor();
    Me {
        owner,
        pid: a.pid,
        start: a.start,
        label: format!("{} session {short}", a.name),
    }
}

/// Is the trace on? With it, routine activity (rentals, releases, waits) and
/// every adb and adbrent command are logged too, for studying how agents
/// behave. Off unless someone turns it on with `adbrent debug on`.
pub fn debug() -> bool {
    env::var_os("ADBRENT_DEBUG").is_some_and(|v| !v.is_empty()) || home().join("debug").exists()
}

/// Turn the trace on or off for every session on this machine.
pub fn set_debug(on: bool) -> io::Result<()> {
    let flag = home().join("debug");
    if on {
        fs::create_dir_all(home())?;
        fs::write(flag, "")
    } else {
        match fs::remove_file(flag) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// Note something that happened: a rental, a refusal, a wait, an agent's
/// feedback. Kept on this machine only; never allowed to fail a command.
pub fn log_event(kind: &str, me: &Me, serial: &str, detail: &str) {
    log_as(kind, &me.label, &me.owner, serial, detail, None);
}

/// The same, with how long something took or lasted, in seconds.
pub fn log_timed(kind: &str, me: &Me, serial: &str, detail: &str, secs: u64) {
    log_as(kind, &me.label, &me.owner, serial, detail, Some(secs));
}

/// What is logged only with the trace on (`adbrent debug on`).
const ROUTINE: [&str; 11] = [
    "rented",
    "released",
    "waiting",
    "wait ended",
    "shared",
    "joined",
    "adb",
    "adbrent",
    "tool call",
    "session started",
    "session ended",
];

/// Set while a command is waiting for a device: each retry would otherwise be
/// logged as one more refusal.
pub static WAITING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn log_as(kind: &str, who: &str, owner: &str, serial: &str, detail: &str, secs: Option<u64>) {
    use std::io::Write;
    if kind.starts_with("refused") && WAITING.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    // A normal install records only problems. How agents use the desk day to
    // day is recorded while someone is studying it, with the trace on.
    if ROUTINE.contains(&kind) && !debug() {
        return;
    }
    let path = home().join("log.jsonl");
    let line = serde_json::json!({
        "t": now(), "kind": kind, "who": who, "owner": owner, "serial": serial,
        "detail": detail, "secs": secs, "version": env!("CARGO_PKG_VERSION"),
    });
    // Keep the log small: past 4 MB, drop the older half.
    if fs::metadata(&path).is_ok_and(|m| m.len() > 4_000_000) {
        if let Ok(text) = fs::read_to_string(&path) {
            let lines: Vec<&str> = text.lines().collect();
            let _ = fs::write(&path, lines[lines.len() / 2..].join("\n") + "\n");
        }
    }
    let _ = fs::create_dir_all(home());
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

pub fn events() -> Vec<serde_json::Value> {
    fs::read_to_string(home().join("log.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

pub fn anchor() -> proc::Proc {
    proc::anchor()
        .or_else(|| proc::info(std::process::id() as i32))
        .unwrap_or(proc::Proc {
            pid: std::process::id() as i32,
            ppid: 0,
            name: "unknown".into(),
            start: 0,
        })
}
