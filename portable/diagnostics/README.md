# Boot console diagnostics

`oer-diagnostics` sends typed boot observations over the existing synchronous
line console. It installs no queue, retained storage, transport singleton or
trace clock. The platform still owns panic capture and retained memory.
`oer-espressif-staged-runtime::diagnostics::previous_panic` is the shared
conversion from its unchanged `PanicRecord` to the borrowed `PreviousPanic`.

The writer is a concrete closure which writes each supplied slice completely
or returns an error. It runs only on CPU0 after USB initialization; this is
not a concurrent task/IRQ logging backend. Encoding calls no formatter.
The previous panic carries hart, interrupt flag, optional interrupted PC,
line, column, the retained file suffix (up to 48 UTF-8 bytes), and the static
message prefix (up to 64 bytes), distinguishing an empty static message from
a formatted message that was not retained. Platform accessors preserve their
existing handling of UTF-8 boundaries.

An event uses the existing `oer_trace::Event` identity and two words. Platform
event 1, channel 0, is the radio adapter's `RadioRoot`; platform event 2,
channel 0, is `BootSmokePassed` with words `[0, 0]`. Neither event requires
enabling or changing the retained trace. The final verdict is an event too:
losing an entire panic record creates a sequence gap before that verdict.

## Line encoding, version 1

Each record starts on a fresh line: CR LF, ASCII `OER-DIAG:01`, then lowercase
hex bytes, and CR LF. All multibyte fields are little endian. Hex escaping
keeps embedded NULs and newlines out of the console line reader.

| Field | Bytes |
| --- | ---: |
| Sequence, starting at zero each boot, wrapping u32 | 4 |
| Record tag: previous panic = 1, event = 2 | 1 |
| Payload | variable |
| CRC-16/CCITT-FALSE of sequence, tag and payload | 2 |

Panic payload: hart u8, flags u8 (interrupt bit 0, PC present bit 1, message
present bit 2), line u32, column u32, PC u32 (zero if absent), file length
u8, message length u8, file bytes, message bytes. Event payload: kind u16,
two u32 words. There is no truncated-text fallback. A failed write consumes
its sequence; a subsequent record restores the line boundary and exposes the
loss. A partial final record cannot produce a verdict.

`decode` uses caller-owned scratch space, rejects unknown tags/versions,
oversized or malformed payloads, bad UTF-8, invalid hex and CRC mismatches.
The host owns event interpretation, sequence checking and text rendering.
The pilot's host is [`cargo fw`](../../tools/image/cli/README.md).
HIL agents continue using their existing `base/boot` envelopes; the HIL
framing, message schemas and panic handler are unchanged.

```console
cargo test -p oer-diagnostics
```
