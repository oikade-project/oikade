# 0005: Use HTTP and JSON over a Unix socket for administration

- Status: Accepted

## Context

The CLI needs a supported control boundary that does not read the database
directly or expose a network administration service.

## Decision

Provide versioned HTTP endpoints and JSON payloads over a Unix domain socket in
the runtime state directory. Restrict the socket to its owning user. Unary
requests and streaming responses are bounded.

The server never replaces a non-socket filesystem entry and only removes the
socket inode it owns.

## Consequences

- The CLI and future local tools share one administration contract.
- Unix permissions provide local access control, but not isolation from the
  same operating-system user.
- Windows support requires an equivalent local transport.
- API compatibility must be defined before v1 stability.
