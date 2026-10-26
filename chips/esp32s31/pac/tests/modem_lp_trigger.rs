//! Bus-image contracts of the silicon-qualified mixed mode word. The host
//! memory models a bus write, not the silicon's subsequent latch transition.
use oer_esp32s31_pac::{
    instances,
    modem_pwr::{BtdmRuntimeControl, word_0058_opaque as mode},
};

#[test]
fn mode_trigger_preserves_every_other_bit_of_the_fresh_read() {
    let mut memory = vec![0; instances::modem_pwr::SIZE / size_of::<u32>()];
    let slot = mode::OFFSET / size_of::<u32>();
    // Include an asserted read-only CONTROL_2, as observed on silicon, and
    // opaque lanes. Start with the trigger lane clear; preserve all others.
    let inherited = (0xa5a5_5a5a | mode::CONTROL_2_MASK) & !mode::CONTROL_1_MASK;
    memory[slot] = inherited;
    {
        let mut owner = BtdmRuntimeControl::host(&mut memory).expect("owned host aperture");
        assert!(owner.read_word_0058_opaque().control_2());
        owner.trigger_word_0058_opaque_control_1();
    }
    assert_eq!(memory[slot], inherited | mode::CONTROL_1_MASK);
}

#[test]
fn ordinary_mode_update_never_replays_a_read_trigger_lane() {
    let mut memory = vec![0; instances::modem_pwr::SIZE / size_of::<u32>()];
    let slot = mode::OFFSET / size_of::<u32>();
    // A plain-RW regression would replay this one while changing control0/4.
    let inherited = 0xa5a5_5a5a | mode::CONTROL_1_MASK;
    memory[slot] = inherited;
    {
        let mut owner = BtdmRuntimeControl::host(&mut memory).expect("owned host aperture");
        owner.modify_word_0058_opaque(|w| {
            w.set_control_0(false).set_control_4(true);
        });
    }
    let expected =
        (inherited & !(mode::CONTROL_0_MASK | mode::CONTROL_1_MASK)) | mode::CONTROL_4_MASK;
    assert_eq!(memory[slot], expected);
}
