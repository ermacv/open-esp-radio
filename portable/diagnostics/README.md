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
`record::MAX_RECORD_BYTES` (139) bounds a complete tagged record.

`record::decode(bytes)` decodes exactly one record, borrowing its text from
those bytes. It reports `Truncated`, `UnknownRecord` or `Payload` for an
incomplete record, unknown tag/kind, invalid fields/UTF-8 or trailing bytes.
The containing transport owns integrity and message order. The encoder's
previous-panic fields must fit the retained limits of 48 file bytes and 64
message bytes. Invalid caller input returns `EncodeError::Input` with the
field and observed byte length before emitting any bytes; `EncodeError::Sink`
preserves the sink failure. A rejected panic consumes no local console
sequence. Encoding calls no formatter.

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

## Notes: `oer_diagnostics::note`

A **note** is a typed HIL diagnostic: a `Topic` and its values, record tag 3.
Each topic has a constructor on `Note` whose arguments the compiler checks
(`Note::cca_register(point, register, raw)`), so the firmware never builds
text. The words are 32-bit values in the topic's field order (a 64-bit value
is two words, low first), and a name is static text of a variant or register
(at most 8 words, 6 names, 64 bytes a name and 96 bytes of names in a note).
Fixed names use `Name<BYTES>`, a transparent static string whose byte budget
is part of its type. `name!("literal")` constructs it in a const block with
its exact UTF-8 byte length; `name!("literal"; 32)` gives variants a common
budget. `Name::from_static` accepts only an associated-constant text source
and checks it in an inline const block even when called at runtime. No
arbitrary runtime string can enter this type. Each topic constructor
const-checks the sum of its fixed-name budgets against 96 bytes. Terminal
reason producers return these bounded values, so terminal diagnostics are
infallible. The decoder continues to borrow and validate arbitrary wire text.

Payload: topic u16, word count u8, name count u8 (top bit: the path
overflowed), the words, then each name as a length u8 and its bytes. The
decoder accepts exactly the counts of the topic's layout and, for a topic that
ends with a path, up to 4 more words and 6 more names.

`DiagnosticPath` describes a nested error's variants without `core::fmt`: a
type pushes its variant's name (`&'static str`, from an exhaustive `match`)
and calls its payload's `push_path`; the numbers a variant carries are
pushed as words. `DiagPath` keeps 6 names and 4 words (the deepest real chain
is `Ok(Unavailable(Acquire(TrackingTime(TimeReversed(class)))))` with two
64-bit numbers); anything beyond is not dropped silently but marked, and the
host prints `path-truncated`.

With the `render` feature (host only; firmware never enables it)
`render::note` prints a note as the text the firmware used to format
(`OPEN_RADIO_HIL cca-register point=… name=… raw=0x…`; a path prints nested,
`Err(Failed(Run(Port(…))))`, then ` numbers=…`), and `render::Stream` decodes
and numbers a boot's lines. `emit_note` writes a note as an `OER-DIAG:01`
line numbered from the boot's one shared sequence, so notes from the ROM and
from a USB writer in one image do not each count from zero; the S31 board
prints them through the ROM in 64-byte pieces. The HIL host (`oer-hil-link`)
renders them in `uart.log` and for its console checks, and shows a lost,
damaged or unknown line as an `OER-DIAG-ERROR` line with the raw text.

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
