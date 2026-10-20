//! PHY signal fields carried by HT and HE PPDUs.

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 27.3.11 (HE-SIG-A field).
    pub struct HeSuSigA1[4] (little) {
        pub format, with_format: bool = 0..1;
        pub beam_change, with_beam_change: bool = 1..2;
        pub uplink, with_uplink: bool = 2..3;
        pub mcs, with_mcs: u8 = 3..7;
        pub dcm, with_dcm: bool = 7..8;
        pub bss_color, with_bss_color: u8 = 8..14;
        pub spatial_reuse, with_spatial_reuse: u8 = 15..19;
        pub bandwidth, with_bandwidth: u8 = 19..21;
        pub guard_interval_and_ltf, with_guard_interval_and_ltf: u8 = 21..23;
        pub nsts_and_midamble_periodicity, with_nsts_and_midamble_periodicity: u8 = 23..26;
    }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 27.3.11 (HE-SIG-A field).
    pub struct HeSuSigA2[2] (little) {
        pub txop, with_txop: u8 = 0..7;
        pub ldpc, with_ldpc: bool = 7..8;
        pub ldpc_extra_symbol, with_ldpc_extra_symbol: bool = 8..9;
        pub stbc, with_stbc: bool = 9..10;
        pub beamformed, with_beamformed: bool = 10..11;
        pub pre_fec_padding_factor, with_pre_fec_padding_factor: u8 = 11..13;
        pub packet_extension_disambiguity, with_packet_extension_disambiguity: bool = 13..14;
        pub doppler, with_doppler: bool = 15..16;
    }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 27.3.11 (HE-SIG-A field).
    pub struct HeMuSigA1[4] (little) {
        pub uplink, with_uplink: bool = 0..1;
        pub sig_b_mcs, with_sig_b_mcs: u8 = 1..4;
        pub sig_b_dcm, with_sig_b_dcm: bool = 4..5;
        pub bss_color, with_bss_color: u8 = 5..11;
        pub spatial_reuse, with_spatial_reuse: u8 = 11..15;
        pub bandwidth, with_bandwidth: u8 = 15..18;
        pub sig_b_symbols_or_mu_mimo_users_minus_one, with_sig_b_symbols_or_mu_mimo_users_minus_one: u8 = 18..22;
        pub sig_b_compression, with_sig_b_compression: bool = 22..23;
        pub guard_interval_and_ltf, with_guard_interval_and_ltf: u8 = 23..25;
        pub doppler, with_doppler: bool = 25..26;
    }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 27.3.11 (HE-SIG-A field).
    pub struct HeMuSigA2[2] (little) {
        pub txop, with_txop: u8 = 0..7;
        pub nltf_and_midamble_periodicity, with_nltf_and_midamble_periodicity: u8 = 8..11;
        pub ldpc_extra_symbol_segment, with_ldpc_extra_symbol_segment: bool = 11..12;
        pub stbc, with_stbc: bool = 12..13;
        pub pre_fec_padding_factor, with_pre_fec_padding_factor: u8 = 13..15;
        pub packet_extension_disambiguity, with_packet_extension_disambiguity: bool = 15..16;
    }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 27.3.11 (HE-SIG-A field).
    pub struct HeTriggerBasedSigA1[4] (little) {
        pub format, with_format: bool = 0..1;
        pub bss_color, with_bss_color: u8 = 1..7;
        pub bandwidth, with_bandwidth: u8 = 24..26;
        pub spatial_reuse0, with_spatial_reuse0: u8 = 7..11;
        pub spatial_reuse1, with_spatial_reuse1: u8 = 11..15;
        pub spatial_reuse2, with_spatial_reuse2: u8 = 15..19;
        pub spatial_reuse3, with_spatial_reuse3: u8 = 19..23;
    }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 27.3.11 (HE-SIG-A field).
    pub struct HeTriggerBasedSigA2[2] (little) {
        pub txop, with_txop: u8 = 0..7;
    }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 19.3.9.4 (HT-SIG field).
    pub struct HtSig[4] (little) {
        pub mcs, with_mcs: u8 = 0..7;
        pub width40, with_width40: bool = 7..8;
        pub aggregation, with_aggregation: bool = 27..28;
        pub short_gi, with_short_gi: bool = 31..32;
    }
}
oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 27.3.11 (HE MU bandwidth code).
    pub struct HeMuBandwidthCode[1] (little) {
        pub code, with_code: u8 = 0..3;
    }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 27.3.11 (BSS Color code).
    pub struct BssColorCode[1] (little) {
        pub color, with_color: u8 = 0..6;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ht_signal_round_trip_preserves_the_carried_length_and_other_flags() {
        let carried = HtSig::from_bytes([0x87, 0x34, 0x12, 0x88]);
        assert_eq!(carried.mcs(), 7);
        assert!(carried.width40());
        assert!(carried.aggregation());
        assert!(carried.short_gi());
        let changed = carried
            .with_mcs(3)
            .unwrap()
            .with_width40(false)
            .with_short_gi(false);
        let decoded = HtSig::from_bytes(changed.to_bytes());
        assert_eq!(decoded.mcs(), 3);
        assert!(!decoded.width40());
        assert!(!decoded.short_gi());
        assert!(decoded.aggregation());
        assert_eq!(decoded.to_bytes(), [0x03, 0x34, 0x12, 0x08]);
        assert!(carried.with_mcs(128).is_err());
        assert_eq!(carried.to_bytes(), [0x87, 0x34, 0x12, 0x88]);
    }

    #[test]
    fn he_su_and_mu_round_trip_their_distinct_air_field_domains() {
        for color in 0..64 {
            let su = HeSuSigA1::from_bytes([0; 4])
                .with_format(true)
                .with_uplink(true)
                .with_mcs(9)
                .unwrap()
                .with_bss_color(color)
                .unwrap()
                .with_spatial_reuse(10)
                .unwrap()
                .with_bandwidth(2)
                .unwrap()
                .with_guard_interval_and_ltf(3)
                .unwrap();
            let su = HeSuSigA1::from_bytes(su.to_bytes());
            assert!(su.format());
            assert!(su.uplink());
            assert_eq!(
                (
                    su.mcs(),
                    su.bss_color(),
                    su.spatial_reuse(),
                    su.bandwidth(),
                    su.guard_interval_and_ltf()
                ),
                (9, color, 10, 2, 3)
            );
            let mu = HeMuSigA1::from_bytes([0; 4])
                .with_bss_color(color)
                .unwrap()
                .with_bandwidth(7)
                .unwrap()
                .with_sig_b_mcs(5)
                .unwrap()
                .with_sig_b_compression(true)
                .with_guard_interval_and_ltf(2)
                .unwrap();
            let mu = HeMuSigA1::from_bytes(mu.to_bytes());
            assert_eq!(
                (
                    mu.bss_color(),
                    mu.bandwidth(),
                    mu.sig_b_mcs(),
                    mu.guard_interval_and_ltf()
                ),
                (color, 7, 5, 2)
            );
            assert!(mu.sig_b_compression());
        }
        assert!(HeSuSigA1::from_bytes([0; 4]).with_bss_color(64).is_err());
        assert!(HeMuSigA1::from_bytes([0; 4]).with_bandwidth(8).is_err());
    }

    #[test]
    fn trigger_based_spatial_reuse_fields_survive_round_trip_independently() {
        let sig = HeTriggerBasedSigA1::from_bytes([0; 4])
            .with_format(true)
            .with_bss_color(37)
            .unwrap()
            .with_bandwidth(1)
            .unwrap()
            .with_spatial_reuse0(3)
            .unwrap()
            .with_spatial_reuse1(6)
            .unwrap()
            .with_spatial_reuse2(9)
            .unwrap()
            .with_spatial_reuse3(12)
            .unwrap();
        let decoded = HeTriggerBasedSigA1::from_bytes(sig.to_bytes());
        assert!(decoded.format());
        assert_eq!((decoded.bss_color(), decoded.bandwidth()), (37, 1));
        assert_eq!(
            (
                decoded.spatial_reuse0(),
                decoded.spatial_reuse1(),
                decoded.spatial_reuse2(),
                decoded.spatial_reuse3()
            ),
            (3, 6, 9, 12)
        );
        assert!(sig.with_spatial_reuse2(16).is_err());
        let tail = HeTriggerBasedSigA2::from_bytes([0; 2])
            .with_txop(77)
            .unwrap();
        assert_eq!(HeTriggerBasedSigA2::from_bytes(tail.to_bytes()).txop(), 77);
    }
}
