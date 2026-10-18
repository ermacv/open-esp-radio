# Radio library architecture

`crates/` contains the source-owned radio implementation, its protocol code
and production integrations. Applications supply board identity, credentials
and peripherals. HIL scenarios, traffic generators, UART test protocols
and vendor artifacts live outside this tree. See the
[source policy](../docs/source-policy.md).

The [ESP32-S31 radio feature map](hardware/esp32s31/driver/FEATURES.md) indexes shared
lifecycle, PHY, coexistence and protocol inventories.

## Source map

Paths describe responsibilities; a directory need not be a Cargo crate.
Cargo package identities are independent of this directory hierarchy.

| Path | Responsibility |
| --- | --- |
| `oer/` | Thin public facade; reexports protocols, chip backends and selected compositions |
| `radio/port/` | The lifecycle every block's port shares (`Installed`, `Owner`, `run_port`, `DriverExit`) and every block's vocabulary (`oer-radio-port`): the brand and `branded`/`Mint`, slots, permits, seats and lines, the nested `PortResult` with typed refusals and the `Poisoned` outer error carrying the backend's cause, `EventsLost` with its ordering rule, and the clock resolution and epoch relation |
| `radio/coex/` | Portable radio client identity (`RadioClient`) and coexistence priority vocabulary (`CoexPriority`) every protocol and backend shares |
| `memory/` | Audited stable-memory proofs, affine buffer/queue handoff and statics in zeroed regions (`zeroed_static!` over `bytemuck::Zeroable`) |
| `trace/` | Typed, reset-retained event trace and snapshot slots; drivers define the events, images place the storage |
| `network/interface/` | Stack-neutral interface, link and error values |
| `protocols/ieee80211/{mac,softmac,sta,ap,security/rsn}/` | Sans-IO frame/protocol code, MAC contracts, role policy and security state machines, and the ports their drivers wait on |
| `protocols/ieee80211/lower-mac/` | The Wi-Fi [lower-MAC port](protocols/ieee80211/lower-mac/README.md) on the owner shape: `LowerMacBackend` with its setup and its running phase (one object under `&mut`, tickets, buffers and retry plans, every caller rule a type); the values its backends' cores take |
| `protocols/ieee80211/arbiter/` | The [arbiter](protocols/ieee80211/arbiter/README.md) of the channel a station and an access point share: named leases, absences beside the access point, the gate and retunes of the port; the access point's follow, search and loss policies (`ChannelCoordinator`) |
| `protocols/ieee80211/upper-mac/` | Transmit policy above the lower-MAC port: MPDU retry counters and Retry bit over a `RateLadder`, A-MPDU retry selection from the BlockAck, RTS/CTS and CTS-to-self selection, and the `TxPlanner` that turns one exchange into lower-MAC attempts and statistics |
| `protocols/espressif/ieee80211/policy/` | Espressif policy data for those algorithms, recovered from the vendor stack: rate-schedule arenas and the retry ladder, LMAC retry limits, A-MPDU lifetime, default contention, the HE TXOP byte budget and the CCMP packet-number step, and the coexistence events of a reconnecting station's connection frames |
| `protocols/ieee80211/datapath/` | Software TX frame ownership, destination queues, airtime scheduling and physical materialization contracts |
| `protocols/bluetooth/le/ll/` | Portable LE PDU codecs, protocol-role state, the software Link Layer acknowledgement (`connection::acknowledgement`) and the Direct Test Mode test planner and counters |
| `protocols/bluetooth/le/radio/` | Portable LE radio values: the `LeRadio` clock domain, channels, PDUs, role configuration and event geometry in physical values, what an event observed, the capabilities a backend states (`LeCapabilities`) and the `Supported` proof that a value fits them |
| `protocols/bluetooth/le/controller/` | Pure LE Controller pieces: connection-event and legacy advertising geometry (`timing`), authenticated data PDU receive (`receive`), the vendor coexistence priority rules (`coexistence`) and the diagnostic MIC-fault command |
| `protocols/bluetooth/hci/` | `wire` holds packet views; `transport` holds Controller-to-Host packet validation; `controller` holds reset-scoped bootstrap state, command classification and the `le` codecs |
| `protocols/i/` | `mac/frame` holds bounded bytes; `radio/{command,event,channel,capabilities,interface,power}` holds portable values; `port` holds the capabilities and settings every backend shares; `owner` states the port on the owner shape, over `radio/port/` (`oer-radio-port`), whose in-memory backend `model` serves the crate's own tests |
| `services/ieee80211/rsn/` | Executor-independent drivers of the security state machines: WPA2 handshake and key-install runners |
| `hardware/esp32s31/{pac,hal,phy}/` | PAC `ownership` partitions register authority; HAL `root` and `owner` own the radio root and protocol routes; domain modules hold register operations, transactions and RF algorithms |
| `hardware/esp32s31/driver/ieee80211/{dma,mac}/` | S31 descriptor ownership and MAC `rx/tx/rate`; `mac/tx/metadata` lowers portable traffic intent |
| `hardware/ieee80211/pac/` | Chip-neutral closed Wi-Fi MAC register blocks (`pac/raw`, generated) and reviewed transactions over the layouts chips share; each chip PAC places the blocks at its own addresses |
| `hardware/espressif/coex/` | Espressif coexistence policy recovered from esp-coex-lib: the shared event priority table and IEEE 802.15.4 levels, timer programming, clock conversion and the time-slice schedule, over the timer and clock ports each chip binds |
| `hardware/espressif/i/engine/` | Chip-neutral IEEE 802.15.4 MAC engine ported from the public ESP-IDF driver, over the `ILowLevel` interface each chip's HAL implements |
| `hardware/radio/analog/` | Chip-neutral non-blocking analog register bus contract of the Espressif PHY and its polled field transactions (`phy_i2c_readReg_Mask`, `phy_i2c_writeReg_Mask`) |
| `hardware/radio/clock/` | Chip-neutral shared modem clock reference-count planner of ESP-IDF `modem_clock.c`, over the dependency table and edge executor each chip's HAL supplies |
| `hardware/esp32s31/driver/{bluetooth,coex}/` | Chip radio drivers; the Bluetooth root is the role-free hardware engine (clocks, PHY, IRQ, scheduler) and its `memory/` holds its lower ownership boundary; `coex/` binds the Espressif coexistence policy to the radio arbiter; `i/` keeps only the ESP32-S31 IEEE 802.15.4 capability page |
| `adapters/esp-hal/esp32s31/{soc,radio,ieee80211}/` | Upstream SoC access, singleton acquisition and concrete hardware bindings |
| `adapters/embassy/espressif/executor/` | Scheduler-free Embassy executor and time driver of the staged-boot chips |
| `adapters/esp-hal/espressif/interrupt-table/` | The image's interrupt table on an Espressif interrupt matrix, through esp-hal |
| `adapters/virtual/time/` | Per-instance virtual monotonic time for host tests and simulations: a `VirtualClock` its owner advances and a `SkipClock` whose waits skip to their deadline |
| `runtime/radio/permit-pool/` | `oer-radio-permit-pool`: the allocation-free first-in, first-out `PermitPool` a radio port backend keeps in its held part of the install |
| `runtime/ieee80211/` | Portable Wi-Fi execution primitives: monitor handoffs, task shutdown, station network ownership and poll boundaries |
| `runtime/interrupt-table/` | Each image's static table of peripheral interrupt sources (handler, level, core) with unique tokens that enable and disable them |
| [`runtime/esp32s31/ieee80211-form/`](runtime/esp32s31/ieee80211-form/README.md) | S31 Wi-Fi pinned driver/IRQ execution form: installation-owned registers, exclusive IRQ consumers, cancellation/forget custody and linked SRAM proof |
| `adapters/embassy-net/owned/` | Owned-packet network adapter over the pinned Embassy/Xarxa forks |

