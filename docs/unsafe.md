# Unsafe boundaries

Safe callers must not manufacture register, interrupt, descriptor or DMA
ownership. The policy lives in standard Rust and Cargo lint settings, so every
`cargo check`, `cargo build` and `cargo clippy` enforces it:

- Most production crates start with `#![forbid(unsafe_code)]`.
- Trusted exceptions start with
  `#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]`, permit unsafe
  code only through documented item-level allowances and retain
  `unsafe_op_in_unsafe_fn = "deny"` from `[workspace.lints]`. Membership in
  the exception list is not blanket permission to add unsafe operations.
- `[workspace.lints.clippy]` denies `disallowed_methods`; `clippy.toml` lists
  them, and an intentional use carries a local `allow` with a reason.

In audited packages every
`unsafe` block and `unsafe impl` needs a `// SAFETY:` comment on the lines
immediately before it, stating why its prerequisites hold. A forwarding
call inside an `unsafe fn` names the `# Safety` contract it relies on. Put the
comment below any `#[allow(unsafe_code, ...)]` attribute: Clippy 1.97 does
not accept a comment above an attribute that spans several lines.

`cargo clippy --all-targets` (the default members) applies these lints on the host.
Generated PAC code has no crate-root unsafe attribute. The table below maps
package identities to source owners.

## Generated access and trusted handwritten code

`esp32s31/pac-raw/` is the `oer-esp32s31-pac-raw` package and
`espressif/ieee80211-pac-raw/` the `oer-espressif-ieee80211-pac-raw` package of the
Wi-Fi MAC register blocks a chip places at its own addresses. Each
contains only SVD-generated register access, checked through the
generator/publisher pipeline; the shared package contains no address. The
publisher has no declaration for
handwritten modules in this package.

`esp32s31/pac/` contains the handwritten semantic radio PAC and its
generated capability catalog in `src/generated.rs`. The
[PAC provenance map](../chips/esp32s31/pac/README.md) distinguishes both generated
Rust outputs from its handwritten ownership and domain modules,
including the IEEE 802.15.4 task/interrupt split. Non-radio register
access through upstream `esp-hal`/`esp-pacs` belongs to
`adapters/esp-hal/soc/`, which also contains cache/MMU and GDMA
transactions. Its Cargo package is `oer-adapters-esp-hal-soc`; its implementation
is handwritten code.

The executable policy permits scoped unsafe exceptions in these handwritten
packages. Prefixes below omit `oer-` only to keep the mapping readable.

| Package suffix | Source path |
| --- | --- |
| `memory` | `base/memory/` |
| `embassy-net-owned` | `adapters/embassy/net/` |
| `esp32s31-hal` | `esp32s31/hal/` |
| `esp32s31-bluetooth-memory` | `esp32s31/bluetooth/memory/` |
| `esp32s31-pac` | `esp32s31/pac/` |
| `esp32s31-soc-esp-hal` | `adapters/esp-hal/soc/` |
| `esp32s31-phy` | `esp32s31/phy/` |
| `espressif-executor-embassy` | `exec/executor/` |
| `espressif-interrupt-table-esp-hal` | `exec/esp-hal-irq/` |
| `interrupt-table` | `exec/interrupt-table/` |
| `platform-staged-layout` | `platform/staged-layout/` |

These exceptions cover distinct obligations: singleton acquisition and MMIO
serialization, stable addresses and CPU/DMA transfer, target ABI and placement,
and one-time static resource or interrupt binding. `embassy-net-owned` has one
unsafe module, `external_rx`: it adopts a detached RX DMA buffer from an
`oer-base-memory` handoff slot as a Xarxa `PacketBuf` (`ExternalPacketOrigin::new`
and `adopt`) and returns the slot through `release_adopted` when the packet
drops. A slot is adopted again only after that release returned, because the
radio must first claim the slot the release freed. Preserve the proof at the
smallest operation; a safe state machine in an audited crate remains safe.

