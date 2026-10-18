use crate::*;

fn single_frame_segment<'a>(storage: &'a mut [u8; 128], frame_control_low: u8) -> RxSegment<'a> {
    const SIGNAL_LENGTH: usize = 34;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const RECEIVED: usize = FRAME_OFFSET + SIGNAL_LENGTH;

    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    storage[FRAME_OFFSET] = frame_control_low;
    storage[FRAME_OFFSET + 1] = 0;
    storage[FRAME_OFFSET + 22] = 0;

    RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: rx_landed_word(128, RECEIVED as u32, true),
        buffer: storage,
        next_descriptor_address: 0,
    }
}

#[test]
fn management_rx_extracts_one_bounded_mpdu_and_strips_fcs() {
    let mut storage = [0_u8; 128];
    let segment = single_frame_segment(&mut storage, 0xb0);
    let mut output = [0_u8; 64];
    let frame = extract_management(
        &[segment],
        RxIngressConfig {
            ring_entry_limit: 4,
            csi_config: 0,
            flags: INGRESS_STRICT_RXEND | INGRESS_STRICT_DUMP,
        },
        &mut output,
    )
    .unwrap();

    assert_eq!(frame.length, 30);
    assert_eq!(frame.signal_length, 34);
    assert_eq!(frame.dump_length, 38);
    assert!(frame.dump_length_matches);
    assert_eq!(output[0], 0xb0);
}

#[test]
fn control_rx_extracts_trigger_mpdu_without_interpreting_its_payload() {
    let mut storage = [0_u8; 128];
    let segment = single_frame_segment(&mut storage, 0x24);
    let mut output = [0_u8; 64];
    let frame = extract_control(
        &[segment],
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: INGRESS_STRICT_RXEND | INGRESS_STRICT_DUMP,
        },
        &mut output,
    )
    .unwrap();

    assert_eq!(frame.length, 30);
    assert_eq!(output[0], 0x24);
    assert_eq!(output[1], 0);
}

#[test]
fn management_rx_rejects_failed_hardware_status() {
    let mut storage = [0_u8; 128];
    let mut segment = single_frame_segment(&mut storage, 0xb0);
    let mut failed = [0_u8; 128];
    failed.copy_from_slice(segment.buffer);
    failed[0x38 + 4] = 0xf5;
    segment.buffer = &failed;
    let mut output = [0_u8; 64];
    assert_eq!(
        extract_management(
            &[segment],
            RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
            &mut output,
        ),
        Err(RxError::MicFailure)
    );
}

#[test]
fn data_rx_reports_qos_llc_payload_offset() {
    const SIGNAL_LENGTH: usize = 26 + 8 + 4;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const RECEIVED: usize = FRAME_OFFSET + SIGNAL_LENGTH;

    let mut storage = [0_u8; 128];
    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    storage[FRAME_OFFSET] = 0x88;
    storage[FRAME_OFFSET + 1] = 0x02;
    storage[FRAME_OFFSET + 22] = 0;
    storage[FRAME_OFFSET + 26..FRAME_OFFSET + 34]
        .copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e]);
    let segment = RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: rx_landed_word(128, RECEIVED as u32, true),
        buffer: &storage,
        next_descriptor_address: 0,
    };
    let mut output = [0_u8; 64];
    let frame = extract_data(
        &[segment],
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: INGRESS_STRICT_RXEND | INGRESS_STRICT_DUMP,
        },
        &mut output,
    )
    .unwrap();

    assert_eq!(frame.mpdu.length, SIGNAL_LENGTH - 4);
    assert_eq!(frame.payload_offset, 26);
    assert_eq!(
        &output[frame.payload_offset..frame.payload_offset + 8],
        &[0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e]
    );
}

