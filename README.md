# Oikade

**Oikade** is a lightweight, extensible smart-home integration runtime written
in Rust. It connects devices and integrations to major smart-home ecosystems
through Matter while keeping vendor and protocol concerns outside its core
model.

> Everything, homeward.

## Why Oikade

Smart-home users should not need a different fragile bridge for every vendor,
protocol and ecosystem. Oikade aims to provide:

- Low and predictable resource use.
- Isolated, supervised integrations that cannot crash the core runtime.
- A stable Rust plugin API with explicit lifecycle and capability contracts.
- Straightforward installation, configuration, upgrades and diagnostics.
- Local operation without an Oikade-hosted cloud dependency.
- Matter connectivity to Apple Home, Google Home and Amazon Alexa for device
  types and features those ecosystems support.
- A future optional companion project for a documented subset of existing
  Homebridge plugins, kept outside the core repository and installation.

Matter is the first outward protocol and the active M3 target. It does not
define Oikade's internal device model: plugins publish canonical devices and a
dedicated Matter adapter projects supported capabilities into the Matter data
model. Other adapters such as MQTT can be added without pretending to be Matter
devices internally.

Homebridge compatibility is an optional future input path. Existing plugins
can only be exposed where their concepts map safely to Oikade capabilities and
supported Matter device types.

## Architecture

The Rust daemon owns configuration, plugin lifecycle, the canonical device model,
state routing, persistence and diagnostics. Rust plugins and protocol
adapters run as supervised child processes over versioned local RPC contracts.
The Matter adapter is a second native Rust application in this Cargo workspace,
built on pinned `rs-matter`. It remains a supervised child process so protocol
failures, credentials and SDK state stay outside the core process. It builds
and runs without Docker or an external native SDK toolchain. It consumes the
same typed adapter contract as the host.

Native release archives build and attest both executables from one source
commit and lockfile, installing the sidecar under `libexec/oikade`. Neither the
package nor the running daemon downloads executable code.

The Matter vertical slice supports switches, dimmable lights, outlets,
temperature, humidity, contact and occupancy sensors, with persistent fabrics
and stable endpoints across restart. It requires explicit validated
commissioning data. Local single-fabric removal remains fail-closed because
`rs-matter` 0.2.0 does not expose safe live session expiry; controller-driven
fabric removal remains available. Matter stays experimental and disabled by
default while M3 completes interoperability, multi-admin and soak validation.
Enable it explicitly with a unique eight-digit setup passcode. On each process
start with no fabrics, the adapter opens one 15-minute commissioning window
automatically. Retrieve its private pairing payload with
`oikade adapters commissioning-info matter`; normal restarts with an existing
fabric do not reopen commissioning.

## Project documentation

- [Project and implementation plan](docs/PROJECT_PLAN.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Architecture decisions](docs/adr/README.md)
- [Matter sidecar](apps/oikade-matter-adapter/README.md)
- [Protocol adapter API](docs/PROTOCOL_ADAPTER_API.md)
- [Canonical capability registry](docs/CAPABILITY_REGISTRY.md)
- [Rust plugin API](docs/PLUGIN_API.md)
- [Plugin SDK repository strategy](docs/PLUGIN_SDKS.md)
- [Plugin compatibility policy](docs/PLUGIN_COMPATIBILITY.md)
- [Core runtime](docs/CORE_RUNTIME.md)
- [Local administration API and CLI](docs/ADMIN_API.md)
- [Performance measurements](docs/PERFORMANCE.md)
- [Native packaging](docs/PACKAGING.md)
- [Logging and secret redaction](docs/LOGGING.md)

## The name

**Oikade** derives from the Ancient Greek **οἴκαδε**, meaning “homeward” or
“towards home.” The name reflects the goal of bringing devices, integrations
and ecosystems together into one coherent home system.

## Development

Oikade requires Rust 1.89 or newer. The daemon, Matter sidecar and supported
plugin binding share one Rust toolchain.

Rust modules are divided by responsibility rather than one file per type.
Internal APIs use the narrowest practical visibility (`pub(super)` or
`pub(crate)`). White-box unit tests that need private implementation details
live in a module's sibling `tests.rs`; black-box API tests belong in a crate's
top-level `tests/` directory. Production modules use explicit imports, while
test modules may use `use super::*` for the unit under test.

```sh
# Check the Rust workspace, Matter process contract and native packaging.
./scripts/check.sh

# Run the repeatable core performance suite.
./scripts/perf.sh

# Run the M3 Matter soak on the designated Linux ARM64 reference host.
./scripts/matter-soak.sh

# Build both production executables.
cargo build --locked -p oikade -p oikade-matter-adapter

# Validate and run the example configuration.
cargo run -p oikade -- validate --config oikade.example.yaml
cargo run -p oikade -- run --config oikade.example.yaml \
  --state-dir /tmp/oikade-development

# In another terminal, inspect and control the daemon.
cargo run -p oikade -- status --state-dir /tmp/oikade-development
cargo run -p oikade -- devices list --state-dir /tmp/oikade-development
cargo run -p oikade -- devices set --state-dir /tmp/oikade-development \
  builtin.virtual.example-switch on true
cargo run -p oikade -- adapters list --state-dir /tmp/oikade-development
```

Exercise the workspace Matter sidecar through Oikade's production adapter host
with a new private state directory:

```sh
./scripts/matter-smoke.sh
```

The smoke script uses fresh disposable state and covers commissioning-window
control, events, restart persistence and explicit safe Matter-state reset.

Oikade source code is available under the [Apache License 2.0](LICENSE). The
Oikade name and project branding remain reserved under the
[branding policy](TRADEMARKS.md).
