# ESP32-S31 Wi-Fi station composition

This crate specializes portable STA policy for ESP32-S31 while remaining
independent of Embassy, a network stack, board allocation and HIL protocols.
It owns chip transaction ordering and typed resource transitions, but not the
executor mechanisms used to wait for them.

Module map:

- `hardware/channel`: persistent PHY/channel owner retained across scan and reconnect;
- `hardware/control`: concrete receive and control hardware operations;
- `association`: PHY/rate/power plan derived from a selected candidate;
- `peer`: associated-peer WMM/HT/HE/rate-control programming;
- `peer_policy`: value-only Scan/Association/WMM/HE plan consumed by `peer`;
- `control_tx`, `scan_tx`, `single_mpdu_tx`: bounded management/data TX
  transactions and their entropy, power, time and completion ports;
- `tx_epoch`: unique pre-connected control-TX ownership state;
- `scan`: the portable scan values and `StaScanPort`, re-exported; the
  channel-visit transaction is `oer-ieee80211-sta-service`'s `StaScanBackend`;
- `join`: executor-independent RX, TX and observation contracts for the
  ESP32-S31 Authentication/Association adapter;
- `attempt`: the chip parts of an attempt (selected channel, installed key
  slots, phase reports); the attempt values are `oer_ieee80211_sta::attempt`
  and the transaction is `oer-ieee80211-sta-service`'s `StaAttempt`;
- `wpa2`: chip handshake and atomic hardware-key publication ports;
- `connected`, `connected_rx`, `connected_control`: association-scoped data,
  security, receive admission and control ownership;
- `profile`: local capability advertisement and channel lowering;
- `modem_sleep`, `connection_coex`: re-exports of the power manager of
  `oer-ieee80211-sta` and of the reconnect coexistence requests of
  `oer-espressif-ieee80211-policy`;
- `standalone_esp_now_rx`: ESP-NOW reception while no station is connected;
- `ftm`, `hardware/beacon_monitor`: bounded hardware admission frontiers whose
  physical activation limitations are stated in [FEATURES.md](../../../../hardware/esp32s31/driver/ieee80211/FEATURES.md).

`crates/protocols/ieee80211/sta` owns portable MLME, reconnect and association
preference policy. The chip profile determines eligible modes and lowers the
selected mode to S31 PHY parameters.
`crates/runtime/esp32s31/ieee80211` supplies concrete timers, DMA/TX
owners, IRQ wakeups and task/network composition. HIL may observe those public
boundaries but must not implement a second driver path.
