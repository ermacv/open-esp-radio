#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::todo, clippy::unimplemented)]

//! Role-neutral ESP32-S31 Wi-Fi device composition.
//!
//! This layer joins shared RF/PHY and Wi-Fi hardware ownership before a
//! station, access point or standalone monitor role is materialized. It does
//! not own MLME policy, an executor, a network stack or board resources.
//!
//! SPP A-MSDU is interface-scoped on S31. `hal_crypto_enable` writes one
//! control for STA/AP/NAN, not one per key-table entry. The backend declares
//! `security::spp::Interface`; an association fixes that field before PTK,
//! and its PTK/GTK retain the association's decision. The current AP does not
//! advertise SPP and refuses clients requiring it. A future SPP AP must put
//! every client in Negotiated mode; mixed modes cannot share this hardware
//! interface. Changing the mode requires removal of every previous key.
//!
//! SOURCE(esp32s31): complete `libpp[hal_crypto.o]::hal_crypto_enable`
//! selects the interface and applies its fourth argument at +0x2e..+0x34.
//! `libnet80211[ieee80211_supplicant.o]::ieee80211_get_spp` supplies the
//! association decision through `libpp[if_hwctrl.o]::ic_set_key`
//! and `libpp[wdev.o]::wDev_Insert_KeyEntry` (Wi-Fi library
//! `f856b5db55cb18e2d1826044430f3f4089e47cf9`).
//!
//! The station has one physical GTK slot. Eviction destroys the installed
//! owner; a saved RAM key cannot satisfy portable `select_group_key`.
//! STA accepts all four wire KeyIDs in slot 1. AP group slots use `1 + key_id`
//! and exclude zero because it would overlap that STA slot. These are backend
//! allocation constraints, not restrictions on the portable RSN wire ID.
//! The shared conformance suite does not by itself prove S31's hardware RX cut.
//! HE20 and hardware insertion of HE Control are likewise backend capabilities;
//! its DMA image omits the four inserted bytes and private metadata accounts
//! for their on-air length.
//! Aggregate retry adapters retain the MAC reception-clock generation and
//! reject another generation before consuming a completion. DMA encoding
//! applies S31's density rounding after portable delimiter geometry.
//! The source-frontier FTM consumer refuses admission before publication;
//! portable session ownership does not establish hardware timestamp capture.
//! SOURCE(esp32s31): `libpp[hal_utilities.o]::rutype2str` names RU types
//! zero through three in this HE20 backend profile.

#[cfg(test)]
extern crate std;

// Host driver fixtures own virtual time; no installed image trace consumes
// these timestamps. The clock binding lets the real trace path link in tests.
#[cfg(all(test, oer_trace, oer_debug_control))]
mod trace_clock {
    fn now() -> u64 {
        0
    }
    oer_trace::clock!(now);
}

pub mod ampdu_tx;
pub mod coex;
pub mod cooperative_hardware;
pub mod datapath;
pub mod irq;
pub mod lower_mac;
pub mod monitor_injection;
pub mod ordinary_tx;
pub mod port;
#[cfg(target_arch = "riscv32")]
pub mod runtime;
pub mod rx;
#[cfg(target_arch = "riscv32")]
pub mod startup;
pub mod station_tsf;
pub mod tx;
