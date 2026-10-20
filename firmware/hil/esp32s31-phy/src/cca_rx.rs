//! Bounded, polled RX experiment after the host has read the calibration image.
use crate::cca_checkpoint::{Point, observe_enables, observe_running};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_hal::{
    domain::RadioDomain,
    ieee80211::{
        client::WifiClocked,
        dma::{
            rx_ring::{RxRingError, RxRingLive},
            rx_storage::RxDmaStorage,
        },
    },
    owner::RadioAccess,
    power::PlatformClockBackend,
    settle::Settle,
};
use oer_esp32s31_ieee80211_driver::rx::{
    receive::{DRAIN_POLL, DRAIN_TIMEOUT, RELOAD_SETTLE_LIMIT},
    storage::walker_enable_settle,
    upper::W1_INGRESS,
};
use oer_esp32s31_ieee80211_mac::{
    init::{MacRuntimeStopHardware, activate_promiscuous_receive, deactivate_promiscuous_receive},
    rx::{RxSegment, view_normalized_rx_frame},
};
use oer_hil_protocol::base::RejectReason;
use oer_ieee80211_wire::{frame::DecodedHeader, scan::parse_management};
use oer_time::{Clock, Duration, Timer};

const COUNT: usize = 4;
const CAPACITY: usize = 1_664;
const STORAGE_BYTES: usize = CAPACITY + core::mem::size_of::<u32>();
type Storage = RxDmaStorage<COUNT, CAPACITY, STORAGE_BYTES>;

oer_esp32s31_hal::dma_static! {
    static STORAGE: Storage = zeroed in ".dma.bss.cca_rx";
}

type Result<T> = core::result::Result<T, RejectReason>;

fn failure(step: &str, error: RxRingError) -> RejectReason {
    oer_esp32s31_board::rom::print_fmt(format_args!(
        "OPEN_RADIO_HIL phy=cca-rx FAIL step={step} error={error:?}\n"
    ));
    RejectReason::Internal
}

/// Always return the Wi-Fi owner so the caller can close RF even on failure.
pub async fn run<B: PlatformClockBackend, T>(
    domain: &RadioDomain<B, T>,
    wifi: WifiClocked,
    channel: u8,
) -> (WifiClocked, Result<()>) {
    let (mut runtime, interrupts, clocks) = wifi.into_running();
    oer_esp32s31_board::rom::print_fmt(format_args!(
        "OPEN_RADIO_HIL phy=cca-rx begin channel={channel} descriptors={COUNT} capacity={CAPACITY}\n"
    ));
    let result = experiment(domain, &mut runtime, channel).await;
    (
        WifiClocked::from_running(runtime, interrupts, clocks),
        result,
    )
}

async fn experiment<B: PlatformClockBackend, T>(
    domain: &RadioDomain<B, T>,
    runtime: &mut RadioAccess,
    channel: u8,
) -> Result<()> {
    observe_running(domain, runtime, Point::BeforeRing)?;
    observe_enables(domain, runtime, Point::BeforeRing)?;
    let placed = Storage::place(STORAGE.take().ok_or(RejectReason::Internal)?)
        .map_err(|_| RejectReason::Internal)?;
    let storage = placed.storage();
    let stopped = placed
        .prepare_ring(runtime.rx_dma())
        .map_err(|error| failure("prepare", error))?;
    // Same hardware edges as W1's receive() and W2's Rx::start(). Opening
    // only the DMA gate leaves the post-cold-init MAC receive path stopped.
    activate_promiscuous_receive(runtime);
    walker_enable_settle(&EmbassyClock).await;
    let mut ring = match stopped.try_start(runtime.rx_dma()) {
        Ok(ring) => ring,
        Err((_, error)) => {
            deactivate_promiscuous_receive(runtime);
            return Err(failure("start", error));
        }
    };
    runtime.resume_mac_runtime();

    // No CPU interrupt route, station role, upper buffer leases or TX. Keep
    // the finite ring supplied while the hardware receives the router's air.
    let observed = sample(domain, runtime, storage, &mut ring, channel).await;

    // As in the working receive teardown, close admission and prove the MAC
    // idle while the walker still retains every DMA credit. A timeout keeps
    // the static arena quarantined until the caller's RF teardown/reset.
    deactivate_promiscuous_receive(runtime);
    runtime.request_mac_runtime_stop();
    let deadline = EmbassyClock.now().saturating_add(DRAIN_TIMEOUT);
    while runtime.mac_runtime_active_state() != 0 {
        if EmbassyClock.now() >= deadline {
            ring.require_reset();
            oer_esp32s31_board::rom::line(c"OPEN_RADIO_HIL phy=cca-rx FAIL step=mac-idle");
            return Err(RejectReason::Internal);
        }
        EmbassyClock
            .wait_until(EmbassyClock.now().saturating_add(DRAIN_POLL))
            .await;
    }
    let halted = ring
        .try_stop(runtime.rx_dma())
        .map_err(|(_, error)| failure("stop", error))?;
    let closed = observe_running(domain, runtime, Point::RingClosed)
        .and_then(|()| observe_enables(domain, runtime, Point::RingClosed));
    let stopped = storage
        .prepare_halted(halted, runtime.rx_dma())
        .map_err(|(_, error)| failure("reclaim", error))?;
    let returned = !runtime.receive_dma_snapshot().walker_enabled
        && storage.detached_buffer_count() == 0
        && stopped.topology_snapshot().valid;
    oer_esp32s31_board::rom::print_fmt(format_args!(
        "OPEN_RADIO_HIL phy=cca-rx cleanup mac_idle=true gate_closed={} buffers_returned={returned} buffers={}\n",
        !runtime.receive_dma_snapshot().walker_enabled,
        COUNT + 1,
    ));
    if !returned {
        return Err(RejectReason::Internal);
    }
    closed?;
    observed
}

