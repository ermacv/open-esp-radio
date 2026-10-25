# Register model library

## Analog-I2C space

The chip manifest's `indirect-spaces = ["analog/phy.toml"]` selects one
independent `schema = 1`, `kind = "analog-i2c"` document. Its `[[blocks]]`
have a unique `name`, byte `code`, cited `sources`, optional `routes` and
explicit sparse byte `registers`. There is no CPU base, pointer conversion
or inferred 256-register array. A route has `profile`, original
`request-code`, `host` (0/1), one-hot-or-zero `read-select`,
`read-mask-bits` (24/32) and sources. Its command code is the enclosing
physical block's code. Thus two C5 request routes 0x62/0x65 reach the same
physical 0x65 bytes while retaining different host and selection facts.

A byte register has `name`, `index`, optional `description`, `access`,
`read-action`, `modified-write-values`, `reset-value`/`reset-known-mask`,
`reserved = { mask, sources }`, `read-safe = { epoch, sources }` and fields.
The optional `read-safe.comparison-mask` selects lanes for the qualified
capture without declaring their meaning or reset state.
Fields use the same `FieldInfo` declarations as MMIO (`name`, `bitOffset`,
`bitWidth`, access, enumerations, read/write actions and constraints).
Inheritance and arrays require explicit expanded field geometry in this
first transport model; unsupported extension keys are errors.
Sourced `views` select strict subranges of a declared parent field using
`name`, `field`, `bit-offset`, `bit-width` and `sources`; they do not duplicate
the parent's hardware declaration or change bit states.

`[blocks.review]` supplies a whole-record review default. `[[review]]`
records replace it for a fully qualified `BLOCK.REGISTER.FIELD` or
`BLOCK.REGISTER`. They use the common provenance, accuracy, completeness
and naming rules. An opaque byte is `BYTE_<two-digit-index>_OPAQUE`;
opaque fields use the usual `BIT_n_OPAQUE` / `BITS_low_high_OPAQUE` names.
Every publication review and every block, route, reserved and safety source
is validated against the chip's evidence catalogs.

Both MMIO and analog geometry expose `BitStates`: described, opaque,
reserved and absent. Gaps never become reserved. Unknown reset is absence
of both reset properties; a partial reset supplies only its nonempty known
mask, with zeroes outside that mask carrying no knowledge. Readable access
does not qualify a dump: `read-safe` additionally needs a finite epoch and
sources, and rejects destructive/modified reads. The current real models
claim no known reset. The S31 model qualifies only the existing 40-byte R1
image in its calibrated-radio epoch, with explicit comparison masks. These
masks do not promote unknown data lanes to described or OPAQUE.

`AnalogAddress { block, index }` is a separate host type. Semantic IDs retain
the existing grammar: `register:<chip>/analog-i2c/0xBBII/8`, where BB/II encode
block/index, and `register-field:<chip>/analog-i2c/0xBBII/8/<offset>/<width>`.
This number is a transport identity; it is never supplied to the CPU memory
map. CMSIS-SVD exports only direct registers and its omission summary records
the indirect spaces. `cargo registers inspect-analog --manifest
chips/<chip>/registers/publication/registers.toml` queries full geometry,
routes, reviews, bit states and semantic IDs without hardware access.

The ordinary publication command writes the `[outputs].analog` destination,
`pac/src/analog_generated.rs`, containing the closed PAC's `analog_i2c` module:
addresses, bounded byte fields, original-request route lookup, register facts
and a separately qualified `READ_SAFE` list. Shared value types live in
`chips/espressif/analog::model`. Sampled values preserve all other bits and
reject another register's field or a value exceeding its width. Unknown or
partial reset never creates a whole-byte reset image. These are pure values;
no analog bus transaction is generated here. Both production PACs compile
this vocabulary; PAC/HAL/PHY and probes consume its fields, physical byte
identities and routes. `Request` retains an original request code separately
from its physical address, preserving C5 alias routing. Configuration payloads
remain algorithms in PAC code and do not assert a hardware reset. Sourced
`views` select strict subranges of declared hardware fields without overlapping
field declarations or changing their bit states. `CAPTURE` contains only
read-safe bytes with an explicit comparison mask, in physical address order.

## The vendor SVD

