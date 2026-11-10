//! Deciding who may use a device: rentals first, then signs that something
//! outside adbrent is using it.

use crate::adb::{self, Device};
use crate::store::{self, Lease, Me};
use std::path::Path;

/// A device touched more recently than this is handed out last.
const RECENT_MS: u64 = 120_000;

fn name(serial: &str, devs: &[Device]) -> String {
    devs.iter()
        .find(|d| d.serial == serial)
        .map_or(serial.to_string(), Device::name)
}

/// What an agent with nothing free should do. The wait is meant to run in the
/// background, so the agent is told when it has a device instead of blocking.
const WAIT: &str = "adbrent: run `adbrent rent --wait` as a background command: it returns holding a device as soon as one is released.";

/// One line telling an agent where to go instead.
pub fn alternatives(devs: &[Device]) -> String {
    let mut taken: Vec<String> = store::leases().into_iter().map(|l| l.serial).collect();
    taken.extend(store::list("exclude"));
    let free: Vec<String> = devs
        .iter()
        .filter(|d| d.ready() && !taken.contains(&d.serial))
        .map(Device::name)
        .collect();
    if free.is_empty() {
        format!("adbrent: no device is free.\n{WAIT}")
    } else {
        format!(
            "adbrent: free now: {}. Use plain `adb`, or `adb -s <serial>`.",
            free.join(", ")
        )
    }
}

pub fn refusal(serial: &str, holder: &Lease, devs: &[Device]) -> String {
    format!(
        "adbrent: {} is already rented by another agent ({}, for {}). Do not use it.\n{}\nadbrent: working for that agent? Get a code from its `adbrent share` and run `adbrent join <code>`.",
        name(serial, devs),
        holder.label,
        holder.age(),
        alternatives(devs)
    )
}

fn in_use(serial: &str, streams: &[String], devs: &[Device]) -> String {
    // The device being refused is not an alternative to itself.
    let others: Vec<Device> = devs
        .iter()
        .filter(|d| d.serial != serial)
        .cloned()
        .collect();
    format!(
        "adbrent: {} is not rented, but something outside adbrent is using it right now (on the device: {}). Do not use it.\n{}\nadbrent: if that is wrong, `adbrent rent --force {serial}` takes it; tell the user, or record it with `adbrent feedback \"<what happened>\"`.",
        name(serial, devs),
        streams.join("; "),
        alternatives(&others)
    )
}

/// What is using `serial` that adbrent does not account for.
pub struct Outside {
    /// Commands running on the device that no rental explains.
    pub streams: Vec<String>,
    pub idle_ms: Option<u64>,
    /// Not a phone or tablet (a TV, a watch, a car unit): handed out last.
    pub special: bool,
}

/// Ask the device what is running on it, and set aside what adbrent can
/// explain: commands started while the device was rented (a renter's tools
/// often leave helpers running after the rental ends) and commands the user
/// has chosen to ignore.
pub fn outside(real: &Path, serial: &str) -> Outside {
    let usage = adb::usage(real, serial);
    let ignored = store::list("ignore");
    let spans = store::spans(serial);
    let now = store::now();
    let streams = usage
        .streams
        .iter()
        .filter(|s| !ignored.iter().any(|text| s.args.contains(text.as_str())))
        .filter(|s| {
            let started = now.saturating_sub(s.age);
            // A few seconds of slack: a tool may start its helper just before the rental is written.
            !spans
                .iter()
                .any(|&(from, to)| started + 5 >= from && started <= to)
        })
        .map(adb::Stream::label)
        .collect();
    let special = usage
        .characteristics
        .split(',')
        .any(|kind| matches!(kind.trim(), "tv" | "watch" | "automotive" | "embedded"));
    Outside {
        streams,
        idle_ms: usage.idle_ms,
        special,
    }
}

pub enum Claim {
    Rented {
        new: bool,
    },
    Refused(String),
    /// The device is ours to use, but the rental could not be written down.
    Unrecorded(std::io::Error),
}

