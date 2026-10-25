use super::COLD_RECEIVE_QUEUES;
use crate::ieee80211::registers::HostWords;
use oer_esp32s31_pac::MacInterface;

#[test]
fn the_cold_policy_clears_the_first_three_interfaces_and_keeps_their_high_bits() {
    let mut words = HostWords::filled(u32::MAX);
    words.owners().registers.initialize_cold_receive_policy();
    // Queue three has no interface context: the fourth pair and every low
    // word keep their value; each of the first three high words lost only its
    // address bytes.
    let changed: std::vec::Vec<(usize, u32)> = words
        .interface_address
        .iter()
        .enumerate()
        .filter(|(_, word)| **word != u32::MAX)
        .map(|(index, word)| (index, *word))
        .collect();
    assert_eq!(changed, [(1, 0xffff_0000), (3, 0xffff_0000), (5, 0xffff_0000)]);
}

#[test]
fn queue_three_has_no_interface_context() {
    let contexts = COLD_RECEIVE_QUEUES.map(|(queue, context)| (queue.get(), context));
    assert_eq!(
        contexts,
        [
            (0, Some(MacInterface::Station)),
            (1, Some(MacInterface::AccessPoint)),
            (2, Some(MacInterface::Context2)),
            (3, None),
        ]
    );
}