#[test]
fn ccmp_data_rx_reproduces_the_oracle_header_and_mic_adjustment() {
    const HEADER_LENGTH: usize = 26;
    const LLC_LENGTH: usize = 8;
    const PAYLOAD_LENGTH: usize = 4;
    const MPDU_LENGTH: usize = HEADER_LENGTH + 8 + LLC_LENGTH + PAYLOAD_LENGTH + 8;
    const SIGNAL_LENGTH: usize = MPDU_LENGTH + 4;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const RECEIVED: usize = FRAME_OFFSET + SIGNAL_LENGTH;

    let mut storage = [0_u8; 128];
    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    storage[FRAME_OFFSET] = 0x88;
    storage[FRAME_OFFSET + 1] = 0x42;
    storage[FRAME_OFFSET + 22] = 0;
    storage[FRAME_OFFSET + HEADER_LENGTH..FRAME_OFFSET + HEADER_LENGTH + 8]
        .copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
    storage[FRAME_OFFSET + HEADER_LENGTH + 8..FRAME_OFFSET + HEADER_LENGTH + 16]
        .copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x06]);
    storage[FRAME_OFFSET + HEADER_LENGTH + 16..FRAME_OFFSET + HEADER_LENGTH + 20]
        .copy_from_slice(&[1, 2, 3, 4]);
    let segment = RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: rx_landed_word(128, RECEIVED as u32, true),
        buffer: &storage,
        next_descriptor_address: 0,
    };
    let mut output = [0_u8; 80];
    let frame = extract_ccmp_data(
        &[segment],
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: INGRESS_STRICT_RXEND | INGRESS_STRICT_DUMP,
        },
        &mut output,
    )
    .unwrap();

    assert_eq!(frame.mpdu.length, MPDU_LENGTH);
    assert_eq!(frame.ccmp_header.packet_number().value(), 3);
    assert_eq!(frame.ccmp_header.key_id().value(), 0);
    assert_eq!(frame.ccmp_header_offset, HEADER_LENGTH);
    assert_eq!(frame.payload_offset, HEADER_LENGTH + 8);
    assert_eq!(frame.payload_length, LLC_LENGTH + PAYLOAD_LENGTH);
    assert_eq!(frame.mic_offset, MPDU_LENGTH - 8);
    assert_eq!(frame.mic_bytes_in_dma, 8);
    assert!(frame.mic_present_in_dma);
    assert_eq!(
        &output[frame.payload_offset..frame.payload_offset + LLC_LENGTH],
        &[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x06]
    );
}

#[test]
fn ccmp_data_rx_rejects_reserved_header_encodings() {
    const HEADER_LENGTH: usize = 24;
    const MPDU_LENGTH: usize = HEADER_LENGTH + 8 + 8 + 8;
    const SIGNAL_LENGTH: usize = MPDU_LENGTH + 4;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const RECEIVED: usize = FRAME_OFFSET + SIGNAL_LENGTH;

    for header in [
        [1, 0, 1, 0x20, 0, 0, 0, 0],
        [1, 0, 0, 0, 0, 0, 0, 0],
        [1, 0, 0, 0x21, 0, 0, 0, 0],
    ] {
        let mut storage = [0_u8; 128];
        storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
            &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
        );
        storage[FRAME_OFFSET..FRAME_OFFSET + 2].copy_from_slice(&0x4008_u16.to_le_bytes());
        storage[FRAME_OFFSET + HEADER_LENGTH..FRAME_OFFSET + HEADER_LENGTH + 8]
            .copy_from_slice(&header);
        let segment = RxSegment {
            descriptor_address: 0x2f00_4000,
            descriptor_word0: rx_landed_word(128, RECEIVED as u32, true),
            buffer: &storage,
            next_descriptor_address: 0,
        };
        let mut output = [0_u8; 80];
        assert_eq!(
            extract_ccmp_data(
                &[segment],
                RxIngressConfig {
                    ring_entry_limit: 1,
                    csi_config: 0,
                    flags: INGRESS_STRICT_RXEND | INGRESS_STRICT_DUMP,
                },
                &mut output,
            ),
            Err(RxError::Unsupported)
        );
    }
}

#[test]
fn first_segment_layout_exposes_a_consumed_ccmp_mic_shortfall() {
    const MPDU_LENGTH: usize = 26 + 8 + 8 + 4 + 8;
    const SIGNAL_LENGTH: usize = MPDU_LENGTH + 4;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const DMA_FRAME_LENGTH: usize = MPDU_LENGTH - 8;
    const RECEIVED: usize = FRAME_OFFSET + DMA_FRAME_LENGTH;

    let mut storage = [0_u8; 128];
    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    let segment = RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: rx_landed_word(128, RECEIVED as u32, true),
        buffer: &storage,
        next_descriptor_address: 0,
    };
    let layout = first_segment_layout(
        &segment,
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: INGRESS_STRICT_RXEND | INGRESS_STRICT_DUMP,
        },
    )
    .unwrap();

    assert_eq!(layout.received_length, RECEIVED);
    assert_eq!(layout.expected_frame_length, MPDU_LENGTH);
    assert_eq!(layout.available_frame_bytes, DMA_FRAME_LENGTH);
    assert_eq!(layout.frame_shortfall, 8);
}

