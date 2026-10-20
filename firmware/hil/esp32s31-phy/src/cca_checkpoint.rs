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
}

pub fn observe<B: PlatformClockBackend, T>(
    domain: &RadioDomain<B, T>,
    wifi: WifiClocked,
    point: Point,
) -> Result<WifiClocked, RejectReason> {
    // Both ownership transitions perform no MMIO. No DMA storage or interrupt
    // route is installed; the observation borrows the existing typed RX owner.
    let (runtime, interrupts, clocks) = wifi.into_running();
    let dma = runtime.receive_dma_snapshot();
    let wifi = WifiClocked::from_running(runtime, interrupts, clocks);
    let sample = match point {
        Point::Settled(sample) => Some(sample),
        _ => None,
    };
    oer_esp32s31_board::rom::print_fmt(format_args!(
        "OPEN_RADIO_HIL phy=cca-checkpoint point={point:?} sample={sample:?} micros={} mac_rx={} reload={} base={:#x} next={:#x} last={:#x}\n",
        EmbassyClock.now().as_micros(),
        dma.walker_enabled,
        dma.reload_pending,
        dma.descriptor_base,
        dma.next_descriptor_low,
        dma.last_descriptor_low,
    ));
    for &(name, index) in REGISTERS {
        let raw = domain
            .phy_register_image(index)
            .ok_or(RejectReason::Internal)?;
        oer_esp32s31_board::rom::print_fmt(format_args!(
            "OPEN_RADIO_HIL phy=cca-register point={point:?} name={name} raw={raw:#010x}\n",
        ));
    }
    Ok(wifi)
}
