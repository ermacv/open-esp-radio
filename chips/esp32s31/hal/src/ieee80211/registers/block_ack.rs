//! The finite MAC BlockAck register leaves.

use oer_esp32s31_pac::wifi_mac::WifiMacBlockAck;
use oer_mmio::{Bits, Index, Placement};

use super::{
    MacExtraSoftApRxBlockAckEntryIndex, MacInterface, MacPlacement, MacRxBlockAckEntryIndex,
    MacRxBlockAckStartingSequence, MacRxBlockAckTid, MacRxBlockAckWindow, WifiRadioRegisters, bits,
    byte, element, halfword,
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

/// The element of the ordinary receive BlockAck arrays that holds the
/// vendor's entry `hardware_index`: the banks are reverse-addressed, so
/// element `i` is the vendor's entry `7 − i`. This is the one place the
/// index is translated.
const fn rx_block_ack_element(hardware_index: MacRxBlockAckEntryIndex) -> Index<8> {
    element::<8>(7 - hardware_index.get() as usize)
}

/// The bit of `ORDINARY_ENTRY_UPDATE` that publishes the vendor's entry
/// `hardware_index`: the update field counts entries in vendor order.
const fn rx_block_ack_update_bit(hardware_index: MacRxBlockAckEntryIndex) -> u8 {
    1 << hardware_index.get()
}

/// The extra-SoftAP staging index field of a bounded entry index.
const fn extra_softap_index(hardware_index: MacExtraSoftApRxBlockAckEntryIndex) -> Bits<5> {
    bits(hardware_index.get())
}

/// The one-way peer split of a 48-bit address: four head bytes, two tail.
const fn peer_words(peer: [u8; 6]) -> (u32, u16) {
    let [a, b, c, d, e, f] = peer;
    (u32::from_le_bytes([a, b, c, d]), u16::from_le_bytes([e, f]))
}

/// The zero-based `rx_block_ack_peer_head`, `rx_block_ack_peer_tail_and_policy`
/// stores of one ordinary entry: peer head, then peer tail with interface
/// and window, bits 31:25 zero.
fn store_rx_block_ack_peer<P: Placement>(
    block: &mut WifiMacBlockAck<P>,
    entry: Index<8>,
    interface: MacInterface,
    peer: [u8; 6],
    window: MacRxBlockAckWindow,
) {
    let (head, tail) = peer_words(peer);
    block.modify_rx_block_ack_entry_peer_head(entry, |w| {
        w.set_peer_address_head(head);
    });
    block.modify_rx_block_ack_entry_peer_tail_and_policy(entry, |w| {
        w.set_peer_address_tail(tail)
            .set_interface(interface.field())
            .set_window(bits(window.get()))
            .set_bits_25_31_opaque(bits(0));
    });
}

/// Zero both bitmap load words of one ordinary entry: the
/// `clear_rx_block_ack_bitmap_low_load` and `..._high_load` zero writes.
fn clear_rx_block_ack_bitmap_load<P: Placement>(block: &mut WifiMacBlockAck<P>, entry: Index<8>) {
    block.modify_rx_block_ack_entry_bitmap_low_load(entry, |w| {
        w.set_bitmap(0);
    });
    block.modify_rx_block_ack_entry_bitmap_high_load(entry, |w| {
        w.set_bitmap(0);
    });
}

/// The `clear_rx_block_ack_entry_control` zero write of one ordinary entry.
fn clear_rx_block_ack_entry_control<P: Placement>(block: &mut WifiMacBlockAck<P>, entry: Index<8>) {
    block.modify_rx_block_ack_entry_control(entry, |w| {
        w.set_enable(false)
            .set_bits_1_11_opaque(bits(0))
            .set_tid(bits(0))
            .set_bits_16_29_opaque(bits(0))
            .set_write(false)
            .set_valid(false);
    });
}

/// The `request_rx_block_ack_entry_update` indexed bit set.
fn request_rx_block_ack_entry_update<P: Placement>(
    block: &mut WifiMacBlockAck<P>,
    hardware_index: MacRxBlockAckEntryIndex,
) {
    let bit = rx_block_ack_update_bit(hardware_index);
    block.modify_rx_block_ack_agreement_update(|w| {
        let update = w.ordinary_entry_update();
        w.set_ordinary_entry_update(update | bit);
    });
}

/// The zero-based `rx_block_ack_active_control` store: enable, TID, write
/// and valid set, every opaque field zero.
fn publish_rx_block_ack_active_control<P: Placement>(
    block: &mut WifiMacBlockAck<P>,
    entry: Index<8>,
    tid: MacRxBlockAckTid,
) {
    block.modify_rx_block_ack_entry_control(entry, |w| {
        w.set_enable(true)
            .set_bits_1_11_opaque(bits(0))
            .set_tid(bits(tid.get()))
            .set_bits_16_29_opaque(bits(0))
            .set_write(true)
            .set_valid(true);
    });
}

/// Zero both extra-SoftAP staging bitmap words: the
/// `clear_extra_softap_rx_block_ack_bitmap_low` and `..._high` zero writes.
fn clear_extra_softap_bitmap<P: Placement>(block: &mut WifiMacBlockAck<P>) {
    block.modify_extra_softap_rx_block_ack_bitmap_low(|w| {
        w.set_bitmap(0);
    });
    block.modify_extra_softap_rx_block_ack_bitmap_high(|w| {
        w.set_bitmap(0);
    });
}

/// Select one extra-SoftAP entry in the staging window.
fn select_extra_softap_entry<P: Placement>(block: &mut WifiMacBlockAck<P>, index: Bits<5>) {
    block.modify_extra_softap_rx_block_ack_control(|w| {
        w.set_index(index);
    });
}

/// Set or release the extra-SoftAP readback latch.
fn set_extra_softap_latch<P: Placement>(block: &mut WifiMacBlockAck<P>, latched: bool) {
    block.modify_rx_block_ack_agreement_update(|w| {
        w.set_extra_softap_readback_latch(latched);
    });
}

/// Pulse the extra-SoftAP commit bit: set, then clear.
fn commit_extra_softap_entry<P: Placement>(block: &mut WifiMacBlockAck<P>) {
    block.modify_rx_block_ack_agreement_update(|w| {
        w.set_extra_softap_commit(true);
    });
    block.modify_rx_block_ack_agreement_update(|w| {
        w.set_extra_softap_commit(false);
    });
}

/// The three words copied by `hal_mac_tx_get_blockack` for queues zero to
/// three: control, starting sequence and the bitmap's two halves.
macro_rules! tx_block_ack_words {
    ($block:expr, $control:ident, $bitmap_low:ident, $bitmap_high:ident) => {{
        let control = $block.$control();
        (
            byte(control.tid_or_control()),
            halfword(control.starting_sequence()),
            $block.$bitmap_low().bitmap(),
            $block.$bitmap_high().bitmap(),
        )
    }};
}

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Sample the three words copied by `hal_mac_tx_get_blockack`.
    pub fn read_tx_block_ack_payload(&self, hardware_queue: u8) -> Option<TxBlockAckPayload> {
        let block = &self.mac.wifi_mac_block_ack;
        let (control, starting_sequence, bitmap_low, bitmap_high) = match hardware_queue {
            0 => tx_block_ack_words!(
                block,
                read_tx_block_ack_control_sequence_q0,
                read_tx_block_ack_bitmap_low_q0,
                read_tx_block_ack_bitmap_high_q0
            ),
            1 => tx_block_ack_words!(
                block,
                read_tx_block_ack_control_sequence_q1,
                read_tx_block_ack_bitmap_low_q1,
                read_tx_block_ack_bitmap_high_q1
            ),
            2 => tx_block_ack_words!(
                block,
                read_tx_block_ack_control_sequence_q2,
                read_tx_block_ack_bitmap_low_q2,
                read_tx_block_ack_bitmap_high_q2
            ),
            3 => tx_block_ack_words!(
                block,
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
        let block = &mut self.mac.wifi_mac_block_ack;
        let entry = rx_block_ack_element(hardware_index);
        store_rx_block_ack_peer(block, entry, interface, peer, window);
        block.modify_rx_block_ack_entry_start_sequence_load(entry, |w| {
            w.set_sequence(bits(starting_sequence.get()));
        });
        block.modify_rx_block_ack_entry_control(entry, |w| {
            w.set_valid(false);
        });
        clear_rx_block_ack_bitmap_load(block, entry);
        request_rx_block_ack_entry_update(block, hardware_index);
        publish_rx_block_ack_active_control(block, entry, tid);
    }

    /// Delete one ordinary receive BlockAck entry.
    ///
    /// SOURCE: complete `libpp[hal_ampdu.o]::hal_agreement_del_rx_ba`, size `0x72`.
    pub fn delete_rx_block_ack_entry(&mut self, hardware_index: MacRxBlockAckEntryIndex) {
        let block = &mut self.mac.wifi_mac_block_ack;
        let entry = rx_block_ack_element(hardware_index);
        block.modify_rx_block_ack_entry_control(entry, |w| {
            w.set_valid(false);
        });
        clear_rx_block_ack_bitmap_load(block, entry);
        block.modify_rx_block_ack_entry_control(entry, |w| {
            w.set_valid(true);
        });
        // The final full-word zero is a distinct observable edge in the blob.
        clear_rx_block_ack_entry_control(block, entry);
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
        let block = &mut self.mac.wifi_mac_block_ack;
        let entry = rx_block_ack_element(hardware_index);
        clear_rx_block_ack_entry_control(block, entry);
        block.modify_rx_block_ack_entry_start_sequence_load(entry, |w| {
            w.set_sequence(bits(starting_sequence.get()));
        });
        block.modify_rx_block_ack_entry_peer_tail_and_policy(entry, |w| {
            w.set_window(bits(window.get()));
        });
        clear_rx_block_ack_bitmap_load(block, entry);
        request_rx_block_ack_entry_update(block, hardware_index);
        publish_rx_block_ack_active_control(block, entry, tid);
    }

    /// Program one extra-SoftAP receive BlockAck entry through the shared
    /// staging window.
    ///
    /// SOURCE: complete `libpp[hal_ampdu.o]::hal_agreement_add_extra_softap_rx_ba`, size `0x130`.
    ///
    /// This transaction is distinct from the eight direct ordinary banks.
    /// The selected entry is staged, committed through update bit eight,
    /// latched for the vendor's readback sequence through bit nine, and then
    /// released. The diagnostic log call is intentionally outside the HAL;
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
        let block = &mut self.mac.wifi_mac_block_ack;
        let index = extra_softap_index(hardware_index);
        let (peer_head, peer_tail) = peer_words(peer);

        select_extra_softap_entry(block, index);
        block.modify_extra_softap_rx_block_ack_peer_head(|w| {
            w.set_peer_address_head(peer_head);
        });
        block.modify_extra_softap_rx_block_ack_peer_tail_and_policy(|w| {
            w.set_peer_address_tail(peer_tail)
                .set_interface(interface.field())
                .set_window(bits(window.get()))
                .set_bits_25_31_opaque(bits(0));
        });
        block.modify_extra_softap_rx_block_ack_start_sequence(|w| {
            w.set_sequence(bits(starting_sequence.get()));
        });
        clear_extra_softap_bitmap(block);
        // The zero-based `extra_softap_rx_block_ack_active_control` store.
        block.modify_extra_softap_rx_block_ack_control(|w| {
            w.set_enable(true)
                .set_bits_1_4_opaque(bits(0))
                .set_index(index)
                .set_bits_10_11_opaque(bits(0))
                .set_tid(bits(tid.get()))
                .set_bits_16_29_opaque(bits(0))
                .set_write(true)
                .set_valid(true);
        });

        commit_extra_softap_entry(block);
        set_extra_softap_latch(block, true);

        // Preserve the complete vendor readback order. The values feed only
        // wifi_log in the blob and therefore do not cross this boundary.
        let peer_tail_and_policy = block.read_extra_softap_rx_block_ack_peer_tail_and_policy();
        let observed_peer_head = block
            .read_extra_softap_rx_block_ack_peer_head()
            .peer_address_head();
        let observed_starting_sequence = block
            .read_extra_softap_rx_block_ack_start_sequence()
            .sequence();
        let _ = block.read_extra_softap_rx_block_ack_peer_tail_and_policy();
        let control = block.read_extra_softap_rx_block_ack_control();

        set_extra_softap_latch(block, false);

        observed_peer_head == peer_head
            && peer_tail_and_policy.peer_address_tail() == peer_tail
            && peer_tail_and_policy.interface() == interface.field()
            && peer_tail_and_policy.window().get() == window.get()
            && peer_tail_and_policy.bits_25_31_opaque().get() == 0
            && observed_starting_sequence.get() == starting_sequence.get()
            && control.enable()
            && control.bits_1_4_opaque().get() == 0
            && control.index() == index
            && control.bits_10_11_opaque().get() == 0
            && control.tid().get() == tid.get()
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
        select_extra_softap_entry(block, extra_softap_index(hardware_index));
        set_extra_softap_latch(block, true);
        let _ = block.read_extra_softap_rx_block_ack_control();
        set_extra_softap_latch(block, false);
        block.modify_extra_softap_rx_block_ack_start_sequence(|w| {
            w.set_sequence(bits(starting_sequence.get()));
        });
        block.modify_extra_softap_rx_block_ack_peer_tail_and_policy(|w| {
            w.set_window(bits(window.get()));
        });
        clear_extra_softap_bitmap(block);
        commit_extra_softap_entry(block);
    }

    /// Latch and sample one indirect extra-SoftAP agreement without changing
    /// its committed contents.
    pub fn extra_softap_rx_block_ack_entry_snapshot(
        &mut self,
        hardware_index: MacExtraSoftApRxBlockAckEntryIndex,
    ) -> ExtraSoftApRxBlockAckEntrySnapshot {
        let block = &mut self.mac.wifi_mac_block_ack;
        select_extra_softap_entry(block, extra_softap_index(hardware_index));
        set_extra_softap_latch(block, true);

        // Preserve the complete vendor diagnostic read order before sampling
        // the two adjacent staging words.
        let control = block.read_extra_softap_rx_block_ack_control();
        let peer_head = block
            .read_extra_softap_rx_block_ack_peer_head()
            .peer_address_head();
        let peer_tail_and_policy = block.read_extra_softap_rx_block_ack_peer_tail_and_policy();
        let starting_sequence = block
            .read_extra_softap_rx_block_ack_start_sequence()
            .sequence();
        let _ = block.read_extra_softap_rx_block_ack_peer_head();
        let _ = block.read_extra_softap_rx_block_ack_control();
        let bitmap_low = block.read_extra_softap_rx_block_ack_bitmap_low().bitmap();
        let bitmap_high = block.read_extra_softap_rx_block_ack_bitmap_high().bitmap();

        set_extra_softap_latch(block, false);

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
            interface: MacInterface::from_field(peer_tail_and_policy.interface()),
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
        select_extra_softap_entry(block, extra_softap_index(hardware_index));
        set_extra_softap_latch(block, true);
        let _ = block.read_extra_softap_rx_block_ack_control();
        set_extra_softap_latch(block, false);
        block.modify_extra_softap_rx_block_ack_control(|w| {
            w.set_valid(false);
        });
        clear_extra_softap_bitmap(block);
        commit_extra_softap_entry(block);
    }

    /// Sample both hardware-maintained and software-load words of one entry.
    pub fn rx_block_ack_entry_snapshot(
        &self,
        hardware_index: MacRxBlockAckEntryIndex,
    ) -> Option<RxBlockAckEntrySnapshot> {
        let block = &self.mac.wifi_mac_block_ack;
        let entry = rx_block_ack_element(hardware_index);
        let control = block.read_rx_block_ack_entry_control(entry);
        let [h0, h1, h2, h3] = block
            .read_rx_block_ack_entry_peer_head(entry)
            .peer_address_head()
            .to_le_bytes();
        let peer_tail_and_policy = block.read_rx_block_ack_entry_peer_tail_and_policy(entry);
        let [t0, t1] = peer_tail_and_policy.peer_address_tail().to_le_bytes();
        let bitmap_status = block_ack_bitmap(
            block
                .read_rx_block_ack_entry_bitmap_low_status(entry)
                .bitmap(),
            block
                .read_rx_block_ack_entry_bitmap_high_status(entry)
                .bitmap(),
        );
        let bitmap_load = block_ack_bitmap(
            block
                .read_rx_block_ack_entry_bitmap_low_load(entry)
                .bitmap(),
            block
                .read_rx_block_ack_entry_bitmap_high_load(entry)
                .bitmap(),
        );
        Some(RxBlockAckEntrySnapshot {
            enabled: control.enable(),
            tid: byte(control.tid()),
            write_enabled: control.write(),
            valid: control.valid(),
            control_unknown_clear: control.bits_1_11_opaque().get() == 0
                && control.bits_16_29_opaque().get() == 0,
            peer: [h0, h1, h2, h3, t0, t1],
            interface: MacInterface::from_field(peer_tail_and_policy.interface()),
            window: byte(peer_tail_and_policy.window()),
            current_sequence: halfword(
                block
                    .read_rx_block_ack_entry_current_sequence(entry)
                    .sequence(),
            ),
            loaded_start_sequence: halfword(
                block
                    .read_rx_block_ack_entry_start_sequence_load(entry)
                    .sequence(),
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
            0 => block
                .read_tx_block_ack_transmitter_address_high_q0()
                .block_ack_received(),
            1 => block
                .read_tx_block_ack_transmitter_address_high_q1()
                .block_ack_received(),
            2 => block
                .read_tx_block_ack_transmitter_address_high_q2()
                .block_ack_received(),
            3 => block
                .read_tx_block_ack_transmitter_address_high_q3()
                .block_ack_received(),
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
    /// by `0x7c` through queue seven at `0x2010_51c4..0x2010_51d4`. The model
    /// names each queue's words after the vendor's queue, so no index is
    /// translated here.
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
            bitmap: block_ack_bitmap(
                block.read_bitmap_low().bitmap(),
                block.read_bitmap_high().bitmap(),
            ),
            fragment_number: byte(control.fragment_number()),
            starting_sequence: halfword(control.starting_sequence()),
            tid: byte(control.tid()),
        }
    }
}

#[cfg(test)]
mod tests;
