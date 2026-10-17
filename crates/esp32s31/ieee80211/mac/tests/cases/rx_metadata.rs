use crate::*;

#[test]
fn rx_phy_info_matches_the_pinned_s31_public_metadata_layout() {
    let mut metadata = [0_u8; 0x40];
    metadata[1] = 0xe9;
    metadata[4..8].copy_from_slice(&0x1234_5678_u32.to_le_bytes());
    metadata[9..11].copy_from_slice(&0x9abc_u16.to_le_bytes());
    metadata[0x25] = 0x4f;
    assert_eq!(
        decode_rx_phy_info(&metadata),
        Some(RxPhyInfo {
            rate: 9,
            bb_format: 4,
            he_siga1: 0x1234_5678,
            he_siga2: 0x9abc,
        })
    );
    assert_eq!(decode_rx_phy_info(&metadata[..0x25]), None);
}

#[test]
fn rx_local_timestamp_is_the_little_endian_word_after_the_group_flag() {
    let mut metadata = [0_u8; 0x40];
    metadata[0x0b] = 0xff;
    metadata[0x0c..0x10].copy_from_slice(&0x8765_4321_u32.to_le_bytes());
    metadata[0x10] = 0xff;
    assert_eq!(decode_rx_local_timestamp(&metadata), Some(0x8765_4321));
    assert_eq!(decode_rx_local_timestamp(&metadata[..0x0f]), None);
}

#[test]
fn staged_rx_metadata_decodes_only_instruction_proved_s31_fields() {
    let mut metadata = [0_u8; 0x40];
    metadata[0] = (-47_i8) as u8;
    metadata[1] = 0xeb;
    metadata[4..8].copy_from_slice(&0x0040_5b4b_u32.to_le_bytes());
    metadata[9..11].copy_from_slice(&0x1234_u16.to_le_bytes());
    metadata[0x1c] = 6;
    metadata[0x1f] = 0;
    metadata[0x25] = 0x4f;

    assert_eq!(
        decode_normalized_rx_metadata(&metadata),
        Some(MacRxMetadata {
            channel: MacRxEvidence::Unavailable,
            rate: MacRxEvidence::HardwareObserved(RxPhyInfo {
                rate: 11,
                bb_format: 4,
                he_siga1: 0x0040_5b4b,
                he_siga2: 0x1234,
            }),
            rssi_dbm: MacRxEvidence::HardwareObserved(-47),
            crypto: MacRxEvidence::Unavailable,
            s_mpdu: MacRxEvidence::HardwareObserved(false),
            ampdu: MacRxEvidence::ProtocolValidated(true),
            amsdu: MacRxEvidence::Unavailable,
        })
    );
    assert_eq!(decode_normalized_rx_metadata(&metadata[..0x1c]), None);

    // A plausible callback-ABI value still is not raw-DMA evidence.
    metadata[0x1c] = 11;
    assert_eq!(
        decode_normalized_rx_metadata(&metadata)
            .expect("complete metadata")
            .channel,
        MacRxEvidence::Unavailable,
    );
}

#[test]
fn normalized_ht_rx_metadata_uses_the_direct_ht_sig_aggregation_bit() {
    let mut metadata = [0_u8; 0x40];
    metadata[4..8].copy_from_slice(&(7_u32 | (1 << 7) | (1 << 27) | (1 << 31)).to_le_bytes());
    metadata[0x1c] = 11;
    metadata[0x1f] = 0;
    metadata[0x25] = 2 << 4;

    let decoded = decode_normalized_rx_metadata(&metadata).unwrap();
    assert_eq!(decoded.s_mpdu, MacRxEvidence::HardwareObserved(false));
    assert_eq!(decoded.ampdu, MacRxEvidence::HardwareObserved(true));
    let MacRxEvidence::HardwareObserved(phy) = decoded.rate else {
        panic!("HT PHY metadata must remain hardware-observed");
    };
    let signal = phy.ht_signal().unwrap();
    assert_eq!(signal.mcs, 7);
    assert_eq!(signal.channel_width_mhz, 40);
    assert!(signal.aggregation);
    assert!(signal.short_guard_interval);

    metadata[4..8].fill(0);
    assert_eq!(
        decode_normalized_rx_metadata(&metadata).unwrap().ampdu,
        MacRxEvidence::HardwareObserved(false)
    );
}

