#!/bin/sh
# Entry point for the adbrent Claude Code hooks: hands the hook to the adbrent
# binary. When a session starts on a machine that does not have this version of
# the binary (or a newer one), it is fetched first: a prebuilt one from the
# GitHub release, checked against the checksum below, or else built with cargo.
#
# ADBRENT_NO_FETCH=1 turns the fetching off (use whatever binary is installed).

VERSION=0.4.5
REPO=iammosab/adbrent

event="$1"
home="${ADBRENT_HOME:-$HOME/.adbrent}"
bin="$home/bin/adbrent"

# SHA-256 of each prebuilt binary of this version.
checksum() {
  case "$1" in
    adbrent-darwin-arm64) echo "c337b2d552c3e0895d4470fb0939c0d9a67f52a0e4511434cdf3b692904fa5a6" ;;
    adbrent-darwin-x86_64) echo "c0ceab0ab7b08b339b224abf4ef32226f420a7ea870c66a82b7439e3616b5e79" ;;
  esac
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

# Is the installed binary this version or newer? A session that has been open
# since before an update still runs an older copy of this script, and that copy
# must never replace a newer binary with its own older one.
current() {
  [ -x "$bin" ] || return 1
  have="$("$bin" version 2>/dev/null | sed 's/^adbrent //')"
  [ -n "$have" ] || return 1
  newest="$(printf '%s\n%s\n' "$have" "$VERSION" | sort -t. -k1,1n -k2,2n -k3,3n | tail -1)"
  [ "$newest" = "$have" ]
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
