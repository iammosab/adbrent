//! Talking to, and understanding the command line of, the real adb.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The real adb: `ADBRENT_REAL_ADB`, or the first `adb` on PATH that is not us.
pub fn real_adb() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ADBRENT_REAL_ADB").filter(|p| !p.is_empty()) {
        return Some(p.into());
    }
    let me = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok());
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join("adb"))
        .find(|candidate| {
            candidate
                .canonicalize()
                .is_ok_and(|real| Some(&real) != me.as_ref())
        })
}

#[derive(Debug, Clone)]
pub struct Device {
    pub serial: String,
    pub state: String,
    pub model: String,
    pub transport_id: String,
}

impl Device {
    pub fn ready(&self) -> bool {
        self.state == "device"
    }
    pub fn emulator(&self) -> bool {
        self.serial.starts_with("emulator-")
    }
    pub fn name(&self) -> String {
        if self.model.is_empty() {
            self.serial.clone()
        } else {
            format!("{} ({})", self.serial, self.model.replace('_', " "))
        }
    }
}

pub fn parse_devices(output: &str) -> Vec<Device> {
    output
        .lines()
        .filter(|l| !l.starts_with("List of") && !l.starts_with('*') && !l.trim().is_empty())
        .filter_map(|line| {
            let mut tokens = line.split_whitespace();
            let serial = tokens.next()?.to_string();
            let state = tokens.next()?.to_string();
            let field = |key: &str| {
                line.split_whitespace()
                    .find_map(|t| t.strip_prefix(key))
                    .unwrap_or_default()
                    .to_string()
            };
            Some(Device {
                serial,
                state,
                model: field("model:"),
                transport_id: field("transport_id:"),
            })
        })
        .collect()
}

/// Devices the real adb can see. `conn` carries any -H/-P/-L options so we ask
/// the same adb server the user's command will.
pub fn devices(real: &PathBuf, conn: &[String]) -> Vec<Device> {
    Command::new(real)
        .args(conn)
        .args(["devices", "-l"])
        .output()
        .map(|o| parse_devices(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

/// Signs that a device is being used, whoever is using it.
#[derive(Debug, Default)]
pub struct Usage {
    /// Commands running on the device over adb right now (logcat, sh, ...),
    /// started by any client: another adb, Gradle, an IDE, scrcpy.
    pub streams: Vec<String>,
    /// How long ago the device last saw input, when it reports that.
    pub idle_ms: Option<u64>,
}

/// Runs on the device. Every adb stream is a child of adbd; `$$` is this probe
/// itself and `abb` is adbd's own permanent helper.
const PROBE: &str = "a=$(pidof adbd); ps -A -o PID,PPID,NAME 2>/dev/null | while read p pp n; do \
    [ \"$pp\" = \"$a\" ] && [ \"$p\" != \"$$\" ] && [ \"$n\" != abb ] && echo \"S $n\"; done; \
    dumpsys power 2>/dev/null | grep -m1 'astUserActivityTime=.*ms ago'";

pub fn parse_usage(output: &str) -> Usage {
    let mut usage = Usage::default();
    for line in output.lines() {
        if let Some(name) = line.strip_prefix("S ") {
            usage.streams.push(name.trim().to_string());
        } else if let Some(ago) = line
            .rsplit('(')
            .next()
            .and_then(|t| t.strip_suffix(" ms ago)"))
        {
            usage.idle_ms = ago.trim().parse().ok();
        }
    }
    usage
}

/// Ask the device who is using it. A device that does not answer within a few
/// seconds, or cannot tell, counts as unused. `ADBRENT_NO_PROBE=1` skips this.
pub fn usage(real: &Path, serial: &str) -> Usage {
    if std::env::var_os("ADBRENT_NO_PROBE").is_some_and(|v| !v.is_empty()) {
        return Usage::default();
    }
    let spawned = Command::new(real)
        .args(["-s", serial, "shell", PROBE])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = spawned else {
        return Usage::default();
    };
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Usage::default();
            }
        }
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    parse_usage(&out)
}

#[derive(Debug, Default, PartialEq)]
pub enum Kind {
    #[default]
    Any,
    Usb,
    Emulator,
}

/// What an adb command line targets.
#[derive(Debug, Default)]
pub struct Parsed {
    pub serial: Option<String>,
    pub transport: Option<String>,
    pub kind: Kind,
    /// -H / -P / -L options, to reuse when we query the server ourselves.
    pub conn: Vec<String>,
    /// Indices of -d / -e, dropped when we choose the device for the caller.
    pub strip: Vec<usize>,
    /// Index of the adb command, if there is one.
    pub cmd: Option<usize>,
}

pub fn parse(args: &[String]) -> Parsed {
    let mut p = Parsed::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        // Options that take a value, either attached (-sSERIAL) or as the next argument.
        let valued = ["-s", "-t", "-H", "-P", "-L"]
            .iter()
            .find(|o| a.starts_with(**o))
            .copied();
        if let Some(opt) = valued {
            let (value, next) = if a.len() > 2 {
                (a[2..].to_string(), i + 1)
            } else {
                (args.get(i + 1).cloned().unwrap_or_default(), i + 2)
            };
            match opt {
                "-s" => p.serial = Some(value),
                "-t" => p.transport = Some(value),
                _ => p.conn.extend([opt.to_string(), value]),
            }
            i = next;
            continue;
        }
        match a {
            "-d" => {
                p.kind = Kind::Usb;
                p.strip.push(i);
            }
            "-e" => {
                p.kind = Kind::Emulator;
                p.strip.push(i);
            }
            "-a" | "--exit-on-write-error" => {}
            "--one-device" => i += 1,
            _ => {
                p.cmd = Some(i);
                break;
            }
        }
        i += 1;
    }
    p
}

