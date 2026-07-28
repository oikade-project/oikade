# Canonical capability registry

Oikade devices are protocol-neutral collections of typed capabilities. A
capability type describes semantics, not a Matter cluster, HomeKit service or
vendor API. Plugins may add namespaced extension types; every outward adapter
must explicitly map or diagnose each capability it receives.

## Built-in contracts

| Capability type | Kind | Role | Matter mapping |
| --- | --- | --- | --- |
| `oikade.switch.on` | `bool` | actuator | Compatibility mapping to On/Off Light (`0x0100`) |
| `oikade.light.on` | `bool` | actuator | On/Off Light (`0x0100`) |
| `oikade.light.level` | `number` | actuator | Dimmable Light (`0x0101`) with Level Control (`0x0008`), percent |
| `oikade.outlet.on` | `bool` | actuator | On/Off Plug-in Unit (`0x010A`) |
| `oikade.sensor.temperature` | `number` | sensor | Temperature Sensor (`0x0302`), degrees Celsius |
| `oikade.sensor.relative-humidity` | `number` | sensor | Relative Humidity Measurement (`0x0405`), percent |
| `oikade.sensor.contact-open` | `bool` | sensor | Contact Sensor (`0x0015`) with Boolean State (`0x0045`) |
| `oikade.sensor.occupancy-detected` | `bool` | sensor | Occupancy Sensor (`0x0107`) with Occupancy Sensing (`0x0406`) |

Actuator mappings currently require read, write and observe permissions. Sensor
mappings require read and observe permissions and reject write permission.
Light level is a finite percentage from `0` through `100`, inclusive, where
`0` is zero output and `100` is maximum output. It is a readable, writable and
observable capability independent of `oikade.light.on`: writing level `0` does
not implicitly change the canonical on/off state. The owning integration must
report any resulting `oikade.light.on` state change explicitly, and Oikade does
not infer one from the level value.
Temperature values are Celsius and must be finite from `-273.15` through
`327.66`; Matter stores them in hundredths of a degree.
Relative-humidity values are percentages and must be finite from `0` through
`100`; Matter stores them in hundredths of a percent. For contact sensors,
canonical `true` means open and maps to an asserted Matter Boolean State. For
occupancy sensors, canonical `true` sets the occupied bit. The current projection
advertises the required Matter sensor-type fields as PIR until the canonical
model grows an explicit occupancy-modality contract; plugins must not infer a
physical modality from that compatibility field. The adapter emits an
`assumed_occupancy_modality` warning for every such projection so the
compatibility assumption is never silent.

`oikade.switch.on` preserves the generic logical-switch contract. Matter
has no equivalent persistent generic actuator device type, so its On/Off Light
projection is explicitly a compatibility mapping. New plugins should use
`oikade.light.on` or `oikade.outlet.on` when the physical class is known.

## Projection rules

- Each independently mapped capability receives a stable endpoint keyed by
  canonical `device ID/capability ID`; endpoint IDs survive sidecar restart and
  upgrade.
- A valid `oikade.light.on` and `oikade.light.level` pair on one canonical
  device is composed into one stable Dimmable Light endpoint. Both capabilities
  appear in the accepted projection result but consume one endpoint slot. A
  level without exactly one compatible light On/Off capability is diagnosed
  and not projected.
- Other independently mapped capabilities on one canonical device may become
  multiple bridged endpoints until further composed-device profiles are
  defined.
- Unknown extension types produce an `unsupported_capability` diagnostic with
  the exact device and capability IDs. They do not make the adapter unhealthy.
- A known type with the wrong kind, permissions, state shape or value range is
  rejected with a specific diagnostic rather than coerced.
- Adding a canonical type does not imply support in every outward protocol.
  The table and adapter diagnostics are the support boundary.
- The pinned Matter bridge has 16 active dynamic endpoint slots. A complete
  projection is preflighted and rejected before mutation when mapped
  capabilities exceed that limit; unsupported capabilities consume no slot.

The Rust constants are exposed by `oikade-core` and represented by the same
stable strings on plugin API v1. The core registry records kind and role
semantics while the Matter sidecar owns the protocol-specific table.
