//! Finite Wi-Fi CCMP key-table transactions.

use oer_esp32s31_pac::{
    MacInterface, MacKeyEntryIndex,
    wifi_mac_crypto_control::WifiMacCryptoControl,
};
use oer_mmio::{Bits, Index, Placement};

use super::{MacPlacement, WifiRadioRegisters};

const KEY_ENTRY_COUNT: usize = 25;
const CCMP_ALGORITHM: Bits<2> = Bits::constant::<3>();
const PAIRWISE_DIRECTION: Bits<3> = Bits::constant::<3>();
const GROUP_DIRECTION: Bits<3> = Bits::constant::<6>();
const PAIRWISE_LOGICAL_KEY_INDEX: u8 = 0;
const LOGICAL_INDEX_WITHOUT_FLAG: u8 = 3;
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
/// and key-byte packing are private implementation details.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacCcmpKeyIdentity {
    Pairwise { peer: [u8; 6] },
    Group { key_id: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacKeyInstallOutcome {
    Installed,
    Occupied,
    Rejected,
}

/// The key-table entry of a bounded index.
const fn key_entry(index: MacKeyEntryIndex) -> Option<Index<KEY_ENTRY_COUNT>> {
    match Index::new(index.get() as usize) {
        Ok(entry) => Some(entry),
        Err(_) => None,
    }
}

/// The crypto-control interface word of an interface.
const fn interface_word(interface: MacInterface) -> Option<Index<3>> {
    match interface {
        MacInterface::Station => Some(Index::constant::<0>()),
        MacInterface::AccessPoint => Some(Index::constant::<1>()),
        MacInterface::Context2 => Some(Index::constant::<2>()),
        MacInterface::Context3 => None,
    }
}

fn key_entry_validity<P: Placement>(
    control: &WifiMacCryptoControl<P>,
) -> [bool; KEY_ENTRY_COUNT] {
    let validity = control.read_key_valid_bitmap();
    [
        validity.entry_0_valid(),
        validity.entry_1_valid(),
        validity.entry_2_valid(),
        validity.entry_3_valid(),
        validity.entry_4_valid(),
        validity.entry_5_valid(),
        validity.entry_6_valid(),
        validity.entry_7_valid(),
        validity.entry_8_valid(),
        validity.entry_9_valid(),
        validity.entry_10_valid(),
        validity.entry_11_valid(),
        validity.entry_12_valid(),
        validity.entry_13_valid(),
        validity.entry_14_valid(),
        validity.entry_15_valid(),
        validity.entry_16_valid(),
        validity.entry_17_valid(),
        validity.entry_18_valid(),
        validity.entry_19_valid(),
        validity.entry_20_valid(),
        validity.entry_21_valid(),
        validity.entry_22_valid(),
        validity.entry_23_valid(),
        validity.entry_24_valid(),
    ]
}

/// Whether `entry` is valid in `validity`.
fn is_valid(validity: &[bool; KEY_ENTRY_COUNT], entry: Index<KEY_ENTRY_COUNT>) -> bool {
    validity.get(entry.get()).copied().unwrap_or(false)
}

/// Mark `entry` valid or invalid in `validity`.
fn set_valid(validity: &mut [bool; KEY_ENTRY_COUNT], entry: Index<KEY_ENTRY_COUNT>, valid: bool) {
    if let Some(bit) = validity.get_mut(entry.get()) {
        *bit = valid;
    }
}

/// Publish the complete validity image: every entry's bit in one
/// read-modify-write, the reserved high bits zero.
fn publish_key_entry_validity<P: Placement>(
    control: &mut WifiMacCryptoControl<P>,
    validity: [bool; KEY_ENTRY_COUNT],
) {
    control.modify_key_valid_bitmap(|w| {
        w
            .set_entry_0_valid(validity[0])
            .set_entry_1_valid(validity[1])
            .set_entry_2_valid(validity[2])
            .set_entry_3_valid(validity[3])
            .set_entry_4_valid(validity[4])
            .set_entry_5_valid(validity[5])
            .set_entry_6_valid(validity[6])
            .set_entry_7_valid(validity[7])
            .set_entry_8_valid(validity[8])
            .set_entry_9_valid(validity[9])
            .set_entry_10_valid(validity[10])
            .set_entry_11_valid(validity[11])
            .set_entry_12_valid(validity[12])
            .set_entry_13_valid(validity[13])
            .set_entry_14_valid(validity[14])
            .set_entry_15_valid(validity[15])
            .set_entry_16_valid(validity[16])
            .set_entry_17_valid(validity[17])
            .set_entry_18_valid(validity[18])
            .set_entry_19_valid(validity[19])
            .set_entry_20_valid(validity[20])
            .set_entry_21_valid(validity[21])
            .set_entry_22_valid(validity[22])
            .set_entry_23_valid(validity[23])
            .set_entry_24_valid(validity[24]);
    });
}

/// One interface's crypto-control word: every field written, as the vendor's
/// whole-word store does.
fn publish_interface_control<P: Placement>(
    control: &mut WifiMacCryptoControl<P>,
    interface: Index<3>,
    bits_0_1: Bits<2>,
    bit_8: bool,
    spp_amsdu: bool,
    bits_16_17: Bits<2>,
) {
    control.modify_interface_control(interface, |w| {
        w.set_bits_0_1_opaque(bits_0_1)
            .set_bit_8_opaque(bit_8)
            .set_spp_amsdu(spp_amsdu)
            .set_bits_16_17_opaque(bits_16_17)
            .set_bit_28_opaque(false)
            .set_bits_30_31_opaque(Bits::constant::<0>());
    });
}

// Complete ordinary CCMP enable leaf. Keys preserve the interface's prior SPP
// decision; only configuring a new association supplies a different value.
fn enable_ccmp<P: Placement>(control: &mut WifiMacCryptoControl<P>, interface: Index<3>, spp: bool) {
    publish_interface_control(
        control,
        interface,
        Bits::constant::<3>(),
        true,
        spp,
        Bits::constant::<3>(),
    );
    control.modify_policy_control(|w| {
        w.set_bits_6_21_opaque(0);
    });
    control.modify_interface_control(interface, |w| {
        w.set_bits_30_31_opaque(Bits::constant::<0>());
    });
}

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Report whether one bounded hardware key-table entry is currently valid.
    pub fn mac_key_entry_is_valid(&self, index: MacKeyEntryIndex) -> bool {
        key_entry_validity(&self.mac.wifi_mac_crypto_control)
            .get(index.get() as usize)
            .copied()
            .unwrap_or(false)
    }

    /// Establish the common cold hardware-crypto bypass state.
    ///
    /// SOURCE: complete pinned
    /// `libpp.a[hal_crypto.o]::hal_crypto_init` (BLOB_LIBPP_HAL_CRYPTO_INIT).
    pub fn initialize_mac_crypto_bypass(&mut self) {
        let control = &mut self.mac.wifi_mac_crypto_control;
        let [station, access_point, context_2] = [
            Index::constant::<0>(),
            Index::constant::<1>(),
            Index::constant::<2>(),
        ];
        let zero = Bits::constant::<0>();
        publish_interface_control(control, station, zero, false, false, Bits::constant::<3>());
        publish_interface_control(control, access_point, zero, false, false, Bits::constant::<3>());
        publish_interface_control(control, context_2, zero, false, false, zero);
        control.modify_word_000c_opaque(|w| {
            w.set_bits_0_31_opaque(0);
        });
        control.modify_policy_control(|w| {
            w.set_bits_6_21_opaque(0);
        });
    }

    /// Apply an association's SPP mode before installing any of its keys.
    /// The driver establishes removal of every previous key before changing
    /// this per-interface field; installation itself preserves the decision.
    ///
    /// SOURCE: complete `libpp[hal_crypto.o]::hal_crypto_enable` consumes
    /// SPP from `libpp[wdev.o]::wDev_Insert_KeyEntry` at +0x2e..+0x34.
    pub fn configure_mac_ccmp_association(&mut self, interface: MacInterface, mode: MacSppMode) {
        let Some(word) = interface_word(interface) else {
            return;
        };
        enable_ccmp(
            &mut self.mac.wifi_mac_crypto_control,
            word,
            matches!(mode, MacSppMode::Authenticated),
        );
        oer_mmio::fence();
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
            MacCcmpKeyIdentity::Pairwise { peer } => {
                (peer, PAIRWISE_DIRECTION, PAIRWISE_LOGICAL_KEY_INDEX)
            }
            MacCcmpKeyIdentity::Group { key_id } if key_id <= MAX_GROUP_KEY_ID => {
                ([u8::MAX; 6], GROUP_DIRECTION, key_id)
            }
            MacCcmpKeyIdentity::Group { .. } => return MacKeyInstallOutcome::Rejected,
        };
        let (Some(entry), Some(word)) = (key_entry(index), interface_word(interface)) else {
            return MacKeyInstallOutcome::Rejected;
        };
        let mut validity = key_entry_validity(&self.mac.wifi_mac_crypto_control);
        if is_valid(&validity, entry) {
            return MacKeyInstallOutcome::Occupied;
        }

        self.clear_mac_key_entry_words(entry);
        let table = &mut self.mac.wifi_mac_key_table;
        let [a, b, c, d, e, f] = peer;
        table.modify_entry_peer_low(entry, |w| {
            w.set_peer_low(u32::from_le_bytes([a, b, c, d]));
        });
        table.modify_entry_peer_control(entry, |w| {
            w.set_peer_high(u16::from_le_bytes([e, f]))
                .set_bits_16_17_opaque(Bits::constant::<0>())
                .set_algorithm(CCMP_ALGORITHM)
                .set_bit_20_opaque(false)
                .set_direction(direction)
                .set_interface(Bits::truncate(interface.bits()))
                .set_bit_26_opaque(false)
                .set_logical_index_not_three(logical_key_index != LOGICAL_INDEX_WITHOUT_FLAG)
                .set_logical_key_index(Bits::truncate(u32::from(logical_key_index)));
        });
        let [
            k0, k1, k2, k3, k4, k5, k6, k7, k8, k9, k10, k11, k12, k13, k14, k15,
        ] = *temporal_key;
        table.modify_entry_key_word_0(entry, |w| {
            w.set_value(u32::from_le_bytes([k0, k1, k2, k3]));
        });
        table.modify_entry_key_word_1(entry, |w| {
            w.set_value(u32::from_le_bytes([k4, k5, k6, k7]));
        });
        table.modify_entry_key_word_2(entry, |w| {
            w.set_value(u32::from_le_bytes([k8, k9, k10, k11]));
        });
        table.modify_entry_key_word_3(entry, |w| {
            w.set_value(u32::from_le_bytes([k12, k13, k14, k15]));
        });

        let control = &mut self.mac.wifi_mac_crypto_control;
        set_valid(&mut validity, entry, true);
        publish_key_entry_validity(control, validity);
        let spp = control.read_interface_control(word).spp_amsdu();
        enable_ccmp(control, word, spp);
        oer_mmio::fence();

        if is_valid(&key_entry_validity(control), entry) {
            MacKeyInstallOutcome::Installed
        } else {
            MacKeyInstallOutcome::Rejected
        }
    }

    /// Invalidate and zero all ten words of one hardware key entry.
    pub fn clear_mac_key_entry(&mut self, index: MacKeyEntryIndex) {
        let Some(entry) = key_entry(index) else {
            return;
        };
        let control = &mut self.mac.wifi_mac_crypto_control;
        let mut validity = key_entry_validity(control);
        set_valid(&mut validity, entry, false);
        publish_key_entry_validity(control, validity);
        self.clear_mac_key_entry_words(entry);
        oer_mmio::fence();
    }

    fn clear_mac_key_entry_words(&mut self, entry: Index<KEY_ENTRY_COUNT>) {
        let table = &mut self.mac.wifi_mac_key_table;
        let zero = Bits::constant::<0>();
        table.modify_entry_peer_low(entry, |w| {
            w.set_peer_low(0);
        });
        table.modify_entry_peer_control(entry, |w| {
            w.set_peer_high(0)
                .set_bits_16_17_opaque(zero)
                .set_algorithm(zero)
                .set_bit_20_opaque(false)
                .set_direction(Bits::constant::<0>())
                .set_interface(zero)
                .set_bit_26_opaque(false)
                .set_logical_index_not_three(false)
                .set_logical_key_index(zero);
        });
        table.modify_entry_key_word_0(entry, |w| {
            w.set_value(0);
        });
        table.modify_entry_key_word_1(entry, |w| {
            w.set_value(0);
        });
        table.modify_entry_key_word_2(entry, |w| {
            w.set_value(0);
        });
        table.modify_entry_key_word_3(entry, |w| {
            w.set_value(0);
        });
        table.modify_entry_tail_word_0(entry, |w| {
            w.set_value(0);
        });
        table.modify_entry_tail_word_1(entry, |w| {
            w.set_value(0);
        });
        table.modify_entry_tail_word_2(entry, |w| {
            w.set_value(0);
        });
        table.modify_entry_tail_word_3(entry, |w| {
            w.set_value(0);
        });
    }
}
