//! The BlockAck register leaves over host memory.

use oer_chips_esp32s31_pac::wifi_mac::{
    bitmap_high, bitmap_low, extra_softap_rx_block_ack_bitmap_high,
    extra_softap_rx_block_ack_bitmap_low, extra_softap_rx_block_ack_control,
    extra_softap_rx_block_ack_peer_head, extra_softap_rx_block_ack_peer_tail_and_policy,
    extra_softap_rx_block_ack_start_sequence, rx_block_ack_agreement_update,
    rx_block_ack_entry_bitmap_high_load, rx_block_ack_entry_bitmap_low_load,
    rx_block_ack_entry_control, rx_block_ack_entry_peer_head,
    rx_block_ack_entry_peer_tail_and_policy, rx_block_ack_entry_start_sequence_load,
    tx_block_ack_bitmap_high_q0, tx_block_ack_bitmap_high_q7, tx_block_ack_bitmap_low_q0,
    tx_block_ack_bitmap_low_q7, tx_block_ack_control_sequence_q7,
    tx_block_ack_transmitter_address_high_q7, tx_block_ack_transmitter_address_low_q7,
};

use super::{
    block_ack_bitmap, extra_softap_index, select_extra_softap_entry, set_extra_softap_latch,
};
use crate::radio::ieee80211::registers::{
    HostWords, MacExtraSoftApRxBlockAckEntryIndex, MacInterface, MacPlacement,
    MacRxBlockAckEntryIndex, MacRxBlockAckStartingSequence, MacRxBlockAckTid, MacRxBlockAckWindow,
    WifiRadioRegisters, byte, halfword,
};

const PEER: [u8; 6] = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];

/// The byte distance between two ordinary receive BlockAck banks.
const BANK: usize = 0x24;

fn entry(index: u32) -> MacRxBlockAckEntryIndex {
    MacRxBlockAckEntryIndex::new(index).expect("an ordinary entry")
}

fn tid() -> MacRxBlockAckTid {
    MacRxBlockAckTid::new(5).expect("a TID")
}

fn sequence() -> MacRxBlockAckStartingSequence {
    MacRxBlockAckStartingSequence::new(0x0abc).expect("a sequence number")
}

fn window() -> MacRxBlockAckWindow {
    MacRxBlockAckWindow::new(64).expect("a window")
}

/// The (part, offset) of each changed word.
fn changed(before: &HostWords, after: &HostWords) -> std::vec::Vec<(&'static str, usize)> {
    before
        .changes(after)
        .into_iter()
        .map(|(part, offset, _)| (part, offset))
        .collect()
}

/// Store `value` at byte `offset` of the BlockAck part's memory.
fn store(words: &mut HostWords, offset: usize, value: u32) {
    let word = words
        .wifi_mac_block_ack
        .get_mut(offset / 4)
        .expect("the offset lies in the part");
    *word = value;
}

#[test]
fn extra_softap_entry_domain_matches_the_vendor_allocator() {
    assert!(MacExtraSoftApRxBlockAckEntryIndex::new(0).is_some());
    assert!(MacExtraSoftApRxBlockAckEntryIndex::new(7).is_some());
    assert!(MacExtraSoftApRxBlockAckEntryIndex::new(8).is_none());
}

#[test]
fn vendor_entry_zero_lives_in_the_last_bank_and_updates_bit_zero() {
    let mut words = HostWords::filled(0);
    let before = words.clone();
    words.owners().registers.program_rx_block_ack_entry(
        entry(0),
        MacInterface::AccessPoint,
        PEER,
        tid(),
        sequence(),
        window(),
    );
    // Element seven is the vendor's entry zero, at 0x2010_4274.
    let bank = 7 * BANK;
    assert_eq!(rx_block_ack_entry_control::OFFSET + bank, 0x274);
    assert_eq!(
        changed(&before, &words),
        [
            (
                "wifi_mac_block_ack",
                rx_block_ack_entry_control::OFFSET + bank
            ),
            (
                "wifi_mac_block_ack",
                rx_block_ack_entry_peer_tail_and_policy::OFFSET + bank
            ),
            (
                "wifi_mac_block_ack",
                rx_block_ack_entry_peer_head::OFFSET + bank
            ),
            (
                "wifi_mac_block_ack",
                rx_block_ack_entry_start_sequence_load::OFFSET + bank
            ),
            ("wifi_mac_block_ack", rx_block_ack_agreement_update::OFFSET),
        ]
    );

    let owners = words.owners();
    let snapshot = owners
        .registers
        .rx_block_ack_entry_snapshot(entry(0))
        .expect("an ordinary entry");
    assert!(snapshot.enabled && snapshot.write_enabled && snapshot.valid);
    assert!(snapshot.control_unknown_clear);
    assert_eq!(snapshot.tid, 5);
    assert_eq!(snapshot.peer, PEER);
    assert_eq!(snapshot.interface, MacInterface::AccessPoint);
    assert_eq!(snapshot.window, 64);
    assert_eq!(snapshot.loaded_start_sequence, 0x0abc);
    assert_eq!(snapshot.bitmap_load, 0);
    assert_eq!(
        owners
            .registers
            .mac
            .wifi_mac_block_ack
            .read_rx_block_ack_agreement_update()
            .ordinary_entry_update(),
        1
    );
}

