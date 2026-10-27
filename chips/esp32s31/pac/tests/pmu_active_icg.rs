//! Active modem selection changes its field and preserves the inherited word.
use oer_esp32s31_pac::{
    instances,
    pmu::{PmuRadio, hp_active_icg_modem},
};

#[test]
fn active_icg_selection_preserves_inherited_bits_and_other_words() {
    for (inherited, selected) in [
        (0x0000_0000, 0x8000_0000),
        (0x4000_0000, 0x8000_0000),
        (0x8000_0000, 0x8000_0000),
        (0xc000_0000, 0x8000_0000),
        (0xffff_ffff, 0xbfff_ffff),
        (0x5555_5555, 0x9555_5555),
        (0xaaaa_aaaa, 0xaaaa_aaaa),
    ] {
        let mut memory = vec![inherited; instances::pmu::SIZE.div_ceil(size_of::<u32>())];
        let mut expected = memory.clone();
        expected[hp_active_icg_modem::OFFSET / size_of::<u32>()] = selected;
        PmuRadio::host(&mut memory)
            .expect("host memory holds the PMU owner")
            .modify_hp_active_icg_modem(|w| {
                w.set_hp_active_dig_icg_modem_code(
                    hp_active_icg_modem::HpActiveDigIcgModemCode::Active,
                );
            });
        assert_eq!(memory, expected);
    }
}