#[test]
fn normalized_ht_rx_metadata_separates_mcs32_from_the_five_bit_rate_summary() {
    let mut metadata = [0_u8; 0x40];
    // The public `rate` summary at byte one is only five bits and therefore
    // wraps MCS32 to zero. The format-specific HT-SIG word remains the owner
    // of the complete seven-bit MCS selector and CBW geometry.
    metadata[1] = 0;
    metadata[4..8].copy_from_slice(&(32_u32 | (1 << 7)).to_le_bytes());
    metadata[0x25] = 2 << 4;

    let decoded = decode_normalized_rx_metadata(&metadata).unwrap();
    let MacRxEvidence::HardwareObserved(phy) = decoded.rate else {
        panic!("HT PHY metadata must remain hardware-observed");
    };
    assert_eq!(phy.rate, 0);
    let signal = phy.ht_signal().unwrap();
    assert_eq!(
        signal.ht_duplicate_mcs32_classification(),
        HtDuplicateRxClassification::Ht40(oer_ieee80211_mac::ht::HtDuplicateMcs32::new())
    );
    assert!(signal.ht_duplicate_mcs32().is_some());

    metadata[4..8].copy_from_slice(&32_u32.to_le_bytes());
    let MacRxEvidence::HardwareObserved(phy) =
        decode_normalized_rx_metadata(&metadata).unwrap().rate
    else {
        panic!("HT PHY metadata must remain hardware-observed");
    };
    let signal = phy.ht_signal().unwrap();
    assert_eq!(
        signal.ht_duplicate_mcs32_classification(),
        HtDuplicateRxClassification::Mismatch {
            channel_width_mhz: 20,
        }
    );
    assert_eq!(signal.ht_duplicate_mcs32(), None);
}

#[test]
fn normalized_rx_metadata_separates_format_validated_ampdu_from_ht_hardware_status() {
    let mut metadata = [0_u8; 0x40];
    metadata[0x25] = 4 << 4;
    assert_eq!(
        decode_normalized_rx_metadata(&metadata).unwrap().ampdu,
        MacRxEvidence::ProtocolValidated(true)
    );

    metadata[0x25] = 1 << 4;
    assert_eq!(
        decode_normalized_rx_metadata(&metadata).unwrap().ampdu,
        MacRxEvidence::ProtocolValidated(false)
    );

    metadata[0x25] = 9 << 4;
    assert_eq!(
        decode_normalized_rx_metadata(&metadata).unwrap().ampdu,
        MacRxEvidence::Unavailable
    );
}

#[test]
fn normalized_monitor_view_excludes_the_vendor_prefix_and_stripped_fcs() {
    const MPDU_LENGTH: usize = 24;
    const RECEIVED: usize = 0x40 + MPDU_LENGTH;
    let mut storage = [0_u8; RECEIVED];
    storage[0] = (-42_i8) as u8;
    storage[1] = 3;
    storage[0x1c] = 11;
    storage[0x25] = 1 << 4;
    let signal_length = (MPDU_LENGTH + 4) as u32;
    storage[0x38..0x3c].copy_from_slice(&signal_length.to_le_bytes());
    for (index, byte) in storage[0x40..].iter_mut().enumerate() {
        *byte = index as u8;
    }
    let segment = RxSegment {
        descriptor_address: 0x2f00_1000,
        descriptor_word0: (RECEIVED as u32) | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        buffer: &storage,
        next_descriptor_address: 0,
    };

    let frame = view_normalized_rx_frame(
        &segment,
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
    )
    .unwrap();
    assert_eq!(frame.mpdu, &storage[0x40..]);
    assert_eq!(frame.logical_length, MPDU_LENGTH);
    assert_eq!(
        frame.metadata.rssi_dbm,
        MacRxEvidence::HardwareObserved(-42)
    );
}

#[test]
fn rx_phy_info_decodes_the_qualified_he20_mcs9_signal() {
    let phy = RxPhyInfo {
        rate: 11,
        bb_format: 4,
        he_siga1: 0x0040_5b4b,
        he_siga2: 0,
    };
    assert_eq!(phy.baseband_format(), RxBasebandFormat::HeSu);
    assert_eq!(
        phy.he_su_signal(),
        Some(HeSuSignal {
            format: true,
            beam_change: true,
            uplink: false,
            mcs: 9,
            dcm: false,
            bss_color: 27,
            spatial_reuse: 0,
            bandwidth: HeBandwidth::Mhz20,
            guard_interval_and_ltf: HeGuardIntervalAndLtf::TwoLtf1600Ns,
            nsts_and_midamble_periodicity: 0,
            txop: 0,
            ldpc: false,
            ldpc_extra_symbol: false,
            stbc: false,
            beamformed: false,
            pre_fec_padding_factor: 0,
            packet_extension_disambiguity: false,
            doppler: false,
        })
    );
    let signal = phy.he_su_signal().unwrap();
    assert_eq!(signal.bandwidth.mhz(), 20);
    assert_eq!(signal.guard_interval_and_ltf.guard_interval_ns(), 1_600);
    assert_eq!(signal.guard_interval_and_ltf.ltf_count(), 2);
    assert_eq!(signal.space_time_stream_count(), Some(1));
    assert_eq!(signal.spatial_stream_count(), Some(1));
}

