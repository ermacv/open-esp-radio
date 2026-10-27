//! The system timer's owners over host memory from SYSTIMER's base.
//!
//! Word offsets are ESP-IDF 4d80c1b2 `systimer_reg.h`'s: `CONF` 0x0,
//! `UNIT0_OP` 0x4, `TARGETn_HI/LO` 0x1c + 8n / 0x20 + 8n, `TARGETn_CONF`
//! 0x34 + 4n, `UNIT0_VALUE_HI/LO` 0x40/0x44, `COMPn_LOAD` 0x50 + 4n,
//! `INT_ENA` 0x64, `INT_CLR` 0x6c.

use std::{vec, vec::Vec};

use oer_esp32s31_pac::instances;
use oer_mmio::InterruptSource;

use super::{
    AlarmPart, Comparator, SystimerAlarm0, SystimerAlarm1, SystimerAlarm2, SystimerComparators,
    SystimerCounter, alarms_host,
};

fn memory() -> Vec<u32> {
    vec![0; instances::systimer::SIZE / 4]
}

/// The word at byte offset `offset`.
fn word(words: &[u32], offset: usize) -> u32 {
    words[offset / 4]
}

#[test]
fn programming_comparator_one_loads_its_target_and_enables_it() {
    let mut words = memory();
    let mut comparators = SystimerComparators::host(&mut words).expect("memory holds the block");
    comparators.program(Comparator::One, Some(0x000a_bcde_1234_5678));
    drop(comparators);
    let mut expected = memory();
    expected[0x24 / 4] = 0x000a_bcde;
    expected[0x28 / 4] = 0x1234_5678;
    expected[0x54 / 4] = 1;
    // SOURCE: ESP-IDF 4d80c1b2 `systimer_reg.h`: `TARGET1_WORK_EN` bit 23.
    expected[0] = 1 << 23;
    assert_eq!(words, expected);
}

#[test]
fn no_deadline_only_disables_the_comparator() {
    let mut words = memory();
    words[0] = u32::MAX;
    let mut comparators = SystimerComparators::host(&mut words).expect("memory holds the block");
    comparators.program(Comparator::Two, None);
    drop(comparators);
    let mut expected = memory();
    // SOURCE: ESP-IDF 4d80c1b2 `systimer_reg.h`: `TARGET2_WORK_EN` bit 22.
    expected[0] = !(1 << 22);
    assert_eq!(words, expected);
}

#[test]
fn binding_sets_one_shot_mode_acknowledges_then_enables_the_interrupt() {
    let mut words = memory();
    words[0x34 / 4] = u32::MAX;
    words[0] = u32::MAX;
    let before = words.clone();
    {
        let mut comparators =
            SystimerComparators::host(&mut words).expect("memory holds the block");
        let mut alarm_words = memory();
        let (mut zero, _, _) = alarms_host(&mut alarm_words).expect("memory holds the block");
        comparators.bind(&mut zero);
        drop(zero);
        // SOURCE: ESP-IDF 4d80c1b2 `systimer_reg.h`: `TARGET0_INT_CLR` bit 0.
        assert_eq!(
            word(&alarm_words, 0x6c),
            1,
            "the stale interrupt is acknowledged"
        );
    }
    let conf = word(&words, 0x34);
    // SOURCE: ESP-IDF 4d80c1b2 `systimer_reg.h`: `TARGET0_PERIOD_MODE` bit
    // 30, `TARGET0_TIMER_UNIT_SEL` bit 31.
    assert_eq!(conf, !(0b11 << 30), "one shot on unit zero");
    assert_eq!(word(&words, 0), before[0] & !(1 << 24), "disabled");
    assert_eq!(word(&words, 0x64), 1, "comparator 0's interrupt enabled");
}

#[test]
fn each_alarm_part_strobes_its_own_bit_and_names_its_comparator() {
    let mut words = memory();
    let (_, _, mut two) = alarms_host(&mut words).expect("memory holds the block");
    two.acknowledge();
    drop(two);
    assert_eq!(word(&words, 0x6c), 1 << 2);
    assert_eq!(
        [
            SystimerAlarm0::<oer_mmio::Host<'_>>::COMPARATOR,
            SystimerAlarm1::<oer_mmio::Host<'_>>::COMPARATOR,
            SystimerAlarm2::<oer_mmio::Host<'_>>::COMPARATOR,
        ],
        [Comparator::Zero, Comparator::One, Comparator::Two]
    );
    assert_eq!(
        [
            <SystimerAlarm0 as InterruptSource>::SOURCE,
            <SystimerAlarm1 as InterruptSource>::SOURCE,
            <SystimerAlarm2 as InterruptSource>::SOURCE,
        ],
        [
            oer_esp32s31_pac::Interrupt::SYSTIMER_TARGET0.source(),
            oer_esp32s31_pac::Interrupt::SYSTIMER_TARGET1.source(),
            oer_esp32s31_pac::Interrupt::SYSTIMER_TARGET2.source(),
        ]
    );
}

#[test]
fn the_counter_reads_its_latched_words_and_strobes_its_update() {
    let mut words = memory();
    // SOURCE: ESP-IDF 4d80c1b2 `systimer_reg.h`: `TIMER_UNIT0_VALUE_VALID`
    // bit 29, `TIMER_UNIT0_UPDATE` bit 30.
    words[0x4 / 4] = 1 << 29;
    words[0x40 / 4] = 0x000f_ffff;
    words[0x44 / 4] = 0x8765_4321;
    let counter = SystimerCounter::host(&mut words).expect("memory holds the block");
    assert!(counter.valid());
    assert_eq!((counter.high(), counter.low()), (0x000f_ffff, 0x8765_4321));
    counter.update();
    assert_eq!(word(&words, 0x4), 1 << 30, "the strobe is stored alone");
}
