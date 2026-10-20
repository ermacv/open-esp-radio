//! Bounded, polled RX experiment after the host has read the calibration image.
use crate::cca_checkpoint::{Point, observe_running};
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
    settle::{Settle, SettleLimit},
};
use oer_hil_protocol::base::RejectReason;
use oer_time::{Clock, Duration, Timer};

const COUNT: usize = 4;
const CAPACITY: usize = 1_664;
const STORAGE_BYTES: usize = CAPACITY + core::mem::size_of::<u32>();
type Storage = RxDmaStorage<COUNT, CAPACITY, STORAGE_BYTES>;
const RELOAD_SETTLE: SettleLimit =
    SettleLimit::new(Duration::from_micros(20)).expect("bounded RX reload settle");

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
    let result = experiment(domain, &mut runtime).await;
    (
        WifiClocked::from_running(runtime, interrupts, clocks),
        result,
    )
}

async fn experiment<B: PlatformClockBackend, T>(
    domain: &RadioDomain<B, T>,
    runtime: &mut RadioAccess,
) -> Result<()> {
    observe_running(domain, runtime, Point::BeforeRing)?;
    let placed = Storage::place(STORAGE.take().ok_or(RejectReason::Internal)?)
        .map_err(|_| RejectReason::Internal)?;
    let storage = placed.storage();
    let stopped = placed
        .prepare_ring(runtime.rx_dma())
        .map_err(|error| failure("prepare", error))?;
    let mut ring = stopped
        .try_start(runtime.rx_dma())
        .map_err(|(_, error)| failure("start", error))?;

    // No CPU interrupt route, station role, upper buffer leases or TX. Keep
    // the finite ring supplied while the hardware receives the router's air.
    let observed = sample(domain, runtime, storage, &mut ring).await;

    // Even a failed observation must retire DMA before RF/clock teardown.
    let halted = ring
        .try_stop(runtime.rx_dma())
        .map_err(|(_, error)| failure("stop", error))?;
    let closed = observe_running(domain, runtime, Point::RingClosed);
    let stopped = storage
        .prepare_halted(halted, runtime.rx_dma())
        .map_err(|(_, error)| failure("reclaim", error))?;
    let returned = !runtime.receive_dma_snapshot().walker_enabled
        && storage.detached_buffer_count() == 0
        && stopped.topology_snapshot().valid;
    oer_esp32s31_board::rom::print_fmt(format_args!(
        "OPEN_RADIO_HIL phy=cca-rx cleanup gate_closed={} buffers_returned={returned} buffers={}\n",
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
) -> Result<()> {
    observe_running(domain, runtime, Point::RingOpen)?;
    let mut received = 0_u32;
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
                let credit = unit.retain_discard().ok_or(RejectReason::Internal)?;
                credit
                    .append(engine)
                    .map_err(|error| failure("return", error))?;
                ring.complete_pending_reload(engine, Settle::new(RELOAD_SETTLE, &EmbassyClock))
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
        if !runtime.receive_dma_snapshot().walker_enabled {
            return Err(RejectReason::Internal);
        }
        oer_esp32s31_board::rom::print_fmt(format_args!(
            "OPEN_RADIO_HIL phy=cca-rx sample={sample} received={received}\n"
        ));
    }
    Ok(())
}
