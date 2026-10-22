//! Image-only observations while the existing PHY clock reference is held.
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_hal::{
    domain::RadioDomain,
    phy::i2c::{self, fields::SARADC_ADC_SAR2_INITIAL_CODE_HIGH_ADDR},
    power::PlatformClockBackend,
};
use oer_hil_protocol::base::RejectReason;
use oer_time::{Clock, Duration, Timer};

#[derive(Debug)]
pub enum Point {
    AfterPhyRegistration,
    BeforeRfClose,
    AfterRfWake,
}

/// Sample the named field without changing analog state or clock ownership.
///
/// Registration and wake retain PHY clocks; the pre-close call precedes their
/// release. There is deliberately no read after close with those clocks off.
pub async fn observe<B: PlatformClockBackend, T>(
    domain: &mut RadioDomain<B, T>,
    point: Point,
) -> Result<(), RejectReason> {
    let field = TEMPERATURE_SENSOR_SAR2_STATUS;
    let address = field.address();
    let mut registers = domain.phy_hal();
    let mut started = false;
    for _ in 0..oer_esp32s31_phy::HARDWARE_EDGE_LIMIT {
        if !started {
            started = i2c::try_start_read(&mut registers, address).is_ok();
        }
        if started && let Ok(raw) = i2c::try_finish_read(&registers, address) {
            oer_esp32s31_board::rom::print_fmt(format_args!(
                "OPEN_RADIO_HIL phy=sar2-checkpoint point={point:?} raw={raw:#04x} init={}\n",
                field.extract(raw)
            ));
            return Ok(());
        }
        EmbassyClock
            .wait_until(EmbassyClock.now().saturating_add(Duration::from_micros(1)))
            .await;
    }
    Err(RejectReason::Internal)
}
