# Unsafe boundaries

Safe callers must not manufacture register, interrupt, descriptor or DMA
ownership. An `unsafe` contract states only memory safety: memory the
hardware reaches, a raw descriptor's ownership, a symbol's ABI or placement.
Lifecycle order, power and membership are correctness, which typestate
holds without `unsafe`. The policy lives in standard Rust and Cargo lint
settings, so every `cargo check`, `cargo build` and `cargo clippy` enforces
it:

- Every crate above the boundary starts with `#![forbid(unsafe_code)]`. The
  layer test `every_crate_above_the_unsafe_boundary_forbids_unsafe_code`
  (`tools/test/layers`) checks each library and executable root against the
  boundary below, which it lists.
- A crate below the boundary starts with
  `#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]`, permits unsafe
  code only through documented item- or module-level allowances and retains
  `unsafe_op_in_unsafe_fn = "deny"` from `[workspace.lints]`. Being below is
  not blanket permission to add unsafe operations.
- A macro another crate expands emits no `allow(unsafe_code)`: `forbid`
  refuses that attribute even from a foreign macro (E0453), while the lint
  does not fire on the macro's own unsafe code. A crate that expands its own
  macro allows the unsafe code at the call site.
- `[workspace.lints.clippy]` denies `disallowed_methods`; `clippy.toml` lists
  them, including direct `core::mem::{zeroed, transmute}` and their `std`
  counterparts. An intentional use carries a local `allow` or `expect`
  with a reason. Crate-local Clippy configurations retain these bans.
- A clean panic lint is denied at the crate root while `[lints]` retains
  `workspace = true`. A non-test local `allow` or `expect` means the lint
  is not clean, even when the host emits no diagnostic. New panic-lint
  allowances belong only in tests. Remove existing non-test allowances as
  the crate is cleaned, then move each clean lint to `deny`. Compile-time
  checks remain allowed.

Every `unsafe` block and `unsafe impl` below the boundary needs a
`// SAFETY:` comment on the lines immediately before it, stating why its
prerequisites hold. A forwarding call inside an `unsafe fn` names the
`# Safety` contract it relies on. Put the comment below any
`#[allow(unsafe_code, ...)]` attribute: Clippy 1.97 does not accept a comment
above an attribute that spans several lines.

An extern declaration without a memory-safety precondition is declared
`safe fn` or `safe static`. A `safe static` requires the bytes at its address
to be a valid, immutable value of its declared type. A linker symbol used
only as an address is declared `safe static X: [u8; 0]`: reading this
zero-sized value touches no memory. Take its address with `&raw const X` or
`core::ptr::addr_of!(X)` without `unsafe`.

Use `#[derive(Zeroable)]` when it expresses the zero-initialization contract;
an `unsafe impl Zeroable` is reserved for types the derive cannot express.

`cargo clippy --all-targets` (the default members) applies these lints on the
host. Generated PAC code has no crate-root unsafe attribute.

## The boundary

Below it are, by directory (`/` ends a domain), for the S31 workspace.
C5 code lies outside the workspace and its checks:

| Kind | Directories |
| --- | --- |
| The volatile register-access core | `portable/mmio` |
| Semantic PACs and the HAL | `chips/esp32s31/pac`, `chips/esp32s31/hal` |
| Platform, executor, architectural instructions and framework bindings | `chips/esp32s31/board`, `chips/espressif/staged-bootstrap`, `chips/espressif/staged-runtime`, `chips/espressif/executor`, `portable/embassy/`, `portable/riscv` |
| Cores of memory the hardware shares | `portable/memory`, `portable/interrupt-table`, `chips/espressif/staged-layout` |
| The host's operating-system bindings | `tools/os/process`, `tools/os/serial` |
| The C-driver stand | `tools/vendor/ieee802154-esp32s31` |
| Bootstrap and probe images | `firmware/bootstrap/`, `firmware/probes/` |

## Generated access and trusted handwritten code

Typed register blocks are generated without `unsafe`: `cargo registers
generate --chip <chip>` renders every MMIO block type of the chip's register
model, with the owners of its policy's parts, into the chip PAC's
`generated.rs`. The unsafe code under a typed block is `oer-mmio`
(`portable/mmio`) alone, and the PAC root's one claim:

