use super::*;

const HE20_CAPABILITY: [u8; 24] = [
    255, 22, 35, 0x03, 0x18, 0x9c, 0xca, 0x10, 0x80, 0x00, 0x10, 0x8a, 0x1b, 0x0d, 0xc0, 0x1f,
    0x00, 0x02, 0x82, 0x01, 0xfd, 0xff, 0xfd, 0xff,
];
const HE20_OPERATION: [u8; 9] = [255, 7, 36, 4, 0, 0, 5, 0xfd, 0xff];

#[derive(Default)]
struct TouchDetector {
    touched: bool,
}

impl He20PeerHardware for TouchDetector {
    fn program_he20_peer(
        &mut self,
        _config: MacHe20PeerConfig,
        _rts_threshold: Option<u16>,
    ) -> Result<(), MacHe20PeerError> {
        self.touched = true;
        Ok(())
    }

    fn program_he20_association(
        &mut self,
        _association_id: MacAssociationId,
        _minimum_mpdu_start_spacing: MacMinimumMpduStartSpacing,
        _bssid_index: u8,
    ) {
        self.touched = true;
    }

    fn initialize_he_buffer_status_report(&mut self) {
        self.touched = true;
    }
}

#[test]
fn invalid_association_inputs_fail_before_hardware_access() {
    let state = parse_he20_peer_state(&HE20_CAPABILITY, &HE20_OPERATION).unwrap();
    let mut hardware = TouchDetector::default();
    assert_eq!(
        program_he20_peer_state(&mut hardware, state, 0, 0, 0),
        Err(He20InstallError::InvalidAssociationId(0))
    );
    assert!(!hardware.touched);

    assert_eq!(
        program_he20_peer_state(&mut hardware, state, 1, 8, 0),
        Err(He20InstallError::InvalidMinimumMpduStartSpacing(8))
    );
    assert!(!hardware.touched);
}

#[test]
fn forged_peer_fields_fail_before_hardware_access() {
    let mut state = parse_he20_peer_state(&HE20_CAPABILITY, &HE20_OPERATION).unwrap();
    state.bss_color = u8::MAX;
    let mut hardware = TouchDetector::default();
    assert_eq!(
        program_he20_peer_state(&mut hardware, state, 1, 0, 0),
        Err(He20InstallError::InvalidPeerState)
    );
    assert!(!hardware.touched);
}
