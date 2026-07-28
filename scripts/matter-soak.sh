#!/usr/bin/env bash
set -euo pipefail

timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
results_dir="${1:-performance-results/matter-soak-$timestamp}"
adapter="${OIKADE_MATTER_ADAPTER:-}"
duration="${OIKADE_MATTER_SOAK_DURATION:-168h}"
warmup="${OIKADE_MATTER_SOAK_WARMUP:-5m}"
sample_interval="${OIKADE_MATTER_SOAK_SAMPLE_INTERVAL:-30s}"
event_interval="${OIKADE_MATTER_EVENT_INTERVAL:-250ms}"
setup_passcode="${OIKADE_MATTER_SETUP_PASSCODE:-02022021}"

[[ ! -e "$results_dir" && ! -L "$results_dir" ]] || {
  echo "Matter soak output path already exists: $results_dir" >&2
  exit 1
}

repository="$(cd "$(dirname "$0")/.." && pwd -P)"
cargo build --locked --release -p oikade -p oikade-matter-adapter
adapter="${adapter:-$repository/target/release/oikade-matter-adapter}"
[[ "$adapter" == /* && -f "$adapter" && -x "$adapter" && ! -L "$adapter" ]] || {
  echo "OIKADE_MATTER_ADAPTER must be an absolute executable regular file" >&2
  exit 2
}
mkdir -p "$results_dir"
results_dir="$(cd "$results_dir" && pwd -P)"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/oikade-matter-soak.XXXXXX")"
cleanup() { rm -rf "$work_dir"; }
trap cleanup EXIT HUP INT TERM

machine="$(uname -m)"
system="$(uname -s)"
if [[ "$system" != "Linux" || "$machine" != "aarch64" ]]; then
  echo "warning: M3 reference soak evidence requires Linux ARM64; found $system/$machine" >&2
fi

binary="$repository/target/release/oikade"
socket="$work_dir/state/oikade.sock"
config="$work_dir/oikade.yaml"
cat > "$config" <<EOF
version: 1
runtime:
  log_level: info
  state_dir: $work_dir/state
  admin_socket: $socket
integrations:
  virtual:
    switches:
      - id: soak-switch
        name: Soak Switch
        initial_on: false
adapters:
  matter:
    enabled: true
    executable: $adapter
    log_level: none
    commissioning:
      setup_passcode: "$setup_passcode"
      discriminator: 3840
EOF

{
  echo "schema_version=2"
  echo "scenario=matter-active-event-soak"
  echo "measured_at=$timestamp"
  echo "git_commit=$(git rev-parse HEAD)"
  echo "git_dirty=$(test -z "$(git status --porcelain)" && echo false || echo true)"
  echo "rust_version=$(rustc --version)"
  echo "system=$system"
  echo "machine=$machine"
  echo "duration=$duration"
  echo "warmup=$warmup"
  echo "sample_interval=$sample_interval"
  echo "event_interval=$event_interval"
  echo "matter_adapter=$adapter"
  echo "matter_adapter_metadata=$($adapter --oikade-metadata)"
} > "$results_dir/metadata.txt"

echo "Running Matter active-event soak for $duration..."
python3 "$repository/scripts/measure_process.py" \
  --binary "$binary" \
  --scenario matter-active-event-soak \
  --ready-token "runtime started" \
  --sidecar-binary "$adapter" \
  --include-samples \
  --runs 1 \
  --warmup "$warmup" \
  --idle-duration "$duration" \
  --sample-interval "$sample_interval" \
  --startup-timeout 30s \
  --shutdown-timeout 30s \
  --stimulus-binary "$binary" \
  --stimulus-interval "$event_interval" \
  --stimulus-arg=devices \
  --stimulus-arg=set \
  --stimulus-arg=builtin.virtual.soak-switch \
  --stimulus-arg=on \
  --stimulus-arg='{toggle}' \
  --stimulus-arg=--socket \
  --stimulus-arg="$socket" \
  -- run --config "$config" \
  > "$results_dir/process-matter-soak.json"

echo "Matter soak evidence written to $results_dir"
