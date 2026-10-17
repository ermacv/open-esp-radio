#![no_std]
#![forbid(unsafe_code)]

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
//! `af55a0ca258ce9d791d1661d7c2bbb65f08c0c21`).
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

#[cfg(any(target_arch = "riscv32", test))]
mod channel;
#[cfg(target_arch = "riscv32")]
pub use channel::{PhyChannel, lower_wifi_channel, switch_esp32s31_wifi_channel};
#[cfg(target_arch = "riscv32")]
pub use channel::{PublishedWifiChannelError, switch_published_esp32s31_wifi_channel};
pub mod ampdu_tx;
pub mod coex;
#[cfg(target_arch = "riscv32")]
pub mod cold_start;
pub mod cooperative_hardware;
pub mod datapath;
pub mod esp_now;
pub mod lower_mac;
#[cfg(target_arch = "riscv32")]
pub mod mac_start;
pub mod monitor_injection;
pub mod ordinary_tx;
pub mod protected_data_rx;
#[cfg(target_arch = "riscv32")]
pub mod runtime;
pub mod rx;
#[cfg(target_arch = "riscv32")]
pub mod startup;
pub mod station_tsf;
pub mod tx;
