//! Generated-PAC ownership for finite Wi-Fi CCMP key-table transactions.

#![forbid(unsafe_code)]

use crate::{MacInterface, MacKeyEntryIndex, WifiRadioRegisters, device_fence, svd};

const KEY_ENTRY_WORDS: usize = 10;
const KEY_ENTRY_COUNT: usize = 25;
const CCMP_ALGORITHM: u32 = 3;
const PAIRWISE_LOGICAL_KEY_INDEX: u32 = 0;
const MAX_GROUP_KEY_ID: u8 = 3;

/// Hardware authentication of the QoS A-MSDU-present bit for one interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacSppMode {
    Authenticated,
    Unauthenticated,
}

/// Semantic identity encoded into one CCMP hardware key-table entry.
///
/// The register image is deliberately not exposed. Peer/control word layout
/// and key-byte packing are private PAC implementation details.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacCcmpKeyIdentity {
    Pairwise { peer: [u8; 6] },
    Group { key_id: u8 },
}

fn key_entry_validity(control: &svd::WifiMacCryptoControl) -> [bool; KEY_ENTRY_COUNT] {
    let validity = control.key_valid_bitmap().read();
    [
        validity.entry_0_valid().bit(),
        validity.entry_1_valid().bit(),
        validity.entry_2_valid().bit(),
        validity.entry_3_valid().bit(),
        validity.entry_4_valid().bit(),
        validity.entry_5_valid().bit(),
        validity.entry_6_valid().bit(),
        validity.entry_7_valid().bit(),
        validity.entry_8_valid().bit(),
        validity.entry_9_valid().bit(),
        validity.entry_10_valid().bit(),
        validity.entry_11_valid().bit(),
        validity.entry_12_valid().bit(),
        validity.entry_13_valid().bit(),
        validity.entry_14_valid().bit(),
        validity.entry_15_valid().bit(),
        validity.entry_16_valid().bit(),
        validity.entry_17_valid().bit(),
        validity.entry_18_valid().bit(),
        validity.entry_19_valid().bit(),
        validity.entry_20_valid().bit(),
        validity.entry_21_valid().bit(),
        validity.entry_22_valid().bit(),
        validity.entry_23_valid().bit(),
        validity.entry_24_valid().bit(),
    ]
}