/// Commands that address the adb server or the host, not a particular device.
pub fn is_global(cmd: &str) -> bool {
    matches!(
        cmd,
        "devices"
            | "help"
            | "--help"
            | "-h"
            | "version"
            | "--version"
            | "start-server"
            | "kill-server"
            | "connect"
            | "disconnect"
            | "pair"
            | "keygen"
            | "host-features"
            | "mdns"
            | "server"
            | "nodaemon"
            | "fork-server"
    ) || cmd.starts_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> (Parsed, Vec<String>) {
        let args: Vec<String> = s.split_whitespace().map(String::from).collect();
        (parse(&args), args)
    }

    #[test]
    fn finds_serial_and_command() {
        let (r, args) = p("-s ABC shell ls -s");
        assert_eq!(r.serial.as_deref(), Some("ABC"));
        assert_eq!(args[r.cmd.unwrap()], "shell");
        let (r, args) = p("-sABC -P 5038 install -r app.apk");
        assert_eq!(r.serial.as_deref(), Some("ABC"));
        assert_eq!(r.conn, ["-P", "5038"]);
        assert_eq!(args[r.cmd.unwrap()], "install");
    }

    #[test]
    fn finds_kind_and_transport() {
        let (r, args) = p("-d logcat");
        assert_eq!(r.kind, Kind::Usb);
        assert_eq!(r.strip, [0]);
        assert_eq!(args[r.cmd.unwrap()], "logcat");
        assert_eq!(p("-t 12 shell").0.transport.as_deref(), Some("12"));
        assert!(p("").0.cmd.is_none());
    }

    #[test]
    fn reads_usage() {
        let u = parse_usage("S logcat\nS sh\n  mLastUserActivityTime=1670085052 (476844 ms ago)\n");
        assert_eq!(u.streams, ["logcat", "sh"]);
        assert_eq!(u.idle_ms, Some(476844));
        let none = parse_usage("REAL -s AAA shell whatever\n");
        assert!(none.streams.is_empty() && none.idle_ms.is_none());
    }

    #[test]
    fn reads_device_list() {
        let out = "List of devices attached\n\
            3A021FDJH000XY         device usb:0-1.3.1 product:bluejay model:Pixel_6a device:bluejay transport_id:10\n\
            emulator-5554\toffline\n\n";
        let d = parse_devices(out);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].name(), "3A021FDJH000XY (Pixel 6a)");
        assert_eq!(d[0].transport_id, "10");
        assert!(d[0].ready() && !d[1].ready() && d[1].emulator());
    }
}
