use super::*;
use oer_portable_time::Duration;
use oer_protocols_ieee80211_contracts_port::{EdcaParameterSet, SupportedEdca};

fn lifetime(core: &mut Core<'_>, role: VifRole, micros: u64) {
    core.set_checked_edca(
        role,
        SupportedEdca::<WideEdca>::new(EdcaParameterSet::new(
            EdcaParameterSet::STANDARD_DEFAULT.wmm(),
            [Duration::from_micros(micros); 4],
        ))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn a_held_msdu_expires_at_its_vifs_exact_deadline_and_returns_once() {
    installed(|pools| {
        let mut hardware = Hardware::default();
        let mut core = enabled(&mut hardware, pools);
        lifetime(&mut core, VifRole::Station, 100);
        lifetime(&mut core, VifRole::AccessPoint, 200);
        core.hold_new_attempts();
        submit(
            &mut core,
            &mut hardware,
            values_on(WmmAccessCategory::BestEffort),
        )
        .unwrap();
        let mut ap = values_on(WmmAccessCategory::Voice);
        ap.role = VifRole::AccessPoint;
        submit(&mut core, &mut hardware, ap).unwrap();
        assert_eq!(core.next_deadline(), Some(at(100)));
        let mut events = Events::default();
        core.tx.timer.advance_to(at(99));
        core.service(&mut hardware, WifiTxWake::Deadline, &mut events)
            .unwrap();
        assert!(events.completions.is_empty());
        core.tx.timer.advance_to(at(100));
        core.service(&mut hardware, WifiTxWake::Deadline, &mut events)
            .unwrap();
        assert_eq!(events.completions, [PublicationEnd::Expired { msdus: 1 }]);
        assert_eq!(events.slots.len(), 1);
        assert_eq!(core.next_deadline(), Some(at(200)));
        core.service(&mut hardware, WifiTxWake::Deadline, &mut events)
            .unwrap();
        assert_eq!(events.slots.len(), 1);
        core.tx.timer.advance_to(at(200));
        core.service(&mut hardware, WifiTxWake::Deadline, &mut events)
            .unwrap();
        assert_eq!(
            events.completions,
            [PublicationEnd::Expired { msdus: 1 }; 2]
        );
        assert_eq!(events.slots.len(), 2);
        assert_eq!(core.expired_msdus(), 2);
        assert_eq!(hardware.publications(), 0);
        assert_eq!(core.next_deadline(), None);
    });
}

#[test]
fn a_hardware_owned_ack_survives_age_but_an_aged_retry_never_publishes() {
    installed(|pools| {
        let mut hardware = Hardware::default();
        let mut core = enabled(&mut hardware, pools);
        lifetime(&mut core, VifRole::Station, 100);
        submit(&mut core, &mut hardware, values(&data_frame(BSSID), false)).unwrap();
        core.tx.timer.advance_to(at(100));
        assert_eq!(
            complete(&mut core, &mut hardware, 0),
            [completed(TxStatus::Success, Some(0x60))]
        );
        assert_eq!(core.expired_msdus(), 0);
        // The port backend supplies the original sample on every retry.
        let mut retry = values(&data_frame(BSSID), false);
        retry.retry = true;
        retry.admitted_at = at(0);
        submit(&mut core, &mut hardware, retry).unwrap();
        let mut events = Events::default();
        core.service(&mut hardware, WifiTxWake::Deadline, &mut events)
            .unwrap();
        assert_eq!(events.completions, [PublicationEnd::Expired { msdus: 1 }]);
        assert_eq!(events.slots.len(), 1);
        assert_eq!(hardware.publications(), 1);
        assert_eq!(core.expired_msdus(), 1);
    });
}
