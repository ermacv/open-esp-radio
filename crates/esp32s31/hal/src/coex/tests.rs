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
