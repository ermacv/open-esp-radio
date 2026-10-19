use super::{ExternalCoexConfig, ExternalCoexLevel, ExternalCoexRole, ExternalCoexWires};

/// `ic_set_extern_coex` publishes 3 first, then the MID and HIGH levels ESP-IDF
/// passes as 8 and 0xc.
#[test]
fn the_vendor_external_levels_become_the_published_priorities() {
    let config = ExternalCoexConfig::vendor(ExternalCoexRole::Leader, ExternalCoexWires::Three);
    let expected = [3, 8, 0xc].map(|value| {
        oer_esp32s31_pac::ExternalCoexPriority::new(value)
            .unwrap_or_else(|| panic!("{value} is a priority"))
    });
    assert_eq!(config.priorities(), expected);
    let high_first = ExternalCoexConfig {
        levels: [ExternalCoexLevel::High, ExternalCoexLevel::Mid],
        ..config
    };
    assert_eq!(high_first.priorities()[1], expected[2]);
}

/// The grant delay is the 4-bit hardware field: 16 µs and above are not
/// representable and are refused rather than wrapped.
#[test]
fn a_grant_delay_above_the_field_is_refused() {
    let config = ExternalCoexConfig::vendor(ExternalCoexRole::Leader, ExternalCoexWires::Three);
    let delay = |grant_delay_us| ExternalCoexConfig { grant_delay_us, ..config }.grant_delay();
    assert_eq!(delay(15).map(|delay| delay.get()), Some(15));
    assert_eq!(delay(16), None);
    assert_eq!(delay(u8::MAX), None);
}
