//! Value reconstruction over host PAC memory; no hardware read-effect claim.
#![forbid(unsafe_code)]

use super::*;
use pac::{bt_mac, instances};

fn words(image: u32) -> std::vec::Vec<u32> {
    std::vec![image; instances::bt_mac::SIZE.div_ceil(core::mem::size_of::<u32>())]
}

fn check_images(observe: impl Fn(u32) -> u32, included: u32) {
    for image in [0, u32::MAX, 0xa5a5_5a5a, 0x1234_5678] {
        assert_eq!(observe(image), image & included);
    }
    for bit in 0..u32::BITS {
        let image = 1u32 << bit;
        assert_eq!(observe(image), image & included);
        assert_eq!(observe(!image), !image & included);
    }
}

#[test]
fn primary_bank_0_preserves_the_complete_enable_image() {
    check_images(
        |image| {
            let mut memory = words(image);
            let bank = bt_mac::BluetoothInterruptBank::host(&mut memory)
                .expect("memory holds the instance");
            irq_enable_0(bank.read_irq_enable_0())
        },
        u32::MAX,
    );
}

#[test]
fn primary_bank_1_preserves_the_complete_enable_image() {
    check_images(
        |image| {
            let mut memory = words(image);
            let bank = bt_mac::BluetoothInterruptBank::host(&mut memory)
                .expect("memory holds the instance");
            irq_enable_1(bank.read_irq_enable_1())
        },
        u32::MAX,
    );
}

#[test]
fn task_bank_preserves_the_complete_enable_image() {
    check_images(
        |image| {
            let mut memory = words(image);
            let task =
                bt_mac::BtmacBlePhyInit::host(&mut memory).expect("memory holds the instance");
            interrupt_enable(task.read_interrupt_enable())
        },
        u32::MAX,
    );
}

#[test]
fn opaque_task_state_retains_only_the_modeled_lanes() {
    check_images(
        |image| {
            let mut memory = words(image);
            let task =
                bt_mac::BtmacBlePhyInit::host(&mut memory).expect("memory holds the instance");
            opaque_task_state(task.read_word_04bc_opaque())
        },
        bt_mac::word_04bc_opaque::FIELDS,
    );
}
