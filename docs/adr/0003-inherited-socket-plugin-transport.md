# 0003: Use inherited sockets for plugin RPC

- Status: Accepted

## Context

Plugin protocol traffic needs a private full-duplex channel. Standard output
must remain safe for ordinary plugin and dependency logs.

## Decision

Create an anonymous Unix socket pair for each plugin. The child inherits one
descriptor through `OIKADE_PLUGIN_RPC_FD`; the daemon retains the peer.
Standard output and standard error are bounded diagnostic streams only.

The socket carries the versioned framing defined by ADR 0002. A future Windows
host may use an equivalent inherited transport without changing the contract.

## Consequences

- Accidental logging cannot corrupt RPC frames.
- No socket path, listener port, token or connection retry is required.
- Plugin executables normally run under Oikade supervision.
- The current carrier is Unix-specific.