`esp32s31-bluetooth-memory` is the core of the controller-SRAM protocol:
the controller walks and writes that memory, so, like the HAL's DMA modules,
the crate whose codecs and scheduler lists encode every address the
controller follows holds the narrow unsafe code that publishes them.
`scheduler::hardware::live` publishes list-zero items only after resolving
each address to a submitted item of the `'static` scheduler pools, in the
executor's order. The BLE PHY engine, the direction-finding workspace, the
device table and the receive chain each mint, in one place, the HAL proof of
the storage the hardware is pointed at (`BlePhyHardwareObjects`,
`DirectionFindingDescriptor`, `DeviceTablePublication`, `RxChainHead`),
whose constructor is unsafe; the HAL publications that take them are safe.
An `unsafe` contract there states only the memory the hardware reaches: the
Controller's lifecycle order, power and PHY membership are correctness,
which the driver's typestate holds, and the driver forbids unsafe code. The
scanner start takes the scanning list's publication of the same Controller
epoch, so a list another epoch published, or a reset head, never starts
it.

`esp32s31-hal` has one module that makes typed DMA addresses,
`dma_address`: the only caller of the `unsafe` `DmaWindowAddress` and
`DmaRef` constructors of the PAC's generated `layouts` (clippy
`disallowed-methods` refuses them elsewhere). Its `placed`/`placed_link`
take an address of storage the caller keeps pinned in the DMA window: a
placed static's descriptors and buffers, or a placed pool's buffer
(`PlacedBacking`); each call site names its proof.

Its `dma_static` module holds the placement constructor:
`DmaStatic::placed`, `unsafe` and called only by the `dma_static!` macro,
whose section is checked to be `.dma.bss.<block>`; the runtime linker
script's ASSERTs over the DMA range (from the generated `dma-window.x`) are
the proof.

