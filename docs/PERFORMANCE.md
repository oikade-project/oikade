# Performance measurements

Oikade keeps raw performance evidence separate from correctness tests. The
harness makes Rust-runtime comparisons repeatable; hosted-runner results remain
informational until a fixed Linux ARM64 reference device is designated.

## Environments

- GitHub Actions `ubuntu-24.04-arm` provides clean comparative Linux ARM64
  runs, but the hosted hardware can change.
- The Apple M2 Max development machine is a useful secondary ARM64 signal, not
  evidence that Linux resource budgets have been met.
- Emulated ARM64 is suitable for compatibility checks, not performance
  baselines.

The authoritative reference host must record its model, memory, operating
system, filesystem, power policy and background services alongside results.

## Repeatable process suite

Run from a clean checkout on an idle machine:

```sh
./scripts/perf.sh
```

An explicit new output directory may be supplied as the first argument.
Existing paths are rejected so prior evidence cannot be overwritten. Defaults
are five launches, three seconds of warmup, sixty seconds of measurement and
one-second samples. Short checks can override them:

```sh
OIKADE_PERF_RUNS=1 \
OIKADE_PERF_WARMUP=1s \
OIKADE_PERF_IDLE_DURATION=3s \
OIKADE_PERF_SAMPLE_INTERVAL=500ms \
./scripts/perf.sh performance-results/quick
```

The harness builds the optimized Rust daemon and workspace Matter sidecar with
the locked dependency graph,
uses fresh disposable state, toggles a canonical switch at a fixed interval,
and reports the Oikade host and Matter sidecar separately. Override event load
with `OIKADE_MATTER_EVENT_INTERVAL`. `OIKADE_MATTER_ADAPTER` remains available
as an explicit development override, but is not required.

Each result directory contains:

- `metadata.txt`: commit, dirty state, Rust/Cargo versions, architecture, CPU
  count and kernel details.
- `process-empty-core.json`: startup, sampled CPU, peak RSS, RSS delta and
  graceful shutdown for the Rust core, redb and local admin socket.
- `process-matter-sidecar.json`: the same host measurements plus separate
  Matter-sidecar CPU and memory under canonical events.

`scripts/measure_process.py` uses the operating system's `ps` process table,
cumulative process CPU time and resident-set size. CPU is calculated over the
sampling window, excluding startup and warmup. JSON contains each run plus
median/nearest-rank-p95 summaries. Temporary runtime and protocol state is
deleted after each suite; user state is never discovered or opened.

## Seven-day Matter soak

The M3 reference run has a dedicated entry point:

```sh
./scripts/matter-soak.sh performance-results/matter-soak-reference
```

Its default is a five-minute warmup followed by 168 hours of active canonical
events, sampled every 30 seconds. It retains sparse raw samples so reviewers can
look for a sustained upward RSS trend rather than treating one final allocation
step as a leak. A short harness validation is not release evidence:

```sh
OIKADE_MATTER_SOAK_WARMUP=1s \
OIKADE_MATTER_SOAK_DURATION=10s \
OIKADE_MATTER_SOAK_SAMPLE_INTERVAL=1s \
./scripts/matter-soak.sh /tmp/oikade-matter-soak-check
```

For retained evidence, use a clean checkout on the designated idle Linux ARM64
host using the two binaries built from that checkout. Keep both
`metadata.txt` and `process-matter-soak.json`.

## Interpretation and CI policy

Use absolute budgets only on the fixed reference host. On hosted CI and
developer machines compare similar runs and investigate sustained regressions,
not small single-run differences. Host and plugin/adapter resource costs must
remain separate because isolation is part of Oikade's architecture.

The scheduled ARM64 workflow builds both workspace binaries from the same
commit and lockfile and uploads raw results. It intentionally has no narrow
threshold checks.
