# ESP32-S31 PHY architecture

This crate owns S31 RF algorithms, calibration state and finite hardware
transitions. It consumes typed HAL operations; it does not link the vendor PHY
library. Protocol runtimes own their MAC, descriptors, interrupt routes and
connection state. The radio system decides when those resources can be
withheld from normal traffic to admit a PHY operation.

The [capability matrix](FEATURES.md) distinguishes implemented primitives from
composed services. The [tracking contract](src/tracking/README.md) is the
canonical reference for demand, admission, cancellation and restoration.

The opt-in `lifecycle-fault-injection` feature exposes destructive one-shot
checkpoints in real maintenance. It owns no timer, reset mechanism or transport;
the caller must provide independent termination. Default builds omit its code.

## Terminology and phases

| Term | Meaning in this module |
| --- | --- |
| Initial calibration | Measurements and hardware setup performed during cold registration, before a protocol receives its operational radio owner. |
| Calibration results | Retained measurements and derived configuration. Having these values does not prove that hardware currently contains them. |
| Restore / replay | Re-establishing hardware from retained results after an invalidating power/reset transition. It is distinct from measuring new results. |
| Runtime tracking | The service of observing operating conditions, deciding which RF parameters need attention and executing admitted updates. It is an umbrella term, not a PLL-only operation. |
| Compensation | Computing and publishing an adjustment from an existing calibration model, such as temperature-dependent power/gain or analog-I2C band selection. It can still change hardware used by active traffic. |
| Recalibration | Repeating selected measurements or searches, such as common DCODE/RX gain or class TXDC/PWDET calibration. It does not necessarily repeat the full cold graph. |
| Maintenance | The execution interval and ownership transaction around tracking or another disruptive PHY operation. Demand for maintenance is not permission to execute it. |
| Packet reception adaptation | Per-frame AGC, carrier/frequency correction and channel estimation in the receive path. These are not the runtime temperature-tracking scheduler. |

RFPLL capacitor correction is named by its actual operation; it must not be
confused with complete frequency initialization or a proof of PLL lock.
Likewise, restoring MAC baseband enable is not restoring a protocol's DMA,
interrupt or connection epoch.

```mermaid
flowchart TD
    Power[Unique powered radio] --> Init[Cold registration and initial calibration]
    Init --> Registered[Registered PHY state and physical owner]
    Registered --> Role[Protocol operation]
    Role --> Demand[Read-only tracking demand]
    Demand --> Admission[Composition admits maintenance]
    Admission --> Track[Selected compensation or recalibration]
    Track --> Restore[Restore protocol hardware and checked release]
    Restore --> Role
    Init --> Cache[Optional calibration snapshot]
    Cache --> Validate[Validate schema, identity and complete products]
    Validate --> Init
    Track --> Fault[Failed epoch retained; reset required]
    Restore --> Fault
```

[Registration](src/calibration/registration.rs) admits a cache only when its
schema, physical identity, completion guards and RX table shape match. A valid
snapshot restores semantic calibration products and selects partial cold
calibration. The ordinary RF/baseband graph regenerates frequency memory and
republishes RX and TX gain state that cannot be assumed to survive a cold
epoch. An invalid or incomplete snapshot selects `FullAfterRejectedCache`.
Persistence belongs to the caller. Runtime tracking is not a substitute for
reset or retained-sleep wakeup replay.

## Owners and dependencies

