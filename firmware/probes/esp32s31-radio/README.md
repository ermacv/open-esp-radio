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
the exported SRAM request word while halted. Read only the exported journal.
Return the disposable session to ROM download before releasing the lease.

The companion is an unqualified diagnostic, like the existing staged RX-chain
probe. `--diagnostic` explicitly reports the product checks and is restricted
to probe packages; ordinary application builds retain their enforced checks.
No panic budget or stack/frame limit is raised. The full production registration
currently reaches inherited dependency panic sites and has an unresolved P2
stack path through compiler division helpers. Its product qualification does
not follow from completion of these finite fact sheets.

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
| `1530` | `(first low result,initializer's block-byte reference,match)`; the observed mode0 reference is `67`, rather than the full three-byte command |

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
addresses 0/400/0 in mode0, issues the separate two-fresh-RMW bit18 clear/set,
then reads0 again. The first result must match the initialized BBTOP command's
low-byte reference observed in `hw19-20261027-est-freq-01`; no universal read-mode
or memory-bank geometry is inferred from that reference. Equal uninitialized values are
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
