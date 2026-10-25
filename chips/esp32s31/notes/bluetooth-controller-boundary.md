# ESP32-S31 Bluetooth controller contracts

This reference separates silicon lifecycle requirements from the vendor
Controller software architecture. Pinned public glue describes clock, reset,
PHY, interrupt and platform ordering; it does not make FreeRTOS queues,
allocators or callback registries hardware requirements. Production controller-memory ownership belongs to the HAL Bluetooth engine;
the Bluetooth driver owns role value builders and initialization sequencing.
BLE is not linked into a firmware image.

## Pinned public inputs

All sources are from ESP-IDF commit
[`aeab6dcfbeb44aba4b1f8ed102e3086172833153`](https://github.com/espressif/esp-idf/tree/aeab6dcfbeb44aba4b1f8ed102e3086172833153).
The hashes cover the complete raw files used for this review.

| Public ESP-IDF source | Relevant lines | SHA-256 |
| --- | --- | --- |
| [`components/bt/controller/esp32s31/bt.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/controller/esp32s31/bt.c#L78-L145) | shutdown, disable and deinit order | `a7eb5a77c543b244757c08cc595960911c7aff73cae16eb7bd78503322d9f76d` |
| [`components/bt/controller/esp32s31/bt.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/controller/esp32s31/bt.c#L168-L350) | top-level init, enable and rollback order | same file/hash as above |
| [`components/bt/porting_btdm/controller/btdm_common/src/btdm_lp.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/btdm_common/src/btdm_lp.c#L350-L469) | module clocks, MAC reset, LP timer, PHY and BTBB ownership | `1d681581eedcf5aa23b8462a148bd377a92edae676f941d9318a1e8947bf7585` |
| [`components/bt/porting_btdm/controller/ble/src/ble.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/ble/src/ble.c#L613-L756) | BLE controller, BB callback, packet pool and address setup | `ed793921668c5276e4726a506dbeca2d2ca4ae453f1d6b4c3b4943d7c1c7f279` |
| [`components/bt/porting_btdm/controller/btdm_common/src/btdm_coex.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/btdm_common/src/btdm_coex.c#L40-L235) | standalone coexistence defaults and optional shared-radio hooks | `da92929e125a830553d6d251af301ab2fbdfb6aef31d1342e9d14b483bdac1f9` |
| [`components/bt/porting_btdm/controller/btdm_common/src/btdm_external.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/btdm_common/src/btdm_external.c#L29-L43) | empty S31 external-init wrapper | `843a19181e1f4e25d3d9bdc924b2ec0c112ddee4f5c133c428a4ff95a62e5916` |
| [`components/bt/porting_btdm/controller/btdm_common/src/btdm_log.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/btdm_common/src/btdm_log.c#L115-L129) | empty default log-init wrapper | `952b6cd74004dea4254f615c93298bfd392fff4586820de6231916a0fe107ac5` |
| [`components/bt/porting_btdm/controller/btdm_common/src/btdm_osal_freertos.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/btdm_common/src/btdm_osal_freertos.c#L84-L291) | FreeRTOS event queues and ISR wake behavior | `436d009f67fbe7c83fa367a6b80dbf87316727605e751febc03c5118a669b5c8` |
| [`components/bt/porting_btdm/controller/btdm_common/src/btdm_osal_freertos.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/btdm_common/src/btdm_osal_freertos.c#L1084-L1093) | vendor task creation | same file/hash as above |
| [`components/bt/porting_btdm/controller/btdm_common/src/btdm_osal_freertos.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/btdm_common/src/btdm_osal_freertos.c#L1107-L1137) | level-3 interrupt allocation, optional IRAM flag and pinned-core routing | same file/hash as above |
| [`components/soc/esp32s31/include/soc/interrupts.h`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/soc/esp32s31/include/soc/interrupts.h#L139-L153) | Bluetooth MAC CPU sources 124 and 133 | `1a4f155b87090376b1a40ac62e19de344c7f10dc53d9b4451b66d545e9e4717d` |
| [`components/bt/porting_btdm/transport/src/hci_transport.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/transport/src/hci_transport.c#L23-L103) | HCI command/ACL/event routing | `db3fea7daa8d313e16f570eac6dc7e87a860edf03e896af49cce9d5dd7d13b45` |
| [`components/bt/porting_btdm/transport/src/hci_transport.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/transport/src/hci_transport.c#L214-L291) | transport selection, callback registration and teardown | same file/hash as above |
| [`components/esp_hw_support/port/esp32s31/Kconfig.mac`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/esp_hw_support/port/esp32s31/Kconfig.mac) | default two-universal-address policy and Bluetooth final-octet offset | `7b2e44be28d7b51e459ca5a407bf33430c543849836cfa479f2657d7e9fbdd10` |
| [`esp32s31-bt-lib/libbtdm_common.a`](https://github.com/espressif/esp32s31-bt-lib/blob/7f20740dd66ee774ffce5db0b55507892551aa31/libbtdm_common.a) | exact public S31 BTDM object code used to split composite HAL/task boundaries | `fa22a8a2aca48b807addda2bbad78868d6774c82bcdeb8090f9140f6cbccd099` |

The exact public ESP32-S31 controller-library revision is additionally pinned
in [`vendor/artifacts.toml`](../vendor/artifacts.toml). Public glue proves calls and order, not the
internal effects of binary `r_*` entries. Reviewed binary-analysis facts remain
the evidence source for those internal MMIO transactions.

Reviewing the complete HAL-init body exposed and corrected one pre-existing
register-model error: the words at `0x2010_134c` and `0x2010_1350` contain
eight observed four-bit lanes each, not eight dense two-bit lanes. Each
iteration first clears nibble mask `0xc`, then performs a fresh read and ORs
`1 | ((global_index & 3) << 1)`. Bit one can retain a pre-existing one, so the
operation is not a complete nibble assignment. The reviewed register source,
SVD, raw PAC and bindings preserve that exact positional geometry without
inventing lane meaning.

The same review closes the HAL body's runtime-input question. Complete
`r_btdm_task_init` constructs positional inputs `16`, `11`, `33`, `2000` and
`0x2f00_0000`; the complete setter maps period images `500/1000/2000` to a
selector in the same internal object, and the complete helper uses that
selector to scale the two bytes. For the standalone caller the resulting
hardware images are sleep-timer shift `3`, byte zero `22`, byte one `66` and
SRAM prefix `0x2f00_0000`. The HAL represents these accepted
domains directly and executes the complete 49-operation MMIO body. It does
not reproduce the vendor object layout, task, event primitive or IRQ allocator.

The next complete task-initialization component is current symbol
`r_sym_bt_AJP6s1I6EhaAHnz8n72F`. Structural review finds no MMIO in its
complete 182-byte archive body. It calls three software environment helpers
(one reads the public eFuse MAC and reverses its six bytes), clears 48 bytes of
HCI state, initializes a vendor mempool with 272-byte elements, registers
numeric broker source four and initializes a software list head. These are
open-Controller storage, address and dispatch policy rather than silicon
transactions. The factory base identity comes from the reset root's eFuse
owner (`EfuseRegisters`); the pinned S31 two-universal-address policy applies:
Bluetooth is the next final-octet value after Wi-Fi STA. The result is kept
in canonical EUI-48 order; the generic HCI bootstrap performs the
observed single six-byte reversal only when encoding `BD_ADDR`. The Rust
replacement therefore binds the already initialized scheduler epoch to one
bounded `LeControllerHciResources` owner instead of copying the private
environment, pool or callback node. This state exposes only the conservative
bootstrap dispatcher—not operational Link-Layer HCI.

The interrupt boundary contains three CPU routes. The primary Bluetooth MAC
route uses source 124, level 3 and an IRAM-capable handler request. The modem
low-power timer uses source 127 with the same level and residency requirement.
The non-real-time (NRT) Bluetooth MAC INT1 route uses source 133 and level 3
without requesting IRAM placement. The public OSAL installs all three on the
configured Controller core. The primary setup clears and enables
baseline groups `0x0000_8000` in bank 0 and `0x0000_1300` in bank 1 before the
output strobe. Its handler samples masked status; the NRT handler samples its
dedicated snapshot status. Both acknowledge through the same two W1C clear banks. The HAL
therefore exposes a transition that stages a single shared register owner
before either CPU route is bound and preserves the distinct snapshot modes.

This closes route identity, level, residency policy, baseline masks and the
setup/teardown prefix. The complete primary suffix review structurally
classifies dynamic groups `0x1820_0000` and `0x0000_0008`, including bank-one
precedence, two distinct scheduler-state observations and the sticky coalesced
work marker. Individual bit names remain positional. The scheduler words are
transferred with the affine interrupt-register partition, so the hard
handler can preserve the exact read/optional-clear/read order without a
task-side MMIO alias. The public OSAL and complete producer/consumer review
prove that this scheduler work is intentionally coalesced and drained through
a software intrusive completion list, not a hardware FIFO. The default BLE
callback graph further proves that selector 4 drives conditional scheduler
retry and selector 6 drives a post-clear transaction/list consistency action.
The callback registry and list-node layout are vendor software architecture,
but those two effects must have typed open replacements. The distilled
contract and RTOS-free replacement boundary are recorded in
[`bluetooth-interrupt-runtime.md`](bluetooth-interrupt-runtime.md). The
pinned `esp-pacs` revision names source 124 as `BT_MAC` and source 133 as
`BT_MAC_INT1`. One interrupt-table entry binds both, with the runtime timer's
(127) and the doorbell's sources, as one composite part (`BluetoothInterrupts`) on
one line; its handler holds the bank and the timer through gates for the
image's life. A lossless live epoch still needs the feature-specific NRT
policy, typed selector-4/6 actions, open completion queue and lost-wake-safe
waker. The
baseline groups are classified by the complete
source-124 prefix proves four fault/assert lanes, and the HAL
performs their conditional diagnostic reads at the reviewed temporal points
without exporting the opaque words. Bluetooth policy then selects fail-stop
over any simultaneous scheduler wake.

The BLE PHY environment is an address-owned hardware object, not a vendor
configuration ABI. Complete allocator `r_sym_ble_ragNLqDkbVnj7GUutTgs`
(`r_ble_phy_env_init`) requests one `0x80`-byte environment, then stores
separately zeroed allocations of `0x28`, `2 * 0x04` and `0x04` bytes through
pointer fields `+0x30`, `+0x34` and `+0x38`. It then records the BTBB
transmit-power table: its address at `+0x48`, its length at `+0x4c` and the
sign-extended first and last levels at `+0x78` and `+0x7c`.
`r_sym_ble_eRawefsj3HtGBKxFAm81` later fills the forty per-channel bytes
`+0x4d..+0x75`; the register-init body publishes the raw addresses of members
`+0x2c` and `+0x4d`. The open memory owner therefore replaces those
allocations with one zero-based static graph that also retains the
transmit-power table, and installs the three proven internal pointers and the
table record. Resolving-list setup calls
`r_os_mempool_init` with element size `0x40`, obtains one block, writes the
initial `0x80000000` head word and publishes that block through the positional
global pair. The first open owner reserves one such opaque hardware object;
this is address/extent evidence only, not a privacy or resolving-list semantic
claim.

## Reference Controller implementations

These inputs are architecture references, not ESP32-S31 register evidence.
They answer which software layers a complete open Controller actually needs
and which apparent Rust integrations still terminate in closed firmware.

| Implementation | Actual boundary | Ownership consequence |
| --- | --- | --- |
| [Trouble 0.8.0](https://docs.rs/crate/trouble-host/0.8.0/source/README.md) | Async Rust BLE **Host** above HCI. It explicitly requires a Controller implementing `bt-hci`; its ESP32, nRF SDC, Pico W and STM32WB examples do not make those Controllers open. | Reuse after the open HCI Controller is real. Do not put timing, Link Layer or ESP32-S31 register policy in Trouble adapters. |
| [`bt-hci` 0.10.1](https://docs.rs/crate/bt-hci/0.10.1/source/README.md) | Packet types and Host-side traits for the standardized HCI boundary, not a Link Layer. The legacy Transmitter Test v1 declaration in this release uses the LE Read Supported States opcode; the correct v2 command types are unaffected. | Keep as the direct in-process Controller contract. Do not use the defective v1 transmitter type or fork the crate: production uses typed v2 commands and the Controller retains its standard-opcode v1 decoder for external Hosts. `bt-hci` cannot supply radio scheduling, packet ownership, ACL credits or LL procedures. |
| [Zephyr open LE Controller](https://github.com/zephyrproject-rtos/zephyr/blob/08e13d28c3aed050b9742ea28d5d573f260c13a7/doc/services/connectivity/bluetooth/bluetooth-ctlr-arch.rst) | Vendor HAL, soft-real-time ticker, hard-deadline LLL, ULL roles/control procedures, HCI and fixed/lockless utility queues. Its documented priority model keeps radio-event handling above preparation, completion, role management and Host work. | Adopt the HAL/LLL/ULL/HCI decomposition and explicit prepare/abort/done flow. Replace Zephyr kernel threads and Mayfly contexts with typed ISR frontiers plus one executor-neutral async owner; do not move inter-frame deadlines into ordinary futures. |
| [Apache NimBLE](https://github.com/apache/mynewt-nimble/blob/1e8ed60276f35a80ed4d4b4f8bb9d9c6fee53845/README.md) | A complete open Host and Controller with distinct `controller`, radio `drivers`, HCI `transport` and OS `porting` directories. Its published Controller hardware support does not include ESP32-S31. | Use its controller-only split, LL/HCI semantics and conformance tests as behavioral references. Do not port its NPL/event-task ABI or Nordic/Renesas radio shims as an S31 HAL. |
| [Pinned `esp-radio` BLE connector](https://github.com/ermacv/esp-hal/blob/7f914782f5c689a74f37df83b89a0827d3800323/esp-radio/src/ble/controller/mod.rs) | Async `bt-hci` facade over BTDM initialization and symbols supplied by the `esp-wifi-sys` binary packages. | Useful interoperability and lifecycle evidence, but not a source-open Controller. Replacing only its connector or OS adapter would leave the closed Link Layer intact. |

## Software and hardware responsibilities

| Owner | Responsibility |
| --- | --- |
| Platform/HAL | Clock/reset, PHY/BTBB leases, interrupt routing and powered teardown |
| Controller time and scheduler | Radio-time conversion, reservations, list membership, command publication and completion |
| Controller memory | Pinned packet/descriptor graphs, private pointer encodings and affine CPU/hardware transfer |
| Lower Link Layer | Hardware event parameters and hard-deadline radio behavior |
| Upper Link Layer | Advertising/scanning/connection policy, retry, supervision and LL control procedures |
| HCI Controller | Supported command/event surface, transport backpressure and ACL credits |
| Host | L2CAP, ATT/GATT, GAP/SMP and application policy above HCI |

The open Controller composes these layers without the vendor's software
container. The source tables below classify the vendor lifecycle; they do not
claim that a feature is operational.

## 2. Init/deinit classification

The public top-level init order is OSAL pool, log, external hooks,
coexistence, clocks/reset, BTDM task, low-power state, HCI flow-control state,
BLE stack and HCI transport. The open lifecycle must preserve only the
hardware dependencies in this order; it must not reproduce the software
container around them.

| Vendor step | Class | Open-driver decision |
| --- | --- | --- |
| `ble_osal_elem_calc`, `btdm_osal_elem_mempool_init` | open-controller replacement | Do not port the object pool or FreeRTOS layouts. Use statically bounded Rust-owned queues, packet slots and intrusive/free-list state with explicit lifetimes. |
| `btdm_log_init` | profile-optional | The default S31 wrapper is empty. Diagnostics may be added through compile-time logging without becoming a radio prerequisite. |
| `btdm_external_init` | profile-optional at this level | The S31 wrapper is empty. Concrete BLE callbacks such as randomness or cryptography must be introduced only by the feature that consumes them. Heap/allocation callbacks are not required by silicon. |
| `btdm_coex_init` | profile-optional for standalone; required for joint radio ownership | With software coexistence disabled, the public implementation is a set of no-op/default adapters. The first profile may own the radio exclusively and implement that declared standalone contract. Wi-Fi plus BLE requires a later shared arbiter and hardware testing. |
| `btdm_lp_enable_clock` | silicon-required | Acquire the BT and BT-APB clock dependencies, pulse the BT MAC reset and establish the selected low-power timer source. These operations belong to platform/HAL typestates. |
| `r_btdm_task_init` | unresolved composite | Do not port the task. Reviewed binary evidence shows finite hardware scheduler/IRQ initialization inside the boundary, mixed with event/list and OSAL setup. Recover and publish those transactions as separate HAL operations before replacing the task with the async Controller owner. |
| `btdm_lp_init` | split | Publishing the measured low-power clock frequency to controller timing is silicon/controller-HAL work. FreeRTOS tickless callbacks, PM locks, light-sleep retention and wake registration are optional low-power features and are out of the always-awake profile. |
| `r_btdm_hci_fc_env_init` | open-controller replacement | HCI ACL credits and completed-packet accounting are Controller state. Implement them in bounded Rust queues; no vendor flow-control environment is needed. |
| `ble_stack_init` / `r_ble_controller_init` | unresolved composite, predominantly open-controller replacement | This is the closed BLE Controller: Link Layer, scheduling and command/event behavior must be replaced. Only separately recovered register, IRQ, timer and memory transactions may move into HAL. Completion of this vendor call is not an acceptable open typestate. |
| `esp_ble_register_bb_funcs` | unresolved | This callback table may bridge hardware/platform helpers into the closed Link Layer. Recover call sites and effects before deciding which entries are silicon-required. Do not clone the ABI as the open architecture. |
| duplicate-scan cache and public-address setup | open-controller replacement | These are Link Layer/HCI state. The first DTM slice needs neither; later advertising/scanning owns address and duplicate-filter policy. |
| `ble_msys_init` | open-controller replacement | Vendor mbufs are transport/storage policy. Replace with bounded, typed packet ownership shared by LLL, ULL and HCI. |
| `hci_transport_init` | open-controller replacement | UART/VHCI selection and callback registration are not radio requirements. The open in-process `bt-hci::ExternalController` channel is the native path; an H4/UART adapter can remain optional. |

Deinitialization is the reverse ownership proof, not merely a list of cleanup
calls. The current open path must remain fail-stop after the first unpaired
powered mutation until IRQ quiescence, scheduled-command cancellation, packet
reclamation, BTBB release, PHY release and clock release are all proven.

## 3. Enable/disable classification

The public enable path first executes `btdm_lp_reset(true)`. On S31 that
enables the Bluetooth PHY client and only then enables the shared BT baseband.
This establishes a mandatory silicon order:

```text
clock/reset owner
  -> target-registered common PHY
  -> settled Bluetooth PHY client
  -> finite BTBB transaction
  -> BLE radio-engine initialization
```

The corresponding disable path first stops the Controller software, then
disables BLE/coexistence, releases BTBB and finally releases the PHY client.
An open implementation therefore needs a quiesced hardware state before either
shared hardware lease can be dropped.

| Vendor step | Class | Open-driver decision |
| --- | --- | --- |
| `esp_phy_enable(PHY_MODEM_BT)` | silicon-required | The shared radio's owner registers the shared PHY domain for the first client or wakes its closed RF; the Controller then joins the domain as the Bluetooth client of the radio arbiter. Tracking due at the join runs at once under the arbiter lease, with the grant-protect brackets, before the BLE PHY engine is enabled. A started tracking failure poisons the domain and keeps the Controller fail-stop. Periodic tracking belongs to the shared radio's owner; leaving reports the last-client fact that closes RF. The radio system that owns the arbiter is not in this repository. |
| `esp_btbb_enable()` | silicon-required | The join takes the shared BTBB reference; only the first holder runs the reviewed finite BTBB transaction, with the gain byte projected from the registration that describes the lease. This finite transaction is not a complete common-PHY/baseband or radio-readiness claim. |
| `btdm_coex_enable` | profile-optional for standalone | A no-op success is source-correct only while the product contract excludes simultaneous Wi-Fi. |
| `ble_stack_enable` | unresolved composite | Replace protocol activation; recover any remaining radio-engine activation transaction separately. |
| `r_btdm_hci_fc_enable` | open-controller replacement | Start a fresh bounded HCI credit epoch in Rust. |
| `r_btdm_task_enable` | split, still incomplete | The hardware-only 49-operation BTDM HAL-init body, exact baseline fault masks and diagnostic capture, controller-output strobes, generated runtime-timer start command, typed three-route ESP-HAL primitives, affine ISR scheduler MMIO, dynamic scheduler classifier, the RTOS-free coalesced scheduler wake and pure controller-time latch/epoch phases are recovered as separate contracts. One affine powered epoch composes scheduler initialization and low-power hardware with borrowed target common-PHY registration, explicit Bluetooth-client acquisition and terminal initial tracking. Only a settled client can cross the finite BTBB gate and remain retained through BLE-PHY register initialization, Controller-output preparation, runtime-timer start and stable interrupt-owner publication. Its powered task endpoint performs the scheduler executor's list-zero steps. Registration and tracking failures retain the fail-stop. The radio runtime and role that drove those steps, and the portable LE Controller core and service loop that served HCI over them, are not in this repository; no Host interface or final HCI target exists. Periodic client tracking, client/BTBB/PHY teardown, unrelated-list and source-127 expiration consumers, feature-specific NRT policy, effective controller-counter width and wake causality, typed selector-4/6 actions, on-air BLE readiness and hardware testing remain unresolved. |
| `r_btdm_task_disable` and `ble_stack_disable` | unresolved composite | Define a stop barrier: mask sources, cancel/abort commands, acknowledge residual status, reclaim every packet, then expose a quiesced owner. |

The public OSAL demonstrates that the vendor implementation uses FreeRTOS
queues, ISR variants and an RTOS task. This is evidence about its software
architecture, not evidence that ESP32-S31 requires an RTOS. The hardware
contract is the ordered interrupt/status/timer/memory interaction underneath
those adapters.

## Controller ownership boundary

The ownership direction is:

```text
SVD / generated PAC blocks
  -> HAL register owners over their field accessors
  -> shared clock, reset, PHY and BTBB leases
  -> BLE Controller HAL: primary/timer/NRT IRQs + scheduler + packet memory + radio command
  -> Lower Link Layer (hard-deadline radio-event engine)
  -> Upper Link Layer and LL control procedures
  -> bounded HCI command/event/ACL worker
  -> bt-hci::ExternalController
  -> Trouble Host
```

The sole Controller owner is async and executor-neutral. Both same-core,
level-3 handlers serialize access to one staged clear-bank owner; each handler
only acknowledges/classifies a bounded hardware snapshot and wakes the async
owner. Hard
radio deadlines, including inter-frame turnaround, stay in the hardware
scheduler and the minimal lower-link-layer interrupt path; they are not
delegated to arbitrary executor latency. No mutex shared with the Host may be
held across a radio deadline.

Trouble starts above HCI. It can validate and consume the Controller boundary,
but it does not own the PHY, scheduler, packet engine or Link Layer.
The existing software bootstrap proves transport/API compatibility only.