#[test]
fn ccmp_data_rx_accepts_a_hardware_consumed_mic() {
    const HEADER_LENGTH: usize = 26;
    const LLC_LENGTH: usize = 8;
    const PAYLOAD_LENGTH: usize = 4;
    const MPDU_LENGTH: usize = HEADER_LENGTH + 8 + LLC_LENGTH + PAYLOAD_LENGTH + 8;
    const SIGNAL_LENGTH: usize = MPDU_LENGTH + 4;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const DMA_FRAME_LENGTH: usize = MPDU_LENGTH - 8;
    const RECEIVED: usize = FRAME_OFFSET + DMA_FRAME_LENGTH;

    let mut storage = [0_u8; 128];
    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    storage[FRAME_OFFSET] = 0x88;
    storage[FRAME_OFFSET + 1] = 0x42;
    storage[FRAME_OFFSET + 22] = 0;
    storage[FRAME_OFFSET + HEADER_LENGTH..FRAME_OFFSET + HEADER_LENGTH + 8]
        .copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
    storage[FRAME_OFFSET + HEADER_LENGTH + 8..FRAME_OFFSET + HEADER_LENGTH + 16]
        .copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x06]);
    storage[FRAME_OFFSET + HEADER_LENGTH + 16..FRAME_OFFSET + HEADER_LENGTH + 20]
        .copy_from_slice(&[1, 2, 3, 4]);
    let segment = RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: rx_landed_word(128, RECEIVED as u32, true),
        buffer: &storage,
        next_descriptor_address: 0,
    };
    let mut output = [0_u8; 80];
    let frame = extract_ccmp_data(
        &[segment],
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: INGRESS_STRICT_RXEND | INGRESS_STRICT_DUMP,
        },
        &mut output,
    )
    .unwrap();

    assert_eq!(frame.mpdu.length, DMA_FRAME_LENGTH);
    assert_eq!(frame.payload_offset, HEADER_LENGTH + 8);
    assert_eq!(frame.payload_length, LLC_LENGTH + PAYLOAD_LENGTH);
    assert_eq!(frame.mic_offset, DMA_FRAME_LENGTH);
    assert_eq!(frame.mic_bytes_in_dma, 0);
    assert!(!frame.mic_present_in_dma);
}

#[test]
fn ccmp_data_rx_accepts_a_dma_view_ending_inside_the_verified_mic() {
    const HEADER_LENGTH: usize = 24;
    const LLC_LENGTH: usize = 8;
    const ARP_AND_PADDING_LENGTH: usize = 46;
    const MPDU_LENGTH: usize = HEADER_LENGTH + 8 + LLC_LENGTH + ARP_AND_PADDING_LENGTH + 8;
    const SIGNAL_LENGTH: usize = MPDU_LENGTH + 4;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    // The external-LAN ARP hardware frame retained the first two MIC bytes.
    const DMA_FRAME_LENGTH: usize = MPDU_LENGTH - 6;
    const RECEIVED: usize = FRAME_OFFSET + DMA_FRAME_LENGTH;

    let mut storage = [0_u8; 192];
    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    storage[FRAME_OFFSET] = 0x08;
    storage[FRAME_OFFSET + 1] = 0x42;
    storage[FRAME_OFFSET + 22] = 0;
    storage[FRAME_OFFSET + HEADER_LENGTH..FRAME_OFFSET + HEADER_LENGTH + 8]
        .copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
    storage[FRAME_OFFSET + HEADER_LENGTH + 8..FRAME_OFFSET + HEADER_LENGTH + 16]
        .copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x06]);
    let segment = RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: rx_landed_word(192, RECEIVED as u32, true),
        buffer: &storage,
        next_descriptor_address: 0,
    };
    let mut output = [0_u8; 128];
    let frame = extract_ccmp_data(
        &[segment],
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
        &mut output,
    )
    .unwrap();

    assert_eq!(frame.mpdu.length, DMA_FRAME_LENGTH);
    assert_eq!(frame.payload_offset, HEADER_LENGTH + 8);
    assert_eq!(frame.payload_length, LLC_LENGTH + ARP_AND_PADDING_LENGTH);
    assert_eq!(frame.mic_offset, MPDU_LENGTH - 8);
    assert_eq!(frame.mic_bytes_in_dma, 2);
    assert!(!frame.mic_present_in_dma);
}

#[test]
fn ccmp_data_rx_rejects_missing_extiv_and_hardware_mic_failure() {
    const SIGNAL_LENGTH: usize = 26 + 8 + 8 + 8 + 4;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const RECEIVED: usize = FRAME_OFFSET + SIGNAL_LENGTH;

    let mut storage = [0_u8; 128];
    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    storage[FRAME_OFFSET] = 0x88;
    storage[FRAME_OFFSET + 1] = 0x42;
    let config = RxIngressConfig {
        ring_entry_limit: 1,
        csi_config: 0,
        flags: 0,
    };
    let mut output = [0_u8; 80];
    {
        let segment = RxSegment {
            descriptor_address: 0x2f00_4000,
            descriptor_word0: rx_landed_word(128, RECEIVED as u32, true),
            buffer: &storage,
            next_descriptor_address: 0,
        };
        assert_eq!(
            extract_ccmp_data(&[segment], config, &mut output),
            Err(RxError::Unsupported)
        );
    }

    storage[FRAME_OFFSET + 26 + 3] = 0x20;
    storage[TAIL_OFFSET + 4] = 0xf5;
    let failed = RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: rx_landed_word(128, RECEIVED as u32, true),
        buffer: &storage,
        next_descriptor_address: 0,
    };
    assert_eq!(
        extract_ccmp_data(&[failed], config, &mut output),
        Err(RxError::MicFailure)
    );
}
