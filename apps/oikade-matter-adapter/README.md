# Oikade Matter sidecar

`oikade-matter-adapter` projects Oikade's canonical device model into Matter
using pinned `rs-matter`. It is built in the main Oikade Cargo workspace and
shipped in the same native package as `oikade`.

The adapter deliberately remains a separate supervised process. It owns Matter
commissioning, fabrics, credentials, endpoint allocation and protocol state;
the Oikade daemon remains authoritative for canonical devices and values. The
two processes communicate through the typed, versioned
`oikade-adapter-api` contract over an inherited private Unix socket. Standard
output and standard error are diagnostic streams.

`adapters.matter.log_level: info` is intentionally concise and preserves
commissioning, fabric, subscription and genuine failure records. Use `debug`
only when packet/exchange diagnostics are needed; expected Apple optional-
cluster probes and late acknowledgements are suppressed at normal levels.

Matter is disabled by default. Configure a unique eight-digit setup passcode
before enabling it. After loading state, the adapter opens one 15-minute basic
commissioning window when no fabrics exist. Every process start makes this
decision from the loaded fabric table, so restarting an uncommissioned adapter
opens a fresh window; a restart with any fabric does not. Use the local CLI to
inspect the current window or explicitly open another one:

```sh
oikade adapters commissioning-info matter
oikade adapters open-commissioning-window --duration 10m matter
```

Pairing payloads are returned only by explicit direct local admin responses,
through either the CLI or local API. They never appear in logs, health checks,
or passive status output.

An explicit open request is idempotent while an Oikade-owned window is active:
it returns the current payload and remaining time without extending the window.
A controller-owned window produces a conflict without exposing credentials. If
an Oikade window has expired but rs-matter has not completed its periodic
cleanup, retry after the adapter reports it closed.

The adapter binds Matter UDP before opening automatically. Opening notifies the
mDNS backend to refresh its services, but rs-matter 0.2.0 does not expose a
reliable acknowledgement for the first published advertisement. A one-time
warning records that limitation; commissioning remains available for the full
bounded window even if discovery initialization is delayed.

The current projection supports switches, dimmable lights, outlets,
temperature, relative humidity, contact and occupancy sensors. It does not
require Docker or an external native SDK toolchain.

Build and exercise the real process contract from the repository root:

```sh
cargo build --locked -p oikade-matter-adapter
python3 apps/oikade-matter-adapter/tests/rpc_integration.py \
  --binary target/debug/oikade-matter-adapter
./scripts/matter-smoke.sh
```

The runtime supplies a private state directory. Matter state lives under its
`rs-matter-v1` child and is owned exclusively by the sidecar.

`rs-matter` 0.2.0 does not expose safe live session expiry for local
single-fabric removal. The adapter therefore fails that operation closed;
controller-driven fabric removal remains available.
