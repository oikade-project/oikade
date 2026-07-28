# Rust plugin API

Oikade currently supports Rust plugins. Each plugin runs as a supervised child
process connected through a dedicated inherited Unix socket. A plugin is a
local artifact directory containing a Rust executable and a strict
`oikade-plugin.yaml` manifest.

The process boundary remains a versioned JSON protocol rather than a Rust ABI.
This keeps plugins isolated from the daemon and leaves room for separately
maintained language SDKs later, but this repository does not currently build,
test or publish non-Rust plugin tooling.

## Configure a local artifact

```yaml
api_version: 1
id: example.weather
name: Example weather integration
version: 0.1.0
executable: example-weather
args: []
```

`executable` must be a regular executable file inside the artifact directory.
The manifest ID and version must exactly match the plugin's handshake. Oikade
rejects incompatible API ranges before registering any devices.

One artifact can be configured as multiple stable instances:

```yaml
version: 1
plugins:
  - id: garden-weather
    artifact: ./plugins/example-weather
    config:
      address: 192.0.2.10
```

Relative artifact paths are resolved from the Oikade configuration file. A
plugin device with local ID `station` receives the stable core ID
`plugin.garden-weather.station`. Restarting or rediscovering the same instance
preserves that identity; changing the configured instance ID is an explicit
identity change.

## Rust binding

`crates/oikade-plugin-api` contains the Rust API v1 wire types, strict codec and
golden-frame tests. `crates/oikade-plugin-host` implements the daemon side of
the process lifecycle. An ergonomic Rust plugin-author runtime, including
transport setup and lifecycle dispatch, is the next SDK layer to add; until it
exists, the wire crate is the supported low-level Rust binding.

Canonical capability constants and semantics live in `oikade-core` and are
documented in the [capability registry](CAPABILITY_REGISTRY.md). Plugins should
publish canonical or namespaced extension types rather than Matter-shaped
types. Outward adapters diagnose extension types they cannot map.

## Protocol lifecycle

The child sends `hello` with its artifact ID, version and supported API range.
Oikade selects API v1 and sends `initialize` with the instance ID and
plugin-owned JSON configuration. The plugin returns its complete device set
before it is marked ready.

After initialization:

- Oikade sends `command` requests and commits the effective returned value.
- The plugin sends unsolicited `event` frames for device-originated changes.
- The plugin sends `reconcile` with its complete device set after topology
  changes.
- Oikade sends `health` requests to detect an unresponsive protocol loop and
  expose integration health.
- Oikade sends `cancel` when a command context expires.

Every frame is one JSON object followed by a newline and is limited to 1 MiB.
Requests and responses use non-zero request IDs; unsolicited frames use zero.
The socket descriptor number is supplied in `OIKADE_PLUGIN_RPC_FD`. It has no
filesystem path or listening endpoint. A plugin normally fails with an
actionable error when launched directly rather than by Oikade.

Standard output and standard error are available for plugin logs. Oikade
captures, bounds and tags both streams, so ordinary output cannot corrupt the
RPC channel.

Command, frame and notification queues are bounded. Events and reconciliations
share one ordered queue. A malformed frame, unknown response ID, queue overflow
or health timeout closes the session; supervision then applies restart backoff
and restart-loop quarantine while the core and other plugins remain running.

Operators can inspect plugin supervision without accessing plugin state:

```sh
oikade plugins list
oikade plugins inspect garden-weather
```

The API remains experimental. Additive and breaking-change rules are documented
in the [plugin compatibility policy](PLUGIN_COMPATIBILITY.md).
