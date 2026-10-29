#![no_std]
use oer_esp32s31_hal::domain::{BtbbError, BtbbGain, BtbbPublisher};
use oer_esp32s31_hal::owner::PhyRegistrationGrant;

// A BTBB gain is published only with the registration's grant, which only
// the owner that began the registration holds.
pub fn with_the_grant(
    publisher: BtbbPublisher<'_>,
    grant: &PhyRegistrationGrant,
) -> Result<BtbbGain, BtbbError> {
    publisher.publish(grant, 0x4f)
}

#[cfg(ui_reject)]
pub fn without_a_grant(publisher: BtbbPublisher<'_>) -> Result<BtbbGain, BtbbError> {
    publisher.publish(0x4f) // ui-error: E0061
}

// The grant is not copied for a second owner.
#[cfg(ui_reject)]
pub fn copied(grant: &PhyRegistrationGrant) -> PhyRegistrationGrant {
    grant.clone() // ui-error: E0308
}

// A gain is not minted outside the domain.
#[cfg(ui_reject)]
pub fn forged(
    gain: oer_espressif_radio::domain::Gain<oer_esp32s31_hal::owner::PhyRegistrationEpoch>,
) -> BtbbGain {
    BtbbGain(gain) // ui-error: E0423
}
