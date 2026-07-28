# Core runtime

Oikade's protocol-neutral core is deliberately small: integrations register
devices and typed capabilities, the core routes commands and committed values,
and protocol adapters can subscribe to events.
Matter clusters and other protocol-specific structures are not
core model types.

## Canonical contracts

A device has a stable runtime-wide identifier, a human-readable name and one or
more capabilities. A capability has:

- A stable device-scoped identifier.
- An extensible semantic type such as `oikade.switch.on`.
- One scalar value kind: boolean, integer, number or string.
- Explicit read, write and observe permissions.
- An initial value of the declared kind.

Identifiers use lowercase ASCII letters, digits, dots, underscores and hyphens.
The built-in virtual integration namespaces its devices as
`builtin.virtual.<configured-id>`. Plugin devices use
`plugin.<instance-id>.<device-id>`. These stable wire contracts do not expose
internal Rust implementation types as a public SDK.

Built-in type semantics and their current outward support are listed in the
[canonical capability registry](CAPABILITY_REGISTRY.md). Unknown namespaced
types remain valid in the core and are diagnosed by adapters that cannot map
them.

## Command and event flow

For a write, the core validates the requested value, serialises commands for the
device, calls its owning integration and commits the effective value returned by
that integration. A failed command does not alter committed state. Spontaneous
integration updates use the same commit and event path.

Subscriptions have an explicit bounded buffer. A subscriber that fills its
buffer is closed with `ErrSlowConsumer`; it cannot block command processing or
other subscribers. Cancellation closes only the affected subscription. Events
carry a process-local monotonic revision and a UTC commit timestamp.

## Persistence

Each device's values are saved atomically in the redb device-state table.
Records have their own format version, separate from the database schema. State
for capabilities not present in the current device definition is retained so a
temporary integration change does not silently discard it.

An unsupported record version fails startup for that device with an explicit
error. Oikade does not rewrite unsupported records automatically.
The configured virtual device ID provides stable identity, and its last
committed value takes precedence over `initial_on` after restart.

## Built-in virtual integration

The strict YAML configuration can create deterministic switches for development
and runtime tests:

```yaml
version: 1
integrations:
  virtual:
    switches:
      - id: example-switch
        name: Oikade Example Switch
        initial_on: false
```

Start it with an isolated state directory:

```sh
cargo run -p oikade -- run \
  --config oikade.example.yaml \
  --state-dir /tmp/oikade-development
```

The integration is a core fixture, not a user-facing hardware integration and
not a Homebridge compatibility mechanism.

## Local control boundary

The local admin server starts after configured integrations and stops before
them. It maps status, device reads, validated commands and event subscriptions
onto the same core methods used by tests and protocol adapters. See the
[local administration guide](ADMIN_API.md) for its socket security and API.
