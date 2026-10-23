//! Read-only observations through the ownership-bound typed PAC image.
use oer_diagnostics::Note;
use oer_esp32s31_hal::domain::RadioDomain;
use oer_esp32s31_hal::power::PlatformClockBackend;
use oer_hil_protocol::base::RejectReason;

// The register image index and the two field decoders come from the published
// model in build.rs, using the PAC's own partition enumeration.
include!(concat!(env!("OUT_DIR"), "/tx_rate_checkpoint.rs"));

struct I2cTxRateControl {
    raw: u32,
    low: u8,
    high: u8,
}

pub enum Point {
    AfterPhyRegistration,
    AfterIeee802154CommonPower,
    AfterWifiCommonPower,
}

impl Point {
    const fn name(&self) -> &'static str {
        match self {
            Self::AfterPhyRegistration => "AfterPhyRegistration",
            Self::AfterIeee802154CommonPower => "AfterIeee802154CommonPower",
            Self::AfterWifiCommonPower => "AfterWifiCommonPower",
        }
    }
}

pub async fn observe<B: PlatformClockBackend, T>(
    domain: &RadioDomain<B, T>,
    point: Point,
) -> Result<(), RejectReason> {
    let value = decode(
        domain
            .phy_register_image(IMAGE_INDEX)
            .ok_or(RejectReason::Internal)?,
    );
    // This is outside the calibration/hold brackets; it adds no PHY writes.
    crate::transport::CONSOLE
        .note_reliably(&Note::tx_rate_checkpoint(
            point.name(),
            value.raw,
            value.low.into(),
            value.high.into(),
        ))
        .await;
    Ok(())
}
