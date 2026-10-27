# Unsafe boundaries

Safe callers must not manufacture register, interrupt, descriptor or DMA
ownership. An `unsafe` contract states only memory safety: memory the
hardware reaches, a raw descriptor's ownership, a symbol's ABI or placement.
Lifecycle order, power and membership are correctness, which typestate
holds without `unsafe`. The policy lives in standard Rust and Cargo lint
settings, so every `cargo check`, `cargo build` and `cargo clippy` enforces
it:

- Every sans-IO crate and every crate above the boundary starts with
  `#![forbid(unsafe_code)]`. The layer test `every_crate_above_the_unsafe_boundary_forbids_unsafe_code`
  (`tools/test/layers`) checks each library and executable root against the
  boundary below. Sans-IO membership is defined in [architecture](architecture.md).
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
  with a reason. Every `clippy.toml` in the tree retains all common method
  bans; `every_clippy_config_retains_all_common_method_bans` checks this.
  The root also sets `suppress-restriction-lint-in-const = false`: a
  runtime-callable `const fn` is not a compile-time-only check.
- A clean panic lint is denied at the crate root while `[lints]` retains
  `workspace = true`. A non-test local `allow` or `expect` means the lint
  is not clean, even when the host emits no diagnostic. New panic-lint
  allowances belong only in tests. Remove existing non-test allowances as
  the crate is cleaned, then move each clean lint to `deny`.
  `tools/test/layers/sans-io-lints.toml` is the canonical sans-IO catalogue:
  assertion macros (including `vendor_assert!`), panicking core methods and
  `core::task::Waker::noop`. A sans-IO crate with explicit crate-root deny of
  both `disallowed_methods` and `disallowed_macros` has a local `clippy.toml`
  containing that entire catalogue, all common root bans and its existing
  local entries. Layer tests enforce catalogue membership; they do not
  replace Clippy or prove transitive absence of panics.

Sans-IO code contains no `unsafe` and no runtime panic paths, including
`panic!`, `unreachable!`, `todo!`, `unimplemented!`, assertions and debug
assertions, `vendor_assert!`, `unwrap`/`expect`, indexing or slicing that can
panic, arithmetic overflow or invalid division, and panicking core methods.
Those paths return typed errors. Sans-IO crates do not depend on fatal,
reset or retained-record owners and do not reset: reset belongs to the
image or HAL owner. Compile-time checks use a const item or inline const
of the form `const { if !cond { panic!("…") } }`; assertion macros are
forbidden in these checks. Panics evaluated at compile time and test
assertions are allowed. A `const fn` alone does not require compile-time
evaluation.

Every `unsafe` block and `unsafe impl` below the boundary needs a
`// SAFETY:` comment on the lines immediately before it, stating why its
prerequisites hold. A forwarding call inside an `unsafe fn` names the
`# Safety` contract it relies on. Put the comment below any
`#[allow(unsafe_code, ...)]` attribute: Clippy 1.97 does not accept a comment
above an attribute that spans several lines.

Memory operations move into `oer-memory`, architectural instructions into
`oer-riscv`, and PSRAM slice creation into its HAL owner. Their unsafe counts
may grow only by the relocated operations while the branch total falls and
no calling crate's count grows. Runtime panic counts never increase. Image
panic budgets report counts and sites without gating builds; zero reachable
panics remains the code goal.

Compiler-runtime coverage reviews explain missing frame metadata only; they
supply no trusted frame size or callee bound. A frame without metadata needs
complete local control flow and a known entry-relative stack pointer.
Fall-through and call continuations outside the function, conflicting
boundaries and stop edges leave its local graph open. Unresolved callees
still leave the stack bound unresolved. Captured indexed
addresses require the same complete read-only table and target proofs as
straight-line dispatches; relocation labels alone do not prove immutability.
Conditional branches out of a function count as tail transfers, like direct
jumps: their destination's bound counts below the source's retained stack
depth. Missing entries, interior targets and unknown frames remain unresolved.

Only `oer-memory` defines `unsafe impl Sync` or `Send`; the layer test
`unsafe_sync_and_send_impls_stay_in_oer_memory` maintains the shrinking
file-to-count list of existing exceptions and rejects new or stale entries.

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
| Volatile register words and their claim | `portable/mmio` |
| Semantic PACs and the HAL | `chips/esp32s31/pac`, `chips/esp32s31/hal` |
| Platform, interrupt bindings, executor and architectural instructions | `chips/esp32s31/board`, `chips/espressif/staged-bootstrap`, `chips/espressif/staged-runtime`, `chips/espressif/interrupt-table`, `chips/espressif/executor`, `portable/riscv` |
| Cores of memory the hardware shares | `portable/memory`, `chips/espressif/staged-layout` |
| The host's operating-system bindings | `tools/os/process`, `tools/os/serial` |
| The C-driver stand | `tools/vendor/ieee802154-esp32s31` |
| Bootstrap and probe images | `firmware/bootstrap/`, `firmware/probes/` |

`oer-espressif-interrupt-table` owns the Espressif interrupt matrix contract,
CLIC encodings, vector ABI and staged-runtime symbols. Its narrow unsafe
contracts cover unique entry tokens, handler-part storage, signal execution
contexts and exported symbols. It depends on the memory and MMIO cores;
chip crates and firmware consume its bindings directly.

## Generated access and trusted handwritten code

