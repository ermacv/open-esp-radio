# Bluetooth LE and IEEE 802.15.4 facts (out of the build)

These experiments of the facts image drive or observe the Bluetooth LE or
IEEE 802.15.4 blocks and are out of the build with their radios: no module
declares their files, and the image answers their selections as unknown.
The sections below describe those files and the records they produced.

## BLE interrupt facts

BLE commands use disjoint PAC owners from the same root claim. They contain
hart 1, keep CPU interrupts masked, observe both MAC banks and matrix status,
and never publish a scheduler descriptor, start scheduler RUN or change a CPU
route. They do not run a radio protocol or require a peer. Production baseline
and arming policy are unaffected.

| Selection | Bounded experiment |
| --- | --- |
| 20 | Bank-one source-1 enable before clock setup, before baseline, and after applying/removing baseline; records clock and enable readbacks |
| 21 | Candidate BTLC cause, baseline masking/re-enable, separate controls 1/2 and their combination |
| 22 | Candidate arrivals between bank snapshots and before sampled acknowledgement |
| 23 | Near compare, software pending, bit-18 disable, retained-pending re-enable and zero acknowledgement |
| 24 | Pending compare, counter progression, `+0x04` image 1, then START and phase observation |
| 25 | One natural counter wrap without compare, bit-18 disable and rollover-byte zero |
| 26 | Half-range compare, all-ones disable, independent bit-1 command, positional modes and a past target |
| 27 | Bank-one source 1 enabled only in the probe for the local BTLC candidates; restores the inherited enable |
| 28 | Counter movement across XTAL divider images 399/1/0 and positional mode writes |
| 29 | Software-pending delivery across mode writes, disable/re-enable, zero, all-ones disable, bit-1 command and `+0x04` |
| 30 | One natural wrap plus software pending; independent suppression, retained-pending redelivery and acknowledgement of the two causes |
| 31 | Minimal sourced MAC/latch prefix and the BTLC candidate, without any DMA or RF publication |
| 32 / 33 | BLE-timer reset with LP clocks on / off; software-pending diagnostic, then one natural rollover with compare disabled |
| 34 / 35 | Production controller-domain reset order with LP clocks on / off; same two positive causes and separate suppression/stop/clear observations |

Reset leaves observe delivery before any post-reset enable/publication. The
software-only phase retains bit 18 enabled as a diagnostic; the rollover phase
disables compare before reset, matching shutdown admission. LP-off deselects
all four LP sources, clears the BLE-timer gate and the MODEM_PWR LP-timer gate,
and keeps the register bus and Wi-Fi-power gate live. Clock restoration sends
neither START nor a pending/enable command. Each state has three records:
`tag` = counter/status/matrix bank 3; `tag+1` = bytes 0x24/0x2c/BLE-reset;
`tag+2` = BLE gate/LP gate/XTAL selector. Tags 0x3220–0x3250 and
0x3340–0x3370 precede all post-reset pending/enable controls; 0x3380 onward separates
all-ones suppression, counter stop, pending clear and positive rearm controls.
There is one bounded natural wrap per command and no direct read of +0x10.

These commands are explicit selections, excluded from selection 0. An inactive
BTLC candidate does not establish an output equation or a set/clear race result:
the records must show a real pending cause before interpreting masking or ack.
The LP counter experiments use divider image 1, which advances the counter in
the tested setup; image 0 did not advance it. They establish no production-clock
frequency equation. Counter/cause polls end after 40,000,000 cycles, and selections 25/30/32–35 stop after one wrap. Multiple-wrap capacity and
collision priority are outside those selections' observations.

BLE leaves feed the existing ROM RTC watchdog through their typed PAC owner
before each experiment; they do not disable it. The named write-protect images
are in the register model. Debugger command dwell is not part of a measured
operation. A reset, zeroed output buffer or untriggered cause is not a passing
hardware claim even if the debugger command itself exits successfully.

## LP sleep and delivery lanes

