//! The lines over host memory from HP_SYS's base: a raise stores one whole,
//! a lower stores zero, and each line reaches its own word alone.
//!
//! Word offsets are ESP-IDF 4d59230d `hp_system_reg.h`'s
//! (`HP_SYSTEM_CPU_INT_FROM_CPU_0_REG` 0x10 .. `_3_REG` 0x1c).

use std::{vec, vec::Vec};

use oer_esp32s31_pac::{hp_system, instances};
use oer_mmio::InterruptSource;

use super::{FromCpu0, FromCpu0Irq, FromCpu1Irq, FromCpu2Irq, FromCpu3, FromCpu3Irq};

fn memory(fill: u32) -> Vec<u32> {
    vec![fill; instances::hp_sys::SIZE / 4]
}

#[test]
fn a_raise_stores_one_and_a_lower_stores_zero_in_the_line_s_word_alone() {
    let mut words = memory(0xa5a5_a5a4);
    let before = words.clone();
    let line = FromCpu0::host(&mut words).expect("host memory holds the block");
    line.raise();
    assert!(line.raised());
    drop(line);
    let mut expected = before.clone();
    expected[0x10 / 4] = 1;
    assert_eq!(words, expected);
    let line = FromCpu0::host(&mut words).expect("host memory holds the block");
    line.lower();
    assert!(!line.raised());
    drop(line);
    expected[0x10 / 4] = 0;
    assert_eq!(words, expected);
}

#[test]
fn line_three_reaches_its_own_word() {
    let mut words = memory(0);
    FromCpu3::host(&mut words)
        .expect("host memory holds the block")
        .raise();
    let raised: Vec<usize> = (0..words.len())
        .filter(|index| words[*index] != 0)
        .collect();
    assert_eq!(raised, [0x1c / 4]);
}

#[test]
fn each_interrupt_part_names_its_line_s_source() {
    assert_eq!(
        [
            FromCpu0Irq::SOURCE,
            FromCpu1Irq::SOURCE,
            FromCpu2Irq::SOURCE,
            FromCpu3Irq::SOURCE,
        ],
        [
            <hp_system::FromCpu0 as InterruptSource>::SOURCE,
            <hp_system::FromCpu1 as InterruptSource>::SOURCE,
            <hp_system::FromCpu2 as InterruptSource>::SOURCE,
            <hp_system::FromCpu3 as InterruptSource>::SOURCE,
        ]
    );
}
