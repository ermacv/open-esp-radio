//! A software interrupt at the radio interrupts' level, timed from its raise
//! to its handler's entry ().
//!
//! The probe's line routes to core 0 at `Priority1`, the level of the Wi-Fi
//! and IEEE 802.15.4 MAC interrupts, so a sample waits for a radio handler
//! that runs at the raise: the latency an interrupt-context radio owner
//! would see for a command a task posts. Samples wait the request's interval
//! between raises, so they spread over the window the host measures and the
//! image's other work, its radio traffic included, runs between them.

use core::{
    arch::asm,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

use oer_hil_protocol::system::{InterruptLatencyEvidence, InterruptLatencyRequest};

/// The most samples one request takes.
const MAX_SAMPLES: usize = 1024;
/// A sample whose handler has not run after this many cycles is missed.
const MISSED_AFTER_CYCLES: u32 = 50_000_000;

static ENTERED: AtomicU32 = AtomicU32::new(0);
static FIRED: AtomicBool = AtomicBool::new(false);

#[inline(always)]
fn cycles() -> u32 {
    let value: u32;
    // SAFETY: `rdcycle` reads the CPU cycle counter; it touches no memory.
    #[allow(unsafe_code, reason = "the cycle counter is a CSR read")]
    unsafe {
        asm!("rdcycle {value}", value = out(reg) value, options(nomem, nostack))
    };
    value
}

/// Route the probe's line; call on core 0 before it enables interrupts.
pub(crate) fn enable(token: crate::InterruptLatencyProbe) {
    if let Err(error) = oer_espressif_interrupt_table_esp_hal::enable(&token) {
        panic!("interrupt-latency probe: {error:?}");
    }
}

/// The probe line's handler: stamp the entry, then clear the line.
pub(crate) fn handler() {
    let entered = cycles();
    crate::software_interrupt::interrupt_latency_probe().reset();
    ENTERED.store(entered, Ordering::Relaxed);
    FIRED.store(true, Ordering::Release);
}

/// Take `request.samples` samples, at most [`MAX_SAMPLES`].
pub(crate) async fn measure(request: InterruptLatencyRequest) -> InterruptLatencyEvidence {
    let line = crate::software_interrupt::interrupt_latency_probe();
    let wanted = usize::from(request.samples).min(MAX_SAMPLES);
    let mut latencies = [0u32; MAX_SAMPLES];
    let mut taken = 0;
    let mut missed = 0u16;
    for _ in 0..wanted {
        FIRED.store(false, Ordering::Release);
        let raised = cycles();
        line.raise();
        while !FIRED.load(Ordering::Acquire) {
            if cycles().wrapping_sub(raised) > MISSED_AFTER_CYCLES {
                break;
            }
        }
        if FIRED.load(Ordering::Acquire) {
            latencies[taken] = ENTERED.load(Ordering::Relaxed).wrapping_sub(raised);
            taken += 1;
        } else {
            missed = missed.saturating_add(1);
        }
        if request.interval_micros == 0 {
            embassy_futures::yield_now().await;
        } else {
            embassy_time::Timer::after_micros(u64::from(request.interval_micros)).await;
        }
    }
    summarize(&mut latencies[..taken], missed)
}

fn summarize(latencies: &mut [u32], missed: u16) -> InterruptLatencyEvidence {
    latencies.sort_unstable();
    let at = |per_mille: usize| {
        latencies
            .get((latencies.len() * per_mille / 1000).min(latencies.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0)
    };
    InterruptLatencyEvidence {
        cpu_hz: esp_hal::clock::cpu_clock().as_hz(),
        samples: u16::try_from(latencies.len()).unwrap_or(u16::MAX),
        missed,
        minimum_cycles: latencies.first().copied().unwrap_or(0),
        median_cycles: at(500),
        p99_cycles: at(990),
        maximum_cycles: latencies.last().copied().unwrap_or(0),
    }
}
