use super::*;
use epoch::{Moderated, Unmoderated};

#[derive(Default)]
struct Interrupt {
    observation: MacInterruptObservation,
    operations: std::vec::Vec<&'static str>,
}
impl MacInterrupt for Interrupt {
    type Snapshot = MacInterruptObservation;
    fn status(&mut self) -> Self::Snapshot {
        self.operations.push("read");
        self.observation
    }
    fn mask_rx_delivery(&mut self) {
        self.operations.push("mask");
    }
    fn acknowledge(&mut self, _: Self::Snapshot) {
        self.operations.push("ack");
    }
}

#[test]
fn moderated_ack_masks_before_clear_and_returns_all_facts() {
    let mut interrupt = Interrupt {
        observation: MacInterruptObservation::from_semantic_events(
            MacInterruptEvents::RX_SUCCESS.union(MacInterruptEvents::TX_COMPLETE),
            true,
            true,
        ),
        ..Default::default()
    };
    let (disposition, snapshot) = acknowledge_mac::<_, Moderated>(&mut interrupt);
    assert_eq!(interrupt.operations, ["read", "mask", "ack"]);
    assert_eq!(disposition, IrqDisposition::Posted);
    assert_eq!(snapshot.posted_events, EVENT_RX_SUCCESS | EVENT_TX_COMPLETE);
    assert!(snapshot.had_auxiliary_event && snapshot.had_unhandled_event);
}

#[test]
fn unmoderated_ack_has_no_mask_and_auxiliary_only_is_not_work() {
    let mut interrupt = Interrupt {
        observation: MacInterruptObservation::from_semantic_events(
            MacInterruptEvents::empty(),
            true,
            false,
        ),
        ..Default::default()
    };
    let (disposition, snapshot) = acknowledge_mac::<_, Unmoderated>(&mut interrupt);
    assert_eq!(interrupt.operations, ["read", "ack"]);
    assert_eq!(disposition, IrqDisposition::AcknowledgedOnly);
    assert_eq!(snapshot.posted_events, 0);
}

#[test]
fn spurious_mac_does_not_mask_acknowledge_or_publish() {
    let mut interrupt = Interrupt::default();
    let (disposition, snapshot) = acknowledge_mac::<_, Moderated>(&mut interrupt);
    assert_eq!(interrupt.operations, ["read"]);
    assert_eq!(disposition, IrqDisposition::Spurious);
    assert!(!snapshot.had_status);
}

struct Power {
    observation: MacPowerInterruptObservation,
    acks: usize,
}
impl MacPowerInterrupt for Power {
    type Snapshot = MacPowerInterruptObservation;
    fn status(&mut self) -> Self::Snapshot {
        self.observation
    }
    fn acknowledge(&mut self, _: Self::Snapshot) {
        self.acks += 1;
    }
}

#[test]
fn power_ack_returns_every_cause_and_spurious_does_not_ack() {
    let mut interrupt = Power {
        observation: MacPowerInterruptObservation::from_semantic_events(
            true, false, false, false, true, true,
        ),
        acks: 0,
    };
    let (disposition, snapshot) = acknowledge_power(&mut interrupt);
    assert_eq!(disposition, PowerIrqDisposition::Posted);
    assert_eq!(interrupt.acks, 1);
    assert!(
        snapshot.observation.tsf_timer_0()
            && snapshot.observation.sta_tbtt()
            && snapshot.observation.has_unhandled_event()
    );
    interrupt.observation = MacPowerInterruptObservation::default();
    assert_eq!(
        acknowledge_power(&mut interrupt).0,
        PowerIrqDisposition::Spurious
    );
    assert_eq!(interrupt.acks, 1);
}
