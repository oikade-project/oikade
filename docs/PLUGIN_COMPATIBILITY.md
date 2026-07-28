# Plugin compatibility policy

API v1 establishes the first Rust plugin wire compatibility baseline. The
golden frame corpus is in `contracts/plugin/v1/frames.jsonl` and is consumed by
the `oikade-plugin-api` codec tests.

## Compatible v1 changes

The following changes can remain within API v1:

- Adding an optional JSON field whose absence preserves existing behaviour.
- Allowing a new value where older peers already report unknown values as
  unsupported.
- Relaxing a size, timing or validation limit.
- Adding a method only when support is negotiated and neither peer sends it to
  a peer that did not advertise it.

Decoders ignore unknown fields but continue to reject malformed JSON, trailing
documents, invalid tagged values and oversized frames.

## Breaking changes

These require a new negotiated API version:

- Removing, renaming or changing the type or meaning of an existing field.
- Adding a required field without a backwards-compatible default.
- Reusing a method name, request ID or error code with different semantics.
- Sending a new method unconditionally to an older peer.
- Changing device identity construction or persisted-state interpretation.
- Tightening frame or queue limits below values accepted by supported peers.
- Changing the inherited descriptor contract or JSON framing.

Changes to future high-level Rust SDK traits must also follow normal Rust source
compatibility rules; optional protocol behaviour should not be introduced by
silently adding required trait methods.

## Test requirement

Any plugin protocol or Rust SDK change must:

1. Keep the v1 golden-frame test passing or introduce a new API version with
   new fixtures.
2. Add protocol or SDK tests for new optional behaviour.
3. Document whether the change is additive or version-breaking.

Protocol negotiation never converts plugin configuration, identifiers or
persisted device state.
