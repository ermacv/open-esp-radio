# ESP32-S31 vendor verification

## Pinned vendor artifacts

[`artifacts.toml`](artifacts.toml) is the single pin of every vendor archive,
the ROM ELF and the SDK firmware this project compares against: upstream
repository, revision, path and SHA-256 of each fetched artifact, and for the
firmware outputs the catalog image whose tracked recipe builds them, with no
digest. Fetch the pinned artifacts into `target/vendor/<source>/<revision>/` with

```console
cargo verification fetch esp32s31
```

which verifies every file and reports unbuilt firmware outputs. Every scenario
authenticates its fetched inputs against the manifest, and its `--library`, `--rom`,
`--libpp`, `--libnet80211`, `--libcoexist`, `--sdk`, `--phy-sdk` and
`--rftest` arguments default to those pinned locations; an explicit path must still match the pin.
`--sdk` and `--phy-sdk` are the bootloader and image of the
[`phy-tracking-reference`](hil-vendor/README.md) catalog project, built with

```console
cargo hil firmware build phy-tracking-reference
```

and accepted only while that build follows the project's current recipe:
its files and the pinned ESP-IDF tree. Changing a pin
means changing the manifest; production follows the pinned behaviour.
Hashes in reviewed register evidence and reference notes record the artifact
a fact was observed in; they are not pins.

## All PHY comparison scenarios

`all` runs every native comparison scenario (`gain`, `i2c`, `channel`,
`rx-gain`, `tx-dc`, `tracking`, `wifi-mac`, `bluetooth`, `coex`, `coex-hw`) concurrently under one budget, each in its own
directory below `--output`. Any scenario's failure fails the run; it prints
each scenario's duration.
With `--index <directory>` it then writes every scenario's shard of the
native evidence index qualification reads: every claimed vendor root with its
production entry, compared cases and retained executions, the input
identities and the digests of the sources the verdicts depend on. A shard
rests on its own scenario's claims, so `all` writes the shard of every
scenario that passed even when another did not; it then skips the decision
checks that judge every scenario together, says so, and still fails. Every
scenario subcommand takes the same `--index` and writes only its own shard.
The index is derived data: `cargo verification evidence --chip esp32s31`
computes it into `target/verification/esp32s31/evidence`, and after any
production, probe, scenario or Blobray change it depends on qualification
counts no evidence from it, and reports it stale, until it is computed again.

`LLD` below is the pinned Rust toolchain's linker, which
`cargo verification evidence` picks by default; Ubuntu's LLD 18 fails these
RV32 links:

```console
LLD="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin/gcc-ld/ld.lld"
cargo verification scenario --chip esp32s31 all \
  --production target/verification/esp32s31/oer-esp32s31-probe-radio-elf \
  --bluetooth-production target/verification/esp32s31/oer-esp32s31-probe-bluetooth-elf \
  --linker "$LLD" --output target/blobray-research/all \
  --index target/verification/esp32s31/evidence
```

## Captured I2C command-memory comparison

The `i2c` scenario ([`i2c.rs`](scenarios/src/phy/i2c.rs)) compares the authenticated archive/ROM command initializer
and its descriptor/no-op leaves with a freshly built production probe ELF. It
checks all 45 ordered command writes against independent instruction-derived
expectations on zero, mixed, lower and upper parameter profiles. Descriptor
outputs are checked byte for byte from both zero and filled initial memory.
Command-memory writes use an explicit passive register model. The runner also
executes the [transport scenarios](scenarios/src/phy/i2c_transport.rs) below with bounded
packed-command responses; neither model claims physical timing or RF behavior.

```console
cargo verification probes --chip esp32s31
cargo verification scenario --chip esp32s31 i2c \
  --production target/verification/esp32s31/oer-esp32s31-probe-radio-elf \
  --linker "$LLD" --output target/blobray-phy-i2c
```

Besides the command-memory, transport and call-boundary comparisons, the SDK
bootloader firmware (`--sdk`) enables the calibration leaves and the
PBus/DCODE prefix, and the PHY SDK firmware (`--phy-sdk`) adds RFPLL. Both
default to their pins in [`artifacts.toml`](artifacts.toml), which are local
builds: `cargo verification fetch esp32s31` reports them when missing.

The scenario owns construction and independent expected-value assertions;
Blobray operations own capture and linking, and Blobray's in-process
verification owns execution and comparison. The scenario
reads `phy_param` directly from the exported linked ELF's symbol table;
inexact source mappings are not used to infer that address. All inputs are captured before local source copies
are removed. The pinned archive/object/table/ROM hashes authenticate vendor inputs;
the exact production ELF digest identifies the freshly compiled replacement.

An explicit setup entry copies scenario parameters into writable captured memory.
Warm execution then uses the shipping HAL/PAC command-memory transaction on one
side and the linked archive root plus captured ROM callees on the other. A retained
guest entry shim supplies zero values for integer callee-saved registers on both
sides. This is a declared concrete input: the executor still stops on unknown
register copies/spills and never initializes them implicitly. The shim may only
be entered as an isolated root because it destroys its caller's saved registers.

The relation compares all MMIO/fence/delay events and selected final descriptor
bytes. Void returns, setup addresses, internal branches and stack transactions are
excluded explicitly. MATCH establishes only this software relation under these
inputs. Changed replacement input must produce DIFF; an unknown argument must
produce INCOMPLETE; a missing ROM companion must stop at its unmapped fetch;
exhausted event capacity must fail with a resource limit. There is no hardware
claim.