- `oer_mmio::assume_owned` promises exclusive ownership of the declared
  words, rather than every word of an overlapping address range. The chip's
  PAC takes those words once; the HAL root splits them into domain owners.
  Only the root's owned partitions authorize register access.
  A probe may reclaim a block only after dropping its previous owner.
  Generated const assertions exclude overlapping claims. Fatal paths mask
  their hart permanently and use the HAL's `StopTheWorld` proof to prepare
  reset; they do not revoke or take over another owner's peripheral.
- The volatile reads and writes of a `Block` and of a `Part`. Every word
  offset, stride and element count is a const generic, checked against the
  block's word list at compile time; an array index is an `Index<N>` from a
  checked constructor; the two parts of a split own disjoint word lists,
  checked at compile time, and a part writes only the words it owns. A
  block the policy divides into its interrupt-handler and task sides
  (`[[register-sides]]`, owners `<Block>Irq` and `<Block>Task`) is checked
  at compile time to give every word to exactly one side. Two joined parts
  own every word of their block, checked at compile time.
- A division of a block into up to four parts (`Block::divide`), whose
  owned words are disjoint and whose shared write-trigger words no part
  owns, each part holding its own bits of them, checked at compile time;
  `Part::write_trigger` stores only the part's bits, zero in the others.
  A device word is not Rust memory, so the parts' stores from several
  contexts are no data race; a host part is `!Send`.
- `oer_mmio::Reader`, the `Copy` read-only authority a block or part
  becomes when it gives up its writes: no owner writes the words it took
  over, and every copy only reads.