Selections 110–114 observe MODEM_PWR sleep state and the known source-127
bank, initially enabling both modeled delivery commands (bits 1/18); the
individual leaves then apply their documented mask/enable phases. No unobserved
bit0/16/17 command at +0x10 is invented from the sleep-command positions.
The absence of such a sourced enable prevents an exhaustive cause-map claim.

| Selection | Bounded experiment |
| --- | --- |
| 110 | Sleep16/+64 crossing under each control0/4 image; persistence, +44 disable, +40 re-enable, +34 all-clear and fresh software delivery without a new enable |
| 111 | One real rollover plus software pending with the counter stopped; all four control0/4 images and each sleep0/16/17 command; delivery mask, separate enable commands, enabled +34 clear and fresh pending |
| 112 | Advancing completed BTLC prerequisite; +68 target before sleep0/17 commands and ETM companions, alone and with a positive sleep16 companion |
| 113 | Sleep0-only +68 crossing, independent sleep17 crossing, +44 retention, +34/zero clear, disabled crossing/late enable and fresh-target control |
| 114 | Near+6c compare under all four control0/4 images, control1 trigger, pending persistence and separate +34 clear |

Each state is four records: counter/cause/matrix bank3; bytes1c/20/24;
byte2c/compact mode58 controls0/2/4/matrix bank4; PMU HP RAW/LP RAW/wake cause. PMU RAW
records contain the modeled fields, read without acknowledgement. Context records add reject and
LP-CPU wake causes, compact wake/reject/LP-wake STATUS and their enables.
Only SOC_WAKEUP and SOC_SLEEP_REJECT (2 of the 5 HP enable fields), and
LP_CPU_WAKEUP (1 of the 12 LP enable fields) are sampled; other enables
are unobserved.
The read-only `PmuWakeStatus` capability reads the interrupt status, enable
and LP words; the raw HP status and the wake and reject causes are read,
never written, through the sleep request's `PmuSleepControl`. The
radio clock/power owner has no PMU wake enable or acknowledgement access. A change of a
local sleep flag alone does not establish a PMU sleep/wake transition.

Cause/target waits stop after 200,000 cycles. Selection 111 requests exactly
one natural wrap, bounded by 40,000,000 cycles. Persistence windows are
4096 cycles. Selection 110 emits at most 124 records in the 128-record buffer.
CPU interrupts remain masked; no CPU handler delivery is measured. Hart1
stays held. The SRAM loader and debugger capture must run inside an s31-c
stand lease; only command/result SRAM is accessed by the debugger.

CONTROL_1 is a write trigger. All callers use its generated fresh-read
trigger; diagnostic mode encoding omits that write-only lane.

Sleep17/+68 observations use advancing completed BTLC captures, live digital
and ETM clocks, ETM channels6/7 enabled and the ETM bit11 command issued.
None of those prerequisites is independently varied; their necessity and
sufficiency remain unknown. R1/R2 capture only low bytes of +1c/+20 and
mode58 controls0/1/2/4 (controls0/2/4 after the trigger-class correction).
Upper +1c/+20 bits31:8 and mode58 bit3/bits31:5 are not recorded; no claim
that only mode58[2] changes across the complete word follows.

### LP record tags

A state at `t` is four records at `t+0..3`, in the field order above.
Each `pmu_context(p)` is two records: `p` contains reject cause, LP-CPU wake
cause, and compact STATUS (HP wake=1, HP reject=2, LP CPU wake=4); `p+1`
contains the three individually sampled enable booleans in that same order.
Context bases are 0xcff0 (110), 0xdff2 (111), 0xeff0/0xeff2 (112),
0xaff0/0xaff2 (113), and 0x9ff0 (114).

For loop index `i=0..3`, the control0/4 configurations are 0/1/0x10/0x11;
`i` itself is result metadata, not a register image. Leaf tag maps:

