//! The finite MAC BlockAck register leaves.

use oer_esp32s31_pac::{
    MacExtraSoftApRxBlockAckEntryIndex, MacInterface, MacRxBlockAckEntryIndex,
    MacRxBlockAckStartingSequence, MacRxBlockAckTid, MacRxBlockAckWindow,
    wifi_mac_block_ack::WifiMacBlockAck,
};
use oer_mmio::{Bits, Index, Placement};

use super::{MacPlacement, WifiRadioRegisters, byte, halfword};

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

/// The MAC interface of a two-bit interface field: every value is one.
const fn mac_interface_from_field(value: Bits<2>) -> MacInterface {
    match value.get() {
        0 => MacInterface::Station,
        1 => MacInterface::AccessPoint,
        2 => MacInterface::Context2,
        _ => MacInterface::Context3,
    }
}

/// The direct bank of one ordinary receive BlockAck entry: the eight banks
/// are reverse-addressed, logical index zero in the last one.
const fn rx_block_ack_bank(hardware_index: MacRxBlockAckEntryIndex) -> Option<Index<8>> {
    match 7_usize.checked_sub(hardware_index.get() as usize) {
        Some(bank) => match Index::new(bank) {
            Ok(bank) => Some(bank),
            Err(_) => None,
        },
        None => None,
    }
}

/// The agreement-update bit of one ordinary entry: one shifted by its
/// logical hardware index, not by its reverse-addressed bank.
const fn ordinary_entry_update_bit(hardware_index: MacRxBlockAckEntryIndex) -> u8 {
    match 1_u8.checked_shl(hardware_index.get()) {
        Some(bit) => bit,
        None => 0,
    }
}

/// Set an ordinary entry's agreement-update bit through one fresh-read OR.
///
/// SOURCE: complete `libpp[hal_ampdu.o]::hal_agreement_add_rx_ba` and
/// `hal_agreement_clr_rx_ba` (BLOB_LIBPP_HAL_MAC_RX_BLOCK_ACK).
fn request_rx_block_ack_entry_update<P: Placement>(
    block: &mut WifiMacBlockAck<P>,
    hardware_index: MacRxBlockAckEntryIndex,
) {
    let bit = ordinary_entry_update_bit(hardware_index);
    block.modify_rx_block_ack_agreement_update(|w| {
        let updates = w.ordinary_entry_update() | bit;
        w.set_ordinary_entry_update(updates);
    });
}

/// The active control word of one ordinary entry: every field written, as
/// the vendor's whole-word store does.
fn publish_rx_block_ack_active_control<P: Placement>(
    block: &mut WifiMacBlockAck<P>,
    bank: Index<8>,
    tid: MacRxBlockAckTid,
) {
    block.modify_rx_block_ack_entry_control(bank, |w| {
        w.set_enable(true)
            .set_bits_1_11_opaque(Bits::constant::<0>())
            .set_tid(Bits::truncate(tid.get()))
            .set_bits_16_29_opaque(Bits::constant::<0>())
            .set_write(true)
            .set_valid(true);
    });
}

/// Zero one ordinary entry's whole control word.
fn clear_rx_block_ack_entry_control<P: Placement>(block: &mut WifiMacBlockAck<P>, bank: Index<8>) {
    block.modify_rx_block_ack_entry_control(bank, |w| {
        w.set_enable(false)
            .set_bits_1_11_opaque(Bits::constant::<0>())
            .set_tid(Bits::constant::<0>())
            .set_bits_16_29_opaque(Bits::constant::<0>())
            .set_write(false)
            .set_valid(false);
    });
}

/// Zero both bitmap load words of one ordinary entry.
fn clear_rx_block_ack_bitmap_load<P: Placement>(block: &mut WifiMacBlockAck<P>, bank: Index<8>) {
    block.modify_rx_block_ack_entry_bitmap_low_load(bank, |w| {
        w.set_bitmap(0);
    });
    block.modify_rx_block_ack_entry_bitmap_high_load(bank, |w| {
        w.set_bitmap(0);
    });
}