#[test]
fn he_su_stbc_distinguishes_space_time_and_spatial_stream_counts() {
    let signal = HeSuSignal::decode(0x00e0_591b, 0x4a0c);
    assert!(signal.stbc);
    assert!(!signal.doppler);
    assert_eq!(signal.nsts_and_midamble_periodicity, 1);
    assert_eq!(signal.space_time_stream_count(), Some(2));
    assert_eq!(signal.spatial_stream_count(), Some(1));

    let doppler = HeSuSignal::decode(0x00e0_591b, 0xca0c);
    assert!(doppler.doppler);
    assert_eq!(doppler.space_time_stream_count(), None);
    assert_eq!(doppler.spatial_stream_count(), None);
}

#[test]
fn rx_phy_info_uses_the_blob_su_layout_for_extended_range_su() {
    let phy = RxPhyInfo {
        rate: 11,
        bb_format: 6,
        he_siga1: 0x0040_5b4b,
        he_siga2: 0,
    };
    assert_eq!(phy.he_su_signal().map(|signal| signal.mcs), Some(9));
}

#[test]
fn rx_phy_info_decodes_complete_he_mu_common_signal_fields() {
    let phy = RxPhyInfo {
        rate: 0,
        bb_format: 5,
        he_siga1: 0x03de_4d5b,
        he_siga2: 0xdbb5,
    };
    assert_eq!(
        phy.he_mu_signal(),
        Some(HeMuSignal {
            uplink: true,
            sig_b_mcs: 5,
            sig_b_dcm: true,
            bss_color: 42,
            spatial_reuse: 9,
            bandwidth: HeMuBandwidth::Unknown(4),
            sig_b_symbols_or_mu_mimo_users_minus_one: 7,
            sig_b_compression: true,
            guard_interval_and_ltf: HeGuardIntervalAndLtf::FourLtf3200Ns,
            doppler: true,
            txop: 0x35,
            nltf_and_midamble_periodicity: 3,
            ldpc_extra_symbol_segment: true,
            stbc: true,
            pre_fec_padding_factor: 2,
            packet_extension_disambiguity: true,
        })
    );
    let signal = phy.he_mu_signal().unwrap();
    assert_eq!(signal.bandwidth.mhz(), None);
    assert_eq!(signal.bandwidth.raw(), 4);
    assert_eq!(signal.sig_b_symbols_or_mu_mimo_users(), 8);
    assert_eq!(signal.he_ltf_symbols(), 6);
}

#[test]
fn rx_phy_info_decodes_complete_he_trigger_based_common_signal_fields() {
    let siga1 = 1 | (17 << 1) | (1 << 7) | (2 << 11) | (3 << 15) | (4 << 19) | (1 << 24);
    let phy = RxPhyInfo {
        rate: 0,
        bb_format: 7,
        he_siga1: siga1,
        he_siga2: 0x01d5,
    };
    assert_eq!(
        phy.he_trigger_based_signal(),
        Some(HeTriggerBasedSignal {
            format: true,
            bss_color: 17,
            spatial_reuse: [1, 2, 3, 4],
            bandwidth: HeBandwidth::Mhz40,
            txop: 0x55,
        })
    );
}

