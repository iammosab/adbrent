//! End-to-end tests against a fake adb with two devices, AAA and BBB.

use std::io::Write;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const FAKE_ADB: &str = r#"#!/bin/sh
for a in "$@"; do
  if [ "$a" = devices ]; then
    printf 'List of devices attached\n'
    printf 'AAA\tdevice usb:1 product:p model:Pixel_A device:a transport_id:1\n'
    printf 'BBB\tdevice usb:2 product:p model:Pixel_B device:b transport_id:2\n\n'
    exit 0
  fi
done
# The usage probe: FAKE_BUSY names a device with a logcat running on it.
if [ "$3" = shell ] && [ "$2" = "$FAKE_BUSY" ]; then
  case "$4" in *pidof*) echo "S ${FAKE_AGE:-00:05} logcat -v time"; exit 0 ;; esac
fi
# FAKE_TV names a device that reports itself as a TV.
if [ "$3" = shell ] && [ "$2" = "$FAKE_TV" ]; then
  case "$4" in *pidof*) echo "C tv,nosdcard"; exit 0 ;; esac
fi
echo "REAL $*"
"#;

struct Desk {
    dir: PathBuf,
}

impl Desk {
    fn new(name: &str) -> Desk {
        let dir = std::env::temp_dir().join(format!("adbrent-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("shim")).unwrap();
        let fake = dir.join("real-adb");
        std::fs::write(&fake, FAKE_ADB).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        symlink(env!("CARGO_BIN_EXE_adbrent"), dir.join("shim/adb")).unwrap();
        Desk { dir }
    }

    fn command(&self, program: PathBuf, session: &str, args: &[&str]) -> Command {
        let mut c = Command::new(program);
        c.args(args)
            .env("ADBRENT_HOME", self.dir.join("home"))
            .env("ADBRENT_REAL_ADB", self.dir.join("real-adb"))
            .env("ADBRENT_SESSION", session)
            .env_remove("ANDROID_SERIAL");
        c
    }

    fn adb(&self, session: &str, args: &[&str]) -> Output {
        self.command(self.dir.join("shim/adb"), session, args)
            .output()
            .unwrap()
    }

    fn rent(&self, session: &str, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_adbrent").into(), session, args)
            .output()
            .unwrap()
    }

    fn hook(&self, event: &str, input: &str) -> Output {
        let mut c = self.command(env!("CARGO_BIN_EXE_adbrent").into(), "", &["hook", event]);
        c.env("CLAUDE_ENV_FILE", self.dir.join("env"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut child = c.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }
}

impl Drop for Desk {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn each_session_gets_its_own_device() {
    let d = Desk::new("own");
    assert_eq!(
        out(&d.adb("a", &["shell", "echo", "hi"])),
        "REAL -s AAA shell echo hi\n"
    );
    assert_eq!(out(&d.adb("b", &["shell", "id"])), "REAL -s BBB shell id\n");
    // Plain adb keeps going to the device a session already holds.
    assert_eq!(out(&d.adb("a", &["-d", "logcat"])), "REAL -s AAA logcat\n");

    let c = d.adb("c", &["shell", "id"]);
    assert_eq!(c.status.code(), Some(1));
    assert!(out(&c).is_empty(), "the real adb must not run");
    assert!(
        err(&c).contains("every connected device is rented by another agent"),
        "{}",
        err(&c)
    );
    assert!(err(&c).contains("adbrent rent --wait"), "{}", err(&c));
}

#[test]
fn a_rented_device_is_refused_and_a_free_one_is_named() {
    let d = Desk::new("refuse");
    assert_eq!(
        out(&d.adb("a", &["-s", "AAA", "shell", "id"])),
        "REAL -s AAA shell id\n"
    );

    let b = d.adb("b", &["-s", "AAA", "install", "app.apk"]);
    assert_eq!(b.status.code(), Some(1));
    assert!(out(&b).is_empty());
    assert!(
        err(&b).contains("AAA (Pixel A) is already rented by another agent"),
        "{}",
        err(&b)
    );
    assert!(err(&b).contains("free now: BBB (Pixel B)"), "{}", err(&b));

    // By transport id and through ANDROID_SERIAL too.
    assert_eq!(
        d.adb("b", &["-t", "1", "shell", "id"]).status.code(),
        Some(1)
    );
    let via_env = d
        .command(d.dir.join("shim/adb"), "b", &["shell", "id"])
        .env("ANDROID_SERIAL", "AAA")
        .output()
        .unwrap();
    assert_eq!(via_env.status.code(), Some(1));
}

#[test]
fn devices_lists_only_what_a_session_may_use() {
    let d = Desk::new("list");
    d.adb("a", &["-s", "AAA", "shell", "id"]);
    let b = d.adb("b", &["devices"]);
    assert!(
        !out(&b).contains("AAA") && out(&b).contains("BBB"),
        "{}",
        out(&b)
    );
    assert!(err(&b).contains("1 device(s) not listed"), "{}", err(&b));
    assert!(out(&d.adb("a", &["devices"])).contains("AAA"));
}

#[test]
fn release_frees_a_device_for_the_next_session() {
    let d = Desk::new("release");
    d.adb("a", &["-s", "AAA", "shell", "id"]);
    assert_eq!(
        d.rent("b", &["release", "AAA"]).status.code(),
        Some(1),
        "b cannot release a's device"
    );
    assert!(out(&d.rent("a", &["release"])).contains("Released AAA"));
    assert_eq!(
        out(&d.adb("b", &["-s", "AAA", "shell", "id"])),
        "REAL -s AAA shell id\n"
    );

    // The owner of the desk can take a device back.
    assert!(out(&d.rent("a", &["release", "--force", "AAA"])).contains("Released AAA"));
    assert_eq!(out(&d.rent("a", &["rent", "AAA"])), "AAA\n");
    assert_eq!(
        out(&d.rent("a", &["rent"])),
        "AAA\n",
        "rent keeps the device already held"
    );
    assert_eq!(out(&d.rent("b", &["rent"])), "BBB\n");
    assert_eq!(d.rent("c", &["rent", "--wait=0"]).status.code(), Some(1));
}

#[test]
fn a_rental_dies_with_its_owner() {
    let d = Desk::new("stale");
    // A rental left behind by a process that no longer exists.
    let leases = d.dir.join("home/leases");
    std::fs::create_dir_all(&leases).unwrap();
    std::fs::write(
        leases.join("AAA.json"),
        r#"{"serial":"AAA","owner":"s:gone","pid":2147483600,"start":1,"label":"ghost","since":1}"#,
    )
    .unwrap();
    assert_eq!(
        out(&d.adb("b", &["-s", "AAA", "shell", "id"])),
        "REAL -s AAA shell id\n"
    );
}

#[test]
fn kill_server_is_refused_while_others_hold_devices() {
    let d = Desk::new("kill");
    d.adb("a", &["-s", "AAA", "shell", "id"]);
    let b = d.adb("b", &["kill-server"]);
    assert_eq!(b.status.code(), Some(1));
    assert!(err(&b).contains("kill-server refused"));
    assert_eq!(out(&d.adb("a", &["kill-server"])), "REAL kill-server\n");
    assert_eq!(out(&d.adb("b", &["version"])), "REAL version\n");
    // Listing forwards needs no device, so it rents none.
    assert_eq!(
        out(&d.adb("c", &["forward", "--list"])),
        "REAL forward --list\n"
    );
    assert_eq!(
        out(&d.adb("c", &["-s", "BBB", "shell", "id"])),
        "REAL -s BBB shell id\n"
    );
}

#[test]
fn hooks_cover_the_session_lifecycle_and_tool_servers() {
    let d = Desk::new("hooks");
    let start = d.hook(
        "session-start",
        r#"{"session_id":"s1","cwd":"/work/app","source":"startup"}"#,
    );
    assert!(
        out(&start).is_empty(),
        "a session start must add nothing to the session's context"
    );
    let env = std::fs::read_to_string(d.dir.join("env")).unwrap();
    assert!(
        env.contains("export ADBRENT_SESSION='s1'") && env.contains("/shim'"),
        "{env}"
    );
    // Starting again (resume, compaction) does not write the same lines twice.
    d.hook(
        "session-start",
        r#"{"session_id":"s1","cwd":"/work/app","source":"compact"}"#,
    );
    assert_eq!(std::fs::read_to_string(d.dir.join("env")).unwrap(), env);

    // A tool-server call naming a free device rents it; naming a rented one is denied.
    let call = |session: &str| {
        format!(
            r#"{{"session_id":"{session}","tool_name":"mcp__argent__gesture-tap","tool_input":{{"udid":"AAA","x":0.5}}}}"#
        )
    };
    assert!(out(&d.hook("pre-tool-use", &call("s1"))).is_empty());
    let denied = out(&d.hook("pre-tool-use", &call("s2")));
    assert!(
        denied.contains(r#""permissionDecision":"deny""#),
        "{denied}"
    );
    assert!(
        denied.contains("already rented by another agent"),
        "{denied}"
    );
    // Calls that name no device, or are not tool-server calls, pass untouched.
    let bash =
        r#"{"session_id":"s2","tool_name":"Bash","tool_input":{"command":"adb -s AAA shell"}}"#;
    assert!(out(&d.hook("pre-tool-use", bash)).is_empty());

    // The shim and the hook agree on who holds the device.
    assert_eq!(
        out(&d.adb("s1", &["shell", "id"])),
        "REAL -s AAA shell id\n"
    );
    assert_eq!(
        d.adb("s2", &["-s", "AAA", "shell", "id"]).status.code(),
        Some(1)
    );

    d.hook("session-end", r#"{"session_id":"s1","reason":"other"}"#);
    assert_eq!(
        out(&d.adb("s2", &["-s", "AAA", "shell", "id"])),
        "REAL -s AAA shell id\n"
    );
}

#[test]
fn a_device_in_use_outside_adbrent_is_left_alone() {
    let d = Desk::new("outside");
    let busy = |session: &str, args: &[&str]| {
        d.command(d.dir.join("shim/adb"), session, args)
            .env("FAKE_BUSY", "AAA")
            .output()
            .unwrap()
    };
    // Nobody rents AAA, but something is running on it: plain adb skips it.
    assert_eq!(out(&busy("a", &["shell", "id"])), "REAL -s BBB shell id\n");

    // Naming it is refused, and says why and what to do instead.
    let b = busy("b", &["-s", "AAA", "shell", "id"]);
    assert_eq!(b.status.code(), Some(1));
    assert!(out(&b).is_empty());
    assert!(
        err(&b).contains("something outside adbrent is using it"),
        "{}",
        err(&b)
    );
    assert!(
        err(&b).contains("logcat") && err(&b).contains("rent --force AAA"),
        "{}",
        err(&b)
    );
    // BBB is rented by now, and the busy device is not offered as its own alternative.
    assert!(err(&b).contains("no device is free"), "{}", err(&b));

    // With BBB rented and AAA busy, nothing is left to hand out.
    let c = busy("c", &["shell", "id"]);
    assert_eq!(c.status.code(), Some(1));
    assert!(
        err(&c).contains("In use outside adbrent right now: AAA"),
        "{}",
        err(&c)
    );

    // The same check guards tool-server calls.
    let call = r#"{"session_id":"s9","tool_name":"mcp__x__tap","tool_input":{"udid":"AAA"}}"#;
    let mut hook = d.command(
        env!("CARGO_BIN_EXE_adbrent").into(),
        "",
        &["hook", "pre-tool-use"],
    );
    hook.env("FAKE_BUSY", "AAA")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let mut child = hook.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(call.as_bytes())
        .unwrap();
    assert!(out(&child.wait_with_output().unwrap()).contains(r#""permissionDecision":"deny""#));

    // The person in charge can overrule it; after that the device is b's.
    let forced = d
        .command(
            env!("CARGO_BIN_EXE_adbrent").into(),
            "b",
            &["rent", "--force", "AAA"],
        )
        .env("FAKE_BUSY", "AAA")
        .output()
        .unwrap();
    assert_eq!(out(&forced), "AAA\n");
    assert_eq!(
        out(&busy("b", &["-s", "AAA", "shell", "id"])),
        "REAL -s AAA shell id\n"
    );
}

#[test]
fn the_plugin_matches_this_version_and_runs_hooks_through_the_binary() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let version = env!("CARGO_PKG_VERSION");
    let script = std::fs::read_to_string(root.join("plugin/scripts/hook.sh")).unwrap();
    assert!(
        script.contains(&format!("\nVERSION={version}\n")),
        "hook.sh VERSION is out of date"
    );
    let manifest = std::fs::read_to_string(root.join("plugin/.claude-plugin/plugin.json")).unwrap();
    assert!(
        manifest.contains(&format!("\"version\": \"{version}\"")),
        "plugin.json version is out of date"
    );

    // With this version already installed, the script fetches nothing and
    // passes the hook straight to the binary.
    let d = Desk::new("plugin");
    let bin = d.dir.join("home/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_adbrent"), bin.join("adbrent")).unwrap();
    let mut hook = Command::new(root.join("plugin/scripts/hook.sh"));
    hook.arg("session-start")
        .env("ADBRENT_HOME", d.dir.join("home"))
        .env("ADBRENT_REAL_ADB", d.dir.join("real-adb"))
        .env("CLAUDE_ENV_FILE", d.dir.join("env"))
        .env("PATH", "/usr/bin:/bin") // no curl-able network use, no cargo: a fetch would fail
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let mut child = hook.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"session_id":"p1","cwd":"/w","source":"startup"}"#)
        .unwrap();
    let result = out(&child.wait_with_output().unwrap());
    assert!(result.is_empty(), "{result}");
    assert!(std::fs::read_to_string(d.dir.join("env"))
        .unwrap()
        .contains("ADBRENT_SESSION='p1'"));
}

#[test]
fn leftovers_from_a_rental_and_ignored_helpers_are_not_outside_use() {
    let d = Desk::new("leftover");
    let adb = |session: &str, age: &str, args: &[&str]| {
        d.command(d.dir.join("shim/adb"), session, args)
            .env("FAKE_BUSY", "AAA")
            .env("FAKE_AGE", age)
            .output()
            .unwrap()
    };
    // Session a rents AAA (taking it by force: something already runs there), then hands it back.
    let forced = d
        .command(
            env!("CARGO_BIN_EXE_adbrent").into(),
            "a",
            &["rent", "--force", "AAA"],
        )
        .output()
        .unwrap();
    assert_eq!(out(&forced), "AAA\n");
    std::thread::sleep(std::time::Duration::from_secs(2));
    assert!(out(&d.rent("a", &["release"])).contains("Released AAA"));

    // What started during that rental is a leftover: the next session may have the device.
    assert_eq!(
        out(&adb("b", "00:01", &["-s", "AAA", "shell", "id"])),
        "REAL -s AAA shell id\n"
    );
    d.rent("b", &["release"]);

    // Something that has been running since long before any rental is outside use...
    let c = adb("c", "1-00:00:00", &["-s", "AAA", "shell", "id"]);
    assert_eq!(c.status.code(), Some(1));
    assert!(err(&c).contains("rent --force AAA"), "{}", err(&c));

    // ...until the person in charge says to ignore it.
    assert_eq!(d.rent("c", &["ignore", "logcat -v"]).status.code(), Some(0));
    assert_eq!(out(&d.rent("c", &["ignore"])), "logcat -v\n");
    assert_eq!(
        out(&adb("c", "1-00:00:00", &["-s", "AAA", "shell", "id"])),
        "REAL -s AAA shell id\n"
    );
    d.rent("c", &["release"]);
    d.rent("c", &["ignore", "--remove", "logcat -v"]);
    assert_eq!(
        adb("e", "1-00:00:00", &["-s", "AAA", "shell", "id"])
            .status
            .code(),
        Some(1)
    );
}

#[test]
fn a_background_job_cut_loose_from_its_session_is_still_that_session() {
    let d = Desk::new("orphan");
    // No session id of ours: only what Claude Code puts in every command's environment.
    let run = |session: &str, script: &str| {
        Command::new("sh")
            .args(["-c", script])
            .env("ADBRENT_HOME", d.dir.join("home"))
            .env("ADBRENT_REAL_ADB", d.dir.join("real-adb"))
            .env_remove("ADBRENT_SESSION")
            .env("CLAUDE_CODE_SESSION_ID", session)
            .env("CLAUDE_PID", std::process::id().to_string())
            .output()
            .unwrap()
    };
    let shim = d.dir.join("shim/adb");
    let result = d.dir.join("job.txt");
    assert_eq!(
        out(&run("one", &format!("{} -s AAA shell id", shim.display()))),
        "REAL -s AAA shell id\n"
    );
    // A detached job: its parent shell is gone by the time it runs adb.
    run(
        "one",
        &format!(
            "(sleep 1; {} -s AAA shell id > {} 2>&1) > /dev/null 2>&1 &",
            shim.display(),
            result.display()
        ),
    );
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert_eq!(
        std::fs::read_to_string(&result).unwrap(),
        "REAL -s AAA shell id\n"
    );
    // The rental is still the session's, not the short-lived job's.
    let other = run("two", &format!("{} -s AAA shell id", shim.display()));
    assert_eq!(other.status.code(), Some(1));
    assert!(
        err(&other).contains("claude session one"),
        "{}",
        err(&other)
    );
}

#[test]
fn refusals_and_feedback_are_logged_for_the_user() {
    let d = Desk::new("log");
    d.adb("a", &["-s", "AAA", "shell", "id"]);
    d.adb("b", &["-s", "AAA", "shell", "id"]);
    let note = d.rent("b", &["feedback", "AAA was refused but", "I own it"]);
    assert!(out(&note).contains("Recorded"));

    let log = out(&d.rent("a", &["log"]));
    assert!(
        log.contains("refused: rented AAA") && log.contains("held by"),
        "{log}"
    );
    assert!(
        log.contains("feedback") && log.contains("AAA was refused but I own it"),
        "{log}"
    );
    assert_eq!(out(&d.rent("a", &["feedback"])).lines().count(), 1);
    let link = out(&d.rent("a", &["feedback", "--issue"]));
    assert!(
        link.starts_with(
            "https://github.com/iammosab/adbrent/issues/new?title=AAA%20was%20refused"
        ),
        "{link}"
    );
    assert!(link.contains("refused%3A%20rented"), "{link}");
}

#[test]
fn a_renter_can_let_a_helper_session_use_its_device() {
    let d = Desk::new("share");
    d.adb("a", &["-s", "AAA", "shell", "id"]);
    assert_eq!(
        d.adb("helper", &["-s", "AAA", "shell", "id"]).status.code(),
        Some(1)
    );
    assert_eq!(
        d.rent("helper", &["share", "AAA"]).status.code(),
        Some(1),
        "only the renter can share"
    );

    let code = out(&d.rent("a", &["share"])).trim().to_string();
    assert_eq!(code.len(), 6, "{code}");
    assert_eq!(d.rent("helper", &["join", "nope"]).status.code(), Some(1));
    assert_eq!(out(&d.rent("helper", &["join", &code])), "AAA\n");

    // The helper now uses the device, named or not; everyone else is still refused.
    assert_eq!(
        out(&d.adb("helper", &["-s", "AAA", "shell", "id"])),
        "REAL -s AAA shell id\n"
    );
    assert_eq!(
        out(&d.adb("helper", &["shell", "id"])),
        "REAL -s AAA shell id\n"
    );
    assert!(out(&d.adb("helper", &["devices"])).contains("AAA"));
    assert_eq!(
        d.adb("stranger", &["-s", "AAA", "shell", "id"])
            .status
            .code(),
        Some(1)
    );
    assert!(out(&d.rent("a", &["status"])).contains("shared with 1 other session"));

    // A helper cannot end the rental; when the renter does, the sharing ends with it.
    assert!(out(&d.rent("helper", &["release", "AAA"])).contains("Nothing to release"));
    assert_eq!(
        d.adb("stranger", &["-s", "AAA", "shell", "id"])
            .status
            .code(),
        Some(1)
    );
    assert!(out(&d.rent("a", &["release"])).contains("Released AAA"));
    assert_eq!(
        out(&d.adb("stranger", &["-s", "AAA", "shell", "id"])),
        "REAL -s AAA shell id\n"
    );
    assert_eq!(
        d.adb("helper", &["-s", "AAA", "shell", "id"]).status.code(),
        Some(1)
    );
}

#[test]
fn excluded_devices_are_off_limits_and_tvs_are_handed_out_last() {
    let d = Desk::new("exclude");
    assert_eq!(d.rent("u", &["exclude", "AAA"]).status.code(), Some(0));
    assert_eq!(out(&d.rent("u", &["exclude"])), "AAA\n");
    let a = d.adb("a", &["-s", "AAA", "shell", "id"]);
    assert_eq!(a.status.code(), Some(1));
    assert!(
        err(&a).contains("off limits") && err(&a).contains("free now: BBB"),
        "{}",
        err(&a)
    );
    assert!(!out(&d.adb("a", &["devices"])).contains("AAA"));
    assert_eq!(out(&d.adb("a", &["shell", "id"])), "REAL -s BBB shell id\n");
    assert_eq!(
        d.adb("b", &["shell", "id"]).status.code(),
        Some(1),
        "the excluded device is never handed out"
    );
    assert!(out(&d.rent("u", &["status"])).contains("off limits"));
    d.rent("a", &["release"]);
    d.rent("u", &["exclude", "--remove", "AAA"]);

    // AAA says it is a TV: a session that names no device gets the phone first.
    let tv = |session: &str| {
        d.command(d.dir.join("shim/adb"), session, &["shell", "id"])
            .env("FAKE_TV", "AAA")
            .output()
            .unwrap()
    };
    assert_eq!(out(&tv("c")), "REAL -s BBB shell id\n");
    assert_eq!(
        out(&tv("e")),
        "REAL -s AAA shell id\n",
        "the TV is still used when nothing else is free"
    );
}

#[test]
fn a_rental_that_cannot_be_recorded_stops_the_command() {
    let d = Desk::new("readonly");
    // A state folder that cannot be created: its parent is a file.
    std::fs::write(d.dir.join("blocker"), "").unwrap();
    let run = |allow: bool| {
        let mut c = d.command(d.dir.join("shim/adb"), "a", &["-s", "AAA", "shell", "id"]);
        c.env("ADBRENT_HOME", d.dir.join("blocker/home"));
        if allow {
            c.env("ADBRENT_ALLOW_UNRECORDED", "1");
        }
        c.output().unwrap()
    };
    let stopped = run(false);
    assert_eq!(stopped.status.code(), Some(1));
    assert!(out(&stopped).is_empty(), "the real adb must not run");
    assert!(
        err(&stopped).contains("cannot record a rental") && err(&stopped).contains("allowWrite"),
        "{}",
        err(&stopped)
    );
    assert_eq!(out(&run(true)), "REAL -s AAA shell id\n");
}

#[test]
fn the_status_page_shows_devices_and_takes_one_back() {
    use std::io::{BufRead, BufReader, Read};
    let d = Desk::new("serve");
    d.adb("a", &["-s", "AAA", "shell", "id"]);
    let mut server = d
        .command(
            env!("CARGO_BIN_EXE_adbrent").into(),
            "viewer",
            &["serve", "--port", "0"],
        )
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(server.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let port: u16 = line
        .split("127.0.0.1:")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let ask = |request: &str| {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(request.as_bytes()).unwrap();
        let mut reply = String::new();
        s.read_to_string(&mut reply).unwrap();
        reply
    };
    let host = format!("Host: 127.0.0.1:{port}\r\n");
    assert!(ask(&format!("GET / HTTP/1.1\r\n{host}\r\n")).contains("<title>adbrent</title>"));
    let state = ask(&format!("GET /api/state HTTP/1.1\r\n{host}\r\n"));
    assert!(
        state.contains(r#""serial":"AAA""#) && state.contains("session a"),
        "{state}"
    );
    // Another site, or a page that cannot send our header, gets nowhere.
    assert!(
        ask("GET /api/state HTTP/1.1\r\nHost: evil.example\r\n\r\n").starts_with("HTTP/1.1 403")
    );
    assert!(
        ask(&format!("POST /api/release/AAA HTTP/1.1\r\n{host}\r\n")).starts_with("HTTP/1.1 404")
    );
    assert_eq!(
        d.adb("b", &["-s", "AAA", "shell", "id"]).status.code(),
        Some(1)
    );
    let taken = ask(&format!(
        "POST /api/release/AAA HTTP/1.1\r\n{host}X-Adbrent: 1\r\n\r\n"
    ));
    assert!(taken.contains(r#""released":true"#), "{taken}");
    assert_eq!(
        out(&d.adb("b", &["-s", "AAA", "shell", "id"])),
        "REAL -s AAA shell id\n"
    );
    server.kill().unwrap();
    let _ = server.wait();
}

/// Run to the end or be killed: a runaway must fail a test, never the machine.
fn finish(mut command: Command) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed().as_secs() > 10 {
            child.kill().unwrap();
            panic!("still running after 10 seconds");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    child.wait_with_output().unwrap()
}

#[test]
fn two_copies_on_one_path_never_hand_off_to_each_other() {
    let d = Desk::new("twins");
    // A second, separately installed copy with its own stand-in.
    let other = d.dir.join("other");
    std::fs::create_dir_all(other.join("bin")).unwrap();
    std::fs::create_dir_all(other.join("shim")).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_adbrent"), other.join("bin/adbrent")).unwrap();
    symlink(other.join("bin/adbrent"), other.join("shim/adb")).unwrap();
    // The real adb, found by name further along the PATH.
    std::fs::create_dir_all(d.dir.join("sdk")).unwrap();
    std::fs::copy(d.dir.join("real-adb"), d.dir.join("sdk/adb")).unwrap();
    let path = |dirs: &[&str]| {
        dirs.iter()
            .map(|x| d.dir.join(x).display().to_string())
            .chain(["/usr/bin:/bin".to_string()])
            .collect::<Vec<_>>()
            .join(":")
    };
    let run = |path: String, real: Option<PathBuf>| {
        let mut c = d.command(d.dir.join("shim/adb"), "a", &["-s", "AAA", "shell", "id"]);
        c.env("PATH", path)
            .env_remove("ADBRENT_REAL_ADB")
            .env_remove("ADBRENT_HANDOFF");
        if let Some(real) = real {
            c.env("ADBRENT_REAL_ADB", real);
        }
        finish(c)
    };

    // Both stand-ins come before the real adb: the real one is still the one used.
    assert_eq!(
        out(&run(path(&["shim", "other/shim", "sdk"]), None)),
        "REAL -s AAA shell id\n"
    );
    // No real adb at all: say so, at once.
    let none = run(path(&["shim", "other/shim"]), None);
    assert_eq!(none.status.code(), Some(127));
    assert!(
        err(&none).contains("cannot find the real adb"),
        "{}",
        err(&none)
    );
    // Pointed at the other copy by hand: refused the same way.
    assert_eq!(
        run(path(&["shim"]), Some(other.join("shim/adb")))
            .status
            .code(),
        Some(127)
    );
    // A copy under a name we cannot recognise is caught on arrival, one step in.
    std::fs::create_dir_all(d.dir.join("odd")).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_adbrent"), d.dir.join("odd/renamed")).unwrap();
    symlink(d.dir.join("odd/renamed"), d.dir.join("odd/adb")).unwrap();
    let caught = run(path(&["shim"]), Some(d.dir.join("odd/adb")));
    assert!(out(&caught).is_empty());
    assert!(
        err(&caught).contains("another adbrent took for the real adb"),
        "{}",
        err(&caught)
    );
}

#[test]
fn watch_reports_rentals_and_releases_and_a_wait_gets_the_freed_device() {
    use std::io::{BufRead, BufReader};
    let d = Desk::new("watch");
    d.adb("a", &["-s", "AAA", "shell", "id"]);
    d.adb("b", &["-s", "BBB", "shell", "id"]);
    let mut watcher = d
        .command(
            env!("CARGO_BIN_EXE_adbrent").into(),
            "viewer",
            &["watch", "--once"],
        )
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    // A session with nothing free waits in the background, as agents are told to.
    let waiter = d
        .command(
            env!("CARGO_BIN_EXE_adbrent").into(),
            "c",
            &["rent", "--wait=20"],
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert!(out(&d.rent("a", &["release"])).contains("Released AAA"));

    let mut line = String::new();
    BufReader::new(watcher.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line, "released AAA (Pixel A)\n");
    assert!(
        watcher.wait().unwrap().success(),
        "--once stops at the first release"
    );
    let got = waiter.wait_with_output().unwrap();
    assert_eq!(out(&got), "AAA\n", "{}", err(&got));
    assert!(err(&d.adb("e", &["shell", "id"])).contains("as a background command"));
}

#[test]
fn the_log_and_report_show_how_agents_got_on() {
    let d = Desk::new("observe");
    let cli = |session: &str, args: &[&str]| d.rent(session, args);
    assert!(out(&cli("u", &["debug", "on"])).contains("The trace is on"));

    d.adb("a", &["shell", "id"]); // rents AAA
    d.adb("b", &["-s", "AAA", "shell", "id"]); // refused
    d.adb("b", &["-s", "AAA", "shell", "id"]); // goes straight back to it
    assert_eq!(out(&cli("b", &["rent", "--force", "BBB"])), "BBB\n");
    cli("b", &["feedback", "the refusal did not say why"]);
    assert_eq!(cli("c", &["rent", "--wait=1"]).status.code(), Some(1));
    cli("a", &["release"]);

    let log = out(&cli("u", &["log", "-n", "100"]));
    for expected in [
        "rented AAA",
        "refused: rented AAA",
        "released AAA",
        "handed back by its renter",
        "waiting",
        "wait gave up",
        "adb shell id  [passed to adb",
        "adb -s AAA shell id  [stopped",
        "adbrent rent --force BBB",
    ] {
        assert!(log.contains(expected), "the log lacks `{expected}`:\n{log}");
    }

    let report = out(&cli("u", &["report"]));
    assert!(
        report.contains("3 sessions")
            && report.contains("2 rentals")
            && report.contains("3 refusals"),
        "{report}"
    );
    assert!(
        report.contains("1 time an agent went back to a device it had just been refused"),
        "{report}"
    );
    assert!(report.contains("1 forced rental used"), "{report}");
    assert!(report.contains("1 wait gave up"), "{report}");
    assert!(report.contains("the refusal did not say why"), "{report}");
    assert!(
        report.contains("From the trace: 3 adb commands run, 2 stopped by adbrent"),
        "{report}"
    );

    // With the trace off, commands are no longer logged, but what happened still is.
    cli("u", &["debug", "off"]);
    d.adb("a", &["-s", "AAA", "shell", "id"]);
    let after = out(&cli("u", &["log", "-n", "100"]));
    assert_eq!(
        after.matches("  adb  ").count() + after.matches(" adb AAA").count(),
        log.matches("  adb  ").count() + log.matches(" adb AAA").count()
    );
    assert!(after.matches("rented AAA").count() > log.matches("rented AAA").count());
}