- The `Sync` implementation of a shared block at a fixed address
  (`oer_mmio::SharedRegisters`, a policy's `[[shared-blocks]]`) and its
  `write_shared`: a device word is not Rust memory, and the block's writes
  are whole-word stores that read nothing back, so every execution context
  reaches its one owner through `&self`.
- `oer_mmio::fence`, the device-access ordering instruction.
- `VolatileCell`, the volatile word of the generated memory layouts; both
  the cell and the layouts derive `Zeroable`. The constructors of
  `DmaWindowAddress` and `DmaRef` are `unsafe`.

A generated register writer takes typed field values only: `bool`, a value
enum, an exact-width integer or `oer_mmio::Bits<WIDTH>`; a reader returns
the same type, so a value read back is written again without a
conversion. A read-modify-write keeps every other bit, reserved and
undeclared ones included; a write-one-to-clear register, or the clear
register of a status-to-clear snapshot, is acknowledged with exactly one
affine sample. A policy fixed image on a snapshot's clear register
acknowledges events no sample captured, so it names its proof: `masks`,
its enable register, whose bits of the image the same transaction clears
before the store, so no enabled event is acknowledged unsampled; or
`consumed-event`, the reviewed reason its one caller consumes that event
itself. A DMA address field is written only from a typed DMA
reference: its policy transaction or the field's image of a `DmaRef`. Each rendered
transaction asserts its masks against the register's declared fields in a
`const` item beside it. The one exception is a policy fixed image marked
`whole-word`: the vendor's store of a whole word, with its evidence
sources, which also writes the bits the model leaves undeclared.

`esp32s31/pac/` is the generated PAC (`src/generated.rs`) and its root
(`src/lib.rs`, the one claim); it has no handwritten module
([PAC](../chips/esp32s31/pac/README.md)).

The HAL's core-control partition owns `LP_SYS.SYS_CTRL` and
`BOOT_ADDR_HP_CORE1` alongside its stall, clock and reset words. Bootstrap
containment and fatal reset use those owned typed accessors, with no ROM
FFI. A mixed-word trigger preserves ordinary fields and unassigned bits,
writes zero to every WTC lane and every other WT lane, and asserts only its
selected WT field; it grants no generic modify of a WTC word. The reset
path keeps the existing ClaimCell election through the trigger, then waits
for interrupts forever if hardware has not reset it. A path refused by
that election retains the existing watchdog outcome.

The crates below cover distinct obligations: singleton acquisition and MMIO
serialization, stable addresses and CPU/DMA transfer, target ABI and placement,
and one-time static resource or interrupt binding. The embassy network
adapter binds Xarxa's external-packet API, which is unsafe by design, in one
unsafe module, `external_rx`: it adopts a detached RX DMA buffer from an
`oer-memory` handoff slot as a Xarxa `PacketBuf` (`ExternalPacketOrigin::new`
and `adopt`) and returns the slot through `release_adopted` when the packet
drops. A slot is adopted again only after that release returned, because the
radio must first claim the slot the release freed. Preserve the proof at the
smallest operation; a safe state machine in a crate below the boundary
remains safe.

`oer_esp32s31_hal::bluetooth::engine` owns the controller-SRAM protocol.
One `Engine` owns the register token and the complete `Placed<BluetoothArena>`;
its private components retain graph bindings. The codecs
and scheduler lists encode the addresses the controller follows and hold the
narrow unsafe publication boundary. Driver role builders select detached
values, never layout references; HAL checks their binding anchors before
writing them. Pool and epoch brands bind each affine lease to its owner;
logical item identities also carry an acquisition generation.
The same engine moves through initialization, activation, scheduler workers,
stop and reset. PHY/DF owners cannot be detached. Address publication and
completion return go through the engine; raw proofs and address getters stay
private. Finished-list receipts carry the capturing engine installation epoch, so a
fence from before reset cannot authorize a later installation's completion. Opaque
projections and public scheduler Debug output do not expose physical links.
`scheduler::hardware::live` publishes list-zero items only after resolving
each address to a submitted item of the `'static` scheduler pools, in the
executor's order. The BLE PHY engine, the direction-finding workspace, the
device table and the receive chain each mint, in one place, the HAL proof of
the storage the hardware is pointed at (`BlePhyHardwareObjects`,
`DirectionFindingDescriptor`, `DeviceTablePublication`, `RxChainHead`),
whose constructor is unsafe; the HAL publications that take them are safe.
The private placement adapter consumes `Placed`, projects fields within its
allocation and calls the HAL's typed-address constructors. `engine::arena`
retains the root pointer, then derives every field borrow from that pointer.
After a verified reset, consuming all graph owners ends their borrows; the
backing is dropped in place to end its pin contract and zero-initialized in
place before returning the original `Placed` allocation. No field reference
escapes this return. Constructor refusal returns both original inputs before
mutation. Unconfirmed stop and reset failure retain the whole graph with its
register partitions in absorbing quarantine; Drop never returns a placement.
The P2 engine holds only the task endpoint of `scheduler::interrupt`'s static
request/reply channel. Its unique P3 endpoint stays beside the IRQ register
owner. Release/Acquire state transitions transfer exclusive access to an
`UnsafeCell` payload: P2 writes an empty slot, P3 consumes its request and
publishes its reply, then P2 consumes that reply exactly once. Neither side
accesses the other's phase, and no task waker enters P3. The static typed
signal carries only reply readiness. Installation epochs and command numbers
that never wrap or reset reject stale replies. Abandoning a pending future
retains the request seat, quarantines graph publications and prevents
retirement. Reusing the slot consumes both quiet endpoints after scheduler
stop and route removal. Adjacent tests
cover delayed replies, duplicate entries, stale replies, registration races,
abandonment and endpoint reunion; a paired UI case rejects repeated retirement.
The adjacent `engine::owner::tests` cover exact-allocation retry, completion,
stop, reset, stale epochs and both terminal failure paths; paired UI cases
cover moved inputs, private custody and stale leases. Its range metadata
holds no reference that could alias the retained owner. The RX byte storage
and its transparent volatile byte cells derive `Zeroable`; this permits
zero initialization and grants no concurrent access.
An `unsafe` contract there states only the memory the hardware reaches: the
Controller's lifecycle order, power and PHY membership are correctness,
which the driver's typestate holds, and the driver forbids unsafe code. The
scanner start takes the scanning list's publication of the same Controller
epoch, so a list another epoch published, or a reset head, never starts
it.

Each chip's HAL has one module that makes typed DMA addresses,
`dma_address`: the only caller of the `unsafe` `DmaWindowAddress` and
`DmaRef` constructors of `oer-mmio`, which the PAC's generated `layouts`
bind to the chip's DMA window (clippy `disallowed-methods` refuses them
elsewhere). Its `placed`/`placed_link`
take an address of storage the caller keeps pinned in the DMA window: a
placed static's descriptors and buffers, or a placed pool's buffer
(`PlacedBacking`); each call site names its proof.

Its `dma_static` module holds the placement constructor:
`DmaStatic::placed`, `unsafe` and called only by the `dma_static!` macro,
whose section is checked to be `.dma.bss.<block>`; the runtime linker
script's ASSERTs over the DMA range (from the generated `dma-window.x`) are
the proof.

