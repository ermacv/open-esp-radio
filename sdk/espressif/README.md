# oer-espressif

The Espressif family's application entry. An image selects its chip by
feature (`esp32s31`, exactly one) and the capabilities it uses (`wifi`,
`console`),
takes the peripherals it needs once and builds drivers and services with
defaults. The library places their interrupts, executors and service tasks
and applies record and reset on the image's behalf.

```rust
oer_espressif::entry!(boot);

fn boot(p: Peripherals, mut boot: Boot) -> Result<impl Started, Error> {
    let aes = Aes::start(p.aes, &mut boot)?;
    let config = wifi::Config::new(*b"DE", 6, 10)?;
    let (station, device) = Station::start(p.wifi, &aes, config, &mut boot)?;
    let (net, boot) = Net::start(device, NetConfig::Dhcpv4, boot)?;
    Ok(boot.run(app(station, net)))
}

async fn app(mut station: Station, net: Net) -> AppResult {
    station.connect(wifi::Profile::wpa2(b"ssid", b"passphrase")?).await?;
    let ipv4 = net.wait_ipv4().await?;
    // sockets on `net.stack()`
}
```

`firmware/apps/wifi-station` is the complete example.

## Boot and run

`entry!(boot)` declares the image's entry. After stage-two adoption the
library reports the previous boot's retained faults (what it read, hart 0's
reset cause, the previous panic and the previous CPU exception, stays
readable through `Boot::evidence`) and the task stack's watchpoint, then
calls `boot` once
on hart 0, before any executor and with interrupts disabled. `boot` moves
peripherals into drivers and services through the boot context (`Boot`):
their interrupt bindings, executors and service tasks are made there and
nowhere else. `Boot::run` ends the boot with the application's future. The
library then installs the system timer (the radio executor writes it when
a station started, the application executor otherwise), enables interrupts
and runs the application executor in thread mode with its service tasks
(`SdkTasks`) and the image's two futures as the executor's roots: the IP
service, at the image's pool sizes, and the application. They stay in the
frame of the library's run, which never returns, and are tasks of the
executor after `SdkTasks`; no static slot names their types, so an image
declares no task set and needs no unstable feature. `boot` returns `impl
Started`, which names both future types: `Boot::run` makes it, and
`wifi::Net::start` returns the boot with its IP service.

## Defaults and settings

Every constructor works with the library's defaults; an image that needs
other values states them in its own `boot`, the same way an example and
a HIL image of one function build it:

| Setting | Default | Override |
| --- | --- | --- |
| Reconnect after a lost association | every 1 s (`wifi::RECONNECT`) | `wifi::Config::with_reconnect(interval)`, `without_reconnect()` |
| Drain at a stop after a failure | 2 s (`wifi::DRAIN`) | `wifi::Config::with_drain(deadline)` |
| AES client seats | 4 (`AES_CLIENTS`) | `static SEATS: AesSeats<C>`, `Aes::start_with(p.aes, &SEATS, &mut boot)` |
| IP pools (receive, transmit, general frames) | 8, 8, 8 (`wifi::RX`, `TX`, `GENERAL`) | `static POOLS: wifi::Pools<RX, TX, GENERAL>`, `Net::start_with(device, &POOLS, config, boot)` |

A zero interval or deadline is refused (`ConfigError`). The seats' count
(1 to 32) is the storage's, not a type's: `Aes` and `AesClient` are the same
for every count. The pools' sizes are types (`Net<RX, TX>`, `Stack<RX, TX>`),
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

The radio entries exist with the `wifi` feature, the console's with
`console`; an entry no driver binds routes no source. Service tasks on the application executor: the RTC
watchdog feed, the AES service, the station runner and the IP service. The
radio executor runs the radio actor and the MAC driver.

