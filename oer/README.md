# oer

The Espressif family's application entry. An image names the capabilities
it uses (`console`, `group`) and no chip: the image pipeline
selects the board's chip (`esp32s31`, exactly one). It takes the peripherals
it needs once and builds drivers and services with defaults. The library
places their interrupts, executors and service tasks and applies record and
reset on the image's behalf. The entry has no Wi-Fi role in the built tree:
the old Wi-Fi path it stood on is outside the build
([repository layout](../docs/repository-layout.md)).

```rust
oer::entry!(boot, group = second);

static LINK: Channel<Request, Reply, 1> = Channel::new();

fn boot(p: Peripherals, mut boot: Boot) -> Result<impl Started, Error> {
    let engine = p.aes.engine(&mut boot)?;
    let link = LINK.application(&mut boot)?;
    Ok(boot.run(move || app(engine, link)))
}

fn second(_p: GroupPeripherals, mut boot: GroupBoot) -> Result<impl GroupStarted, Error> {
    let link = LINK.group(&mut boot)?;
    Ok(boot.run(move || serve(link)))
}
```

`hil/images/aes` is the built image on the entry.

## Boot and run

`entry!(boot)` declares the image's entry. After stage-two adoption the
library reports the previous boot's retained faults (what it read, hart 0's
reset cause, the previous panic and the previous CPU exception, stays
readable through `Boot::evidence`) and the task stack's watchpoint, then
calls `boot` once
on hart 0, before any executor and with interrupts disabled. `boot` moves
peripherals into drivers and services through the boot context (`Boot`):
their interrupt bindings, executors and service tasks are made there and
nowhere else. `Boot::run` ends the boot with what builds the application's
future: a closure the library calls where the future stays, so the future
is never moved.

`entry!(boot, group = boot1)` names the group. The library calls
`boot1` once `boot` returned, still before any executor, with what the boot
handed it (`GroupPeripherals`) and its own context (`GroupBoot`). `boot1`
ends with `GroupBoot::run` and the closure that builds its future. Data of
the image's own reaches it through its statics: a `Channel` links the
application's future and the group's. `entry!(boot)` declares no group.

The library then installs the system timer, draws the boot's identity
(`boot_id`, nonzero, random) from the entropy source once the timer runs,
enables interrupts and runs the application executor in thread mode with
its service tasks (`SdkTasks`) and the image's futures as the executor's
roots: the group's future and the application. The library builds them in
the frame of its run, which never returns, and they stay there as tasks of
the executor after `SdkTasks`; no static slot names their types, so an image
declares no task set and needs no unstable feature. `boot` returns `impl
Started` and `boot1` `impl GroupStarted`, which name the future types:
`Boot::run` and `GroupBoot::run` make them.

## Default layout (ESP32-S31)

| Context | Level | Entries |
| --- | --- | --- |
| application | 1 | executor wake (line 40), its alarm (system timer comparator 1, line 41), console (USB Serial/JTAG, line 42) |
| hart 1 application (`group`) | 1 | hart 1's executor wake (line 40), its alarm (comparator 2, line 41) |

The levels 2 to 5 of the grid (`radio_executor`, `keeper`, `radio`,
`profile`) have no entry of the entry's. The console's entry exists with
`console` and hart 1's with `group`; an entry no driver binds routes no
source. With `group` the library launches hart 1 once the boot ended: it
runs the group's setup there and then its future on its own thread-mode
executor, and hart 0 waits for the setup's end before it enables its
interrupts. The pipeline gives such an image hart 1's stacks. Service tasks
on the application executor: the RTC watchdog feed.

| Default | Value |
| --- | --- |
| `println!` line | `{}` holes only; 8 segments (holes and non-empty pieces), 7 value words, 6 pieces of 96 bytes together, all checked when it compiles; an error's path, the last value, past the line's room is marked `line-truncated` |

## Services

`AesPeripheral::engine` makes the accelerator the image's synchronous
AES-128 engine (`AesEngine`, an `oer_portable_crypto::block::Aes128Engine`), owned by
whoever holds it: each block waits on its `STATE` flag within its settle,
and an operation borrows the engine for the length of its call
(`oer_portable_crypto::rfc3394::unwrap`), so no operation outlives it and nothing
waits on another holder. The AES interrupt stays silent.

