use super::*;
use oer_ieee80211_port::{EdcaParameterSet, SupportedEdca};
use oer_time::Duration;

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

#[test]
fn aggregate_expiry_returns_every_owner_and_reformation_keeps_the_live_age() {
    installed(|pools| {
        let mut hardware = Hardware::default();
        let mut core = enabled(&mut hardware, pools);
        lifetime(&mut core, VifRole::Station, 100);
        let source = backings();
        let mut buffer = aggregate_of(source, 1, 26);
        let mpdu = buffer.push_mpdu(26, at(50)).unwrap();
        mpdu.copy_from_slice(&data_frame(BSSID));
        mpdu[22..24].copy_from_slice(&(101_u16 << 4).to_le_bytes());
        let plan = core
            .plan_ampdu(ampdu_values(ht_rate(7)), &mut buffer)
            .unwrap();
        core.tx.timer.advance_to(at(100));
        let Err(ampdu::AmpduStartError::Expired(expired)) = ampdu::publish(
            &mut hardware,
            buffer,
            plan,
            core.now(),
            Duration::from_micros(TIMEOUT),
        ) else {
            panic!("the first member expired before publication");
        };
        assert_eq!(expired.expired_msdus(), 1);
        let mut events = Events::default();
        let mut buffer = core
            .complete_expired_ampdu(&mut hardware, expired, &mut events)
            .unwrap();
        core.reclaim(&mut events);
        assert_eq!(events.completions, [PublicationEnd::Expired { msdus: 1 }]);
        assert_eq!(source.free(), BACKINGS - 2);
        assert_eq!(buffer.admitted_at(0), Some(at(0)));
        assert_eq!(buffer.admitted_at(1), Some(at(50)));
        assert!(buffer.retain_subframes(4).is_err());
        assert_eq!(source.free(), BACKINGS - 2);
        buffer.retain_subframes(2).unwrap();
        assert_eq!(source.free(), BACKINGS - 1);
        assert_eq!(buffer.admitted_at(0), Some(at(50)));
        let plan = core
            .plan_ampdu(ampdu_values(ht_rate(7)), &mut buffer)
            .unwrap();
        core.tx.timer.advance_to(at(150));
        let Err(ampdu::AmpduStartError::Expired(expired)) = ampdu::publish(
            &mut hardware,
            buffer,
            plan,
            core.now(),
            Duration::from_micros(TIMEOUT),
        ) else {
            panic!("reformation keeps the live member's original age");
        };
        drop(
            core.complete_expired_ampdu(&mut hardware, expired, &mut events)
                .unwrap(),
        );
        core.reclaim(&mut events);
        assert_eq!(
            events.completions,
            [PublicationEnd::Expired { msdus: 1 }; 2]
        );
        assert_eq!(core.expired_msdus(), 2);
        assert_eq!(hardware.publications(), 0);
        assert_eq!(source.free(), BACKINGS);
    });
}

#[test]
fn a_published_aggregate_returns_its_original_admission_samples_after_age() {
    installed(|pools| {
        let mut hardware = Hardware::default();
        let mut core = enabled(&mut hardware, pools);
        lifetime(&mut core, VifRole::Station, 100);
        let source = backings();
        let published = publish_ampdu(
            &mut core,
            &mut hardware,
            aggregate_of(source, 2, 26),
            ampdu_values(ht_rate(7)),
        )
        .unwrap();
        core.tx.timer.advance_to(at(100));
        hardware.block_ack_completion[BE] = Some(block_ack_completion(0, 100, 3, true));
        let AmpduProgress::Complete(completed) = ampdu::service(
            &mut hardware,
            published,
            interrupt(EVENT_TX_COMPLETE),
            true,
            core.now(),
        )
        .unwrap() else {
            panic!("the hardware-owned aggregate completed");
        };
        let mut events = Events::default();
        let (owner, samples) = core
            .complete_ampdu(&mut hardware, completed, &mut events)
            .unwrap();
        core.reclaim(&mut events);
        assert_eq!(&samples[..2], &[at(0); 2]);
        assert_eq!(status_of(&events.completions[0]), Some(TxStatus::Success));
        assert_eq!(core.expired_msdus(), 0);
        drop(owner);
        assert_eq!(source.free(), BACKINGS);
    });
}

#[test]
fn unpublished_aggregate_expiry_releases_the_key_after_its_terminal() {
    installed(|pools| {
        let mut hardware = Hardware::default();
        let mut core = enabled(&mut hardware, pools);
        lifetime(&mut core, VifRole::Station, 100);
        let key = core
            .install_key(
                &mut hardware,
                TransmitKeyScope::StationPairwise { peer: BSSID },
                &[0x39; 16],
            )
            .unwrap();
        let index = key.index;
        let queue = core.free[BE].take().unwrap();
        let ticket = core.take_ticket();
        let admission = TxAdmission::Protected(core.admit_key(&key, ticket).ok().unwrap());
        let source = backings();
        let mut buffer = aggregate_of(source, 0, 26);
        let frame = buffer.push_mpdu(26, at(0)).unwrap();
        frame.copy_from_slice(&data_frame(BSSID));
        let fc =
            oer_ieee80211_wire::frame::FrameControl::new(u16::from_le_bytes([frame[0], frame[1]]))
                .unwrap();
        frame[..2].copy_from_slice(&fc.with_protected(true).get().to_le_bytes());
        let plan = core
            .inner
            .plan_ampdu(queue, admission, ampdu_values(ht_rate(7)), &mut buffer)
            .unwrap();
        let mut events = Events::default();
        core.remove_checked_key(&mut hardware, key, &mut events);
        assert!(events.removed.is_empty());
        core.tx.timer.advance_to(at(100));
        let Err(ampdu::AmpduStartError::Expired(expired)) = ampdu::publish(
            &mut hardware,
            buffer,
            plan,
            core.now(),
            Duration::from_micros(TIMEOUT),
        ) else {
            panic!("age precedes the first DMA commit");
        };
        drop(
            core.complete_expired_ampdu(&mut hardware, expired, &mut events)
                .unwrap(),
        );
        assert_eq!(events.order, ["tx", "key"]);
        assert_eq!(events.removed.len(), 1);
        assert_eq!(core.keys.uses[index].get(), 0);
        assert_eq!(source.free(), BACKINGS);
        assert_eq!(hardware.publications(), 0);
    });
}