### Captured I2C transport

The same runner links captured byte/field read/write, host selection and reset
paths with their exact ROM callbacks. An explicit guest interface table points to
captured code; it supplies no replacement callback semantics. The production
probe calls the shipping cold-I2C transaction state machine, host configuration
and bounded wake-reset helper. Its finite harness only supplies arguments,
capabilities and completion edges. A retained entry shim supplies all eight
integer arguments and explicit callee-saved register values.

The generic [packed-command model](../../tools/blobray/cli/reference/execution/README.md#packed-command-bank)
owns two ports over one seeded bank. Scenario declarations select register
geometry, initial values, optional scripted replies and busy poll counts. These
are environmental assumptions. Bank writes commit on a ready read; pending
commands and unused scripted samples prevent a complete closed result. Reset
aborts the selected pending command without restoring bank values or samples.

The positive matrix checks thirteen host selectors, byte and masked read/write
on both hosts with immediate and delayed completion, and idle/busy reset. Each
of its forty cases checks independent expected command writes and applicable
read returns, not just equality between implementations. Cases use independent
cold instances and are batched to fit the ordinary request-size limit. The
relation selects read-operation returns and a reviewed effect contract per
vendor/production pair: production performs a pre-issue readiness check, so the
contract ignores analog I2C port reads, and every other read, write, fence and
delay compares exactly, including the read-mask and host-map accesses. Void
returns are explicitly excluded.

Exhausted scripted replies must produce INCOMPLETE without retained-value
fallback. An out-of-field write must produce DIFF: captured code spills the
value into an adjacent bit while the typed production path clips it. Limited
completion edges and the shipping reset polling bound must leave pending-model
INCOMPLETE on timeout; they cannot become MATCH merely because a CPU returned.
This establishes the selected software
relation under declared peripheral responses, not analog-bus or RF qualification.

### Current calibration leaves

`--sdk` with the pinned bootloader firmware includes the native
[four-leaf matrix](scenarios/src/phy/calibration_leaves.rs). Its eleven independent cold cases
exercise TX-gain restore, forced signed digital gains, temperature-to-power and
post-init AGC with complementary retained register inputs. All MMIO reads/writes
remain selected, with independently checked ordered writes and temperature
returns. AGC executes its captured ROM saturation-gain child. The guest probe
constructs a validation owner and calls existing shipping HAL/PHY leaves; no
production capability is fabricated by seeding a pointer.

The extra SDK bootloader ELF, the `sdk` pin, is a static symbol companion.
AGC shares one archive `.iram1` section with unrelated functions. Their retained
relocations require the physical SDK clock symbol in addition to ROM
symbols. The runner records all these explicit symbol bindings and captures the
SDK before deleting source copies. It does not execute SDK bodies or install
responses for them: the selected roots use only their retained image, ROM and
probe mappings. Entering an unmapped SDK dependency would be an execution gap.
The comparison does not cover the other functions co-located in `.iram1`.

Restore comparison requires the explicitly enabled domain. Temperature conversion
uses the current archive policy: positive Wi-Fi delta divides by eight, negative
by three; Bluetooth uses five/four. Selecting a similar ROM function would change
that policy. Void returns are excluded, while temperature returns are compared.
Changed temperature must produce DIFF, an unknown argument buffer or absent
register input must produce INCOMPLETE, and exhausted event capacity must fail
with a resource limit. These leaves do not
establish a complete calibration or physical timing claim.

### PBus and DCODE prefix

`--sdk` also runs
[the native prefix matrix](scenarios/src/phy/calibration_prefix.rs). PBus covers 24 combinations
of retained values, work-mode settling, immediate/delayed readiness and stack
fills. It compares all MMIO observations and requested delays, with independent
expected command/acknowledgment writes. Three stuck-command positions execute the
production timeout policy and check that unfinished commands are not acknowledged
and work mode is not restored afterward. Finite read runs encode the long busy
prefix without allocating one declaration element per response.

DCODE covers four crystal selectors, two stack/analog fills and two readiness
delays. A setup phase executes captured ROM `memcpy` to initialize the retained
parameter buffer. A small guest ABI shim calls the captured PBus and DCODE bodies
in order; the production wrapper executes its actual parent transitions and both
children. ROM I2C callbacks select captured archive code. The peripheral bank
supplies retained CKGEN values and eight finite samples; delay responses expose
requested microseconds only. No model replaces a calibration algorithm.

The DCODE relation compares each of the eight selected final bytes and every
effect under a reviewed contract. Production performs extra I2C busy prechecks,
so the contract ignores reads of the two command ports; every other read, write,
fence and requested delay compares exactly. The runner independently checks the
four frequency programs, NRX values, forty CKGEN commands, sample consumption and
exact output `[0,63,31,32,32,31,63,0]`. The DCODE and PBus matrices are one
request each, with a stack fill per case. A comparison without the contract
retains the expected polling DIFF.

Stuck channel readiness, stuck I2C and a read/modify/write whose halves individually
fit but jointly exhaust the production budget must return their specific failure,
leave all eight initialized output bytes unchanged and stop issuing commands at
the failed boundary. Changed samples produce DIFF, an unknown parameter pointer
produces INCOMPLETE, and insufficient event capacity fails with a resource
limit. This is a software prefix comparison under the
declared environment, not complete RX calibration or hardware qualification.

### RFPLL search and frequency maintenance

`--phy-sdk` runs
[the native RFPLL matrix](scenarios/src/phy/rfpll.rs). The additional linked SDK
firmware, the `phy-sdk` pin, contributes the exact static `phy_printf` definition needed by the linked
archive section. It is captured and retained but is not an execution companion:
its TLS and overlapping code/data load segments exceed the current loader
profile. Diagnostics are disabled in positive cases; enabling them must stop at
an unmapped instruction fetch. No stub or successful diagnostic model is used.

Search covers nine finite status/capacitor profiles, two stack fills and two busy
prefixes (36 cases), including signed underflow and the 511-capacitor boundary.
The ROM capacitor helpers and archive I2C callbacks execute captured code. The
compiled wrappers acquire the production radio capability and invoke shipping
search/maintenance owners. Finite lock-status samples are peripheral inputs;
no model supplies the search result. Independent assertions require the exact
signed result, command sequence and requested five-microsecond settles.

The production delay adapter executes a guest save/restore shim around
`open_phy_trace_delay_event`; the model binds that inner requested-time edge.
Integer caller registers are preserved by captured instructions, avoiding
unspecified call clobbers in inactive coroutine fields. Entry explicitly seeds
unused temporaries as well as saved registers. The modeled ROM delay retains the
ordinary external-call ABI. No general unknown-value handling is relaxed. ROM
`memcpy` may use unaligned normal-memory words; the recorded execution environment
explicitly admits those accesses while MMIO and atomics remain aligned.

Maintenance includes eight zero-delta cases and forty positive/negative,
underflow/overflow cases across channels 1, 13, 14, 2412 and 2484. Setup executes
captured ROM `memcpy` to initialize the archive parameter object. The measured
phase retains those bytes and executes real weak grant functions. Nonzero
corrections read and rewrite all 85 frequency-memory entries, preserve unrelated
bits and restore the selected channel. An independent transaction oracle checks
all frequency and SDM reads/writes, including signed narrowing and high bits;
requested delays must remain exactly two microseconds followed by the search
settles. The stuck-command case returns the production timeout, leaves the first
command pending and must not restore hardware frequency control.

Search comparison selects the low return; maintenance omits the vendor's void
return. Both compare every effect under a reviewed contract that ignores analog
I2C port reads; every other read, write, fence and delay compares exactly. For
nonzero maintenance only, the vendor samples channel status once more
immediately before reading frequency control: the installed layout query, whose
count and exact value the runner checks. The maintenance contract lets
production omit that one read; production owns that fixed layout. Search and
maintenance are one request each, with a stack fill per case. A comparison
without the contract retains the polling DIFF. Changed capacitor input, unknown callback pointer, unmapped diagnostics and
event exhaustion have explicit DIFF/INCOMPLETE/resource-limit checks. Software
observations do not establish hardware/RF or grant qualification.

### Wi-Fi and Bluetooth gain arithmetic/publication

The `gain` scenario of the [typed scenario package](scenarios/src/phy/gain.rs)
executes captured archive callbacks and ROM children against the compiled
production arithmetic and publishers. Build and validate the probe catalog
with `cargo verification probes --chip esp32s31`. Then run from the
repository root; `xtask` builds the scenarios:

```console
cargo verification scenario --chip esp32s31 gain \
  --production target/verification/esp32s31/oer-esp32s31-probe-radio-elf \
  --linker "$LLD" \
  --output target/blobray-research/gain
```

Requests and evidence are Blobray's own serde types. `cargo test --manifest-path verification/Cargo.toml -p oer-esp32s31-vendor-scenarios`
checks the harness, the evidence interpretation and the oracles. Those tests
need no private inputs.

Each scenario passes its Blobray budget explicitly: `--timeout-secs` (600),
`--working-memory-mib` (256) and `--max-work-units` (2,000,000,000) set each
operation's deadline and capacities. Every finite matrix is one execution
request. Shared guest addresses and analog command
encodings are named in [`layout.rs`](scenarios/src/engine/layout.rs).

Scenarios hold vendor knowledge, peripheral inputs and independent
expectations; Blobray mechanisms supply the rest:

- ROM companions are proposed by `propose-companions` from the ROM, then from
  any supplied SDK firmware, and copied into the retained link request; no
  scenario keeps a companion list.
- Requested-delay models answer every call. Delay counts are evidence, and the
  scenarios compare delay sequences instead of declaring budgets.
- Where a scenario uses the radio aperture, every radio register without an
  explicit model is retained storage that starts with the fill pattern. Only
  semantic inputs are modeled.
- The channel, RX-gain and TX-DC roots publish their committed `phy_param`
  fields through a reviewed output projection: Blobray compares each field's
  final vendor bytes with its production output location, and output padding
  is not claimed.
- The channel, RX-gain and TX-DC roots compare their effects under a reviewed
  effect contract ([`contracts.rs`](scenarios/src/engine/contracts.rs)). The contract
  is a typed value reviewed through git; each root's relation selects it by the
  digest of its canonical encoding, and the scenario supplies it with the
  in-process comparison. Blobray
  evaluates it with `unclassified: required`: analog I2C transport reads,
  read-mask and host-map writes, and the single-microsecond wait before a
  transport or declared status read are ignored plumbing; every other MMIO,
  fence and delay effect compares exactly, in order and value. Because every
  read compares, both sides necessarily consume the same never-written
  registers. Each verdict carries the `reviewed-effect-refinement` claim.
  The TX-DC root issues no analog I2C command and the RX-gain root's commands
  never wait, so their contracts carry only the reviews their cases exercise.
- Every scenario counts, through Blobray's own effect classification, the
  effects each contract rule selects and writes the counts to
  `rules-<scenario>.txt` beside its run. A review (one disposition and
  reason, possibly instantiated per register and shared by several
  contracts) that selects no effect anywhere in the scenario fails it: the
  review is stale or no case exercises its path.
- ROM storage addresses name their ROM symbols (`rom_phyFuns`,
  `phy_param_rom`, `g_phyFuns_instance`) and are checked against the captured
  ROM inventory at session start.

Archive and ROM identities are the same pinned inputs as the I2C runner; no SDK
companion is needed. The gain object and its complete 216-byte coefficient
section must match independently extracted SHA-256 identities before comparison.
The callback's actual three `memcpy` source ranges are checked against those
captured bytes, and its real ROM kernel arguments include signed narrowing and
stack ABI words. The entry adapter supplies sixteen explicit words and initializes
saved registers; it supplies no gain algorithm or callback result.

The finite matrix contains 150 callback/kernel input combinations, 360 Wi-Fi
production arithmetic combinations, 24 complete Wi-Fi publication combinations
and 120 Bluetooth combinations. Another 72 calculation cases select every one of
the 18 Wi-Fi and 18 Bluetooth coefficient thresholds exactly; the original matrix
alone does not select every interval in the production path. All counts include
both declared stack fills. Every
Wi-Fi calculation checks all 160 output bytes; Bluetooth checks all 80. An
independent interval-selection oracle consumes the authenticated coefficients.
Separate vendor-only observations retain the distinction between current additive
Wi-Fi adjustment and the older ROM callback's subtractive attenuation and tables;
these observations do not assert equivalence of the two profiles.

Bluetooth executes the real `phy_get_romfunc_addr` installer and follows the
installed callback during full publication. The explicit ROM pointer names
captured writable BSS storage; no synthetic callback table supplies the result.
The pure production calculation borrows ordinary arrays, while complete Wi-Fi/BT
publishers acquire the shipping radio capability. A retained index register and
three passive data ports model only peripheral storage. Independent instruction-
derived expectations require all 161 Wi-Fi or 81 Bluetooth MMIO events, including
bank selection, index wrap and preserved control bits. No MMIO events are excluded
from publication comparison; void return values and physical call addresses are
not equivalence criteria. Raw call observations remain retained.

The runner batches bounded native requests, each with explicit cold/warm phases.
Selected RAM and every MMIO/fence/delay event form the native comparison relation.
Unknown curve data and missing callback installation must be INCOMPLETE. A changed
caller-supplied coefficient at the direct ROM boundary must produce DIFF and a new
execution identity; this is a vendor-boundary characterization. Event exhaustion
must fail with a resource limit. These are software gain-child comparisons, not RF,
whole-TXCAL, Wi-Fi or BT/154 protocol qualification.

### Gain state and RF-test power producer

The same scenario characterizes captured calibration storage and the RF-test
power policy ([`gain_state.rs`](scenarios/src/phy/gain_state.rs)). These are vendor-only executions: no
production storage or RF power API exists, and none is inferred.

Storage always runs, because it needs only the pinned archive and ROM. The
captured image's startup adjustment byte must be zero. For both stack fills and
adjustments 0, 1, 31, 127, 128 and 255, warm phases execute the real backup,
destruction of live state, recovery, parameter registration, curve/base
isolation, restored-adjustment consumption and consumption after moving the
value to the base. Independent expectations check the 532-byte backup image,
the exact registration writes and every 160-byte gain output against the
coefficient oracle. No hardware event occurs.

`--rftest` supplies `librftest.a`, by default the `librftest` pin of
`artifacts.toml`. Its `set_rate_power_index` and `mac_power_set` are linked
as additional roots; no power or gain callee is substituted. The real callback
installer runs, then fifteen policy rows per fill check rounding, saturation,
signed-byte wrap, the adjustment byte, conditional `phy_wifi_set_tx_gain_new`
publication with all MMIO events, and both MAC index writes.

Negative cases are:

- corrupted saved adjustments on both sides propagating to a DIFF;
- recovery from an unknown cache, which is INCOMPLETE;
- a missing callback installation, which is INCOMPLETE and publishes neither gain
  nor MAC state;
- an unknown policy input, which stops at the exact byte read and blocks later
  phases;
- event exhaustion, which fails with a resource limit.

## Coexistence schedule comparison

The `coex` scenario ([`coex.rs`](scenarios/src/coexistence/coex.rs)) compares the time-slice
schedule of the pinned `libcoexist.a[coexist_scheme.o]` with the production
`CoexSchedule` through the `open_coex_schm_trace_step` probe. A cold setup case
points the ROM cell `g_coa_funcs_p` at a modeled adapter table (semaphore
take/give, timer disarm and `timer_arm_us`); before each compared case the ROM
`memcpy` writes one schedule state over the vendor `coex_schm_env`. The probe
receives the same state as a vendor-shaped image together with the linked
address of every `coex_schm_<name>` scheme, so both sides report the selected
scheme as the same pointer; schemes the linker drops keep unique unmapped
addresses.

Each case enters one schedule entry: `coex_schm_status_change` over every
combination of the tested Wi-Fi, BLE, classic-Bluetooth, external-coexistence
and IEEE 802.15.4 status values; `coex_schm_status_bit_set` and `_clear` from
every Wi-Fi state with and without BLE; and `coex_schm_timeout_process` and
`coex_schm_process_restart` at every phase of every linked scheme under
looping and non-looping status. The relation compares the scheme pointer, the
phase index and the five status words; the scenario then requires the
production step to re-arm the phase timer for the vendor's `timer_arm_us`
microseconds and to notify exactly the phase callbacks the vendor called. The
static entries are entered at their linked addresses, where their claims
name them too.

```console
cargo verification scenario --chip esp32s31 coex \
  --production target/verification/esp32s31/oer-esp32s31-probe-radio-elf \
  --linker "$LLD" --output target/blobray-research/coex
```

## BLE PHY register initialization

The `bluetooth` scenario compares the pinned `r_sym_ble_nENHlP4KBuQYlFVffaR5`
(`r_ble_phy_init_registers`) with the production transaction through
`open_ble_phy_register_init_trace_r_ble_phy_init_registers` over timing bytes,
environment and resolving-list bases, the `0x20101470` branch and
configuration words. The vendor reads those inputs from linked globals, which
each case seeds; its scheduler-timing and ETM resource bookkeeping calls are
answered without effect. The vendor routes modem ETM channel zero from event 8
to task 20; IEEE 802.15.4 owns that channel here, so production routes channel
two instead. Three reviewed replacement rules pair those writes, production's
closing device fence is the one added effect, and every other register access
compares exactly.

## Bluetooth low-power clock

The `bluetooth` scenario ([`ble.rs`](scenarios/src/bluetooth/ble.rs)) also links the
ESP-IDF `libesp_hw_support.a` and `libhal.a` of the reference build and
compares `modem_clock_select_lp_clock_source(PERIPH_BT_MODULE, MAIN_XTAL, 399)`
with the production lease selection, and
`modem_clock_deselect_lp_clock_source(PERIPH_BT_MODULE)` with the production
deselection transaction, over each fill of the retained radio register block.
The driver's FreeRTOS critical section and its sleep power-domain bookkeeping
are answered without effect; every modem clock register access is compared
exactly. The driver's HAL context is copied into the linked image with the
`MODEM_SYSCON` and `MODEM_LPCON` bases of the pinned firmware, because Blobray
binds no linker-script absolute symbol.

## Wi-Fi low-power clock

`wifi-mac` links the same two archives and compares
`modem_clock_select_lp_clock_source(PERIPH_WIFI_MODULE, source, 0)`, the call
of ESP-IDF's `esp_perip_clk_init`, with production
`RadioPhyRegisters::select_wifi_low_power_clock` for the RC slow oscillator and
the 32 kHz crystal ([`low_power_clock.rs`](scenarios/src/wifi/low_power_clock.rs)).
The driver's record of the selected source and the Wi-Fi power clock
reference count are reviewed state differences: production selects once at
the first cold radio power-up and never deselects.

## Wi-Fi A-MPDU completion

`wifi-mac` compares `libpp.a[pp.o]::ppResortTxAMPDU` with the production
retained A-MPDU owner ([`ampdu_resort.rs`](scenarios/src/wifi/ampdu_resort.rs)).
Both sides hold the same encoded QoS MPDUs. The vendor side starts from the
queue record `ppTxqUpdateBitmap` leaves: the BlockAck starting sequence and
bitmap, the resorted aggregate's `esf_buf` chain, and each MPDU's eight-byte
A-MPDU metadata, whose word 0 carries the PSDU length and, in bits 16..24, the
sequence number's low byte that the resort measures against the starting
sequence. The production probe commits the MPDUs to `RetainedDmaAmpduTx`,
publishes them through a hardware double that returns one completion with the
same BlockAck result, observes it through `AmpduRetryState`, and applies
`retain_for_ampdu_retry` when the aggregate is retained. Each case compares
every MPDU's header afterwards, so an MPDU kept for retry shows the Retry bit
on both sides and an acknowledged MPDU shows none. Rate control, recycling,
the next transmission and the BlockAckReq are answered calls.

The cases cover every MPDU acknowledged, none, holes, a missing tail, a
starting sequence beyond the window, sequence wrap from 4095 to 0 and a single
missing MPDU kept in the aggregate, all MATCH. Two reviewed differences must
DIFF at exactly the named MPDU's Retry bit: production acknowledges the MPDU
immediately left of the starting sequence, which the vendor retransmits, and
production hands a single missing HT MPDU to its ordinary retry owner. One
known gap is checked: production sends no BlockAckReq, while the vendor
(`ppFillAMPDUBar`, `ppReSendBar`) requests a BlockAck starting after the
aggregate head when a resort discards that head as aged, or acknowledges it
while the station has a request pending for the TID. Rate control sets that
pending bit when it resumes aggregation (`trc_onAmpduOp`). The vendor sends
that request only while the aggregate keeps an MPDU after the resort, and a
later MPDU discarded as aged in the same resort cancels it; cases with the
head and later MPDUs aged, or with nothing left, check that no request is
sent. The cases check the request's TID and starting sequence.

The retry bound is compared too. Both sides keep a missing MPDU in the
aggregate without counting publications (MATCH with the vendor's descriptor
counters exhausted), bounded only by the MSDU lifetime: cases execute the
vendor's own `lmacMSDUAged` after `lmacInit` installed its lifetimes, which
the run reports, and give production the same lifetime; a fresh MPDU is kept
and an expired one discarded on both sides. One reviewed difference, checked
by a head queued long before the rest (DIFF at the head's Retry bit):
production starts an MSDU's lifetime when its aggregate is
committed, the moment the MSDU enters the radio, while the vendor starts it at
the pp queue enqueue timestamp; frames waiting in production's software queue
before commit do not age, so under queueing delay a production MSDU is
discarded later than the vendor's by that delay.

Completions without a BlockAck compare as sequences: after the vendor's
`lmacInit`, each warm phase applies one timeout to the vendor aggregate and
to the production aggregate the probe keeps in flight, compares every MPDU
header, and checks that the vendor retries exactly while production continues.
A CTS timeout sends no MPDU and neither side sets the Retry bit; an
acknowledgement timeout sets it on every MPDU on both sides. Both end at the
vendor's short retry limit, which its rate record's publication limit does
not undercut for the compared rate (MATCH). There the vendor ends the frame
exchange and recycles the aggregate, as production does, except for an
RTS-protected aggregate whose CTS never arrived: `lmacEndRetryAMPDUFail`
keeps it and sends a BlockAckReq starting at its head, whose BlockAck drives
the ordinary resort, while production ends it (known gap).

Each case also checks where both sides leave the MPDUs: the vendor by its
queue record (kept in the aggregate, handed to the ordinary queue, or
ended), production by its decision. They agree except for the reviewed HT
single-MPDU difference. When the BlockAck agreement is no longer operational
at the resort (`trc_isTxAmpduOperational`, `trc_tid_isTxAmpduOperational`),
both sides hand the missing MPDUs to ordinary transmission, the vendor
converting a missing aggregate head through `ppHEAMPDU2Normal`; the vendor
sets their Retry bit in the resort and production on the copy it
republishes, a reviewed DIFF at the first missing MPDU's Retry bit. A
Trigger-based success that ends an S-MPDU, which the vendor resorts without
reading a BlockAck, ends the aggregate without a Retry bit on both sides
(MATCH against production's Trigger-flow completion).

Production writes zero in the metadata bits the
vendor fills with the sequence number's low byte: the vendor writer of that
byte and whether hardware reads it are not established, so it stays an
unexplained difference.

## Inspecting vendor code

Two subcommands read every pinned code artifact of the manifest (archives,
the ROM and the reference firmware images present in the checkout, each
authenticated against its pinned SHA-256) without running a scenario:

```console
cargo verification scenario --chip esp32s31 xref 0x2010d830 [END]
cargo verification scenario --chip esp32s31 show pm_on_isr_twt_wake
cargo verification scenario --chip esp32s31 fields 0x34 0
cargo verification scenario --chip esp32s31 prints 0x20101000 0x20101500
```

`xref` lists every load and store whose address a forward pass over the
function resolves inside `START..END` (one word without `END`): the
register and field names of the published bindings, the origin
(`artifact` or `artifact[member]`), the function and offset, and for a
store the bits it clears, sets to a constant or takes from a value the pass
does not know, such as an argument. `show` prints each definition of a
function with folded constants, named registers, relocation targets and the
string literals they address. `fields OFFSET...` lists the accesses to a
structure field reached through pointers whose last offsets are the given
ones, from any argument or symbol: `0x34 0` is the word at offset 0 of the
pointer stored at offset 0x34, printed as a path such as `a0->0x34->0x0`.
`prints START [END]` ties register bits to debug output: a call whose
argument register addresses a string with printf conversions is a print, and
each following argument that carries bits of a word read from the range, as
read or after shifts and masks, is listed with those bits, the shift and the
format text up to its conversion. The pass is intraprocedural: it merges the
states of the paths that join at a branch target and iterates loops to a fixed
point, but a value returned by a call, or an address computed from an index,
is not followed. The report crate owns these commands in
[`inspect.rs`](../harness/report/src/inspect.rs).

## Inputs and probes

Private vendor artifacts are explicit scenario arguments; they are captured into
ignored scenario storage and never enter the repository. Never commit vendor
binaries, disassembly dumps, unreviewed extraction artifacts or private paths.
Necessary recovered hardware tables belong in production source with provenance
and consuming-code verification, as defined by the
[source policy](../../docs/source-policy.md).

Build the Rust probe images with `cargo verification probes --chip esp32s31`;
`--list-roles` lists them without building. The scenarios consume the
`rust-artifact` production probe ELF. Normal builds use Cargo's parallelism; set
`OPEN_RADIO_ANALYSIS_BUILD_JOBS` only to impose an explicit local resource limit.

The open ESP-IDF IEEE 802.15.4 controller is ported from source and requires no
controller archive or ELF. Its closed baseband and coexistence functions have no
native vendor comparison.

## Scenario crate layout

[`scenarios/src`](scenarios/src) separates the shared engine from the domain
scenarios and the reviewed data:

- `engine/`: the chip-neutral [scenario engine](../harness/README.md)
  (Blobray session, comparison harness, artifact authentication, coverage,
  observation and state analysis, evidence shards) under its crate paths, plus
  the ESP32-S31 guest layout, reviewed PHY effect contracts and harness call
  edges;
- `phy/`, `wifi/`, `bluetooth/`, `coexistence/`: the scenarios of each domain,
  with `wifi/mac.rs` also owning the leaf-suite machinery the Bluetooth and
  coexistence leaves reuse;
- `decisions/`: the reviewed observation and state decisions, one module per
  kind. Their types and stale-decision checks stay in the engine.

The reviewed coverage decisions are data outside the crate, in
[`decisions/coverage.toml`](decisions/coverage.toml), so that a shard does not
record them as a scenario source.

Domain modules keep their crate-root paths (`crate::gain`, `crate::ble`) as
re-exports. The library's `run.rs` holds the scenario command line and the
dispatch that decides verdicts and writes shards. The library is the verdict
package and never depends on a report package. The binary is the entry
package `oer-esp32s31-vendor-scenarios-cli` ([`scenarios/cli`](scenarios/cli)),
which depends on the library and on every chip's one report package,
[`vendor-scenarios`](../harness/cli) (`vendor-scenarios esp32s31 <command>`):
it adds the reviewer commands of the [report crate](../harness/README.md) and
passes its reviewer to `run`, so neither changes a shard's sources.

## Coverage decisions

Every claimed vendor root reports the coverage of its closure: the code
statically reachable from the root through direct transfers and the observed
targets of executed indirect ones, excluding call models. Each uncovered block
or branch direction is either excluded by a reviewed decision in
[`decisions/coverage.toml`](decisions/coverage.toml), with its reason, or listed as
untriaged in the evidence index. Decisions currently exclude vendor runtime
helpers (diagnostic formatting, compiler arithmetic and copy helpers, prologue
millicode) as whole functions. A decision that excludes whole functions also
excludes, within one claim's closure, the functions no execution entered that
the closure reaches only through excluded functions: their code is
unreachable for the same reason. A decision on a closure function that is
fully covered fails its scenario.

Each decision has a `reason` and one or more places: a vendor `function`,
optionally with offsets `start` up to `end` or `diagnostic = true`. A shard
records the SHA-256 of the decisions with a place in one of its closure
functions, so editing, adding or removing a decision stales only the shards
whose closures it names; `cargo verification evidence --chip esp32s31 <scenario>`
recomputes only those shards.

A shard lists what its own scenario leaves untriaged, so a vendor function
several scenarios reach can appear in one shard while another scenario
covers the location. `cargo verification evidence --chip esp32s31 --untriaged`
prints the chip-wide set: the locations no scenario whose closures contain
their function covers or reviews. `cargo verification evidence` prints its
per-function counts after regenerating shards. An untriaged location has neither a case that
covers it nor a reviewed decision. A whole-function decision whose function
only produces diagnostic output sets `diagnostic = true` on its place.

A run with untriaged locations writes `untriaged-<scenario>.txt` beside its
results: each location's instructions, decoded and lifted by Blobray's RISC-V
decoder, with constants folded and addresses and bit masks named by the
published register bindings, the definitions of the registers its instruction
reads, and a candidate mark on a block that only calls diagnostic
functions, stores nothing outside the stack and reads nothing outside it
after its last call. The mark names how the block ends: an assertion that
spins after its output, an early return (behavior, not diagnostics), or
another path; a candidate shows its whole block. The report proposes; the
decision stays reviewed.

`functions-<scenario>.txt` shows each function with untriaged locations in
full, marking every uncovered location `U` untriaged, `E<n>` excluded by the
numbered decision whose reason follows the listing, or `C` reached only
through excluded functions; unmarked code is covered.

Beside it, `gateways-<scenario>.txt` names, per claim, the closure functions
no execution entered that alone lead to untriaged code, with how many
locations and functions each leads to: a boundary to declare, a decision to
review or a case to add removes what it leads to.

## Observation decisions

Every compared request also reports, through Blobray observation dependence,
which executed production instructions a compared observation depends on:
data dependence through registers and memory, control dependence on
conditional branches and the transfers that lead to observed code. The probe's
debug line information maps those instructions to production PHY source lines,
including inlined frames; a line is observed when any of its executed
instructions is. Each evidence entry counts the lines its executions executed,
observed, reviewed and left untriaged. A line executed but observed by no
scenario is either reviewed by a decision in
[`decisions/observation.rs`](scenarios/src/decisions/observation.rs) (file and trimmed source text,
with its reason) or listed as unobserved in the evidence index. A multiline
anchor ends at the reviewed line and includes its preceding context; that
context is not itself reviewed. The ordinary status-four TX-error dispatch
uses this to distinguish its `match detail` from the RTS-error dispatch.
Its selector-zero cases compare reaching the selected retry handler through
the stop goal, an observation not recorded by register-effect dependence.
A decision that
matches no unobserved line fails `all`. An untriaged unobserved line has no
case, compared relation or reviewed decision that accounts for it. Dependence is a necessary condition for a comparison to notice a
defect on a line, not a sufficient one.

## Point mutants

A point mutant confirms one finding without rebuilding the probe: each
`--patch TARGET[:ORIGINAL]:REPLACEMENT` replaces bytes of the loaded radio
probe image in every comparison that loads it. TARGET is a hexadecimal
address or `symbol+OFFSET`; ORIGINAL the bytes there in memory order, the
instruction the probe holds when omitted; REPLACEMENT hexadecimal bytes,
`nop` (no-operations over the original length) or `ret` (a return at the
target). The Bluetooth comparisons load the Bluetooth probe instead and take
only `--bluetooth-patch`, in the same form. Every mutant resolves against an
executable segment of its probe ELF before any scenario runs, so a failing
run means a killed mutant, not a bad patch; a mutant run writes no evidence
index. A passing run reports, per mutant, whether any comparison executed
it: a mutant that survives unexecuted says nothing about the comparisons.

```console
cargo verification scenario --chip esp32s31 all ... --patch 1001c6bc:a30aed00:13000000
cargo verification scenario --chip esp32s31 bluetooth ... \
    --bluetooth-patch open_bluetooth_trace_deselect_low_power_clock:ret
```

A mutant of an unobserved line that survives confirms the finding; one that a
scenario kills shows the dependence is real but indirect. Patches are for
named addresses, not a campaign. Radio patches apply to every scenario that
loads the radio probe, not only the PHY ones: a patch that drops the RX append
doorbell bit fails `wifi-mac` at the doorbell model, and one that changes the
HE-SIG-A2 control image fails its HE PPDU leaf as DIFF. A Bluetooth patch that
returns from the low-power clock deselection leaf at its entry fails
`bluetooth` as DIFF. A failing run writes its departing cases to the run's
`failures/` directory. When a side stopped at memory its case does not
declare, the report also lists every undeclared symbol, register or callee
the request reaches: the engine reruns it with each one found mapped as a
zero-filled placeholder, or a callee returning zero, until no side stops
there. The placeholders only locate the declarations a scenario still owes.

## Contract ownership

| Input | Responsibility |
| --- | --- |
| `artifacts.toml` | Pins of every vendor archive, ROM ELF and SDK firmware |
| `scenarios/` | Typed comparison scenarios, their expected verdicts and claim tables |
| `evidence/` | Committed native evidence index consumed by qualification, and hardware cross-check summaries |
| `probes/` | [Compiled calls](probes/README.md) into production code for comparison |
| `host/ieee802154/` | [Host stand](host/ieee802154/README.md) that compiles the public ESP-IDF IEEE 802.15.4 driver against recorded boundaries |
| `facts/` | Reviewed code fingerprints of cited vendor functions (`provenance.toml`) and [name maps](facts/names/README.md) |
| `hil-vendor/` | [Vendor firmware](hil-vendor/README.md) for hardware cross-checks |
| `hardware/` | [Boot-state firmware probe](hardware/boot-state-probe/README.md) in its own workspace |
| `../../hil/phy/esp32s31/calibration/` | The [calibration cross-check](../../hil/phy/esp32s31/calibration/README.md)'s comparison of vendor and production on the board; the HIL `phy` scenarios capture it |
| `../../hil/phy/esp32s31/relation/` | [The reviewed calibration relation](../../hil/phy/esp32s31/relation/README.md) the probes, the tracking scenario and the cross-check share |

The [technical references](../../docs/vendor/esp32s31/README.md) describe Bluetooth Controller,
DTM, advertising, scanning, connection and IEEE 802.15.4 boundaries. They do not
serve as run results or operational readiness declarations.
The [name maps](facts/names/README.md) give source names for generated function
names of the pinned Controller archive.

## Register publication

Register review and publication belong to the independent
[register tool](../../tools/registers/README.md):

```console
cargo registers generate --manifest registers/esp32s31/publication/registers.toml --check
```

Discovery observations feed the shared reviewed register model. Publication
produces SVD, raw PAC, closed PAC API and binding index from explicitly selected
inputs; observations do not invent hardware semantics. The source-only
composition validates the same publication outputs without private artifacts.

## Verification boundary

A scenario compares vendor execution with a compiled production probe entry and
checks each expected `MATCH`, `DIFF` or `INCOMPLETE`. Only a root that compared
with `MATCH` in every retained execution selecting it becomes an index entry.
The [qualification evaluator](../../qualification/README.md) is the readiness
authority; the index supplies vendor evidence only while its source digests are
current. For CLI concepts, see [Blobray](../../tools/blobray/README.md).

## Shared scenario preparation

[`harness.rs`](../harness/scenarios/src/harness.rs) and [`session.rs`](../harness/scenarios/src/session.rs)
own authenticated input capture, in-process inventory, linking, data export and comparison,
exact symbol selection and the shared memory/phase/comparison builders. The I2C, transport and calibration scenarios keep their independent
expected values and explicit peripheral assumptions.

The comparison runner exports `.blobray.probes` from the captured production ELF
through in-process inventory and data export. Every declared entry must resolve;
selected production calls must be catalog members. Named scalar arguments are
lowered with signed range checks. Reference parameters are explicit guest
addresses, not host pointers or inferred private layouts. Unsupported argument
types require an explicit adapter. `Buffer` declarations derive size and alignment for references to primitive
fixed arrays, including nested arrays, and generate memory regions automatically.
Addresses, byte seeds, lifetime, reset, models and observation relations remain
scenario choices. Opaque owner/layout types require explicit adapters. Word padding requires
both an explicit count and fill value; unknown memory stays unknown.

Every operation runs in the scenario's process over the authenticated input
bytes, identified by content: inventory, the probe catalog, linked images and
data exports are recomputed on each run and nothing is cached. Each linked
image's ELF, map and manifest are written below the scenario output for
inspection. The generated requests use the Blobray execution format and run
through Blobray's in-process verification; records stay in memory. A run keeps only its results: claim verdicts, case counts, coverage
and input/source digests. The combined I2C route also checks
[compiled call boundaries](scenarios/src/engine/harness_edges.rs) alongside positive and
negative PHY comparisons.

Run preparation-only regressions without private inputs:

```console
cargo test --manifest-path verification/Cargo.toml -p oer-esp32s31-vendor-scenarios
```
