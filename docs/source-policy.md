# Source policy

What may enter the repository, and how production code records the hardware
knowledge it uses.

Included:

- the radio libraries: protocols, PAC, HAL, PHY and drivers;
- reviewed hardware descriptions (`chips/<chip>/registers/`) and the PAC generated from them;
- recovered hardware tables and calibration coefficients, including values
  recovered from vendor binaries, represented in production source;
- recovered facts and research notes as data (`chips/<chip>/notes/`, `vendor/`);
- architecture, API contracts, operating instructions and capability limits.

Excluded:

- vendor ELF files and static archives;
- disassembly dumps and unreviewed extraction artifacts;
- generated proprietary headers;
- radio ROM or vendor ABI bindings, `esp-wifi-sys` and hidden runtime dependencies;
- credentials.

Production builds link no vendor radio binary and no radio ROM ABI.

Platform ROM functions called by our code are chip facts: each entry point
has its address and a `SOURCE:` citation of a pinned ESP-IDF ROM linker
script or ROM ELF. The ROM linker script is generated from those facts and
exports only the declared entry points. libc and compiler intrinsics come
from `compiler_builtins`, not ROM.

## Recovered tables and coefficients

Required hardware data belongs in the implementation that consumes it. Its
origin in a binary is not a reason to exclude it, replace it with an older
table, approximate it, or defer an otherwise supported implementation, as long
as the source's license permits it (see below).

A recovered value carries its source. Beside the definition, a `SOURCE:` block
names the pinned artifact (library and member, or ROM), the symbol, and the
chips whose pinned artifacts the facts describe, in parentheses after the
marker: `SOURCE(esp32s31):`. A block under a chip's own directory may omit the
list. The pinned artifacts are listed in
`chips/<chip>/vendor/artifacts.toml` (`esp32s31`, `esp32c5`); reviewed
function fingerprints and recovered names are in
`chips/<chip>/vendor/facts/`. State unknown
physical units explicitly. Retain exact numerical behaviour; do not assert
equivalence by comparing a table with a duplicate fixture.

A function that several pinned artifacts define (the ROM's copy and a
library's) is cited by the copy its facts were read from
(`libpp[pm.o]::pm_parse_beacon`, `rom[]::pm_tbtt_process`).

Raw archive or ELF files, section dumps, disassembly and unreviewed extraction
outputs stay outside the repository. A reviewed table expressed as named
source constants is hardware knowledge, not an excluded dump.

Audit reports, work plans, experiment diaries and migration histories are not
tracked documentation. Run outputs belong under ignored output directories.

References to vendor or ROM symbol names in comments describe provenance and
behaviour only; they are not link dependencies.

## Attribution and licenses

The root [`NOTICE`](../NOTICE) holds every attribution: each third-party work
the repository derives from, its license, and the files derived from it, with
the full text of each BSD-3-Clause license. A new derived file is recorded in
the same change:

- **its own file** names the source, its pinned revision and its license
  beside the code; a file ported from an Apache-2.0 work carries the
  modification notice "Ported from … and modified by Vladislav Ermachenkov"
  (Apache-2.0 §4(b));
- **`NOTICE`** gains the file under its source's entry, or a new entry;
- **the crate** becomes `Apache-2.0` when it holds a port of Apache-2.0 code
  (as the ESP-IDF port crates are), and stays `MIT OR Apache-2.0` for
  BSD-3-Clause material, whose notice `NOTICE` keeps; a crate depending on an
  `Apache-2.0` crate states its effective license. Code written from the
  vendor libraries and ROMs (libphy, libpp, the ROM) records facts, not a
  port, and keeps `MIT OR Apache-2.0`.

Copyleft code (GPL, such as Linux `mac80211`) is never translated: such a
project may be cited for observed behaviour that a specification also fixes.

### Sources without a license

Exception: the Bluetooth LE hardware engine (the S31 driver, the HAL's controller-memory
engine, the PAC's Bluetooth initialization and the HAL's scheduler locks)
keeps what it takes from `esp32s31-bt-lib` as it is.

The current `esp32s31-bt-lib` pin includes an Apache-2.0 binary distribution
license. Earlier unlicensed inputs retain their facts-only attribution; a
new distribution license does not relabel an earlier derivation.

From a source that has no license, the repository
records only facts: values, layouts, addresses, order of operations and names.
It records neither code (including code translated into another form, such as
a state machine) nor verbatim data blocks (whole tables copied as they are).
A verbatim table is replaced by values measured on the stand; where that is
not possible, it goes to the owner as a question and is never kept silently.

Work from such a source is clean-room: facts are extracted into the chip's `notes/`,
and code is written from the specification, those facts and measurements, not
from the vendor code.

Using a pinned vendor library or ROM locally or in CI is a different matter
and is allowed in any form, licensed or not: analysis, execution in
emulation, linking into a comparison image. Neither the library nor a
modified copy of it is ever published or presented as ours.
