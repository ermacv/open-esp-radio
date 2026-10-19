# HIL trace header

`oer-hil-trace-header` is the header every HIL conformance trace starts with.
It is `no_std` and allocation-free, so the target
firmware uses it as the host does, and it is a `formats` package.

| Item | Owns |
| --- | --- |
| `TraceHeader<B>` | The backend's actual bounds, as the port's own `Bounds` type, and the `ClockDomain` of the trace's times |
| `ClockDomain`, `Clock` | The clock (system timer or the host model's virtual clock) and its generation, which starts anew when the clock restarts |
| `check_run`, `Restart`, `RunHeaderError` | The rule for one run's headers: one bound set and one clock; the same generation, which may grow only at a case that declares a restart |

A port's conformance crate owns its bounds (`struct Bounds` with named
fields and `const CEILINGS: Bounds`) and needs no HIL dependency; the HIL
agent sends `(case, header, trace)`. The `wire` feature derives
`postcard_schema::Schema` for the HIL protocol.
