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
- `[workspace.lints.clippy]` denies `disallowed_methods`; the root
  `clippy.toml` lists them for every product crate, including direct
  `core::mem::{zeroed, transmute}` and their `std` counterparts. Its
  entries with a reason are the common bans; its plain entries are the
  catalogue of methods that panic. A crate that cannot yet pass it keeps a
  local `clippy.toml`: the root's without the entries it still calls,
  naming in its first line the step that removes it; the tools read
  `tools/clippy.toml`. Every `clippy.toml` in the tree retains all common
  method bans; `every_clippy_config_retains_all_common_method_bans` checks
  this. A field's value is cut only inside the PAC: its `Bits` is the
  generated root's, its cut `pub(crate)`, so no ban is needed for it (the
  PAC root's compile-fail examples show the refusal). The four lints
  `cast_possible_truncation`, `cast_sign_loss`, `cast_possible_wrap` and
  `let_underscore_must_use` are denied in the workspace. A crate root with
  existing sites expects each lint once, naming the step that cleans it
  (in a host tool, that the rule covers the code an image reaches; a
  crate whose sites are only in its tests, under `cfg_attr(test, …)`).
  Every attribute that silences a lint in built code outside tests is
  listed in `SILENCED` (S8d) by file and lint, with its reason; the list
  only shrinks.
  The root also sets `suppress-restriction-lint-in-const = false`: a
  runtime-callable `const fn` is not a compile-time-only check.
- The panic lints (`unwrap_used`, `expect_used`, `panic`, `unreachable`,
  `indexing_slicing`, `todo`, `unimplemented`, `arithmetic_side_effects`,
  `disallowed_macros`) are denied in the workspace table for every package,
  and no crate states them again. A package with sites left expects the
  lint at its crate root, naming the step that removes them (a host tool:
  that the ban covers the code an image reaches; code built only for the
  chip, under `cfg_attr(target_arch = "riscv32", …)`); `SILENCED` counts
  each. Tests may panic: Clippy's in-tests options cover `unwrap`,
  `expect`, `panic!` and indexing in a test, and a crate root with unit
  tests carries the one line
  `cfg_attr(test, allow(clippy::disallowed_methods, clippy::disallowed_macros, clippy::unreachable, clippy::arithmetic_side_effects))`;
  an integration test file whose helpers need it, and a test-support
  module, allow at their top. `disallowed_methods` is otherwise allowed
  only at a banned method's own home, with its reason, and never
  expected; `SILENCED` counts every such attribute.
  The root `clippy.toml` is the catalogue: assertion macros, panicking core
  methods and `core::task::Waker::noop`.
  A sans-IO package (testing packages aside) has no local `clippy.toml`
  (`a_local_clippy_configuration_is_the_exception_and_names_its_step`); the
  layer tests do not replace Clippy or prove transitive absence of panics.
  A build-time check takes the form `if !cond { panic!("…") }` in a const
  item or block, never an assertion macro.

Sans-IO code contains no `unsafe` and no runtime panic paths, including
`panic!`, `unreachable!`, `todo!`, `unimplemented!`, assertions and debug
assertions, `unwrap`/`expect`, indexing or slicing that can
panic, arithmetic overflow or invalid division, and panicking core methods.
Those paths return typed errors. HIL completion receiver exhaustion is a
terminal-barrier refusal: the image retains the receiver-limit
observations in the existing named-fault slot, keeps the operation's owners
quarantined and resets through `fatal::reset_recording`, without executing the
guarded action. Sans-IO crates do not depend on fatal, reset or retained-record
owners and do not reset: reset belongs to the image or HAL owner. Their
explicitly selected finite reset deadlines are enforced by the platform
watchdog owner, outside sans-IO; the portable deadline policy only takes time
observations and returns typed errors.
Compile-time checks use a const item or inline const
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

`oer-portable-waker` holds one task's waker in an `UnsafeCell`. Its state word
gives the cell to one context at a time; a `SAFETY:` comment at each access names
the state that holds it.

