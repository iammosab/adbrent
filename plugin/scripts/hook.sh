#!/bin/sh
# Entry point for the adbrent Claude Code hooks: hands the hook to the adbrent
# binary. When a session starts on a machine that does not have this version of
# the binary, it is fetched first: a prebuilt one from the GitHub release,
# checked against the checksum below, or else built with cargo.
#
# ADBRENT_NO_FETCH=1 turns the fetching off (use whatever binary is installed).

VERSION=0.2.1
REPO=iammosab/adbrent

event="$1"
home="${ADBRENT_HOME:-$HOME/.adbrent}"
bin="$home/bin/adbrent"

# SHA-256 of each prebuilt binary of this version.
checksum() {
  case "$1" in
    adbrent-darwin-arm64) echo "bb3dd4e9fa300f7b81e94fb627881e62d8547791c4475468d6feea0e06a5bc27" ;;
    adbrent-darwin-x86_64) echo "d0032910da7cec9265c42fc145ec94697fca5a6f77de3dfe000834f511ef7319" ;;
  esac
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

current() {
  [ -x "$bin" ] && [ "$("$bin" version 2>/dev/null)" = "adbrent $VERSION" ]
}

fetch() {
  asset="adbrent-$(uname -s | tr '[:upper:]' '[:lower:]')-$(uname -m)"
  want="$(checksum "$asset")"
  tmp="$bin.new.$$"
  mkdir -p "$home/bin" || return 1
  if [ -n "$want" ] && command -v curl >/dev/null 2>&1 &&
    curl -fsSL --max-time 60 -o "$tmp" "https://github.com/$REPO/releases/download/v$VERSION/$asset" &&
    [ "$(sha256 "$tmp")" = "$want" ]; then
    chmod +x "$tmp" && mv -f "$tmp" "$bin" && return 0
  fi
  rm -f "$tmp"
  # No usable prebuilt binary for this machine: build this version instead.
  command -v cargo >/dev/null 2>&1 || return 1
  cargo install --quiet --git "https://github.com/$REPO" --tag "v$VERSION" --root "$home/build" >/dev/null 2>&1 &&
    cp -f "$home/build/bin/adbrent" "$tmp" && mv -f "$tmp" "$bin"
}

if [ "$event" = session-start ] && [ -z "$ADBRENT_NO_FETCH" ] && ! current; then
  if ! fetch && [ ! -x "$bin" ]; then
    cat >/dev/null
    printf '%s\n' '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"The adbrent plugin is installed but not active in this session: its program could not be downloaded or built, so Android devices are NOT being shared safely between agents. Tell the user, and point them to https://github.com/iammosab/adbrent#install."}}'
    exit 0
  fi
fi

[ -x "$bin" ] || exit 0
exec "$bin" hook "$event"
