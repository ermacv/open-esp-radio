//! Generated-PAC ownership for the finite MAC BlockAck register leaves.

#![forbid(unsafe_code)]

use crate::{
    MacExtraSoftApRxBlockAckEntryIndex, MacInterface, MacRxBlockAckEntryIndex,
    MacRxBlockAckStartingSequence, MacRxBlockAckTid, MacRxBlockAckWindow, WifiRadioRegisters,
};

/// Result sampled for one completed TX hardware queue.
///
/// SOURCE: complete `libpp[hal_debug.o]::dbg_read_rx_ba`.
/// The hot TX completion path samples the three BlockAck payload words plus
/// the independent hardware result bit. Use [`TxBlockAckDiagnosticSnapshot`]
/// when the transmitter address and queues four through seven are needed for
/// diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TxBlockAckObservation {
    pub(crate) control: u8,
    pub(crate) starting_sequence: u16,
    pub(crate) bitmap: u64,
    pub(crate) block_ack_received: bool,
}

/// Semantic TX BlockAck payload sampled without adjacent validity state.
///
/// SOURCE: complete `libpp[hal_mac_tx.o]::hal_mac_tx_get_blockack`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckPayload {
    control: u8,
    starting_sequence: u16,
    bitmap: u64,
}

impl TxBlockAckPayload {
    pub const fn control(self) -> u8 {
        self.control
    }

    pub const fn starting_sequence(self) -> u16 {
        self.starting_sequence
    }

    pub const fn bitmap(self) -> u64 {
        self.bitmap
    }

    /// Low word of the protocol bitmap for the vendor-shaped validation ABI.
    pub const fn bitmap_low(self) -> u32 {
        self.bitmap as u32
    }

    /// High word of the protocol bitmap for the vendor-shaped validation ABI.
    pub const fn bitmap_high(self) -> u32 {
        (self.bitmap >> 32) as u32
    }
}

/// Semantic `WDEVTXQBA` result plus adjacent TX queue information.
///
/// SOURCE: complete `libpp[hal_debug.o]::dbg_read_rx_ba`.
/// Despite the function name, its strings identify eight reverse-addressed
/// TX BlockAck result banks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckDiagnosticSnapshot {
    control: u8,
    starting_sequence: u16,
    bitmap: u64,
    transmitter_address: [u8; 6],
    acknowledgement_tid: u8,
    acknowledgement_received: bool,
    block_ack_received: bool,
    last_tx_was_trigger_based: bool,
    trigger_based_packet_count: u8,
}

impl TxBlockAckDiagnosticSnapshot {
    pub const fn control(self) -> u8 {
        self.control
    }

    pub const fn starting_sequence(self) -> u16 {
        self.starting_sequence
    }

    pub const fn bitmap(self) -> u64 {
        self.bitmap
    }

    pub const fn transmitter_address(self) -> [u8; 6] {
        self.transmitter_address
    }

    pub const fn acknowledgement_tid(self) -> u8 {
        self.acknowledgement_tid
    }

    pub const fn acknowledgement_received(self) -> bool {
        self.acknowledgement_received
    }

    pub const fn block_ack_received(self) -> bool {
        self.block_ack_received
    }

    pub const fn last_tx_was_trigger_based(self) -> bool {
        self.last_tx_was_trigger_based
    }

