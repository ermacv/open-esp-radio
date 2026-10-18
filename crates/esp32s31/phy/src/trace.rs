//! This PHY's outcomes as the chip-neutral [`oer_phy_trace`] events.
//!
//! Only the mapping lives here; the lifecycle operations emit where their
//! outcome is known. Without the `trace` feature every emission compiles to
//! nothing.

use oer_phy_trace::{
    CalibrationPath, Client, Clients, Refusal, TemperatureReferences, TrackingProgress,
};

use crate::{
    PhyState,
    calibration::registration::PhyCalibrationPath,
    concurrent::ConcurrentPhyError,
    state::client::{PhyClientSnapshot, RadioClient},
    tracking::PhyParamTrackingOutcome,
};

pub(crate) const fn client(client: RadioClient) -> Client {
    match client {
        RadioClient::Wifi => Client::Wifi,
        RadioClient::Bluetooth => Client::Bluetooth,
        RadioClient::Ieee802154 => Client::Ieee802154,
    }
}

pub(crate) const fn clients(snapshot: PhyClientSnapshot) -> Clients {
    let mut clients = Clients::NONE;
    if snapshot.contains(RadioClient::Wifi) {
        clients = clients.with(Client::Wifi);
    }
    if snapshot.contains(RadioClient::Bluetooth) {
        clients = clients.with(Client::Bluetooth);
    }
    if snapshot.contains(RadioClient::Ieee802154) {
        clients = clients.with(Client::Ieee802154);
    }
    clients
}

pub(crate) const fn refusal(error: ConcurrentPhyError) -> Refusal {
    match error {
        ConcurrentPhyError::NotRegistered => Refusal::NotRegistered,
        ConcurrentPhyError::AlreadyRegistered => Refusal::AlreadyRegistered,
        ConcurrentPhyError::TrackingPending => Refusal::TrackingPending,
        ConcurrentPhyError::NoTrackingPending => Refusal::NoTrackingPending,
        ConcurrentPhyError::Poisoned => Refusal::Poisoned,
        ConcurrentPhyError::RfClosed => Refusal::RfClosed,
        ConcurrentPhyError::RfOpen => Refusal::RfOpen,
        ConcurrentPhyError::EpochMismatch => Refusal::EpochMismatch,
        ConcurrentPhyError::ClientsActive => Refusal::ClientsActive,
        ConcurrentPhyError::Clock(_) => Refusal::Clock,
        ConcurrentPhyError::Acquire(_) => Refusal::Acquire,
        ConcurrentPhyError::Release(_) => Refusal::Release,
        ConcurrentPhyError::Time(_) => Refusal::Time,
        ConcurrentPhyError::ClientAbsent(absent) => Refusal::ClientAbsent(client(absent)),
    }
}

pub(crate) const fn calibration_path(path: PhyCalibrationPath) -> CalibrationPath {
    match path {
        PhyCalibrationPath::FullUncached => CalibrationPath::FullUncached,
        PhyCalibrationPath::FullForCache => CalibrationPath::FullForCache,
        PhyCalibrationPath::FullAfterRejectedCache => CalibrationPath::FullAfterRejectedCache,
        PhyCalibrationPath::PartialFromCache => CalibrationPath::PartialFromCache,
    }
}

pub(crate) const fn progress(outcome: &PhyParamTrackingOutcome) -> TrackingProgress {
    TrackingProgress {
        wifi_requested: outcome.clients.wifi(),
        bluetooth_ieee802154_requested: outcome.clients.bluetooth_ieee802154(),
        inhibited: outcome.tracking_inhibited,
        rfpll_corrected: outcome.rfpll_corrected,
        tx_power_wifi: outcome.tx_power.wifi,
        tx_power_bluetooth_ieee802154: outcome.tx_power.bluetooth_ieee802154,
        calibration_common: outcome.calibration.common,
        calibration_transmit: outcome.calibration.transmit,
        calibration_wifi: outcome.calibration.wifi,
        calibration_bluetooth_ieee802154: outcome.calibration.bluetooth_ieee802154,
    }
}

/// Whether a tracking run committed a temperature reference.
pub(crate) const fn committed_reference(outcome: &PhyParamTrackingOutcome) -> bool {
    outcome.rfpll_corrected
        || outcome.calibration.common
        || outcome.calibration.transmit
        || outcome.tx_power.wifi
        || outcome.tx_power.bluetooth_ieee802154
}

#[cfg(feature = "trace")]
/// The radio's platform clock references, saturating, in
/// [`PlatformClock::index`](oer_esp32s31_hal::power::PlatformClock::index) order.
pub(crate) fn platform_clocks(
    holds: oer_esp32s31_hal::power::PlatformClockHolds,
) -> [u8; oer_phy_trace::PLATFORM_CLOCKS] {
    let mut counts = [0; oer_phy_trace::PLATFORM_CLOCKS];
    for clock in oer_esp32s31_hal::power::PlatformClock::ALL {
        counts[clock.index()] = u8::try_from(holds.get(clock)).unwrap_or(u8::MAX);
    }
    counts
}

pub(crate) fn temperatures(state: &PhyState) -> TemperatureReferences {
    let references = state.temperature_references();
    let narrow = |celsius: i16| celsius.clamp(i16::from(i8::MIN), i16::from(i8::MAX)) as i8;
    TemperatureReferences {
        rfpll: narrow(references.rfpll()),
        calibration: narrow(references.common()),
        transmit: narrow(references.transmit()),
        power: narrow(references.power()),
        observed: state.temperature_observation().value,
    }
}

#[cfg(test)]
mod tests;
