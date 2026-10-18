use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use super::*;

#[test]
fn protocol_consumer_reports_its_retained_owner_capacity() {
    type Consumer = AccessPointRxConsumer<'static, 'static, NoopRawMutex, 32, 1700, 32>;

    assert_eq!(
        <Consumer as AccessPointRxProtocolConsumer>::MAXIMUM_RETAINED_FRAMES,
        32
    );
}

#[test]
fn active_tx_admits_data_and_observation_only_control_frames() {
    let mut protected = [0_u8; PUBLIC_HEADER_SIZE + 2];
    protected[PUBLIC_HEADER_SIZE..].copy_from_slice(&0x4008_u16.to_le_bytes());
    assert!(can_process_ap_frame_during_tx(
        RxSegment {
            descriptor_address: 0,
            descriptor_word0: 0,
            buffer: &protected,
            next_descriptor_address: 0,
        },
        LinkProtection::Ccmp,
    ));

    let mut control = protected;
    control[PUBLIC_HEADER_SIZE..].copy_from_slice(&0x00b4_u16.to_le_bytes());
    assert!(can_process_ap_frame_during_tx(
        RxSegment {
            descriptor_address: 0,
            descriptor_word0: 0,
            buffer: &control,
            next_descriptor_address: 0,
        },
        LinkProtection::Ccmp,
    ));

    let mut management = protected;
    management[PUBLIC_HEADER_SIZE..].copy_from_slice(&0_u16.to_le_bytes());
    assert!(!can_process_ap_frame_during_tx(
        RxSegment {
            descriptor_address: 0,
            descriptor_word0: 0,
            buffer: &management,
            next_descriptor_address: 0,
        },
        LinkProtection::Ccmp,
    ));
}
