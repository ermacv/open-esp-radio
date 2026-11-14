# oer

The Espressif family's application entry. An image names the capabilities
it uses (`station`, `access-point`, `console`, `group`, `observe`) and no chip: the
image pipeline selects the board's chip (`esp32s31`, exactly one). It
takes the peripherals it needs once and builds drivers and services with
defaults. The library places their interrupts, executors and service tasks
and applies record and reset on the image's behalf.

```rust
oer::entry!(boot, group = network);

static LINK: Channel<Request, Reply, 1> = Channel::new();
enum Brand {}
static NETWORK: ConstStaticCell<wifi::NetworkStorage<Brand, Producer, wifi::Copying>> =
    ConstStaticCell::new(wifi::NetworkStorage::new());
static MEMORY: ConstStaticCell<wifi::NetworkMemory> =
    ConstStaticCell::new(wifi::NetworkMemory::new());

fn boot(p: Peripherals, mut boot: Boot) -> Result<impl Started, Error> {
    let config = wifi::Config::new(*b"DE", 6, 10)?;
    let station = Station::start(p.wifi, p.aes, config, &mut boot)?;
    let link = LINK.application(&mut boot)?;
    Ok(boot.run(move || app(station, link)))
}

fn network(p: GroupPeripherals, boot: GroupBoot) -> Result<impl GroupStarted, Error> {
    let device = p.device.ok_or(Error::NoDevice)?;
    let storage = NETWORK.try_take().ok_or(Error::Storage)?;
    let memory = MEMORY.try_take().ok_or(Error::Storage)?;
    let (net, socket, mut boot) = wifi::Network::start(
        device, wifi::Addressing::Dhcp, boot, storage, memory, Producer, PORTS,
    )?;
    let link = LINK.group(&mut boot)?;
    Ok(boot.run(move || sockets(net, socket, link)))
}

async fn app(mut station: Station, link: ApplicationEnd<Request, Reply, 1>) -> AppResult {
    station.connect(wifi::Profile::wpa2(b"ssid", b"passphrase")?).await?;
    // requests to the sockets' future over `link`
}

async fn sockets(net: wifi::Network<Brand, Producer, wifi::Copying>, socket: wifi::Socket<Brand>, link: GroupEnd<Request, Reply, 1>) -> AppResult {
    // the address from `net.configuration()`, datagrams on `socket`, answering `link`
}
```

`examples/wifi-station` is the complete example.

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
handed it (`GroupPeripherals`: each role's network device, which
`Station::start` and `AccessPoint::start` keep for it) and its own context
(`GroupBoot`). `boot1` starts the networks (`wifi::Network::start`), whose
sockets stay in its future, and ends with `GroupBoot::run` and the closure
that builds that future. A stopped network closes its sockets, which return
their datagrams' lent receive buffers to the role. Data of the image's own
reaches it through its statics: a `Channel` links the application's future
and the group's. `entry!(boot)` declares no group; a role's device no
network takes tells its role that no network takes its frames.

The library then installs the system timer (the radio executor writes it
when a station started, the application executor otherwise), enables
interrupts and runs the application executor in thread mode with its
service tasks (`SdkTasks`) and the image's three futures as the executor's
roots: the group's networks, over the image's storage, the second
boot's future and the application. The library builds them in the frame of
its run, which never returns, and they stay there: and are tasks of the executor after `SdkTasks`; no
static slot names their types, so an image declares no task set and needs
no unstable feature. `boot` returns `impl Started` and `boot1` `impl
GroupStarted`, which name the future types: `Boot::run` and
`GroupBoot::run` make them, and `wifi::Network::start` returns the second
boot's context with that network beside the ones before.

## Defaults and settings

Every constructor works with the library's defaults; an image that needs
other values states them in its own `boot`, the same way an example and
a HIL image of one function build it:

| Setting | Default | Override |
| --- | --- | --- |
| Reconnect after a lost association | every 1 s (`wifi::RECONNECT`) | `wifi::Config::with_reconnect(interval)`, `without_reconnect()` |
| Drain at a stop | 2 s (`wifi::DRAIN`) | `wifi::Config::with_drain(deadline)` |
| Channel width of a join | 20 MHz | `wifi::Config::with_ht40()`: 40 MHz where the access point's HT elements name a secondary channel the domain permits |
| HE (802.11ax) of a connect | off: HT | `wifi::Profile::with_he20()`, experimental: HE20 (one stream, HE-MCS 0 to 9, no optional HE capability, no answer to Trigger frames) with an access point that admits it, HT otherwise; `Connected::phy` names the mode |
| A network's storage | `wifi::BLOCKS` blocks of `wifi::BLOCK` octets, two sockets | the image's own `static NETWORK: wifi::NetworkStorage<Brand, Producer, Receive>` and its zeroed `static MEMORY: wifi::NetworkMemory` (the blocks), one each per network |

A zero interval or deadline is refused (`ConfigError`). Each storage is
taken once; a second start of a device returns `Error::AlreadyStarted`.
Before the executor starts the library draws the boot's identity
(`boot_id`, nonzero, random) from the entropy source: a station draws it
with the source it takes, otherwise the library does once the timer runs.