    pub const fn trigger_based_packet_count(self) -> u8 {
        self.trigger_based_packet_count
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InternalTxBlockAckSnapshot {
    bitmap: u64,
    fragment_number: u8,
    starting_sequence: u16,
    tid: u8,
}

impl InternalTxBlockAckSnapshot {
    pub const fn bitmap(self) -> u64 {
        self.bitmap
    }

    pub const fn fragment_number(self) -> u8 {
        self.fragment_number
    }

    pub const fn starting_sequence(self) -> u16 {
        self.starting_sequence
    }

    pub const fn tid(self) -> u8 {
        self.tid
    }
}

/// Live image of one ordinary receive BlockAck hardware bank.
///
/// SOURCE: complete `libpp[hal_ampdu.o]::hal_ba_session_store`.
/// The status and load words are deliberately exposed separately: the blob
/// snapshots the hardware-maintained words and restores them through their
/// adjacent software-load words.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBlockAckEntrySnapshot {
    pub enabled: bool,
    pub tid: u8,
    pub write_enabled: bool,
    pub valid: bool,
    pub control_unknown_clear: bool,
    pub peer: [u8; 6],
    pub interface: MacInterface,
    pub window: u8,
    pub current_sequence: u16,
    pub loaded_start_sequence: u16,
    pub bitmap_status: u64,
    pub bitmap_load: u64,
}

/// Latched projection of one indirect extra-SoftAP receive BlockAck bank.
///
/// SOURCE: complete `libpp[hal_ampdu.o]::hal_debug_read_extra_softap_rx_ba` selects each logical index through
/// control bits 5..9, asserts agreement-update bit nine, reads the control,
/// peer, policy and starting-sequence staging words, then releases the latch.
/// The two bitmap words are the adjacent raw staging words used by complete
/// `hal_agreement_clr_extra_softap_rx_ba`; they remain raw observations rather
/// than a claimed protocol interpretation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExtraSoftApRxBlockAckEntrySnapshot {
    pub enabled: bool,
    pub index: u8,
    pub tid: u8,
    pub write_enabled: bool,
    pub valid: bool,
    pub control_unknown_clear: bool,
    pub peer: [u8; 6],
    pub interface: MacInterface,
    pub window: u8,
    pub starting_sequence: u16,
    pub bitmap: u64,
}

const fn block_ack_bitmap(bitmap_low: u32, bitmap_high: u32) -> u64 {
    let low = bitmap_low.to_le_bytes();
    let high = bitmap_high.to_le_bytes();
    u64::from_le_bytes([
        low[0], low[1], low[2], low[3], high[0], high[1], high[2], high[3],
    ])
}

const fn mac_interface_from_field(value: u8) -> MacInterface {
    match value {
        0 => MacInterface::Station,
        1 => MacInterface::AccessPoint,
        2 => MacInterface::Context2,
        3 => MacInterface::Context3,
        _ => unreachable!(),
    }
}

const fn rx_block_ack_register_index(hardware_index: MacRxBlockAckEntryIndex) -> usize {
    7 - hardware_index.get() as usize
}

impl WifiRadioRegisters {
    /// Sample the three words copied by `hal_mac_tx_get_blockack`.
    pub fn read_tx_block_ack_payload(&self, hardware_queue: u8) -> Option<TxBlockAckPayload> {
        let block = &self.peripherals.wifi_mac.wifi_mac_block_ack;
        let (control, starting_sequence, bitmap_low, bitmap_high) = match hardware_queue {
            0 => {
                let control = block.tx_block_ack_control_sequence_q0().read();
                (
                    control.tid_or_control().bits(),
                    control.starting_sequence().bits(),
                    block.tx_block_ack_bitmap_low_q0().read().bitmap().bits(),
                    block.tx_block_ack_bitmap_high_q0().read().bitmap().bits(),
                )
            }
            1 => {
                let control = block.tx_block_ack_control_sequence_q1().read();
                (
                    control.tid_or_control().bits(),
                    control.starting_sequence().bits(),
                    block.tx_block_ack_bitmap_low_q1().read().bitmap().bits(),
                    block.tx_block_ack_bitmap_high_q1().read().bitmap().bits(),
                )
            }
            2 => {
                let control = block.tx_block_ack_control_sequence_q2().read();
                (
                    control.tid_or_control().bits(),
                    control.starting_sequence().bits(),
                    block.tx_block_ack_bitmap_low_q2().read().bitmap().bits(),
                    block.tx_block_ack_bitmap_high_q2().read().bitmap().bits(),
                )
            }
            3 => {
                let control = block.tx_block_ack_control_sequence_q3().read();
                (
                    control.tid_or_control().bits(),
                    control.starting_sequence().bits(),
                    block.tx_block_ack_bitmap_low_q3().read().bitmap().bits(),
                    block.tx_block_ack_bitmap_high_q3().read().bitmap().bits(),
                )
            }
            _ => return None,
        };
        Some(TxBlockAckPayload {
            control,
            starting_sequence,
            bitmap: block_ack_bitmap(bitmap_low, bitmap_high),
        })
    }

