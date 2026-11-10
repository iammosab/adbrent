//! A live status page, served to this machine only.

use crate::{cli, desk, store};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PAGE: &str = include_str!("page.html");

/// Asking a device what is running on it costs it a little work each time, so
/// an open page asks each unrented device at most this often.
const PROBE_EVERY: Duration = Duration::from_secs(15);

type Probes = Mutex<HashMap<String, (Instant, Vec<String>)>>;

fn state(probes: &Probes) -> String {
    let probe = |real: &Path, serial: &str| -> Vec<String> {
        let cached = probes.lock().ok().and_then(|p| p.get(serial).cloned());
        if let Some((at, streams)) = cached.filter(|(at, _)| at.elapsed() < PROBE_EVERY) {
            let _ = at;
            return streams;
        }
        let streams = desk::outside(real, serial).streams;
        if let Ok(mut p) = probes.lock() {
            p.insert(serial.to_string(), (Instant::now(), streams.clone()));
        }
        streams
    };
    let events = store::events();
    let recent: Vec<_> = events.iter().rev().take(40).collect();
    serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "now": store::now(),
        "devices": cli::rows(&probe),
        "log": recent,
    })
    .to_string()
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%').then(|| text.get(i + 1..i + 3)).flatten();
        match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn respond(stream: &mut TcpStream, status: &str, kind: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn handle(mut stream: TcpStream, probes: &Probes) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut request = Vec::new();
    let mut chunk = [0u8; 2048];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") && request.len() < 16_384 {
        match stream.read(&mut chunk) {
            Ok(n) if n > 0 => request.extend_from_slice(&chunk[..n]),
            _ => break,
        }
    }
    let text = String::from_utf8_lossy(&request);
    let mut lines = text.lines();
    let mut first = lines.next().unwrap_or_default().split_whitespace();
    let (method, path) = (
        first.next().unwrap_or_default(),
        first.next().unwrap_or_default(),
    );
    let header = |name: &str| {
        text.lines()
            .filter_map(|l| l.split_once(':'))
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim().to_string())
    };
    // Only this machine's own browser, addressing this machine by name.
    let host = header("host").unwrap_or_default();
    if !(host.starts_with("127.0.0.1") || host.starts_with("localhost")) {
        return respond(
            &mut stream,
            "403 Forbidden",
            "text/plain",
            "adbrent serves this machine only",
        );
    }
    match (method, path) {
        ("GET", "/") => respond(&mut stream, "200 OK", "text/html", PAGE),
        ("GET", "/api/state") => respond(&mut stream, "200 OK", "application/json", &state(probes)),
        // The header cannot be sent by another site's page, so only ours can take a device back.
        ("POST", path) if path.starts_with("/api/release/") && header("x-adbrent").is_some() => {
            let serial = decode(&path["/api/release/".len()..]);
            let released =
                store::release(|l| l.serial == serial).is_ok_and(|done| !done.is_empty());
            if released {
                store::log_event("taken back", &store::me(), &serial, "from the status page");
            }
            respond(
                &mut stream,
                "200 OK",
                "application/json",
                &serde_json::json!({ "released": released }).to_string(),
            );
        }
        _ => respond(&mut stream, "404 Not Found", "text/plain", "not found"),
    }
}

pub fn run(args: &[String]) -> i32 {
    let port = args
        .iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(7878);
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "adbrent: cannot serve on port {port}: {e}. Try `adbrent serve --port <other>`."
            );
            return 1;
        }
    };
    let port = listener.local_addr().map_or(port, |a| a.port());
    println!("adbrent status page: http://127.0.0.1:{port}  (Ctrl-C to stop)");
    let _ = std::io::stdout().flush();
    let probes: Arc<Probes> = Arc::default();
    for stream in listener.incoming().flatten() {
        let probes = Arc::clone(&probes);
        std::thread::spawn(move || handle(stream, &probes));
    }
    0
}