## Default layout (ESP32-S31)

| Context | Level | Entries |
| --- | --- | --- |
| application | 1 | executor wake (line 40), its alarm (system timer comparator 1, line 41), console (USB Serial/JTAG, line 42) |
| radio executor | 2 | SAR/PWDET pilot (line 16), executor (line 17) |
| radio | 3 | radio alarm (comparator 0, line 19), Wi-Fi MAC and doorbell (line 21) |
| profile (`observe`) | 4 | PC sampler (TIMG1 T0, line 22); with `group`, hart 1's PC sampler (TIMG1 T1, line 22 of hart 1) |
| hart 1 application (`group`) | 1 | hart 1's executor wake (line 40), its alarm (comparator 2, line 41) |

The radio entries exist with a Wi-Fi role (`station`, `access-point`), the console's with
`console`, the sampler's with `observe` and hart 1's with `group`; an entry
no driver binds routes no source. With `group` the library launches hart 1
once the boot ended: it runs the group's setup there and then its networks
and future on its own thread-mode executor, and hart 0 waits for the setup's
end before it enables its interrupts. The pipeline gives such an image hart
1's stacks. Service tasks on the application executor: the RTC
watchdog feed, the station runner and the MAC driver beside it, or the
access point's runner; without `group`, the group's networks. The radio executor
runs the radio actor.

| Default | Value |
| --- | --- |
| IP packet pools | 8 receive, 8 transmit credits, 8 general, 1514 bytes each |
| IP waiters | 4 tasks waiting at once |
| Station reconnect | every second after a lost association |
| `println!` line | `{}` holes only; 8 segments (holes and non-empty pieces), 7 value words, 6 pieces of 96 bytes together, all checked when it compiles; an error's path, the last value, past the line's room is marked `line-truncated` |

## Services

`AesPeripheral::engine` makes the accelerator the image's synchronous
AES-128 engine (`AesEngine`, an `oer_portable_crypto::block::Aes128Engine`), owned by
whoever holds it: each block waits on its `STATE` flag within its settle,
and an operation borrows the engine for the length of its call
(`oer_portable_crypto::block::unwrap_now`), so no operation outlives it and nothing
waits on another holder. The AES interrupt stays silent.

`wifi::Station::start` takes the radio, the entropy source and the AES
accelerator, which it owns for good: the station unwraps its EAPOL-Key data
in its own step on the accelerator, each block waited for on its `STATE`
flag. An image that starts the station therefore takes no AES engine of its
own (the peripheral moves into one of them). Its configuration names one channel (`wifi::Config::new`) or every
2.4 GHz channel the country permits, scanned at each connect
(`wifi::Config::all_channels`). `connect(profile)`
ends with the result of that connection attempt: `Connected`, or a
`ConnectError` with the station's own cause (`Failed(DisconnectReason)` names
`NotFound`, `AuthenticationFailed`, `AssociationFailed`, `HandshakeFailed`
and the others; none is reported as a wrong password). Once a connect
linked, the station keeps the network and rejoins it after a lost
association; a connect that ended without a link leaves the station idle
for the application's next connect, with the same or another profile.
`linked()` names the access point and channel while the link is up.
`stop()` leaves the network and stops the station for good: the future ends
once the radio retired, with `StopError::Failed` and the station's typed
failure when it ended by one instead (`Unavailable` when it never started,
`RunnerLost` when its runner ended without an exit), `StopError::ReleaseFailed`
with its typed cause when it stopped but its radio's release failed (nothing
resets), and a later
connect answers `ConnectError::Stopped`. A
start the radio refused resets nothing: the first connect answers
`ConnectError::NotStarted` with the refusal's typed cause and how its
owners ended.
`Station::start` returns the station and keeps its network device for the
group (`GroupPeripherals::device`); `AccessPoint::start` keeps the access
point's (`GroupPeripherals::access_point`). `wifi::Network::start`, in the
group, starts the network core over a role's device with its addressing
(`Addressing::Dhcp` or `Addressing::Given`), the storage the image lends
(`NetworkStorage`, of a brand the image names, and `NetworkMemory`, its
blocks) and a producer, opens its
channel socket (`Copying` or `Lending<CREDIT>`) and its producer's socket,
and returns them with the group's context. `configuration()`, `changes()`
and `changed()` read the interface's address, its lease and the link as the
service publishes them. The station lends received frames to the network in
place; when a role stops (by `stop()` or after a failure, within the drain
deadline) its network stops first and returns every buffer it holds. A role
whose device no `Network::start` took tells its role that no network takes
its frames. TCP has no implementation.

`Console::open` takes the USB Serial/JTAG block from the early writer and
binds its interrupt in the application's context: the stream
(`UsbConsole`) reads and writes as `embedded_io_async` does, for a HIL agent
or a log of the image's own. From then on `println!` writes nothing and
returns `false`; a fatal note still reaches the host.

## Failures