`memory` owns backing stability, range proofs and affine handoff; chip DMA
modules own hardware descriptors and controller transitions. `network/interface`
is a dependency-free value boundary. The owned adapter, the only network
integration, uses the pinned Git Embassy/Xarxa contract and its maintained
packet-pool extensions over the radio scheduler and final SRAM allocator.
Product radio policy cannot depend on network adapters; the architecture
audit follows transitive normal/build dependencies across their domain paths.

Product resource profiles live in integration, which also executes whole-radio
lifecycles. The [Embassy map](adapters/embassy/README.md) distinguishes external
bindings from the [radio execution domain](runtime/README.md). Folder names alone
do not enforce these boundaries.
The [protocol naming convention](../docs/protocol-naming.md) defines
`protocols/ieee80211`, `protocols/i` and `protocols/bluetooth` as technical family namespaces.
Wi-Fi remains the application-facing technology name; portable LE Link Layer
code lives in `protocols/bluetooth/le/ll`, beside the family-wide HCI boundary.

The [IEEE 802.11 module map](protocols/ieee80211/README.md) separates format parsing from
retained state inside each protocol domain. Block Ack has `frame/session`,
fragmentation has `parsing/reassembly`, and sequence/duplicate history has
explicit owners beside its STA/data consumers. QoS values are distinct from
WMM element parsing; ESP-NOW wire formats remain below SoftMAC peer/security
owners. These boundaries are modules within their protocol packages.

Portable IEEE 802.11 encoders accept explicit local capabilities.
Authentication and association retry epochs belong to `ieee80211/sta/join`,
while `ieee80211/ap/limits` owns the fixed software peer budget. WPA2 retains
secret bytes and zeroization; register-word conversion stays in the PAC and
does not require another key copy. No ESP32-S31 Wi-Fi role, runtime or
composition exists: the old stack was deleted and its sources are kept by the
tag `legacy-wifi-2026-10-16`.