`Console::open` takes the USB Serial/JTAG block from the early writer and
binds its interrupt in the application's context: the stream
(`UsbConsole`) reads and writes as `embedded_io_async` does, for a HIL agent
or a log of the image's own. From then on `println!` writes nothing and
returns `false`; a fatal note still reaches the host.

## Failures

| When | Fate of the owners | Result |
| --- | --- | --- |
| A boot step fails after a binding or a claimed storage | bound owners stay bound, claimed storage stays claimed | `Error`, which the library records and resets on |
| `boot` returns `Err`, or the application's future returns `Err` | all owners stay where they are | record and reset |
| The entropy source fails its health check when the library draws the boot's identity | all owners stay where they are | record and reset |
| A CPU exception | all owners stay where they are | the runtime records the trap frame before it stops the other hart, the library's hook (`oer-chips-esp32s31-platform-runtime::exception`) what the interrupt controller showed after the stop; reset. The next boot prints both and serves them in `Boot::evidence` (`previous_exception`) |
| A channel's refusal | unchanged | handled `Result` |

## Cancellation

| Dropped | Effect |
| --- | --- |
| `ApplicationEnd`/`GroupEnd::send` or `receive` | the value not sent stays with the caller; a received value is never lost: it stays in the queue until a receive returns it |

## Channels

`static LINK: Channel<A, B, N>` links the application's future and the
group's: `A` values go to the group, `B` values back, each way
a queue of `N` (`A` and `B` are `Send`). The boot takes the application's
end (`LINK.application(&mut boot)`), which splits the channel; the second
boot takes its end after it (`LINK.second(&mut boot1)`). Each end is taken
once (`Error::Channel`). `send` waits for room and `receive` for a value;
`try_send` and `try_receive` do not wait. An end stays with the future that
polls it (`!Send`).

## A hart's counters (`oer::cpu`)

The family's observation counters (`chips/family/app/src/observe`) are
outside the build with their one reader, `hil-wifi-station`; their files
stay as sources. What a hart counts of its own work is in `oer::cpu`, in
every build: it hooks no poll and no frame. The entry hands
each hart's reader to the code that runs there; each stays on its hart.

| Reader | What |
| --- | --- |
| `Peripherals::cpu` | the boot hart's `Cpu0`: `counters(start)` reads its cycles, retired instructions and the L1 cache's counters, with `start` the cache's cleared and running first |
| `GroupPeripherals::cpu` (`group`) | hart 1's `Cpu1`, made on hart 1: `counters()` reads its cycles and retired instructions |
| `Cpu0::idle`, `Cpu1::idle` (`cpu-load` class) | the hart's idle account: its microseconds in the executor's wait for an interrupt since boot and its wakes, read on the hart that writes it |

`start_epoch()` starts a new epoch of the longest polls and the extremes.
Each counter has one writer; a reader in thread mode on either hart takes a
counter whole or yields and reads again. Counts wrap at 2^32: a reader
takes differences.

## Limits

The application runs on hart 0. The AES engine runs the accelerator in
Typical mode without DMA, one synchronous block at a time.
`println!("ipv4 {}/{}", Ipv4(address), prefix)` writes a typed line
(`oer_portable_diagnostics::typed_line!`): the format's static pieces and the
values, which the host prints; the target formats nothing. A hole is a bare
`{}`; `Hex` and `Ipv4` choose a value's text, and an error that describes
itself as a diagnostic path is the last value.
It uses the early writer, which waits for room within its budget per line.

```console
cargo test -p oer
cargo clippy -p oer --all-targets
cargo clippy -p oer --target riscv32imafc-unknown-none-elf --features esp32s31,console,group
RUSTFLAGS="--cfg oer_trace" CARGO_TARGET_DIR=target/instrumented/oer \
  cargo clippy -p oer --target riscv32imafc-unknown-none-elf --features esp32s31,console,group
```