| Owner | Responsibility | Boundary |
| --- | --- | --- |
| [Calibration](src/calibration/registration.rs), [analog](src/analog.rs), [RX](src/rx.rs), [TX](src/tx.rs) | Finite measurements, searches, ordered hardware actions and their results | No network sockets or protocol scheduling policy |
| [PHY state](src/state.rs) | Configuration, calibration results, temperature references and cache values | Values do not grant register access |
| [Client state](src/state/client.rs) and [tracking schedule](src/tracking/schedule.rs) | Active clients, source intervals and read-only demand | Wi-Fi and BT/154 are scheduling classes; BT and 154 remain distinct clients |
| [Registered domain](src/domain.rs) | One `PhyDomain` per registration: calibration state and the shared client set | A domain cannot be cloned, rebuilt or paired with another epoch; operations check the registration epoch before hardware access |
| [Concurrent domain](src/concurrent.rs) and its [target port](src/target_port/concurrent.rs) | The shared domain in the HAL radio domain; Wi-Fi, Bluetooth and IEEE 802.15.4 join it as clients | Every operation borrows the radio domain mutably |
| [Tracking graphs](src/tracking.rs) and [executor](src/executor.rs) | Execute selected children and validate their completions | No independent RF arbitration |
| [Target port](src/target_port.rs) and [HAL PHY](../hal/src/phy.rs) | Typed MMIO, analog buses, hardware completion and bounded waits | Hardware access is borrowed from the admitted owner |
| [Analog register bus](../../espressif/analog/README.md) | Field, configuration and parallel transitions of the vendor analog-I2C leaves | The transitions touch no bus; the target port and HAL drive them |
| The owner of the shared radio (none in this tree yet; the radio system) | The radio domain, the platform resources and the vendor periodic tracking timer | Tracking runs on the borrowed domain and the grant-protect brackets; it pauses no protocol |
| [HAL stopped-MAC check](../hal/src/owner/maintenance.rs) | The stopped Wi-Fi MAC check at the final-client boundary | A CPU mutex does not stop MAC or DMA |
| [Hardware coex control](../../espressif/coex/README.md) | Recovered timer requests, PTI, clock conversion and withdrawal accounting | Programmed timer identity is not an RF grant |

```mermaid
flowchart TD
    W[Wi-Fi client] --> L[Radio domain, owned by the radio system]
    Z[IEEE 802.15.4 client] --> L
    T[Tracking timer of the shared radio's owner] --> L
    P[PHY demand and calibration state] --> T
    L --> E[PHY finite executor]
    E --> H[HAL / semantic PAC]
    H --> RF[Shared RF / PLL / analog buses / baseband]
    E --> G[Grant-protect request]
    G --> C[Hardware coex timer bank]
```

PHY supplies the algorithms, demand and the concurrent domain; the radio
system owns the radio domain and runs the vendor periodic tracking (none exists in
this tree yet); coex supplies the hardware arbitration mechanism. Tracking follows the vendor
admission: the exclusive borrow of the radio domain is the PHY lock, and the tracking graph brackets
its RF-sensitive regions with the grant-protect request instead of pausing
the protocols. A coex timer reply, client bit or PTI is never an ownership
proof for the shared PHY.

The Wi-Fi airtime scheduler selects packet service within a MAC epoch. It does
not own the shared RF resource and cannot independently admit PHY maintenance.

## Power lifecycle boundary

Client bookkeeping and physical RF power are separate state transitions. The
last client leaving keeps RF, analog state and shared clocks physically
powered until the radio system closes RF; a closed domain admits no client
until it is woken.

```mermaid
stateDiagram-v2
    [*] --> Registered: register_concurrent_phy (first client)
    Registered --> Registered: clients join and leave
    Registered --> RfClosed: close_concurrent_rf (no client)
    RfClosed --> Registered: wake_concurrent_rf
    Registered --> Poisoned: started failure
    RfClosed --> Poisoned: started wake failure
    Poisoned --> [*]: reset required
```

RF close runs the current-vendor pre-close temperature observation and the
complete finite `phy_close_rf` graph: it disables hardware frequency control
and AGC, closes the RF and frontend/baseband domains
and publishes both retained analog close images. A preparation failure keeps
RF open; failure after close begins poisons the domain.

