# Architecture

Oikade is a Rust workspace containing one user-facing daemon/CLI and one
first-party Matter sidecar. Supported device plugins are Rust external
processes.

```text
vendor devices and services
            |
            v
Rust device plugins -------- built-in integrations
            |                        |
            +---- plugin RPC v1 -----+
                         |
                         v
                 Oikade runtime
          Device -> Capability -> Value
             |          |          |
             |        redb      admin API
             |                     |
             |                 oikade CLI
             v
          adapter RPC v1
             |
             v
   oikade-matter-adapter -> rs-matter -> Matter controllers
```

## Workspace boundaries

- `apps/oikade` contains the daemon and its administration commands.
- `apps/oikade-matter-adapter` is the supervised Matter process.
- `crates/oikade-core` owns protocol-neutral devices, capabilities, values,
  command routing and state events.
- `crates/oikade-runtime` coordinates lifecycle and built-in integrations.
- `crates/oikade-storage` owns the redb runtime database.
- `crates/oikade-supervisor` owns sanitized child processes, readiness,
  restart backoff, quarantine and bounded logs.
- `crates/oikade-plugin-api` and `crates/oikade-plugin-host` define and host
  the Rust device-plugin boundary.
- `crates/oikade-adapter-api` and `crates/oikade-adapter-host` define and host
  outward protocol sidecars.
- `crates/oikade-admin` provides the local HTTP/JSON API and client over a Unix
  socket.
- `crates/oikade-config` parses the strict versioned YAML configuration.

The two application binaries share a version, Rust toolchain, lockfile,
contract crates, CI and release. They remain separate processes because Matter
owns network-facing protocol code, credentials, fabrics and a distinct runtime.
A Matter failure can therefore be restarted or quarantined without terminating
the core or device plugins.

## Authority and data flow

Device integrations own communication with hardware or vendor services. They
register canonical device definitions, execute commands and publish remote
state changes. The core validates and serializes commands, persists effective
values and emits ordered events.

Outward adapters consume complete canonical topology snapshots and subsequent
state events. They do not own canonical devices. A controller-originated Matter
write returns through the adapter RPC, core and owning integration before the
effective value is committed and reported outward.

The Matter sidecar owns only Matter-specific identity and state: operational
credentials, fabrics, endpoint assignments, cluster projection and controller
subscriptions. Full topology synchronization makes either process
restartable without sharing memory or SDK implementation types.

## Process contracts

Plugin and adapter traffic uses bounded newline-delimited JSON over inherited
anonymous Unix sockets. Standard output and standard error are logs and cannot
corrupt RPC framing. Versioned Rust bindings are used throughout this
repository. The wire format remains language-neutral so future SDKs can be
developed independently without adding their runtimes to Oikade.

Children receive an allowlisted environment, a private state directory when
required, bounded queues and logs, explicit health checks, graceful termination
and restart limits. This is a reliability and secret-reduction boundary, not a
complete operating-system sandbox.

## State boundaries

The core stores canonical state in `runtime-v1.redb` beneath a marked runtime
directory. Matter stores protocol state under its own marked adapter directory.
Plugin-owned state, when needed, remains private to that plugin.

The daemon opens only its marked redb state, and each adapter receives a
separate private state directory. Unsupported record and marker versions fail
with actionable errors.

## Extension rule

Vendor logic belongs in device plugins. Ecosystem projection belongs in
protocol adapters. Management interfaces consume the local admin API. The core
accepts a new responsibility only when it cannot safely live behind one of
those boundaries.