| When | Fate of the owners | Result |
| --- | --- | --- |
| A boot step refuses a value (configuration, profile) before binding | the peripheral was moved into the call and is dropped | `Error`, which the library records and resets on |
| A boot step fails after a binding or a claimed storage (`Storage`, `Entropy`, `Net`) | bound owners stay bound, claimed storage stays claimed | `Error`, record and reset |
| `boot` returns `Err`, or the application's future returns `Err` | all owners stay where they are | record and reset |
| The entropy source fails its health check when the library draws the boot's identity | all owners stay where they are | record and reset |
| The station runner exits other than an ordinary stop (failed, poisoned, quarantined, retirement or release failure, abandoned, initialization) | the runner keeps every owner | record and reset |
| A fatal MAC assertion, a PHY transaction fault, nonce exhaustion | owners quarantined by their fault owner | record and reset |
| The radio actor's periodic PHY tracking fails, or its deadline is one the clock cannot hold | the actor keeps every owner | the fault's class and variant path (`oer-chips-esp32s31-platform-runtime::radio_fault`) recorded, reset; the next boot prints it |
| A CPU exception | all owners stay where they are | the runtime records the trap frame before it stops the other hart, the library's hook (`oer-chips-esp32s31-platform-runtime::exception`) what the interrupt controller showed after the stop; reset. The next boot prints both and serves them in `Boot::evidence` (`previous_exception`) |
| A network's service ends other than by its role's stop | the service keeps its owners | record and reset |
| `connect` refused or failed, a socket's refusal | unchanged | handled `Result` |

## Cancellation

| Dropped | Effect |
| --- | --- |
| `Station::connect` while its profile is prepared | the derivation and the passphrase are erased; nothing was sent |
| `Station::connect` after admission | the attempt continues; the next connect waits for its end; `disconnect` leaves the network |
| `Network::changed` | nothing to free: one task waits at a time, and the next publication wakes it |
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

## Observation (`observe`)

An image that measures the library's work enables `observe` and declares
`instrumentation = ["trace"]` (without it the build fails). The library's
executors, station, RF actor and network then report to its own
counters, which `oer::observe` reads in the library's types; an
image translates them into whatever reports them:

| Reader | What |
| --- | --- |
| `executors()` | each executor's work and idle microseconds since boot (application, radio, hart 1) |
| `parts()` | each part's polls and their time, and the longest of the epoch: the library's tasks and futures, the Wi-Fi datapath actor's spans, and up to four futures the image names (`Part::claim(name)?.polled(future)`) |
| `counters()` | the station's receive decisions, its transmit agreements' settlements, the link's rates and the receive driver's counters |
| `tracking()` | the RF actor's PHY tracking ticks, with the epoch's temperatures and longest tick |
| `Boot::observe` | the boot hart's handle (`Hart0`, `!Send`): the PC sampler on TIMG1 T0 and the receive path's stall snapshot, once |
| `GroupBoot::observe` (`group`) | hart 1's handle (`Hart1`, `!Send`): its PC sampler on TIMG1 T1, once; each hart's sampler keeps 1024 samples |

What a hart counts of its own work is apart from these, in `oer::cpu`,
with or without `observe`: it hooks no poll and no frame.

| Reader | What |
| --- | --- |
| `Boot::cpu` | the boot hart's `Cpu0`, once (`Error::CpuTaken` after): `counters(start)` reads its cycles, retired instructions and the L1 cache's counters, with `start` the cache's cleared and running first |
| `GroupBoot::cpu` (`group`) | hart 1's `Cpu1`, once: `counters()` reads its cycles and retired instructions |
| `cpu::idle(hart)` | the hart's idle account: its microseconds in the executor's wait for an interrupt since boot and its wakes, or `Concurrent` while the hart writes it (read again) |

`start_epoch()` starts a new epoch of the longest polls and the extremes.
Each counter has one writer; a reader in thread mode on either hart takes a
counter whole or yields and reads again. Counts wrap at 2^32: a reader
takes differences.

## Limits

The application runs on hart 0. The AES engine runs the accelerator in
Typical mode without DMA, one synchronous block at a time; the station
computes RFC 3394 key unwrap on it. One radio holder at a time: an image
with both roles serves one of them on the air at once; each role's network
is its own instance. `println!("ipv4 {}/{}", Ipv4(address), prefix)` writes a typed line
(`oer_portable_diagnostics::typed_line!`): the format's static pieces and the
values, which the host prints; the target formats nothing. A hole is a bare
`{}`; `Hex` and `Ipv4` choose a value's text, and an error that describes
itself as a diagnostic path is the last value.
It uses the early writer, which waits for room within its budget per line.

```console
cargo test -p oer
cargo clippy -p oer --all-targets
cargo clippy -p oer --target riscv32imafc-unknown-none-elf --features esp32s31,station,console,group
RUSTFLAGS="--cfg oer_trace" CARGO_TARGET_DIR=target/instrumented/oer \
  cargo clippy -p oer --target riscv32imafc-unknown-none-elf --features esp32s31,station,console,group,observe
```
