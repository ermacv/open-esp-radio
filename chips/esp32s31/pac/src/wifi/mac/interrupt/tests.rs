use super::{
    InterruptBankMaskBackend, MacInterruptActivationBackend, MacInterruptMask,
    activate_mac_interrupt_epoch, mask_interrupt_bank,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Event {
    MaskMac,
    MaskPower,
    ClearMac,
    ClearPower,
    Fence,
    PublishMac(MacInterruptMask),
    PublishPower,
}

#[derive(Default)]
struct Backend {
    events: std::vec::Vec<Event>,
}

impl MacInterruptActivationBackend for Backend {
    fn mask_mac_events(&mut self) {
        self.events.push(Event::MaskMac);
    }

    fn mask_power_events(&mut self) {
        self.events.push(Event::MaskPower);
    }

    fn clear_mac_events(&mut self) {
        self.events.push(Event::ClearMac);
    }

    fn clear_power_events(&mut self) {
        self.events.push(Event::ClearPower);
    }

    fn publish_mac_events(&mut self, event_mask: MacInterruptMask) {
        self.events.push(Event::PublishMac(event_mask));
    }

    fn publish_power_events(&mut self) {
        self.events.push(Event::PublishPower);
    }

    fn fence(&mut self) {
        self.events.push(Event::Fence);
    }
}

#[test]
fn activation_clears_stale_events_before_publishing_runtime_masks() {
    let mut backend = Backend::default();

    activate_mac_interrupt_epoch(&mut backend, MacInterruptMask::COLD_RX);

    assert_eq!(
        backend.events,
        [
            Event::MaskMac,
            Event::MaskPower,
            Event::ClearMac,
            Event::ClearPower,
            Event::Fence,
            Event::PublishMac(MacInterruptMask::COLD_RX),
            Event::PublishPower,
            Event::Fence,
        ]
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BankEvent {
    Mask,
    Fence,
}

#[derive(Default)]
struct Bank {
    events: std::vec::Vec<BankEvent>,
}

impl InterruptBankMaskBackend for Bank {
    fn mask_events(&mut self) {
        self.events.push(BankEvent::Mask);
    }

    fn fence(&mut self) {
        self.events.push(BankEvent::Fence);
    }
}

#[test]
fn masking_a_live_bank_publishes_the_mask_then_orders_it_without_clearing() {
    let mut bank = Bank::default();

    mask_interrupt_bank(&mut bank);

    // The bank backend has no clear operation: a masked event stays latched.
    assert_eq!(bank.events, [BankEvent::Mask, BankEvent::Fence]);
}