Typed register blocks are generated without `unsafe`: `cargo registers
generate --chip <chip>` renders every MMIO block type of the chip's register
model, with the owners of its policy's parts, into the chip PAC's
`generated.rs`. The unsafe code under a typed block is `oer-mmio`
(`portable/mmio`) alone, and the PAC root's one claim. A policy
`register-image-reads` operation reads one side-effect-free 32-bit word
through its existing owner, including a read-only owner. Every declared
field must have a defined observational class: plain, read-only,
read-observed, set-only, self-clearing, hardware-written or debug-only.
Write-only, write-trigger, write-one-to-clear, clear-on-read and unknown
fields reject the entire word even beside readable fields. FIFO and other
read side effects also reject it. An admitted read returns the complete
`u32`, including undeclared and reserved bits, without decoding, masking,
writing or acknowledging it. Splitting fields grants no additional register
authority or unsafe access. `oer-mmio` holds
only volatile register words and their access authority (DMA addresses and
volatile DMA-memory cells live in `oer-memory`):

- `oer_mmio::assume_owned` promises exclusive ownership of the declared
  words, rather than every word of an overlapping address range. The chip's
  PAC takes those words once; the HAL root splits them into domain owners.
  Only the root's owned partitions authorize register access.
  A probe may reclaim a block only after dropping its previous owner.
  Generated const assertions exclude overlapping claims. The HAL's safe
  `cores::fatal` masks its hart before lending `Fatal` and halts if the path
  returns. Keeping interrupts masked is a correctness requirement, not an
  unsafe precondition: safe architectural APIs can enable them. Fatal paths
  use `StopTheWorld` to prepare reset; they do not revoke or take over
  another owner's peripheral.