    /// Program one ordinary receive BlockAck entry.
    ///
    /// SOURCE: complete `libpp[hal_ampdu.o]::hal_agreement_add_rx_ba`, size `0x12e`.
    ///
    /// The eight entries are direct reverse-addressed banks, not the shared
    /// extra-SoftAP staging window at `0x2010_4ea4`. Logical index zero lives
    /// at `0x2010_4274`; each successive index subtracts `0x24`.
    pub fn program_rx_block_ack_entry(
        &mut self,
        hardware_index: MacRxBlockAckEntryIndex,
        interface: MacInterface,
        peer: [u8; 6],
        tid: MacRxBlockAckTid,
        starting_sequence: MacRxBlockAckStartingSequence,
        window: MacRxBlockAckWindow,
    ) {
        let block = &self.peripherals.wifi_mac.wifi_mac_block_ack;
        let register_index = rx_block_ack_register_index(hardware_index);
        let peer_head = u32::from_le_bytes([peer[0], peer[1], peer[2], peer[3]]);
        let peer_tail = u16::from_le_bytes([peer[4], peer[5]]);

        crate::svd::zero_based_field_write::rx_block_ack_peer_head(
            block,
            register_index,
            peer_head,
        );
        crate::svd::zero_based_field_write::rx_block_ack_peer_tail_and_policy(
            block,
            register_index,
            peer_tail,
            interface.bits() as u8,
            window.get() as u8,
        );
        block
            .rx_block_ack_entry_start_sequence_load(register_index)
            .modify(|_, w| w.sequence().set(starting_sequence.get() as u16));
        block
            .rx_block_ack_entry_control(register_index)
            .modify(|_, w| w.valid().clear_bit());
        crate::svd::zero_register_write::clear_rx_block_ack_bitmap_low_load(block, register_index);
        crate::svd::zero_register_write::clear_rx_block_ack_bitmap_high_load(block, register_index);
        crate::generated::request_rx_block_ack_entry_update(block, hardware_index);
        crate::svd::zero_based_field_write::rx_block_ack_active_control(
            block,
            register_index,
            true,
            tid.get() as u8,
            true,
            true,
        );
    }

    /// Delete one ordinary receive BlockAck entry.
    ///
    /// SOURCE: complete `libpp[hal_ampdu.o]::hal_agreement_del_rx_ba`, size `0x72`.
    pub fn delete_rx_block_ack_entry(&mut self, hardware_index: MacRxBlockAckEntryIndex) {
        let block = &self.peripherals.wifi_mac.wifi_mac_block_ack;
        let register_index = rx_block_ack_register_index(hardware_index);
        block
            .rx_block_ack_entry_control(register_index)
            .modify(|_, w| w.valid().clear_bit());
        crate::svd::zero_register_write::clear_rx_block_ack_bitmap_low_load(block, register_index);
        crate::svd::zero_register_write::clear_rx_block_ack_bitmap_high_load(block, register_index);
        block
            .rx_block_ack_entry_control(register_index)
            .modify(|_, w| w.valid().set_bit());
        // The final full-word zero is a distinct observable edge in the blob.
        crate::svd::zero_register_write::clear_rx_block_ack_entry_control(block, register_index);
    }

