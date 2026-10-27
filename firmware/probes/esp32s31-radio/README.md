# Radio comparison and calibrated silicon sheets

The comparison binary retains its Blobray catalog and public-API session.
The staged `oer-firmware-probes-esp32s31-radio-hardware` companion uses the same
session source and the same P-EST/P-FREQ entry. It performs no scan, MAC RX/TX
start, DMA publication or protocol. A production bootstrap supplies the normal
PSRAM, clock and runtime handoff; a P2 task performs one full uncached production
`register_concurrent_phy` before entering Wi-Fi's public power/clock path.
The session retains every owner. There is no second PAC claim.

```console
cargo fw probe oer-firmware-probes-esp32s31-radio
cargo fw build ../probes/esp32s31-radio --bin oer-firmware-probes-esp32s31-radio-hardware --diagnostic
```

Build before leasing. Flash the built bundle with `cargo fw flash
../probes/esp32s31-radio --board s31-a` inside a held board/air lease. Select
HP0 only in OpenOCD, suppress its automatic watchdog-MMIO hook, and write only
the SRAM request word while halted. Read only the SRAM journal.
Return the disposable session to ROM download before releasing the lease.

`open_phy_est_freq_buffer_address(0)` returns the journal address and argument 1
returns the request address (other arguments return 0). This C entry and the sheet
entry use `oer_probe_macros::probe!`; the buffers use `oer_memory::zeroed_static!`
in critical SRAM BSS. A debugger can also resolve their demangled Rust symbols,
`est_freq::EST_FREQ_OUTPUT` and `est_freq::EST_FREQ_REQUEST`. The request starts at 0,
which is ignored; only selection 150 starts calibration. The runtime's interrupt
table supplies the handlers for the image linker's placement analysis.

The companion is an unqualified diagnostic, like the existing staged RX-chain
probe. `--diagnostic` selects `Enforced::Reported`, as `cargo fw run` already does:
requested product checks run and report failures without rejecting the bundle.
It is restricted to `firmware/probes` packages; ordinary application builds
retain their enforced checks. Compiler and mandatory bootstrap-placement checks
still reject failures.
No panic budget or stack/frame limit is raised. The full production registration
currently reaches inherited dependency panic sites and has an unresolved P2
stack path through compiler division helpers. Its product qualification does
not follow from completion of these finite fact sheets.

The revised companion also reports `__oer_interrupt_h0_l16` outside SRAM when
using the runtime table without a manual section attribute. The three other
bound handlers and both buffers are in SRAM. This bundle is not admitted for
another hardware run until automatic placement covers that P1 wake wrapper;
the image keeps no handwritten unsafe placement attribute to bypass the gap.

`EST_FREQ_REQUEST=150` admits exactly one cold calibration. Before that command
the companion performs no radio work; rebooting does not repeat calibration.
After success it retains the calibrated session and accepts 151 (P-EST) and 152
(P-FREQ). Other commands issue no register operations. Selections 153–159 are
unused. The host bounds the full calibration observation to a few seconds;
the sheets themselves contain no unbounded wait. Hardware runs retain one
epoch for the two sheets and do not qualify or compare complete images.

`EST_FREQ_OUTPUT` is 2050 little-endian words in SRAM: marker, record count,
then four-word `(tag,a,b,c)` rows. Markers are `19500000` awaiting calibration,
`19500002` executing, `19500001` calibrated, `19500003` sheet returned success,
`1950bad0` refused/failed and `1950bad1` exhausted output. Completion means the
defined operations returned, not that a hardware hypothesis passed. All tags
and markers in this paragraph and the tables below are hexadecimal; command
selections and estimator window 1000 are decimal.

| Tag | Row arguments |
| --- | --- |
| `96` (selection 150) | calibration start `(mcycle,0,0)`, then `(reason,elapsed_cycles,success)` |
| `1510/1512` | first/second enabled window: `(mcycle before ordered reads,READY,opaque two-bit pair)`; exactly 100 rows each |
| `1511/1513` | matching disabled windows; exactly 100 rows each |
| `1540..1543` | `(total_power,mcycle,measurement_enabled)` after each window |
| `97/98` (selection 151/152) | `(return_code,mcycle,0)` |
| `1520/1522` | `(control image,parameter/status image,read-result)` before/after holding frequency updates |
| `1524/1526/1528` | same observations after read0/read400/read0 |
| `152a/152c/152e` | after bit18 clear/set, after final read0, after restoration |
| each frequency tag +1 | `(bit18,FREQUENCY_READY,mcycle)`; these are later reads, not an atomic snapshot |
| `1624/1626/1628/162c` | `(requested address,returned result,mcycle)` |
| `1530` | `(first low result,expected.block(),match)`; the empirical reference `67` was adopted from run01 after the original guard rejected it, and matched again in run03; this is no established decoding |

P-EST reopens the internal tone, RX/TX clocks and ten forced PBus paths of
`phy/src/analog/crystal_duty.rs::XtalDutyPrepareTransition`, using the default
production RX-path image `bf`. The cold registration already executed RX-DCO
calibration and settled RFPLL. This diagnostic reuses those calibrated products
and that settled frequency; it does not rerun the RFPLL or RX-DCO searches.
It saves/clears the RX-DCO control field, then performs two identical window-1000
measurements: configure fields 1/2/window, start1, 1-µs settle, measurement1,
100 ordered READY/pair samples, measurement0, 1-µs settle, start0, 100 samples.
It restores estimator fields, RX-DCO control and the calibration clock, then
stops the tone and forced paths through the reviewed restoration sequence.
An already enabled estimator is refused before this setup. A ready transition
and the retained power observation qualify the measurement separately from the
opaque pair's value. No per-chain saturation identity is asserted.

P-FREQ captures quiet control/read-mode state, holds hardware updates, reads
addresses 0/400/0 in mode0, then issues two-fresh-RMW bit18 clear/set from bit18=0
and reads0 again. Clear is a no-op and set writes 0→1; the address transitions
0x400→0 supply 1→0 edges. This does not establish a separate reset pulse.
The first result must match the empirical byte 0x67 adopted from
`hw19-20261027-est-freq-01` after the original whole-command guard rejected it.
It coincided with `expected.block()` in two calibrated epochs; that coincidence
does not establish a BBTOP block-byte decoding, universal read mode or memory-bank
geometry. Equal uninitialized values are
not a positive control. Restoration changes only the saved selection and
ownership lanes, preserving unrelated fields; it emits no write or
channel-switch pulse. Only the sourced READY field is observed: no guessed
busy/status field is added.

Each PBus command has at most 21 completion observations, separated by at
most twenty admitted 1-µs minimum settles. This is an experiment bound on
observation count, not a 20-µs whole-operation guarantee. A timeout stops
further PBus publications and requires the disposable session's host reset.

Return codes: 1 unknown selection, 2/3 calibration-clock acquire/release,
4 absent session, 5 unavailable short settle, 6 PBus timeout, 7 already enabled
estimator, 8/9 RX-DCO save/restore refusal, 10 unavailable diagnostic word,
11 active frequency pulse, 12 low-entry reference mismatch, 14 restoration mismatch. Calibration failure
uses reason6 in its selection150 record and halts; it does not install an epoch.
Code13 refuses a comparison session that has not completed full calibration,
before any sheet MMIO.
