//! Read-only CCA/RX observations around diagnostic Wi-Fi bring-up.
use oer_esp32s31_hal::types::MacTxQueueIndex;
use oer_diagnostics::Note;
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_hal::{
    domain::RadioDomain, ieee80211::client::WifiClocked, power::PlatformClockBackend,
};
use oer_hil_protocol::base::RejectReason;
use oer_time::Clock;

// Generated from the same published enumeration the typed PAC image uses.
include!(concat!(env!("OUT_DIR"), "/cca_checkpoint.rs"));

#[derive(Clone, Copy)]
pub enum Point {
    BeforeRx,
    AfterRx,
    BeforeChannel,
    AfterChannel,
    BeforeMac,
    AfterMac,
    Settled(u8),
    CapturePrepared,
    BeforeRing,
    RingOpen,
    RingSample(u8),
    RingClosed,
}

impl Point {
    /// The variant's name, as the host prints it.
    const fn name(self) -> oer_diagnostics::Name<32> {
        match self {
            Self::BeforeRx => oer_diagnostics::name!("BeforeRx"; 32),
            Self::AfterRx => oer_diagnostics::name!("AfterRx"; 32),
            Self::BeforeChannel => oer_diagnostics::name!("BeforeChannel"; 32),
            Self::AfterChannel => oer_diagnostics::name!("AfterChannel"; 32),
            Self::BeforeMac => oer_diagnostics::name!("BeforeMac"; 32),
            Self::AfterMac => oer_diagnostics::name!("AfterMac"; 32),
            Self::Settled(_) => oer_diagnostics::name!("Settled"; 32),
            Self::CapturePrepared => oer_diagnostics::name!("CapturePrepared"; 32),
            Self::BeforeRing => oer_diagnostics::name!("BeforeRing"; 32),
            Self::RingOpen => oer_diagnostics::name!("RingOpen"; 32),
            Self::RingSample(_) => oer_diagnostics::name!("RingSample"; 32),
            Self::RingClosed => oer_diagnostics::name!("RingClosed"; 32),
        }
    }

    /// The sample number of a point that has one.
    const fn sample(self) -> Option<u8> {
        match self {
            Self::Settled(sample) | Self::RingSample(sample) => Some(sample),
            _ => None,
        }
    }
}

pub async fn observe<B: PlatformClockBackend, T>(
    domain: &RadioDomain<B, T>,
    wifi: WifiClocked,
    point: Point,
) -> Result<WifiClocked, RejectReason> {
    // Both ownership transitions perform no MMIO.
    let (runtime, clocks) = wifi.into_running();
    let result = observe_running(domain, &runtime, point).await;
    let wifi = WifiClocked::from_running(runtime, clocks);
    result?;
    Ok(wifi)
}

pub async fn observe_running<B: PlatformClockBackend, T>(
    domain: &RadioDomain<B, T>,
    runtime: &oer_esp32s31_hal::owner::RadioAccess,
    point: Point,
) -> Result<(), RejectReason> {
    let dma = runtime.receive_dma_snapshot();
    crate::transport::CONSOLE
        .note_reliably(&Note::cca_checkpoint(
            point.name(),
            point.sample(),
            EmbassyClock.now().as_micros(),
            dma.walker_enabled,
            dma.reload_pending,
            dma.descriptor_base,
            dma.next_descriptor_low,
            dma.last_descriptor_low,
        ))
        .await;
    for &(name, index) in REGISTERS {
        let raw = domain
            .phy_register_image(index)
            .ok_or(RejectReason::Internal)?;
        crate::transport::CONSOLE
            .note_reliably(&Note::cca_register(point.name(), point.sample(), name, raw))
            .await;
    }
    Ok(())
}

/// Read-only core preparation and post-capture RX observations.
pub async fn observe_enables<B: PlatformClockBackend, T>(
    domain: &RadioDomain<B, T>,
    runtime: &mut oer_esp32s31_hal::owner::RadioAccess,
    point: Point,
) -> Result<(), RejectReason> {
    // This existing read-only projection samples the shared CCA force fields
    // without publishing or acknowledging anything on the unused TX queue.
    let mac = runtime
        .wifi_mac_hal()
        .ordinary_tx_queue_snapshot(MacTxQueueIndex::Zero);
    crate::transport::CONSOLE
        .note_reliably(&Note::cca_enable(
            point.name(),
            point.sample(),
            mac.cca_force.into(),
            mac.cca_aux_force.into(),
            mac.mac_active_state.into(),
        ))
        .await;
    for &(name, index) in ENABLE_REGISTERS {
        let raw = domain
            .phy_register_image(index)
            .ok_or(RejectReason::Internal)?;
        crate::transport::CONSOLE
            .note_reliably(&Note::cca_enable_register(
                point.name(),
                point.sample(),
                name,
                raw,
            ))
            .await;
    }
    Ok(())
}
