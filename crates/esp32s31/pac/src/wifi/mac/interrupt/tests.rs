use super::{MacInterruptActivationBackend, MacInterruptMask, activate_mac_interrupt_epoch};

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
