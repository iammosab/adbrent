//! Claude Code hook entry points. Each reads the hook's JSON on stdin.
//! A hook must never break the session, so every path exits 0.

use crate::desk::{self, Claim};
use crate::store::{self, Session};
use crate::{adb, install, proc};
use serde_json::{json, Value};
use std::io::{Read, Write};

pub fn run(event: Option<&str>) -> i32 {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let v: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let Some(id) = v["session_id"].as_str().filter(|s| !s.is_empty()) else {
        return 0;
    };
    match event {
        Some("session-start") => session_start(id, &v),
        Some("pre-tool-use") => pre_tool_use(id, &v),
        Some("session-end") => session_end(id, &v),
        _ => {}
    }
    0
}

/// Register the session and put the adb stand-in on its PATH. Nothing is added
/// to the session's context: an agent hears from adbrent only when it uses a
/// device, so sessions that never touch Android pay nothing.
fn session_start(id: &str, v: &Value) {
    // Claude Code names its own process to its hooks; otherwise find the agent above us.
    let agent = std::env::var("CLAUDE_PID")
        .ok()
        .and_then(|pid| proc::info(pid.parse().ok()?))
        .map(|p| proc::Proc {
            name: "claude".into(),
            ..p
        })
        .unwrap_or_else(store::anchor);
    let session = Session {
        pid: agent.pid,
        start: agent.start,
        name: agent.name,
        cwd: v["cwd"].as_str().unwrap_or_default().to_string(),
    };
    store::sweep_sessions();
    let _ = store::write_session(id, &session);
    // After /clear the same process carries on under a new session id: keep its devices.
    let _ = store::adopt(&store::me_for_session(id));
    let _ = install::ensure_shim();

    if let Some(path) = std::env::var_os("CLAUDE_ENV_FILE").filter(|p| !p.is_empty()) {
        // SessionStart fires again on resume and after every compaction; write
        // only when this session is not already the one the file names.
        let lines = install::env_lines(Some(id));
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        let named = current
            .lines()
            .rfind(|l| l.starts_with("export ADBRENT_SESSION="));
        if named != lines.lines().next() {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = writeln!(f, "{lines}");
            }
        }
    }
}

/// Device ids named in a tool call: string values under keys such as `udid`,
/// `serial`, or `device_id`.
fn device_args(v: &Value, key: &str, out: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            let k = key.to_ascii_lowercase();
            if ["udid", "serial", "device"].iter().any(|n| k.contains(n)) {
                out.push(s.clone());
            }
        }
        Value::Array(items) => items.iter().for_each(|i| device_args(i, key, out)),
        Value::Object(map) => map.iter().for_each(|(k, val)| device_args(val, k, out)),
        _ => {}
    }
}

/// Tool servers talk to adb themselves, out of the stand-in's reach, so check
/// the device named in the call before it runs.
fn pre_tool_use(id: &str, v: &Value) {
    if !v["tool_name"]
        .as_str()
        .is_some_and(|n| n.starts_with("mcp__"))
    {
        return;
    }
    let mut named = Vec::new();
    device_args(&v["tool_input"], "", &mut named);
    if named.is_empty() {
        return;
    }
    let Some(real) = adb::real_adb() else { return };
    let devs = adb::devices(&real, &[]);
    let me = store::me_for_session(id);
    for d in devs.iter().filter(|d| named.contains(&d.serial)) {
        if let Claim::Refused(reason) = desk::claim(&real, &d.serial, &me, &devs, false) {
            println!(
                "{}",
                json!({"hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": reason,
                }})
            );
            return;
        }
    }
}

fn session_end(id: &str, v: &Value) {
    // On /clear the process lives on; the next session-start adopts its devices.
    if v["reason"].as_str() != Some("clear") {
        let owner = format!("s:{id}");
        let _ = store::release(|l| l.owner == owner);
        let _ = store::leave(&owner);
    }
    store::remove_session(id);
}
