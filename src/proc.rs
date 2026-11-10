//! Process inspection: who is asking, and is a renter still alive.

#[derive(Clone, Debug)]
pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub name: String,
    /// Start time, used with the pid so a recycled pid is not mistaken for the renter.
    pub start: u64,
}

#[cfg(target_os = "macos")]
pub fn info(pid: i32) -> Option<Proc> {
    let mut bi: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut bi as *mut _ as *mut libc::c_void,
            size,
        )
    };
    if n != size {
        return None;
    }
    let cstr = |b: &[libc::c_char]| {
        let bytes: Vec<u8> = b
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    // pbi_comm is the command name (`claude`); pbi_name can be the binary's
    // file name, which for a versioned install is just a version number.
    let mut name = cstr(&bi.pbi_comm);
    if name.is_empty() {
        name = cstr(&bi.pbi_name);
    }
    Some(Proc {
        pid,
        ppid: bi.pbi_ppid as i32,
        name,
        start: bi
            .pbi_start_tvsec
            .wrapping_mul(1_000_000)
            .wrapping_add(bi.pbi_start_tvusec),
    })
}

#[cfg(target_os = "linux")]
pub fn info(pid: i32) -> Option<Proc> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat.get(open + 1..close)?.to_string();
    // After the name: state ppid pgrp ... starttime is the 20th field here.
    let rest: Vec<&str> = stat.get(close + 1..)?.split_whitespace().collect();
    Some(Proc {
        pid,
        ppid: rest.get(1)?.parse().ok()?,
        name,
        start: rest.get(19)?.parse().ok()?,
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn info(_pid: i32) -> Option<Proc> {
    None
}

pub fn alive(pid: i32, start: u64) -> bool {
    info(pid).is_some_and(|p| p.start == start)
}

/// Agent harnesses we recognise by process name. Anything else falls back to
/// the nearest ancestor that is not a shell or a command wrapper.
const AGENTS: &[&str] = &[
    "claude",
    "codex",
    "opencode",
    "gemini",
    "aider",
    "goose",
    "amp",
    "cursor-agent",
    "crush",
    "droid",
    "qwen",
    "grok",
    "copilot",
];
const WRAPPERS: &[&str] = &[
    "sh", "bash", "zsh", "fish", "dash", "ksh", "csh", "tcsh", "env", "sudo", "nohup", "timeout",
    "time", "xargs", "adb", "adbrent",
];

/// Path of a process's executable, when the OS will tell us.
#[cfg(target_os = "macos")]
fn exe(pid: i32) -> Option<String> {
    let mut buf = [0u8; 4096];
    let n =
        unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

#[cfg(not(target_os = "macos"))]
fn exe(pid: i32) -> Option<String> {
    Some(
        std::fs::read_link(format!("/proc/{pid}/exe"))
            .ok()?
            .to_string_lossy()
            .into_owned(),
    )
}

/// The agent a process is, if it is one. The command name is checked first,
/// then the executable path: a versioned install runs as `.../claude/versions/2.1.3`.
fn agent(p: &Proc) -> Option<&'static str> {
    let is = |s: &str| {
        AGENTS
            .iter()
            .copied()
            .find(|a| s.trim_start_matches('-').eq_ignore_ascii_case(a))
    };
    is(&p.name).or_else(|| exe(p.pid)?.split('/').find_map(is))
}

/// The long-lived process a rental belongs to: the agent that (indirectly)
/// started us. The rental lasts exactly as long as that process does.
pub fn anchor() -> Option<Proc> {
    let mut cur = info(unsafe { libc::getppid() })?;
    let mut fallback: Option<Proc> = None;
    loop {
        if let Some(name) = agent(&cur) {
            cur.name = name.to_string();
            return Some(cur);
        }
        let name = cur.name.trim_start_matches('-').to_ascii_lowercase();
        if fallback.is_none() && !WRAPPERS.contains(&name.as_str()) {
            fallback = Some(cur.clone());
        }
        if cur.ppid <= 1 {
            break;
        }
        match info(cur.ppid) {
            Some(p) => cur = p,
            None => break,
        }
    }
    fallback.or(Some(cur))
}