#[test]
fn vendor_entry_seven_lives_in_the_first_bank_and_updates_bit_seven() {
    let mut words = HostWords::filled(0);
    let before = words.clone();
    words.owners().registers.program_rx_block_ack_entry(
        entry(7),
        MacInterface::Station,
        PEER,
        tid(),
        sequence(),
        window(),
    );
    let offsets = changed(&before, &words);
    assert_eq!(
        offsets.first(),
        Some(&("wifi_mac_block_ack", rx_block_ack_entry_control::OFFSET))
    );
    let owners = words.owners();
    assert_eq!(
        owners
            .registers
            .mac
            .wifi_mac_block_ack
            .read_rx_block_ack_agreement_update()
            .ordinary_entry_update(),
        0x80
    );
    let snapshot = owners
        .registers
        .rx_block_ack_entry_snapshot(entry(7))
        .expect("an ordinary entry");
    assert_eq!(snapshot.peer, PEER);
    assert_eq!(snapshot.interface, MacInterface::Station);
}

#[test]
fn a_delete_ends_with_the_whole_control_word_zero() {
    let mut words = HostWords::filled(u32::MAX);
    let before = words.clone();
    words.owners().registers.delete_rx_block_ack_entry(entry(2));
    let bank = 5 * BANK;
    assert_eq!(
        before.changes(&words),
        [
            (
                "wifi_mac_block_ack",
                rx_block_ack_entry_control::OFFSET + bank,
                0
            ),
            (
                "wifi_mac_block_ack",
                rx_block_ack_entry_bitmap_low_load::OFFSET + bank,
                0
            ),
            (
                "wifi_mac_block_ack",
                rx_block_ack_entry_bitmap_high_load::OFFSET + bank,
                0
            ),
        ]
    );
}

#[test]
fn a_window_reset_keeps_the_peer_and_republishes_the_active_control() {
    let mut words = HostWords::filled(0);
    {
        let mut owners = words.owners();
        owners.registers.program_rx_block_ack_entry(
            entry(3),
            MacInterface::AccessPoint,
            PEER,
            tid(),
            sequence(),
            window(),
        );
        owners.registers.reset_rx_block_ack_window(
            entry(3),
            MacRxBlockAckTid::new(2).expect("a TID"),
            MacRxBlockAckStartingSequence::new(0x0123).expect("a sequence number"),
            MacRxBlockAckWindow::new(32).expect("a window"),
        );
    }
    let owners = words.owners();
    let snapshot = owners
        .registers
        .rx_block_ack_entry_snapshot(entry(3))
        .expect("an ordinary entry");
    assert_eq!(snapshot.peer, PEER);
    assert_eq!(snapshot.interface, MacInterface::AccessPoint);
    assert_eq!(snapshot.tid, 2);
    assert_eq!(snapshot.window, 32);
    assert_eq!(snapshot.loaded_start_sequence, 0x0123);
    assert!(snapshot.enabled && snapshot.write_enabled && snapshot.valid);
}

#[test]
fn an_extra_softap_entry_is_staged_committed_and_read_back() {
    let mut words = HostWords::filled(0);
    let before = words.clone();
    let index = MacExtraSoftApRxBlockAckEntryIndex::new(6).expect("an extra entry");
    let confirmed = words
        .owners()
        .registers
        .program_extra_softap_rx_block_ack_entry(
            index,
            MacInterface::AccessPoint,
            PEER,
            tid(),
            sequence(),
            window(),
        );
    assert!(confirmed);
    // The commit and the readback latch are released again.
    assert_eq!(
        changed(&before, &words),
        [
            (
                "wifi_mac_block_ack",
                extra_softap_rx_block_ack_control::OFFSET
            ),
            (
                "wifi_mac_block_ack",
                extra_softap_rx_block_ack_peer_tail_and_policy::OFFSET
            ),
            (
                "wifi_mac_block_ack",
                extra_softap_rx_block_ack_peer_head::OFFSET
            ),
            (
                "wifi_mac_block_ack",
                extra_softap_rx_block_ack_start_sequence::OFFSET
            ),
        ]
    );
    let snapshot = extra_softap_rx_block_ack_entry_snapshot(&mut words.owners().registers, index);
    assert!(snapshot.enabled && snapshot.write_enabled && snapshot.valid);
    assert_eq!(snapshot.index, 6);
    assert_eq!(snapshot.tid, 5);
    assert_eq!(snapshot.peer, PEER);
    assert_eq!(snapshot.interface, MacInterface::AccessPoint);
    assert_eq!(snapshot.window, 64);
    assert_eq!(snapshot.starting_sequence, 0x0abc);
    assert_eq!(snapshot.bitmap, 0);
}

