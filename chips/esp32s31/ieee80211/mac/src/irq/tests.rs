use super::*;
use epoch::{EpochState, InterruptEpoch, Moderated, Unmoderated};
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
    fn clear_all(&mut self, _: &BanksMasked) {
        self.operations.push("clear all");
    }
    fn publish(&mut self, _: MacInterruptMask) {
        self.operations.push("publish");
    }
    fn enable_state(&self) -> MacInterruptEnableState {
        MacInterruptEnableState::Unknown
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
    let (disposition, snapshot) =
        handle_mac_irq::<Moderated, _>(&mut interrupt).expect("nonfatal sample");
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
    let (disposition, snapshot) =
        handle_mac_irq::<Unmoderated, _>(&mut interrupt).expect("nonfatal sample");
    assert_eq!(interrupt.operations, ["read", "ack"]);
    assert_eq!(disposition, IrqDisposition::AcknowledgedOnly);
    assert_eq!(snapshot.posted_events, 0);
}

#[test]
fn spurious_mac_does_not_mask_acknowledge_or_publish() {
    let mut interrupt = Interrupt::default();
    let (disposition, snapshot) =
        handle_mac_irq::<Moderated, _>(&mut interrupt).expect("nonfatal sample");
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
    let (disposition, snapshot) =
        handle_mac_irq::<Moderated, _>(&mut interrupt).expect("nonfatal sample");
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
    let (disposition, _) = handle_mac_irq::<Moderated, _>(&mut interrupt).expect("nonfatal sample");
    assert_eq!(disposition, IrqDisposition::Spurious);
    assert_eq!(interrupt.latched(), color);

    interrupt.latch(MacInterruptObservation::from_semantic_events(
        MacInterruptEvents::RX_SUCCESS,
        false,
        false,
    ));
    let (_, snapshot) = handle_mac_irq::<Moderated, _>(&mut interrupt).expect("nonfatal sample");
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
    fn clear_all(&mut self, _: &BanksMasked) {}
    fn enable_station_tbtt(&mut self) {}
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

type HostEpoch = InterruptEpoch<Moderated, MacInterruptModel, PowerInterruptModel>;

fn host_epoch() -> HostEpoch {
    InterruptEpoch::for_host_test(MacInterruptModel::new(), PowerInterruptModel::new())
}

fn clear_logs(epoch: &mut HostEpoch) {
    let (mac, power) = epoch.banks_mut();
    mac.clear_operations();
    power.clear_operations();
}

#[test]
fn activation_masks_clears_then_publishes_mac_before_power() {
    let mut epoch: HostEpoch =
        InterruptEpoch::new(MacInterruptModel::masked(), PowerInterruptModel::masked());
    assert_eq!(epoch.state(), EpochState::Closed);
    // Stale events from before the epoch.
    let (mac, power) = epoch.banks_mut();
    mac.latch(rx_and_tx());
    power.latch(sta_tbtt());

    epoch
        .activate(MacInterruptMask::STATION)
        .expect("healthy epoch");

    assert_eq!(epoch.state(), EpochState::Active);
    let (mac, power) = epoch.banks();
    assert!(mac.operations().matches(&[
        MacInterruptOperation::MaskAll,
        MacInterruptOperation::ClearAll,
        MacInterruptOperation::Publish(MacInterruptMask::STATION),
    ]));
    assert!(power.operations().matches(&[
        PowerInterruptOperation::MaskAll,
        PowerInterruptOperation::ClearAll,
        PowerInterruptOperation::EnableStationTbtt,
    ]));
    // The stale events were cleared before the mask went live.
    assert!(!mac.line_asserted() && !power.line_asserted());
    assert!(mac.latched().is_empty() && power.latched().is_empty());
    assert_eq!(
        epoch.mac_interrupt_enable(),
        MacInterruptEnableState::Station
    );
}

#[test]
fn closing_masks_then_clears_both_banks() {
    let mut epoch = host_epoch();
    clear_logs(&mut epoch);
    let (mac, power) = epoch.banks_mut();
    mac.latch(rx_and_tx());
    power.latch(sta_tbtt());

    epoch.close();

    assert_eq!(epoch.state(), EpochState::Closed);
    let (mac, power) = epoch.banks();
    assert!(mac.operations().matches(&[
        MacInterruptOperation::MaskAll,
        MacInterruptOperation::ClearAll,
    ]));
    assert!(power.operations().matches(&[
        PowerInterruptOperation::MaskAll,
        PowerInterruptOperation::ClearAll,
    ]));
    assert!(mac.latched().is_empty() && power.latched().is_empty());
    assert_eq!(
        epoch.mac_interrupt_enable(),
        MacInterruptEnableState::Disabled
    );
}

#[test]
fn a_closed_epoch_closes_its_banks_again_on_a_stray_interrupt() {
    let mut epoch = host_epoch();
    epoch.close();
    clear_logs(&mut epoch);
    // Nothing enabled: nothing to close, and no work is serviced.
    assert!(!epoch.close_stray().expect("healthy epoch"));
    assert_eq!(epoch.service_mac(), Ok(None));
    assert_eq!(epoch.service_power(), None);
    assert!(
        epoch
            .banks()
            .0
            .operations()
            .matches(&[MacInterruptOperation::Status])
    );

    // An enable a reset restored, with an event latched under it.
    clear_logs(&mut epoch);
    epoch.banks_mut().0.publish(MacInterruptMask::STATION);
    epoch.banks_mut().0.latch(rx_and_tx());
    assert!(epoch.banks().0.line_asserted());

    assert!(epoch.close_stray().expect("healthy epoch"));

    assert_eq!(epoch.state(), EpochState::Closed);
    let (mac, _) = epoch.banks();
    assert!(!mac.line_asserted() && mac.latched().is_empty());
    // Sampled, then closed; nothing acknowledged as work.
    assert!(mac.operations().matches(&[
        MacInterruptOperation::Publish(MacInterruptMask::STATION),
        MacInterruptOperation::Status,
        MacInterruptOperation::MaskAll,
        MacInterruptOperation::ClearAll,
    ]));
    // An open epoch has nothing stray.
    epoch
        .activate(MacInterruptMask::STATION)
        .expect("healthy epoch");
    assert!(!epoch.close_stray().expect("healthy epoch"));
}

#[test]
fn mask_all_masks_both_banks_without_acknowledging() {
    let mut epoch = host_epoch();
    let (mac, power) = epoch.banks_mut();
    mac.latch(rx_and_tx());
    power.latch(sta_tbtt());
    assert!(mac.line_asserted() && power.line_asserted());

    // Dropping the proof keeps the banks masked and their events latched.
    drop(epoch.mask_all());

    assert_eq!(epoch.state(), EpochState::Masked);
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

    // A late entry of the handler sees nothing and acknowledges nothing.
    assert_eq!(
        epoch
            .service_mac()
            .expect("healthy epoch")
            .map(|(disposition, _)| disposition),
        Some(IrqDisposition::Spurious)
    );
    assert_eq!(
        epoch.service_power().map(|(disposition, _)| disposition),
        Some(PowerIrqDisposition::Spurious)
    );
    let (mac, power) = epoch.banks();
    assert_eq!(mac.latched(), rx_and_tx());
    assert_eq!(power.latched(), sta_tbtt());
}

#[test]
fn an_unmask_after_mask_all_or_close_is_refused() {
    let mut epoch = host_epoch();
    epoch.banks_mut().0.latch(rx_and_tx());
    drop(epoch.mask_all());
    clear_logs(&mut epoch);

    assert!(!epoch.unmask_rx_after_drain());
    epoch.close();
    clear_logs(&mut epoch);
    assert!(!epoch.unmask_rx_after_drain());

    let (mac, _) = epoch.banks();
    assert!(mac.operations().is_empty());
    assert!(!mac.rx_delivery_enabled() && !mac.other_sources_enabled());
}

#[test]
fn moderated_rx_stays_latched_until_the_drain_unmasks_it() {
    let mut epoch = host_epoch();
    let rx =
        MacInterruptObservation::from_semantic_events(MacInterruptEvents::RX_SUCCESS, false, false);
    epoch.banks_mut().0.latch(rx);
    assert_eq!(
        epoch
            .service_mac()
            .expect("healthy epoch")
            .map(|(disposition, _)| disposition),
        Some(IrqDisposition::Posted)
    );
    // A frame completing while the group is masked stays latched, unseen.
    epoch.banks_mut().0.latch(rx);
    assert!(!epoch.banks().0.line_asserted());

    assert!(epoch.unmask_rx_after_drain());

    let (mac, _) = epoch.banks();
    assert!(mac.line_asserted());
    assert!(mac.rx_delivery_enabled() && mac.other_sources_enabled());
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

#[test]
fn hardware_assertion_keeps_the_complete_sample_and_quarantines_both_banks() {
    let status = 0xe531_aa57; // Diagnostic payload, never decoded by policy.
    let observation = rx_and_tx().with_hardware_assertion(status);
    let mut mac = MacInterruptModel::new();
    mac.latch(observation);
    let mut power = PowerInterruptModel::new();
    power.latch(sta_tbtt());
    let mut epoch = InterruptEpoch::<Moderated, _, _>::for_host_test(mac, power);
    let fault = Fault::HardwareAssertion { status };
    assert_eq!(epoch.service_mac(), Err(fault));
    assert_eq!(epoch.state(), EpochState::Quarantined(fault));
    let (mac, power) = epoch.banks();
    assert_eq!(mac.latched().hardware_assertion_status(), Some(status));
    assert!(mac.operations().matches(&[
        MacInterruptOperation::Status,
        MacInterruptOperation::MaskAll
    ]));
    assert!(
        power
            .operations()
            .matches(&[PowerInterruptOperation::MaskAll])
    );
    assert!(!mac.line_asserted() && !power.line_asserted());
    assert!(!epoch.unmask_rx_after_drain());
    assert_eq!(epoch.activate(MacInterruptMask::STATION), Err(fault));
    epoch.close();
    assert_eq!(epoch.state(), EpochState::Quarantined(fault));
    assert_eq!(epoch.service_mac(), Err(fault));
}

#[test]
fn closed_epoch_reports_an_inherited_hardware_assertion_before_clearing() {
    let mut mac = MacInterruptModel::new();
    mac.latch(MacInterruptObservation::default().with_hardware_assertion(0x1234_5678));
    let mut epoch = InterruptEpoch::<Moderated, _, _>::new(mac, PowerInterruptModel::new());
    let fault = Fault::HardwareAssertion {
        status: 0x1234_5678,
    };
    assert_eq!(epoch.close_stray(), Err(fault));
    assert_eq!(epoch.state(), EpochState::Quarantined(fault));
    assert!(epoch.banks().0.operations().matches(&[
        MacInterruptOperation::Status,
        MacInterruptOperation::MaskAll
    ]));
}

#[test]
fn masked_power_status_hides_raw_and_only_a_retained_positive_sample_clears_it() {
    let mut power = PowerInterruptModel::new();
    let cause = sta_tbtt();
    power.latch(cause);
    let positive = power.status();
    power.mask_all();
    assert!(power.status().is_empty());
    assert_eq!(power.latched(), cause);
    let (_, masked) = handle_power_irq(&mut power);
    assert!(masked.observation.is_empty());
    assert_eq!(
        power.latched(),
        cause,
        "zero STATUS cannot clear hidden RAW"
    );
    power.enable_station_tbtt();
    assert_eq!(power.status(), cause, "unmask redelivers pending");
    power.mask_all();
    power.acknowledge(positive);
    power.enable_station_tbtt();
    assert!(power.status().is_empty());
    assert!(
        power.latched().is_empty(),
        "positive sample clears under mask"
    );
}
