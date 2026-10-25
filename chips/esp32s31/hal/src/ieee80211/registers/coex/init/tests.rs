//! The cold COEX/PTI transaction over host memory.

use crate::ieee80211::registers::{HostWords, MacPti};

fn pti(value: u32) -> MacPti {
    MacPti::new(value).expect("a four-bit PTI")
}

#[test]
fn cold_coex_publishes_every_priority_in_its_lane() {
    let before = HostWords::filled(0);
    let mut memory = before.clone();
    memory.owners().registers.initialize_mac_coex(
        pti(1),
        pti(2),
        [pti(3), pti(4), pti(5), pti(6), pti(7), pti(8), pti(9)],
        [pti(10), pti(11), pti(12)],
        0xff,
        pti(13),
    );

    // Four words of the COEX part change: RX, TB/beamforming, beamforming
    // and the default word.
    let changes = before.changes(&memory);
    assert!(
        changes
            .iter()
            .all(|(part, _, _)| *part == "wifi_mac_coex_init")
    );
    assert_eq!(changes.len(), 4);

    let mut memory = memory;
    let owners = memory.owners();
    let snapshot = owners.registers.mac_coex_priority_snapshot();
    assert_eq!(
        (snapshot.rx_active, snapshot.rx_ack, snapshot.wifi_default),
        (0, 1, 2)
    );
    let coex = &owners.registers.mac.wifi_mac_coex_init;
    let tb = coex.read_ofdma_tb_and_beamforming();
    assert_eq!(
        [
            tb.tb_0(),
            tb.tb_1(),
            tb.tb_2(),
            tb.tb_3(),
            tb.tb_4(),
            tb.tb_5(),
            tb.tb_6()
        ]
        .map(|v| v.get()),
        [3, 4, 5, 6, 7, 8, 9]
    );
    assert_eq!(tb.beamforming_0().get(), 10);
    let beamforming = coex.read_beamforming();
    assert_eq!(beamforming.beamforming_1().get(), 11);
    assert_eq!(beamforming.beamforming_2().get(), 12);
    assert_eq!(beamforming.multi_target_1().get(), 13);
    // The unmasked byte lands in the low eight bits of the ten-bit field.
    assert_eq!(beamforming.multi_target_0().get(), 0xff);
    let default = coex.read_default_control();
    assert!(default.default_pti_enable() && default.bit_5_opaque());
}
