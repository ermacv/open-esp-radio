use super::*;

macro_rules! phase {
    ($owner:expr, $phase:ident) => {{
        let CoreView::$phase(view) = $owner.view() else {
            panic!("retained owner has the expected phase")
        };
        view
    }};
}

#[test]
fn retained_bank_waits_for_the_published_prefix_and_preserves_its_closed_gate() {
    installed(|pools| {
        let fixture = core(pools);
        let mut free = fixture.free;
        let mut tickets = fixture.tickets;
        let mut bank = fixture.inner.0;
        let mut hardware = Hardware::default();
        let mut events = Events::default();
        assert!(matches!(
            phase!(bank, Disabled).enable_checked(),
            LifecycleStart::Enabled(_)
        ));
        let command = phase!(bank, Open).configure(station()).ok().unwrap();
        assert!(matches!(
            command.advance(&mut hardware, &mut events),
            ConfigurationProgress::Configured(_)
        ));
        phase!(bank, Open)
            .submit_checked(
                &mut hardware,
                free[BE].take().unwrap(),
                TxAdmission::Open(tickets[0].take().unwrap()),
                values_on(WmmAccessCategory::BestEffort).values,
            )
            .ok()
            .unwrap();
        let command = phase!(bank, Open)
            .configure(CoreVif {
                receive: ReceiveFilter::NONE,
                ..station()
            })
            .ok()
            .unwrap();
        assert!(matches!(
            command.advance(&mut hardware, &mut events),
            ConfigurationProgress::Waiting(_)
        ));
        assert!(matches!(bank.view(), CoreView::ConfigureOpen(_)));
        assert_eq!(hardware.station_disabled, 0);
        assert!(events.completions.is_empty());
        hardware.completion[BE] = Some(MacTxCompletionObservation::new_model(0, 0));
        phase!(bank, ConfigureOpen)
            .service(&mut hardware, interrupt(EVENT_TX_COMPLETE), &mut events)
            .unwrap();
        assert_eq!(events.returned.len(), 1);
        assert_eq!(hardware.station_disabled, 0);
        assert!(matches!(
            phase!(bank, ConfigureOpen).advance(&mut hardware, &mut events),
            ConfigurationProgress::Configured(_)
        ));
        assert_eq!(hardware.station_disabled, 1);

        let closing = phase!(bank, Open).hold_new_attempts();
        assert!(matches!(
            closing.close_gate(&mut hardware),
            GateClosure::Closed(_)
        ));
        assert!(hardware.tx_blocked);
        assert!(
            phase!(bank, Closed)
                .configure(CoreVif {
                    address: OTHER_BSS,
                    ..station()
                })
                .is_err()
        );
        assert!(matches!(bank.view(), CoreView::Closed(_)));
        let command = phase!(bank, Closed).configure(station()).ok().unwrap();
        assert!(matches!(
            command.advance(&mut hardware, &mut events),
            ConfigurationProgress::Configured(_)
        ));
        assert!(hardware.tx_blocked);
        let disabling = phase!(bank, Closed).disable_checked(&mut hardware, &mut events);
        assert!(disabling.finish(&mut events).is_ok());
        for _ in 0..3 {
            assert!(matches!(bank.view(), CoreView::DisabledClosed(_)));
        }
        assert_eq!(events.disabled, 1);
        let mut disabled = phase!(bank, DisabledClosed);
        disabled.set_checked_channel(channel(11));
        disabled.radio_tuned_to(channel(11));
        assert!(matches!(
            disabled.enable_checked(),
            LifecycleStart::Enabled(_)
        ));
        assert!(hardware.tx_blocked);
        assert!(
            phase!(bank, Closed)
                .open_gate(&mut hardware, &mut events)
                .is_ok()
        );
        assert!(!hardware.tx_blocked);
        assert_eq!(hardware.publications(), 1);
        let disabling = phase!(bank, Open).disable_checked(&mut hardware, &mut events);
        assert!(disabling.finish(&mut events).is_ok());
        assert_eq!(events.disabled, 2);
        drop(phase!(bank, Disabled).dismantle().into_transmitter());
        assert!(matches!(bank.view(), CoreView::Vacant(_)));
    });
}

#[test]
fn direct_owner_prepares_and_runs_the_complete_lifecycle_in_place_without_a_port() {
    installed(|pools| {
        let mut free = pools.queues.map(Some);
        let mut tickets = pools.tickets.map(Some);
        let mut owner = CoreOwner::vacant();
        let mut hardware = Hardware::default();
        let mut events = Events::default();
        let disabled = phase!(owner, Vacant).prepare(
            pools.authority,
            OrdinaryTxOwner::new(WifiTxResources {
                slot: slot(),
                policy: WifiTxRuntimePolicy::vendor_defaults(),
                power: Power,
                entropy: entropy as fn() -> u32,
                timer: Timer::default(),
            })
            .into_idle()
            .ok()
            .unwrap(),
            LowerMacConfig {
                station_address: STATION,
                channel: channel(6),
                publication_timeout: oer_time::Duration::from_micros(TIMEOUT),
                tsf_epoch: 1,
            },
            &RadioNine,
        );
        let address = core::ptr::from_ref(&*disabled);
        let LifecycleStart::Enabled(mut enabled) = disabled.enable_checked() else {
            panic!("the prepared radio is already on its configured channel")
        };
        assert_eq!(core::ptr::from_ref(&*enabled), address);
        enabled
            .submit_checked(
                &mut hardware,
                free[BE].take().unwrap(),
                TxAdmission::Open(tickets[0].take().unwrap()),
                values_on(WmmAccessCategory::BestEffort).values,
            )
            .ok()
            .unwrap();
        let closing = enabled.hold_new_attempts();
        assert_eq!(core::ptr::from_ref(&*closing), address);
        let GateClosure::Closing(mut closing) = closing.close_gate(&mut hardware) else {
            panic!("a published predecessor retains the closing barrier")
        };
        assert!(!hardware.tx_blocked);
        hardware.completion[BE] = Some(MacTxCompletionObservation::new_model(0, 0));
        closing
            .service(&mut hardware, interrupt(EVENT_TX_COMPLETE), &mut events)
            .unwrap();
        let GateClosure::Closed(closed) = closing.close_gate(&mut hardware) else {
            panic!("the final terminal releases the gate")
        };
        assert_eq!(core::ptr::from_ref(&*closed), address);
        assert!(hardware.tx_blocked);
        let enabled = closed.open_gate(&mut hardware, &mut events).ok().unwrap();
        assert_eq!(core::ptr::from_ref(&*enabled), address);
        assert!(!hardware.tx_blocked);
        let disabling = enabled.disable_checked(&mut hardware, &mut events);
        assert_eq!(core::ptr::from_ref(&*disabling), address);
        let disabled = disabling.finish(&mut events).ok().unwrap();
        assert_eq!(core::ptr::from_ref(&*disabled), address);
        assert_eq!(events.disabled, 1);
        assert_eq!(hardware.publications(), 1);
        assert_eq!(events.returned.len(), 1);
        drop(disabled.dismantle());
        assert!(matches!(owner.view(), CoreView::Vacant(_)));
    });
}