`render_soc` renders every peripheral of a chip's vendor SVD as a typed
register block in the PAC API's `soc` module. The vendor SVD names fields
and an access, but not what a read or a write does: a write-one-to-clear
status, a field the hardware changes under a read-modify-write and a
reserved lane all read `read-write` there. The pinned ESP-IDF headers state
it, so they decide the API. The overlay's `[[headers]]` table names each
vendor peripheral's `*_reg.h` (offsets, field bits, access classes,
descriptions) and `*_struct.h` (the device layout); the generator reads them
as pinned artifacts of the chip's `artifacts.toml` (`cargo pins fetch`) and
finds every vendor register in its header by name and offset. From the
header (`soc_headers.rs`):

- `R/W` is a plain field, `RO`/`R`/`HRO` a read-only one, a `WT` a write
  trigger written only by its trigger (a wider one with its value); a
  register of write triggers alone is write-only, written from zero and
  never read;
- a register whose every field is write-one-to-clear (`WTC`, `W1C`) is
  sampled and acknowledged with exactly that sample, never modified;
- a writable field the hardware also changes (`R/W/SC`, `R/W/WTC`,
  `R/W/SS`, ...), a set-only one (`W1S`, `WTS`, `WS`, `R/W1`), an unknown
  class and a write-one-to-clear field beside others leave the register
  read-only, except in a command register or a command port's data words:
  that a field clears itself does not say that writing zero to it is
  harmless, so no command is inferred from a class;