Memory operations move into `oer-portable-memory`, architectural instructions into
`oer-portable-riscv`, and PSRAM slice creation into its HAL owner. Their unsafe counts
may grow only by the relocated operations while the branch total falls and
no calling crate's count grows. Runtime panic counts never increase. Image
panic budgets report counts and sites without gating builds; zero reachable
panics remains the code goal.
Compiler `panic_const_async_fn_resumed` sites have a separate row outside
the budgets and target of 0, based on the executor's runtime `DONE` check
(not a type guarantee) and `.await` never polling a completed child again.

The stack analyzer's DWARF Waker-slot proof also requires the image
pipeline's [code-generation preconditions](../tools/image/stack/README.md#code-generation-preconditions):
MergeFunctions and machine outlining are disabled, merged source locations
are not picked from a single origin, and linker ICF is off. The build
validates the effective compiler flags and expanded linker arguments;
missing scope identity is not repaired by a trusted frame size.

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

Only `oer-portable-memory` defines `unsafe impl Sync` or `Send`; the layer test
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

Below it are, by directory (`/` ends a domain), for the S31 workspace:

| Kind | Directories |
| --- | --- |
| Volatile register words and their claim | `portable/mmio` |
| Semantic PACs and the HAL | `chips/esp32s31/pac`, `chips/esp32s31/hal` |
| Platform, interrupt bindings, executor and architectural instructions | `chips/esp32s31/platform/board`, `chips/family/platform/bootstrap`, `chips/family/platform/runtime`, `chips/family/platform/interrupt-table`, `chips/family/platform/executor`, `portable/riscv` |
| Cores of memory the hardware shares | `portable/memory`, `chips/family/platform/layout` |
| The hot-code attribute | `portable/hot` |
| The host's operating-system bindings | `tools/os/process`, `tools/os/serial` |
| Bootstrap and probe images | `chips/esp32s31/platform/bootstrap/`, `hil/probes/` |

`oer-chips-family-platform-interrupt-table` owns the Espressif interrupt matrix contract,
vector ABI and platform-runtime symbols. It holds no CLIC line encoding: a
chip's controller binds, disables and checks a line. Its narrow unsafe
contracts cover the existing vector/table/exemption slot names and ABI
types; signal execution proofs belong to `oer-portable-memory`. Every safe `Entry` implementation carries
`EntryProof<Self>`; root Clippy policy restricts both proof and token
constructors to the table macro. This policy requires an explicit lint waiver
to bypass; it cannot validate scheduling metadata supplied under such a waiver.
An entry's membership of its router's table, its class and its vector/cell
identity are the table's by type; its source, line, level and hart are
checked at compile time. Claimed storage retains owners in quarantine. Memory cells enforce
exclusive access independently of entry metadata. The memory core owns the
symbol-export attributes. Chip crates and firmware consume the bindings directly.
An entry's vector reaches its part and the witness of the call (`Here<'_, E>`,
neither `Send` nor `Sync`, bounded by the call) through one hidden entry
point, `__private::vector`, which root and every local Clippy policy forbid
outside the table macro's vector (`disallowed-methods`), as they forbid a
direct `EntryProof::new`; an entry's handler cell is the crate's own. The
crate's one call of the hart's promise, `OnHart::assume` in
`Here::on_hart`, gives the proof of the entry's hart, checked at build to be
the asked hart. Its `// SAFETY:` names three facts: only the entry's vector
calls `vector` (the lint policy), that vector is exported as
`__oer_interrupt_h<HART>_l<LINE>`, which only that hart's vector table names
(generated from the facts), and the router binds the entry only on its hart.
The crate has 7 unsafe sites, the promise's one call among them.

`oer-portable-hot` is a proc-macro crate whose one attribute, `#[hot(N)]`,
writes `#[unsafe(link_section = ".hot.<N>.text.<module path>::<name>")]`
and `#[inline(never)]` on a bare-metal target, and nothing on the host. Its
contract: it takes functions only, and writes section names of the one
family `.hot.<N>.text.*`, which the platform's runtime script maps to
executable memory whether the image takes the level (SRAM) or not (`.text`).
A wrong level costs SRAM, never soundness. The attribute is the expansion's,
so a crate that uses it keeps `#![forbid(unsafe_code)]`; the crate itself
forbids unsafe code and has no unsafe site of its own.

## Generated access and trusted handwritten code

