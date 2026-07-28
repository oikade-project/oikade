#!/usr/bin/env bash
set -euo pipefail

adapter="${OIKADE_MATTER_ADAPTER:-}"
binary="${OIKADE_BINARY:-}"
setup_passcode="${OIKADE_MATTER_SETUP_PASSCODE:-02022021}"
discriminator="${OIKADE_MATTER_DISCRIMINATOR:-3840}"

repository="$(cd "$(dirname "$0")/.." && pwd -P)"
if [[ -z "$adapter" || -z "$binary" ]]; then
  cargo build --locked -p oikade -p oikade-matter-adapter
  adapter="${adapter:-$repository/target/debug/oikade-matter-adapter}"
  binary="${binary:-$repository/target/debug/oikade}"
fi
[[ "$adapter" == /* && -f "$adapter" && -x "$adapter" && ! -L "$adapter" ]] || {
  echo "OIKADE_MATTER_ADAPTER must be an absolute executable regular file" >&2
  exit 2
}
[[ "$binary" == /* && -f "$binary" && -x "$binary" && ! -L "$binary" ]] || {
  echo "OIKADE_BINARY must be an absolute executable regular file" >&2
  exit 2
}

work_dir="$(mktemp -d "${TMPDIR:-/tmp}/oikade-matter-smoke.XXXXXX")"
socket="$work_dir/state/oikade.sock"
config="$work_dir/oikade.yaml"
log="$work_dir/oikade.log"
daemon_pid=""
cleanup() {
  if [[ -n "$daemon_pid" ]] && kill -0 "$daemon_pid" 2>/dev/null; then
    kill -INT "$daemon_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
  fi
  rm -rf "$work_dir"
}
trap cleanup EXIT HUP INT TERM

cat > "$config" <<EOF
version: 1
runtime:
  log_level: info
  state_dir: $work_dir/state
  admin_socket: $socket
integrations:
  virtual:
    switches:
      - id: smoke-switch
        name: Matter Smoke Switch
        initial_on: false
adapters:
  matter:
    enabled: true
    executable: $adapter
    log_level: none
    commissioning:
      setup_passcode: "$setup_passcode"
      discriminator: $discriminator
EOF

start_daemon() {
  "$binary" run --config "$config" >"$log" 2>&1 &
  daemon_pid="$!"
  for _ in {1..150}; do
    if "$binary" status --socket "$socket" >/dev/null 2>&1; then
      return
    fi
    if ! kill -0 "$daemon_pid" 2>/dev/null; then
      cat "$log" >&2
      echo "Matter smoke daemon exited before readiness" >&2
      exit 1
    fi
    sleep 0.1
  done
  cat "$log" >&2
  echo "Matter smoke daemon did not become ready" >&2
  exit 1
}

stop_daemon() {
  kill -INT "$daemon_pid"
  wait "$daemon_pid"
  daemon_pid=""
}

start_daemon
"$binary" adapters inspect --socket "$socket" matter >/dev/null
window="$($binary adapters open-commissioning-window --duration 3m --socket "$socket" matter)"
grep -q '^Manual code: [0-9]' <<<"$window"
grep -q '^QR payload: MT:' <<<"$window"
"$binary" devices set builtin.virtual.smoke-switch on true --socket "$socket" >/dev/null
stop_daemon

start_daemon
restored="$($binary devices get builtin.virtual.smoke-switch on --socket "$socket")"
grep -q '= true$' <<<"$restored"
"$binary" adapters reset --confirm matter --socket "$socket" matter >/dev/null
"$binary" adapters inspect --socket "$socket" matter >/dev/null
stop_daemon

echo "PASS: Rust daemon, admin CLI and Matter adapter survived commissioning, events, restart and safe reset"
