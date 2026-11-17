# The Wi-Fi MAC's block reset mid-run, and how long a stop takes

`oer-hil-probes-esp32s31-mac-reset` answers four questions of the stop's
last resort (`tasks/084` of the notes) on a board under receive load. It
answers them for receive; it has no transmit case.

1. **A pulse mid-run.** With the receive walker live on a busy channel,
   `RST_WIFIMAC` is asserted and released (`WifiClocked::reset_mac`). After
   it: the MAC's activity field (`channel_active_state`), the MAC's raw
   interrupt image and the power bank's latched causes, the walker's
   registers (`receive_dma_snapshot`); then, in the vendor's order
   (`wifi_reset_mac`), the walker stopped at once. Whether the reset state
   writes is the hold case's question (kind 7): its guard paints every
   buffer of the ring, and a changed word is a write of a bus master that
   went on.
2. **A second run after it.** The walker's stop, the owner's cold
   preparation alone (`initialize_wifi_mac`, which begins with the pulse
   and the cold handshake), the channel selected again (no new calibration), the ring rebuilt and
   published once more after the MAC's gates are open (`prepare_halted`,
   `reopen_mac_core`, `into_halted`, `prepare_halted`: a BASE written into
   the gates the cold preparation holds is not kept) and the port's start;
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

Each receive start, the first, after an ordinary stop and after a pulse's
cold preparation, follows the removed port's `port_start` (the knowledge
table's section 2.3): the ring prepared with BASE on its reserved
descriptor, the open promiscuous receive, the interrupt epoch (both banks
masked and cleared, the station mask published, the station TBTT enabled;
no line of this image takes a Wi-Fi source), the walker enabled, the MAC's
runtime released. After the open receive each start applies the station's scan policy as
the backend does (`StaScanRxPolicyHardware`: the normal station transition,
which clears queue three's `AUTOACK_DISABLE`, then the scan's policy with
the broadcast BSSID): a frame addressed to the station's own address is
taken and acknowledged whatever its BSSID, with no key.

## Records

While the session runs the console prints `MARK <n>` at each phase (10 the baseline, 20/21 case 0, 30/31 case 1, 50/51 case 3, 60 the cases' end; the second of each pair opens its window), so that the host can place a peer's lines beside them. Then it prints one line a record, `R <kind> <values…>`, then
`OER-RESULT: pass mac-reset` or `fail mac-reset`.

| Kind | Values |
|---|---|
| 1 up | units received in the first 20 ms |
| 2 stop | index, units before it, cycles to the stop request, cycles to the walker stopped, cycles to the activity field clear (or the 10 ms bound), the field then, the MAC's raw interrupts, the walker's flags and next descriptor, the MAC's MPDU and data-success counts over the window (high and low half), its FCS-error and buffer-full counts over it |
| 3 pulse | index, units before it, delay (µs), the pulse's cycle, its length in cycles, the walker's flags and next descriptor before, its flags, next, last descriptor and base after, the activity field after, the MAC's raw interrupts after, the power bank's causes after, the key entry valid after the pulse, units in the second run, the key entry valid after the second run's cold preparation, the MPDU and data-success counts over the window before the pulse and over the second run's |
| 6 quiet | a pulse with the walker stopped first: index, the walker's flags and base before, its flags, next, last and base after, its flags and base after the cold preparation |
| 7 hold | a pulse under load left without the cold preparation for 20 ms: index, units before it, the walker's flags, next and base at the pulse, its flags, next, last and base after the 20 ms, the activity field then, the MPDU and data-success counts over the 20 ms, the FCS-error and buffer-full counts over them, the guard's changed words in the ring |
| 8 case | a recovery case of `tasks/097` over a 1 s window: the case (`0xb` the baseline, 0 an ordinary stop and the cold preparation again, 1 the pulse mid-run then RX enable, channel, cold preparation and start, 3 the client's leave, join and track with the registration kept), units, the MPDU and data-success counts, the FCS-error and buffer-full counts, the walker's next descriptor |
| 4 end | cycles and microseconds over the series: the `mcycle` rate |
| 5 failed | the phase and the index where the session stopped |

The walker's flags are bit 0 `RX_CONTROL.WALKER_ENABLE`, bit 1 a reload pending, bit 2 the upper next-descriptor field nonzero.

## Running

Build with `cargo fw build esp32s31-mac-reset --diagnostic` before the
lease. The load is the stand's injector peer (`tools/lab/peers/esp32c5-wifi-injector`)
sending to the probe board's station address on channel 13, both boards
under one lease (the run's script holds the peer's lines beside the
probe's console):

```console
cargo stand --owner wifi lease --board s31-a --board c5-a --air exclusive -- sh -c '
  cargo stand board send c5-a --timeout 30s SYNC "MODE 13 20 6m" &&
  { cargo stand board send c5-a --timeout 90s "TX 60000 2500 1500 <s31-a station> <c5-a> <c5-a> 0" &
    cargo fw run esp32s31-mac-reset --board s31-a --timeout 180s; wait; }'
```

Every owner stays held until the leased host reset; the probe returns no
reusable radio.
