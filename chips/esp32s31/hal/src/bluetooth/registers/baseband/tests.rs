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
        bt_bb::{BtV32Baseband, PhyBtagcRecovered, SharedBasebandTxTiming, ZbbbRadioControl},
        fe::PhyBasebandConfigOracleFe,
        instances,
        wifi_bb::PhyBasebandConfigOracleWifiBb,
        wifi_mac::SharedRadioInitControl,
    };
    fn words(size: usize) -> Vec<u32> {
        std::vec![0; size.div_ceil(4)]
    }
    let mut btagc_words = words(instances::bt_bb::SIZE);
    let mut oracle_fe_words = words(instances::fe::SIZE);
    let mut oracle_wifi_bb_words = words(instances::wifi_bb::SIZE);
    let mut baseband_words = words(instances::bt_bb::SIZE);
    let mut init_words = words(instances::wifi_mac::SIZE);
    let mut timing_words = words(instances::bt_bb::SIZE);
    let mut zbbb_words = words(instances::bt_bb::SIZE);
    let mut btagc = PhyBtagcRecovered::host(&mut btagc_words).expect("btagc");
    let mut oracle_fe = PhyBasebandConfigOracleFe::host(&mut oracle_fe_words).expect("oracle");
    let mut oracle_wifi_bb =
        PhyBasebandConfigOracleWifiBb::host(&mut oracle_wifi_bb_words).expect("oracle");
    let mut baseband = BtV32Baseband::host(&mut baseband_words).expect("baseband");
    let mut init_control = SharedRadioInitControl::host(&mut init_words).expect("init");
    let mut tx_timing = SharedBasebandTxTiming::host(&mut timing_words).expect("timing");
    let mut zbbb = ZbbbRadioControl::host(&mut zbbb_words).expect("zbbb");
    let mut port = super::BtbbV2Registers::<HostBtbb<'_>> {
        btagc: &mut btagc,
        oracle_fe: &mut oracle_fe,
        oracle_wifi_bb: &mut oracle_wifi_bb,
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

/// Every instance in host memory.
struct HostBtbb<'a>(core::marker::PhantomData<&'a ()>);

impl<'a> super::BtbbPlacements for HostBtbb<'a> {
    type BtBb = oer_mmio::Host<'a>;
    type Fe = oer_mmio::Host<'a>;
    type WifiBb = oer_mmio::Host<'a>;
    type WifiMac = oer_mmio::Host<'a>;
}