The 802.15.4 HAL engine consumes its `Placed` buffers and constructs the
complete address table at one unsafe site. Every projection names a frame
inside the retained allocation; subsequent register publications take those
typed addresses. The shared engine and the combined engine/MAC owner contain
no unsafe code and perform no address arithmetic.

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

`oer-memory` holds the shared proofs for executor-crossing storage.
Its `init_in_place!` macro writes a complete struct into exclusively borrowed
`MaybeUninit` storage, one field at a time. An unused constructor checks field
completeness, uniqueness and visibility. Cleanup guards retain initialized fields until
the complete value is published; unaligned access also supports packed
structs. The returned loan retains the storage lifetime. Caller expressions
run outside the macro's unsafe context, and callers keep `forbid(unsafe_code)`.
The macro resolves storage and pointer operations through its defining crate,
so a caller cannot replace them by shadowing `core`.
Nested `field => initializer` placement lends that field as `MaybeUninit` only
after checking its alignment. The returned valid reference must have exactly
the destination address before a cleanup guard or the whole struct is exposed.
Thus a callback cannot substitute another allocation for an unwritten field.
Cached SPSC placement initializes its cursor words directly and composes with
this nested form in the Wi-Fi RX arena; compact P1/P2 queues retain their layout.

`InlinePlace` owns optional inline storage. Initialization checks the returned
address before publishing its initialized flag; clearing removes that flag
before running the destructor. Interrupted initialization leaves the place
empty. `Placed::replace_into` swaps two distinct, exclusively owned initialized
values directly between their places, then drops the previous destination in
the consumed loan's storage. If that destructor unwinds, the new destination
remains initialized and exclusively owned.

`StaticPlace` permanently claims an exclusive static allocation, then lends
uninitialized storage with the actual value's shorter lifetime. Its layout
type supplies only size and alignment, checked at monomorphization; no
initialized value or reference is reinterpreted and no loan is extended.
Unclaimed backing is `Send` regardless of its layout type: it contains no
initialized layout value, and its permanent static loan prevents moving it
after initialization. The initialized handle keeps its own thread restrictions.
`PlacedFuture` owns one future constructed in that allocation, pins it once,
and polls and drops it in place. Only the handle can move. Forgetting it
permanently retains the backing; no safe reset or reuse path exists.

`AffineSpscQueue` lends short local endpoints without new unsafe operations.
The original endpoint remains exclusively borrowed. A local producer loan can
be copied within its one context, supports only synchronous publication and
has no reservation; both local endpoints are neither Send nor Sync. The
underlying Release/Acquire transfer still moves each complete owner once.
Its sealed cursor layouts retain the default cache-line-separated counters
for existing queues and permit compact atomic counters for the same-core
P1/P2 handoff. Layout selection changes no endpoint or publication authority.
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

`OnceValue` retains one value. The winning atomic claim owns the write;
an Acquire reader can copy a Copy + Sync payload only after Release publishes
it. Other owners stay opaque until exclusive reset/destruction. A preempted
writer is already observable as a request, but readers never wait for it.
Reset requires exclusive access after the old endpoints ended. The S31 port
uses it for the first fault cause and durable retention of failed messages.

`ReturnBank` pairs each taken value with one affine return token. An Acquire
CAS transfers the initialized value out of its cell; only the consumed token
can write a replacement and publish it with Release. A dropped token leaves
the cell unavailable. The S31 port's free DMA buffers carry these tokens
through both execution contexts, so return-on-drop shares no producer or lock.

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

`ClaimCell` holds one value by value, installed once, and lends it to one
holder at a time until one keeps it for good: the chip's root installs the
USB Serial/JTAG console's parts there for the early writer, the fatal note
and the console's driver. Its state (`EMPTY`, `INSTALLING`, `FREE`, `LENT`,
`TAKEN`, `SEALED`) admits one reference at a time: `install` writes the
value before a Release store of `FREE`; `lend`, `take` and `seal` each win
one Acquire exchange from `FREE` before they reach the value. `lend` creates
the only `&mut T` and returns the state to `FREE` once that borrow ended;
`take` reads the value out once and `seal` lends it once, and neither state
is ever left. `hold(key)` keeps the value for one key for good (a hart's
fatal path holds the core-control words): its exchange from `FREE`, or from
that key's held state, to the key's lent state admits one call at a time,
which returns the state to the key's held state; no other key or claim
reaches the value again, and an enclosing call of the same key refuses a
nested one. A caller that does not find its state is refused without
waiting. It is `Sync` only for `T: Send`, and its drop drops a value it
still holds (`FREE`, `SEALED` or held).