The Wi-Fi DMA storage, `esp32s31-hal`'s `ieee80211::dma` module, has one
unsafe site that exists only under `test-support`: the host model of the DMA
engine's memory port, `RxDmaStorage<…, HostDma>::{dma_write, dma_read}` in
`ieee80211/dma/rx_storage/host_dma.rs`.
It copies bytes into or out of one buffer's `UnsafeCell` at a published
`HostDma` address (a ring buffer or the reserved descriptor's buffer),
refusing any range outside a single buffer and any buffer detached to an
upper owner. Its caller lends the arena's live ring shared:
completed-descriptor views borrow that ring mutably, and the ring is not
`Sync`, so no CPU view of the bytes is live meanwhile.

`oer-base-memory` holds the shared proofs for executor-crossing storage.
`AffineIndexedSpscQueue` lets an affine permission's place select its value
cell while the audited SPSC ring stores only indices; available ring space
follows from N distinct indices, without an occupancy check, a backend
free-credit pool or another await. An implementation of the unsafe
`UniqueQueueIndex` trait proves index bounds and uniqueness. Producer Release
publishes cells; consumer Acquire moves them completely before permission
return orders the next use.

`IndexedCustody` retains an owner while its affine name travels through an
executor. Its unsafe `CustodyDomain` binds one private store to its affine
place names; this is stronger than an index bound, and two same-typed
installations must not exchange names. Insert returns an initialized-cell
name; only returning that exact name permits take. A name contains only an
index, never a pointer or a borrow into the retained owner. `finish` borrows
the binding's `Stopped` proof that hardware use has ended. Destroying a
non-empty custody store is the quarantine path only: `Quarantine` is an
unsafe trait whose method erases key material in place and releases, moves
and awaits nothing, so DMA leases stay reserved until reset.

`RequestSlot` carries one request at a time from a client to a server and
its answer back. Its state (`IDLE`, `POSTED`, `WORKING`, `ANSWERED`) admits
exactly one accessor to each payload place, a Release store hands the place
over and the other side's Acquire CAS takes it; the client posts only with
the slot's one `Poster`, which the answer returns. The radio-system client
uses it and has no unsafe code of its own.

`retained` holds records in memory that survives a reset. A
`RetainedCell<T: bytemuck::AnyBitPattern>` reads what the previous boot left
as a `T`, so every bit pattern must be a value; `try_lock` hands the record
to one holder at a time without waiting, and its claim lives in a section
every boot zeroes, so a reset never leaves the record claimed. The cell is
defined only by `__retained_static!`, whose two statics are its own; the
staged runtime's `retained_static!` names the sections. `read_volatile`
reads a value behind a shared borrow with a volatile load. `StaticRef<T>`
publishes a `&'static T` through an atomic pointer that interrupt handlers
and the panic path read without a lock. `link_fn!` binds a function at link
time: the calling crate declares the symbol and its own macro has the image
define it, so every call is a direct call that the stack analysis follows,
where a function pointer would leave the callee unknown; the trace's clock
is one. `DmaBytes<N>` holds bytes whose address a driver publishes to a
DMA engine: reads copy them out by volatile accesses through `&self`,
writes copy them in through `&mut self`, so no reference aliases memory the
engine writes; the IEEE 802.15.4 engine's frame buffers are such bytes.

`HandlerCell` hands one `&'static mut T` from the context that installs it
to an interrupt handler. Its state (`EMPTY`, `INSTALLING`, `READY`, `BUSY`)
admits one reference at a time: `install` consumes the reference and
publishes its pointer with a Release store of `READY`; `with` creates the
only `&mut T` after its Acquire exchange from `READY` to `BUSY` and returns
the state to `READY` once that borrow ended. A call that does not find
`READY` gets `None` without waiting, so the cell is not a lock and a nested
call cannot alias. It is `Sync` only for `T: Send` and never reclaims its
value.

`platform-staged-layout` declares the stage-two handoff contract: the unsafe
trait `AdoptsBootstrapMapping`, a board's promise that its adoption of the
bootstrap's PSRAM mapping neither resets nor remaps PSRAM, and the
`StageTwoHandoff` proof only the staged runtime's entry mints. It holds no
other unsafe code.

## Firmware and platform code below the boundary

The platform crates (`chips/platform/*`), the bootstrap images
(`firmware/bootstrap/*`) and the probe images (`firmware/probes/*`) own the
target ABI and placement, so they hold unsafe code that no crate above them
needs:

- The staged runtime defines the image entry (`entry!`, the `Handoff` and
  `Adopted::enable_interrupts`), the sections the linker script maps
  (`placed_static!`, `sram_fn!`, `retained_static!`), the panic hook and
  exception handler symbols (`panic_hook!`, `exception_handler!`), the trap
  CSRs (`trap`) and the linked sections (`layout`). Images and HIL agents
  reach these through safe APIs and forbid unsafe code.
- Each board implements `AdoptsBootstrapMapping` for its PSRAM and wraps the
  mask ROM's console output (`rom`), a C variadic.
- The bootstrap places the packed stage two and the flash tuning reference in
  its own read-only flash sections.
- A probe image is entered by the Blobray verifier through the exported
  symbols the vendor code calls (`_start`, `ets_delay_us`, the probe
  entries), with that code's C signatures, and keeps its fixtures in statics
  the verifier's serial calls reach one at a time.

The bootstrap and probe crates deny `clippy::undocumented_unsafe_blocks`, and
each unsafe attribute in them carries its `// SAFETY:` comment as well.

## PAC dependency authority

Direct dependencies on the semantic radio PAC are restricted independently
of unsafe syntax. The only allowed consumers are the chip's `pac-raw`, `pac`
and `hal` under `esp32s31/`, and the chip-neutral Wi-Fi MAC register
crates under `espressif/ieee80211-pac*/` that those PACs place: drivers and
adapters reach registers through HAL owners. Check the executable list when changing this boundary.

Upper layers use opaque, finite capabilities rather than raw pointers,
unchecked lifetimes, generic PAC callbacks or independently reusable interrupt
owners. MMIO outside the restricted PAC must use typed accessors. Missing
register fields are reviewed and published in the SVD/PAC, not reconstructed
with local masks in a HAL or adapter.

## Active-owner lifetime

Normal stop proves IRQ, RX, TX and queue quiescence before returning reusable
storage. `Drop` cannot perform that asynchronous proof: abnormal drop retains
or quarantines active static storage and faults the lifecycle. It must not
reset hardware, panic as a release mechanism or claim successful shutdown.
Because safe Rust permits `mem::forget`, forgetting an owner must remain a
harmless leak rather than release memory still accessible by hardware.

Task boundaries, cancellation and rendezvous do not weaken the same ownership
or quiescence obligations. The wider execution and
memory-placement contracts are in the
[architecture](architecture.md).
