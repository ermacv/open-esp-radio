# IEEE 802.15.4 MAC engine

`oer-espressif-ieee802154-engine` is the MAC state machine and public API layer of the
public ESP-IDF IEEE 802.15.4 driver (`esp_ieee802154_dev.c` and its
helpers), ported once for every supported chip.

| Module | Contents |
| --- | --- |
| `engine` | Operation state machine, interrupt handler, receive ring and DMA frame buffers, statistics |
| `pib` | The driver's PIB and its publication to the MAC |
| `ll` | The `Ieee802154LowLevel` interface (the driver's `ieee802154_ll_*` accessors, modem ETM steps and timer commands) and the hardware helpers generic over it; `ll::model` is a register model for the crate's own tests (`cfg(test)`) |
| `capabilities` | The portable capabilities a radio over the engine reports (ESP-IDF's OpenThread port defaults) and the table a chip builds from its transmit-power level set |
| `types`, `channel`, `coex`, `tx_power` | The chip-neutral values the interface exchanges, the 2.4 GHz channel/frequency-code map, the software-coexistence priorities and the vendor transmit-power resolution |

Each chip's HAL implements `Ieee802154LowLevel` over its own PAC and converts
between these values and its register types; the ESP32-S31 implementation is
`oer_esp32s31_hal::ieee802154::ll::Ieee802154MacOwners`. What differs between
chips stays with the chip: register geometry, the transmit-on delays and the
rest of the MAC lifecycle, the transmit-power level set, the coexistence
table, the diagnostic counters a chip's LL adds and the memory the MAC DMA
reaches, which the caller passes to
`Ieee802154Engine::buffers_dma_visible` before any hardware is touched.

The engine names no executor, interrupt route or chip. A runtime holds it
with the chip's MAC owners in one critical section and calls its operation
entries and interrupt handler. Its accessor sequence was compared on the host
with the compiled vendor driver.

## Bring-up and teardown order

No runtime holds the engine now; the former ESP32-S31 runtime brought the
radio up in the order of ESP-IDF's `esp_ieee802154_enable`:

1. the common radio power and the IEEE 802.15.4 module clocks;
2. the shared PHY domain: registered, woken or joined as the
   [first PHY client](../../esp32s31/phy/README.md#first-and-last-phy-client);
3. the shared BTBB reference and the transmit-on delay;
4. the MAC reset and the masked foundation;
5. the interrupt owner;
6. the engine's `mac_init`, under the runtime's critical section;
7. the CPU route of modem interrupt source 132.

Stop reversed every step. A failure before shared state was touched rolled
back; a started PHY or clock transaction that failed was fail-stop. The
runtime was a process singleton: source 132 had one handler.

## Enhanced ACK and transmit security

The engine calls its environment for each received 2015 frame that requests
an ACK and at each transmit SFD; no generator answers it now. The former
radio role answered as ESP-IDF's OpenThread
port does (`esp_openthread_radio.c` at ESP-IDF
[`7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/openthread/src/port/esp_openthread_radio.c)),
and a new generator keeps these rules:

- IE order (`ot_radio_enh_ack_generator`):
  1. the CSL IE template, when the receiver runs CSL (a nonzero period);
  2. the Link Metrics enhanced-ACK probing IE, when the frame's source is a
     probing initiator (`otLinkMetricsEnhAckGenData`);
  3. the configured header IEs, at most `ENHANCED_ACK_IE_CAPACITY` bytes.
- At the transmit SFD (`ot_radio_transmit_sfd_done`), with a nonzero CSL
  period, the CSL IE of the frame or enhanced ACK going out gets the period
  and the phase from the radio clock at that moment to the next sample time
  (`oer_ieee802154::mac::csl`); with a zero period nothing is written. A
  transmitted frame's Time IE gets the time-sync sequence and the network
  time (the radio clock plus the network time offset,
  `oer_ieee802154::mac::time_sync`); an enhanced ACK never carries one.
- A retransmission keeps its frame counter and key index, except that a CSL
  receiver takes a new frame counter on every retry: the CSL IE changes
  with each attempt.
- A secured ACK uses the keys of the interface the received frame matched
  (`mpf_index`; `enh_ack_set_security_addr_and_key`). The ACK takes the next
  frame counter of those keys and the frame's key index, which the port
  keeps as `s_ack_frame_counter` and `s_ack_key_id`. When the ACK must be
  secured, a frame that no single interface claims, or an interface without
  keys, gets no ACK.
- The generator also refuses a received frame it cannot parse and an ACK it
  cannot generate (`generate_enhanced_ack` fails) or secure. A refused ACK
  leaves the engine to deliver the frame unacknowledged.