    /// Move one live ordinary receive BlockAck entry to a new window.
    ///
    /// SOURCE: complete `libpp[hal_ampdu.o]::hal_agreement_clr_rx_ba`, size `0x92`.
    ///
    /// The vendor clears validity with a full-word zero, reloads the starting
    /// sequence and physical window, clears both bitmap load words, publishes
    /// the ordinary-bank update bit, and finally restores the active control
    /// word. Peer and interface fields remain unchanged.
    pub fn reset_rx_block_ack_window(
        &mut self,
        hardware_index: MacRxBlockAckEntryIndex,
        tid: MacRxBlockAckTid,
        starting_sequence: MacRxBlockAckStartingSequence,
        window: MacRxBlockAckWindow,
    ) {
        let block = &self.peripherals.wifi_mac.wifi_mac_block_ack;
        let register_index = rx_block_ack_register_index(hardware_index);

        crate::svd::zero_register_write::clear_rx_block_ack_entry_control(block, register_index);
        block
            .rx_block_ack_entry_start_sequence_load(register_index)
            .modify(|_, w| w.sequence().set(starting_sequence.get() as u16));
        block
            .rx_block_ack_entry_peer_tail_and_policy(register_index)
            .modify(|_, w| w.window().set(window.get() as u8));
        crate::svd::zero_register_write::clear_rx_block_ack_bitmap_low_load(block, register_index);
        crate::svd::zero_register_write::clear_rx_block_ack_bitmap_high_load(block, register_index);
        crate::generated::request_rx_block_ack_entry_update(block, hardware_index);
        crate::svd::zero_based_field_write::rx_block_ack_active_control(
            block,
            register_index,
            true,
            tid.get() as u8,
            true,
            true,
        );
    }

