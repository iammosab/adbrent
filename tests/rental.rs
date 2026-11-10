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
        err(&c).contains("already rented by another agent"),
        "{}",
        err(&c)
    );
    assert!(err(&c).contains("no device is free"), "{}", err(&c));
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
    assert!(out(&start).contains("additionalContext"));
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
    assert!(
        result.contains("shared between agents through adbrent"),
        "{result}"
    );
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
    assert!(err(&c).contains("adbrent ignore"), "{}", err(&c));

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