Typed register blocks are generated without `unsafe`: `cargo registers
generate --chip <chip>` renders every MMIO block type of the chip's register
model, with the owners of its policy's parts, into the chip PAC's
`generated.rs`. The unsafe code under a typed block is `oer-portable-mmio`
(`portable/mmio`) alone, and the PAC root's one claim. A policy
`record-words` entry gives a register's read value `R` the method
`record_word`, its complete `u32` for a diagnostic record, including
undeclared and reserved bits, without decoding or another access; only a
side-effect-free word whose every declared field has a defined
observational class is admitted. Splitting fields grants no additional register
authority or unsafe access. `oer-portable-mmio` holds
only volatile register words and their access authority (DMA addresses and
volatile DMA-memory cells live in `oer-portable-memory`):

- `oer_portable_mmio::assume_owned` promises exclusive ownership of the declared
  words, rather than every word of an overlapping address range. The chip's
  PAC takes those words once; the HAL root splits them into domain owners.
  A hart-local instance (the S31 CLIC: each hart reaches its own at one
  address) is claimed once per hart; the generated disjoint checks run per
  hart context, and the HAL's per-hart `InterruptGrant` lets element `h` be
  used only on hart `h` (a runtime hart check when it becomes a controller,
  which is `!Send`).
  Only the root's owned partitions authorize register access.
  A probe may reclaim a block only after dropping its previous owner, with
  one exception: the timer facts probe (`hil/probes/esp32s31-timer-facts`,
  `src/comparators.rs`) makes one second claim of the system timer's
  `Systimer` part and counter latch beside the executor's time driver, which
  holds the root's. It does not keep the letter of this contract; its
  `// SAFETY:` holds the table of which context writes which word, so that
  no word has two writers in two contexts, and the claim's blocks do not
  leave that module.
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
- `oer_portable_mmio::Reader`, the `Copy` read-only authority a block or part
  becomes when it gives up its writes: no owner writes the words it took
  over, and every copy only reads.
- The marker-derived `Sync` of a shared block at a fixed address
  (`oer_portable_mmio::SharedRegisters`, a policy's `[[shared-blocks]]`) and its
  `write_shared`: a device word is not Rust memory, and the block's writes
  are whole-word stores that read nothing back, so every execution context
  reaches its one owner through `&self`. `Registers::Sharing` is
  `sharing::Shared` for those blocks and `sharing::Owned` otherwise;
  owned blocks and parts remain `!Sync`, and host placements also stay
  `!Send`. No unsafe sharing implementation is needed.

`oer-portable-memory::VolatileCell` is the volatile word of generated DMA-memory
layouts; both the cell and the layouts derive `Zeroable`. `DmaWindow`,
`DmaLayout`, `DmaWindowAddress` and `DmaRef` live beside `DmaBytes` in
`oer-portable-memory`. Their two unsafe constructors preserve the placed-storage
and retention contracts.

`oer-portable-memory::arena` is the custody of DMA buffers. It has six kinds of
`unsafe`:

- the slice over a held buffer's CPU bytes (shared and unique, one site each),
  reached only through the buffer's one `Handle`;
- the copy of a held buffer's floor (`Arena::floor`, `Floor::copy`), the bytes
  the hardware shares below its slices, read out by volatile reads into an
  array with no reference to them (one site); a lent buffer has no `Handle`,
  so the floor of memory the hardware owns cannot be asked for. The HAL reads
  the header's fields from the copy through the PAC's generated layouts;
- `Sync` of the place;
- `Released::assume`, the release proof's one constructor. Each kind of release
  is a HAL atom that calls it, citing its stand record;
- `Quiet::assume`, the borrowed release's one constructor (`unsafe fn`): the
  claim that the hardware does not touch one lent buffer while the borrow
  lives, read through the safe `Arena::inspect`, which lends `f` the buffer's
  `Handle` by shared reference for the call (bytes and floor read; nothing
  written or moved; the `Lent` stays). The HAL atom that makes one is the
  receive ring's chain copy, citing `HW_RX_CHAIN_LAPS_20261117`;
- `unsafe trait Brand`. Its one `unsafe impl` is the one that `arena!` writes:
  the brand names its own place; that expansion is counted as
  `portable/memory`'s and builds in a crate that forbids `unsafe`, where a
  hand-written `unsafe impl Brand` does not (`tests/ui/arena_under_forbid.rs`);
The HAL's receive ring (`radio::ieee80211::dma::receive`) makes three
release proofs:

- `Released::assume` when a completed single unit is exchanged for a spare
  (`HW_RX_REBIND_BEFORE_LAST_20261118`, `HW_RX_REBIND_AT_LAST_20261118`);
- `Quiet::assume` while a completed chain is copied in place, before its
  buffers are re-armed (`HW_RX_CHAIN_LAPS_20261117`). This one and the first
  lie at or before the `LAST` the walker reported at the service's start;
- `Released::assume` for every buffer at the ring's release, which takes the
  proof of the MAC block's reset after a stop (`MacStopReset`). Only the radio
  domain's reset after a stop makes it, from the MAC's stop request, which
  takes the walker's edge, which takes the walker's one start of a
  publication (`HW_RST_WIFIMAC_AFTER_STOP_PORT_ORDER_20261119`,
  `HW_RST_WIFIMAC_AFTER_STOP_REQUEST_20261119`,
  `HW_RST_WIFIMAC_RUNNING_RX_20261119`).

`oer-portable-memory::Occupied` is a place that holds a value, the handle a
filling of `&mut MaybeUninit<T>` gives: three sites, its two dereferences
and the read of `vacate`, which gives the value and the empty place back.
The handle's unique borrow of its place is the proof that the place holds
the value; it has no `Drop`, so a handle let go leaves its value in place.
The receive ring is published into one and released out of it.

- `unsafe trait PlacedStatic`. The HAL's placed static (`DmaStatic`, made
  only by `dma_static!`) implements it in its own home, the placement
  constructor (`system/memory/dma_static.rs`): the claim takes the static's
  own value once and makes its address through `dma_address::placed_link`,
  two sites; a second claim is refused. Its proof of the address is the
  section `dma_static!` names and the runtime linker script's assertions
  over the DMA window.

