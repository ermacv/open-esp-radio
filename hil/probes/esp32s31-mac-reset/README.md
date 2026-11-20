# Case C's sender

`oer-hil-probes-esp32s31-mac-reset` sends case C's frames (`tasks/154`,
`159`): in rounds, one frame of each case a round, each awaited:

1. QoS Data, A-MSDU Present, two short subframes;
2. the same with one subframe;
3. the control: case 1's bytes with A-MSDU Present clear;
4. QoS Data, A-MSDU Present, two long subframes: longer than a buffer;
5. and 6. non-QoS Data of 700 and 1 500 octets whose body carries the
   counter every 64 octets.

The layout is `src/case_c.rs`, which the receiver (`esp32s31-rx-ring`)
includes. `--features ccmp` sends cell E's (`tasks/161`) QoS cases sealed
under CCMP in software instead.

The probe holds the radio domain itself and brings Wi-Fi up by the public
operations of the PHY crate and the HAL, in the radio system's order
(`src/radio.rs`, shared with the receiver):
- the PHY's registration, coexistence, power;
- the clocks (which give the block's first reset its proof,
  `MacFirstReset`), the PHY client and the tracking due;
- RX enable and channel 13 at 20 MHz.

It then starts the MAC on the receive ring's atoms, over a small ring of
its own (8 descriptors and R, 256-octet buffers):
- the first reset, the cold preparation with the walker's registers lent,
  the channel;
- the core opened by the preparation's proof, the cold publication, the
  walker, the MAC's receive.

It learns its peer from the first frame addressed to its station, served
from that ring with every unit copied in place. Then it sends the rounds,
12 000 of them. The rounds' length is the `End` record's.

| Record | Values |
|---|---|
| 1 up | |
| 4 end | the rounds' microseconds |
| 5 failed | phase, index |
| 13 peer | the peer's address, its first two octets and its last four |
| 15 sent (case C) | the case's tag; acknowledged by the frame before (none, then each case); ACK timeouts by the frame before; other ends |
| 16 sent (cell E) | the case's tag, its protected length, acknowledged, ACK timeouts, other ends |

The console prints `MARK <n>` (70 the peer, 90 and 91 the rounds), then one
line a record, `R <kind> <values…>`, then `OER-RESULT: pass case-c-sender`
unless a failure was recorded.

## Running

Build with `cargo fw probe oer-hil-probes-esp32s31-mac-reset` before the
lease. The injector (`tools/lab/peers/esp32c5-wifi-injector`) teaches the
sender its peer: frames to the sender's station from the receiver's address
(`tasks/167`). The receiver is `esp32s31-rx-ring`; see its README for the
three boards' lease.

Every owner stays held until the leased host reset; the probe returns no
reusable radio.
