# Runtime RFPLL correction

This component owns the finite measured capacitor search and conditional
frequency-memory update. Its source reference is `phy_rfpll_cap_init_cal_track`
and `phy_rfpll_cap_correct_track` of the pinned esp-phy-lib `libphy.a`
(`chips/esp32s31/vendor/artifacts.toml`).

`search::Search` issues helper operations and accepts their completions. It
keeps requested capacitor values separate from the helper's programmed value:
`phy_write_pll_cap` clamps negative signed inputs, while the search accumulates
the original unsigned candidate. The sum wraps at 16 bits. A status accepting
no candidate restores the initial requested value and returns a zero delta;
this is not proof of RFPLL lock.

```mermaid
flowchart TD
    read[Read initial capacitor] --> enable[Enable capacitor search]
    enable --> down[Search downward: initial minus offset]
    down --> up[Search upward: initial plus one plus offset]
    up --> select[Mean accepted candidates; initial value if none]
    select --> write[Program selected capacitor]
    write --> delta{Signed delta is nonzero?}
    delta -- Yes --> memory[Update all 85 frequency-memory entries]
    memory --> restore[Restore current channel index]
    restore --> done[Return correction outcome]
    delta -- No --> done
```

Each direction permits at most thirty status samples and ends early after two
direction-specific boundary observations. The observations need not be
consecutive. Every candidate status read occurs at least five microseconds after its
completed capacitor write. The final write has no additional search settle.
`Correction` consumes the search outcome and updates the existing frequency
memory with the measured signed delta, which is not restricted to ±2.

The memory child uses the layout installed by OER's cold initialization:
85 entries, base 32, stride 7. Vendor code reads that layout from hardware;
OER retains it as part of its initialization contract. Replacing that layout
requires changing the memory child as well. The adjustment retains the full
signed intermediate before splitting data and write mode for the typed PAC.
In particular, underflow can propagate into the mode byte, as in ROM. Matching
that behavior does not qualify an out-of-range correction on physical RF.

`target_port::rfpll::{search, correct}` uses the existing typed I2C bindings,
direct bounded status polling and frequency-memory operations. Required 2-us
entry settle uses the short ROM delay backend. The admitted transaction uses
the private P1 transport engine. A complete capacitor write returns `Pending`
through the waker before status is read. An `Instant` stamped after the write
measures the five-microsecond minimum: the next poll reads status immediately
if it has elapsed, otherwise the short ROM delay supplies only the remainder.
Each complete memory entry (read and write), setup operation, final write and
channel-index restoration also has a checkpoint. These voluntary yields
construct no timer. Grant protection and software frequency control remain
held across them; failure or cancellation cannot publish restoration.
The finite edge limits do not establish a wall-clock bound.

Cold/registration RFPLL I2C also enters the P1 Driver and the same outlined
start/sample atom. Its first and repeated completion observations retain the
one-microsecond wait and the original attempt budget. Direct admitted callers
use that atom with immediate status polling inside their named twenty-
microsecond settle. The two wait policies do not change each other's bounds.
There is no separate native asynchronous RFPLL I2C executor. The thermal
entry/restoration envelope is synchronous around the single correction await;
semantic commit remains after hardware restoration and grant release. The caller must
already hold exclusive physical PHY access. This child does not acquire a
coexistence grant, disable hardware frequency updates, change thermal reference
state, or authorize traffic to resume after failure; the thermal transition
around it does the first two.

`thermal::Transition` owns the temperature gate, the grant-protect bracket,
measured correction and current frequency-control envelope. As
`phy_rfpll_cap_track_new` does, it requests grant protection only after the
temperature gate admits work and withdraws it after hardware frequency
control is restored. `target_port::rfpll::track` executes it.
`maintain` requests unconditional work through that same transition, using a
zero threshold for diagnostic callers; it does not duplicate the envelope:

```mermaid
sequenceDiagram
    participant Owner as Exclusive physical owner
    participant PHY as RFPLL maintenance
    participant HAL as Typed HAL / PAC
    Owner->>PHY: maintain(current_channel)
    PHY->>HAL: Select software frequency control (baseband mode 2)
    PHY->>PHY: Await 2 microseconds
    PHY->>HAL: Read SDM counter, then I2C-number control
    PHY->>PHY: Search and conditional memory correction
    alt Successful correction
        PHY->>HAL: Restore hardware frequency control (baseband mode 0)
        PHY-->>Owner: Correction outcome
    else Hardware failure
        PHY-->>Owner: Error, no restoration write
        Note over Owner: Retain/quarantine physical owner
    end
```

The current archive uses the low two bits of `FREQUENCY_PARAMETER_1_STATUS`
at `0x20100028`; the ROM helpers instead change the disable bit of
`FREQUENCY_CONTROL` at `0x2010001c`. These operations are not interchangeable.
The existing baseband-mode accessor implements the new register update while
preserving all other bits. Success selects mode zero, as the vendor does;
it does not restore an arbitrary saved mode. Once started, `maintain` must not
be cancelled. On error its caller remains responsible for fault containment.