| Default | Value |
| --- | --- |
| AES client seats | 4 (`AES_CLIENTS`); key data per request 512 bytes (`AES_CAPACITY`) |
| IP packet pools | 8 receive, 8 transmit credits, 8 general, 1514 bytes each |
| IP waiters | 4 tasks waiting at once |
| Station reconnect | every second after a lost association |
| `println!` line | `{}` holes only; 8 segments (holes and non-empty pieces), 7 value words, 6 pieces of 96 bytes together, all checked when it compiles; an error's path, the last value, past the line's room is marked `line-truncated` |

## Services

`Aes::start` makes the accelerator the shared service: one engine owner,
clients from `Aes::client` (in the boot or in a task). A request moves into
the client's seat and its response comes back by value; the service runs
one request at a time, the next pending client after the last served. A
client is also a `KeyUnwrap` engine; the Wi-Fi station takes one.

`wifi::Station::start` takes the radio, the entropy source and an AES
client. Its configuration names one channel (`wifi::Config::new`) or every
2.4 GHz channel the country permits, scanned at each connect
(`wifi::Config::all_channels`). `connect(profile)`
ends with the result of that connection attempt: `Connected`, or a
`ConnectError` with the station's own cause (`Failed(DisconnectReason)` names
`NotFound`, `AuthenticationFailed`, `AssociationFailed`, `HandshakeFailed`
and the others; none is reported as a wrong password). Once a connect
linked, the station keeps the network and rejoins it after a lost
association; a connect that ended without a link leaves the station idle
for the application's next connect, with the same or another profile. `wifi::Net::start`
runs the existing IP service (`oer-embassy-net`) over the station's device;
`wait_ipv4` ends with the configuration over an up link and keeps waiting
across a link loss, `wait_link_down` ends on a loss or a new link epoch,
and both end with `NetError::Stopped` or `Failed` when the service ends.
Sockets open on `net.stack()` (`embassy_net`, re-exported). The
station lends received frames to the IP service in place; when the station
stops (after a failure, within the drain deadline) the IP service stops first and returns
every buffer it holds. A station whose device no `Net::start` took tells
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
| A CPU exception | all owners stay where they are | the runtime records the trap frame before it stops the other hart, the library's hook (`oer-esp32s31-compose::exception`) what the interrupt controller showed after the stop; reset. The next boot prints both and serves them in `Boot::evidence` (`previous_exception`) |
| The IP service is refused its context | the service keeps its owners | record and reset |
| `connect` refused or failed, `wait_ipv4` timed out by the caller, a TCP error | unchanged | handled `Result` |

## Cancellation

| Dropped | Effect |
| --- | --- |
| `Station::connect` while its profile is prepared | the derivation and the passphrase are erased; nothing was sent |
| `Station::connect` after admission | the attempt continues; the next connect waits for its end; `disconnect` leaves the network |
| `AesClient::execute` before the service took the request | the request is withdrawn and erased |
| `AesClient::execute` while the service runs it | the service finishes, erases the response; the client's next request waits for that |
| An `AesClient::execute` or key unwrap that ended without its response (polled outside the service's context, or leaked) | its request or response stays in the seat under its request number; the client's next request withdraws or erases it, or waits for it while it runs, and never takes its response |
| An `AesClient` with a running request | its seat is freed once the service finished it; in the boot, before the executor runs, a dropped client keeps its seat |
| `Net::wait_ipv4` or `wait_link_down` | it frees its waiter slot; dropped outside the application's context, its waker stays until the next change takes it |

## Limits

The application runs on hart 0. The AES service computes RFC 3394 key
unwrap only, on the CPU without DMA. One station and one IP service per
image. `println!("ipv4 {}/{}", Ipv4(address), prefix)` writes a typed line
(`oer_diagnostics::typed_line!`): the format's static pieces and the
values, which the host prints; the target formats nothing. A hole is a bare
`{}`; `Hex` and `Ipv4` choose a value's text, and an error that describes
itself as a diagnostic path is the last value.
It uses the early writer, which waits for room within its budget per line.

```console
cargo test -p oer-espressif
cargo clippy -p oer-espressif --all-targets
cargo clippy -p oer-espressif --target riscv32imafc-unknown-none-elf --features esp32s31,wifi,console
```
