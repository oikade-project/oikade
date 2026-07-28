# 0001: Run integrations out of process

- Status: Accepted

## Context

Third-party integrations communicate with remote services and devices, parse
untrusted data and bring their own dependencies. A failure in one integration
must not terminate the smart-home runtime.

## Decision

Run device plugins and protocol adapters as supervised child processes. The
daemon owns lifecycle, readiness, bounded logs, restart backoff and quarantine.
Children receive an allowlisted environment and communicate through explicit
local contracts.

Process isolation is a reliability and secret-reduction boundary, not a
complete operating-system sandbox.

## Consequences

- Crashes, hangs and dependency conflicts are contained to one integration.
- Integrations can be restarted or quarantined independently.
- Every integration needs lifecycle, health and reconciliation semantics.
- Stronger sandboxing remains platform-specific.
