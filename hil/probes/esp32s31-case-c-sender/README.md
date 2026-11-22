# Case C's sender

`oer-hil-probes-esp32s31-case-c-sender` sends case C's frames (`tasks/154`,
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

The probe speaks the HIL protocol (`console = "hil"`): its console serves
the agent (`hil/agent`) and the sender's requests
(`oer-hil-services-radio-pair`); the cases and the verdict are
[`hil/scenarios/radio-pair`](../../scenarios/radio-pair/README.md)'s.

| Request | What the probe does |
|---|---|
| `snapshot-cold`, `bring-up`, `snapshot-up`, `receive-counters`, `stop` | as the receiver's ([`esp32s31-rx-ring`](../esp32s31-rx-ring/README.md)) |
| `listen` | drains its ring, then learns as its peer the sender of the first frame to its station that comes after this answer |
| `peer` | serves its ring once and answers the peer it heard, or that it heard none yet, without waiting |
| `round` | the given number of case C's rounds to the peer, one frame of each case a round, each awaited; per case, by the frame before it: acknowledged, ACK timeouts, other ends, and the rounds' microseconds |

With `--features ccmp`, `round` sends cell E's protected QoS cases instead;
each case's ends are its first column, and a seal the protocol layer refused
is answered as a failure.
