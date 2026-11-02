# oer-espressif

The Espressif family's application entry. An image selects its chip by
feature (`esp32s31`, exactly one) and the capabilities it uses (`wifi`,
`console`, `hart1`, `observe`, `udp`),
takes the peripherals it needs once and builds drivers and services with
defaults. The library places their interrupts, executors and service tasks
and applies record and reset on the image's behalf.

```rust
oer_espressif::entry!(boot, hart1 = network);

static LINK: Channel<Request, Reply, 1> = Channel::new();

fn boot(p: Peripherals, mut boot: Boot) -> Result<impl Started, Error> {
    let config = wifi::Config::new(*b"DE", 6, 10)?;
    let (station, status) = Station::start(p.wifi, p.aes, config, &mut boot)?;
    let link = LINK.application(&mut boot)?;
    Ok(boot.run(move || app(station, status, link)))
}

fn network(p: Hart1Peripherals, boot: Hart1Boot) -> Result<impl Hart1Started, Error> {
    let device = p.device.ok_or(Error::NoDevice)?;
    let (net, mut boot) = Net::start(device, NetConfig::Dhcpv4, boot)?;
    let link = LINK.second(&mut boot)?;
    Ok(boot.run(move || sockets(net, link)))
}

async fn app(mut station: Station, status: NetStatus, link: ApplicationEnd<Request, Reply, 1>) -> AppResult {
    station.connect(wifi::Profile::wpa2(b"ssid", b"passphrase")?).await?;
    let ipv4 = status.wait_ipv4().await?;
    // requests to the sockets' future over `link`
}

async fn sockets(net: Net, link: SecondEnd<Request, Reply, 1>) -> AppResult {
    // sockets on `net.stack()`, answering `link`
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

`entry!(boot, hart1 = boot1)` names the second boot. The library calls
`boot1` once `boot` returned, still before any executor, with what the boot
handed it (`Hart1Peripherals`: the station's network device, which
`Station::start` keeps for it) and its own context (`Hart1Boot`). `boot1`
starts the IP service (`wifi::Net::start`), whose stack and sockets stay in
its future, and ends with `Hart1Boot::run` and the closure that builds that
future. The stack has TCP sockets; with
`udp` also UDP sockets, each queueing up to 16 received datagrams. A
stopped IP service closes every UDP socket, which returns its queued
datagrams' lent receive buffers to the station. Data of the image's own reaches
it through its statics: a `Channel` links the application's future and the
second boot's. `entry!(boot)` declares no second boot; a station's device
it would take tells the station that no network takes its frames.

The library then installs the system timer (the radio executor writes it
when a station started, the application executor otherwise), enables
interrupts and runs the application executor in thread mode with its
service tasks (`SdkTasks`) and the image's three futures as the executor's
roots: the second boot's IP service, at the image's pool sizes, the second
boot's future and the application. The library builds them in the frame of
its run, which never returns, and they stay there: and are tasks of the executor after `SdkTasks`; no
static slot names their types, so an image declares no task set and needs
no unstable feature. `boot` returns `impl Started` and `boot1` `impl
Hart1Started`, which name the future types: `Boot::run` and
`Hart1Boot::run` make them, and `wifi::Net::start` returns the second
boot's context with its IP service.

## Defaults and settings

Every constructor works with the library's defaults; an image that needs
other values states them in its own `boot`, the same way an example and
a HIL image of one function build it:

| Setting | Default | Override |
| --- | --- | --- |
| Reconnect after a lost association | every 1 s (`wifi::RECONNECT`) | `wifi::Config::with_reconnect(interval)`, `without_reconnect()` |
| Drain at a stop | 2 s (`wifi::DRAIN`) | `wifi::Config::with_drain(deadline)` |
| Channel width of a join | 20 MHz | `wifi::Config::with_ht40()`: 40 MHz where the access point's HT elements name a secondary channel the domain permits |
| IP pools (receive, transmit, general frames) | 8, 8, 8 (`wifi::RX`, `TX`, `GENERAL`) | `static POOLS: wifi::Pools<RX, TX, GENERAL>`, `Net::start_with(device, &POOLS, config, boot)` in the second boot |

A zero interval or deadline is refused (`ConfigError`). The pools' sizes are types (`Net<RX, TX>`, `Stack<RX, TX>`),
defaulting to the library's. Each storage is taken once; a second start
returns `Error::AlreadyStarted`.
Before the executor starts the library draws the boot's identity
(`boot_id`, nonzero, random) from the entropy source: a station draws it
with the source it takes, otherwise the library does once the timer runs.

## Default layout (ESP32-S31)

| Context | Level | Entries |
| --- | --- | --- |
| application | 1 | executor wake (line 40), its alarm (system timer comparator 1, line 41), console (USB Serial/JTAG, line 42) |
| radio executor | 2 | SAR/PWDET pilot (line 16), executor (line 17) |
| radio | 3 | radio alarm (comparator 0, line 19), Wi-Fi MAC and doorbell (line 21) |
| profile (`observe`) | 4 | PC sampler (TIMG1 T0, line 22) |
| hart 1 application (`hart1`) | 1 | hart 1's executor wake (line 40), its alarm (comparator 2, line 41) |

The radio entries exist with the `wifi` feature, the console's with
`console`, the sampler's with `observe` and hart 1's with `hart1`; an entry
no driver binds routes no source. With `hart1` the library launches hart 1
once the boot ended: it runs the second boot there and then its IP service
and future on its own thread-mode executor, and hart 0 waits for the second
boot's end before it enables its interrupts. The image declares
`harts = ["boot", 1]`. Service tasks on the application executor: the RTC
watchdog feed, the station runner, the MAC driver beside it and the IP
service. The radio executor
runs the radio actor.

| Default | Value |
| --- | --- |
| IP packet pools | 8 receive, 8 transmit credits, 8 general, 1514 bytes each |
| IP waiters | 4 tasks waiting at once |
| Station reconnect | every second after a lost association |
| `println!` line | `{}` holes only; 8 segments (holes and non-empty pieces), 7 value words, 6 pieces of 96 bytes together, all checked when it compiles; an error's path, the last value, past the line's room is marked `line-truncated` |

## Services

`AesPeripheral::engine` makes the accelerator the image's synchronous
AES-128 engine (`AesEngine`, an `oer_crypto::block::Aes128Engine`), owned by
whoever holds it: each block waits on its `STATE` flag within its settle,
and an operation borrows the engine for the length of its call
(`oer_crypto::block::unwrap_now`), so no operation outlives it and nothing
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
once the radio retired, with `StopError::Failed` when the station ended by
a failure instead, and a later connect answers `ConnectError::Stopped`.
`Station::start` returns the station and the IP service's status for the
application (`NetStatus`), and keeps the station's network device for the
second boot. `wifi::Net::start`, in the second boot,
runs the existing IP service (`oer-embassy-net`) over that device;
`wait_ipv4` ends with the configuration over an up link and keeps waiting
across a link loss, `wait_link_down` ends on a loss or a new link epoch,
and both end with `NetError::Stopped` or `Failed` when the service ends.
`Net` has these waits for the second boot's future, `NetStatus` the same
and `status()` for the application's: the service publishes each status
whole for it and wakes it. A `NetStatus` stays with the application's
future (`!Send`), the one task its waits wake. Sockets open on
`net.stack()` (`embassy_net`, re-exported) in the second boot's future. The
station lends received frames to the IP service in place; when the station
stops (by `stop()` or after a failure, within the drain deadline) the IP
service stops first and returns every buffer it holds, and its waits end
with `NetError::Stopped`. A station whose device no `Net::start` took tells
the station that no network takes its frames.

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
| The radio actor's periodic PHY tracking fails, or its deadline is one the clock cannot hold | the actor keeps every owner | the fault's class and variant path (`oer-esp32s31-compose::radio_fault`) recorded, reset; the next boot prints it |
| A CPU exception | all owners stay where they are | the runtime records the trap frame before it stops the other hart, the library's hook (`oer-esp32s31-compose::exception`) what the interrupt controller showed after the stop; reset. The next boot prints both and serves them in `Boot::evidence` (`previous_exception`) |
| The IP service is refused its context | the service keeps its owners | record and reset |
| `connect` refused or failed, `wait_ipv4` timed out by the caller, a TCP error | unchanged | handled `Result` |

## Cancellation

| Dropped | Effect |
| --- | --- |
| `Station::connect` while its profile is prepared | the derivation and the passphrase are erased; nothing was sent |
| `Station::connect` after admission | the attempt continues; the next connect waits for its end; `disconnect` leaves the network |
| `Net::wait_ipv4` or `wait_link_down` | it frees its waiter slot; dropped outside the application's context, its waker stays until the next change takes it |
| `NetStatus::status`, `wait_ipv4` or `wait_link_down` | nothing to free: the application's waker stays registered until the next publication wakes it |
| `ApplicationEnd`/`SecondEnd::send` or `receive` | the value not sent stays with the caller; a received value is never lost: it stays in the queue until a receive returns it |

## Channels

`static LINK: Channel<A, B, N>` links the application's future and the
second boot's: `A` values go to the second boot, `B` values back, each way
a queue of `N` (`A` and `B` are `Send`). The boot takes the application's
end (`LINK.application(&mut boot)`), which splits the channel; the second
boot takes its end after it (`LINK.second(&mut boot1)`). Each end is taken
once (`Error::Channel`). `send` waits for room and `receive` for a value;
`try_send` and `try_receive` do not wait. An end stays with the future that
polls it (`!Send`).

## Observation (`observe`)

An image that measures the library's work enables `observe` and declares
`instrumentation = ["trace"]` (without it the build fails). The library's
executors, station, RF actor and IP service then report to its own
counters, which `oer_espressif::observe` reads in the library's types; an
image translates them into whatever reports them:

| Reader | What |
| --- | --- |
| `executors()` | each executor's work and idle microseconds since boot (application, radio, hart 1) |
| `parts()` | each part's polls and their time, and the longest of the epoch: the library's tasks and futures, the station service's spans, and up to four futures the image names (`Part::claim(name)?.polled(future)`) |
| `counters()` | the network bridge's frames, the station's receive decisions, its transmit agreements' settlements, the link's rates and the receive driver's counters |
| `tracking()` | the RF actor's PHY tracking ticks, with the epoch's temperatures and longest tick |
| `Boot::observe` | the boot hart's handle (`Hart0`, `!Send`): its cycle and instruction counters, the L1 cache's counters and the PC sampler on TIMG1 T0, once |

`start_epoch()` starts a new epoch of the longest polls and the extremes.
Each counter has one writer; a reader in thread mode on either hart takes a
counter whole or yields and reads again. Counts wrap at 2^32: a reader
takes differences.

## Limits

The application runs on hart 0. The AES engine runs the accelerator in
Typical mode without DMA, one synchronous block at a time; the station
computes RFC 3394 key unwrap on it. One station and one IP service per
image. `println!("ipv4 {}/{}", Ipv4(address), prefix)` writes a typed line
(`oer_diagnostics::typed_line!`): the format's static pieces and the
values, which the host prints; the target formats nothing. A hole is a bare
`{}`; `Hex` and `Ipv4` choose a value's text, and an error that describes
itself as a diagnostic path is the last value.
It uses the early writer, which waits for room within its budget per line.

```console
cargo test -p oer-espressif
cargo clippy -p oer-espressif --all-targets
cargo clippy -p oer-espressif --target riscv32imafc-unknown-none-elf --features esp32s31,wifi,console,hart1,udp
RUSTFLAGS="--cfg oer_trace" CARGO_TARGET_DIR=target/instrumented/oer-espressif \
  cargo clippy -p oer-espressif --target riscv32imafc-unknown-none-elf --features esp32s31,wifi,console,hart1,observe,udp
```