#[test]
fn rx_he_mu_sig_b_borrows_only_the_blob_advertised_complete_bytes() {
    let mut metadata = [0_u8; 0x40];
    metadata[0x25] = 5 << 4;
    metadata[4..8].copy_from_slice(&(1_u32 << 22).to_le_bytes());
    metadata[0x1a] = 0xfe;
    metadata[0x1e] = 0xb7;

    let selected_user = (1 << 20) | (7 << 15) | (12 << 11) | 0x345;
    metadata[0x28] = selected_user as u8;
    metadata[0x29] = (selected_user >> 8) as u8;
    metadata[0x2a] = ((selected_user >> 16) as u8 & 0x1f) | (5 << 5);
    metadata[0x2b] = 0x80 | 2;

    let common = 0x1a_bcde_u32;
    metadata[0x2d] = (common << 2) as u8;
    metadata[0x2e] = (common >> 6) as u8;
    metadata[0x2f] = (common >> 14) as u8 & 0x7f;
    metadata[0x38..0x3b].copy_from_slice(&[0xaa, 0xbb, 0x1c]);
    metadata[0x3b] = 0xee;

    let sig_b = decode_rx_he_mu_sig_b(&metadata).unwrap();
    assert_eq!(sig_b.bit_length, 21);
    assert_eq!(sig_b.common_info_raw, common);
    assert_eq!(sig_b.selected_user_info_raw, selected_user);
    assert_eq!(
        sig_b.selected_user,
        HeMuSigBUser::Mimo(HeMuSigBMimoUser {
            station_id: 0x345,
            spatial_configuration: 12,
            mcs: 7,
            ldpc: true,
        })
    );
    assert_eq!(sig_b.ru_size, 2);
    assert_eq!(sig_b.ru_position, 11);
    assert_eq!(sig_b.complete_bytes, &[0xaa, 0xbb, 0x1c]);
    let compressed_users: Vec<_> = sig_b.he20_mimo_users().unwrap().collect();
    assert_eq!(compressed_users.len(), 1);
    assert_eq!(compressed_users[0].bit_offset, 0);
    assert_eq!(compressed_users[0].raw, 0x1c_bbaa & 0x1f_ffff);

    assert_eq!(decode_rx_he_mu_sig_b(&metadata[..0x3a]), None);
    metadata[0x2b] &= 0x7f;
    assert_eq!(
        decode_rx_he_mu_sig_b(&metadata).unwrap().complete_bytes,
        &[]
    );
    assert_eq!(decode_rx_he_mu_sig_b(&metadata[..0x30]), None);
    metadata[0x25] = 4 << 4;
    assert_eq!(decode_rx_he_mu_sig_b(&metadata), None);
}

#[test]
fn rx_he20_non_mimo_sig_b_iterates_complete_users_and_rejects_other_layouts() {
    fn write_user(bytes: &mut [u8], bit_offset: usize, word: u32) {
        for output_bit in 0..21 {
            let destination_bit = bit_offset + output_bit;
            if word & (1 << output_bit) != 0 {
                bytes[destination_bit / 8] |= 1 << (destination_bit % 8);
            }
        }
    }

    let mut metadata = [0_u8; 0x48];
    metadata[0x25] = 5 << 4;
    let bit_length = 101_u16;
    metadata[0x2a] = ((bit_length % 8) as u8) << 5;
    metadata[0x2b] = 0x80 | (bit_length / 8) as u8;

    let users = [
        (1 << 20) | (3 << 15) | 0x123,
        (1 << 19) | (5 << 15) | 0x456,
        (1 << 14) | (7 << 15) | 0x321,
    ];
    write_user(&mut metadata[0x38..], 18, users[0]);
    write_user(&mut metadata[0x38..], 39, users[1]);
    write_user(&mut metadata[0x38..], 70, users[2]);

    let sig_b = decode_rx_he_mu_sig_b(&metadata).unwrap();
    assert_eq!(sig_b.signal.bandwidth, HeMuBandwidth::Mhz20);
    assert!(!sig_b.signal.sig_b_compression);
    let entries: Vec<_> = sig_b.he20_non_mimo_users().unwrap().collect();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].bit_offset, 18);
    assert_eq!(entries[1].bit_offset, 39);
    assert_eq!(entries[2].bit_offset, 70);
    assert_eq!(entries[2].user, HeMuSigBNonMimoUser::decode(users[2]));

    metadata[4..8].copy_from_slice(&(1_u32 << 22).to_le_bytes());
    assert_eq!(
        decode_rx_he_mu_sig_b(&metadata)
            .unwrap()
            .he20_non_mimo_users(),
        Err(RxHe20MuSigBUsersError::MuMimoCompressed)
    );
    metadata[4..8].copy_from_slice(&(1_u32 << 15).to_le_bytes());
    assert_eq!(
        decode_rx_he_mu_sig_b(&metadata)
            .unwrap()
            .he20_non_mimo_users(),
        Err(RxHe20MuSigBUsersError::WiderOrUnknownBandwidth)
    );
}

#[test]
fn rx_baseband_format_preserves_unknown_hardware_values() {
    assert_eq!(RxBasebandFormat::decode(9), RxBasebandFormat::Unknown(9));
    assert_eq!(RxBasebandFormat::Unknown(9).raw(), 9);
}