/// Zero both extra-SoftAP staging bitmap words.
fn clear_extra_softap_rx_block_ack_bitmap<P: Placement>(block: &mut WifiMacBlockAck<P>) {
    block.modify_extra_softap_rx_block_ack_bitmap_low(|w| {
        w.set_bitmap(0);
    });
    block.modify_extra_softap_rx_block_ack_bitmap_high(|w| {
        w.set_bitmap(0);
    });
}

/// Pulse the extra-SoftAP commit bit: set, then clear, each a fresh-read RMW.
fn commit_extra_softap<P: Placement>(block: &mut WifiMacBlockAck<P>) {
    block.modify_rx_block_ack_agreement_update(|w| {
        w.set_extra_softap_commit(true);
    });
    block.modify_rx_block_ack_agreement_update(|w| {
        w.set_extra_softap_commit(false);
    });
}

fn set_extra_softap_readback_latch<P: Placement>(block: &mut WifiMacBlockAck<P>, latched: bool) {
    block.modify_rx_block_ack_agreement_update(|w| {
        w.set_extra_softap_readback_latch(latched);
    });
}

/// The staging index field of an extra-SoftAP entry.
const fn extra_softap_index(hardware_index: MacExtraSoftApRxBlockAckEntryIndex) -> Bits<5> {
    Bits::truncate(hardware_index.get())
}

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Sample the three words copied by `hal_mac_tx_get_blockack`.
    pub fn read_tx_block_ack_payload(&self, hardware_queue: u8) -> Option<TxBlockAckPayload> {
        let block = &self.mac.wifi_mac_block_ack;
        macro_rules! payload {
            ($control:ident, $low:ident, $high:ident) => {{
                let control = block.$control();
                (
                    byte(control.tid_or_control()),
                    halfword(control.starting_sequence()),
                    block.$low().bitmap(),
                    block.$high().bitmap(),
                )
            }};
        }
        let (control, starting_sequence, bitmap_low, bitmap_high) = match hardware_queue {
            0 => payload!(
                read_tx_block_ack_control_sequence_q0,
                read_tx_block_ack_bitmap_low_q0,
                read_tx_block_ack_bitmap_high_q0
            ),
            1 => payload!(
                read_tx_block_ack_control_sequence_q1,
                read_tx_block_ack_bitmap_low_q1,
                read_tx_block_ack_bitmap_high_q1
            ),
            2 => payload!(
                read_tx_block_ack_control_sequence_q2,
                read_tx_block_ack_bitmap_low_q2,
                read_tx_block_ack_bitmap_high_q2
            ),
            3 => payload!(
                read_tx_block_ack_control_sequence_q3,
                read_tx_block_ack_bitmap_low_q3,
                read_tx_block_ack_bitmap_high_q3
            ),
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
        let Some(bank) = rx_block_ack_bank(hardware_index) else {
            return;
        };
        let block = &mut self.mac.wifi_mac_block_ack;
        let [p0, p1, p2, p3, p4, p5] = peer;

        // The vendor's whole-word stores of the peer words: every field.
        block.modify_rx_block_ack_entry_peer_head(bank, |w| {
            w.set_peer_address_head(u32::from_le_bytes([p0, p1, p2, p3]));
        });
        block.modify_rx_block_ack_entry_peer_tail_and_policy(bank, |w| {
            w.set_peer_address_tail(u16::from_le_bytes([p4, p5]))
                .set_interface(Bits::truncate(interface.bits()))
                .set_window(Bits::truncate(window.get()))
                .set_bits_25_31_opaque(Bits::constant::<0>());
        });
        block.modify_rx_block_ack_entry_start_sequence_load(bank, |w| {
            w.set_sequence(Bits::truncate(starting_sequence.get()));
        });
        block.modify_rx_block_ack_entry_control(bank, |w| {
            w.set_valid(false);
        });
        clear_rx_block_ack_bitmap_load(block, bank);
        request_rx_block_ack_entry_update(block, hardware_index);
        publish_rx_block_ack_active_control(block, bank, tid);
    }

    /// Delete one ordinary receive BlockAck entry.
    ///
    /// SOURCE: complete `libpp[hal_ampdu.o]::hal_agreement_del_rx_ba`, size `0x72`.
    pub fn delete_rx_block_ack_entry(&mut self, hardware_index: MacRxBlockAckEntryIndex) {
        let Some(bank) = rx_block_ack_bank(hardware_index) else {
            return;
        };
        let block = &mut self.mac.wifi_mac_block_ack;
        block.modify_rx_block_ack_entry_control(bank, |w| {
            w.set_valid(false);
        });
        clear_rx_block_ack_bitmap_load(block, bank);
        block.modify_rx_block_ack_entry_control(bank, |w| {
            w.set_valid(true);
        });
        // The final full-word zero is a distinct observable edge in the blob.
        clear_rx_block_ack_entry_control(block, bank);
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
        let Some(bank) = rx_block_ack_bank(hardware_index) else {
            return;
        };
        let block = &mut self.mac.wifi_mac_block_ack;

        clear_rx_block_ack_entry_control(block, bank);
        block.modify_rx_block_ack_entry_start_sequence_load(bank, |w| {
            w.set_sequence(Bits::truncate(starting_sequence.get()));
        });
        block.modify_rx_block_ack_entry_peer_tail_and_policy(bank, |w| {
            w.set_window(Bits::truncate(window.get()));
        });
        clear_rx_block_ack_bitmap_load(block, bank);
        request_rx_block_ack_entry_update(block, hardware_index);
        publish_rx_block_ack_active_control(block, bank, tid);
    }

    /// Program one extra-SoftAP receive BlockAck entry through the shared
    /// staging window.
    ///
    /// SOURCE: complete `libpp[hal_ampdu.o]::hal_agreement_add_extra_softap_rx_ba`, size `0x130`.
    ///
    /// This transaction is distinct from the eight direct ordinary banks.
    /// The selected entry is staged, committed through update bit eight,
    /// latched for the vendor's readback sequence through bit nine, and then
    /// released. The diagnostic log call is intentionally left out; its
    /// ordered volatile readback is retained here.
    pub fn program_extra_softap_rx_block_ack_entry(
        &mut self,
        hardware_index: MacExtraSoftApRxBlockAckEntryIndex,
        interface: MacInterface,
        peer: [u8; 6],
        tid: MacRxBlockAckTid,
        starting_sequence: MacRxBlockAckStartingSequence,
        window: MacRxBlockAckWindow,
    ) -> bool {
        let block = &mut self.mac.wifi_mac_block_ack;
        let index = extra_softap_index(hardware_index);
        let [p0, p1, p2, p3, p4, p5] = peer;
        let peer_head = u32::from_le_bytes([p0, p1, p2, p3]);
        let peer_tail = u16::from_le_bytes([p4, p5]);
        let interface_field = Bits::<2>::truncate(interface.bits());
        let window_field = Bits::<7>::truncate(window.get());
        let tid_field = Bits::<4>::truncate(tid.get());
        let sequence_field = Bits::<12>::truncate(starting_sequence.get());

        block.modify_extra_softap_rx_block_ack_control(|w| {
            w.set_index(index);
        });
        // The vendor's whole-word stores of the staged words: every field.
        block.modify_extra_softap_rx_block_ack_peer_head(|w| {
            w.set_peer_address_head(peer_head);
        });
        block.modify_extra_softap_rx_block_ack_peer_tail_and_policy(|w| {
            w.set_peer_address_tail(peer_tail)
                .set_interface(interface_field)
                .set_window(window_field)
                .set_bits_25_31_opaque(Bits::constant::<0>());
        });
        block.modify_extra_softap_rx_block_ack_start_sequence(|w| {
            w.set_sequence(sequence_field);
        });
        clear_extra_softap_rx_block_ack_bitmap(block);
        block.modify_extra_softap_rx_block_ack_control(|w| {
            w.set_enable(true)
                .set_bits_1_4_opaque(Bits::constant::<0>())
                .set_index(index)
                .set_bits_10_11_opaque(Bits::constant::<0>())
                .set_tid(tid_field)
                .set_bits_16_29_opaque(Bits::constant::<0>())
                .set_write(true)
                .set_valid(true);
        });

        commit_extra_softap(block);
        set_extra_softap_readback_latch(block, true);

        // Preserve the complete vendor readback order. The values feed only
        // wifi_log in the blob and therefore leave this boundary only as the
        // returned agreement check.
        let peer_tail_and_policy = block.read_extra_softap_rx_block_ack_peer_tail_and_policy();
        let observed_peer_head = block
            .read_extra_softap_rx_block_ack_peer_head()
            .peer_address_head();
        let observed_starting_sequence = block.read_extra_softap_rx_block_ack_start_sequence().sequence();
        let _ = block.read_extra_softap_rx_block_ack_peer_tail_and_policy();
        let control = block.read_extra_softap_rx_block_ack_control();

        set_extra_softap_readback_latch(block, false);

        observed_peer_head == peer_head
            && peer_tail_and_policy.peer_address_tail() == peer_tail
            && peer_tail_and_policy.interface() == interface_field
            && peer_tail_and_policy.window() == window_field
            && peer_tail_and_policy.bits_25_31_opaque().get() == 0
            && observed_starting_sequence == sequence_field
            && control.enable()
            && control.bits_1_4_opaque().get() == 0
            && control.index() == index
            && control.bits_10_11_opaque().get() == 0
            && control.tid() == tid_field
            && control.bits_16_29_opaque().get() == 0
            && control.write()
            && control.valid()
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
        let block = &mut self.mac.wifi_mac_block_ack;
        let index = extra_softap_index(hardware_index);

        block.modify_extra_softap_rx_block_ack_control(|w| {
            w.set_index(index);
        });
        set_extra_softap_readback_latch(block, true);
        let _ = block.read_extra_softap_rx_block_ack_control();
        set_extra_softap_readback_latch(block, false);
        block.modify_extra_softap_rx_block_ack_start_sequence(|w| {
            w.set_sequence(Bits::truncate(starting_sequence.get()));
        });
        block.modify_extra_softap_rx_block_ack_peer_tail_and_policy(|w| {
            w.set_window(Bits::truncate(window.get()));
        });
        clear_extra_softap_rx_block_ack_bitmap(block);
        commit_extra_softap(block);
    }

    /// Latch and sample one indirect extra-SoftAP agreement without changing
    /// its committed contents.
    pub fn extra_softap_rx_block_ack_entry_snapshot(
        &mut self,
        hardware_index: MacExtraSoftApRxBlockAckEntryIndex,
    ) -> ExtraSoftApRxBlockAckEntrySnapshot {
        let block = &mut self.mac.wifi_mac_block_ack;
        let index = extra_softap_index(hardware_index);
        block.modify_extra_softap_rx_block_ack_control(|w| {
            w.set_index(index);
        });
        set_extra_softap_readback_latch(block, true);

        // Preserve the complete vendor diagnostic read order before sampling
        // the two adjacent staging words.
        let control = block.read_extra_softap_rx_block_ack_control();
        let peer_head = block
            .read_extra_softap_rx_block_ack_peer_head()
            .peer_address_head();
        let peer_tail_and_policy = block.read_extra_softap_rx_block_ack_peer_tail_and_policy();
        let starting_sequence = block.read_extra_softap_rx_block_ack_start_sequence().sequence();
        let _ = block.read_extra_softap_rx_block_ack_peer_head();
        let _ = block.read_extra_softap_rx_block_ack_control();
        let bitmap_low = block.read_extra_softap_rx_block_ack_bitmap_low().bitmap();
        let bitmap_high = block.read_extra_softap_rx_block_ack_bitmap_high().bitmap();

        set_extra_softap_readback_latch(block, false);

        let [h0, h1, h2, h3] = peer_head.to_le_bytes();
        let [t0, t1] = peer_tail_and_policy.peer_address_tail().to_le_bytes();
        ExtraSoftApRxBlockAckEntrySnapshot {
            enabled: control.enable(),
            index: byte(control.index()),
            tid: byte(control.tid()),
            write_enabled: control.write(),
            valid: control.valid(),
            control_unknown_clear: control.bits_1_4_opaque().get() == 0
                && control.bits_10_11_opaque().get() == 0
                && control.bits_16_29_opaque().get() == 0,
            peer: [h0, h1, h2, h3, t0, t1],
            interface: mac_interface_from_field(peer_tail_and_policy.interface()),
            window: byte(peer_tail_and_policy.window()),
            starting_sequence: halfword(starting_sequence),
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
        let block = &mut self.mac.wifi_mac_block_ack;
        let index = extra_softap_index(hardware_index);

        block.modify_extra_softap_rx_block_ack_control(|w| {
            w.set_index(index);
        });
        set_extra_softap_readback_latch(block, true);
        let _ = block.read_extra_softap_rx_block_ack_control();
        set_extra_softap_readback_latch(block, false);
        block.modify_extra_softap_rx_block_ack_control(|w| {
            w.set_valid(false);
        });
        clear_extra_softap_rx_block_ack_bitmap(block);
        commit_extra_softap(block);
    }

    /// Sample both hardware-maintained and software-load words of one entry.
    pub fn rx_block_ack_entry_snapshot(
        &self,
        hardware_index: MacRxBlockAckEntryIndex,
    ) -> Option<RxBlockAckEntrySnapshot> {
        let bank = rx_block_ack_bank(hardware_index)?;
        let block = &self.mac.wifi_mac_block_ack;
        let control = block.read_rx_block_ack_entry_control(bank);
        let [h0, h1, h2, h3] = block
            .read_rx_block_ack_entry_peer_head(bank)
            .peer_address_head()
            .to_le_bytes();
        let peer_tail_and_policy = block.read_rx_block_ack_entry_peer_tail_and_policy(bank);
        let [t0, t1] = peer_tail_and_policy.peer_address_tail().to_le_bytes();
        let bitmap_status = block_ack_bitmap(
            block.read_rx_block_ack_entry_bitmap_low_status(bank).bitmap(),
            block.read_rx_block_ack_entry_bitmap_high_status(bank).bitmap(),
        );
        let bitmap_load = block_ack_bitmap(
            block.read_rx_block_ack_entry_bitmap_low_load(bank).bitmap(),
            block.read_rx_block_ack_entry_bitmap_high_load(bank).bitmap(),
        );
        Some(RxBlockAckEntrySnapshot {
            enabled: control.enable(),
            tid: byte(control.tid()),
            write_enabled: control.write(),
            valid: control.valid(),
            control_unknown_clear: control.bits_1_11_opaque().get() == 0
                && control.bits_16_29_opaque().get() == 0,
            peer: [h0, h1, h2, h3, t0, t1],
            interface: mac_interface_from_field(peer_tail_and_policy.interface()),
            window: byte(peer_tail_and_policy.window()),
            current_sequence: halfword(block.read_rx_block_ack_entry_current_sequence(bank).sequence()),
            loaded_start_sequence: halfword(
                block.read_rx_block_ack_entry_start_sequence_load(bank).sequence(),
            ),
            bitmap_status,
            bitmap_load,
        })
    }

    /// Sample the TX BlockAck payload and its independent validity result.
    pub(crate) fn read_tx_block_ack_observation(
        &self,
        hardware_queue: u8,
    ) -> Option<TxBlockAckObservation> {
        let block = &self.mac.wifi_mac_block_ack;
        let payload = self.read_tx_block_ack_payload(hardware_queue)?;
        let block_ack_received = match hardware_queue {
            0 => block.read_tx_block_ack_transmitter_address_high_q0().block_ack_received(),
            1 => block.read_tx_block_ack_transmitter_address_high_q1().block_ack_received(),
            2 => block.read_tx_block_ack_transmitter_address_high_q2().block_ack_received(),
            3 => block.read_tx_block_ack_transmitter_address_high_q3().block_ack_received(),
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
        let block = &self.mac.wifi_mac_block_ack;
        macro_rules! snapshot {
            ($control:ident, $bitmap_low:ident, $bitmap_high:ident, $address_high:ident, $address_low:ident, $information:ident) => {{
                let control = block.$control();
                let bitmap_low = block.$bitmap_low().bitmap();
                let bitmap_high = block.$bitmap_high().bitmap();
                let address_high = block.$address_high();
                let [a0, a1, a2, a3] = block.$address_low().address_bytes_0_3().to_le_bytes();
                let [a4, a5] = address_high.address_bytes_4_5().to_le_bytes();
                let information = block.$information();
                TxBlockAckDiagnosticSnapshot {
                    control: byte(control.tid_or_control()),
                    starting_sequence: halfword(control.starting_sequence()),
                    bitmap: block_ack_bitmap(bitmap_low, bitmap_high),
                    transmitter_address: [a0, a1, a2, a3, a4, a5],
                    acknowledgement_tid: byte(address_high.ack_tid()),
                    acknowledgement_received: address_high.ack_received(),
                    block_ack_received: address_high.block_ack_received(),
                    last_tx_was_trigger_based: information.last_tx_was_trigger_based(),
                    trigger_based_packet_count: byte(information.trigger_based_packet_count()),
                }
            }};
        }

        Some(match hardware_queue {
            0 => snapshot!(
                read_tx_block_ack_control_sequence_q0,
                read_tx_block_ack_bitmap_low_q0,
                read_tx_block_ack_bitmap_high_q0,
                read_tx_block_ack_transmitter_address_high_q0,
                read_tx_block_ack_transmitter_address_low_q0,
                read_tx_queue_information_q0
            ),
            1 => snapshot!(
                read_tx_block_ack_control_sequence_q1,
                read_tx_block_ack_bitmap_low_q1,
                read_tx_block_ack_bitmap_high_q1,
                read_tx_block_ack_transmitter_address_high_q1,
                read_tx_block_ack_transmitter_address_low_q1,
                read_tx_queue_information_q1
            ),
            2 => snapshot!(
                read_tx_block_ack_control_sequence_q2,
                read_tx_block_ack_bitmap_low_q2,
                read_tx_block_ack_bitmap_high_q2,
                read_tx_block_ack_transmitter_address_high_q2,
                read_tx_block_ack_transmitter_address_low_q2,
                read_tx_queue_information_q2
            ),
            3 => snapshot!(
                read_tx_block_ack_control_sequence_q3,
                read_tx_block_ack_bitmap_low_q3,
                read_tx_block_ack_bitmap_high_q3,
                read_tx_block_ack_transmitter_address_high_q3,
                read_tx_block_ack_transmitter_address_low_q3,
                read_tx_queue_information_q3
            ),
            4 => snapshot!(
                read_tx_block_ack_control_sequence_q4,
                read_tx_block_ack_bitmap_low_q4,
                read_tx_block_ack_bitmap_high_q4,
                read_tx_block_ack_transmitter_address_high_q4,
                read_tx_block_ack_transmitter_address_low_q4,
                read_tx_queue_information_q4
            ),
            5 => snapshot!(
                read_tx_block_ack_control_sequence_q5,
                read_tx_block_ack_bitmap_low_q5,
                read_tx_block_ack_bitmap_high_q5,
                read_tx_block_ack_transmitter_address_high_q5,
                read_tx_block_ack_transmitter_address_low_q5,
                read_tx_queue_information_q5
            ),
            6 => snapshot!(
                read_tx_block_ack_control_sequence_q6,
                read_tx_block_ack_bitmap_low_q6,
                read_tx_block_ack_bitmap_high_q6,
                read_tx_block_ack_transmitter_address_high_q6,
                read_tx_block_ack_transmitter_address_low_q6,
                read_tx_queue_information_q6
            ),
            7 => snapshot!(
                read_tx_block_ack_control_sequence_q7,
                read_tx_block_ack_bitmap_low_q7,
                read_tx_block_ack_bitmap_high_q7,
                read_tx_block_ack_transmitter_address_high_q7,
                read_tx_block_ack_transmitter_address_low_q7,
                read_tx_queue_information_q7
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
        let block = &self.mac.wifi_mac_internal_tx_block_ack;
        let control = block.read_control_sequence();
        InternalTxBlockAckSnapshot {
            bitmap: block_ack_bitmap(block.read_bitmap_low().bitmap(), block.read_bitmap_high().bitmap()),
            fragment_number: byte(control.fragment_number()),
            starting_sequence: halfword(control.starting_sequence()),
            tid: byte(control.tid()),
        }
    }
}

#[cfg(test)]
mod tests;