| Selection | State bases and phases |
| --- | --- |
| 110 | `b=0xc000+i*0x100`; offsets 0x00 before command, 0x10 after crossing, 0x20 after persistence window, 0x30 after +44 disable16, 0x40 after +40 enable16, 0x50 after +34/window, 0x60 after fresh software pending/window |
| 111 | 0xd000 positive stopped-counter pair; 0xd010/20/30/40 control0/4 indices0/1/2/3; 0xd050/60/70 enable sleep0/16/17; 0xd080/90/a0 disable sleep0/16/17; 0xd0b0 all-delivery mask; 0xd0c0 enable1; 0xd0d0 enable18; 0xd0e0 +34/window; 0xd0f0 fresh software pending/window; 0xd100 zero24/window |
| 112 | `b=0xe000/0xe100` without/with sleep16 companion; offsets 0x00 before commands, 0x10 after crossing, 0x20 after window, 0x30 after sleep0/17/16 disable |
| 113 | 0xa000/0xa010 sleep0-only crossing/disable; 0xa100/110/120/130/140 independent sleep17 crossing, disable17, +34/window, zero20, late enable17/window; 0xa200/210 disabled crossing/late enable17; 0xa300 fresh enabled-target positive control |
| 114 | `b=0x9000+i*0x100`; offsets 0x00 after compare crossing, 0x10 after CONTROL_1 trigger, 0x20 after window, 0x30 after +34 clear |

Selections112/113 prepend the digital BTLC prerequisite's 0x9000–0x90e0
records, followed by LP initialization tags0x2f00/0x2f01. Selection114's
0x9000 bases belong to its own leaf and are not prerequisite records.

`b+0xf0` records `(origin_counter, programmed_target, metadata)`:
metadata is the control0/4 loop index for110/114, the companion16 boolean
for112, and zero for113's 0xa000/0xa100/0xa200/0xa300 target phases.
For112 the target is +68 at origin+1024 and the +64 companion is origin+512;
other target phases use origin+512. Arithmetic wraps in the modeled low
counter width.

`b+0xf1` records `(crossing_budget_passed, sampled_counter, elapsed_cycles)`.
The finite poll watches counter distance >=1024 (110/113/114) or >=2048
(112), under the 200,000-cycle bound. Passing proves counter progress past
the programmed target, not a positive pending flag or IRQ. Those require
the subsequent state records. On timeout the leaf cleans up and returns;
completion alone does not qualify a cause.