- The private `DeviceWord::{load, store}` pair implements volatile reads
  and writes for `Block`, `Part` and `Reader`. Every word
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
- The marker-derived `Sync` of a shared block at a fixed address
  (`oer_mmio::SharedRegisters`, a policy's `[[shared-blocks]]`) and its
  `write_shared`: a device word is not Rust memory, and the block's writes
  are whole-word stores that read nothing back, so every execution context
  reaches its one owner through `&self`. `Registers::Sharing` is
  `sharing::Shared` for those blocks and `sharing::Owned` otherwise;
  owned blocks and parts remain `!Sync`, and host placements also stay
  `!Send`. No unsafe sharing implementation is needed.

`oer-memory::VolatileCell` is the volatile word of generated DMA-memory
layouts; both the cell and the layouts derive `Zeroable`. `DmaWindow`,
`DmaLayout`, `DmaWindowAddress` and `DmaRef` live beside `DmaBytes` in
`oer-memory`. Their two unsafe constructors preserve the placed-storage
and retention contracts.

`oer-riscv` (`portable/riscv`) is the only crate with `asm!` for architectural
operations: CSR access, hart and cycle reads, interrupt masking, CLIC threshold,
debug triggers, waits and barriers. Its safe functions have no memory
precondition. Installing trap vectors and `mscratch` is unsafe because the
entries, table and exclusive interrupt stack must implement a live trap ABI.
Entering stage two is unsafe: `jump_to_entry` requires the checked entry of a
published image that installs its own stack. Its inlined assembly disables
interrupts, executes both instruction-publication barriers and jumps to the
constant entry in one block.
Every instruction site has one narrow `// SAFETY:` proof. Moving instructions
into this crate may increase its unsafe count only while the branch total
decreases and no caller's count increases; runtime panic counts never increase.
Device ordering is `oer_riscv::fence`, including the host compiler fence;
`oer-mmio` contains no architectural instruction or barrier API.

The instruction module and its exports exist only on RISC-V. Staged firmware
binaries gate their entry composition and hardware dependencies on the same target;
their build scripts emit image linker arguments only for that target.
Host tests compile pure modules with explicit fixture inputs rather than
providing synthetic CSR reads. Their host entry is empty, as in B0 and W3.

Interrupt stack install takes a local `&mut PreAdmission<H>` and is safe.
The boot handoff wraps its unique entry proof with that token, retains it
through adoption, and consumes it when enabling interrupts, returning the
router and its existing `Hart<H>` together. `Adopted::parts` lends the hart
and mutable router together before enable. Executor startup const-checks the
entry's hart against this token; task wakers stay local to that hart. `Hart<H>` and
`PreAdmission<H>` are neither Send nor Sync and have no public constructor.
Installation checks the executing hart and masks local interrupts before
painting the inactive allocation and installing its trap registers. Its
memory operations remain in the private `stacks::watermark` module; the
trap-register writes remain in `oer-riscv::install_trap_stack`, each with
its narrow safety contract.

The HAL publishes every hart's side-effect-free pending bank at root split.
Its bank owners and exhaustive selection are generated from the same hart
geometry as the vectors. Pending and debug reads take the PAC's checked `Hart`;
they do not claim registers or grant execution authority. The executor validates
the architectural ID before selecting its running-context slot or these reads.
An invalid ID follows the retained `InvalidHart` reset path, releasing
no owner. These selections use no unsafe code.

`Doorbell::new::<Member>` returns a typed `UrgentSource<Member>` beside its
ringing owner only after its const member/line/binding checks. The observation
contains no register owner or waker; copying it permits both local executors
to observe the same urgent source. A sealed `UrgentSources` contract accepts
that observation or `()`; executor startup cannot take a raw numeric list.

Executor and time-driver setup return route or counter-latch refusals through
`Result`. Each `Failure<T>` holds its inputs in `ManuallyDrop`; dropping a
refusal cannot release those owners or authorize retry. Internal invariant
failures carry a named `Fault` with the original observations. The terminal
path masks execution through `fatal::reset_recording` and keeps all static,
handler and interrupted task owners until reset. A numeric reason code and four
exact 32-bit observation words reuse the existing 148-byte panic slot under
tag `NAMED_FAULT = 3`, with its checksum; no retained bytes are added. Capture
reads no reason text, including from an interrupt. Adoption decodes the code
in thread context, prints it through the early console and clears it only after
successful output. The cell's claim lives in boot-zeroed SRAM and stays held
through a fatal reset, preserving the current boot's first record. A fresh
boot's first fatal claim replaces stale retained bytes even without previous
record consumption or successful console output.
Software lines use fallible static placement and one-time value publication,
without a panicking initialization path or additional unsafe code.

`Router::bind` returns ordinary input refusals with their unchanged token
and part. Once admitted, storage, handler installation and controller write
failures return a named `BindFault`; the token and any uninstalled part stay
in `Quarantined`, whose private `ManuallyDrop` fields have no recovery API.
Placed parts retain their claimed static allocation; installed parts remain
in their handler cell. No failed binding issues `Bound` or enables a driver.
The image keeps its router and all interrupted owners while the staged
runtime's binding reporter records the complete source/line/hart, operation
and controller error through `fatal::reset_recording`. The existing panic
slot holds four lossless diagnostic words; no retained allocation is added.
Successful binding still installs the part, writes the line, then routes
sources in declaration order. A failed write attempts no later write or
rollback. No unsafe code is added to placement or ownership transfer.

The generated Rust MTVT is one immutable array of aligned per-hart rows.
Its one unsafe attribute owns the SRAM section. The table and trap assembly
share a module/codegen unit; the existing `global_asm!` exports `_runtime_mtvt`
through a `sym` operand without a Rust export attribute.
Entries are optional references to opaque zero-sized address markers
declared inside `layout`'s existing extern block: reading a marker touches
no instruction memory and Rust cannot call it as a function. Compile-time
checks enforce the RV32 slot width and row size. Machine entry/exit code
remains in `global_asm!`, with frame operands derived from `TrapFrame`.

Outside `oer-riscv`, firmware assembly is only `global_asm!`/`naked_asm!` for
entry/trap ABI work: staged-bootstrap's `entry.rs`, staged-runtime's `entry.rs`
and trap-frame/stack entries in `stacks.rs`, and bootstrap/probe images. The
layer test
`architectural_instructions_stay_in_oer_riscv` lists these owners and locations
and rejects inline operations even there. Host compiler fixtures are test
inputs, not firmware instruction owners.

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

`read-observed` fields expose observations without asserting hardware
read-only behavior or writable-storage semantics. A word containing one has
no generic writer, RMW, command or mixed trigger that could replay it.
Its only publication is a separately sourced `zero-register-writes`
operation: one complete zero store with no preceding load. This capability
establishes neither a reset image nor a pending-clear effect. Wi-Fi MAC RAW
uses it; sampled acknowledgement continues through CLEAR.

Generated SRAM-layout builders check a narrow constant field value through
`with_<field>_constant::<VALUE>()` at monomorphization, including calls from
runtime code. The setter preserves every other bit. A dynamic field value
uses the checked `with_<field>(value)` returning `Option`; declaring its
caller `const fn` does not turn that admission into a compile-time proof.

`esp32s31/pac/` is the generated PAC (`src/generated.rs`) and its root
(`src/lib.rs`, the one claim); it has no handwritten module
([PAC](../chips/esp32s31/pac/README.md)).

The HAL's core-control partition owns `LP_SYS.SYS_CTRL` and
`BOOT_ADDR_HP_CORE%s` for the sourced secondary harts alongside indexed
stall, clock and reset words. Bootstrap
containment and fatal reset use those owned typed accessors, with no ROM
FFI. A mixed-word trigger preserves ordinary fields and unassigned bits,
writes zero to every WTC lane and every other WT lane, and asserts only its
selected WT field; it grants no generic modify of a WTC word. The reset
path keeps the existing ClaimCell election through the trigger, then waits
for interrupts forever if hardware has not reset it. A path refused by
that election retains the existing watchdog outcome.

The PAC's `Hart` is generated from the platform's HP-hart list, with checked
raw conversion and fixed-index admission inside inline const. Its exhaustive
selection of per-hart arrays adds no runtime bounds panic. `SecondaryHart`
names only nonboot harts and their ROM boot-word indices. The core-control
clock hold and release methods require this type, excluding the boot hart.
Neither type grants execution or register ownership. The HAL's fatal proof
carries a checked identity or `Fault::InvalidHart { raw }`; an invalid identity receives no
hart's ClaimCell key. The staged fatal path retains its full raw ID in the
existing record slot and requests reset through a sealed free control owner;
a lent or held owner is never taken over. An unavailable reset owner retains
the existing terminal watchdog outcome. Runtime adoption emits the previous boot's fault through
the existing `FatalFail` note, including images without a previous-panic reader, clearing the slot only after successful output.
No new unsafe boundary is involved.

The crates below cover distinct obligations: singleton acquisition and MMIO
serialization, stable addresses and CPU/DMA transfer, target ABI and placement,
and one-time static resource or interrupt binding. The Embassy network
adapter stays above this boundary and forbids unsafe code. It publishes
packet-pool owners without adopting detached DMA storage. Preserve the proof
at the smallest operation; a safe state machine in a crate below the boundary
remains safe.

`oer_esp32s31_hal::bluetooth::engine` owns the controller-SRAM protocol.
One `Engine` owns the register token and the complete `Placed<BluetoothArena>`;
its private components retain graph bindings. The codecs
and scheduler lists encode the addresses the controller follows: `DmaRef`
and the volatile arena carry the memory contract, while `&mut` register owners
and typestate enforce publication order and lifecycle without `unsafe`.
Driver role builders select detached
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
`DirectionFindingDescriptor`, `DeviceTablePublication`, `RxChainHead`)
through safe crate-private constructors. Their typed `DmaRef`s carry the
complete retained layout's extent and alignment; the device-table count is
an `Index<capacity + 1>` admitted before table mutation, with its register
width checked at compile time. The HAL publications that take them are safe.
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
owner. The channel uses `oer-memory::RequestSlot` in `InPlace` mode, with no
stored wakers. Release/Acquire state transitions transfer exclusive access:
P2 posts the exchange, P3 borrows it in place and writes its reply, then P2
validates and consumes the exchange exactly once. A rejected reply stays in
place. Neither side accesses the other's phase, and no task waker enters P3.
The static typed signal carries only reply readiness. Installation epochs and command numbers
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
`DmaRef` constructors of `oer-memory`, which the PAC's generated `layouts`
bind to the chip's DMA window (clippy `disallowed-methods` refuses them
elsewhere). Consuming constructors select the base exclusively through sealed
Wi-Fi/native or controller address spaces. Only a `cfg(test)` constructor
accepts a synthetic base argument. A single HAL placement projection retains
CPU and DMA geometry without borrowing the retained owner. Its safe `link`,
`table`, `slots` and `region` methods check full extents and alignment before
calling `placed`/`placed_link`; private table/slot/region constructors take
typed starting addresses. Fixed type geometry is checked in const blocks;
dynamic refusal is `ProjectionError`.

