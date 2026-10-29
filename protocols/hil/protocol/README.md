# HIL protocol

`oer-hil-protocol` is the radio-independent message and frame skeleton: keys, `Message`/`Endpoint`, `messages!`, bounded framing, the envelope and optional asynchronous IO. `base` owns discovery, image-key pages, boot/post-mortem evidence, link health and request admission; `lifecycle` owns the optional generic reset exchange; `system` owns image diagnostics and properties, and `telemetry` owns profile messages.

- [Wire contract](wire.md): framing, version checks, discovery and boot identity.
- [Platform diagnostics](diagnostics.md): watchdog, program-counter profile and stack measurements.

Each message's key hashes its path and postcard schema. A semantic change with the same schema requires a new path. `messages!` emits typed endpoints/topics/properties and their compile-time `KEYS`; `registry` additionally exposes descriptors and readable decoders for this crate. `async-io` enables `write_frame`. There are no radio-family features or dependencies.

Radio dictionaries are separate host/image crates beside their owners:

| Vocabulary | Owner |
| --- | --- |
| Wi-Fi station | `protocols/ieee80211/control` |
| Network sessions and evidence | `protocols/network-control` |
| Bluetooth | `protocols/bluetooth/control` |
| Lower-MAC conformance | `protocols/ieee80211/conformance-control` |
| PHY capture, hold and cache experiment | `chips/esp32s31/phy-probe` |
| Radio observations | `chips/esp32s31/instrumentation` |
| Radio-health HIL binding and store | `chips/esp32s31/instrumentation-hil` |

Every message owner keeps its reviewed `messages.lock`; [the host registry](../../../tools/hil/registry/README.md) composes them and checks keys, paths, schemas and lock coverage. Domain crates depend on the framework, never the reverse. The locks describe wire dependencies by path prefix, independently of Rust package paths.

An image's advertised keys come from its mounted [services](../agent/README.md#image-keys). A family requires or forbids keys; unrelated extra keys remain accepted. `system::PanicHook` and `system::UsbJtagOff` describe the installed hook and USB-off variant, respectively. Properties are unit markers and never sent as frames. A refusal is `base::Rejected`; an endpoint has exactly one typed response, which may follow related events.

## Conformance trace header

`trace_header` is the header every HIL conformance trace starts with. It is
part of the always-compiled core and allocation-free, so the firmware uses it
as the host does.

| Item | Owns |
| --- | --- |
| `TraceHeader<B>` | The backend's actual bounds, as the port's own `Bounds` type, and the `ClockDomain` of the trace's times |
| `ClockDomain`, `Clock` | The clock (system timer or the host model's virtual clock) and its generation, which starts anew when the clock restarts |
| `check_run`, `Restart`, `RunHeaderError` | The rule for one run's headers: one bound set and one clock; the same generation, which may grow only at a case that declares a restart |

A port's conformance crate owns its bounds (`struct Bounds` with named
fields and `const CEILINGS: Bounds`) and needs no HIL dependency; the HIL
agent sends `(case, header, trace)`. A run iterator that exceeds the
representable case index returns `RunHeaderError::CaseIndexOverflow`; the
final representable index remains a valid case index.

## Instrumentation captures

`capture` transports immutable image-owned observations without a radio
vocabulary. A description identifies the capture, epoch, record schema,
retained record and aggregate counts, dropped history and a typed failure.
Record and counter pages retain that identity and their absolute range.
`capture/refused` is the typed response to an invalid read. The owning image
controls arming, freezing and release; the generic service never performs a
radio operation or retains counters from another boot.

Image observation readers expose live atomic event counters through the capture service, with zero event record pages. The generic record transport remains for independently defined RF experiment payloads. Counter pages are individual atomic readings, without a simultaneous snapshot guarantee.