Retained wake executes the complete current-vendor wake parent in recovered
order without retiring the registration epoch: frontend/baseband and
analog-I2C power, retained frequency publication, channel and TX-cap state,
PBUS boundaries, PHY and AGC register updates, CKGEN reset, then restoration
of hardware frequency control, BBPLL, force-TX/RX and baseband mode. Every
started failure poisons the domain.


### Concurrent clients

For concurrently running protocols the registered domain lives beside the
shared registers in the HAL radio domain, as `RadioDomain<ConcurrentPhy>`.
Every operation borrows the domain mutably, so the PHY state and the shared PHY are
serialized by one mechanism:

- `register_concurrent_phy` registers the domain once through the domain's
  shared-PHY borrow. As ESP-IDF's calibrating first `esp_phy_enable`, it
  enables the `PHY` and `PHY_CALIBRATION` modem clock modules through the
  domain and releases `PHY_CALIBRATION` after calibration;
- `close_concurrent_rf` runs `phy_close_rf` after the last client left and
  releases `PHY`, as the last `esp_phy_disable` does. `wake_concurrent_rf`
  takes `PHY` and `PHY_CALIBRATION` again, restores the retained
  registration by `phy_wakeup_init` without calibration and releases
  `PHY_CALIBRATION`. A closed domain admits no client;
- `acquire_client` and `release_client` enter and leave Wi-Fi, Bluetooth and
  IEEE 802.15.4 as clients of the one client set;
  `wifi_client::join_wifi` and `leave_wifi` do this for Wi-Fi on the HAL's
  clocked Wi-Fi owner (`esp_phy_enable`/`esp_phy_disable(PHY_MODEM_WIFI)`),
  as `ieee802154_client` does for IEEE 802.15.4 and `bluetooth_client` for the
  Bluetooth Controller's task owner, each with its BTBB reference;
- `select_concurrent_wifi_channel` and `switch_concurrent_wifi_channel` tune
  the shared domain for Wi-Fi through the HAL Wi-Fi client's channel HAL,
  which it lends together with the domain's attachment; the switch stops and
  restarts the Wi-Fi MAC around the retune;
- `evaluate_periodic_tracking` and an acquisition that needs initial tracking
  leave the domain pending, and no client operation proceeds until it settles;
- `admit_maintenance` checks, without register access, that tracking is
  pending, and `maintain_concurrent_phy` then runs the tracking transaction;
  a started failure poisons the domain. The one admission follows ESP-IDF's
  `phy_common.c`: the periodic timer runs `phy_param_track_tot` under the PHY
  lock alone, without pausing any protocol. The exclusive domain borrow is that lock,
  and the tracking graph brackets its RF-sensitive regions with the
  grant-protect request. A protocol operation that loses RF to it ends with
  its ordinary outcome.
- `track_concurrent_phy` is one tick of the vendor `phy_track_pll`: it
  evaluates the clients' tracking period and runs due tracking at once. The
  vendor periodic timer that calls it belongs to the radio system that owns
  the radio domain, as it belongs to ESP-IDF's `esp_phy` component; nothing in
  this tree runs it yet.

### First and last PHY client

The owner of the shared radio chooses the PHY transition for a joining
client as the vendor `esp_phy_enable` does, on the radio domain:

- RF closed: `wake_concurrent_rf` (the retained registration is restored
  without calibration);
- a client set already exists: nothing, the domain is already open;
- otherwise: `register_concurrent_phy` with the retained calibration cache
  when one was handed over (`PhyRegisterConfig::with_calibration_cache`), else
  a calibrating registration; a registration refused as
  `ConcurrentPhyError::AlreadyRegistered` counts as already open.

A client then proves the domain ready (`phy_ready`) on the radio domain it joins
with. Wi-Fi prepares the domain before it enters power, which a cold RF
restart requires. The last client's half of `esp_phy_disable` closes
RF (`close_concurrent_rf`) only when the domain is open and its client set is
empty; a `ConcurrentRfError::Recoverable` failure keeps RF open, any other
started failure requires reset.

