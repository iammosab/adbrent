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
}

impl Lease {
    pub fn alive(&self) -> bool {
        proc::alive(self.pid, self.start)
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
    read_lease(serial).filter(|l| l.owner != me.owner && l.alive())
}

/// True when `me` already holds `serial` and nothing needs writing.
pub fn held_by_me(serial: &str, me: &Me) -> bool {
    read_lease(serial)
        .is_some_and(|l| l.owner == me.owner && l.pid == me.pid && l.start == me.start)
}

/// Rent `serial` for `me`. `Ok(Ok(true))` is a new rental, `Ok(Ok(false))` one
/// already held, `Ok(Err(lease))` means another live owner holds it.
pub fn acquire(serial: &str, me: &Me) -> io::Result<Result<bool, Lease>> {
    let _lock = lock()?;
    let mut since = now();
    let mut new = true;
    if let Some(l) = read_lease(serial) {
        if l.owner == me.owner {
            if l.pid == me.pid && l.start == me.start {
                return Ok(Ok(false));
            }
            // Same session under a new process (a resumed session): keep the rental.
            since = l.since;
            new = false;
        } else if l.alive() {
            return Ok(Err(l));
        }
    }
    let lease = Lease {
        serial: serial.to_string(),
        owner: me.owner.clone(),
        pid: me.pid,
        start: me.start,
        label: me.label.clone(),
        since,
    };
    write_json(&lease_path(serial), &lease)?;
    Ok(Ok(new))
}

/// Remove rentals matching `pick`, plus any whose owner has died. Returns the
/// serials that `pick` released.
pub fn release(pick: impl Fn(&Lease) -> bool) -> io::Result<Vec<String>> {
    let _lock = lock()?;
    let mut released = Vec::new();
    let Ok(dir) = fs::read_dir(home().join("leases")) else {
        return Ok(released);
    };
    for entry in dir.flatten() {
        let path = entry.path();
        let Some(lease) = read_json::<Lease>(&path) else {
            continue;
        };
        if pick(&lease) {
            fs::remove_file(&path)?;
            released.push(lease.serial);
        } else if !lease.alive() {
            let _ = fs::remove_file(&path);
        }
    }
    Ok(released)
}

pub fn write_session(id: &str, session: &Session) -> io::Result<()> {
    write_json(&session_path(id), session)
}

pub fn remove_session(id: &str) {
    let _ = fs::remove_file(session_path(id));
}

pub fn me() -> Me {
    match env::var("ADBRENT_SESSION").ok().filter(|s| !s.is_empty()) {
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
    let a = anchor();
    Me {
        owner,
        pid: a.pid,
        start: a.start,
        label: format!("{} session {short}", a.name),
    }
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
