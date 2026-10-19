use super::*;
use epoch::{ActiveEpoch, Moderated, Unmoderated};
use model::{
    MacInterruptModel, MacInterruptOperation, PowerInterruptModel, PowerInterruptOperation,
};

#[derive(Default)]
struct Interrupt {
    observation: MacInterruptObservation,
    operations: std::vec::Vec<&'static str>,
}
impl sealed::Sealed for Interrupt {}
impl MacInterrupt for Interrupt {
    type Snapshot = MacInterruptObservation;
    fn status(&mut self) -> Self::Snapshot {
        self.operations.push("read");
        self.observation
    }
    fn mask_rx_delivery(&mut self) {
        self.operations.push("mask");
    }
    fn unmask_rx_delivery(&mut self) {
        self.operations.push("unmask");
    }
    fn mask_all(&mut self) {
        self.operations.push("mask all");
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
    let (disposition, snapshot) = handle_mac_irq::<Moderated, _>(&mut interrupt);
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
    let (disposition, snapshot) = handle_mac_irq::<Unmoderated, _>(&mut interrupt);
    assert_eq!(interrupt.operations, ["read", "ack"]);
    assert_eq!(disposition, IrqDisposition::AcknowledgedOnly);
    assert_eq!(snapshot.posted_events, 0);
}

#[test]
fn spurious_mac_does_not_mask_acknowledge_or_publish() {
    let mut interrupt = Interrupt::default();
    let (disposition, snapshot) = handle_mac_irq::<Moderated, _>(&mut interrupt);
    assert_eq!(interrupt.operations, ["read"]);
    assert_eq!(disposition, IrqDisposition::Spurious);
    assert!(!snapshot.had_status);
}

#[test]
fn bss_color_only_interrupt_posts_an_independent_event() {
    let mut interrupt = Interrupt {
        observation: MacInterruptObservation::from_semantic_events(
            MacInterruptEvents::BSS_COLOR_COLLISION,
            false,
            false,
        ),
        ..Default::default()
    };
    let (disposition, snapshot) = handle_mac_irq::<Moderated, _>(&mut interrupt);
    assert_eq!(interrupt.operations, ["read", "ack"]);
    assert_eq!(disposition, IrqDisposition::Posted);
    assert_eq!(snapshot.posted_events, EVENT_BSS_COLOR_COLLISION);
    assert_eq!(
        next_irq_work(snapshot.posted_events),
        Some(IrqWork::BssColorCollision)
    );
}

#[test]
fn station_mask_keeps_bss_color_latched_without_asserting_the_line() {
    let color = MacInterruptObservation::from_semantic_events(
        MacInterruptEvents::BSS_COLOR_COLLISION,
        false,
        false,
    );
    let mut interrupt = MacInterruptModel::new();
    interrupt.latch(color);
    assert!(!interrupt.line_asserted());
    let (disposition, _) = handle_mac_irq::<Moderated, _>(&mut interrupt);
    assert_eq!(disposition, IrqDisposition::Spurious);
    assert_eq!(interrupt.latched(), color);

    interrupt.latch(MacInterruptObservation::from_semantic_events(
        MacInterruptEvents::RX_SUCCESS,
        false,
        false,
    ));
    let (_, snapshot) = handle_mac_irq::<Moderated, _>(&mut interrupt);
    assert_eq!(snapshot.posted_events, EVENT_RX_SUCCESS);
    interrupt.unmask_rx_delivery();
    assert!(!interrupt.line_asserted());
    assert_eq!(interrupt.latched(), color);
}

struct Power {
    observation: MacPowerInterruptObservation,
    acks: usize,
}
impl sealed::Sealed for Power {}
impl MacPowerInterrupt for Power {
    type Snapshot = MacPowerInterruptObservation;
    fn status(&mut self) -> Self::Snapshot {
        self.observation
    }
    fn mask_all(&mut self) {}
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
    let (disposition, snapshot) = handle_power_irq(&mut interrupt);
    assert_eq!(disposition, PowerIrqDisposition::Posted);
    assert_eq!(interrupt.acks, 1);
    assert!(
        snapshot.observation.tsf_timer_0()
            && snapshot.observation.sta_tbtt()
            && snapshot.observation.has_unhandled_event()
    );
    interrupt.observation = MacPowerInterruptObservation::default();
    assert_eq!(
        handle_power_irq(&mut interrupt).0,
        PowerIrqDisposition::Spurious
    );
    assert_eq!(interrupt.acks, 1);
}

fn rx_and_tx() -> MacInterruptObservation {
    MacInterruptObservation::from_semantic_events(
        MacInterruptEvents::RX_SUCCESS.union(MacInterruptEvents::TX_COMPLETE),
        true,
        false,
    )
}

fn sta_tbtt() -> MacPowerInterruptObservation {
    MacPowerInterruptObservation::from_semantic_events(false, false, false, false, true, false)
}

fn host_epoch() -> ActiveEpoch<Moderated, MacInterruptModel, PowerInterruptModel> {
    ActiveEpoch::for_host_test(MacInterruptModel::new(), PowerInterruptModel::new())
}

#[test]
fn mask_all_masks_both_banks_without_acknowledging() {
    let mut epoch = host_epoch();
    let (mac, power) = epoch.banks_mut();
    mac.latch(rx_and_tx());
    power.latch(sta_tbtt());
    assert!(mac.line_asserted() && power.line_asserted());

    epoch.mask_all();

    let (mac, power) = epoch.banks();
    assert!(!mac.line_asserted() && !power.line_asserted());
    assert!(!mac.rx_delivery_enabled() && !mac.other_sources_enabled());
    assert!(!power.sta_tbtt_enabled());
    // Masking acknowledges nothing: every event stays latched.
    assert_eq!(mac.latched(), rx_and_tx());
    assert_eq!(power.latched(), sta_tbtt());
    assert!(mac.operations().matches(&[MacInterruptOperation::MaskAll]));
    assert!(
        power
            .operations()
            .matches(&[PowerInterruptOperation::MaskAll])
    );

    // A late entry of either handler sees nothing and acknowledges nothing.
    assert_eq!(epoch.service_mac().0, IrqDisposition::Spurious);
    assert_eq!(epoch.service_power().0, PowerIrqDisposition::Spurious);
    let (mac, power) = epoch.banks();
    assert_eq!(mac.latched(), rx_and_tx());
    assert_eq!(power.latched(), sta_tbtt());
}

#[test]
fn unmask_after_mask_all_restores_only_the_rx_group() {
    let mut epoch = host_epoch();
    epoch.banks_mut().0.latch(rx_and_tx());
    epoch.mask_all();

    epoch.unmask_rx_after_drain();

    let (mac, power) = epoch.banks();
    assert!(mac.rx_delivery_enabled() && !mac.other_sources_enabled());
    assert!(!power.sta_tbtt_enabled());
    // The latched RX level reasserts the line; the latched TX stays masked.
    assert!(mac.line_asserted());
    let rx_only =
        MacInterruptObservation::from_semantic_events(MacInterruptEvents::RX_SUCCESS, true, false);
    assert_eq!(mac.visible(), rx_only);

    let (disposition, snapshot) = epoch.service_mac();
    assert_eq!(disposition, IrqDisposition::Posted);
    assert_eq!(snapshot.posted_events, EVENT_RX_SUCCESS);
    let (mac, _) = epoch.banks();
    assert!(mac.operations().matches(&[
        MacInterruptOperation::MaskAll,
        MacInterruptOperation::UnmaskRxDelivery,
        MacInterruptOperation::Status,
        MacInterruptOperation::MaskRxDelivery,
        MacInterruptOperation::Acknowledge(rx_only),
    ]));
    assert!(!mac.line_asserted());
    assert_eq!(
        mac.latched(),
        MacInterruptObservation::from_semantic_events(
            MacInterruptEvents::TX_COMPLETE,
            false,
            false
        )
    );
}

#[test]
fn moderated_rx_stays_latched_until_the_drain_unmasks_it() {
    let mut epoch = host_epoch();
    let rx =
        MacInterruptObservation::from_semantic_events(MacInterruptEvents::RX_SUCCESS, false, false);
    epoch.banks_mut().0.latch(rx);
    assert_eq!(epoch.service_mac().0, IrqDisposition::Posted);
    // A frame completing while the group is masked stays latched, unseen.
    epoch.banks_mut().0.latch(rx);
    assert!(!epoch.banks().0.line_asserted());

    epoch.unmask_rx_after_drain();

    assert!(epoch.banks().0.line_asserted());
}

#[test]
fn the_operation_log_counts_what_it_cannot_keep() {
    let mut mac = MacInterruptModel::new();
    for _ in 0..model::OPERATION_LOG_CAPACITY + 2 {
        mac.mask_all();
    }
    assert_eq!(mac.operations().len(), model::OPERATION_LOG_CAPACITY);
    assert_eq!(mac.operations().overflowed(), 2);
    assert!(!mac.operations().matches(&[]));
    mac.clear_operations();
    assert!(mac.operations().is_empty() && mac.operations().matches(&[]));
}
