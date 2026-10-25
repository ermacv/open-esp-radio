//! The finite Wi-Fi CCMP key-table transactions.

use oer_esp32s31_pac::wifi_mac::{WifiMacCryptoControl, WifiMacKeyTable};
use oer_mmio::{Index, Placement};

use super::{MacInterface, MacKeyEntryIndex, MacPlacement, WifiRadioRegisters, bits, element};

const KEY_ENTRY_COUNT: usize = oer_esp32s31_pac::wifi_mac::entry_peer_control::COUNT;
const MAX_GROUP_KEY_ID: u8 =
    oer_esp32s31_pac::wifi_mac::entry_peer_control::LOGICAL_KEY_INDEX_MAX as u8;

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

/// The key-table element of a bounded entry index.
const fn key_entry(index: MacKeyEntryIndex) -> Index<KEY_ENTRY_COUNT> {
    element::<KEY_ENTRY_COUNT>(index.get() as usize)
}

/// The crypto interface-control element of an interface: the hardware has
/// three, for interface selectors zero to two.
const fn interface_control(interface: MacInterface) -> Index<3> {
    element::<3>(interface.bits() as usize)
}

fn key_entry_validity<P: Placement>(control: &WifiMacCryptoControl<P>) -> [bool; KEY_ENTRY_COUNT] {
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

/// Publish the validity of every entry, every other field zero.
///
/// SOURCE: the zero-based `publish_mac_crypto_key_valid_entries` store of
/// complete `libpp.a[hal_crypto.o]::hal_crypto_set_key_entry` and
/// `hal_crypto_clr_key_entry`.
fn publish_key_entry_validity<P: Placement>(
    control: &mut WifiMacCryptoControl<P>,
    validity: [bool; KEY_ENTRY_COUNT],
) {
    let [
        v0,
        v1,
        v2,
        v3,
        v4,
        v5,
        v6,
        v7,
        v8,
        v9,
        v10,
        v11,
        v12,
        v13,
        v14,
        v15,
        v16,
        v17,
        v18,
        v19,
        v20,
        v21,
        v22,
        v23,
        v24,
    ] = validity;
    control.modify_key_valid_bitmap(|w| {
        w.set_entry_0_valid(v0)
            .set_entry_1_valid(v1)
            .set_entry_2_valid(v2)
            .set_entry_3_valid(v3)
            .set_entry_4_valid(v4)
            .set_entry_5_valid(v5)
            .set_entry_6_valid(v6)
            .set_entry_7_valid(v7)
            .set_entry_8_valid(v8)
            .set_entry_9_valid(v9)
            .set_entry_10_valid(v10)
            .set_entry_11_valid(v11)
            .set_entry_12_valid(v12)
            .set_entry_13_valid(v13)
            .set_entry_14_valid(v14)
            .set_entry_15_valid(v15)
            .set_entry_16_valid(v16)
            .set_entry_17_valid(v17)
            .set_entry_18_valid(v18)
            .set_entry_19_valid(v19)
            .set_entry_20_valid(v20)
            .set_entry_21_valid(v21)
            .set_entry_22_valid(v22)
            .set_entry_23_valid(v23)
            .set_entry_24_valid(v24);
    });
}

/// The fields of one interface-control store, the others zero.
#[derive(Clone, Copy)]
struct InterfaceControl {
    bits_0_1: u32,
    bit_8: bool,
    spp_amsdu: bool,
    bits_16_17: u32,
}

/// Store one interface-control word: the zero-based
/// `mac_crypto_interface_control` transaction, bits 28 and 31:30 zero.
fn store_interface_control<P: Placement>(
    control: &mut WifiMacCryptoControl<P>,
    index: Index<3>,
    value: InterfaceControl,
) {
    control.modify_interface_control(index, |w| {
        w.set_bits_0_1_opaque(bits(value.bits_0_1))
            .set_bit_8_opaque(value.bit_8)
            .set_spp_amsdu(value.spp_amsdu)
            .set_bits_16_17_opaque(bits(value.bits_16_17))
            .set_bit_28_opaque(false)
            .set_bits_30_31_opaque(bits(0));
    });
}

/// The fields of one key entry's peer-control store, the others zero.
#[derive(Clone, Copy)]
struct PeerControl {
    peer_high: u16,
    algorithm: u32,
    direction: u32,
    interface: MacInterface,
    logical_index_not_three: bool,
    logical_key_index: u8,
}

/// Store one key entry's peer-control word: the zero-based
/// `mac_key_entry_peer_control` transaction, its opaque bits zero.
fn store_peer_control<P: Placement>(
    table: &mut WifiMacKeyTable<P>,
    index: Index<KEY_ENTRY_COUNT>,
    value: PeerControl,
) {
    table.modify_entry_peer_control(index, |w| {
        w.set_peer_high(value.peer_high)
            .set_bits_16_17_opaque(bits(0))
            .set_algorithm(bits(value.algorithm))
            .set_bit_20_opaque(false)
            .set_direction(bits(value.direction))
            .set_interface(
                oer_esp32s31_pac::wifi_mac::entry_peer_control::Interface::from_field(
                    value.interface.bits(),
                ),
            )
            .set_bit_26_opaque(false)
            .set_logical_index_not_three(value.logical_index_not_three)
            .set_logical_key_index(bits(u32::from(value.logical_key_index)));
    });
}

/// Store the four key words of one entry, each whole.
fn store_key_words<P: Placement>(
    table: &mut WifiMacKeyTable<P>,
    index: Index<KEY_ENTRY_COUNT>,
    words: [u32; 4],
) {
    let [k0, k1, k2, k3] = words;
    table.modify_entry_key_word_0(index, |w| {
        w.set_value(k0);
    });
    table.modify_entry_key_word_1(index, |w| {
        w.set_value(k1);
    });
    table.modify_entry_key_word_2(index, |w| {
        w.set_value(k2);
    });
    table.modify_entry_key_word_3(index, |w| {
        w.set_value(k3);
    });
}

/// Zero all ten words of one key entry, in the vendor's order.
fn clear_key_entry_words<P: Placement>(
    table: &mut WifiMacKeyTable<P>,
    index: Index<KEY_ENTRY_COUNT>,
) {
    table.modify_entry_peer_low(index, |w| {
        w.set_peer_low(0);
    });
    store_peer_control(
        table,
        index,
        PeerControl {
            peer_high: 0,
            algorithm: 0,
            direction: 0,
            interface: MacInterface::Station,
            logical_index_not_three: false,
            logical_key_index: 0,
        },
    );
    store_key_words(table, index, [0; 4]);
    table.modify_entry_tail_word_0(index, |w| {
        w.set_value(0);
    });
    table.modify_entry_tail_word_1(index, |w| {
        w.set_value(0);
    });
    table.modify_entry_tail_word_2(index, |w| {
        w.set_value(0);
    });
    table.modify_entry_tail_word_3(index, |w| {
        w.set_value(0);
    });
}

// Complete ordinary CCMP enable leaf. Keys preserve the interface's prior SPP
// decision; only configuring a new association supplies a different value.
fn enable_ccmp<P: Placement>(control: &mut WifiMacCryptoControl<P>, index: Index<3>, spp: bool) {
    store_interface_control(
        control,
        index,
        InterfaceControl {
            bits_0_1: 3,
            bit_8: true,
            spp_amsdu: spp,
            bits_16_17: 3,
        },
    );
    control.modify_policy_control(|w| {
        w.set_bits_6_21_opaque(0);
    });
    control.modify_interface_control(index, |w| {
        w.set_bits_30_31_opaque(bits(0));
    });
}

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Report whether one bounded hardware key-table entry is currently valid.
    pub fn mac_key_entry_is_valid(&self, index: MacKeyEntryIndex) -> bool {
        let validity = key_entry_validity(&self.mac.wifi_mac_crypto_control);
        validity.get(index.get() as usize).copied().unwrap_or(false)
    }

    /// Establish the common cold hardware-crypto bypass state.
    ///
    /// SOURCE: complete pinned
    /// `libpp.a[hal_crypto.o]::hal_crypto_init`.
    pub fn initialize_mac_crypto_bypass(&mut self) {
        let control = &mut self.mac.wifi_mac_crypto_control;
        for (position, bits_16_17) in [3, 3, 0].into_iter().enumerate() {
            store_interface_control(
                control,
                element::<3>(position),
                InterfaceControl {
                    bits_0_1: 0,
                    bit_8: false,
                    spp_amsdu: false,
                    bits_16_17,
                },
            );
        }
        control.modify_word_080c_opaque(|w| {
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
        enable_ccmp(
            &mut self.mac.wifi_mac_crypto_control,
            interface_control(interface),
            matches!(mode, MacSppMode::Authenticated),
        );
        oer_riscv::fence();
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
            MacCcmpKeyIdentity::Pairwise { peer } => (
                peer,
                oer_esp32s31_pac::wifi_mac::entry_peer_control::Direction::Pairwise.bits(),
                oer_esp32s31_pac::wifi_mac::entry_peer_control::LogicalKeyIndex::Pairwise.bits()
                    as u8,
            ),
            MacCcmpKeyIdentity::Group { key_id } if key_id <= MAX_GROUP_KEY_ID => (
                [u8::MAX; 6],
                oer_esp32s31_pac::wifi_mac::entry_peer_control::Direction::Group.bits(),
                key_id,
            ),
            MacCcmpKeyIdentity::Group { .. } => return MacKeyInstallOutcome::Rejected,
        };
        let position = index.get() as usize;
        let entry = key_entry(index);
        let control_index = interface_control(interface);
        let mut validity = key_entry_validity(&self.mac.wifi_mac_crypto_control);
        let Some(valid) = validity.get_mut(position) else {
            return MacKeyInstallOutcome::Rejected;
        };
        if *valid {
            return MacKeyInstallOutcome::Occupied;
        }
        *valid = true;

        let table = &mut self.mac.wifi_mac_key_table;
        clear_key_entry_words(table, entry);
        let [a, b, c, d, e, f] = peer;
        table.modify_entry_peer_low(entry, |w| {
            w.set_peer_low(u32::from_le_bytes([a, b, c, d]));
        });
        store_peer_control(
            table,
            entry,
            PeerControl {
                peer_high: u16::from_le_bytes([e, f]),
                algorithm: oer_esp32s31_pac::wifi_mac::entry_peer_control::Algorithm::Ccmp.bits(),
                direction,
                interface,
                logical_index_not_three: logical_key_index
                    != oer_esp32s31_pac::wifi_mac::entry_peer_control::LogicalKeyIndex::WithoutFlag
                        .bits() as u8,
                logical_key_index,
            },
        );
        let (words, _) = temporal_key.as_chunks::<4>();
        let [k0, k1, k2, k3] = words else {
            unreachable!("a 16-byte key has four words");
        };
        store_key_words(
            table,
            entry,
            [
                u32::from_le_bytes(*k0),
                u32::from_le_bytes(*k1),
                u32::from_le_bytes(*k2),
                u32::from_le_bytes(*k3),
            ],
        );

        let control = &mut self.mac.wifi_mac_crypto_control;
        publish_key_entry_validity(control, validity);
        let spp = control.read_interface_control(control_index).spp_amsdu();
        enable_ccmp(control, control_index, spp);
        oer_riscv::fence();

        if key_entry_validity(control)
            .get(position)
            .copied()
            .unwrap_or(false)
        {
            MacKeyInstallOutcome::Installed
        } else {
            MacKeyInstallOutcome::Rejected
        }
    }

    /// Invalidate and zero all ten words of one hardware key entry.
    pub fn clear_mac_key_entry(&mut self, index: MacKeyEntryIndex) {
        let control = &mut self.mac.wifi_mac_crypto_control;
        let mut validity = key_entry_validity(control);
        if let Some(valid) = validity.get_mut(index.get() as usize) {
            *valid = false;
        }
        publish_key_entry_validity(control, validity);
        clear_key_entry_words(&mut self.mac.wifi_mac_key_table, key_entry(index));
        oer_riscv::fence();
    }
}

#[cfg(test)]
mod tests;
