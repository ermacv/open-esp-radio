# ESP32-S31 IEEE 802.11 MAC primitives

`oer-esp32s31-ieee80211-mac` provides source-owned Wi-Fi MAC building blocks
over the ESP32-S31 HAL: initialization and receive policy, EDCA and channel
state, interrupts, RX/TX DMA ownership, aggregate formatting, local/TSF time
relations, and transmission cost estimates and work receipts. Production
builds use no ESP-IDF ABI, vendor archive, allocator or executor. The explicit
`test-support` feature enables synthetic completion evidence and DMA models.

Portable peer airtime fairness belongs to
[`oer-ieee80211-upper-mac::airtime`](../../../../protocols/ieee80211/upper-mac/README.md).
The chip MAC exposes `tx_cost` and `tx_work`; it does not select peers or own
the fairness ledger. Integration tests under `src/airtime_tests` combine the
upper-MAC scheduler with `MacTxWork`, `TxCost` and network `TxQueues`, including
retry charges, burst limits and packet ownership. These tests use the existing
`oer-ieee80211-upper-mac` and `oer-embassy-net` dev-dependencies; neither is a
normal dependency of this package.
