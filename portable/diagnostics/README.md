# Boot diagnostic records and console framing

`oer-diagnostics` is a portable, chip-independent codec shared by applications,
HIL images and the host decoder. It has two layers: raw typed records and the
synchronous line frame used by the pilot apps. It installs no queue, retained
storage, transport singleton or trace clock. The platform still owns panic
capture and retained memory; `staged-runtime::diagnostics::previous_panic` is
the shared conversion of its unchanged `PanicRecord` to `PreviousPanic`.

## Record codec: `oer_diagnostics::record`

`record::encode(&Record, write)` emits raw byte slices into a caller-supplied
sink. It accepts complete slices or propagates the sink's error. There is no
console marker, hex escaping, sequence, CRC, clock or transport dependency.
The sink can collect the bytes in an existing bounded payload buffer, or pass
them straight to a framing layer without allocating a full record buffer.
`record::MAX_RECORD_BYTES` (129) bounds a complete tagged record.

`record::decode(bytes)` decodes exactly one record, borrowing its text from
those bytes. It reports `Truncated`, `UnknownRecord` or `Payload` for an
incomplete record, unknown tag/kind, invalid fields/UTF-8 or trailing bytes.
The containing transport owns integrity and message order. The encoder's
previous-panic fields must fit the retained limits of 48 file bytes and 64
message bytes; invalid caller input is rejected by assertions before emitting
a record. Encoding calls no formatter.

| Raw record field | Bytes |
| --- | ---: |
| Tag: previous panic = 1, event = 2 | 1 |
| Payload | variable |

All multibyte fields are little endian. Panic payload: hart u8, flags u8
(interrupt bit 0, PC present bit 1, message present bit 2), line u32, column
u32, PC u32 (zero if absent), file length u8, message length u8, file bytes,
message bytes. The file is the retained suffix (up to 48 UTF-8 bytes); the
static message is the retained prefix (up to 64 bytes). `None` distinguishes
a formatted message that was not kept from an empty static message. Platform
accessors preserve their existing handling of UTF-8 boundaries.

Event payload: `oer_trace::Kind` u16 and two u32 words. Platform event 1,
channel 0, is the radio adapter's `RadioRoot`; platform event 2, channel 0,
is `BootSmokePassed` with words `[0, 0]`. No retained trace ring is required.

HIL's `oer-hil-agent` console puts the **same `record::encode` bytes** into its
own envelope and framing, and the host uses `record::decode` on that payload.
It does not wrap them in `Console`, `OER-DIAG:01`, hex, or a second sequence/CRC.
The HIL transport supplies its own header, ordering and integrity; its
integration belongs to the HIL images' migration. The record format has one
definition shared with the line console below.

## Synchronous line frame: `oer_diagnostics::console`

`Console` wraps the record codec. It runs only on CPU0 after USB initialization;
it is not a concurrent task/IRQ logging backend. Its writer is a concrete
closure which accepts the complete supplied slice or returns an error.
The root exports `Console`, `PreviousPanic`, `Record` and the line `decode`
remain available for the pilot's existing callers.

Each frame starts on a fresh line: CR LF, ASCII `OER-DIAG:01`, then lowercase
hex bytes, and CR LF. Hex escaping keeps embedded NULs and newlines out of the
console line reader. The byte layout and emitted lines are unchanged.

| Field before hex escaping | Bytes |
| --- | ---: |
| Sequence, starting at zero each boot, wrapping u32 | 4 |
| Shared raw record (tag and payload, as above) | variable |
| CRC-16/CCITT-FALSE of sequence and raw record | 2 |

There is no truncated-text fallback. A failed write consumes its sequence;
a subsequent frame restores the line boundary and exposes the loss. The
final verdict is an event too, so a missing entire panic record creates a
sequence gap before that verdict. A partial final record cannot pass.

`console::decode` (also exported as root `decode`) uses caller-owned scratch
space, checks version, hex and CRC, then delegates the raw record to
`record::decode`. Its existing error categories remain unchanged. The host
owns event interpretation, sequence checking and text rendering. The pilot's
host is [`cargo fw`](../../tools/image/cli/README.md).

```console
cargo test -p oer-diagnostics
```

Tests cover the raw codec independently, the line frame, and byte-exact frame
fixtures from the saved pilot stand logs.