## Parent boundary and coverage

The current archive initializes the parent's RFPLL-enable byte to one and its
tracking child uses a default delta of 15 sensor units. That initializer is not
a proof that no linked component later changes the policy. The grant acquire
and release symbols in this archive are weak no-op definitions; the pinned
coexistence archive supplies the strong request/release that OER follows.
Vendor comparison of the PHY archive alone uses `WeakPhyGrantProtect`.

The current vendor child admits work when the absolute difference between
signed current and reference temperatures is **at least** the threshold and
its shared busy flag is clear. Debug bit zero selects the caller's byte-sized
override; a zero override admits even an unchanged temperature. Debug bit one
alone does not replace the RFPLL threshold. The difference is evaluated without
wrapping it back into a signed 16-bit temperature.

After a completed correction, including a zero correction with no accepted
capacitor samples, vendor updates its reference to the current temperature and
sets the RFPLL result flag. This indicates that the procedure ran, not that
lock was achieved. A skipped invocation preserves the reference and result
flags. Vendor publishes the reference and result flag before restoring hardware
frequency control, then clears busy. OER's registered parent must retain its
existing rule that semantic
completion cannot be published before checked physical restoration.

The outer RFPLL action lowers to this current thermal child. Its owner keeps
an exclusive borrow of `PhyState`; it cannot mint the outer completion token
until the child consumes hardware-control restoration. Only consuming this
terminal owner commits the reference temperature. Skip completion preserves
the previous reference. Errors before restoration cannot publish a successful
outer completion. The vendor busy flag is represented by physical admission
and exclusive ownership, not a second mutable lock inside this algorithm.

Registered OER policy enables the recovered RFPLL predicate. Compiled
comparisons include the RFPLL-enabled parent with zero, positive and negative
corrections. Physical qualification remains a separate readiness axis.
The ROM status-based correction is a distinct reference primitive with its
own default threshold and fixed ±2 correction; the outer RFPLL action no
longer selects it.

Host tests cover bounded search termination, direction changes, arithmetic
boundaries, completion ordering and channel restoration after memory updates.
The compiled probes `open_phy_rfpll_trace_search` and
`open_phy_rfpll_trace_maintain` invoke production target entries, including I2C
and delay operations. Typed vendor scenarios compare the search's I2C
commands and requested settling delays, and the frequency-control envelope
against the admitted vendor child with zero and nonzero corrections. Every
memory transaction and the final channel restoration are compared for the
installed layout. `open_phy_rfpll_trace_track` additionally compares threshold,
debug override, skipped/executed outcomes and the resulting reference against
the admitted vendor child. Vendor-only cases retain the busy/result-flag
characterization. Tests also
check that a bounded I2C timeout returns without restoring hardware control.

RFPLL's compiled requested-time environment assigns zero time to ready polls,
so it compares the complete five-microsecond delay. A finite clock-query prefix
still lets a stuck synchronous observation reach its timeout. Other observation
probes retain their one-microsecond ticks. Virtual-clock unit tests separately
exercise early and late RFPLL resumes; physical hold measures actual poll cost.
The comparison preserves every requested delay, including any unexpected
executor-added one-microsecond wait. It does not assert equal execution time or raw polling
traces. Alternative memory layouts, linked coexistence grants and physical RF behavior
require separate evidence. The validation-only full-parent profile is described
in the [tracking contract](../README.md).

Production runs this child only inside the periodic parameter-tracking graph,
under its normal thermal predicate; no protocol selects it alone.

## Terminal observations

The parameter target executor calls `PhyTargetObserver::rfpll_completed` only
after the child reaches its terminal state and commits its reference. The
value-only `Observation` retains the thermal request and exact procedure result;
it grants no access to the hardware. No callback runs on an unfinished or failed
child. The maintenance recorder accepts one such observation while RFPLL timing
is active and flags duplicate or out-of-scope records as invalid.

A missing correction means the thermal predicate skipped hardware work. A
present zero correction may have zero accepted samples; it is not a lock claim.
For a nonzero correction, the result includes the completed memory entry count
and restored frequency index. The selected capacitor is the requested helper
input, not readback of the clamped value. Temperature is the retained sample used
at invocation, not an implicit new sensor reading. These distinctions remain
explicit in a separate, correlated RFPLL detail record, preserving the bounded
main pause record. Normal production observers remain no-ops.

Observers opting into `OBSERVE_RFPLL_AGE` also receive the conservative sample
age at RFPLL entry, measured from acquisition start. Missing or invalid timing
is `None`; zero is never substituted. With observation disabled, this detail
adds no clock read. Age records provenance and does not authorize RF access.
