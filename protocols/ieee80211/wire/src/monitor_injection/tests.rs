use super::*;

fn header(frame_control: [u8; 2]) -> [u8; BASE_HEADER_LENGTH] {
    let mut bytes = [0_u8; BASE_HEADER_LENGTH];
    bytes[..2].copy_from_slice(&frame_control);
    bytes[4] = 0x02;
    bytes
}

#[test]
fn every_prefix_of_an_injection_frame_is_handled() {
    let mut bytes = [0_u8; 40];
    bytes[0] = 0x08;
    for len in 0..=bytes.len() {
        if let Ok(frame) = MonitorInjectionMpdu::from_no_fcs(&bytes[..len]) {
            let _ = frame.is_group_addressed();
        }
    }
}

#[test]
fn admits_management_data_and_exact_null_data() {
    let beacon = header([0x80, 0x00]);
    assert_eq!(
        MonitorInjectionMpdu::from_no_fcs(&beacon).unwrap().frame_type(),
        MonitorInjectionFrameType::Management
    );
    let mut data = [0_u8; 30];
    data[..BASE_HEADER_LENGTH].copy_from_slice(&header([0x08, 0x01]));
    assert_eq!(
        MonitorInjectionMpdu::from_no_fcs(&data).unwrap().frame_type(),
        MonitorInjectionFrameType::Data
    );
    let null = header([0x48, 0x01]);
    assert_eq!(
        MonitorInjectionMpdu::from_no_fcs(&null).unwrap().frame_type(),
        MonitorInjectionFrameType::NullData
    );
    assert!(!MonitorInjectionMpdu::from_no_fcs(&beacon).unwrap().is_group_addressed());
    let mut broadcast = beacon;
    broadcast[4] = 0xff;
    assert!(MonitorInjectionMpdu::from_no_fcs(&broadcast).unwrap().is_group_addressed());
}

#[test]
fn refuses_each_geometry_the_boundary_does_not_own() {
    let refuse = |frame_control, sequence: [u8; 2]| {
        let mut bytes = header(frame_control);
        bytes[22..].copy_from_slice(&sequence);
        MonitorInjectionMpdu::from_no_fcs(&bytes).unwrap_err()
    };
    assert_eq!(
        refuse([0x81, 0x00], [0, 0]),
        MonitorInjectionFrameError::UnsupportedProtocolVersion(1)
    );
    assert_eq!(
        refuse([0x08, 0x41], [0, 0]),
        MonitorInjectionFrameError::ProtectedFrame
    );
    assert_eq!(
        refuse([0x08, 0x05], [0, 0]),
        MonitorInjectionFrameError::FragmentedFrame
    );
    assert_eq!(
        refuse([0x08, 0x01], [1, 0]),
        MonitorInjectionFrameError::FragmentedFrame
    );
    assert_eq!(
        refuse([0x08, 0x81], [0, 0]),
        MonitorInjectionFrameError::OrderedFrame
    );
    assert_eq!(
        refuse([0x80, 0x01], [0, 0]),
        MonitorInjectionFrameError::UnsupportedManagementFlags
    );
    assert_eq!(
        refuse([0x08, 0x03], [0, 0]),
        MonitorInjectionFrameError::FourAddressData
    );
    assert_eq!(
        refuse([0x88, 0x01], [0, 0]),
        MonitorInjectionFrameError::QosData
    );
    assert_eq!(
        refuse([0x18, 0x01], [0, 0]),
        MonitorInjectionFrameError::UnsupportedDataSubtype(1)
    );
    assert_eq!(
        refuse([0xd4, 0x00], [0, 0]),
        MonitorInjectionFrameError::UnsupportedFrameType(1)
    );
    let mut long_null = [0_u8; 30];
    long_null[..BASE_HEADER_LENGTH].copy_from_slice(&header([0x48, 0x01]));
    assert_eq!(
        MonitorInjectionMpdu::from_no_fcs(&long_null).unwrap_err(),
        MonitorInjectionFrameError::InvalidNullDataLength {
            expected: BASE_HEADER_LENGTH,
            actual: 30
        }
    );
    assert_eq!(
        MonitorInjectionMpdu::from_no_fcs(&[0x08; 23]).unwrap_err(),
        MonitorInjectionFrameError::TooShort {
            required: BASE_HEADER_LENGTH,
            available: 23
        }
    );
}
