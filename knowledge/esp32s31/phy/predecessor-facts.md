# Facts recorded by the predecessor

Carried from the predecessor's issue discussions as written; unverified unless a line says otherwise.

- **153, HE-TB.** Unverified: the final HE-TB PHY vector/doorbell transition (`HeTriggerBasedTxConfig::with_runtime_handoff_window_micros`, in snap `crates/esp32s31/i/mac/src/tx.rs`). Basic Trigger parsing, AID checks, response deadlines and queue/MPLEN/BSR preparation exist.
- **121, no vendor blob in S31 images.** `esp-rom-sys` at the fork pin links `libs/esp32s31/*.a`, which holds only the empty placeholder `unused`; `esp-phy`/`esp-radio` are absent from the lock. The only code without sources is the ROM.
- **420 → `knowledge/esp32s31/phy/` or the PAC rustdoc.** "SOURCE: S31 rev0 ROM — `phy_i2c_rc_cal_set` calls `phy_i2c_writeReg_Mask` three times with block 107 (0x6b), host 1, registers 17, 15 and 19." Unverified whether `pac/src/phy/i2c.rs` already names these three registers.
- **427 — C5/S31 PHY offset equivalence.** (b, no; low value). "The C5 copies of `adctrig`, `force_rx_gain`, `rx_max_gain`, `rx_init_gain` access the same offsets from 0x600a0000 that the S31 copies access from 0x20100000" (two register-access runs and a manual diff, 2026-10-10). Not evidence for an S31 address (model rule); useful only as a C5 research hint.
- **771i, crates/esp32s31/phy.** `target_port.rs:1396-1418` `switch_channel_hal_with_mac_restart`: on a `select_channel_hal` error or idle timeout it returns without `restart_mac`, leaving the MAC stopped with TX blocked. Rule: after `request_mac_stop`, always attempt `restart_mac` before `RetuneFault`.
- **794.** Features with no enabler: PHY `lifecycle-fault-injection`, `registration-diagnostics`, `tracking-diagnostics`; executor `idle-accounting` (returns with core-1 sampling, h). Crates without a consumer: `oer-radio-permit-pool` (crates/radio/permit-pool), `oer-time-embassy`.
- **834.** The `phy-tracking-reference` hil-vendor image and the `docs/phy/README.md` comparison description are not in M; they are needed when the PHY cross-check returns before R1. Nothing checks `provenance.toml` any more.