IEEE 802.15.4 composes these steps in [its client module](src/ieee802154_client.rs).
`join_ieee802154` runs on the HAL `Ieee802154Clocked` owner and consumes a
`PhyReady` proof (`phy_ready`: the domain registered, RF open and settled,
its registration describing the domain, which the proof borrows until the
join gives it back); every client's entry into the client set consumes one,
so no client enters a domain that is not ready. It acquires the client, takes the
shared BTBB reference with the gain byte projected from that registration and
applies the transmit-on delay, issuing an affine `Ieee802154PhyMembership`.
`RegisteredIeee802154Operational` keeps the membership beside the operational
MAC owners; they return through a foundation readback, not a policy readback,
because the operational MAC rewrote the PIB. `leave_ieee802154` releases the
client and the BTBB reference, neither with register access.

IEEE 802.15.4 stays a PHY client while it sleeps. On the ESP32-S31 the
vendor `ieee802154_rf_disable` closes RF only together with the REGDMA PHY
retention (`esp_hw_support/lowpower/port/esp32s31/sleep_phy.c`), whose I2C
burst the pinned `libphy.a` does not provide
(`phy_ana_i2c_master_burst_rf_onoff`); without it the vendor keeps RF open
for the client's lifetime. RF closes only when the last client leaves.

Protocol runtimes must first return their real TX, RX DMA, IRQ, MAC/LL and
per-protocol receive-enable owners to the radio system. Consequently neither
`PhyClientState::release`, a zero client mask nor a coex request can call the
physical close transaction on its own. Failure after close begins poisons
the domain; the radio then requires reset.

## Conditions and cadence

[Registered-policy inspection](src/tracking/inspection/README.md) reports
scheduling demand separately from the current RFPLL, power, analog-I2C and
common/class calibration conditions. It makes no MMIO calls. Runtime sensor acquisitions have monotonic
start/end provenance; undated replacements invalidate that provenance. A due evaluation is not an instruction
to perform all heavy calibration. No hourly/daily calibration schedule or
qualified maximum thermal deferral is implemented.

## Runtime sequence and timing

A completed common RX calibration carries
[`PhyRxGainDcQuality`](src/rx/gain_calibration/quality.rs) alongside its
coefficients. It identifies convergence for every shared/Wi-Fi baseband gain
and the five `phy_rxdc_fine_cal` radio searches after Wi-Fi gain zero.
Exhausting a baseband search retains its initial pair; fine searches retain
their last correction. These vendor-compatible outcomes complete the transaction,
but do not mean every search converged. Completion flags describe lifecycle;
quality remains a separate result. A skipped DC branch has no new quality.

The source-derived tracking graph contains optional RFPLL work, BT/154 power
and class calibration, Wi-Fi analog-I2C/power and class calibration, followed
by temperature sampling. Decisions in this graph use retained temperature;
the final sample must not be interpreted as the input to every preceding
decision. Temperature sampling can itself adjust the sensor's analog range.

Read-only demand neither advances timestamps nor refreshes calibration results.
`Schedule::At` supports an absolute timer; `Schedule::Inactive` requires none.
Deferral retains the outstanding due instant. The execution path re-observes
the active clients, and only matching child completion commits branch results.
The source interval is not a measured maximum thermally safe deferral.

The vendor periodic tracking is the only runtime maintenance path: the
shared radio's owner runs one `track_concurrent_phy` tick every tracking period
on the radio domain, and due work starts at once (no owner in this tree
runs it yet). There is no
protocol-selected maintenance of individual operations. Joint
Wi-Fi/Bluetooth/154 maintenance and active-traffic publication of individual
compensation classes are not qualified execution modes.

