//! Prepared capture ownership and the later bounded, polled RX experiment.
use crate::cca_checkpoint::{Point, observe_enables, observe_running};
use oer_diagnostics::{DiagPath, Note};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_hal::{
    domain::RadioDomain,
    ieee80211::{
        client::WifiClocked,
        dma::{
            rx_ring::{RxRingError, RxRingLive, RxRingStopped},
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
    init::{
        MacRuntimeStartHardware, MacRuntimeStopHardware, activate_promiscuous_receive,
        deactivate_promiscuous_receive,
    },
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

/// Retained across capture pages and RF restart; DMA has not been started.
pub struct Prepared {
    storage: &'static Storage,
    stopped: RxRingStopped<'static, COUNT>,
}

fn failure(step: &'static str, error: RxRingError) -> RejectReason {
    crate::transport::CONSOLE.note_immediately(&Note::cca_rx_fail(step, &DiagPath::of(&error)));
    RejectReason::Internal
}

/// Complete the common MAC core edge, as the reference's esp_wifi_start does.
/// The walker and MAC runtime stay stopped.
pub async fn prepare<B: PlatformClockBackend, T>(
    domain: &RadioDomain<B, T>,
    wifi: WifiClocked,
    previous: Option<Prepared>,
) -> (WifiClocked, Result<Prepared>) {
    let (mut runtime, interrupts, clocks) = wifi.into_running();
    let result = prepare_inner(domain, &mut runtime, previous).await;
    (
        WifiClocked::from_running(runtime, interrupts, clocks),
        result,
    )
}

async fn prepare_inner<B: PlatformClockBackend, T>(
    domain: &RadioDomain<B, T>,
    runtime: &mut RadioAccess,
    previous: Option<Prepared>,
) -> Result<Prepared> {
    let prepared = if let Some(Prepared { storage, stopped }) = previous {
        // Cold MAC initialization reset the common gates, but the retained
        // stopped owner still proves prepared descriptor memory and binding.
        runtime.wifi_mac_hal().reopen_mac_core(&stopped);
        let stopped = storage
            .prepare_halted(stopped.into_halted(), runtime.rx_dma())
            .map_err(|(_, error)| failure("reprepare", error))?;
        Prepared { storage, stopped }
    } else {
        let placed = Storage::place(STORAGE.take().ok_or(RejectReason::Internal)?)
            .map_err(|_| RejectReason::Internal)?;
        let storage = placed.storage();
        let memory = placed
            .prepare_ring(runtime.rx_dma())
            .map_err(|error| failure("prepare", error))?;
        let stopped = runtime
            .prepare_mac_runtime(memory)
            .publish(runtime.rx_dma())
            .map_err(|error| failure("publish", error))?;
        Prepared { storage, stopped }
    };
    observe_running(domain, runtime, Point::CapturePrepared).await?;
    observe_enables(domain, runtime, Point::CapturePrepared).await?;
    Ok(prepared)
}

/// Always return the Wi-Fi owner so the caller can close RF even on failure.
pub async fn run<B: PlatformClockBackend, T>(
    domain: &RadioDomain<B, T>,
    wifi: WifiClocked,
    channel: u8,
    prepared: Option<Prepared>,
    #[cfg(feature = "diagnostic-radio-health")] health: Option<&mut crate::health::Store>,
) -> (WifiClocked, Result<()>) {
    let (mut runtime, interrupts, clocks) = wifi.into_running();
    crate::transport::CONSOLE
        .note_reliably(&Note::cca_rx_begin(
            channel.into(),
            COUNT as u32,
            CAPACITY as u32,
        ))
        .await;
    let result = experiment(
        domain,
        &mut runtime,
        channel,
        prepared,
        #[cfg(feature = "diagnostic-radio-health")]
        health,
    )
    .await;
    (
        WifiClocked::from_running(runtime, interrupts, clocks),
        result,
    )
}

async fn experiment<B: PlatformClockBackend, T>(
    domain: &RadioDomain<B, T>,
    runtime: &mut RadioAccess,
    channel: u8,
    prepared: Option<Prepared>,
    #[cfg(feature = "diagnostic-radio-health")] mut health: Option<&mut crate::health::Store>,
) -> Result<()> {
    observe_running(domain, runtime, Point::BeforeRing).await?;
    observe_enables(domain, runtime, Point::BeforeRing).await?;
    // Cold capture retains its original observation point and only prepares
    // here. A preparation failure still returns the Wi-Fi owner through run
    // so StopCapture can execute its RF teardown.
    let Prepared { storage, stopped } = match prepared {
        Some(prepared) => prepared,
        None => prepare_inner(domain, runtime, None).await?,
    };
    // The capture prepared common gates and BASE with runtime stop held.
    // Policy preserves that stop until the walker supplies the live proof.
    activate_promiscuous_receive(runtime);
    walker_enable_settle(&EmbassyClock).await;
    let mut ring = match stopped.try_start(runtime.rx_dma()) {
        Ok(ring) => ring,
        Err((_, error)) => {
            deactivate_promiscuous_receive(runtime);
            return Err(failure("start", error));
        }
    };
    runtime.start_mac_runtime(&ring);

    // No CPU interrupt route, station role, upper buffer leases or TX. Keep
    // the finite ring supplied while the hardware receives the router's air.
    #[cfg(feature = "diagnostic-radio-health")]
    if let Some(store) = health.as_deref_mut() {
        crate::health::cut(store, runtime, oer_hil_protocol::health::Cut::Before);
    }
    let observed = sample(
        domain,
        runtime,
        storage,
        &mut ring,
        channel,
        #[cfg(feature = "diagnostic-radio-health")]
        health.as_deref_mut(),
    )
    .await;
    #[cfg(feature = "diagnostic-radio-health")]
    if let Some(store) = health.as_deref_mut() {
        crate::health::cut(store, runtime, oer_hil_protocol::health::Cut::After);
    }

    // As in the working receive teardown, close admission and prove the MAC
    // idle while the walker still retains every DMA credit. A timeout keeps
    // the static arena quarantined until the caller's RF teardown/reset.
    deactivate_promiscuous_receive(runtime);
    runtime.request_mac_runtime_stop();
    let deadline = EmbassyClock.now().saturating_add(DRAIN_TIMEOUT);
    while runtime.mac_runtime_active_state() != 0 {
        if EmbassyClock.now() >= deadline {
            ring.require_reset();
            crate::transport::CONSOLE
                .note_immediately(&Note::cca_rx_fail("mac-idle", &DiagPath::new()));
            return Err(RejectReason::Internal);
        }
        EmbassyClock
            .wait_until(EmbassyClock.now().saturating_add(DRAIN_POLL))
            .await;
    }
    let halted = ring
        .try_stop(runtime.rx_dma())
        .map_err(|(_, error)| failure("stop", error))?;
    let closed = match observe_running(domain, runtime, Point::RingClosed).await {
        Ok(()) => observe_enables(domain, runtime, Point::RingClosed).await,
        Err(error) => Err(error),
    };
    let stopped = storage
        .prepare_halted(halted, runtime.rx_dma())
        .map_err(|(_, error)| failure("reclaim", error))?;
    let returned = !runtime.receive_dma_snapshot().walker_enabled
        && storage.detached_buffer_count() == 0
        && stopped.topology_snapshot().valid;
    crate::transport::CONSOLE
        .note_reliably(&Note::cca_rx_cleanup(
            !runtime.receive_dma_snapshot().walker_enabled,
            returned,
            COUNT as u32 + 1,
        ))
        .await;
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
    #[cfg(feature = "diagnostic-radio-health")] mut health: Option<&mut crate::health::Store>,
) -> Result<()> {
    observe_running(domain, runtime, Point::RingOpen).await?;
    observe_enables(domain, runtime, Point::RingOpen).await?;
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
                        #[cfg(feature = "diagnostic-radio-health")]
                        if let Some(store) = health.as_deref_mut() {
                            crate::health::received(store, &frame, channel);
                        }
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
        #[cfg(feature = "diagnostic-radio-health")]
        if let Some(store) = health.as_deref_mut() {
            crate::health::signal(store, runtime);
        }
        observe_running(domain, runtime, Point::RingSample(sample)).await?;
        observe_enables(domain, runtime, Point::RingSample(sample)).await?;
        if !runtime.receive_dma_snapshot().walker_enabled {
            return Err(RejectReason::Internal);
        }
        crate::transport::CONSOLE
            .note_reliably(&Note::cca_rx_sample(
                sample.into(),
                received as u32,
                normalized as u32,
                beacons as u32,
            ))
            .await;
    }
    if beacons == 0 {
        crate::transport::CONSOLE
            .note_immediately(&Note::cca_rx_fail("no-beacons", &DiagPath::new()));
        return Err(RejectReason::Internal);
    }
    Ok(())
}
