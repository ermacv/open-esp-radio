use super::*;
use oer_ieee80211_wire::extensions::wmm::WmmParameterSet;
use oer_ieee80211_wire::qos::WmmAccessCategory;

fn best_effort() -> ModelWindow {
    ModelWindow::from_wmm(
        WmmParameterSet::STANDARD_DEFAULT.access_category(WmmAccessCategory::BestEffort),
    )
}

#[test]
fn the_window_doubles_to_cw_max_and_resets_to_cw_min() {
    let mut window = best_effort();
    assert_eq!(window.cw(), 15);
    for expected in [31, 63, 127, 255, 511, 1023, 1023] {
        window.record_failure();
        assert_eq!(window.cw(), expected);
    }
    window.reset();
    assert_eq!(window.cw(), 15);
}

#[test]
fn draws_stay_inside_the_window_and_zero_exponent_draws_zero() {
    let window = best_effort();
    assert_eq!(window.draw(u32::MAX), 15);
    assert_eq!(window.draw(0x0001_0007), 7);
    let mut zero = window;
    zero.reconfigure(0, 0);
    assert_eq!(zero.cw(), 0);
    assert_eq!(zero.draw(u32::MAX), 0);
}

#[test]
fn reconfiguration_clamps_the_current_window_into_the_new_bounds() {
    let mut window = best_effort();
    window.record_failure();
    window.record_failure();
    window.reconfigure(2, 5);
    assert_eq!(window.cw(), 31);
    window.reconfigure(6, 3);
    assert_eq!(window.cw(), 63);
    window.reconfigure(20, 20);
    assert_eq!(window.cw(), 0x7fff);
}
