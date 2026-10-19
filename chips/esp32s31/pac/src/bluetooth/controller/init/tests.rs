extern crate std;

use std::vec::Vec;

use super::{
    BluetoothControllerHalInitConfig, BluetoothHalInitPeriod, BluetoothHalInitScale,
    HalInitOperation, HalInitRegister, HalInitTransaction, execute_hal_init,
};

#[derive(Default)]
struct Recorder {
    operations: Vec<HalInitOperation>,
}

impl HalInitTransaction for Recorder {
    fn apply(&mut self, operation: HalInitOperation) {
        self.operations.push(operation);
    }
}

#[test]
fn standalone_time_scale_uses_two_ticks_per_microsecond() {
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();

    assert_eq!(scale.micros_from_raw_ticks(5_000), 2_500);
    assert_eq!(
        scale.project_raw_ticks(5_001),
        super::BluetoothMicrosecondDeltaProjection {
            whole_micros: 2_500,
            remainder_ticks: 1,
        }
    );
    assert_eq!(
        scale.raw_ticks_from_micros(2_503),
        super::BluetoothRawTickDeltaProjection {
            whole_ticks: 5_006,
            remainder_micros: 0,
        }
    );
    assert_eq!(scale.raw_ticks_from_micros(0x8000_0001).whole_ticks, 2);
}

#[test]
fn time_conversion_depends_on_period_and_retains_both_remainders() {
    for hal_scale in [BluetoothHalInitScale::Eight, BluetoothHalInitScale::Sixteen] {
        for (period, raw_ticks, micros, remainder_ticks, remainder_micros) in [
            (BluetoothHalInitPeriod::Image500, 501, 1_003, 0, 1),
            (BluetoothHalInitPeriod::Image1000, 1_003, 1_003, 0, 0),
            (BluetoothHalInitPeriod::Image2000, 2_007, 1_003, 1, 0),
        ] {
            let scale = BluetoothControllerHalInitConfig::new(hal_scale, 11, 33, period)
                .controller_time_scale();
            let forward = scale.project_raw_ticks(raw_ticks);
            let inverse = scale.raw_ticks_from_micros(micros);
            assert_eq!(forward.whole_micros, micros - u32::from(remainder_micros));
            assert_eq!(forward.remainder_ticks, remainder_ticks);
            assert_eq!(inverse.whole_ticks, raw_ticks - u32::from(remainder_ticks));
            assert_eq!(inverse.remainder_micros, remainder_micros);
        }
    }
}

#[test]
fn complete_transaction_has_semantic_prefix_and_thirty_two_lane_edges() {
    let mut recorder = Recorder::default();
    execute_hal_init(
        &mut recorder,
        BluetoothControllerHalInitConfig::reviewed_standalone(),
    );

    assert_eq!(recorder.operations.len(), 49);
    assert_eq!(
        recorder.operations[..17],
        [
            HalInitOperation::PublishSchedulerSramPrefix,
            HalInitOperation::PublishSleepTimerShift(3),
            HalInitOperation::PublishValue0(22),
            HalInitOperation::PublishValue1(66),
            HalInitOperation::InitializeLatch,
            HalInitOperation::InitializeLow20,
            HalInitOperation::EnableLatch,
            HalInitOperation::ConfigureControl1High,
            HalInitOperation::ConfigureControl1Low,
            HalInitOperation::EnableControl0,
            HalInitOperation::ClearSchedulerConfig16To20,
            HalInitOperation::PublishSchedulerConfig16To20,
            HalInitOperation::EnableSchedulerControl,
            HalInitOperation::ClearLowHalf,
            HalInitOperation::FillLowHalf,
            HalInitOperation::ClearSchedulerByte1,
            HalInitOperation::PublishSchedulerByte1,
        ]
    );

    for (global_index, pair) in recorder.operations[17..].chunks_exact(2).enumerate() {
        let register = if global_index < 8 {
            HalInitRegister::SlotMap0
        } else {
            HalInitRegister::SlotMap1
        };
        let lane = (global_index % 8) as u8;
        let index_in_group = global_index % 4;
        assert_eq!(
            pair,
            [
                HalInitOperation::ClearSlotLaneUpper { register, lane },
                HalInitOperation::PublishSlotLane {
                    register,
                    lane,
                    set_retained_index_low: index_in_group % 2 == 1,
                    index_high: u8::from(index_in_group >= 2),
                },
            ]
        );
    }
}

#[test]
fn physical_duration_projection_preserves_remainders_without_wrapping() {
    for (period, maximum, ticks, remainder) in [
        (
            BluetoothHalInitPeriod::Image500,
            u64::from(u32::MAX) * 2 + 1,
            u32::MAX,
            1,
        ),
        (
            BluetoothHalInitPeriod::Image1000,
            u64::from(u32::MAX),
            u32::MAX,
            0,
        ),
        (
            BluetoothHalInitPeriod::Image2000,
            u64::from(u32::MAX) / 2,
            u32::MAX - 1,
            0,
        ),
    ] {
        let scale =
            BluetoothControllerHalInitConfig::new(BluetoothHalInitScale::Eight, 11, 33, period)
                .controller_time_scale();
        let projected = scale.checked_raw_ticks_from_micros(maximum).unwrap();
        assert_eq!(
            (projected.whole_ticks, projected.remainder_micros),
            (ticks, remainder)
        );
        assert_eq!(scale.checked_raw_ticks_from_micros(maximum + 1), None);
        assert_eq!(scale.checked_raw_ticks_from_micros(u64::MAX), None);
    }
}
