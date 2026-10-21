use super::*;

#[test]
fn retained_bank_waits_for_the_published_prefix_and_preserves_its_closed_gate() {
    installed(|pools| {
        let fixture = core(pools);
        let mut free = fixture.free;
        let mut tickets = fixture.tickets;
        let mut bank = CoreOwner::vacant();
        bank.prepare(fixture.inner.take_disabled());
        let mut hardware = Hardware::default();
        let mut events = Events::default();
        bank.tune(channel(6));
        assert!(bank.configure(station()).is_ok());
        assert!(bank.advance_configuration(&mut hardware, &mut events));
        bank.submit(
            &mut hardware,
            free[BE].take().unwrap(),
            TxAdmission::Open(tickets[0].take().unwrap()),
            values_on(WmmAccessCategory::BestEffort).values,
        )
        .ok()
        .unwrap();
        assert!(
            bank.configure(CoreVif {
                receive: ReceiveFilter::NONE,
                ..station()
            })
            .is_ok()
        );
        assert!(!bank.advance_configuration(&mut hardware, &mut events));
        assert!(bank.configuring());
        assert_eq!(hardware.station_disabled, 0);
        assert!(events.completions.is_empty());
        hardware.completion[BE] = Some(MacTxCompletionObservation::new_model(0, 0));
        bank.service(&mut hardware, interrupt(EVENT_TX_COMPLETE), &mut events)
            .unwrap();
        assert_eq!(events.returned.len(), 1);
        assert_eq!(hardware.station_disabled, 0);
        assert!(bank.advance_configuration(&mut hardware, &mut events));
        assert_eq!(hardware.station_disabled, 1);

        bank.hold();
        assert!(bank.close(&mut hardware));
        assert!(hardware.tx_blocked);
        assert!(
            bank.configure(CoreVif {
                address: OTHER_BSS,
                ..station()
            })
            .is_err()
        );
        assert!(!bank.configuring());
        assert!(bank.configure(station()).is_ok());
        assert!(bank.advance_configuration(&mut hardware, &mut events));
        assert!(hardware.tx_blocked);
        bank.disable(&mut hardware, &mut events);
        assert!(bank.finish_disable(&mut events));
        assert!(bank.finish_disable(&mut events));
        assert_eq!(events.disabled, 1);
        bank.tune(channel(11));
        assert!(hardware.tx_blocked);
        assert!(bank.open(&mut hardware, &mut events).is_ok());
        assert!(!hardware.tx_blocked);
        assert_eq!(hardware.publications(), 1);
        bank.disable(&mut hardware, &mut events);
        assert!(bank.finish_disable(&mut events));
        assert_eq!(events.disabled, 2);
        drop(bank.take_transmitter());
    });
}