    /// Program one extra-SoftAP receive BlockAck entry through the shared
    /// staging window.
    ///
    /// SOURCE: complete `libpp[hal_ampdu.o]::hal_agreement_add_extra_softap_rx_ba`, size `0x130`.
    ///
    /// This transaction is distinct from the eight direct ordinary banks.
    /// The selected entry is staged, committed through update bit eight,
    /// latched for the vendor's readback sequence through bit nine, and then
    /// released. The diagnostic log call is intentionally outside the PAC;
    /// its ordered volatile readback is retained here.
    pub fn program_extra_softap_rx_block_ack_entry(
        &mut self,
        hardware_index: MacExtraSoftApRxBlockAckEntryIndex,
        interface: MacInterface,
        peer: [u8; 6],
        tid: MacRxBlockAckTid,
        starting_sequence: MacRxBlockAckStartingSequence,
        window: MacRxBlockAckWindow,
    ) -> bool {
        let block = &self.peripherals.wifi_mac.wifi_mac_block_ack;
        let index = hardware_index.get() as u8;
        let peer_head = u32::from_le_bytes([peer[0], peer[1], peer[2], peer[3]]);
        let peer_tail = u16::from_le_bytes([peer[4], peer[5]]);

        block
            .extra_softap_rx_block_ack_control()
            .modify(|_, w| w.index().set(index));
        crate::svd::zero_based_field_write::extra_softap_rx_block_ack_peer_head(block, peer_head);
        crate::svd::zero_based_field_write::extra_softap_rx_block_ack_peer_tail_and_policy(
            block,
            peer_tail,
            interface.bits() as u8,
            window.get() as u8,
        );
        block
            .extra_softap_rx_block_ack_start_sequence()
            .modify(|_, w| w.sequence().set(starting_sequence.get() as u16));
        crate::svd::zero_register_write::clear_extra_softap_rx_block_ack_bitmap_low(block);
        crate::svd::zero_register_write::clear_extra_softap_rx_block_ack_bitmap_high(block);
        crate::svd::zero_based_field_write::extra_softap_rx_block_ack_active_control(
            block,
            true,
            index,
            tid.get() as u8,
            true,
            true,
        );

        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_commit().set_bit());
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_commit().clear_bit());
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_readback_latch().set_bit());

        // Preserve the complete vendor readback order. The values feed only
        // wifi_log in the blob and therefore do not cross this PAC boundary.
        let peer_tail_and_policy = block
            .extra_softap_rx_block_ack_peer_tail_and_policy()
            .read();
        let observed_peer_head = block
            .extra_softap_rx_block_ack_peer_head()
            .read()
            .peer_address_head()
            .bits();
        let observed_starting_sequence = block
            .extra_softap_rx_block_ack_start_sequence()
            .read()
            .sequence()
            .bits();
        let _ = block
            .extra_softap_rx_block_ack_peer_tail_and_policy()
            .read();
        let control = block.extra_softap_rx_block_ack_control().read();

        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_readback_latch().clear_bit());

        observed_peer_head == peer_head
            && peer_tail_and_policy.peer_address_tail().bits() == peer_tail
            && peer_tail_and_policy.interface().bits() == interface.bits() as u8
            && peer_tail_and_policy.window().bits() == window.get() as u8
            && peer_tail_and_policy.bits_25_31_opaque().bits() == 0
            && observed_starting_sequence == starting_sequence.get() as u16
            && control.enable().bit_is_set()
            && control.bits_1_4_opaque().bits() == 0
            && control.index().bits() == index
            && control.bits_10_11_opaque().bits() == 0
            && control.tid().bits() == tid.get() as u8
            && control.bits_16_29_opaque().bits() == 0
            && control.write().bit_is_set()
            && control.valid().bit_is_set()
    }

    /// Move one live extra-SoftAP receive BlockAck entry to a new window.
    ///
    /// SOURCE: complete `libpp[hal_ampdu.o]::hal_agreement_clr_extra_softap_rx_ba`, size `0xc6`.
    ///
    /// Despite the vendor leaf's `clr` name, this does not delete the entry.
    /// It preserves peer, interface, TID and validity, replaces the starting
    /// sequence and hardware window, clears the accumulated bitmap, and
    /// commits the staged image. The vendor's TID argument feeds only its log
    /// call and therefore does not cross this register-only boundary.
    pub fn reset_extra_softap_rx_block_ack_window(
        &mut self,
        hardware_index: MacExtraSoftApRxBlockAckEntryIndex,
        starting_sequence: MacRxBlockAckStartingSequence,
        window: MacRxBlockAckWindow,
    ) {
        let block = &self.peripherals.wifi_mac.wifi_mac_block_ack;
        let index = hardware_index.get() as u8;

        block
            .extra_softap_rx_block_ack_control()
            .modify(|_, w| w.index().set(index));
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_readback_latch().set_bit());
        let _ = block.extra_softap_rx_block_ack_control().read();
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_readback_latch().clear_bit());
        block
            .extra_softap_rx_block_ack_start_sequence()
            .modify(|_, w| w.sequence().set(starting_sequence.get() as u16));
        block
            .extra_softap_rx_block_ack_peer_tail_and_policy()
            .modify(|_, w| w.window().set(window.get() as u8));
        crate::svd::zero_register_write::clear_extra_softap_rx_block_ack_bitmap_low(block);
        crate::svd::zero_register_write::clear_extra_softap_rx_block_ack_bitmap_high(block);
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_commit().set_bit());
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_commit().clear_bit());
    }

    /// Latch and sample one indirect extra-SoftAP agreement without changing
    /// its committed contents.
    pub fn extra_softap_rx_block_ack_entry_snapshot(
        &mut self,
        hardware_index: MacExtraSoftApRxBlockAckEntryIndex,
    ) -> ExtraSoftApRxBlockAckEntrySnapshot {
        let block = &self.peripherals.wifi_mac.wifi_mac_block_ack;
        let index = hardware_index.get() as u8;
        block
            .extra_softap_rx_block_ack_control()
            .modify(|_, w| w.index().set(index));
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_readback_latch().set_bit());

        // Preserve the complete vendor diagnostic read order before sampling
        // the two adjacent staging words.
        let control = block.extra_softap_rx_block_ack_control().read();
        let peer_head = block
            .extra_softap_rx_block_ack_peer_head()
            .read()
            .peer_address_head()
            .bits();
        let peer_tail_and_policy = block
            .extra_softap_rx_block_ack_peer_tail_and_policy()
            .read();
        let starting_sequence = block
            .extra_softap_rx_block_ack_start_sequence()
            .read()
            .sequence()
            .bits();
        let _ = block
            .extra_softap_rx_block_ack_peer_head()
            .read()
            .peer_address_head()
            .bits();
        let _ = block.extra_softap_rx_block_ack_control().read();
        let bitmap_low = block
            .extra_softap_rx_block_ack_bitmap_low()
            .read()
            .bitmap()
            .bits();
        let bitmap_high = block
            .extra_softap_rx_block_ack_bitmap_high()
            .read()
            .bitmap()
            .bits();

        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_readback_latch().clear_bit());

        let head = peer_head.to_le_bytes();
        let tail = peer_tail_and_policy
            .peer_address_tail()
            .bits()
            .to_le_bytes();
        ExtraSoftApRxBlockAckEntrySnapshot {
            enabled: control.enable().bit(),
            index: control.index().bits(),
            tid: control.tid().bits(),
            write_enabled: control.write().bit(),
            valid: control.valid().bit(),
            control_unknown_clear: control.bits_1_4_opaque().bits() == 0
                && control.bits_10_11_opaque().bits() == 0
                && control.bits_16_29_opaque().bits() == 0,
            peer: [head[0], head[1], head[2], head[3], tail[0], tail[1]],
            interface: mac_interface_from_field(peer_tail_and_policy.interface().bits()),
            window: peer_tail_and_policy.window().bits(),
            starting_sequence,
            bitmap: block_ack_bitmap(bitmap_low, bitmap_high),
        }
    }

    /// Delete one extra-SoftAP receive BlockAck entry.
    ///
    /// SOURCE: complete `libpp[hal_ampdu.o]::hal_agreement_del_extra_softap_rx_ba`, size `0x94`.
    pub fn delete_extra_softap_rx_block_ack_entry(
        &mut self,
        hardware_index: MacExtraSoftApRxBlockAckEntryIndex,
    ) {
        let block = &self.peripherals.wifi_mac.wifi_mac_block_ack;
        let index = hardware_index.get() as u8;

        block
            .extra_softap_rx_block_ack_control()
            .modify(|_, w| w.index().set(index));
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_readback_latch().set_bit());
        let _ = block.extra_softap_rx_block_ack_control().read();
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_readback_latch().clear_bit());
        block
            .extra_softap_rx_block_ack_control()
            .modify(|_, w| w.valid().clear_bit());
        crate::svd::zero_register_write::clear_extra_softap_rx_block_ack_bitmap_low(block);
        crate::svd::zero_register_write::clear_extra_softap_rx_block_ack_bitmap_high(block);
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_commit().set_bit());
        block
            .rx_block_ack_agreement_update()
            .modify(|_, w| w.extra_softap_commit().clear_bit());
    }

    /// Sample both hardware-maintained and software-load words of one entry.
    pub fn rx_block_ack_entry_snapshot(
        &self,
        hardware_index: MacRxBlockAckEntryIndex,
    ) -> Option<RxBlockAckEntrySnapshot> {
        let block = &self.peripherals.wifi_mac.wifi_mac_block_ack;
        let register_index = rx_block_ack_register_index(hardware_index);
        let control = block.rx_block_ack_entry_control(register_index).read();
        let peer_head = block
            .rx_block_ack_entry_peer_head(register_index)
            .read()
            .peer_address_head()
            .bits()
            .to_le_bytes();
        let peer_tail_and_policy = block
            .rx_block_ack_entry_peer_tail_and_policy(register_index)
            .read();
        let peer_tail = peer_tail_and_policy
            .peer_address_tail()
            .bits()
            .to_le_bytes();
        let bitmap_status = block_ack_bitmap(
            block
                .rx_block_ack_entry_bitmap_low_status(register_index)
                .read()
                .bitmap()
                .bits(),
            block
                .rx_block_ack_entry_bitmap_high_status(register_index)
                .read()
                .bitmap()
                .bits(),
        );
        let bitmap_load = block_ack_bitmap(
            block
                .rx_block_ack_entry_bitmap_low_load(register_index)
                .read()
                .bitmap()
                .bits(),
            block
                .rx_block_ack_entry_bitmap_high_load(register_index)
                .read()
                .bitmap()
                .bits(),
        );
        Some(RxBlockAckEntrySnapshot {
            enabled: control.enable().bit(),
            tid: control.tid().bits(),
            write_enabled: control.write().bit(),
            valid: control.valid().bit(),
            control_unknown_clear: control.bits_1_11_opaque().bits() == 0
                && control.bits_16_29_opaque().bits() == 0,
            peer: [
                peer_head[0],
                peer_head[1],
                peer_head[2],
                peer_head[3],
                peer_tail[0],
                peer_tail[1],
            ],
            interface: mac_interface_from_field(peer_tail_and_policy.interface().bits()),
            window: peer_tail_and_policy.window().bits(),
            current_sequence: block
                .rx_block_ack_entry_current_sequence(register_index)
                .read()
                .sequence()
                .bits(),
            loaded_start_sequence: block
                .rx_block_ack_entry_start_sequence_load(register_index)
                .read()
                .sequence()
                .bits(),
            bitmap_status,
            bitmap_load,
        })
    }

    /// Sample the TX BlockAck payload and its independent validity result.
    pub(crate) fn read_tx_block_ack_observation(
        &self,
        hardware_queue: u8,
    ) -> Option<TxBlockAckObservation> {
        let block = &self.peripherals.wifi_mac.wifi_mac_block_ack;
        let payload = self.read_tx_block_ack_payload(hardware_queue)?;
        let block_ack_received = match hardware_queue {
            0 => block
                .tx_block_ack_transmitter_address_high_q0()
                .read()
                .block_ack_received()
                .bit(),
            1 => block
                .tx_block_ack_transmitter_address_high_q1()
                .read()
                .block_ack_received()
                .bit(),
            2 => block
                .tx_block_ack_transmitter_address_high_q2()
                .read()
                .block_ack_received()
                .bit(),
            3 => block
                .tx_block_ack_transmitter_address_high_q3()
                .read()
                .block_ack_received()
                .bit(),
            _ => return None,
        };
        Some(TxBlockAckObservation {
            control: payload.control(),
            starting_sequence: payload.starting_sequence(),
            bitmap: payload.bitmap(),
            block_ack_received,
        })
    }

    /// Sample one complete TX BlockAck result and its queue-information word.
    ///
    /// SOURCE: complete `libpp[hal_debug.o]::dbg_read_rx_ba`.
    /// Queue zero occupies `0x2010_5528..0x2010_5538`; logical queues descend
    /// by `0x7c` through queue seven at `0x2010_51c4..0x2010_51d4`.
    pub fn tx_block_ack_diagnostic_snapshot(
        &self,
        hardware_queue: u8,
    ) -> Option<TxBlockAckDiagnosticSnapshot> {
        let block = &self.peripherals.wifi_mac.wifi_mac_block_ack;
        macro_rules! snapshot {
            ($control:ident, $bitmap_low:ident, $bitmap_high:ident, $address_high:ident, $address_low:ident, $information:ident) => {{
                let control = block.$control().read();
                let bitmap_low = block.$bitmap_low().read().bitmap().bits();
                let bitmap_high = block.$bitmap_high().read().bitmap().bits();
                let address_high = block.$address_high().read();
                let address_low = block
                    .$address_low()
                    .read()
                    .address_bytes_0_3()
                    .bits()
                    .to_le_bytes();
                let address_tail = address_high.address_bytes_4_5().bits().to_le_bytes();
                let information = block.$information().read();
                TxBlockAckDiagnosticSnapshot {
                    control: control.tid_or_control().bits(),
                    starting_sequence: control.starting_sequence().bits(),
                    bitmap: block_ack_bitmap(bitmap_low, bitmap_high),
                    transmitter_address: [
                        address_low[0],
                        address_low[1],
                        address_low[2],
                        address_low[3],
                        address_tail[0],
                        address_tail[1],
                    ],
                    acknowledgement_tid: address_high.ack_tid().bits(),
                    acknowledgement_received: address_high.ack_received().bit(),
                    block_ack_received: address_high.block_ack_received().bit(),
                    last_tx_was_trigger_based: information.last_tx_was_trigger_based().bit(),
                    trigger_based_packet_count: information.trigger_based_packet_count().bits(),
                }
            }};
        }

        Some(match hardware_queue {
            0 => snapshot!(
                tx_block_ack_control_sequence_q0,
                tx_block_ack_bitmap_low_q0,
                tx_block_ack_bitmap_high_q0,
                tx_block_ack_transmitter_address_high_q0,
                tx_block_ack_transmitter_address_low_q0,
                tx_queue_information_q0
            ),
            1 => snapshot!(
                tx_block_ack_control_sequence_q1,
                tx_block_ack_bitmap_low_q1,
                tx_block_ack_bitmap_high_q1,
                tx_block_ack_transmitter_address_high_q1,
                tx_block_ack_transmitter_address_low_q1,
                tx_queue_information_q1
            ),
            2 => snapshot!(
                tx_block_ack_control_sequence_q2,
                tx_block_ack_bitmap_low_q2,
                tx_block_ack_bitmap_high_q2,
                tx_block_ack_transmitter_address_high_q2,
                tx_block_ack_transmitter_address_low_q2,
                tx_queue_information_q2
            ),
            3 => snapshot!(
                tx_block_ack_control_sequence_q3,
                tx_block_ack_bitmap_low_q3,
                tx_block_ack_bitmap_high_q3,
                tx_block_ack_transmitter_address_high_q3,
                tx_block_ack_transmitter_address_low_q3,
                tx_queue_information_q3
            ),
            4 => snapshot!(
                tx_block_ack_control_sequence_q4,
                tx_block_ack_bitmap_low_q4,
                tx_block_ack_bitmap_high_q4,
                tx_block_ack_transmitter_address_high_q4,
                tx_block_ack_transmitter_address_low_q4,
                tx_queue_information_q4
            ),
            5 => snapshot!(
                tx_block_ack_control_sequence_q5,
                tx_block_ack_bitmap_low_q5,
                tx_block_ack_bitmap_high_q5,
                tx_block_ack_transmitter_address_high_q5,
                tx_block_ack_transmitter_address_low_q5,
                tx_queue_information_q5
            ),
            6 => snapshot!(
                tx_block_ack_control_sequence_q6,
                tx_block_ack_bitmap_low_q6,
                tx_block_ack_bitmap_high_q6,
                tx_block_ack_transmitter_address_high_q6,
                tx_block_ack_transmitter_address_low_q6,
                tx_queue_information_q6
            ),
            7 => snapshot!(
                tx_block_ack_control_sequence_q7,
                tx_block_ack_bitmap_low_q7,
                tx_block_ack_bitmap_high_q7,
                tx_block_ack_transmitter_address_high_q7,
                tx_block_ack_transmitter_address_low_q7,
                tx_queue_information_q7
            ),
            _ => return None,
        })
    }

    /// Sample the standalone internal WDEVTXBA result.
    ///
    /// SOURCE: complete `libpp[hal_debug.o]::dbg_read_internal_txba`; the bitmap and control fields are exact. The
    /// two unqualified transmitter-address words remain evidence-only until
    /// their byte ordering is established and are not exposed by production.
    pub fn internal_tx_block_ack_snapshot(&self) -> InternalTxBlockAckSnapshot {
        let block = &self.peripherals.wifi_mac.wifi_mac_internal_tx_block_ack;
        let control = block.control_sequence().read();
        InternalTxBlockAckSnapshot {
            bitmap: block_ack_bitmap(
                block.bitmap_low().read().bitmap().bits(),
                block.bitmap_high().read().bitmap().bits(),
            ),
            fragment_number: control.fragment_number().bits(),
            starting_sequence: control.starting_sequence().bits(),
            tid: control.tid().bits(),
        }
    }
}

#[cfg(test)]
mod tests;
