//! Public counters, independent of acceptance verdicts.
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
pub static DEPARTING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
use oer_hil_protocol::wifi::{StationCounters, StationGroupKey};

pub static TIMEOUTS: AtomicU32 = AtomicU32::new(0);
pub static COLLISIONS: AtomicU32 = AtomicU32::new(0);
pub static RX_REFUSED: AtomicU32 = AtomicU32::new(0);
pub static DEAUTH: AtomicU32 = AtomicU32::new(0);
pub static KEY_ID: AtomicU32 = AtomicU32::new(0);
pub static GENERATION: AtomicU32 = AtomicU32::new(0);
pub static GROUP_ARP: AtomicU32 = AtomicU32::new(0);

pub fn group(attempt: super::Attempt, key_id: u8, replaced: bool) {
    KEY_ID.store(u32::from(key_id), Relaxed);
    GROUP_ARP.store(0, Relaxed);
    let generation = GENERATION.fetch_add(1, Relaxed).saturating_add(1);
    super::CONSOLE.publish(
        attempt.request.session_id,
        attempt.request.request_id,
        &StationGroupKey {
            epoch: super::EPOCH.load(Relaxed),
            generation,
            key_id,
            replaced,
        },
    );
}
pub fn counters() -> StationCounters {
    StationCounters {
        epoch: super::EPOCH.load(Relaxed),
        tx_timeouts: TIMEOUTS.load(Relaxed),
        queue_collisions: COLLISIONS.load(Relaxed),
        rx_refused: RX_REFUSED.load(Relaxed),
        deauth_transmissions: DEAUTH.load(Relaxed),
        group_generation: GENERATION.load(Relaxed),
        group_key_id: KEY_ID.load(Relaxed) as u8,
        group_arp_received: GROUP_ARP.load(Relaxed),
        irq_free: u32::try_from(
            oer_espressif_staged_runtime::stacks::current_hart_interrupt_stack_free_bytes(),
        )
        .expect("S31 interrupt stack fits u32"),
    }
}
