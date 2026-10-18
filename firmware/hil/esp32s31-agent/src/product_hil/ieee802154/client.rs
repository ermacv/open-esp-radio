//! The IEEE 802.15.4 client shared by the air check and peer sessions.

use core::pin::pin;

use esp_hal::time::Instant;
use oer_esp32s31_ieee802154_system::{
    Esp32s31Ieee802154, Ieee802154Parked, Ieee802154System, Ieee802154SystemBackend,
    Ieee802154SystemParts, start,
};
use oer_adapters_esp_hal_radio::EspHalRadioPlatform;
use oer_espressif_ieee802154_engine::pib::Ieee802154PibDefaults;
use oer_tool_data_hil_protocol::ieee802154::Ieee802154AirTxOutcome;
use oer_ieee802154_mac::TxStatus;

pub(super) fn now_micros() -> u64 {
    Instant::now().duration_since_epoch().as_micros()
}

type Radio = oer_esp32s31_radio_system::SharedRadio;

/// The single-interface client's port backend.
pub(super) type Backend = Ieee802154SystemBackend<Esp32s31Ieee802154>;

/// The concurrently split radio and the IEEE 802.15.4 client's owners. The
/// images are terminal: the radio stays split. The radio is placed by the
/// PHY register-image reader, which reads it under its lease.
pub(super) struct Client {
    pub(super) radio: &'static Radio,
    pub(super) defaults: Ieee802154PibDefaults,
}

impl Client {
    /// Start the radio once for this image. The image runs the coexistence
    /// schedule and PHY tracking itself, for the length of a session.
    pub(super) fn claim(
        spawner: embassy_executor::Spawner,
        platform: EspHalRadioPlatform,
    ) -> Option<(Self, Ieee802154Parked)> {
        use oer_esp32s31_radio_system::{RadioStart, Schedule, Tracking};
        let start = RadioStart::new()
            .with_tracking(Tracking::Caller)
            .with_schedule(Schedule::Caller);
        let (radio, partitions) =
            oer_esp32s31_radio_system::start(spawner, platform, start).ok()?;
        crate::product_hil::phy_register_image::adopt(radio);
        let defaults = Ieee802154PibDefaults::default();
        let parked =
            Ieee802154Parked::new(partitions.ieee802154, partitions.ieee802154_coex, defaults)?;
        Some((Self { radio, defaults }, parked))
    }

    /// Start the client: the running system and its port backend.
    /// Bring-up holds the PHY registration future, so it is pinned in
    /// place.
    pub(super) async fn start(
        &mut self,
        parked: Ieee802154Parked,
    ) -> Option<(Ieee802154System, Backend)> {
        let started = pin!(start::<Esp32s31Ieee802154, _, _>(
            self.radio,
            parked,
            self.defaults
        ));
        started.await.ok()
    }

    /// Stop the client with the parts its ended port returned. Teardown
    /// holds the RF close future, pinned in place.
    pub(super) async fn stop(
        &mut self,
        system: Ieee802154System,
        parts: Ieee802154SystemParts,
    ) -> Option<Ieee802154Parked> {
        let stopped = pin!(system.stop(parts, self.radio));
        stopped.await.ok()
    }
}

pub(super) const fn tx_outcome(status: TxStatus) -> Ieee802154AirTxOutcome {
    match status {
        TxStatus::Success => Ieee802154AirTxOutcome::Success,
        TxStatus::ChannelBusy => Ieee802154AirTxOutcome::ChannelBusy,
        TxStatus::NoAcknowledgement => Ieee802154AirTxOutcome::NoAcknowledgement,
        TxStatus::Aborted => Ieee802154AirTxOutcome::Aborted,
        TxStatus::InvalidFrame => Ieee802154AirTxOutcome::InvalidFrame,
        TxStatus::HardwareFailure => Ieee802154AirTxOutcome::HardwareFailure,
        TxStatus::CoexistenceRejected => Ieee802154AirTxOutcome::CoexistenceRejected,
        TxStatus::SecurityFailure => Ieee802154AirTxOutcome::SecurityFailure,
        TxStatus::InvalidAcknowledgement => Ieee802154AirTxOutcome::InvalidAcknowledgement,
    }
}
