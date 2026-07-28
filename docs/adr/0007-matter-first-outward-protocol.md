# 0007: Make Matter the first outward protocol

- Status: Accepted

## Context

Oikade needs a standards-based path to Apple Home, Google Home and Amazon Alexa
without making any one ecosystem's object model canonical.

## Decision

Use Matter as the first outward protocol. The core models devices and typed
capabilities independently; the Matter adapter projects only safe, documented
mappings and reports unsupported concepts explicitly.

Homebridge compatibility may be added later as an optional input path that
translates supported plugin concepts into Oikade capabilities. It does not
promise private API or custom characteristic parity.

## Consequences

- Oikade has one primary outward model for the v1 roadmap.
- Multi-admin can connect supported devices to the three target ecosystems.
- Ecosystem and device-type differences require interoperability evidence.
- Features without a safe Matter mapping remain unsupported.
