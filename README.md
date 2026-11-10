# adbrent

A rental desk for Android devices shared between AI agents.

When several agent sessions run on one machine, they all see the same `adb devices` list
and end up driving the same phone. adbrent gives each session its own device: the first
`adb` command a session runs rents a free device, later commands go to that same device,
and any other session that targets it is refused with a message naming the devices that
are free. The rental ends when the session does.

```console
$ adb shell getprop ro.product.model
adbrent: rented 3A021FDJH000XY (Pixel 8) to this session. Run `adbrent release` when you no longer need it.
Pixel 8

$ adb -s R5CT20ABCDE install app.apk
adbrent: R5CT20ABCDE (SM S911B) is already rented by another agent (claude session 3c45fc60 in shop-app, for 4m). Do not use it.
adbrent: free now: emulator-5554 (sdk gphone64 arm64). Use plain `adb` or `adb -s <serial>` to rent one.
```

- **Nothing for agents to learn.** They keep typing `adb`.
- **No daemon.** One 0.5 MB binary; a rental is one small file. A command on a device the
  session already holds adds under a millisecond.
- **Self-healing.** A session that closes or crashes loses its rental immediately.
- **Sees use it does not manage.** Before handing out a device, adbrent asks the device
  whether anything is running on it over adb. A phone that Gradle, Android Studio, scrcpy
  or a plain `adb` elsewhere is using is left alone.

## Install

You need:

- macOS or Linux
- `adb` on your PATH (Android SDK platform-tools)
- A recent stable [Rust](https://rustup.rs), to build it
- Claude Code 2.1 or newer, if you want the plugin

Then:

```sh
cargo install --git https://github.com/iammosab/adbrent
adbrent install
```

The first command builds the binary into `~/.cargo/bin`. The second one:

1. copies the binary to `~/.adbrent/bin/adbrent`,
2. creates the `adb` stand-in at `~/.adbrent/shim/adb`,
3. registers a Claude Code plugin (`adbrent@adbrent`) from `~/.adbrent/plugin`.

Claude Code sessions started after this share devices. Sessions that were already open
keep using plain adb until they are restarted.

### Check that it works

```sh
adbrent status
```

lists your connected devices with nobody renting them. Then start a new Claude Code
session and ask it to run `adb shell getprop ro.product.model`. It should report that a
device was rented to the session, and `adbrent status` in another terminal should show
that session as the renter. When the session ends, the device is free again.

### Other agents and harnesses

Put the stand-in first on the PATH that the agent runs commands with, for example in the
shell profile or launch script the agent uses:

```sh
eval "$(~/.adbrent/bin/adbrent env)"
```

If you do not use Claude Code, `adbrent install --shim-only` skips the plugin.

The renter is the agent process the command runs under. Codex, opencode, Gemini CLI and
several others are recognised by name; anything else falls back to the nearest parent
process that is not a shell. A harness that runs several sessions inside one process can
name each one by setting `ADBRENT_SESSION=<id>` in that session's environment.

### Update

```sh
cargo install --git https://github.com/iammosab/adbrent --force
adbrent install
```

### Uninstall

```sh
adbrent uninstall
cargo uninstall adbrent
```

`adbrent uninstall` removes the Claude Code plugin and `~/.adbrent`, rentals included.

## What agents see

| Command | Behaviour |
|---|---|
| `adb <cmd>` with no device named | Goes to the session's device, renting a free one first if it has none |
| `adb -s <serial> <cmd>` | Rents that device if free; refused if another session holds it |
| `adb devices` | Lists the session's own devices and the free ones |
| `adb kill-server` | Refused while another session holds a device |

`-t <id>`, `-d`, `-e` and `ANDROID_SERIAL` are understood the same way. Commands that
address no device (`adb version`, `adb connect`, `adb forward --list`) pass straight
through.

Plain `adb` hands out the first suitable free device. When a task needs a particular one,
such as a phone and not a TV, the agent should look at `adbrent status` and name it with
`adb -s <serial>`. The plugin tells Claude Code agents this at the start of each session.

## Commands

```
adbrent status [--json]            Devices and who rents each one
adbrent rent [SERIAL] [--wait[=SECONDS]] [--force]
                                   Rent a device and print its serial; --wait queues for one;
                                   --force takes one that looks in use outside adbrent
adbrent release [SERIAL...]        Hand back your devices (all of them by default)
adbrent release --force SERIAL     Take a device back from whoever holds it
adbrent release --force --all      Clear every rental
adbrent env                        Shell line that puts the adb stand-in on PATH
adbrent install [--shim-only]      Set up ~/.adbrent and the Claude Code plugin
adbrent uninstall                  Remove both
```

`adbrent status` is the view for the person in charge:

```console
$ adbrent status
SERIAL          MODEL                STATE   RENTED BY
3A021FDJH000XY  Pixel 8              device  claude session 3c45fc60 in shop-app, 4m
R5CT20ABCDE     SM S911B             device  nobody, but in use outside adbrent (logcat)
emulator-5554   sdk gphone64 arm64   device  -
```

## How it works

`~/.adbrent/shim/adb` is a symlink to the adbrent binary. Run under that name, it works
out which device the command targets, checks `~/.adbrent/leases/<serial>.json`, and then
`exec`s the real adb, so signals, pipes and exit codes behave exactly as before. A rental
records its owner's process id and start time. When that process is gone the rental is
ignored and overwritten by the next renter, so nothing has to clean up after a crash.

The Claude Code plugin adds three hooks:

- **SessionStart** registers the session, puts the stand-in on the session's PATH, and
  tells the agent in a few sentences how devices are shared.
- **PreToolUse** on MCP tools checks the device named in a tool call (`udid`, `serial`,
  `device_id` and similar arguments). Tool servers talk to adb on their own, so this is
  what keeps them inside the rental.
- **SessionEnd** hands the session's devices back.

## Use outside adbrent

Not everything goes through the stand-in. So the first time a session takes a device,
adbrent runs one short command on it and looks at two things:

- **Commands running over adb right now.** Every adb stream (a shell, `logcat`, a test
  run, scrcpy's server) is a child of the device's `adbd`, whichever program on the
  computer started it. If there is one, the device is in use: plain `adb` skips it, and
  naming it is refused with the reason. `adbrent rent --force <serial>` overrules that.
- **When the device last saw input.** A device touched in the last two minutes is handed
  out only when no quieter one is free. It is never refused for this alone.

`adbrent status` shows the same thing for every unrented device. The check costs 0.3 to
0.7 seconds, once, when a device is first rented; later commands skip it. Set
`ADBRENT_NO_PROBE=1` to turn it off.

It sees what is happening at that moment. A program that uses a device in short bursts,
with nothing running in between, looks idle between bursts.

## Limits

- The stand-in covers whatever runs the `adb` command found on PATH. A program that calls
  adb by its full path, or speaks to the adb server directly over its socket (Gradle's
  `installDebug`, Android Studio), is not stopped from using a rented device, though
  adbrent notices while it is using one. Point those at a rented device with
  `ANDROID_SERIAL=$(adbrent rent)`.
- An MCP tool that picks a device itself, without naming one in its arguments, is not
  checked.
- Inside Claude Code's Bash sandbox, adbrent can read rentals but cannot record new ones
  unless `~/.adbrent` is writable (`sandbox.filesystem.allowWrite`). It still refuses
  devices that others hold, but warns and runs the command without renting.
- Rentals are local to one machine and one user account.

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `adbrent: cannot find the real adb on PATH` | adb is not installed, or only the stand-in is on PATH. Install platform-tools, or set `ADBRENT_REAL_ADB=/path/to/adb`. |
| A Claude Code session still drives any device | The session started before `adbrent install`; restart it. `which adb` inside the session should print `~/.adbrent/shim/adb`. |
| A device stays rented by a session that is idle | The session is still open. Close it, or run `adbrent release --force <serial>`. |
| A device is reported in use outside adbrent, and you know it is not | Something left a command running on it (often a forgotten `adb logcat`). Stop that, or run `adbrent rent --force <serial>`. |
| The plugin did not install | Run the two commands `adbrent install` prints: `claude plugin marketplace add ~/.adbrent/plugin` and `claude plugin install adbrent@adbrent`. |

## Development

```sh
cargo test      # unit tests plus end-to-end tests against a fake adb
```

`ADBRENT_HOME` moves the state directory and `ADBRENT_REAL_ADB` names the adb to hand off
to; the tests use both.

## License

MIT
