use super::{Ieee802154CoexConfig, Ieee802154CoexScene, resolve_priorities};
use crate::coex::{CoexEventId, CoexPti, CoexPtiTable};

/// The driver's default scene levels resolve against the vendor table as
/// the vendor MAC publishes them: idle 1, TX/RX 3, timed TX/RX 8, ACK 8.
#[test]
fn vendor_config_resolves_each_scene_from_the_shared_table() {
    let priorities = resolve_priorities(Ieee802154CoexConfig::VENDOR, &CoexPtiTable::VENDOR);
    for (scene, pti) in [
        (Ieee802154CoexScene::Idle, 1),
        (Ieee802154CoexScene::Tx, 3),
        (Ieee802154CoexScene::Rx, 3),
        (Ieee802154CoexScene::TxAt, 8),
        (Ieee802154CoexScene::RxAt, 8),
    ] {
        assert_eq!(priorities.scene(scene).value(), pti, "{scene:?}");
    }
    assert_eq!(priorities.ack().value(), 8);
}

/// A changed table entry reaches the scenes whose level reads it.
#[test]
fn resolution_follows_the_table_snapshot() {
    let mut table = CoexPtiTable::VENDOR;
    table.set(
        CoexEventId::new(42).unwrap_or_else(|| panic!("event 42 exists")),
        CoexPti::new(11).unwrap_or_else(|| panic!("11 is a priority")),
    );
    let priorities = resolve_priorities(Ieee802154CoexConfig::VENDOR, &table);
    assert_eq!(priorities.scene(Ieee802154CoexScene::TxAt).value(), 11);
    assert_eq!(priorities.ack().value(), 11);
    assert_eq!(priorities.scene(Ieee802154CoexScene::Tx).value(), 3);
}