- a field the header keeps to internal debugging ("Do not use it in
  applications") is never written: read only, and a write trigger among
  them a lane every write leaves zero, unless the overlay's
  `driver-writes = true` cites the vendor driver (a source outside
  `components/soc/`) that writes the register;
- a field the header describes as `Reserved`, and bits no header field
  declares, are reserved: no accessor, kept by every write;
- a field a read clears ("cleared by a read", "cleared when read",
  `R_SS_RC`) or a port read and written in sequence gives its register no
  API, also when only the vendor SVD's description says so;
- a field the header names otherwise at the same bits takes the header's
  name; the report lists each (`[[renamed]]`);
- a register the header does not define is read-only, and has no API when
  the device layout reserves its word (`uint32_t reserved_<offset>[n]`); a
  peripheral without a header is read-only;
- a register the header places at another offset than the SVD is an error
  until the overlay corrects its `offset`; `add = true` builds a register
  the SVD lacks from its header definition.

A peripheral whose semantics the headers describe only in text (flags that
a read clears or a write acknowledges in `R/W` fields, self-clearing bits)
is stated in the overlay's `[[no-api]]` table with its reason and sources:
none of its registers is rendered until a typed API exists.

The SVD-only rules stay below the header's: a register with undeclared
bits, a write-only field, overlapping fields, vendor modified-write values,
a write constraint, a field without a snake-case name, or a raw interrupt
status (`_RAW`) the header does not make write-one-to-clear is only read; a
FIFO or data port by its name, a register not 32 bits wide, without fields,
sharing a word or whose word the curated model describes is not rendered.

`derivedFrom` peripherals take their base's registers, corrected. The
chip's `model/strictness.toml` ([`StrictnessOverlay`]) also states what the
headers do not, per register (`offset`, `add`, `access`, `reserved`,
`read-side-effect`, `fifo`, `write-one-to-clear`, `raw-status-writable`,
`data-port`: a command port's data words, one field the hardware also
writes, read and stored whole; `driver-writes`; `define-fields`: the
register's fields where the SVD and its header disagree, each `name`,
`offset`, `width` and `description` from the cited source, replacing the
SVD's; each takes the class of the header field whose bits hold it, also
one the header calls reserved, and the bits no defined field holds are
reserved) and per field (`rename`, `access`,
`kind = "plain" | "write-trigger"`, `values`), the command registers
(`[[commands]]`: `fields`, optional `qualifiers`, `sources`) and the
interrupt sources the SVD does not. A command register's `fields` are
one-bit `R/W/SC` or `R/W/WTC` fields its sources state software starts and
the hardware clears when done; its `qualifiers` are such bits a start
writes together with the command, by its argument. The register is written
only by those starts, and each refuses, writing nothing, while any
self-clearing bit of the register reads one (the named ones and every other
one-bit `R/W/SC`/`R/W/WTC` field, held at zero): the zero a start writes
never lands on a running command. Every other field of it is only read.
Every entry
names existing vendor geometry and cites `esp-idf:<path>:<line>` (at the
overlay's `esp-idf` pin, which must be the chip's pinned ESP-IDF),
`esp-hal:<path>:<line>` (at its `esp-hal` pin) or `trm:<section>`. An
`esp-idf:` citation is checked against the pinned file
(`check_overlay`): a register definition line names the register at its
offset, a field line the field (its words in order) with its bits and an
access class that justifies the entry, a register union's reserved members
the entry's `reserved`, an `interrupts.h` enumerator the source's number;
`ETS_MAX_INTR_SOURCE`, the source count, is no source. A mismatch fails
the generation. The module holds the chip's `Interrupt` enum, one partition
`SocPeripherals` and its `Claims`, which a PAC root claims at once, and a
`const` assertion that no vendor block lists a word of another typed
block. The generated `registers/published/soc.report.toml` lists every
renamed field and every register the pass narrowed, with its reasons.

A typed block with an interrupt source implements
`oer_mmio::InterruptSource` on its interrupt side (or on the whole owner
of a block without sides).

## Direct register model

`oer-registers-model` owns the portable register publication formats:

- safe loading of a multi-file TOML model;
- native unreviewed initialization from explicit peripheral geometry and import
  of CMSIS-SVD standard declarations; the source tool retains original inputs;
- CMSIS-SVD data structures, arrays, clusters, fields and enumerations;
- structured review records kept outside exported hardware descriptions;
- deterministic clean SVD materialization and expanded register identities;
- deterministic sparse reviewed-assertion overlays keyed by canonical typed
  chip/address-space/register and register-field identities;
- one explicit sparse `register-identity = "REGION.NAME"` fact for renaming,
  no-op confirmation or materialization in a concrete singular peripheral,
  with retained evidence and applicability;
- retained effective classification, applicability and evidence for every
  applied register, field, access, description and write-semantics claim;
- generic physical-layout and write-semantics invariants;
- reviewed PAC transaction, binding-index and evidence-catalog schemas;
- the chip memory map (`memory::Memory`, `chips/<chip>/registers/model/memory.toml`):
  its address spaces and regions, validated against the model and its owned
  ranges, and its MMIO windows, which the register tool reads.

The raw PAC is generated output only. The PAC pack has no module declarations
for hand-written code; ownership and validation transactions that need raw
register access live in the closed parent PAC.

The closed-PAC transaction pack is schema 5 only. Its
`w1c-register-snapshots` operation binds one non-array 32-bit W1C register field
to an affine sample token. The token has no public constructor, is not cloneable
and is consumed by the exact same-register acknowledgement; no caller-built
clear image crosses this boundary. Sequencing and hardware qualification stay
above the generated raw helper.

It does not know about ELF files, discovery facts, ESP32-S31, PAC helper
contents or output paths. The [register tool](../../../chips/espressif/registers/README.md) composes it with
explicitly selected reviewed publication inputs. RTOS, NVS,
logging and delay semantics remain outside this crate.

The format and editing workflow are documented by the
[register tool](../../../chips/espressif/registers/README.md).
An absent physical register is created only by one reviewed
`register-identity = "REGION.NAME"` assertion; the
`register-declaration` and `register-name` kinds are rejected with an error. The
top-level manifest is schema 3 and declares the stable chip ID and address
space. Assertion subjects use only canonical `register:<chip>/<space>/...` or
`register-field:<chip>/<space>/...` semantic IDs; other subject spellings are
rejected. The subject must match this model's chip and address
space, have a supported aligned width, fit the named concrete region (and its
register address blocks when present), and not alias or overlap existing
geometry. Identity application is atomic and rejects arrays, clusters,
derived regions, placeholders and noncanonical SVD identifiers. Generated
observations are never consulted for access or modified-write semantics; those
require their own explicit reviewed assertions.

Reusable `[[review]]` metadata is accepted as reviewed coverage only with a
non-empty source list, all three classification fields, and non-`hint`
provenance. Incomplete metadata remains a navigation hint and cannot close a
publication gate. Array coverage is resolved from the structural SVD template,
not by wildcard-matching expanded names.

## Register bit states

Each register or memory-layout bit has one of four states:

| State | Meaning | Evidence |
| --- | --- | --- |
| Described | Named in an open vendor source, or its meaning is otherwise established | A cited source establishes the name or meaning |
| Opaque | Absent from open sources; a use is observed, meaning unknown | A cited source establishes the use |
| Reserved | A source explicitly marks the bit reserved | The register records a cited reserved mask; no field is published |
| Absent | No use is observed in the examined corpus and no source marks the bit reserved | No field or reserved annotation is published |

A use is a write of a value different from reset, an explicit masked set in
a read-modify-write, or a read whose value changes behaviour. Carrying bits
unchanged through a read-modify-write is not a use; neither is writing their
reset value as part of a whole-word store. A whole-word access alone does not
justify an opaque field. Both described and opaque fields require source
evidence (`SOURCE` in the implementation, `sources` in the reviewed model).
Absence is relative to the examined corpus, whose inputs and coverage limits
must accompany a vendor-use check. A use of either a reserved or absent bit
is an error; the checker reports these two cases separately. A bit named in
an open vendor source is never opaque.

Code preserves reserved and absent bits or writes their reset value, and the
PAC exposes no field accessor for them. Layouts follow the same rule: CPU operations
preserve the exact image of unassigned bits, including bits hardware writes.
An existing layout unit can declare `fields = []`: its cited position, size,
whole image and storage remain, with no bit accessors. The `fields` property
is still required. Encoded pointer fields can copy their pointer from another
word image with `with_<field>_from`, preserving all other bits without naming
unassigned lanes.

A whole-register initialization image written by the vendor as an exact
constant remains one cited image operation even where reset is unknown.
The image alone grants no bit accessor and establishes no reset value or
absence of hardware use. Independent masked uses or behaviour-dependent
reads still qualify their own fields. Keep the unresolved lanes in the
unknown-reset report; callers cannot supply an arbitrary replacement image.

A native schema-2 fragment adds one optional property on a register:

```toml
[peripherals.registers.register]
name = "DELAY"
addressOffset = 0
size = 32
reserved = { mask = 0xfffffc00, sources = ["VENDOR_HEADER"] }
```

The nonzero mask must fit the inherited register width and must not overlap
any field. Sources are nonempty, distinct and checked against the publication's
evidence catalogs. Masks above TOML's signed integer range use a hexadecimal
string (up to 128 bits). Array instances inherit the structural register
template's annotation. Sparse reviewed overlays retain it by physical identity
and cannot add an overlapping field. Clean SVD omits these source annotations;
the native model and expanded geometry retain them for checks. No field or PAC
accessor is generated for reserved bits.

## Name origin

A register or field review also records where its name comes from with
`naming`:

| `naming` | Meaning | Requirement |
| --- | --- | --- |
| `vendor` | The vendor's own name | Cites the header, SVD or manual in `sources` |
| `descriptive` | A name this project assigned from established behavior | Published description starts with "Project-assigned name." |
| `opaque` | The meaning is not established | The name ends in `_OPAQUE`; published description starts with "Opaque: meaning not established." |

Every published register and field carries `naming`:
`RegisterModel::load_for_publication`, which publication uses, rejects a model
where one lacks it, while `RegisterModel::load` also opens unreviewed drafts
such as an SVD import. An `_OPAQUE` name and `naming = "opaque"` require each
other, and no published name spells unknown meaning as `UNKNOWN`, `UNNAMED`,
`UNCLASSIFIED` or `OPAQUE` inside the name, so an unreviewed name cannot claim
or hide it. A `descriptive` name states meaning, not position or the vendor's
write: a trailing register offset (`CONFIG_0088`), bit positions alone
(`BITS_5_9`), a write action (`SET_BY_…`, `FORCE_ZERO`, `FORCE_ONE`) or a
complete written image (`IMAGE`, `INIT_IMAGE`) is opaque instead. An opaque
name is its position and nothing else, so it carries no unproven hint of
meaning: a register is `WORD_<offset>_OPAQUE` with its four-digit hexadecimal
offset in its parent, a field `BIT_<n>_OPAQUE` or `BITS_<low>_<high>_OPAQUE`,
and an array inserts `_%s` before the suffix and names its first element's
position. Validation computes the name from the geometry and refuses any
other. The whole content of a register whose meaning is established is its
`VALUE` field. Vendor names keep their
source's spelling. `naming` applies only
to registers and fields. The SVD description carries the marker, so raw PAC
documentation shows it too.
