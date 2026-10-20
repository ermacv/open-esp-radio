//! Read-only observations through the ownership-bound typed PAC image.
use oer_esp32s31_hal::domain::RadioDomain;
use oer_hil_protocol::base::RejectReason;

// The register image index and the two field decoders come from the published
// model in build.rs, using the PAC's own partition enumeration.
include!(concat!(env!("OUT_DIR"), "/tx_rate_checkpoint.rs"));

struct I2cTxRateControl {
    raw: u32,
    low: u8,
    high: u8,
}

#[derive(Debug)]
pub enum Point {
    AfterPhyRegistration,
    AfterIeee802154CommonPower,
    AfterWifiCommonPower,
}

pub fn observe<T>(domain: &RadioDomain<T>, point: Point) -> Result<(), RejectReason> {
    let value = decode(
        domain
            .phy_register_image(IMAGE_INDEX)
            .ok_or(RejectReason::Internal)?,
    );
    // This is outside the calibration/hold brackets; it adds no PHY writes.
    oer_esp32s31_board::rom::print_fmt(format_args!(
        "OPEN_RADIO_HIL phy=tx-rate-checkpoint point={point:?} raw={:#010x} low={:#x} high={:#x}\n",
        value.raw, value.low, value.high
    ));
    Ok(())
}
