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
            task_enable(&task)
        },
        u32::MAX,
    );
}

#[test]
fn opaque_task_state_retains_the_complete_sourced_word() {
    check_images(
        |image| {
            let mut memory = words(image);
            let task =
                bt_mac::BtmacBlePhyInit::host(&mut memory).expect("memory holds the instance");
            task.read_bluetooth_task_diagnostic_image()
        },
        u32::MAX,
    );
}

#[test]
fn production_task_enable_view_is_independent_of_the_diagnostic_word() {
    let diagnostic = 0xa5a5_5a5a;
    let mut memory = words(diagnostic);
    let mut bank = bt_mac::BtmacBlePhyInit::host(&mut memory).expect("instance memory");
    bank.modify_interrupt_enable(|w| {
        w.set_scheduler_run(false);
        w.set_source_17(false);
        w.set_bits_0_13_opaque(oer_mmio::Bits::constant::<0>());
        w.set_bits_15_16_opaque(oer_mmio::Bits::constant::<0>());
        w.set_bits_18_31_opaque(oer_mmio::Bits::constant::<0>());
    });
    assert_eq!(task_enable(&bank), 0);
    assert_eq!(task(&bank).opaque_state, diagnostic);
}

#[test]
fn cig_reset_enable_remains_visible_to_production_admission() {
    let mut memory = words(0);
    let mut bank = bt_mac::BluetoothInterruptBank::host(&mut memory).expect("instance memory");
    bank.modify_irq_enable_0(|w| {
        w.set_iso_cig_reset_source(true);
    });
    let observed = BluetoothInterruptPolicyObservation {
        primary: primary(&bank),
        task_enable: 0,
    };
    assert_eq!(
        observed.admit(),
        Err(BluetoothInterruptPolicyFault::ForeignEnable(observed))
    );
    bank.modify_irq_enable_0(|w| {
        w.set_iso_cig_reset_source(false);
    });
    assert_eq!(
        BluetoothInterruptPolicyObservation {
            primary: primary(&bank),
            task_enable: 0
        }
        .admit(),
        Ok(())
    );
}
