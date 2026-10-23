//! Read-only CCA/RX observations around diagnostic Wi-Fi bring-up.
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_hal::{
    domain::RadioDomain, ieee80211::client::WifiClocked, power::PlatformClockBackend,
};
use oer_hil_protocol::base::RejectReason;
use oer_time::Clock;

// Generated from the same published enumeration the typed PAC image uses.
include!(concat!(env!("OUT_DIR"), "/cca_checkpoint.rs"));

#[derive(Clone, Copy, Debug)]
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

pub async fn observe<B: PlatformClockBackend, T>(
    domain: &RadioDomain<B, T>,
    wifi: WifiClocked,
    point: Point,
) -> Result<WifiClocked, RejectReason> {
    // Both ownership transitions perform no MMIO.
    let (runtime, interrupts, clocks) = wifi.into_running();
    let result = observe_running(domain, &runtime, point).await;
    let wifi = WifiClocked::from_running(runtime, interrupts, clocks);
    result?;
    Ok(wifi)
}

pub async fn observe_running<B: PlatformClockBackend, T>(
    domain: &RadioDomain<B, T>,
    runtime: &oer_esp32s31_hal::owner::RadioAccess,
    point: Point,
) -> Result<(), RejectReason> {
    let dma = runtime.receive_dma_snapshot();
    let sample = match point {
        Point::Settled(sample) | Point::RingSample(sample) => Some(sample),
        _ => None,
    };
    crate::transport::CONSOLE.line_reliably(format_args!(
        "OPEN_RADIO_HIL phy=cca-checkpoint point={point:?} sample={sample:?} micros={} mac_rx={} reload={} base={:#x} next={:#x} last={:#x}",
        EmbassyClock.now().as_micros(),
        dma.walker_enabled,
        dma.reload_pending,
        dma.descriptor_base,
        dma.next_descriptor_low,
        dma.last_descriptor_low,
    )).await;
    for &(name, index) in REGISTERS {
        let raw = domain
            .phy_register_image(index)
            .ok_or(RejectReason::Internal)?;
        crate::transport::CONSOLE
            .line_reliably(format_args!(
                "OPEN_RADIO_HIL phy=cca-register point={point:?} name={name} raw={raw:#010x}",
            ))
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
        .line_reliably(format_args!(
            "OPEN_RADIO_HIL phy=cca-enable point={point:?} force={} aux_force={} mac_active={}",
            mac.cca_force, mac.cca_aux_force, mac.mac_active_state,
        ))
        .await;
    for &(name, index) in ENABLE_REGISTERS {
        let raw = domain
            .phy_register_image(index)
            .ok_or(RejectReason::Internal)?;
        crate::transport::CONSOLE.line_reliably(format_args!(
            "OPEN_RADIO_HIL phy=cca-enable-register point={point:?} name={name} raw={raw:#010x}",
        )).await;
    }
    Ok(())
}
