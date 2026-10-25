//! Read-only CCA/RX observations around diagnostic Wi-Fi bring-up.
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
    const fn name(self) -> &'static str {
        match self {
            Self::BeforeRx => "BeforeRx",
            Self::AfterRx => "AfterRx",
            Self::BeforeChannel => "BeforeChannel",
            Self::AfterChannel => "AfterChannel",
            Self::BeforeMac => "BeforeMac",
            Self::AfterMac => "AfterMac",
            Self::Settled(_) => "Settled",
            Self::CapturePrepared => "CapturePrepared",
            Self::BeforeRing => "BeforeRing",
            Self::RingOpen => "RingOpen",
            Self::RingSample(_) => "RingSample",
            Self::RingClosed => "RingClosed",
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
    let mac = runtime.wifi_mac_hal().ordinary_tx_queue_snapshot(0);
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