/// Why a command is stopped when its rental cannot be written down. Running it
/// anyway would leave the device looking free to every other agent.
pub fn unrecorded(e: &std::io::Error) -> String {
    format!(
        "adbrent: cannot record a rental ({e}), so the command was not run: without a record, another agent could take the same device.\nadbrent: inside Claude Code's sandbox, allow writes to {} (setting `sandbox.filesystem.allowWrite`). Tell the user. `ADBRENT_ALLOW_UNRECORDED=1` runs commands without a record.",
        store::home().display()
    )
}

/// Rent `serial` for `me`, unless another agent holds it, the user has put it
/// off limits, or (without `force`) it is visibly in use outside adbrent.
pub fn claim(real: &Path, serial: &str, me: &Me, devs: &[Device], force: bool) -> Claim {
    let refuse_held = |holder: &Lease| {
        store::log_event(
            "refused: rented",
            me,
            serial,
            &format!("held by {}", holder.label),
        );
        Claim::Refused(refusal(serial, holder, devs))
    };
    match store::read_lease(serial) {
        Some(l) if l.alive() && l.allows(me) => {}
        Some(l) if l.alive() => return refuse_held(&l),
        _ if force => store::log_event(
            "forced",
            me,
            serial,
            "took the device without the usual checks",
        ),
        _ if store::list("exclude").iter().any(|s| s == serial) => {
            store::log_event("refused: excluded", me, serial, "");
            return Claim::Refused(format!(
                "adbrent: {} is off limits: the user has excluded it from agent use. Do not use it.\n{}",
                name(serial, devs),
                alternatives(devs)
            ));
        }
        _ => {
            let streams = outside(real, serial).streams;
            if !streams.is_empty() {
                store::log_event("refused: outside use", me, serial, &streams.join("; "));
                return Claim::Refused(in_use(serial, &streams, devs));
            }
        }
    }
    match store::acquire(serial, me) {
        Ok(Ok(new)) => Claim::Rented { new },
        Ok(Err(holder)) => refuse_held(&holder),
        Err(e) => Claim::Unrecorded(e),
    }
}

/// Rent a free device from `pool` for `me`: one nobody is using, preferably a
/// phone, and preferably one nobody has touched lately.
pub fn pick<'a>(real: &Path, me: &Me, pool: &[&'a Device]) -> Result<&'a Device, String> {
    let leases = store::leases();
    let excluded = store::list("exclude");
    let free = pool.iter().copied().filter(|d| {
        d.ready() && !excluded.contains(&d.serial) && !leases.iter().any(|l| l.serial == d.serial)
    });
    let mut later = Vec::new(); // (how reluctantly, device)
    let mut busy = Vec::new();
    let take = |d: &'a Device| match store::acquire(&d.serial, me) {
        Ok(Ok(_)) => Ok(Some(d)),
        Ok(Err(_)) => Ok(None), // someone else got there first
        Err(e) => Err(unrecorded(&e)),
    };
    for d in free {
        let usage = outside(real, &d.serial);
        if !usage.streams.is_empty() {
            busy.push(format!("{} ({})", d.name(), usage.streams.join("; ")));
        } else if usage.special {
            later.push((2, d));
        } else if usage.idle_ms.is_some_and(|ms| ms < RECENT_MS) {
            later.push((1, d));
        } else if let Some(d) = take(d)? {
            return Ok(d);
        }
    }
    later.sort_by_key(|&(reluctance, _)| reluctance);
    for (_, d) in later {
        if let Some(d) = take(d)? {
            return Ok(d);
        }
    }
    store::log_event("refused: none free", me, "", &busy.join(", "));
    if busy.is_empty() {
        return Err(format!(
            "adbrent: every connected device is rented by another agent or off limits.\n{WAIT}"
        ));
    }
    Err(format!(
        "adbrent: no device is available. In use outside adbrent right now: {}; the rest are rented by other agents or off limits.\n{WAIT}",
        busy.join(", ")
    ))
}
