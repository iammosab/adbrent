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

/// One line telling an agent where to go instead.
pub fn alternatives(devs: &[Device]) -> String {
    let rented: Vec<String> = store::leases().into_iter().map(|l| l.serial).collect();
    let free: Vec<String> = devs
        .iter()
        .filter(|d| d.ready() && !rented.contains(&d.serial))
        .map(Device::name)
        .collect();
    if free.is_empty() {
        "adbrent: no device is free right now. `adbrent rent --wait` queues for the next one; `adbrent status` shows who holds what.".into()
    } else {
        format!(
            "adbrent: free now: {}. Use plain `adb` or `adb -s <serial>` to rent one.",
            free.join(", ")
        )
    }
}

pub fn refusal(serial: &str, holder: &Lease, devs: &[Device]) -> String {
    format!(
        "adbrent: {} is already rented by another agent ({}, for {}). Do not use it.\n{}",
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
        "adbrent: {} is not rented, but something outside adbrent is using it right now (running on the device: {}). Do not use it.\n{}\nadbrent: if that is a helper that is fine to leave running, `adbrent ignore <part of its name>` stops counting it. `adbrent rent --force {serial}` takes the device anyway.\nadbrent: if this refusal was wrong, tell the user and record it: `adbrent feedback \"<what happened>\"`.",
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
}

/// Ask the device what is running on it, and set aside what adbrent can
/// explain: commands started while the device was rented (a renter's tools
/// often leave helpers running after the rental ends) and commands the user
/// has chosen to ignore.
pub fn outside(real: &Path, serial: &str) -> Outside {
    let usage = adb::usage(real, serial);
    let ignored = store::ignored();
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
    Outside {
        streams,
        idle_ms: usage.idle_ms,
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

/// Rent `serial` for `me`, unless another agent holds it or (without `force`)
/// it is visibly in use outside adbrent.
pub fn claim(real: &Path, serial: &str, me: &Me, devs: &[Device], force: bool) -> Claim {
    match store::read_lease(serial) {
        Some(l) if l.owner == me.owner => {}
        Some(l) if l.alive() => {
            store::log_event(
                "refused: rented",
                me,
                serial,
                &format!("held by {}", l.label),
            );
            return Claim::Refused(refusal(serial, &l, devs));
        }
        _ if force => store::log_event(
            "forced",
            me,
            serial,
            "took the device without checking for outside use",
        ),
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
        Ok(Err(holder)) => {
            store::log_event(
                "refused: rented",
                me,
                serial,
                &format!("held by {}", holder.label),
            );
            Claim::Refused(refusal(serial, &holder, devs))
        }
        Err(e) => Claim::Unrecorded(e),
    }
}

/// Rent a free device from `pool` for `me`: one nobody is using, and
/// preferably one nobody has touched lately.
pub fn pick<'a>(
    real: &Path,
    me: &Me,
    pool: &[&'a Device],
    devs: &[Device],
) -> Result<&'a Device, String> {
    let leases = store::leases();
    let free = pool
        .iter()
        .copied()
        .filter(|d| d.ready() && !leases.iter().any(|l| l.serial == d.serial));
    let mut warm = Vec::new();
    let mut busy = Vec::new();
    let take = |d: &'a Device| match store::acquire(&d.serial, me) {
        Ok(Ok(_)) => Ok(Some(d)),
        Ok(Err(_)) => Ok(None), // someone else got there first
        Err(e) => Err(format!("adbrent: cannot record a rental: {e}")),
    };
    for d in free {
        let usage = outside(real, &d.serial);
        if !usage.streams.is_empty() {
            busy.push(format!("{} ({})", d.name(), usage.streams.join("; ")));
        } else if usage.idle_ms.is_some_and(|ms| ms < RECENT_MS) {
            warm.push(d);
        } else if let Some(d) = take(d)? {
            return Ok(d);
        }
    }
    for d in warm {
        if let Some(d) = take(d)? {
            return Ok(d);
        }
    }
    store::log_event("refused: none free", me, "", &busy.join(", "));
    if busy.is_empty() {
        return Err(format!(
            "adbrent: every connected device is already rented by another agent.\n{}",
            alternatives(devs)
        ));
    }
    Err(format!(
        "adbrent: no device is available. In use outside adbrent right now: {}; the rest are rented by other agents.\nadbrent: `adbrent rent --wait` queues for the next one; `adbrent status` shows who holds what.",
        busy.join(", ")
    ))
}
