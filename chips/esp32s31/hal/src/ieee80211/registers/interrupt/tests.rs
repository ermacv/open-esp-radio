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

    activate_mac_interrupt_epoch(&mut backend, MacInterruptMask::STATION);

    assert_eq!(
        backend.events,
        [
            Event::MaskMac,
            Event::MaskPower,
            Event::ClearMac,
            Event::ClearPower,
            Event::Fence,
            Event::PublishMac(MacInterruptMask::STATION),
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

mod host {
    use crate::ieee80211::registers::{
        HostWords, MacInterruptMask, interrupt::MacInterruptEnableState,
    };

    /// The value each changed word of one part holds, in memory order.
    fn values(changes: &[(&'static str, usize, u32)], part: &str) -> std::vec::Vec<u32> {
        changes
            .iter()
            .filter(|(changed, _, _)| *changed == part)
            .map(|(_, _, value)| *value)
            .collect()
    }

    #[test]
    fn activation_publishes_the_station_image_and_clears_both_banks() {
        let before = HostWords::filled(0);
        let mut after = before.clone();
        {
            let owners = after.owners();
            let (interrupts, _power) = owners.interrupts.activate(MacInterruptMask::STATION);
            let setup_state = interrupts.mac.read_enable();
            assert!(setup_state.tx_complete() && !setup_state.bss_color_collision());
        }
        let changes = before.changes(&after);
        // The MAC bank: the station enable image and the whole clear word.
        assert_eq!(
            values(&changes, "interrupt"),
            [MacInterruptMask::STATION.bits(), u32::MAX]
        );
        // The power bank: the station TBTT enable alone and the clear word.
        let power = values(&changes, "power_interrupt");
        assert_eq!(power.len(), 2);
        assert_eq!(power[1], u32::MAX);
        assert_eq!(changes.len(), 4);

        let owners = after.owners();
        assert_eq!(
            owners.interrupts.mac_interrupt_enable(),
            MacInterruptEnableState::Station
        );
        assert!(owners.interrupts.power.read_enable().tbtt_0());
        assert!(!owners.interrupts.power.read_enable().tbtt_1());
    }

    #[test]
    fn deactivation_masks_both_banks_and_returns_the_setup() {
        let mut words = HostWords::filled(0);
        let owners = words.owners();
        let (mut interrupts, power) = owners.interrupts.activate(MacInterruptMask::STATION);
        interrupts.mask_rx_delivery_interrupts();
        let enable = interrupts.mac.read_enable();
        assert!(!enable.rx_success() && !enable.rx_associated_auxiliary_5());
        assert!(enable.tx_complete(), "TX stays live");
        interrupts.unmask_rx_delivery_interrupts();
        let setup = interrupts.deactivate(power);
        assert_eq!(
            setup.mac_interrupt_enable(),
            MacInterruptEnableState::Disabled
        );
        assert!(!setup.power.read_enable().tbtt_0());
    }

    #[test]
    fn a_sample_acknowledges_exactly_its_events() {
        let before = HostWords::filled(0);
        let mut after = before.clone();
        {
            let owners = after.owners();
            let (mut interrupts, mut power) = owners.interrupts.activate(MacInterruptMask::NONE);
            let snapshot = interrupts.mac_interrupt_status();
            assert!(snapshot.observation().is_empty());
            interrupts.acknowledge_mac_interrupts(snapshot);
            let snapshot = power.power_interrupt_status();
            assert!(snapshot.observation().is_empty());
            power.acknowledge_power_interrupts(snapshot);
        }
        // The empty acknowledgements store zero over the clear images the
        // activation left: the memory is as it started but for the TBTT 0
        // enable the activation publishes.
        let changes = before.changes(&after);
        assert_eq!(changes.len(), 1, "{changes:x?}");
        let (part, _, value) = changes[0];
        assert_eq!((part, value), ("power_interrupt", 1 << 3), "{changes:x?}");
    }

    #[test]
    fn a_full_status_reads_every_cause() {
        let mut words = HostWords::filled(u32::MAX);
        let owners = words.owners();
        let (interrupts, power) = owners.interrupts.activate(MacInterruptMask::NONE);
        let observation = interrupts.mac_interrupt_status().observation();
        assert!(observation.work_events().tx_complete());
        assert!(observation.work_events().bss_color_collision());
        assert!(observation.work_events().rx_success());
        assert!(observation.work_events().tx_timeout());
        assert!(observation.has_auxiliary_event() && observation.has_unhandled_event());
        let power = power.power_interrupt_status().observation();
        assert!(power.sta_tbtt() && power.has_unhandled_event());
        assert!(power.tsf_timer_0() && power.tsf_timer_3());
    }

    #[test]
    fn preparing_a_connected_station_disables_the_beacon_filter_then_its_event() {
        let before = HostWords::filled(u32::MAX);
        let mut after = before.clone();
        {
            let mut owners = after.owners();
            let _prepared = owners
                .interrupts
                .prepare_connected_sta_without_power_save(&mut owners.registers);
            let enable = owners.interrupts.mac.read_enable();
            assert!(!enable.sta_beacon_filter() && enable.rx_success());
            let control = owners
                .registers
                .mac
                .wifi_mac_sta_beacon_filter
                .read_sta_beacon_filter_control();
            assert_eq!(control.bits_0_2_opaque().get(), 0);
            assert_ne!(control.bits_3_31_opaque().get(), 0, "kept");
        }
        assert_eq!(before.changes(&after).len(), 2);
    }

    #[test]
    fn mask_and_clear_stores_the_zero_enable_then_the_whole_clear_word() {
        let before = HostWords::filled(0x5a5a_5a5a);
        let mut after = before.clone();
        after
            .owners()
            .interrupts
            .mask_and_clear_all_mac_interrupts();
        assert_eq!(values(&before.changes(&after), "interrupt"), [0, u32::MAX]);
    }
}