fn publish_key_entry_validity(
    control: &svd::WifiMacCryptoControl,
    validity: [bool; KEY_ENTRY_COUNT],
) {
    crate::svd::zero_based_field_write::publish_mac_crypto_key_valid_entries(
        control,
        validity[0],
        validity[1],
        validity[2],
        validity[3],
        validity[4],
        validity[5],
        validity[6],
        validity[7],
        validity[8],
        validity[9],
        validity[10],
        validity[11],
        validity[12],
        validity[13],
        validity[14],
        validity[15],
        validity[16],
        validity[17],
        validity[18],
        validity[19],
        validity[20],
        validity[21],
        validity[22],
        validity[23],
        validity[24],
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacKeyInstallOutcome {
    Installed,
    Occupied,
    Rejected,
}

impl WifiRadioRegisters {
    /// Report whether one bounded hardware key-table entry is currently valid.
    pub fn mac_key_entry_is_valid(&self, index: MacKeyEntryIndex) -> bool {
        key_entry_validity(&self.peripherals.wifi_mac.wifi_mac_crypto_control)[index.get() as usize]
    }

    /// Establish the common cold hardware-crypto bypass state.
    ///
    /// SOURCE: complete pinned
    /// `libpp.a[hal_crypto.o]::hal_crypto_init`.
    pub fn initialize_mac_crypto_bypass(&mut self) {
        let control = &self.peripherals.wifi_mac.wifi_mac_crypto_control;
        crate::svd::zero_based_field_write::mac_crypto_interface_control(
            control, 0, 0, false, 0xc0, 0,
        );
        crate::svd::zero_based_field_write::mac_crypto_interface_control(
            control, 1, 0, false, 0xc0, 0,
        );
        crate::svd::zero_based_field_write::mac_crypto_interface_control(
            control, 2, 0, false, 0, 0,
        );
        crate::svd::zero_register_write::clear_mac_crypto_init_aux(control);
        crate::svd::zero_register_write::clear_mac_crypto_policy(control);
    }

    /// Apply an association's SPP mode before installing any of its keys.
    /// The driver establishes removal of every previous key before changing
    /// this per-interface field; installation itself preserves the decision.
    ///
    /// SOURCE: complete `libpp[hal_crypto.o]::hal_crypto_enable` consumes
    /// SPP from `libpp[wdev.o]::wDev_Insert_KeyEntry` at +0x2e..+0x34.
    pub fn configure_mac_ccmp_association(&mut self, interface: MacInterface, mode: MacSppMode) {
        enable_ccmp(
            &self.peripherals.wifi_mac.wifi_mac_crypto_control,
            interface.bits() as usize,
            matches!(mode, MacSppMode::Authenticated),
        );
        device_fence();
    }

    /// Encode and install one semantic STA CCMP key into an invalid entry.
    ///
    /// SOURCE: complete `libpp.a::hal_crypto_clr_key_entry`,
    /// `hal_crypto_set_key_entry`, `hal_crypto_is_key_valid`, and the reachable
    /// `hal_crypto_enable` CCMP branch, retaining the association's SPP field.
    pub fn install_sta_ccmp_key_entry(
        &mut self,
        index: MacKeyEntryIndex,
        identity: MacCcmpKeyIdentity,
        temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        self.install_ccmp_key_entry(MacInterface::Station, index, identity, temporal_key)
    }

    /// Encode and install one semantic AP CCMP key into an invalid entry.
    ///
    /// SOURCE: complete `wDev_Insert_KeyEntry` passes interface one to the
    /// same `hal_crypto_enable` transaction. AP group keys occupy logical
    /// slots 1..=4; the first associated AP peer observed hardware slot 8.
    pub fn install_ap_ccmp_key_entry(
        &mut self,
        index: MacKeyEntryIndex,
        identity: MacCcmpKeyIdentity,
        temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        self.install_ccmp_key_entry(MacInterface::AccessPoint, index, identity, temporal_key)
    }

    fn install_ccmp_key_entry(
        &mut self,
        interface: MacInterface,
        index: MacKeyEntryIndex,
        identity: MacCcmpKeyIdentity,
        temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        let (peer, direction, logical_key_index) = match identity {
            MacCcmpKeyIdentity::Pairwise { peer } => (peer, 3_u32, PAIRWISE_LOGICAL_KEY_INDEX),
            MacCcmpKeyIdentity::Group { key_id } if key_id <= MAX_GROUP_KEY_ID => {
                ([u8::MAX; 6], 6_u32, u32::from(key_id))
            }
            MacCcmpKeyIdentity::Group { .. } => return MacKeyInstallOutcome::Rejected,
        };
        let index = index.get();
        let interface_bits = interface.bits();
        let interface_index = interface_bits as usize;
        let mut validity = key_entry_validity(&self.peripherals.wifi_mac.wifi_mac_crypto_control);
        if validity[index as usize] {
            return MacKeyInstallOutcome::Occupied;
        }

        self.clear_mac_key_entry_words(index);
        let table = &self.peripherals.wifi_mac.wifi_mac_key_table;
        let [a, b, c, d, e, f] = peer;
        let peer_low = u32::from_le_bytes([a, b, c, d]);
        let peer_high = u16::from_le_bytes([e, f]);
        let cipher = CCMP_ALGORITHM << 18;
        let control = (direction << 5)
            | (interface_bits << 8)
            | (u32::from(logical_key_index != 3) << 11)
            | (logical_key_index << 14)
            | ((cipher >> 16) & 0x341f);
        crate::svd::zero_based_field_write::mac_key_table_entry_word(
            table,
            index as usize * KEY_ENTRY_WORDS,
            peer_low,
        );
        crate::svd::zero_based_field_write::mac_key_table_entry_word(
            table,
            index as usize * KEY_ENTRY_WORDS + 1,
            u32::from(peer_high) | (control << 16),
        );
        for (word, bytes) in temporal_key.as_chunks::<4>().0.iter().enumerate() {
            crate::svd::zero_based_field_write::mac_key_table_entry_word(
                table,
                index as usize * KEY_ENTRY_WORDS + word + 2,
                u32::from_le_bytes(*bytes),
            );
        }

        let control = &self.peripherals.wifi_mac.wifi_mac_crypto_control;
        validity[index as usize] = true;
        publish_key_entry_validity(control, validity);
        enable_ccmp(
            control,
            interface_index,
            control
                .interface_control(interface_index)
                .read()
                .spp_amsdu()
                .bit(),
        );
        device_fence();

        if !key_entry_validity(control)[index as usize] {
            MacKeyInstallOutcome::Rejected
        } else {
            MacKeyInstallOutcome::Installed
        }
    }

    /// Invalidate and zero all ten words of one hardware key entry.
    pub fn clear_mac_key_entry(&mut self, index: MacKeyEntryIndex) {
        let index = index.get();
        let control = &self.peripherals.wifi_mac.wifi_mac_crypto_control;
        let mut validity = key_entry_validity(control);
        validity[index as usize] = false;
        publish_key_entry_validity(control, validity);
        self.clear_mac_key_entry_words(index);
        device_fence();
    }

    fn clear_mac_key_entry_words(&mut self, index: u32) {
        let table = &self.peripherals.wifi_mac.wifi_mac_key_table;
        for word in 0..KEY_ENTRY_WORDS {
            crate::svd::zero_based_field_write::mac_key_table_entry_word(
                table,
                index as usize * KEY_ENTRY_WORDS + word,
                0,
            );
        }
    }
}

// Complete ordinary CCMP enable leaf. Keys preserve the interface's prior SPP
// decision; only configuring a new association supplies a different value.
fn enable_ccmp(control: &svd::WifiMacCryptoControl, interface_index: usize, spp: bool) {
    crate::svd::zero_based_field_write::mac_crypto_interface_control(
        control,
        interface_index,
        0x103,
        spp,
        0xc0,
        0,
    );
    control
        .policy_control()
        .modify(|_, w| w.bits_6_21_opaque().set(0));
    control
        .interface_control(interface_index)
        .modify(|_, w| w.bits_30_31_opaque().set(0));
}