An arena is made only by `Arena::claim`, from its brand's place; a second
static of the place's type is inert, and the arena makes no address from a
place.

`oer-portable-riscv` (`portable/riscv`) is the only crate with `asm!` for architectural
operations: CSR access, hart and cycle reads, interrupt masking, CLIC threshold,
debug triggers, waits and barriers, and `illegal_instruction`, the
illegal-instruction trap a HIL test image raises on request to check its
exception path. Its safe functions have no memory precondition. It owns the
proof that code runs on a hart, `OnHart<H>` (zero-sized, neither `Send` nor
`Sync`): `OnHart::check` makes it safely where a proof is taken once, as a
hart starts (the platform runtime's `Hart<H>::on_hart`, with no `unsafe` of
its own), by reading `mhartid`; `OnHart::assume` is the `unsafe` promise of
code that runs on its hart by construction, which only the interrupt
table's entry witness calls; a host test makes one with the test-only
`host-hart` feature (`OnHart::host`), never on the target. The proof
guards stores that would reach another hart's copy of a hart-local block
(the CLIC's pending bytes); a read needs none. The crate has 34 unsafe
sites, the promise's declaration among them. Installing trap vectors and `mscratch` is unsafe because the
entries, table and exclusive interrupt stack must implement a live trap ABI.
Entering stage two is unsafe: `jump_to_entry` requires the checked entry of a
published image that installs its own stack. Its inlined assembly disables
interrupts, executes both instruction-publication barriers and jumps to the
constant entry in one block.
Every instruction site has one narrow `// SAFETY:` proof. Moving instructions
into this crate may increase its unsafe count only while the branch total
decreases and no caller's count increases; runtime panic counts never increase.
Device ordering is `oer_portable_riscv::fence` (`fence iorw, iorw`), `fence_w_o`
(memory writes before device outputs: a producer's facts before its
software-interrupt raise) and `fence_o_rw` (device outputs before memory
accesses: a handler's lowering before it takes the facts), each with the
host compiler fence; `oer-portable-mmio` contains no architectural instruction or
barrier API.

The instruction module and its exports exist only on RISC-V. Staged firmware
binaries gate their entry composition and hardware dependencies on the same target;
their build scripts emit image linker arguments only for that target.
Host tests compile pure modules with explicit fixture inputs rather than
providing synthetic CSR reads. Their host entry is empty.

Interrupt stack install takes a local `&mut PreAdmission<H>` and is safe.
The boot handoff wraps its unique entry proof with that token, retains it
through adoption, and consumes it when enabling interrupts, returning the
router and its existing `Hart<H>` together; a secondary hart's
`SecondaryHandoff` does the same with its own token. `Adopted::parts` lends the hart
and mutable router together before enable. Executor startup const-checks the
entry's hart against this token; task wakers stay local to that hart, and a
wake on another hart records and resets. `Hart<H>` and
`PreAdmission<H>` are neither Send nor Sync and have no public constructor.
Installation checks the executing hart and masks local interrupts before
painting the inactive allocation and installing its trap registers. Its
memory operations remain in the private `stacks::watermark` module; the
trap-register writes remain in `oer-portable-riscv::install_trap_stack`, each with
its narrow safety contract.

The HAL publishes every hart's side-effect-free pending bank at root split.
Its bank owners and exhaustive selection are generated from the same hart
geometry as the vectors. Pending and debug reads take the PAC's checked `Hart`;
they do not claim registers or grant execution authority. The executor validates
the architectural ID before selecting its running-context slot or these reads.
An invalid ID follows the retained `InvalidHart` reset path, releasing
no owner. The signal context checks name no binding and so no fatal path: they
treat an invalid ID as no executor's context and refuse with `ForeignContext`;
every executor round enters its context through the reset path first. These
selections use no unsafe code.

`Doorbell::new::<Member>` returns a typed `UrgentSource<Member>` beside its
ringing owner only after its const member/line/binding checks. The observation
contains no register owner or waker; copying it permits both local executors
to observe the same urgent source. A sealed `UrgentSources` contract accepts
that observation or `()`; executor startup cannot take a raw numeric list.

Executor and time-driver setup return route or counter-latch refusals through
`Result`. Each `Failure<P, T>` holds its inputs in `ManuallyDrop`; dropping a
refusal cannot release those owners or authorize retry. Internal invariant
failures carry a named `Fault` with the original observations. The terminal
path masks execution through `fatal::reset_recording` and keeps all static,
handler and interrupted task owners until reset. A numeric reason code and four
exact 32-bit observation words use the common versioned fault envelope's
runtime domain, with their registry class and integrity check. Capture
reads no reason text, including from an interrupt. Adoption decodes the code
in thread context, prints it through the early console and clears it only after
successful output. The cell's claim lives in boot-zeroed SRAM and stays held
through a fatal reset, preserving the current boot's first record. A fresh
boot's first fatal claim replaces stale retained bytes even without previous
record consumption or successful console output.
Software lines use fallible static placement and one-time value publication,
without a panicking initialization path or additional unsafe code.

`Router::bind` refuses nothing of the table's and makes controller writes
that cannot fail. Storage and handler installation failures return a named
`BindFault`; the token and the uninstalled part stay in `Quarantined`, whose
private `ManuallyDrop` fields have no recovery API. Placed parts retain their
claimed static allocation. No failed binding issues `Bound` or enables a driver.
The image keeps its router and all interrupted owners while the staged
runtime's binding reporter records the binding's line and hart (which name its one entry) and the
operation through `fatal::reset_recording`. The existing panic slot holds
four diagnostic words; no retained allocation is added. Successful binding
installs the part, writes the line, then routes the part's sources in the
order of their numbers. No unsafe code is added to placement or ownership transfer.

The generated Rust MTVT is one immutable array of aligned per-hart rows.
Its one unsafe attribute owns the SRAM section. The table and trap assembly
share a module/codegen unit; the existing `global_asm!` exports `_runtime_mtvt`
through a `sym` operand without a Rust export attribute.
Entries are optional references to opaque zero-sized address markers
declared inside `layout`'s existing extern block: reading a marker touches
no instruction memory and Rust cannot call it as a function. Compile-time
checks enforce the RV32 slot width and row size. Machine entry/exit code
remains in `global_asm!`, with frame operands derived from `TrapFrame`.

Outside `oer-portable-riscv`, firmware assembly is only `global_asm!`/`naked_asm!` for
entry/trap ABI work: platform-bootstrap's `entry.rs`, platform-runtime's `entry.rs`
and trap-frame/stack entries in `stacks.rs`, and bootstrap/probe images. The
layer test
`architectural_instructions_stay_in_oer_riscv` lists these owners and locations
and rejects inline operations even there. Host compiler fixtures are test
inputs, not firmware instruction owners.

A generated register writer takes typed field values only: `bool`, a value
enum, an exact-width integer or the PAC's `Bits<WIDTH>`; a reader returns
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
containment, secondary launch and fatal reset use those owned typed
accessors. The HAL calls no ROM function: stage two owns the whole internal
SRAM, the ROM's data included; the facts' export test holds the declarations
to the empty `[rom] functions`.

A secondary hart's entry is the staged runtime's generated assembly
(`entry.rs`, `SECONDARY_ENTRY`): before any stack exists it masks interrupts,
installs its fault stub, stores to the SRAM handshake (a `sym` operand) and
programs the PMA chain from the facts, then installs its own `gp` and task
stack. The same module mints the hart's `SecondaryEntry` once
(`SecondaryEntry::new::<H>`, which refuses the boot hart at monomorphization;
its contract is that address and that entry behaviour). The entry tail-calls
`runtime_secondary_main_h<N>`, which the image's `secondary_entry!` exports
(`export_name`, a second definition fails the link) beside its SRAM
`Mailbox` (`link_section`). That root's `__root` checks the running hart
against `H` and takes the mailbox, which only the launch of the root's one
permit fills, once; only then does it mint the hart's `PreAdmission<H>`, so
each hart's authority exists once per boot. The handshake is a `placed_static!` in SRAM critical data. A mixed-word trigger preserves ordinary fields and unassigned bits,
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

Each chip's HAL has one module that makes typed DMA addresses,
`memory::dma_address`: the only caller of the `unsafe` `DmaWindowAddress` and
`DmaRef` constructors of `oer-portable-memory`, which the PAC's generated `layouts`
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
release path.

Its `memory::dma_static` module holds the placement constructor:
`DmaStatic::placed`, `unsafe` and called only by the `dma_static!` macro,
whose section is checked to be `.dma.bss.<block>`; the runtime linker
script's ASSERTs over the DMA range (from the generated `dma-window.x`) are
the proof.

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
the common fault envelope records the named reason, operation, epoch, SAR
RAW/ST, request/reply lengths and invalid-delay values. Numeric foreign-reunion
identities are observations only, without ownership or dereference authority.
The HAL declares the direct `link_fn!` binding and exports
`phy_fault_reporter!` to define that exact signature. Staged images bind it
to `oer-chips-esp32s31-platform-runtime`'s `phy_fault::reset`, which calls
`fatal::reset_recording`. Its common-format record preserves the source
payload and is cleared only after successful next-boot output.
Isolated comparison probes supply their own reporter through the same HAL
macro, preserve their first named fault in session RAM and terminate through
their own probe halt/panic path. They use verifier linker scripts and
no staged runtime; the radio probe's staged hardware companion uses the
normal runtime record/reset path. Host tests use separate seats and catch
their own reporter's unwind; both payloads remain quarantined there too.
No new unsafe block or sharing implementation is introduced.

`oer-portable-memory` holds the shared proofs for executor-crossing storage.
`ExecutionContext` is the unsafe execution-identity contract; its `Sharing`
marker distinguishes a globally exclusive context from each constructing
thread's local context. `Entered<X>` has a private constructor, checks
`X::running()` at entry and rechecks it before every cell access. It is
!Send/!Sync; retaining it never extends admission beyond its context.
`oer-portable-memory::signal` owns the signal task cell, pending facts and intrusive
list, with no Espressif or CLIC dependency. Its service callback runs after
releasing the cell borrow. `ContextCell<T, P: ContextProof>` holds that state
and timer slots. A private unchecked owner proof or a checked
`Entered<X>` confines each access, including across preemption and cores.
Only a `Sync` sharing marker and a `Send` payload permit sharing the cell;
its move restrictions also include the proof and sharing marker. Host
contexts keep their cells and owners !Send/!Sync, including references to
owners. The transferable host raiser contains only an atomic flag reference,
never a signal, list, task handle or proof; it never services payload or
invokes a waker. Host/deferred constructors return both halves.
The cell's sole `Sync` implementation and mutable accessor live in
`oer-portable-memory`. Its non-atomic borrow flag refuses a nested `with` on the same
cell with `None`, without invoking the closure or changing payload; borrowing
another cell is permitted. A guard ends the borrow on return or unwind.
Signal reads and registration report refusal through `ForeignContext`;
service leaves unvisited facts pending. `SignalExecutor` is safe; each
executor's context type (`ThreadContext<SWI>`, `InterruptContext<SWI>`, which
name no binding, so each signal list stays one static) carries the existing
unsafe implementation that proves `ExecutionContext` instead.
Timer registration wakes the task to register after the enclosing access
ends; timer service leaves its alarm state untouched when refused. No
interrupt masking, waiting or lock is used.

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

`Placed::replace_into` swaps two distinct, exclusively owned initialized
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

`RequestSlot` carries one request at a time from a client to a server and
its answer back, in one place. Its state (`IDLE`, `POSTED`, `WORKING`,
`ANSWERED`) admits exactly one accessor to the exchange, and the slot stores
no wakers. Its unique server endpoint takes a posted request by an Acquire
CAS, lends `&mut Req` only in `WORKING` and publishes `ANSWERED` after that
borrow ends. Its unique client observes `ANSWERED`
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

`retained` holds records in memory selected for best-effort reset retention;
the reset source determines whether bytes survive. `NOLOAD` only excludes
startup initialization ([fatal/reset limits](architecture.md#fatal-decisions-and-reset)). A
`RetainedCell<T: bytemuck::AnyBitPattern>` reads what the previous boot left
as a `T`, so every bit pattern must be a value; `try_lock` hands the record
to one holder at a time without waiting, and its claim lives in a section
every boot zeroes, so a reset never leaves the record claimed. The cell is
defined only by `__retained_static!`, whose two statics are its own; the
staged runtime's `retained_static!` names the sections. `read_volatile`
reads a value behind a shared borrow with a volatile load; `write_volatile`
stores a `Copy` value through an exclusive borrow. `StaticRef<T>`
publishes a `&'static T` through an atomic pointer that interrupt handlers
and the panic path read without a lock. `link_fn!` binds a function at link time through an unsafe-owned `LinkSlot`.
The slot reserves its name (or indexed family), function/static kind and exact
value type, including a function's calling convention and complete signature.
Safe declarations and definitions check all three; private emission arms also
check the name, so a direct call cannot select another symbol. The former
unrestricted `define fn` and string-named `export` forms are unavailable.
`oer-portable-memory::TraceClockSlot` owns `__oer_trace_now_micros: fn() -> u64`;
`oer-portable-trace` retains `forbid(unsafe_code)` and contains no unsafe site.
The HAL's `domain::transaction::PhyFaultSlot` owns
`__oer_phy_transaction_fault: fn(Fault) -> !`. This one narrow unsafe slot
implementation is the HAL's permitted one-site caller-count exception.
`oer-chips-family-platform-interrupt-table` owns its table, vector and exemption slot
contracts; the memory home emits their attributes, preserving the image ABI.
Duplicate strong definitions are rejected by the linker. A slot's unsafe owner
must keep its declared name/type consistent with every foreign reader; safe
exports cannot establish that external contract on the owner's behalf.
Calls remain direct for stack analysis.
`DmaBytes<N>` holds bytes whose address a driver publishes to a
DMA engine: reads copy them out by volatile accesses through `&self`,
writes copy them in through `&mut self`, so no reference aliases memory the
engine writes.

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

`oer-tools-os-process` holds the host's raw system calls that rustix does not
make safe: `child_setup` for what a child does between fork and exec,
`launch_handoff` for the descriptor a fixed launcher passes its
helper across `exec` (the LAB service adopts the socket systemd hands it the
same way), and, behind the `linux-socket` feature, `linux_socket` for the
link-layer, HCI and L2CAP addresses and three socket options. `oer-tools-os-serial`
takes an open serial port's descriptor; it is its own crate so that the root
fixture installer, which links `oer-tools-os-process`, does not link `serialport`.

## Firmware and platform code below the boundary

The platform crates (the boards and `chips/family/staged-*`), the bootstrap images
(`chips/esp32s31/platform/bootstrap`) and the probe images (`hil/probes/*`) own the
target ABI and placement, so they hold unsafe code that no crate above them
needs:

- The staged runtime defines the image entry and its panic handler (`entry!`,
  whose optional `hook` parameter exports the image's panic hook symbol, the
  `Handoff` and `Adopted::enable_interrupts`), the sections the linker script
  maps (`placed_static!`, `retained_static!`), the exception handler symbol
  (the common binding in `entry!`, with an optional supplemental `exception`
  hook), trap snapshots and vector-table reads (`trap`), and the linked sections (`layout`).
  Images and HIL agents reach these through safe APIs and forbid unsafe code.
  The critical section masks its executing hart only. Firmware packages depending
  on the staged runtime declare `critical-section-hart`. The staged build/run
  pipeline checks linked section entry symbols and inline copies against the
  image's declared per-hart task roots, interrupt-table handlers and exception
  roots, using existing call resolutions. Foreign roots must have complete
  resolved paths excluding those entries; the declared hart may retain partial
  bounds. A secondary hart starts only through the image's declared hart set
  and the root's one-shot launch permit. `cargo fw probe`
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
- The family PSRAM driver owns the reset root's controller/MMU adapter while
  running cold bring-up. Only success creates `MappedPsram<R>`, with private
  fields and the same owner; failure retains it in `PsramError<R>`. The chip's
  `PsramMemory` adapter fixes the MMU window, so caller-supplied geometry cannot
  authorize a slice at an arbitrary address. The HAL's
  `PsramRegisters::into_region` consumes the witness for its exact fixed owner
  and retires remap authority before creating one `&'static mut [u8]`, at the
  same single unsafe site. Observed ranges and other owners cannot authorize
  that loan. Chip UI twins check these ownership boundaries. The safe `stage`
  uses slice offsets, checked ranges and volatile borrowed probe/CRC accesses,
  returning typed errors.
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

The bootstrap and probe crates deny `clippy::undocumented_unsafe_blocks`, and
each unsafe attribute in them carries its `// SAFETY:` comment as well.

## PAC dependency authority

Direct dependencies on the PAC are restricted independently of unsafe
syntax. The allowed consumers are the chip's `hal` under `esp32s31/` and the timer
facts probe `hil/probes/esp32s31-timer-facts` for the part types of its one
second claim (above). Any other package requires a separate layer-test
admission. All other drivers and adapters reach registers through HAL owners. Check the executable
list when changing this boundary.

Upper layers use opaque, finite capabilities rather than raw pointers,
unchecked lifetimes, generic PAC callbacks or independently reusable interrupt
owners. MMIO outside the restricted PAC must use typed accessors. Missing
register fields are reviewed in `chips/<chip>/registers/` and regenerated
into the PAC, never reconstructed with local masks in a HAL or adapter.

The Wi-Fi source120 fatal-assertion path keeps the full sampled RAW as
an opaque diagnostic. It quarantines both interrupt banks before returning a
named `irq::Fault` to the image. The image retains the preempted DMA owners and
the handler's register owners through the chip composition's
(`oer-chips-esp32s31-platform-runtime`) never-returning shared `wifi_fault::reset`, which
calls the runtime's `fatal::reset_recording`,
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
Wi-Fi consumes and clears its retained record before console output and
ignores output refusal; next-boot reporting is best effort, without replay
after a failed write. Other codecs have their own claim and consumption
rules ([fatal/reset limits](architecture.md#fatal-decisions-and-reset)).

## Active-owner lifetime

Normal stop proves IRQ, RX, TX and queue quiescence before returning reusable
storage. `Drop` cannot perform that asynchronous proof: abnormal drop retains
or quarantines active static storage and faults the lifecycle. It must not
reset hardware, panic as a release mechanism or claim successful shutdown.
Because safe Rust permits `mem::forget`, forgetting an owner must remain a
harmless leak rather than release memory still accessible by hardware.

Radio port cancellation cleanup belongs to the backend's P2 registration
guard. The guard survives portable driver loans and performs its mandatory
cleanup on Drop, including before first poll. Sans-IO lifecycle and slot code
only report typed failures; they do not invoke the hardware cleanup policy.

Task boundaries, cancellation and rendezvous do not weaken the same ownership
or quiescence obligations. The wider execution and
memory-placement contracts are in the
[architecture](architecture.md).
