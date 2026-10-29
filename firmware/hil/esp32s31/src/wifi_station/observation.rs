//! Public counters, independent of acceptance verdicts.
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
pub static DEPARTING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
use oer_ieee80211_control::{StationCounters, StationGroupKey};

pub static TIMEOUTS: AtomicU32 = AtomicU32::new(0);
pub static COLLISIONS: AtomicU32 = AtomicU32::new(0);
pub static RX_REFUSED: AtomicU32 = AtomicU32::new(0);
pub static DEAUTH: AtomicU32 = AtomicU32::new(0);
pub static KEY_ID: AtomicU32 = AtomicU32::new(0);
pub static GENERATION: AtomicU32 = AtomicU32::new(0);
pub static GROUP_ARP: AtomicU32 = AtomicU32::new(0);

fn group(attempt: super::Attempt, key_id: u8, replaced: bool) {
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

/// The station's, data machine's and service's observer in this image: it
/// turns the service's observations into the HIL station messages of the
/// attempt. Only the station service task observes.
#[derive(Clone, Copy)]
pub struct StationObserver(pub super::Attempt);

/// The addresses of the ARP MSDU whose MPDU identity the next ARP event
/// carries: the service reports both back to back.
static ARP_SENDER: AtomicU32 = AtomicU32::new(0);
static ARP_TARGET: AtomicU32 = AtomicU32::new(0);
static ARP_PENDING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

impl oer_trace::Observer for StationObserver {
    fn event<E: oer_trace::Event + Copy + 'static>(&self, event: E) {
        use oer_esp32s31_wifi::observation::Observation as FacadeObservation;
        match FacadeObservation::of(&event) {
            Some(FacadeObservation::Associated) => self
                .0
                .publish(oer_ieee80211_control::StationLinkState::Associated),
            Some(FacadeObservation::HardwareTimeout) => {
                TIMEOUTS.fetch_add(1, Relaxed);
            }
            Some(FacadeObservation::ControlDelivered) if DEPARTING.load(Relaxed) => {
                DEAUTH.fetch_add(1, Relaxed);
            }
            _ => {}
        }
        use oer_ieee80211_instrumentation::{ArpOperation, Observation};
        let operation = |operation| match operation {
            ArpOperation::Request => 1,
            ArpOperation::Reply => 2,
        };
        match Observation::of(&event) {
            Some(Observation::RxFrameDropped(dropped)) => receive_drop(self.0, dropped.reason),
            Some(Observation::GroupKeyInstalled(installed)) => {
                group(self.0, installed.key_id, installed.replaced)
            }
            Some(Observation::ArpAddresses(addresses)) => {
                ARP_SENDER.store(u32::from_be_bytes(addresses.sender), Relaxed);
                ARP_TARGET.store(u32::from_be_bytes(addresses.target), Relaxed);
                ARP_PENDING.store(true, Relaxed);
            }
            Some(Observation::ArpTransmitted(arp)) => self.arp(ArpFrame {
                transmitted: true,
                group_addressed: false,
                operation: operation(arp.operation),
                sequence: arp.sequence,
                packet_number: arp.packet_number,
                key_id: 0,
            }),
            Some(Observation::ArpReceived(arp)) => self.arp(ArpFrame {
                transmitted: false,
                group_addressed: arp.group_addressed,
                operation: operation(arp.operation),
                sequence: arp.sequence,
                packet_number: arp.packet_number,
                key_id: arp.key_id,
            }),
            _ => {}
        }
    }
}

/// The public identity of an ARP MSDU's MPDU.
struct ArpFrame {
    transmitted: bool,
    group_addressed: bool,
    operation: u16,
    sequence: u16,
    packet_number: u64,
    key_id: u8,
}

impl StationObserver {
    #[inline(never)]
    fn arp(self, frame: ArpFrame) {
        if !ARP_PENDING.swap(false, Relaxed) {
            return;
        }
        super::emit(
            self.0.request,
            &oer_ieee80211_control::StationArpFrame {
                epoch: super::EPOCH.load(Relaxed),
                generation: GENERATION.load(Relaxed),
                transmitted: frame.transmitted,
                group_addressed: frame.group_addressed,
                operation: frame.operation,
                sender: ARP_SENDER.load(Relaxed).to_be_bytes(),
                target: ARP_TARGET.load(Relaxed).to_be_bytes(),
                sequence: frame.sequence,
                packet_number: frame.packet_number,
                key_id: frame.key_id,
            },
        );
    }
}

fn receive_drop(attempt: super::Attempt, reason: oer_ieee80211_instrumentation::RxDrop) {
    use oer_ieee80211_control::{StationReceiveDrop, StationReceiveDropReason as R};
    use oer_ieee80211_instrumentation::{FrameDrop, RxDrop, SecurityDrop};
    let reason = match reason {
        RxDrop::Frame(FrameDrop::Integrity) => R::Integrity,
        RxDrop::Frame(FrameDrop::Replay) => R::Replay,
        RxDrop::Frame(FrameDrop::Protection) => R::Protection,
        RxDrop::Security(SecurityDrop::InvalidMic) => R::InvalidMic,
        RxDrop::Security(SecurityDrop::StaleReplay) => R::StaleReplay,
        RxDrop::Security(SecurityDrop::Other) => R::Security,
        _ => return,
    };
    super::emit(
        attempt.request,
        &StationReceiveDrop {
            epoch: super::EPOCH.load(Relaxed),
            reason,
        },
    );
}

#[derive(Default)]
pub struct DriverObserver;
impl oer_trace::Observer for DriverObserver {
    fn event<E: oer_trace::Event + Copy + 'static>(&self, event: E) {
        if oer_esp32s31_wifi::observation::Observation::of(&event)
            == Some(oer_esp32s31_wifi::observation::Observation::QueueCollision)
        {
            COLLISIONS.fetch_add(1, Relaxed);
        }
    }
}