`PinnedDmaTxPool` retains each slot's contiguous headroom, frame and trailer
bytes beside its CPU ownership metadata. Affine leases and atomic state
transitions govern publication and return. A radio lease exposes only those
bytes to DMA; its owner must prove hardware quiescence before release.
Counting claimed slots observes ownership state and grants no release authority.

`PlacedTxPool` validates each byte region before pinning the pool. The memory
crate exposes only the slots' location observations, without access or
lifetime authority. Each `PlacedTxLease` carries its checked immutable DMA
region, and permits mutation of bytes through the stable-region loan without
allowing replacement of its backing. `PlacedBacking::dma_region` only returns
that cached proof without width, emptiness or containment admission.
A rejected A-MPDU backing insertion returns the last slot and clears its
metadata before reporting refusal; an empty region follows the same LIFO
release path. The BLE adapter uses the same projection; DF workspace overlays
check type geometry in const blocks, derive positions from generated member
accessors and return typed errors before arena mutation. DF preflight returns
the original `Placed` on refusal. After verified reset, the arena's existing
unsafe reclaim block calls `Placed::from_retained`: its contract requires the
exact original placed allocation, with field borrows and hardware use ended.
No safe constructor can restore placement from an arbitrary static reference.

Its `dma_static` module holds the placement constructor:
`DmaStatic::placed`, `unsafe` and called only by the `dma_static!` macro,
whose section is checked to be `.dma.bss.<block>`; the runtime linker
script's ASSERTs over the DMA range (from the generated `dma-window.x`) are
the proof.

The 802.15.4 HAL engine consumes its `Placed` buffers and constructs the
complete address table through the safe placement projection. Refusal returns
the unchanged MAC and placed buffers. Every projection names a frame
inside the retained allocation; subsequent register publications take those
typed addresses. The shared engine and the combined engine/MAC owner contain
no unsafe code and perform no address arithmetic.

The S31 802.15.4 HAL's runtime acknowledgement consumes an affine
`Ieee802154EventSnapshot`; its copyable observation grants no write authority.
Startup is a separate, typed pre-bind discard. `arm` consumes the task and
inactive IRQ owners, masks every event, installs the abort baseline, issues
STOP and both timer stops, then writes the fixed W1C set of all fourteen event
lanes through the existing `Ieee802154MacIrq` part. This write does not use a
status sample: every pre-bind event is stale and cannot be serviced. Its
register read preserves undeclared bits only; masked zero status cannot narrow
the discard. The policy records this consumption proof and the generated
accessor preserves undeclared bits rather than treating it as a whole-word
vendor image. A word consisting entirely of W1C fields is its own status/clear
pair and needs the same explicit fixed-image proof as a named pair.

