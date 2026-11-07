use super::*;

#[test]
fn absolute_temperature_matches_rv32_wrapping_abs() {
    assert_eq!(absolute_temperature(0), 0);
    assert_eq!(absolute_temperature(-123), 123);
    assert_eq!(absolute_temperature(i32::MIN), 0x8000_0000);
}

#[test]
fn signed_saturation_preserves_rom_comparison_order() {
    for raw in 0..=u16::MAX {
        let value = raw as i16 as i32;
        assert_eq!(saturate_signed(value, 80, -60), value.clamp(-60, 80));
        assert_eq!(saturate_signed(value, 105, -60), value.clamp(-60, 105));
    }

    // Standard clamp contracts reject inverted bounds. The ROM still has
    // defined instruction-level behavior, and comparison order matters.
    assert_eq!(saturate_signed(4, -5, 5), -5);
    assert_eq!(saturate_signed(-5, -5, 5), 5);
}
