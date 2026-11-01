# oer-espressif

The Espressif family's application entry. An image selects its chip by
feature (`esp32s31`, exactly one) and the capabilities it uses (`wifi`),
takes the peripherals it needs once and builds drivers and services with
defaults. The library places their interrupts, executors and service tasks
and applies record and reset on the image's behalf.

```rust
oer_espressif::main!(boot);

fn boot(p: Peripherals, mut boot: Boot) -> Result<Started<impl Future<Output = AppResult>>, Error> {
    let aes = Aes::start(p.aes, &mut boot)?;
    let config = wifi::Config::new(*b"DE", 6, 10)?;
    let (station, device) = Station::start(p.wifi, &aes, config, &mut boot)?;
    let net = Net::start(device, NetConfig::Dhcpv4, &mut boot)?;
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

`main!(boot)` declares the image's entry. After stage-two adoption the
library reports the previous boot's retained faults, then calls `boot` once
on hart 0, before any executor and with interrupts disabled. `boot` moves
peripherals into drivers and services through the boot context (`Boot`):
their interrupt bindings, executors and service tasks are made there and
nowhere else. `Boot::run` ends the boot with the application's future. The
library then installs the system timer (the radio executor writes it when
a station started, the application executor otherwise), enables interrupts
and runs the application executor in thread mode with its service tasks
(`SdkTasks`) and the image's task (`main!`'s set extends `SdkTasks`).

## Default layout (ESP32-S31)

| Context | Level | Entries |
| --- | --- | --- |
| application | 1 | executor wake (line 40), its alarm (system timer comparator 1, line 41) |
| radio executor | 2 | SAR/PWDET pilot (line 16), executor (line 17) |
| radio | 3 | radio alarm (comparator 0, line 19), Wi-Fi MAC and doorbell (line 21) |

The radio entries exist with the `wifi` feature; an entry no driver binds
routes no source. Service tasks on the application executor: the RTC
watchdog feed, the AES service, the station runner and the IP service. The
radio executor runs the radio actor and the MAC driver.

| Default | Value |
| --- | --- |
| AES client seats | 4 (`AES_CLIENTS`); key data per request 512 bytes (`AES_CAPACITY`) |
| IP packet pools | 8 receive, 8 transmit credits, 8 general, 1514 bytes each |
| IP waiters | 4 tasks waiting at once |
| Station reconnect | every second after a lost association |
| `println!` line | 160 bytes, cut and marked beyond |

## Services

`Aes::start` makes the accelerator the shared service: one engine owner,
clients from `Aes::client` (in the boot or in a task). A request moves into
the client's seat and its response comes back by value; the service runs
one request at a time, the next pending client after the last served. A
client is also a `KeyUnwrap` engine; the Wi-Fi station takes one.

`wifi::Station::start` takes the radio and an AES client. `connect(profile)`
ends with the result of that connection attempt: `Connected`, or a
`ConnectError` with the station's own cause (`Failed(DisconnectReason)` names
`NotFound`, `AuthenticationFailed`, `AssociationFailed`, `HandshakeFailed`
and the others; none is reported as a wrong password). The station keeps
the network and rejoins it after a lost association. `wifi::Net::start`
runs the existing IP service (`oer-embassy-net`) over the station's device;
`wait_ipv4` ends with the configuration over an up link and keeps waiting
across a link loss, `wait_link_down` ends on a loss or a new link epoch,
and both end with `NetError::Stopped` or `Failed` when the service ends.
Sockets open on `net.stack()` (`embassy_net`, re-exported). The
station lends received frames to the IP service in place; when the station
stops (after a failure, within 2 s) the IP service stops first and returns
every buffer it holds. A station whose device no `Net::start` took tells
the station that no network takes its frames.

## Failures

| When | Fate of the owners | Result |
| --- | --- | --- |
| A boot step refuses a value (configuration, profile) before binding | the peripheral was moved into the call and is dropped | `Error`, which the library records and resets on |
| A boot step fails after a binding or a claimed storage (`Storage`, `Entropy`, `Net`) | bound owners stay bound, claimed storage stays claimed | `Error`, record and reset |
| `boot` returns `Err`, or the application's future returns `Err` | all owners stay where they are | record and reset |
| The station runner exits other than an ordinary stop (failed, poisoned, quarantined, retirement or release failure, abandoned, initialization) | the runner keeps every owner | record and reset |
| A fatal MAC assertion, a PHY transaction fault, nonce exhaustion | owners quarantined by their fault owner | record and reset |
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
image. `println!` uses the early writer, which waits for room within its
budget per line.

```console
cargo test -p oer-espressif
cargo clippy -p oer-espressif --all-targets
cargo clippy -p oer-espressif --target riscv32imafc-unknown-none-elf --features esp32s31,wifi
```
