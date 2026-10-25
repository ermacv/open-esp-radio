use super::{BluetoothBasebandV2TransitionPort, execute_standalone_bluetooth_transition};
use std::vec::Vec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BoundaryStep {
    Body(u8),
    DeviceFence,
}

#[derive(Default)]
struct BoundaryTrace {
    steps: Vec<BoundaryStep>,
}

impl BluetoothBasebandV2TransitionPort for BoundaryTrace {
    fn execute_body(&mut self, gain_parameter: u8) {
        self.steps.push(BoundaryStep::Body(gain_parameter));
    }

    fn order_device_accesses(&mut self) {
        self.steps.push(BoundaryStep::DeviceFence);
    }
}

#[test]
fn shared_transition_closes_the_body_with_one_fence() {
    let mut bluetooth = BoundaryTrace::default();
    execute_standalone_bluetooth_transition(&mut bluetooth, 0x6d);
    assert_eq!(
        bluetooth.steps,
        [BoundaryStep::Body(0x6d), BoundaryStep::DeviceFence]
    );
}

#[test]
fn the_body_writes_the_gain_byte_into_both_gain_fields() {
    use oer_esp32s31_pac::{
        bt_v3_2_baseband::{self, BtV3_2Baseband},
        phy_baseband_config_oracle::{self, PhyBasebandConfigOracle},
        phy_btagc_recovered::{self, PhyBtagcRecovered},
        shared_baseband_tx_timing::{self, SharedBasebandTxTiming},
        shared_radio_init_control::{self, SharedRadioInitControl},
        zbbb_radio_control::{self, ZbbbRadioControl},
    };
    fn words(size: usize) -> Vec<u32> {
        std::vec![0; size.div_ceil(4)]
    }
    let mut btagc_words = words(phy_btagc_recovered::SIZE);
    let mut oracle_words = words(phy_baseband_config_oracle::SIZE);
    let mut baseband_words = words(bt_v3_2_baseband::SIZE);
    let mut init_words = words(shared_radio_init_control::SIZE);
    let mut timing_words = words(shared_baseband_tx_timing::SIZE);
    let mut zbbb_words = words(zbbb_radio_control::SIZE);
    let mut btagc = PhyBtagcRecovered::host(&mut btagc_words).expect("btagc");
    let mut oracle = PhyBasebandConfigOracle::host(&mut oracle_words).expect("oracle");
    let (mut baseband, _) = BtV3_2Baseband::host(&mut baseband_words)
        .expect("baseband")
        .split_rx_info();
    let mut init_control = SharedRadioInitControl::host(&mut init_words).expect("init");
    let mut tx_timing = SharedBasebandTxTiming::host(&mut timing_words).expect("timing");
    let mut zbbb = ZbbbRadioControl::host(&mut zbbb_words).expect("zbbb");
    let mut port = super::BluetoothBasebandV2Transaction {
        btagc: &mut btagc,
        oracle: &mut oracle,
        baseband: &mut baseband,
        init_control: &mut init_control,
        tx_timing: &mut tx_timing,
        zbbb: &mut zbbb,
    };
    // 0x83: the force field keeps the low seven bits (3), the gain image the
    // byte less five (0x7e).
    execute_standalone_bluetooth_transition(&mut port, 0x83);
    assert_eq!(btagc.read_rx_gain_force().dynamic_bits_0_6().get(), 0x03);
    assert_eq!(btagc.read_agc_gain_image().dynamic_bits_2_8().get(), 0x7e);
}
