# 0006: Use dedicated protocol adapter sidecars

- Status: Accepted

## Context

Outward protocol adapters consume canonical topology, issue controller-originated
commands and own ecosystem credentials and identity. Their bidirectional
ownership differs from a device plugin.

## Decision

Use a separate adapter API over an inherited anonymous Unix socket supplied in
`OIKADE_ADAPTER_RPC_FD`. Every envelope declares whether it is a request,
response or notification so either peer can initiate work safely.

Oikade owns canonical topology and state. Adapters own protocol-specific
credentials, mappings and persistence. Full topology sync and acknowledged
events provide deterministic reconciliation after restart.

## Consequences

- Protocol stacks remain isolated from the core process.
- Plugin and adapter ownership stay explicit.
- Sidecars can use independent runtimes when needed.
- Native packages must distribute supported adapter binaries.
