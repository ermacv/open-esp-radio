use super::*;

const fn observation(
    source: CoexistenceLowPowerClockSource,
    divider_minus_one: u16,
) -> Option<CoexistenceLowPowerClockObservation> {
    Some(CoexistenceLowPowerClockObservation {
        source,
        divider_minus_one,
    })
}

#[test]
fn unreviewed_clock_encoding_is_unsupported() {
    assert_eq!(timer_clock(None, true), Err(CoexError::UnsupportedClock));
}

#[test]
fn selector_eight_distinguishes_silicon_from_the_fpga_constant() {
    let sample = observation(CoexistenceLowPowerClockSource::Selector8, 0);
    let silicon = timer_clock(sample, true).unwrap();
    let fpga = timer_clock(sample, false).unwrap();
    assert_eq!(silicon.tick_image(1_000_000), Ok(32_768));
    assert_eq!(fpga.tick_image(1_000_000), Ok(32_000));
}

#[test]
fn divided_selectors_keep_the_decoded_divider_and_board_crystal() {
    let selector_two = timer_clock(
        observation(CoexistenceLowPowerClockSource::Selector2, 39),
        true,
    );
    assert_eq!(selector_two.unwrap().tick_image(1_000_000), Ok(500_000));

    let selector_four = timer_clock(
        observation(CoexistenceLowPowerClockSource::Selector4, 49),
        true,
    );
    assert_eq!(selector_four.unwrap().tick_image(1_000_000), Ok(800_000));

    let selector_one = timer_clock(
        observation(CoexistenceLowPowerClockSource::Selector1, 2),
        true,
    );
    assert_eq!(selector_one.unwrap().tick_image(3), Ok(3 << 19));
}
