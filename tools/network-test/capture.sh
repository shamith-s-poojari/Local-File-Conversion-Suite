#!/usr/bin/env bash
# Network test with packet capture (spec 14.5). Needs root for the capture.
#
#   sudo tools/network-test/capture.sh [--app "/path/to/Local File Conversion Suite.app"]
#
# 1. Captures every packet on every interface while the network test runs
#    (crates/app-core/tests/network.rs: every tool, on a canary corpus).
# 2. Attributes packets to processes and FAILS on any non-loopback packet
#    from the core, a runner, a sandbox wrapper or a worker (item 3).
#    Loopback is the canary's domain, checked by the test itself (item 6).
# 3. With --app, also launches the app bundle, leaves it idle for 30 s and
#    quits it. Traffic from the app's own executable fails; traffic from
#    WebView helper processes is compared with webview-allowlist.txt, and any
#    destination not listed fails (item 4). The allowlist comes from spike S4.
#
# Attribution:
#   macOS  pktap: the kernel tags each packet with the sending process.
#   Linux  strace on the test's process tree: every connect() and sendto()
#          to an internet address is recorded with its PID. tcpdump runs too,
#          as a record. (Packets carry no process on Linux.)
#   Windows: pktmon is not wired up yet (T09 machine needed).
#
# The positive controls live in the network test: it fails if the canary or
# the socket sampler can't see deliberate connections.

set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
app=""
if [[ "${1:-}" == "--app" ]]; then
  app="${2:?--app needs the .app path}"
fi
if [[ "$(id -u)" -ne 0 ]]; then
  echo "capture.sh: run with sudo (packet capture needs root)" >&2
  exit 2
fi
user="${SUDO_USER:-$(logname)}"
out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT
fail=0

# Our processes, by name. Anything else on the machine is ignored here.
ours='converter-desktop|network-[0-9a-f]+|[Pp]ython[0-9.]*|converter-sandbox-run|sandbox-exec|bwrap'

as_user() { sudo -u "$user" -H env PATH="$PATH" "$@"; }

echo "== building the network test (as $user)"
as_user bash -c "cd '$repo' && cargo test -q -p converter-app-core --test network --no-run" >/dev/null

case "$(uname -s)" in
Darwin)
  echo "== capturing with pktap while the network test runs"
  tcpdump -i pktap,all -k -w "$out/net.pcapng" -U 2>/dev/null &
  cap=$!
  sleep 2
  as_user bash -c "cd '$repo' && cargo test -q -p converter-app-core --test network" || fail=1
  sleep 1
  kill "$cap"; wait "$cap" 2>/dev/null || true

  # -k NP prints the process name and PID for each packet.
  tcpdump -r "$out/net.pcapng" -k NP -nn 'not (host 127.0.0.1 or host ::1)' 2>/dev/null \
    | grep -E "proc ($ours):" > "$out/ours.txt" || true
  if [[ -s "$out/ours.txt" ]]; then
    echo "FAIL: our processes sent packets off the machine:"
    head -50 "$out/ours.txt"
    fail=1
  else
    echo "ok: no packets from the core, runners or workers"
  fi

  if [[ -n "$app" ]]; then
    echo "== capturing while the app runs idle for 30 s"
    tcpdump -i pktap,all -k -w "$out/app.pcapng" -U 2>/dev/null &
    cap=$!
    sleep 2
    as_user open -W -a "$app" &
    opener=$!
    sleep 30
    as_user osascript -e 'quit app "Local File Conversion Suite"' || true
    wait "$opener" 2>/dev/null || true
    kill "$cap"; wait "$cap" 2>/dev/null || true

    tcpdump -r "$out/app.pcapng" -k NP -nn 'not (host 127.0.0.1 or host ::1)' 2>/dev/null > "$out/app.txt" || true
    if grep -E "proc converter-desktop:" "$out/app.txt" > "$out/app-own.txt"; then
      echo "FAIL: the app's own process sent packets:"
      head -20 "$out/app-own.txt"
      fail=1
    fi
    # WebView helpers: list destinations, compare with the allowlist.
    grep -E "proc com\.apple\.WebKit\.[A-Za-z]+:" "$out/app.txt" \
      | grep -oE '> [0-9a-fA-F.:]+\.[0-9]+:' | sed -E 's/^> //; s/\.[0-9]+:$//' | sort -u > "$out/webview.txt" || true
    allow="$here/webview-allowlist.txt"
    new="$(comm -23 "$out/webview.txt" <(grep -vE '^\s*(#|$)' "$allow" 2>/dev/null | sort -u) || true)"
    if [[ -n "$new" ]]; then
      echo "FAIL: WebView traffic to destinations not in webview-allowlist.txt:"
      echo "$new"
      fail=1
    else
      echo "ok: WebView traffic matches the allowlist ($(wc -l < "$out/webview.txt" | tr -d ' ') destinations)"
    fi
  fi
  ;;
Linux)
  command -v strace >/dev/null || { echo "capture.sh: install strace" >&2; exit 2; }
  echo "== capturing with tcpdump and tracing connect() while the network test runs"
  tcpdump -i any -nn -w "$out/net.pcap" -U 'not (host 127.0.0.1 or host ::1)' 2>/dev/null &
  cap=$!
  sleep 2
  bin="$(as_user bash -c "cd '$repo' && cargo test -q -p converter-app-core --test network --no-run --message-format=json" \
    | grep -oE '"executable":"[^"]*network-[0-9a-f]+"' | tail -1 | cut -d'"' -f4)"
  as_user strace -f -qq -e trace=connect,sendto,sendmsg -o "$out/trace" "$bin" || fail=1
  kill "$cap"; wait "$cap" 2>/dev/null || true
  # Internet addresses other than loopback, from any traced process.
  grep -hE 'sa_family=AF_INET6?' "$out"/trace* 2>/dev/null \
    | grep -vE 'inet_addr\("127\.0\.0\.1"\)|inet_pton\(AF_INET6, "::1"' > "$out/ours.txt" || true
  if [[ -s "$out/ours.txt" ]]; then
    echo "FAIL: connections to internet addresses from the test's process tree:"
    head -50 "$out/ours.txt"
    fail=1
  else
    echo "ok: no internet connections from the core, runners or workers"
  fi
  if [[ -n "$app" ]]; then
    echo "capture.sh: --app is macOS only for now" >&2
  fi
  ;;
*)
  echo "capture.sh: unsupported OS" >&2
  exit 2
  ;;
esac

exit "$fail"
