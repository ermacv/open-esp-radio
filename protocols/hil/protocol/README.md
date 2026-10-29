# HIL protocol

`oer-hil-protocol` is the radio-independent message and frame skeleton: keys, `Message`/`Endpoint`, `messages!`, bounded framing, the envelope and optional asynchronous IO. `base` owns discovery, image-key pages, boot/post-mortem evidence, link health and request admission; `lifecycle` owns the optional generic reset exchange; `system` holds the stack watermark values other modules carry.

- [Wire contract](wire.md): framing, version checks, discovery and boot identity.
- [Platform diagnostics](diagnostics.md): requested reset and stack measurements.

Each message's key hashes its path and postcard schema. A semantic change with the same schema requires a new path. `messages!` emits typed endpoints/topics/properties and their compile-time `KEYS`; the calling crate's `descriptors` feature additionally emits its `MESSAGES` descriptors. `async-io` enables `write_frame`. There are no radio-family features or dependencies.

Every message owner keeps its reviewed `messages.lock` and checks it in its own test with `check_lock`: the framing version and exactly one `path key kind` line per descriptor. This crate checks its own lock the same way. Domain crates depend on the framework, never the reverse; the framework knows no other owner's messages.

An image's advertised keys come from its mounted [services](../agent/README.md#image-keys). A scenario requires the keys of the services it uses; unrelated extra keys remain accepted. Properties are unit markers and never sent as frames. A refusal is `base::Rejected`; an endpoint has exactly one typed response, which may follow related events.

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
