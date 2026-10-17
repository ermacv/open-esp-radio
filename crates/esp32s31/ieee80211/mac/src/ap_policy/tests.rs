use super::*;

#[derive(Default)]
struct Hardware(Option<[u8; 6]>);

impl ApRxPolicyHardware for Hardware {
    fn apply_ap_link_policy(&mut self, access_point: [u8; 6]) {
        self.0 = Some(access_point);
    }

    fn disable_ap_link_policy(&mut self) {
        self.0 = None;
    }
}

#[test]
fn ap_address_moves_as_one_checked_policy_input() {
    let mut hardware = Hardware::default();
    configure_ap_receive_policy(&mut hardware, [2, 0, 0, 0, 0, 1]);
    assert_eq!(hardware.0, Some([2, 0, 0, 0, 0, 1]));
    disable_ap_receive_policy(&mut hardware);
    assert_eq!(hardware.0, None);
}