async fn sample<B: PlatformClockBackend, T>(
    domain: &RadioDomain<B, T>,
    runtime: &mut RadioAccess,
    storage: &Storage,
    ring: &mut RxRingLive<'_, COUNT>,
    channel: u8,
) -> Result<()> {
    observe_running(domain, runtime, Point::RingOpen)?;
    observe_enables(domain, runtime, Point::RingOpen)?;
    let mut received = 0_u32;
    let mut beacons = 0_u32;
    let mut normalized = 0_u32;
    for sample in 0..5 {
        for _ in 0..100 {
            // Bound each drain as well as the complete observation window.
            for _ in 0..COUNT {
                let engine = runtime.rx_dma();
                let cursor = ring
                    .freeze_cursor(engine)
                    .map_err(|error| failure("cursor", error))?;
                let Some(unit) = storage
                    .take_completed_unit(ring, cursor)
                    .map_err(|error| failure("take", error))?
                else {
                    break;
                };
                received = received.saturating_add(1);
                // Borrow only a complete single-descriptor frame, exactly
                // the ingress shape W1 normalizes. Chains and the tail of a
                // reserved-descriptor landing are discarded and returned.
                if unit.descriptor_count() == 1
                    && !unit.metadata().continues_reserved_landing()
                    && let (Some(word0), Some(buffer)) =
                        (unit.metadata().staged_word0(), unit.segment(0))
                {
                    let segment = RxSegment {
                        descriptor_address: unit.metadata().descriptor_address(),
                        descriptor_word0: word0,
                        buffer,
                        next_descriptor_address: 0,
                    };
                    if let Ok(frame) = view_normalized_rx_frame(&segment, W1_INGRESS) {
                        normalized = normalized.saturating_add(1);
                        if DecodedHeader::decode(frame.mpdu)
                            .is_some_and(|h| !h.control().is_data() && h.control().subtype() == 8)
                            && parse_management(frame.mpdu, channel, frame.rssi_dbm).is_some_and(
                                |bss| {
                                    bss.channel == channel
                                        && bss.beacon_interval_tu != 0
                                        && !bss.information_elements_truncated
                                },
                            )
                        {
                            beacons = beacons.saturating_add(1);
                        }
                    }
                }
                let credit = unit.retain_discard().ok_or(RejectReason::Internal)?;
                credit
                    .append(engine)
                    .map_err(|error| failure("return", error))?;
                ring.complete_pending_reload(
                    engine,
                    Settle::new(RELOAD_SETTLE_LIMIT, &EmbassyClock),
                )
                .map_err(|error| failure("reload", error))?;
            }
            EmbassyClock
                .wait_until(
                    EmbassyClock
                        .now()
                        .saturating_add(Duration::from_micros(2_000)),
                )
                .await;
        }
        observe_running(domain, runtime, Point::RingSample(sample))?;
        observe_enables(domain, runtime, Point::RingSample(sample))?;
        if !runtime.receive_dma_snapshot().walker_enabled {
            return Err(RejectReason::Internal);
        }
        oer_esp32s31_board::rom::print_fmt(format_args!(
            "OPEN_RADIO_HIL phy=cca-rx sample={sample} received={received} normalized={normalized} beacons={beacons}\n"
        ));
    }
    if beacons == 0 {
        oer_esp32s31_board::rom::line(c"OPEN_RADIO_HIL phy=cca-rx FAIL step=no-beacons");
        return Err(RejectReason::Internal);
    }
    Ok(())
}