`platform-staged-layout` declares the stage-two handoff contract: the unsafe
trait `AdoptsBootstrapMapping`, a board's promise that its adoption of the
bootstrap's PSRAM mapping neither resets nor remaps PSRAM, and the
`StageTwoHandoff` proof only the staged runtime's entry mints. It holds no
other unsafe code.

## The host's operating-system bindings

`oer-os-process` holds the host's raw system calls that rustix does not
make safe: `child_setup` for what a child does between fork and exec,
`launch_handoff` for the descriptor a fixed launcher passes its helper
across `exec`, and, behind the `linux-socket` feature, `linux_socket` for the
link-layer, HCI and L2CAP addresses and three socket options. `oer-os-serial`
takes an open serial port's descriptor; it is its own crate so that the root
fixture installer, which links `oer-os-process`, does not link `serialport`.
The 802.15.4 host stand binds the pinned ESP-IDF C driver through its FFI.

## Firmware and platform code below the boundary

`oer-espressif-executor::port_slot` binds a nominal port identity to one
static allocation. `declare_port_slot!` irreversibly claims its private
`StaticCell` before constructing `SlotClaim<StaticId<Declaration>>` at one
unsafe site. The claim has only invariant `PhantomData`; its private safe
constructor remains inside the portable contract. The declaration must
never issue a second authority for the same type, including after software
reset or quarantine, because owned queue permissions rely on that identity
having exactly one backing store. Static placement itself uses `StaticCell`'s
safe initialization; no reference lifetime or owner is cast. Local scope
claims have no static latch and use no unsafe code.

`StaticPortSlot` keeps the declaration's permanent allocation in its P2 task.
It lends that allocation once initially, and again only after consuming its
nominal `Joined`. That proof follows the end of P1 endpoint loans and P2
owner/invocation destruction and unregistration; quarantine issues no proof.
The adapter's two pointer dereferences extend no stack allocation's lifetime.
The S31 composition sends only frontend endpoints to P1 and retains the slot
manager and registration in its P2 task, establishing the cleanup Drop context.

The platform crates (the boards and `chips/espressif/staged-*`), the bootstrap images
(`firmware/bootstrap/*`) and the probe images (`firmware/probes/*`) own the
target ABI and placement, so they hold unsafe code that no crate above them
needs:

- The staged runtime defines the image entry (`entry!`, the `Handoff` and
  `Adopted::enable_interrupts`), the sections the linker script maps
  (`placed_static!`, `retained_static!`), the panic hook and
  exception handler symbols (`panic_hook!`, `exception_handler!`), the trap
  CSRs (`trap`) and the linked sections (`layout`). Images and HIL agents
  reach these through safe APIs and forbid unsafe code. The image linker
  places strict code in SRAM from interrupt, exception and panic analysis.
- Each board implements `AdoptsBootstrapMapping` for its PSRAM.
- The bootstrap places the packed stage two and the flash tuning reference in
  its own read-only flash sections.
- A probe image is entered by the Blobray verifier through the exported
  symbols the vendor code calls (`_start`, `ets_delay_us`, the probe
  entries), with that code's C signatures, and keeps its fixtures in statics
  the verifier's serial calls reach one at a time.

The bootstrap and probe crates deny `clippy::undocumented_unsafe_blocks`, and
each unsafe attribute in them carries its `// SAFETY:` comment as well.

## PAC dependency authority

Direct dependencies on the PAC are restricted independently of unsafe
syntax. The only allowed consumer is the chip's `hal` under `esp32s31/`:
drivers and adapters reach registers through HAL owners. Check the executable list when changing this boundary.

Upper layers use opaque, finite capabilities rather than raw pointers,
unchecked lifetimes, generic PAC callbacks or independently reusable interrupt
owners. MMIO outside the restricted PAC must use typed accessors. Missing
register fields are reviewed in `chips/<chip>/registers/` and regenerated
into the PAC, never reconstructed with local masks in a HAL or adapter.

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
