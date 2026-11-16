# The Wi-Fi MAC's block reset mid-run, and how long a stop takes

`oer-hil-probes-esp32s31-mac-reset` answers four questions of the stop's
last resort (`tasks/084` of the notes) on a board under receive load. The
first part, written now, is receive; the transmit part (a frame queued, a
stop with a frame and with an aggregate on the air, a second run with an
acknowledged frame) follows in the same image.

1. **A pulse mid-run.** With the receive walker live on a busy channel,
   `RST_WIFIMAC` is asserted and released (`WifiClocked::reset_mac`). After
   it: the MAC's activity field (`channel_active_state`), the MAC's raw
   interrupt image and the power bank's latched causes, the walker's
   registers (`receive_dma_snapshot`), and a guard: every buffer of the
   ring is painted while its descriptors stay published to the walker, and
   read back after 50 ms. A changed word is a write of a bus master that
   went on.
2. **A second run after it.** The walker's stop, the owner's cold
   preparation alone (`initialize_wifi_mac`, which begins with the pulse
   and the cold handshake), the ring rebuilt (`prepare_halted`,
   `reopen_mac_core`) and an ordinary start, with no new PHY calibration;
   the units received in the next 20 ms.
3. **The key table.** A station key is installed before each pulse; its
   entry's validity after the pulse, and after the second run's cold
   preparation.
4. **How long a stop takes.** An ordinary stop under receive (the channel
   stop requested, the walker stopped, the activity field polled until it
   is clear, at most 10 ms), 1 000 times, each timed by `mcycle`.

Each pulse lands after a delay drawn from 0 to 2 ms after a 20 ms receive:
spread over a received frame's time.

## The order

The probe holds the radio domain itself, as the hardware companion of
`hil/probes/esp32s31-radio` does, and brings Wi-Fi up by the public
operations of the PHY crate and the HAL that `Esp32s31Radio`
(`chips/esp32s31/radio/system/src/esp32s31.rs`) calls. It does not use
`Esp32s31Radio`, so its second answer is about this order as the probe
writes it:

| Step | `Esp32s31Radio` | This probe |
|---|---|---|
| prepare | `register_concurrent_phy`, through `owned_operation::run` and `finish_registration` | `register_concurrent_phy`, awaited directly; the outcome read |
| enter | `enter_coex`, `WifiCold::power_up`, `WifiPowered::enable_clocks`, `WifiClocked::set_initialized`, `phy_ready`, `client::<WifiPhyMembership>`, `join_wifi` | the same, in that order; a failure ends the session instead of rolling back |
| track: tracking | `maintain_concurrent_phy` when the join says tracking is due | the same |
| track: RX | `set_wifi_rx(true)` | the same |
| track: channel | `lower_ghz2_4_channel`, `detach(Operation::Channel)`, `channel_transaction`, `select_concurrent_wifi_channel` | the same; channel 13, 20 MHz lowered to its primary number and CBW 0 by hand (the radio system's function is crate-private) |
| track: power | `phy_state().tx_target_power_profile().with_maximum_quarter_dbm(ceiling)` | the same, the ceiling as the RX-chain probe computes it (Germany, the board's limits) |
| track: MAC | `MacSources` (the four coexistence events, the power profile, the slow-clock calibration, the delay entropy), `cold_mac_hal`, `initialize_wifi_mac` | `Sources`, the same values (no slow-clock calibration, as the RX-chain probe answers), `cold_mac_hal`, `initialize_wifi_mac` |
| running | `into_running` | the same; `from_running` and `reset_mac` around each pulse |

## Records

The console prints one line a record, `R <kind> <values…>`, then
`OER-RESULT: pass mac-reset` or `fail mac-reset`.

| Kind | Values |
|---|---|
| 1 up | units received in the first 20 ms |
| 2 stop | index, units before it, cycles to the stop request, cycles to the walker stopped, cycles to the activity field clear (or the 10 ms bound), the field then, the MAC's raw interrupts, the walker's flags and next descriptor |
| 3 pulse | index, units before it, delay (µs), the pulse's cycle, its length in cycles, the walker's flags and next descriptor before, its flags, next and last descriptor after, the activity field after, the MAC's raw interrupts after, the power bank's causes after, the key entry valid after the pulse, the guard's changed words, units in the second run, the key entry valid after the second run's cold preparation |
| 4 end | cycles and microseconds over the series: the `mcycle` rate |
| 5 failed | the phase and the index where the session stopped |

## Running

Build with `cargo fw build esp32s31-mac-reset --diagnostic` before the
lease. Under a board and air lease, with the stand's access point on
channel 13 and the load generator sending to it:

```console
cargo stand --owner wifi lease --board s31-a --air exclusive -- \
  cargo fw run esp32s31-mac-reset --board s31-a --timeout 120s
```

Every owner stays held until the leased host reset; the probe returns no
reusable radio.