Only after the final fence does `arm` return the bindable IRQ owner and an
affine task owner without operational methods. `Router::bind` transfers the
IRQ owner permanently into its handler. Consuming task activation with its
`Bound` proof publishes the same ten runtime events as later sessions; it
writes only task-owned enables and never borrows `HandlerCell`. A polled pair
instead requires `Silent`; its startup discard has already completed. The
pre-bind transaction enables no event lane.
Masked acknowledgement is measured for timers0/1 (HW8) and RX_ABORT reason26
(HW26). Hidden pending and masked visibility on 7/10/13 remain unmeasured;
neither a zero read nor the discard is an all-lane visibility or producer-drain
proof. Deactivation issues STOP and both timer-stop commands, samples before
masking and acknowledges the saved sample. Commands and selected
acknowledgements do not prove producer drain.
Engine interrupt service rejects a sample containing lane 7, 10 or 13 with
`Fault::UnhandledEvents` before acknowledgement or dispatch, preserving
all named events, each unnamed lane and the entry state. The consuming
engine/MAC owner becomes an absorbing quarantine with no recovery API.
The PHY HIL image records the fault through safe `fatal::reset_recording`
without dropping its quarantined owners. The new record adds 12 bytes of
retained data to the PHY HIL image: magic, a diagnostic word packing all fourteen
event flags, state and reason, and checksum. It prints at the next boot and
remains retained on output refusal.
Malformed secured ACK output from the environment is refused as API input.
The engine issues STOP, reports the typed security error to its environment,
delivers the received frame without an ACK and selects the next operation.
It writes no security setter or TX DMA publication and returns no fault;
no retained record is added or extended.
During ACK reception the engine stops timer0, samples and clears its event
while enabled, then masks
it. A new ACK wait enables and clears old pending while timer0 is stopped,
before programming and starting it. The HAL never resamples status as the
authority for acknowledgement. Same-lane recurrence is not an event count.
SOURCE: `hw8-20261026-radio-local-05`, `ieee-timers.json`, retained-snapshot
and selective-W1C phases, [PR #689](https://github.com/ermacv/open-esp-radio/pull/689).

Wi-Fi RX completed byte views and ring-owned return-state normalization consume
crate-private affine grants. A sealed binding pairs one arena with its live
or halted ring after identity admission, or takes the pair directly from a retained completed
owner. `with_bound_buffers` gives the ring and its storage access an invariant,
fresh lifetime brand. The ring's real preparation/recycle callbacks quantify
a second, shorter grant lifetime, preventing both foreign-ring grants and
retention across publication. Storage operations select the bound allocation
from the grant; raw shared byte methods are private to `rx_storage::buffers`.
R's grant carries index `COUNT` and a const target parameter, so selection adds
no per-frame branch. Grants and scoped receivers add no persistent state.
The retained single-descriptor read keeps its already validated buffer pointer;
other reads use the descriptor binding. Read grants cannot authorize rearm.
One unsafe read-binding constructor retains the completion owner's loan.
Detachment consumes the completed owner inside the byte core.
The completed byte-cell view, detached-buffer handoff and two sharing
implementations form the memory boundary. Completion observations precede
grants. Preparation and recycle normalize affine return states and publish
descriptors without reading or painting buffer bytes. Backing storage covers
the nonzero advertised capacity; constructor, pre-publication byte views and
DMA placement enforce this geometry at monomorphization, including storage
initialized as zero bytes without calling `new`.
Accepted RX units contain every descriptor from the software cursor through
its first terminal `RX_DONE`, bounded by the frozen LAST. Lengths and metadata
come from that descriptor interval alone; buffer contents never select a
member, a length or a metadata source. Exhaustion uses the same membership
rule. Admission preview refuses chains and reserved-descriptor tails before
reading bytes. R remains in the accepted-unit cursor until its terminal has
been observed, unless the epoch resumes at a retained ring NEXT; its leading
word cannot establish a continuation.
After R's link is cleared, `RX_DONE` or LAST naming R keeps the existing
Corrupt/reset quarantine path; byte contents are never a late-write detector.
The general hardware premise is that LAST releases exact accepted prefixes
with fresh head metadata and excludes later writes into released storage.
SOURCE[HW18_RX_COMMITTED_PREFIX_20261027] in
`chips/esp32s31/registers/evidence/hw-rx-radio.toml`, run
`hw18-20261027-rx-radio-03-s31b`: the cold prefix R,0,1 and the next two exact
prefixes support cursor membership in that capture. The nine committed
buffers remain stable until stop. Foreign nodes released by LAST are excluded
by the exact-prefix assumption, not by a software content test. Arbitrary
in-flight detach and recycle remain unmeasured.
Compiler fixtures build internal callers against the actual HAL source;
weakened-bound controls confirm every callback escape case detects a regression.

The Wi-Fi DMA storage also exposes the host model of the DMA engine's
memory port under `test-support` and in unit tests: `RxDmaStorage<…, HostDma>::{dma_write, dma_read}` in
`ieee80211/dma/rx_storage/host_dma.rs`, a child of the byte-storage module.
Its two unsafe copy operations reach one buffer's `UnsafeCell` at a published
`HostDma` address (a ring buffer or the reserved descriptor's buffer),
refusing any range outside a single buffer and any buffer detached to an
upper owner. Its caller lends the arena's live ring shared:
completed-descriptor views borrow that ring mutably, and the ring is not
`Sync`, so no CPU view of the bytes is live meanwhile.

The HAL's owned PHY transaction adds no unsafe code. A permanently claimed
`TransactionSeat` supplies one affine domain identity. `RadioTransaction`
keeps its PHY and inert return owner paired; after a consuming split,
`DetachedRadioDomain::reunite` checks the exact seat identity and returns
foreign inputs unchanged. Registration-epoch equality grants no reunion.
`PhyHardware` owns its registers and restore state; shared accessors lend
short borrows of that owner, without extending a lifetime. The transaction's
grant-protect words remain with the movable PHY half. Neither half exposes
register ownership from the other. Channel and retune futures own
`ChannelTransaction`, containing the PHY custody and exclusive MAC/platform
loans, and return the domain with their result.
Both payloads stay in `ManuallyDrop` until consuming reunion. Both guards
record `Cancelled(Operation)` with the registration observation, independently
of field drop order. The operation names register, maintain, wake, close,
channel or retune. `FaultHandle::require_owner` reports only a missing owner;
a diagnostic handle cannot select another fault reason.

The radio actor keeps lifecycle results in its parent future's storage;
only exclusive `Placed` loans cross the result-poll boundary. Completion
consumes the result loan before returning the same domain to its backend
slot. Cancellation retains the existing transaction guards and performs no
hardware restoration. This uses the memory crate's existing placement core.

The SAR/PWDET pilot transfers only the movable PHY half: its calibration
restore/epoch and grant state travel together, while the async maintenance
future retains the exact `DetachedRadioDomain` and its cancellation guard.
No reference into the hardware crosses submission. One-slot affine SPSC
queues and the handler's existing `HandlerCell` provide storage; their `Send`
bounds check the complete bundle. The handler acknowledges T0 RAW with CLR
before stepping/rearming; startup clears stale RAW while masked. The actor
arms before deposit on the handler's same hart/level. The same-level signal
publishes facts and pends the next executor round, never a task waker.
`SarTransferFault` retains the original failed queue, alarm or reunion
observation. `SarSetupFault` retains the failed storage or router binding;
startup owners remain held on its non-returning reset path.
`SarAlarmWithoutBundle` retains the acknowledged RAW/ST and request/reply
lengths when a pending event has no custody. IRQ `service` returns this fault
through `Result`; the image immediately calls the existing retained reset
path through a direct call. The handler, IRQ part and actor's owners remain
held without further service or owner recovery. Cancelling the waiting lifecycle
future leaves an in-flight alarm quarantined until reset; completion has
stopped/masked it before reply.
The narrow transaction reporter can report only SAR transfer faults, without
reunion or register authority. Owners stay quarantined before record/reset;
the existing retained codec remains 16 bytes and records the named reason and
epoch. Pointers from a foreign reunion remain session diagnostics, not retained
owner identities.
The HAL declares the direct `link_fn!` binding and exports
`phy_fault_reporter!` to define that exact signature. Staged images bind it
to `fatal::reset_recording`. Their four-word (16-byte), versioned/checksummed
record retains reason/operation and epoch and is cleared only after successful
next-boot output. The address of the image's owner is not stored.
Isolated comparison probes supply their own reporter through the same HAL
macro, preserve their first named fault in session RAM and terminate through
their own probe halt/panic path. They use verifier linker scripts and
no staged runtime; the radio probe's staged hardware companion uses the
normal runtime record/reset path. Host tests use separate seats and catch
their own reporter's unwind; both payloads remain quarantined there too.
No new unsafe block or sharing implementation is introduced.

`oer-memory` holds the shared proofs for executor-crossing storage.
`ContextCell<T, P: ContextProof>` holds executor-owned signal state, signal
list links and timer slots. The owner's private unsafe `ContextProof`
implementation confines each cell's accesses to its one non-concurrent
context, including across preemption and cores, and respects the payload's
thread restrictions during initialization and destruction. Neither the proof
nor the cell escapes the owner's checked access paths. The cell's sole
`Sync` implementation and mutable accessor live in `oer-memory`.
Its non-atomic borrow flag refuses a nested `with` on the same cell with
`None`, without invoking the closure or changing the payload; borrowing
another cell is permitted. A guard ends the borrow on return or unwind.
Signal reads and registration report refusal through `ForeignContext`;
service leaves unvisited facts pending. Timer registration wakes the task
to register after the enclosing access ends; timer service leaves its alarm
state untouched when refused. No interrupt masking, waiting or lock is used.

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
exactly one accessor to each payload place. In `ByValue` mode a Release store
hands the place over and the other side's Acquire CAS takes it; the client
posts only with the slot's one `Poster`, which the answer returns. The
radio-system client uses this mode and has no unsafe code of its own.
`InPlace` mode holds the whole exchange in its request place and stores no wakers. Its
unique server endpoint lends `&mut Req` only in `WORKING` and publishes
`ANSWERED` after that borrow ends. Its unique client observes `ANSWERED`
with Acquire and validates the reply by shared borrow before consuming the
exchange and publishing `IDLE`; a refused validation leaves the payload
untouched. A dropped endpoint retains the
seat and payload, including after the server answers. Reclaiming the seat
consumes both matching idle endpoints. State tests cover phase ownership,
borrow address stability, refusal, abandonment, reunion and repeated requests,
including client/server races on separate host threads.

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
reads a value behind a shared borrow with a volatile load; `write_volatile`
stores a `Copy` value through an exclusive borrow. `StaticRef<T>`
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
other unsafe code. Its memory regions keep their geometry private: const
admission rejects address overflow and out-of-bounds subregions. Generated
maps and chip linker bindings check fixed geometry in const initializers;
the bootstrap handles refusal of a dynamic runtime PSRAM window as an
invalid header.

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
  exception handler symbols (`panic_hook!`, `exception_handler!`), trap
  snapshots and vector-table reads (`trap`), and the linked sections (`layout`).
  Images and HIL agents reach these through safe APIs and forbid unsafe code.
  The critical section masks its executing hart only. Firmware packages depending
  on the staged runtime declare `critical-section-hart`. The staged build/run
  pipeline checks linked section entry symbols and inline copies against the
  image's declared per-hart task roots, interrupt-table handlers and exception
  roots, using existing call resolutions. Foreign roots must have complete
  resolved paths excluding those entries; the declared hart may retain partial
  bounds. The public HAL cannot start or release a secondary hart. `cargo fw probe`
  emits reachability reports only, including for binaries linking the staged
  runtime; it does not run this critical-section gate. No cross-hart lock is involved.
  The image linker places strict code in SRAM from interrupt, exception and
  panic analysis.
  `entry!` mints `StageTwoHandoff` directly and passes it to the safe
  `Handoff::from_proof` wrapper. One extern block in `layout` declares the
  linker and vector address markers. Generated per-hart declarations expand
  inside this same block; indexed address expressions add no dereference or
  additional unsafe site. Empty task and IRQ ranges remain explicit zero-byte
  allocations. The panic handler raises its hart's CLIC
  threshold to the maximum, clears MIE and executes a compiler fence before
  claiming or writing its `retained_static!` slot. Its claim stays held
  through the image's hook until reset. Nested panic
  entry skips metadata, the record and the hook, then resets, preserving
  the first record.
- Each board implements `AdoptsBootstrapMapping` for its PSRAM.
- The bootstrap places the packed stage two and the flash tuning reference in
  its own read-only flash sections.
- The HAL's `PsramRegisters::into_region` runs cold bring-up and consumes
  the reset root's unique PSRAM controller/MMU partition before creating
  one `&'static mut [u8]`. Only that method creates the slice; a caller-supplied
  `PsramReady` cannot authorize it. `PsramReady` exposes its completed mapping
  through a read-only accessor. The safe `stage` uses slice offsets, checked
  ranges and volatile borrowed probe/CRC accesses, returning typed errors.
  Its `Staged` result retains the exclusive payload loan through publication
  and handoff. The bootstrap checks the physical PSRAM base before the probe
  and its flash source immediately after the probe, before header/CRC reads.
  Payload copying iterates over the checked equal-length source and destination.
  The tuning scratch uses a fallible exclusive claim. XIP reads return the
  rejected index and reference length rather than panicking; the sweep and
  verification propagate the typed boot failure through their existing
  suspended-fetch rollback. The terminal boot report emits bytes directly
  and keeps all boot owners on the diverging caller's stack.
- A probe image is entered by the Blobray verifier through the exported
  symbols the vendor code calls (`_start`, `ets_delay_us`, the probe
  entries), with that code's C signatures, and keeps its fixtures in statics
  the verifier's serial calls reach one at a time.
- The PAC-only facts image is entered at `_start` by the ROM RAM loader.
  Its entry establishes the SRAM stack and clears BSS, owns its trap ABI
  in `src/trap.rs`/`src/trap.s`, and preserves an atomic LP `.noinit` reason
  record across system reset. Controlled interrupt-admission entry stubs delimit the
  threshold-write experiment; the installed trap ABI preserves their registers.
  Only the masked thread publishes the LP record before the reset fence.
  In `firmware/probes/esp32s31-facts/src/image.rs`, `SAR_CONTEXT` publishes
  the masked thread's exclusive `Sar` borrow to the isolated P2 handler.
  Its atomic swap to null admits exactly one handler dereference; the
  thread accesses no SAR owner until the handler returns and MIE is masked.
  The stack allocation remains live throughout the dispatch. `SAR_SELECTION`
  selects the same static `PROBES` registry used by the thread. The handler
  publishes `SAR_DONE` after the borrow ends; the thread clears the pointer
  before reusing the session. This is the probe's one SAR raw-pointer site,
  with no production runtime dependency or independently claimed registers.

The bootstrap and probe crates deny `clippy::undocumented_unsafe_blocks`, and
each unsafe attribute in them carries its `// SAFETY:` comment as well.

## PAC dependency authority

Direct dependencies on the PAC are restricted independently of unsafe
syntax. The allowed consumers are the chip's `hal` under `esp32s31/` and the
explicitly admitted PAC-only facts image `firmware/probes/esp32s31-facts`.
Other `firmware/probes/*-facts` packages require a separate layer-test admission.
The facts image's trap/vector ABI lives inside its own package: `global_asm!`
uses offsets from its own Rust frame and a generated CPU-line table. All
nonzero vectors enter the same register-preserving assembly entry and handler.
It shares no private runtime source; its only trap-register installer is the
public architectural API in `oer-riscv`.
The facts image takes the PAC once, uses typed accessors, and has no HAL or
production runtime dependency; production never depends on it. All other
drivers and adapters reach registers through HAL owners. Check the executable
list when changing this boundary.

Upper layers use opaque, finite capabilities rather than raw pointers,
unchecked lifetimes, generic PAC callbacks or independently reusable interrupt
owners. MMIO outside the restricted PAC must use typed accessors. Missing
register fields are reviewed in `chips/<chip>/registers/` and regenerated
into the PAC, never reconstructed with local masks in a HAL or adapter.

Bluetooth admission checks primary and task enables before HAL/PHY
initialization can mask inherited lanes. `ForeignEnable` retains primary
enables, masked status, NRT words and task enables. The opaque task +0x4bc
word is diagnostic-only; production never reads it. The typed production set
covers baseline fault/overflow, dynamic scheduler and task14 lifecycle lanes.
Terminal masking covers sourced primary feature lanes and all task enables,
including unnamed task17. The runtime affine NRT transaction runs before the
last visible-fault check preceding masking and before final release confirmation.
It acknowledges admitted sampled lanes, including those outside the fixed cleanup
subset; a pair containing shared primary fault/overflow lanes refuses both writes
and returns `ReleaseNotQuiet` with its exact snapshots and primary/task observations.
Fixed non-fault W1C images are written under the mask after task RUN-disable,
followed by CONTROL1/2 and zero-enable verification. No enable is reopened and
no matrix route changes. Bank0[15] and bank1[8/9/12] are absent from the fixed
images, so a late masked fault is never blindly erased. Task acknowledgement
uses the sourced WT all-lane strobe. Masked pending is not observable; release
establishes fixed acknowledgement writes and zero enables, not zero pending
or producer shutdown. `ReleaseNotQuiet` retains nonzero enables or unexpected
visible observations. SOURCE: `POLICY_S31_BLE_TERMINAL_NONFAULT_ACK_20261027`.
Bank1 CC0/CC1/overflow have hardware coverage from
`hw32-20261027-ble-masked-01` (#757); bank0/NRT and other feature lanes are
vendor-covered and not hardware-verified. Both named faults quarantine the task/IRQ owners and reach safe
`fatal::reset_recording`. The same 44-byte BLE gate slot has a reason-tagged
eight-word payload: seven policy source words and one reserved zero word.
Next-boot output reports the observed banks. Bank1[0,2] and task17 remain
opaque with their existing SOURCEs. CTE +0x4c4/+0x4c8/+0x4cc is outside this
policy; production has no enabled CTE interrupt consumer. Sequential readback
is not a producer-drain proof.

Bluetooth controller-time overflow stays with the primary IRQ bank owner.
Preparation arms bank-one `CONTROLLER_TIME_OVERFLOW` in baseline `0x1302`;
P3 acknowledges its saved snapshot and publishes a checked wrap count as a
value to P2. This counter is independent of coalesced scheduler work and grants
no MMIO authority. P2 requires an unchanged count throughout each latch, with
a post-latch handler service barrier before the second read, so hardware-pending
overflow cannot escape the comparison. Only then does it join the count and raw
word into continuous time. Terminal release masks
and clears overflow before the bank leaves the handler; CONTROL1/2 alone do
not silence enabled pending (SOURCE: `HW12_S31_BLE_OVERFLOW_ACK_20261026` and
`HW9_S31_MAC_RELEASE_20261027`). Exhaustion retains its original count or raw
word in a named fault. B0 uses the same 44-byte retained slot for these reasons,
without releasing owners, through `fatal::reset_recording`; the next boot
prints the diagnostic. No additional unsafe sharing is involved.

NRT acknowledgement checks the exact sampled pair against shared primary
fault and overflow clear lanes before either clear. A pair containing one returns a
named fault preserving both words. Runtime capture reports `BluetoothNrtInterruptFault`;
terminal release reports `ReleaseNotQuiet` with that pair in its policy observation.
The gate quarantines its owners and B0 records the fault observations in the same
44-byte slot before `fatal::reset_recording`. NRT cannot consume a primary
overflow observation or blindly clear a primary fault. Arrivals absent from
the affine sample are not included in its acknowledgement.

The Bluetooth modem LP timer retains its register authority in the permanently
bound handler throughout software queue work. P2 receives only a positional
time/epoch snapshot and returns an unarmed plan through the gate's single-slot
queues. The snapshot and plan grant no MMIO access. Rollover acknowledgement,
compare programming and software-pending commands execute only with the
handler's retained owner. Repeated interrupts during planning acknowledge and
retain new software work there; no additional unsafe sharing or second register
writer is involved. Preparation samples both inherited pending bytes before
its whole-word all-ones `+0x34` command clears the two tested causes, then
arms the rollover lane with the counter stopped (SOURCE:
HW9_S31_LP_CLEAR34_20261027, hw9-20261027-lp-06). The returned observation
retains the removed stale state. Shutdown
disables both measured delivery lanes, stops the counter and fences, retains
both sampled pending bytes in its returned type, then clears both whole words
and fences again. Controller reset and clock gating supply no LP-quiescence
proof (SOURCE: HW6_S31_LP_QUIET_20261026 and
HW6_S31_LP_RESET_RETENTION_20261026, hw6-20261026-reset-02). A stopped typestate
acknowledges rollover without dispatch or rearming.
Outstanding dispatch refuses stop, while rollover-only planning can retire
its ending epoch projection. The handler steps the timer before taking a
stop request, so a dispatch in that entry is retained and refuses stop.
Pending observations cross retirement, Controller reset and reunion into the
returned result. B0 sends both bytes through its H4 shutdown event; the physical
B0 procedure logs them and includes them in its run report.
The handler permanently retains a copyable, read-only `BtdmLpTimerStatus`
capability for the side-effect-free LP cause word, independent of the timer's
runtime-control owner. A vacant timer with nonzero LP status is an invariant
failure. `GateFault::Invariant` retains the full entry LP cause, both NRT cause words
when the bank is present (explicitly unavailable otherwise), the timer phase,
and the stopped-timer fault's pending bytes and raw unexpected status.
The first fault is published once; subsequent service returns it without
hardware access, queue service or owner release. B0 calls safe
`fatal::reset_recording` directly from the handler without unwinding or dropping
any gate or interrupted task owner. Its 44-byte versioned, checksummed retained
record keeps the exact reason and observation. The next boot consumes and prints
it through the early console. No watchdog expiry drives this transition.
The port lifecycle's fault flag is separate and carries no gate fault.

The Wi-Fi source120 fatal-assertion path keeps the full sampled RAW as
an opaque diagnostic. It quarantines both interrupt banks before returning a
named `irq::Fault` to the image. The image retains the preempted DMA owners and
the handler's register owners through the runtime's never-returning shared
`wifi_fault::reset`, which calls `fatal::reset_recording`,
writes a twelve-byte record through the existing safe retained-cell API, and
reports it at the next boot. The `wifi-fault` feature selects the one record
for staged Wi-Fi images; no image defines a second copy. Every epoch clear
checks RAW under the delivery mask before clearing either bank and acknowledges
only that nonfatal MAC sample, preserving a later fatal arrival in RAW. Magic and
checksum validate ordinary stored values, without a power-fail atomicity
promise. No owner is released, reconstructed or taken over. The quarantined
epoch refuses activation.
BB watchdog reset-enable remains an independent hardware action: initialization
sets it as the ROM does, preserves interrupt-enable, and its reset extent is
unmeasured. It supplies no software retained-fault contract.

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
