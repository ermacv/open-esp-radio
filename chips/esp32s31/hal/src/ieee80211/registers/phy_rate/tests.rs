//! The Wi-Fi-owned PHY words over host memory.

use super::{decode_noise_floor_quarter_db, quarter_db_to_dbm};
use crate::ieee80211::registers::HostWords;

#[test]
fn noise_floor_decodes_like_the_rom_and_the_blob() {
    // Code 0x000 is -0x1000 sixteenths of a dB.
    assert_eq!(decode_noise_floor_quarter_db(0), -0x400);
    assert_eq!(decode_noise_floor_quarter_db(0xfff), -1);
    assert_eq!(quarter_db_to_dbm(-0x400), -128);
    assert_eq!(quarter_db_to_dbm(-370), -92);
}

#[test]
fn low_rate_path_switches_all_three_edges() {
    let mut memory = HostWords::filled(0);
    memory.owners().registers.configure_phy_low_rate(true);
    {
        let owners = memory.owners();
        assert!(owners.registers.phy_low_rate_enabled());
        let rate = &owners.registers.mac.wifi_phy_rate_oracle_wifi_bb_brx;
        assert!(
            rate.read_low_rate_primary_control()
                .low_rate_enable_second()
        );
        assert!(rate.read_low_rate_secondary_control().low_rate_enable());
    }
    let enabled = memory.clone();
    memory.owners().registers.configure_phy_low_rate(false);
    assert_eq!(enabled.changes(&memory).len(), 2);
    assert_eq!(memory, HostWords::filled(0));
}

#[test]
fn noise_floor_snapshot_reads_the_measurement_code() {
    let memory = HostWords::filled(u32::MAX);
    let mut memory = memory;
    let owners = memory.owners();
    assert_eq!(owners.registers.noise_floor_snapshot(), (0xfff, -1));
    assert_eq!(owners.registers.read_noise_floor_dbm(), 0);
}