#[test]
fn an_extra_softap_delete_clears_validity_and_the_staged_bitmap() {
    let mut words = HostWords::filled(u32::MAX);
    let before = words.clone();
    words
        .owners()
        .registers
        .delete_extra_softap_rx_block_ack_entry(
            MacExtraSoftApRxBlockAckEntryIndex::new(1).expect("an extra entry"),
        );
    assert_eq!(
        changed(&before, &words),
        [
            ("wifi_mac_block_ack", rx_block_ack_agreement_update::OFFSET),
            (
                "wifi_mac_block_ack",
                extra_softap_rx_block_ack_control::OFFSET
            ),
            (
                "wifi_mac_block_ack",
                extra_softap_rx_block_ack_bitmap_low::OFFSET
            ),
            (
                "wifi_mac_block_ack",
                extra_softap_rx_block_ack_bitmap_high::OFFSET
            ),
        ]
    );
    let owners = words.owners();
    let block = &owners.registers.mac.wifi_mac_block_ack;
    let control = block.read_extra_softap_rx_block_ack_control();
    assert!(!control.valid());
    assert_eq!(control.index().get(), 1);
    let update = block.read_rx_block_ack_agreement_update();
    assert!(!(update.extra_softap_commit() || update.extra_softap_readback_latch()));
}

#[test]
fn tx_block_ack_results_are_read_by_vendor_queue() {
    let mut words = HostWords::filled(0);
    store(&mut words, tx_block_ack_bitmap_low_q0::OFFSET, 0x0102_0304);
    store(&mut words, tx_block_ack_bitmap_high_q0::OFFSET, 0x0506_0708);
    store(
        &mut words,
        tx_block_ack_control_sequence_q7::OFFSET,
        u32::MAX,
    );
    store(&mut words, tx_block_ack_bitmap_low_q7::OFFSET, 0xaaaa_aaaa);
    store(&mut words, tx_block_ack_bitmap_high_q7::OFFSET, 0x5555_5555);
    store(
        &mut words,
        tx_block_ack_transmitter_address_low_q7::OFFSET,
        0x3322_1102,
    );
    store(
        &mut words,
        tx_block_ack_transmitter_address_high_q7::OFFSET,
        0x0000_5544,
    );
    let before = words.clone();
    let owners = words.owners();
    let payload = owners
        .registers
        .read_tx_block_ack_payload(0)
        .expect("an ordinary queue");
    assert_eq!(payload.bitmap(), 0x0506_0708_0102_0304);
    assert_eq!(payload.bitmap_low(), 0x0102_0304);
    assert_eq!(payload.bitmap_high(), 0x0506_0708);
    assert!(owners.registers.read_tx_block_ack_payload(4).is_none());
    let observation = owners
        .registers
        .read_tx_block_ack_observation(0)
        .expect("an ordinary queue");
    assert!(!observation.block_ack_received);

    drop(owners);
    assert_eq!(before.changes(&words), []);
}

#[test]
fn the_internal_tx_block_ack_result_joins_its_bitmap_halves() {
    let mut words = HostWords::filled(0);
    for (offset, value) in [
        (bitmap_low::OFFSET, 0x89ab_cdef),
        (bitmap_high::OFFSET, 0x0123_4567),
    ] {
        let word = words
            .wifi_mac_internal_tx_block_ack
            .get_mut(offset / 4)
            .expect("the offset lies in the part");
        *word = value;
    }
    let owners = words.owners();
    let snapshot = owners.registers.internal_tx_block_ack_snapshot();
    assert_eq!(snapshot.bitmap(), 0x0123_4567_89ab_cdef);
    assert_eq!(snapshot.tid(), 0);
}

// Reads of what the writers left, from the register image: the HAL keeps
// no read of what it wrote.

/// Latched projection of one indirect extra-SoftAP receive BlockAck bank.
///
/// SOURCE: complete `libpp[hal_ampdu.o]::hal_debug_read_extra_softap_rx_ba` selects each logical index through
/// control bits 5..9, asserts agreement-update bit nine, reads the control,
/// peer, policy and starting-sequence staging words, then releases the latch.
/// The two bitmap words are the adjacent raw staging words used by complete
/// `hal_agreement_clr_extra_softap_rx_ba`; they remain raw observations rather
/// than a claimed protocol interpretation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ExtraSoftApRxBlockAckEntrySnapshot {
    enabled: bool,
    index: u8,
    tid: u8,
    write_enabled: bool,
    valid: bool,
    control_unknown_clear: bool,
    peer: [u8; 6],
    interface: MacInterface,
    window: u8,
    starting_sequence: u16,
    bitmap: u64,
}

/// Latch and sample one indirect extra-SoftAP agreement without changing
/// its committed contents.
fn extra_softap_rx_block_ack_entry_snapshot<M: MacPlacement>(
    registers: &mut WifiRadioRegisters<M>,
    hardware_index: MacExtraSoftApRxBlockAckEntryIndex,
) -> ExtraSoftApRxBlockAckEntrySnapshot {
    let block = &mut registers.mac.wifi_mac_block_ack;
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
        interface: MacInterface::from(peer_tail_and_policy.interface()),
        window: byte(peer_tail_and_policy.window()),
        starting_sequence: halfword(starting_sequence),
        bitmap: block_ack_bitmap(bitmap_low, bitmap_high),
    }
}
