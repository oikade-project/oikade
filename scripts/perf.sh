#!/usr/bin/env bash
set -euo pipefail

timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
results_dir="${1:-performance-results/$timestamp}"
process_runs="${OIKADE_PERF_RUNS:-5}"
warmup="${OIKADE_PERF_WARMUP:-3s}"
idle_duration="${OIKADE_PERF_IDLE_DURATION:-60s}"
sample_interval="${OIKADE_PERF_SAMPLE_INTERVAL:-1s}"
matter_adapter="${OIKADE_MATTER_ADAPTER:-}"
matter_setup_passcode="${OIKADE_MATTER_SETUP_PASSCODE:-02022021}"
matter_event_interval="${OIKADE_MATTER_EVENT_INTERVAL:-250ms}"

repository="$(cd "$(dirname "$0")/.." && pwd -P)"
[[ ! -e "$results_dir" && ! -L "$results_dir" ]] || {
  echo "performance output path already exists: $results_dir" >&2
  exit 1
}
echo "Building optimized Rust runtime and Matter sidecar..."
cargo build --locked --release -p oikade -p oikade-matter-adapter
matter_adapter="${matter_adapter:-$repository/target/release/oikade-matter-adapter}"
[[ "$matter_adapter" == /* && -f "$matter_adapter" && -x "$matter_adapter" && ! -L "$matter_adapter" ]] || {
  echo "OIKADE_MATTER_ADAPTER must be an absolute executable regular file" >&2
  exit 2
}

mkdir -p "$results_dir"
results_dir="$(cd "$results_dir" && pwd -P)"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/oikade-perf.XXXXXX")"
cleanup() { rm -rf "$work_dir"; }
trap cleanup EXIT HUP INT TERM

machine="$(uname -m)"
if [[ "$machine" != "aarch64" && "$machine" != "arm64" ]]; then
  echo "warning: results are comparative only; reference measurements require ARM64 (found $machine)" >&2
fi

{
  echo "schema_version=2"
  echo "measured_at=$timestamp"
  echo "git_commit=$(git rev-parse HEAD)"
  echo "git_dirty=$(test -z "$(git status --porcelain)" && echo false || echo true)"
  echo "rust_version=$(rustc --version)"
  echo "cargo_version=$(cargo --version)"
  echo "machine=$machine"
  echo "num_cpu=$(getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.logicalcpu 2>/dev/null || echo unknown)"
  echo "uname=$(uname -a)"
  echo "matter_adapter=$matter_adapter"
  echo "matter_adapter_metadata=$($matter_adapter --oikade-metadata)"
} > "$results_dir/metadata.txt"

binary="$repository/target/release/oikade"

echo "Measuring empty Rust core ($process_runs runs, $idle_duration each)..."
python3 "$repository/scripts/measure_process.py" \
  --binary "$binary" \
  --scenario empty-core \
  --ready-token "runtime started" \
  --runs "$process_runs" \
  --warmup "$warmup" \
  --idle-duration "$idle_duration" \
  --sample-interval "$sample_interval" \
  -- run --state-dir "$work_dir/core-state" \
  > "$results_dir/process-empty-core.json"

config="$work_dir/matter.yaml"
socket="$work_dir/matter-state/oikade.sock"
cat > "$config" <<EOF
version: 1
runtime:
  log_level: info
  state_dir: $work_dir/matter-state
  admin_socket: $socket
integrations:
  virtual:
    switches:
      - id: perf-switch
        name: Performance Switch
        initial_on: false
adapters:
  matter:
    enabled: true
    executable: $matter_adapter
    log_level: none
    commissioning:
      setup_passcode: "$matter_setup_passcode"
      discriminator: 3840
EOF
echo "Measuring Rust core with Rust Matter sidecar ($process_runs runs, $idle_duration each)..."
python3 "$repository/scripts/measure_process.py" \
  --binary "$binary" \
  --scenario matter-sidecar-active-events \
  --ready-token "runtime started" \
  --sidecar-binary "$matter_adapter" \
  --runs "$process_runs" \
  --warmup "$warmup" \
  --idle-duration "$idle_duration" \
  --sample-interval "$sample_interval" \
  --stimulus-binary "$binary" \
  --stimulus-interval "$matter_event_interval" \
  --stimulus-arg=devices \
  --stimulus-arg=set \
  --stimulus-arg=builtin.virtual.perf-switch \
  --stimulus-arg=on \
  --stimulus-arg='{toggle}' \
  --stimulus-arg=--socket \
  --stimulus-arg="$socket" \
  -- run --config "$config" \
  > "$results_dir/process-matter-sidecar.json"

echo "Performance results written to $results_dir"