The production RX-gain and TXDC/PWDET paths execute as direct blocking
transactions on the already-borrowed radio domain. Required 1–20 us
hardware settles use the ESP32-S31 ROM `ets_delay_us` primitive used by the
recovered vendor graph; bounded PBus, I2C, DC/IQ and SAR readiness reads poll
directly. Interrupts remain enabled and the physical owner remains held until
terminal restoration. The complete common/Wi-Fi runtime calibration graph
runs in one caller poll without a timer suspension. Longer registration,
tracking and lifecycle operations wait on the caller's `oer_time::Timer`
through `executor::wait::delay` and its `executor::wait::Kind`.

PBus, analog-I2C and TX-DC/PWDET readiness complete inside named 20 µs
settles on the caller's clock. W2 measurements on s31-a exercised thermal
calibration, RFPLL tracking and retained RF wake; the longest direct edge
was 2,976 cycles at 160 MHz (18.6 µs). Timeout remains a typed failure.
The retained-wake frequency-ready edge already belongs to an asynchronous
transaction and retries through the timer in 10 µs steps.

The execution limits have different scopes:

| Mechanism | Established bound | Limitation |
| --- | --- | --- |
| [Direct bus polling](src/executor/wait/poll.rs) and [target executor](src/target_executor.rs) | Finite observations per PBus/I2C operation; typed failure on exhaustion | Requires each MMIO access to return; nested operations have separate observation budgets |
| [DC/IQ estimator](src/calibration/estimator.rs) | Finite readiness samples, then measurement/start cleanup on missing-ready | Cleanup and settle calls must themselves return |
| [Calibration executor](src/target_port/calibration.rs) | Root operation accounting and finite gain-state loops | Operation counts do not bound IRQ interference or a stalled bus access |
| [RFPLL search](src/tracking/rfpll/search.rs) | Both search directions terminate after finite samples, including unrecognized status values | Its finite state graph still invokes synchronous bus and settle operations |

The BLE radio owner arms the independent SoC deadline service around
physical lifecycle transactions and keeps the radio domain until checked restoration.
The board/application supplies explicit engineering budgets; PHY owns no timer
or reset peripheral. Hardware reset does not by itself establish a qualified
RF-cessation bound. A pending-future timeout, observed calibration maximum and
finite loop count cannot establish that stronger guarantee either. Failed or
expired work cannot manufacture runnable ownership.

Timing qualification must cover the complete action-to-hardware path, including
executor-added waits, readiness retries, failure cleanup and the selected
runtime adapter. Every wait of at most 100 us requires hot-path classification,
with waits of at most 20 us reviewed first. Record the required minimum,
first-read placement, retry predicate and bound, implementation mechanism, and
measured elapsed time separately. Include variable-duration waits whose range
can enter this interval. A compiled semantic comparison alone does not qualify
these timing properties. Vendor references must include the linked firmware
and its ROM callees: ESP-IDF can override archive leaves, including the I2C
critical-section hooks. Code-placement comparisons require an aligned wait
baseline first.

CPU waiting time, RF exclusion and unavailable packet service are different
quantities. Async hardware waits can release the CPU while the radio remains
unavailable. The current executor uses bounded hardware observations where
there is no owned completion interrupt. Moving those observations to a timer
does not make a long calibration harmless to traffic.

The event-driven RX-gain transitions remain executable semantic models for host
tests and vendor comparison. Production calls the same source-owned arithmetic
and finite hardware primitives directly, without creating an async boundary or
moving a large action enum at each hot edge. Every nested DC/IQ estimator
consumes the same root operation budget, while the fixed outer calibration
graph has its own finite structural bounds. Budget exhaustion cannot produce a
complete coefficient set, and failure retains the recovered cleanup and
outer-control restoration sequence.

The fixed bank encodings are generated at compile time and shared as read-only
data. Measured DC coefficients remain in the individual transition, and entry
encoding borrows them. Terminal products are inspected separately from hardware
admission. Compile-time budgets cover both retained transitions and the compact
external bindings that cross executor call boundaries.

