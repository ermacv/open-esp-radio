//! Image-only observations while the existing PHY clock reference is held.
use oer_diagnostics::Note;
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_hal::{
    domain::RadioDomain,
    phy::i2c::analog_i2c::saradc::adc_sar2_initial_code_high_addr::ADC_SAR2_INITIAL_CODE_HIGH_ADDR,
    power::PlatformClockBackend,
};
use oer_esp32s31_phy::serial as phy_i2c;
use oer_hil_protocol::base::RejectReason;
use oer_time::{Clock, Duration, Timer};

pub enum Point {
    AfterPhyRegistration,
    BeforeRfClose,
    AfterRfWake,
}

impl Point {
    const fn name(&self) -> oer_diagnostics::Name<32> {
        match self {
            Self::AfterPhyRegistration => oer_diagnostics::name!("AfterPhyRegistration"; 32),
            Self::BeforeRfClose => oer_diagnostics::name!("BeforeRfClose"; 32),
            Self::AfterRfWake => oer_diagnostics::name!("AfterRfWake"; 32),
        }
    }
}

/// Sample the named field without changing analog state or clock ownership.
///
/// Registration and wake retain PHY clocks; the pre-close call precedes their
/// release. There is deliberately no read after close with those clocks off.
pub async fn observe<B: PlatformClockBackend, T>(
    domain: &mut Option<RadioDomain<B, T>>,
    fault: oer_esp32s31_hal::domain::transaction::FaultHandle,
    point: Point,
) -> Result<(), RejectReason> {
    let field = ADC_SAR2_INITIAL_CODE_HIGH_ADDR;
    let address = field.address();
    let mut transaction = crate::radio::require_radio(domain.take(), fault)
        .detach(oer_esp32s31_hal::domain::transaction::Operation::Maintain);
    let result = async {
        let registers = transaction.hardware_mut();
        let mut started = false;
        for _ in 0..oer_esp32s31_phy::HARDWARE_EDGE_LIMIT {
            if !started {
                started = phy_i2c::try_start_read(registers, address).is_ok();
            }
            if started && let Ok(raw) = phy_i2c::try_finish_read(registers, address) {
                crate::transport::CONSOLE
                    .note_reliably(&Note::sar2_checkpoint(
                        point.name(),
                        raw.into(),
                        field.extract(raw).into(),
                    ))
                    .await;
                return Ok(());
            }
            EmbassyClock
                .wait_until(EmbassyClock.now().saturating_add(Duration::from_micros(1)))
                .await;
        }
        Err(RejectReason::Internal)
    }
    .await;
    *domain = Some(transaction.reunite());
    result
}
