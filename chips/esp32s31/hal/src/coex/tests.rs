use super::{ExternalCoexConfig, ExternalCoexLevel, ExternalCoexRole, ExternalCoexWires};

/// `ic_set_extern_coex` publishes 3 first, then the MID and HIGH levels ESP-IDF
/// passes as 8 and 0xc.
#[test]
fn the_vendor_external_levels_become_the_published_priorities() {
    let config = ExternalCoexConfig::vendor(ExternalCoexRole::Leader, ExternalCoexWires::Three);
    let expected = [3, 8, 0xc].map(|value| {
        crate::modem::ExternalCoexPriority::new(value)
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
    let delay = |grant_delay_us| {
        ExternalCoexConfig {
            grant_delay_us,
            ..config
        }
        .grant_delay()
    };
    assert_eq!(delay(15).map(|delay| delay.get()), Some(15));
    assert_eq!(delay(16), None);
    assert_eq!(delay(u8::MAX), None);
}

#[test]
fn clock_port_observes_each_configuration_afresh_and_refuses_unknown_sources() {
    use super::CoexClockPort;
    use crate::types::{CoexTimerClockDividerMinusOne, CoexTimerClockSource};
    use oer_espressif_coex::{CoexClockHardware, CoexError};
    crate::phy::registers::host::with_registers(0, |registers| {
        let mut clock = CoexClockPort::new(registers);
        assert_eq!(clock.sample(), Err(CoexError::UnsupportedClock));
        clock.configure_timer_clock(
            CoexTimerClockSource::Selector4,
            CoexTimerClockDividerMinusOne::new(49).unwrap(),
        );
        assert_eq!(clock.sample().unwrap().tick_image(1_000_000), Ok(800_000));
        clock.configure_timer_clock(
            CoexTimerClockSource::Selector8,
            CoexTimerClockDividerMinusOne::new(0).unwrap(),
        );
        assert_eq!(clock.sample().unwrap().tick_image(1_000_000), Ok(32_768));
    });
}

#[test]
fn policy_port_cannot_change_the_reserved_grant_timer() {
    use super::{CoexPolicyTimer, CoexPolicyTimerBank, CoexPtiTable};
    use crate::modem::{
        CoexTimerBankRegisters,
        values::{CoexTimerClientValue, CoexTimerPtiValue},
    };
    use oer_esp32s31_pac::{
        coex::{CoexPolicyTimers, PhyGrantProtectTimer},
        instances,
    };
    let mut policy_words = std::vec![0; instances::coex::SIZE.div_ceil(4)];
    let mut grant_words = std::vec![0x5a5a_31c0; instances::coex::SIZE.div_ceil(4)];
    let original = grant_words.clone();
    let mut policy = CoexPolicyTimers::host(&mut policy_words).unwrap();
    let mut grant = PhyGrantProtectTimer::host(&mut grant_words).unwrap();
    let table = CoexPtiTable::VENDOR;
    let mut timers =
        CoexPolicyTimerBank::new(CoexTimerBankRegisters::new(&mut policy, &mut grant), &table);
    for timer in CoexPolicyTimer::ALL {
        timers.configure(
            timer,
            CoexTimerClientValue::new(2).unwrap(),
            CoexTimerPtiValue::new(7).unwrap(),
        );
        timers.set_primary_target(timer, 1234);
        timers.set_secondary_target(timer, 4321);
        timers.enable(timer);
        timers.force(timer);
        timers.unforce(timer);
        timers.disable(timer);
    }
    drop(timers);
    drop((policy, grant));
    assert_eq!(grant_words, original);
    assert!(CoexPolicyTimer::new(5).is_none());
}