RX diagnostics record only the root interval, its single-poll execution, three
disjoint direct phases and clock-free path counts. They do not install callbacks
inside the minimum-search loop. Search traces and elapsed hardware timing
remain separate measurement scopes.

## Measured RF hold of a tracking tick

The vendor admission tracks with the protocols live (see [Concurrent clients](#concurrent-clients)). How long one tick takes the air from them was measured on ESP32-S31 hardware:
- tracking branches forced with the vendor's debug policy and a zero threshold, so every due tick runs them: the worst case, not the ordinary tick;
- timed on the device by a dedicated tracking-hold probe;
- each grant-protect bracket and the Wi-Fi baseband-off window are wall-time intervals;
- "traffic" ticks ran while the client's traffic did, "idle" ticks after it;
- every run used the same ESP32-S31 board.

A grant-protect interval is the time the request was programmed, not an observation of RF exclusion. During a calibration bracket the RF path is busy measuring, so this port neither receives nor transmits whatever the arbiter grants.

| Clients | Load | RFPLL | Common bracket | Transmit bracket | Baseband off | Whole tick |
| --- | --- | --- | --- | --- | --- | --- |
| Wi-Fi station, HT40 60 Mbit/s UDP RX, 3 repetitions | traffic | ≤ 0.85 ms | ≤ 13.1 ms | ≤ 7.0 ms | ≤ 12.5 ms | ≤ 20.2 ms |
| | idle | ≤ 0.43 ms | ≤ 6.3 ms | ≤ 6.9 ms | ≤ 6.8 ms | ≤ 13.3 ms |
| Wi-Fi station, HT20 30 Mbit/s UDP RX, 3 repetitions | traffic | ≤ 0.94 ms | ≤ 18.0 ms | ≤ 6.9 ms | ≤ 17.5 ms | ≤ 25.0 ms |
| | idle | ≤ 0.52 ms | ≤ 7.6 ms | ≤ 7.0 ms | ≤ 7.0 ms | ≤ 14.6 ms |
| Wi-Fi + Bluetooth LE, HT20 UDP RX and an ATT echo load, 2 runs | traffic | ≤ 0.94 ms | ≤ 16.1 ms | ≤ 13.4 ms | ≤ 15.5 ms | ≤ 30.1 ms |
| | idle | ≤ 0.94 ms | ≤ 7.1 ms | ≤ 13.6 ms | ≤ 13.5 ms | ≤ 21.4 ms |
| IEEE 802.15.4 receiving, 2 boots | idle | ≤ 0.84 ms | ≤ 6.4 ms | ≤ 7.2 ms | ≤ 7.1 ms | ≤ 14.0 ms |

What the runs show:
- **RFPLL.** The correction, the branch an ordinary tick runs most often, holds the air under 1 ms.
- **Common bracket.** DCODE and RX gain calibration is the bracket traffic stretches, from ~6 ms idle to observed maxima of 13–18 ms under traffic (the table gives maxima only, no lower bound): interrupts compete with its polled measurements.
- **Transmit bracket.** Stable at ~7 ms per class, ~13 ms with both classes.
- **Bound.** The worst single bracket observed is 18.0 ms and the worst whole tick 30.1 ms.
- **HT20 rows.** The HT20 30 Mbit/s rows, which set the 18.0 ms bracket, came from a tracking-hold scenario outside this repository in an HT20 configuration; that scenario's other form runs HT40 at 60 Mbit/s, and neither is in this repository, so those rows cannot be re-run as recorded.
- **Spread.** Run-to-run spread under traffic is larger than the effect of the traffic rate. The figures are observed maxima of these runs, not a guaranteed bound.

Against the protocols' tolerances:

| Protocol | Tolerance | Against the observed hold |
| --- | --- | --- |
| Wi-Fi | An ordinary attempt aborts after the 250 ms publication watchdog (`TX_COMPLETION_TIMEOUT`); beacons come every ~102 ms | The whole tick is 8× below the watchdog; a tick misses at most one beacon; a frame received during it is retried by its sender |
| Bluetooth LE | The supervision timeout is at least 100 ms | A tick misses at most a few connection events (up to five at a 7.5 ms interval: a 30.1 ms window can cover events at 0, 7.5, 15, 22.5 and 30 ms); the link survives |
| IEEE 802.15.4 | A sender's MAC retries span about 10–15 ms: 4 attempts, an 864 µs ACK wait, CSMA-CA backoff | A calibration bracket (6–18 ms) is of the same order: a frame that arrives during one can exhaust its sender's retries. The RFPLL correction (< 1 ms) cannot |

The IEEE 802.15.4 row is the open risk: a calibration branch runs when a temperature reference has drifted past its threshold, not on every tick, and higher layers (OpenThread) retry above the MAC. A direct delivery measurement with a numbered peer stream is still open.

## Restoration and failure

The physical operation owns register access across all nested waits. The
protocol retains packet storage, DMA leases and its paused runtime. MAC or
baseband writes inside calibration cannot manufacture a resumable protocol
epoch. Release checks and the exact RX/IRQ handoff establish that boundary.

A pending demand wait is cancellable without starting hardware work. An
abandoned or failed consuming execution is not automatically rolled back;
it exposes no ready owner. Restoration failure also requires reset. Holding
a CPU critical section across async waits is not the exclusion model.

Calibration completion, restoration completion and measured RF quality are
distinct outcomes. The existing branch flags establish the first, while
protocol restoration establishes the second. Neither proves EVM, calibrated
radiated power, sensitivity or sustained thermal stability.

## Vendor reference scope

The runtime parent order and combined RX/TX transaction follow the pinned
esp-phy-lib `libphy.a` of `chips/esp32s31/vendor/artifacts.toml`.
RX has its own temperature reference; TX uses one shared reference and retains
separate Wi-Fi and BT/154 calibration results. Authenticated parent-boundary
execution checks call order and arguments with explicit child models. Compiled
comparisons additionally execute actual archive/ROM children for combined
calibration and the whole parent, including an RFPLL-enabled validation profile.
They compare semantic state and ordered effects under modeled peripheral inputs
and documented transport/wait projections; they do not establish RF quality,
hardware timing or live radio admission. See the
[tracking contract](src/tracking/README.md).

ESP-IDF's [open orchestration](https://github.com/espressif/esp-idf/blob/4d59230ddff16327812782151ef0afef202dc6d7/components/esp_phy/src/phy_common.c)
calls a binary parameter-tracking function from a periodic task timer and on
eligible PHY enable. Its result flags describe selected branches, not a
hardware quiet-window contract. The library's grant-hook names alone are not
evidence that the final firmware acquired exclusive RF access.

RX gain diagnostics retain coarse DC, table-publication and control/restoration
regions without per-edge timing. Minimum-search intervals are nested in the DC
region, including accepted estimator steps and waits. The three region totals
are disjoint; minimum and fine-step totals must not be added to them. Observer
clock reads occur at region boundaries, so these remain instrumented durations,
not pure CPU costs. Early executor errors close active intervals as failed;
cancellation leaves incomplete evidence and does not release hardware ownership.

Coarse RX diagnostics additionally sample one in sixteen minimum invocations and
non-minimum DC executor steps, starting with the first. These sampled intervals
separate prepare/advance from MMIO, minimum settle and readiness execution.
Readiness time includes its timer and suspension, not just a register read.
Sampling is deterministic and may correlate with the search sequence; counts and
scoped maxima accompany totals, which are not whole-calibration costs.

The DC/IQ executor probes readiness immediately after measurement enable, as
rev0 ROM `phy_iq_est_enable` does, around the required 1-us start and stop
settles. Only a result that was not ready incurs the 1-us asynchronous
completion backoff; the observation limit and the timeout disable tail bound
the wait.