Chip PHY waits through `oer_esp32s31_phy::executor::wait::delay`: synchronous
ROM settles of at most 20 us, and an absolute deadline on the caller's
`oer_time::Timer` for every other wait, which never ends when the deadline is
unrepresentable. A chip radio composition passes
[`oer_time_embassy::EmbassyClock`](adapters/embassy/time/). Chip PHY stays
executor-independent, and the executor/time ABI backend owns no PHY policy.

The Bluetooth LE Link Layer, HCI codecs, radio values and pure Controller
pieces live in `protocols/bluetooth/`. IRQ, scheduler and hardware ownership,
and the vendor event policy and bring-up order, live in the
[hardware engine](hardware/esp32s31/driver/bluetooth/README.md). The radio
role, runtime, composition, HCI transport, trace points and GATT adapter that
wired them into a running Controller were removed with the Bluetooth LE
orchestration (, tag `legacy-radio-2026-10-16`).

The chip Wi-Fi `rx/frontier` owns finite physical-ring transitions and borrows
an abstract delay. Its `rx/transaction` synchronously borrows the live ring,
storage and staging pool for one bounded completion/recycle pass. Publication
accepts the unique frame or returns that same lease on rejection. No chip
dependency points back to an adapter. The S31 form routes both Wi-Fi
interrupt handlers through one pinned installation; register banks and
diagnostics belong to that installation, while the sole static slot holds only
its address.

## Register and execution boundaries

The radio-register direction is reviewed SVD → `pac/raw` accessors → semantic
`pac` → `hal` → chip driver. The raw package contains only generated output;
handwritten ownership and validation transactions live in `pac`. The separate
upstream chain is `esp-pacs` through `esp-hal`, bound in
`adapters/esp-hal/esp32s31/`. Its `soc/` adapter contains non-radio SoC
transactions, cache/MMU access and GDMA ownership; it is not generated PAC
code. The [PAC provenance map](hardware/esp32s31/pac/README.md) identifies both
generated outputs, the publisher and the separate upstream bindings.
See [unsafe boundaries](UNSAFE.md) for the enforced exceptions.

PAC operations are stateless single transactions: register-local fields,
straight-line multi-register sequences and the affine receipts they return.
They keep no state between calls, have no phases, never poll and never route.
The HAL owns the radio root, protocol routes, every owner hierarchy, phased
sequences, polling, delays, lifecycle and recovery, and is the only
production consumer of the PAC. Handwritten
code outside the restricted PAC uses typed accessors; missing fields must be
reviewed and published through the SVD/PAC. A Rust ownership proof does not
establish the meaning of a recovered register.

PHY borrows an opaque `SharedPhyHal`; role code uses finite HAL/MAC capabilities.
Do not expose PAC callbacks, `Deref` escapes or owner re-exports above these
boundaries. Shared task-side handles remain tied to the HAL arena's explicit
serialization; a copyable handle grants no unsynchronized cross-thread MMIO
access. Drivers and adapters depend on the HAL only; register-level value
types they need are re-exported by the owning HAL module.

## Lifecycle and network ownership

Public handles carry commands. Dropping a handle never resets or destroys the
hardware. Starting consumes an idle owner; stopping returns idle only after
masking interrupt sources, draining pending IRQ work, completing or
quarantining TX, stopping RX, detaching queues and publishing link down.
Failure to prove quiescence retains a terminal fault owner instead of
fabricating reusable idle. Cancellation, abnormal drop and `mem::forget` must
not release storage still owned by hardware.

STA and AP own separate peer, security, Block-Ack, rate and lifecycle policy.
They share mechanisms below that policy.

Applications own `embassy-net::Stack`, DHCP, sockets and network tasks.
Network adapters do not acquire radio policy or physical DMA ownership.

Radio scheduling selects a flow before reserving scarce internal SRAM.
Materialization transfers selected software work into a finite physical pool;
terminal completion or proven abort returns that storage.

Ordinary code and task stacks may execute from PSRAM. DMA-visible storage,
interrupt/trap stacks and the audited hot/interrupt call graph remain in
internal SRAM; placement checks enforce this boundary.

## Feature scope and verification

Protocol limits, security modes and live/partial/fail-closed boundaries belong
in the domain inventories indexed by the
[radio feature map](hardware/esp32s31/driver/FEATURES.md). A parser,
register setter, descriptor or host test alone does not establish an
operational radio feature. Do not infer concurrent-radio support or new
capabilities from structural reuse.

Unit tests live in separate child files beside the modules whose private
contracts they exercise; public integration tests live in crate `tests/`
directories. Feature gates apply to both the implementation and its tests. Large-error lint expectations stay with ownership transitions that must
return exact inline owners without allocation. An uncomposed hardware path
must identify its missing caller at the entry boundary; broad module-level
warning suppression does not document that lifecycle limitation.
Vendor comparison must exercise compiled production entries and fail
closed. Compositions and HIL may compose or observe the driver, but must not own
another implementation of its protocol or hardware behavior.