Selection111's 0xdff0 instead records `(wrapped, sampled_counter,
elapsed_cycles)` for its one natural-wrap poll under 40,000,000 cycles.
The later positive byte2c check remains an independent precondition.

## LP conditional acknowledgement and rearm

Selections180–182 are finite source127 experiments with CPU interrupts masked,
XTAL/divider1, CONTROL0=1 and CONTROL4=0. They use the session's existing PAC
owners, no RF, scheduler RUN or DMA. Selections184–189 are unassigned.

| Selection | Operation |
| --- | --- |
| 180 | Once each: sample0/arrival/skip-store; sample1/arrival/zero-store; sample1/zero-store/arrival. A software cause seeds the old1; the independently cleared CONTROL2 latch witnesses the new hardware compare. |
| 181 | Exactly two natural wraps without intermediate ack; retained first-wrap nonzero sample conditionally authorizes zero after the second crossing. Each wait stops at40M cycles. |
| 182 | Positive software cause/conditional ack, then targets at origin+2048, origin+64 and origin−64; each observation stops at200k cycles. A final future target tests recovery after the past target. |
| 183 | Width observation: baseline upper0, each bit24–31 individually, then all upper bits; one fresh origin+2048 target per image in each CONTROL0 state. Stop on the first missing compare; each crossing poll stops at200k cycles. |

State tags encode `(counter, cause, matrix_bank3)` and tag+1 encodes
`(byte24, byte2c, CONTROL2)`. Compare bases are0x18000/18100/18200:
offset0 is initial state; +2 is `(target, counter_before_enable,
counter_after_enable)`; +3 is `(saved_byte, counter_before_sample,
latch_after_sample)`; +4 is `(arrival_before_ack, counter_before_ack,
counter_after_ack)`; +5 is `(ack_bracket_cycles, store_taken, target)`;
+6 is eventual-latch success. State offsets10/20 are after ack and after
4096 additional cycles. No output records occur between the saved byte read
and conditional ack; the deliberate latch poll separates those phases.

Wrap states0x18300/18310 precede ack after crossings1/2; 18320/18330 follow
ack immediately/after4096 cycles. Tags18340/18341 are `(saved_byte, wrapped,
wait_cycles)`; 18350/18351 are `(last_counter_before_wrap, first_counter_after,
counter_after_ack)`. Rearm bases18400/18500/18600 use state offsets0/10;
+2 is the enable bracket above; +3 is `(saved_byte, origin, modular_offset)`;
+4 is `(progressed4096, current_counter, poll_cycles)`. Final recovery uses
18700/18710 and18702; 18703 is `(latch_success, counter, 0)`.
Common180f0 records `(CPU_root, integer_divider, fractional_numerator)`;
180f1 records `(CPLL_feedback_divider, reference_divider, counter_max)`.

Completion is not success: require sample-before-target/latch0, a positive
subsequent latch/crossing, and the recorded byte/cause/output transitions.
These phased orderings do not establish simultaneous set/clear priority,
arbitrary wrap capacity, product IRQ latency or a production service bound.
Selection183 uses the same XTAL/divider1 setup with CONTROL4=0 and both
CONTROL0=0/1. It explicitly observes upper-bit write/readback and future
compare delivery. Tags are `0x18800 + mode*0x200 + trial*0x10`: offset0 is
`(written_image, readback_image, origin)`; +1 is `(counter_before_enable,
counter_after_enable, progressed4096)`; +2/+3 are the state pair above;
+4 is `(initial_byte24, initial_CONTROL2, poll_cycles)`. Trials0/1–8/9 are
upper0/bits24–31/all upper bits. Tag18ee `(mode,trial,image)` is a failed
precondition or missing compare even with the ordinary completion marker.
At most104 records and20 distinct conditions; no wrap wait, statistical
baseline or production low-power clock equivalence is claimed.
Silicon evidence: `oer-notes/research/hw-lp127-ack-race.md`.

## Awake BLE LP wake-route observations

Selections 170/171/172 use one near compare / sleep16 / sleep17 target crossing
in each CONTROL4 state, with CONTROL0 set. Each target is 2048 low-counter
ticks ahead; the crossing poll stops after 200,000 cycles, and observations
after a bit change or clear wait 4096 cycles. There is no system sleep request,
RF, scheduler RUN, DMA or CPU interrupt admission. Sleep17 uses the existing
advancing-BTLC/digital-ETM prefix (channels6/7 and bit11); prerequisites are not
varied independently. Known +0x10 enables1/18 remain active. No other +0x10
lane or sleep0 completion read is invented.

Each mode emits five states: before programming, after a witnessed crossing,
after toggling CONTROL4 with pending present, after the CONTROL1 trigger,
and after the all-ones +0x34 clear. CONTROL4 changes preserve CONTROL0.
A positive flag/cause must be checked independently of command completion.
The next target uses the other initial CONTROL4 setting; these are distinct
register conditions, not repeated baseline captures.

State bases are `0x17100` (initial CONTROL4=0) and `0x17300` (initial
CONTROL4=1); phase offsets are `0/0x10/0x20/0x30/0x40`. At each phase:

| Tag offset | a | b | c |
| --- | --- | --- | --- |
| 0 | LP low counter | LP cause | matrix bank3 |
| 1 | low byte1c | low byte20 | low byte24 |
| 2 | low byte2c | readable mode58 image | PMU wakeup cause |
| 3 | all modeled HP RAW fields | all HP ST fields | all HP ENA fields |
| 4 | all modeled LP RAW fields | all LP ST fields | all LP ENA fields |
| 5 | PMU reject cause | LP-CPU wakeup cause | PMU WAKEUP_ENA |
| 6 | PMU last state | PMU target state | PMU current state |
| 7 | LPCON modem interrupt vector | all 14 modem wake enables | LP ETM wake flag |

`base+0xf0` records origin/target/initial mode; `base+0xf1` records crossing
success/current counter/poll cycles. Mode58 contains every modeled readable
field (0/2/3/4/31:5), excluding write-trigger bit1; it is not a full-word raw
read. PMU/LPCON images are reconstructed only from typed getters and generated
field geometry; reserved bits are unobserved. These snapshots neither
acknowledge PMU state nor change any wake enable. A zero PMU cause while awake
is not proof of absent silicon wake routing or a successful sleep/wake cycle.

## Inherited BLE enables and primary arrival windows

Selections 210–219 use the existing PAC owners without RF, DMA, scheduler RUN
or CPU interrupt admission. All cause/latch waits use the existing 200,000-cycle
BTLC limit. Commands remain serial under one board lease and one PAC claim.

| Selection | Experiment |
| --- | --- |
| 210 | All modeled enable fields before writes and after access clocks only; HP reset cause |
| 211 | Reserved; no qualified standalone production-admission sequence or callable leaf |
| 212 / 213 | Seed modeled feature/dynamic/task lanes, HP software reset / automatic next-ROM-entry observer before peripheral writes, then access clocks only |
| 214 | Empty saved pair, then one witnessed CC0 crossing before saved-pair acknowledgement |
| 215 | Saved CC0, then CC1 crossing before acknowledgement |
| 216 | Saved CC0, then a second CC0 crossing before acknowledgement |
| 217 | Saved CC0, then seeded controller-time overflow before acknowledgement |
| 218 | Synthetic inherited feature enables with live CC0, exact PHY task-bank prefix and retirement register suffix, then fresh CC0 |
| 219 | Mask primary banks and synthetic task17/task14; final observation |

Selection 213 is selected by its distinct LP retained record after a ROM
MEM_END handoff. No host reset may intervene after 212. The record retains
the exact three pre-reset enable words, not SRAM result-buffer retention.

The state record at each base tag contains full ENA0, ENA1 and task ENA,
reconstructed from typed getters and generated field geometry. `base+1`
contains dynamic source21, sources27/28 and source3; these are subsets of the
two primary words, not a fourth register bank. `base+2` contains primary
STATUS0/1 and matrix bank3; `base+3` contains NRT snapshots0/1 and matrix bank4;
`base+4` contains APB gate, MAC reset and APB reset readbacks. The first gated
sample alone cannot establish register state. Each word has all 32 bits
modeled; opaque bits retain their unknown meaning.

210 uses `2100` before access-clock setup and `2110` afterward. 212 retains
its `2160` enable triple in LP SRAM; 213 emits `2170` before writes, the
retained record at `2180`–`2183`, validity/reset cause at `218f`, and `2190`
after access clocks. The loader and access-clock prefix are explicit
differences from a pristine POR read and the production platform/RF lifecycle.
No complete production-admission result is established. Selection 211 is
excluded from the callable registry because its standalone parent prerequisites
are unqualified. The external report records the admission boundary and
observations.

214–217 emit `21a0` initial state, `21a8` initial CC0 crossing when applicable,
`21b0` saved pair plus cycle stamp, `21b8` fresh crossing, `21c0` observed
pre-ack state, `21c8` before/after saved-pair acknowledgement stamps, `21d0`
post-ack state, `21d8` fresh control crossing and `21e0` control state. Crossing
records contain origin, completed-capture endpoint and success. `21ee` means
failure (1 prerequisite, 2 initial cause, 3 new cause, 4 fresh control), never
a passing suppression result. 218 uses `2200` seeded positive, `2210` after
the PHY task mask/clear, `2220` after retirement suffix and `2230` after fresh
CC0. Its success crossing records are `220f` and `223f`. 219 ends at `2240`.

The new arrival must be visible before the saved W1C. These experiments test
an ordered sample-to-ack interval; exact same-cycle set/clear priority and
counting multiple same-bit arrivals are separate questions. No positive local
recipe for bank1 bits0/2 is supplied by these selections. SOURCE and silicon
results: `oer-notes/research/hw-ble-inherited-enables.md`.


218 additionally records controller/latch state at `2250/2260/2270`: request,
arm, shift; capture0/config24/compare enables; APB gate/MAC gate/reset bits.
`225f` records the failed crossing's elapsed cycles. `227f` records the
post-CONTROL0 request observation; `228f/2280` witness a fresh CC0 afterward.
The shared capture helper emits `91fd` only when its actual final poll still
has LATCH_REQUEST set at its deadline. A `223f=(0,0,0)` with no `91fd` is an
accepted zero origin followed by crossing-wait expiry, not a latch failure.
This restart is a probe diagnostic after observing release; production does
not use it as retirement or quiescence. SOURCE: `HW25_S31_BLE_LATCH_RESTART_20261027`.

## PB-1 bank1 and divider399 LP facts

Selections240–248 are explicit commands; request0 never runs them. They hold
hart1, mask CPU admission, publish no scheduler RUN or DMA and use no RF.
LP commands use the existing advancing-BTLC prefix, positional ETM6/7
(event46, tasks11/0f) and XTAL/divider399. CPU root0/divider0 with no
fractional numerator is required. They do not request PMU sleep.

| Selection | Finite operation |
| --- | --- |
| 240 | Separately seed bank1 enable0/2; baseline/dynamic clear, enable and mask stores; classic unchanged-image init/deinit stores |
| 241 | Sleep16 disabled crossing then a fresh enabled positive control, each target origin+2048 and observation at4096 ticks |
| 242 | Fresh low24 target68 crossing, sleep17 disable, then whole-zero20; no read of the WO target |
| 243 | Sourced sleep0 enable/disable helper and separate CONTROL25 clear/set, counter/reference brackets |
| 244 | Upper0, each bit24–31 and upperff with fresh low24 compares in sourced CONTROL0/4 clear/set pair states |
| 246 / 247 | The CONTROL0/4=1/1 width conditions alone / only remaining upper25–31 and upperff conditions |
| 248 | One natural wrap solely for post-wrap reference and CONTROL25 clear/set with bit26 held1; no WHOLE or width repetition |
| 245 | Software pending enabled/masked/unmasked and zero ack; exactly two natural LP wraps, with one zero ack between them; sleep0 reference observations near the first wrap |

Selection245 requires explicit authorization for its two approximately167.8s
natural waits. Each wait stops at190 nominal CPU seconds, accumulates wrapping
mcycle32 deltas into u64 and feeds the still-armed ROM watchdog once per
nominal second. No LP counter seed is written. Other counter-crossing polls
stop at4M CPU cycles (100ms at40MHz). Ordinary settles are4096 cycles.
Width preparation clears the sourced CONTROL0/4 pair before CONTROL1, then
restores the tested pair and requires old pending zero. The fresh pending
byte0→1 witnesses compare even if latch2 was already1; both latch values
are recorded without a new-edge claim.
Completion alone never establishes a positive cause. Tag24ee is a refusal or
failed precondition, independently of the completion marker.

Records240f0/1 contain CPU root/divider/fraction and LP divider/XTAL/counter.
Bank tags24100/24300 record enable0, enable2 and compact enables
(overflow, dynamic3, baseline8/9/12 in bits0–4). Offsets0/7/1/8/2/3/4/5/6
are seed, baseline clear/enable, dynamic clear/enable/mask, baseline mask,
classic init/deinit respectively. LP state tagT records counter/cause/matrix3;
T+1 records complete pending24/rollover2c/latch2; T+2 records
flag1c/flag20/LPCON vector. These full-word reads decode no undeclared lane.
Sleep16 bases25000/25100 record initial/final(+10) states, origin/target/enable
at+8 and counter-crossing bracket at+9. Sleep17 states26000/10/20/30
are initial/crossed/disabled/zero-acknowledged, with target+8/crossing+9.

Reference pairs record counter-before/reference-low24/counter-after, followed
by CONTROL25/bit26/ETM10c-bit26. Command blocks27000/27010 hold pairs before,
after and settled(+0/+2/+4);27020/22/24 isolate CONTROL25. Width bases
28000/29000 plus10*trial record written/readback/origin at+0,
origin/end/poll-cycles at+1, old-byte/old-latch/new-latch at+2 and
pending24/cause/matrix3 at+3. Whole bases2a000/2a100 use states+0/+10/+20/+30
(initial/published/unmasked/zero-ack);+20 exists only for the masked case.
Wrap states2a200/210/220/230/240 are initial/first/ack/second/ack.
Wrap journals2a2f0/2a3f0 contain last-before/first-after/elapsed-low;
+1 contains elapsed-high/deadline-seconds/reference-observed. First-wrap
reference blocks are journal+10/+20, in the same command-pair encoding.

Selection248 requires one explicitly authorized additional approximately167.8s
LP wrap. It uses the same190s bound and watchdog feed. The wrap journal is
2b0f0/1 and near-wrap command blocks are2b100/110. Post-wrap reference is
2b000/1, fresh enable command block2b010, CONTROL25-clear references2b020/22,
CONTROL25-set references2b024/26, with crossing brackets2b029/2a. This
isolates a sourced intermediate disable-helper state; bit26 remains1 during
that CONTROL25 observation. Completion requires the same positive advance.

## BT_MAC hidden pending and diagnostic reads

Selections 300/301/302 exercise local BTLC compare0/compare1/seeded overflow.
Each leaf validates both disabled-delivery edge observers with FROM_CPU2,
then uses the sourced digital BTLC setup, without RF, scheduler RUN or DMA.
Both BLE matrix routes are parked at MAP=0 for the first two clear tests.
MIE and both observer IE bits remain zero throughout.

| Tags | Observation |
| --- | --- |
| `32fe` / `32ff` | Cause index and actual route124/133 MAP readback, parked / observer phase |
| `3200`–`3240` | Initial state, fresh masked arrival, positive unmask, mask, retained redelivery |
| `3250` / `3260` | Single fixed W1C while enabled, then quiet reopen |
| `3270`–`3290` | Fresh masked arrival, fixed W1C without opening, then reopen |
| `32a0` / `32b0` | Fresh positive after mapping124/133 to validated edge inputs with IE=0, before/after three diagnostic reads |
| `32c0`–`32e0` | Mask, three more diagnostic reads while hidden, then reopen |
| `3300` | Fixed clear and observer-IP clear after the read observations |

Each state has six rows: primary bank0/1 and matrix3; NRT bank0/1 and
matrix4; observer IP bitmap/IE bitmap/prior MIE (read-and-mask); modem wake
vector/HP PMU RAW/wakeup cause; LP PMU RAW/selected PMU ST bits/ETM+10c[26];
and compare0/compare1/overflow enables. PMU ST packs HP wake, HP reject and
LP CPU wake in bits0–2. Only modeled RAW fields are reconstructed.
`32a8`/`32c8` retain three complete +4bc words; the following row gives BTLC
deltas before/across/after these reads. `32f0`–`32f2` retain each producer's
origin/end/completed observations. `32ee` denotes a failed prerequisite.
The fixed discards consume only this disposable session's timer indication
without changing its enable; production consumers do not call them.

Bank0 and NRT have no positive local producer here. The PMU and ETM rows
are context, with no positive event control; neither their zeros nor ETM
command-state readback establish absence of wake or event activity. A quiet
+4bc word does not qualify its own ungenerated task indications or all read
effects. CPU-delivery masking alone is not a peripheral-output fence.

The leaves positively observe all three bank1 indications: masked arrival becomes visible on enable;
fixed W1C clears while enabled or hidden; matrix124 and modem wake vector0x10
rise even at MAP0. The separate IE0 observer latches124. All eighteen +4bc
words were zero, the tested pending survives their reads and BTLC advances.
The wider read-safety, bank0/NRT and PMU/ETM-event questions remain unqualified.
SOURCE: `HW32_S31_*_20261027` in
`chips/esp32s31/registers/evidence/hw32-ble-masked-pending.toml` and
`oer-notes/research/hw32-ble-masked-pending.md`.
