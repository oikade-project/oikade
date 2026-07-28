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
