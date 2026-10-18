//! Reservation ownership at AP publication, cancellation and completion edges.

use core::num::NonZeroU32;

use crate::datapath::PinnedTxFrame;

use oer_esp32s31_ieee80211::tx::WifiTxProgress;

use oer_ieee80211_softmac::MacTxWork;

use super::{
    super::{
        AccessPointNetworkTx, AccessPointTxStorage, ApTxFlowKey,
        airtime::{
            AccessPointAirtimeError, AccessPointAirtimePeer, AccessPointAirtimeStorage, Accounting,
        },
    },
    support::with_authorized_ap,
};

fn us(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).unwrap()
}
// An intentionally explicit test tariff. It does not estimate radio airtime.
fn tariff(_: AccessPointAirtimePeer, work: &MacTxWork) -> Option<NonZeroU32> {
    work.publications.checked_mul(600).and_then(NonZeroU32::new)
}
fn work(publications: u32) -> MacTxWork {
    MacTxWork {
        publications,
        ..MacTxWork::default()
    }
}
fn group() -> ApTxFlowKey {
    ApTxFlowKey::unbound_from_ethernet(&[255; 14])
}

#[test]
fn active_completion_does_not_refund_prepared_successor() {
    with_authorized_ap(|engine| {
        let mut storage = AccessPointAirtimeStorage::new(us(1000));
        let mut accounting = Accounting::new(&mut storage, us(100), tariff);
        accounting.reserve_active(engine, group()).unwrap();
        accounting.publish_active();
        accounting.reserve_standby(engine, group()).unwrap();
        assert_eq!(accounting.balance(AccessPointAirtimePeer::Group), Some(0));
        // Two publications, including a retry, settle the original reservation.
        accounting.complete_active(work(2)).unwrap();
        assert_eq!(
            accounting.balance(AccessPointAirtimePeer::Group),
            Some(-200)
        );
        accounting.publish_standby();
        accounting.complete_active(work(1)).unwrap();
        assert_eq!(accounting.balance(AccessPointAirtimePeer::Group), Some(200));
    });
}

#[test]
fn cancelling_preparation_keeps_published_work_charged() {
    with_authorized_ap(|engine| {
        let mut storage = AccessPointAirtimeStorage::new(us(1000));
        let mut accounting = Accounting::new(&mut storage, us(100), tariff);
        accounting.reserve_active(engine, group()).unwrap();
        accounting.publish_active();
        accounting.reserve_standby(engine, group()).unwrap();
        accounting.cancel_standby().unwrap();
        let balance = accounting.balance(AccessPointAirtimePeer::Group);
        accounting.cancel_active_preparation().unwrap();
        assert_eq!(accounting.balance(AccessPointAirtimePeer::Group), balance);
        assert_eq!(
            accounting.reserve_active(engine, group()),
            Err(AccessPointAirtimeError::TransactionBusy)
        );
        accounting.complete_active(work(1)).unwrap();
        assert_eq!(
            accounting.balance(AccessPointAirtimePeer::Group),
            Some(1400)
        );
    });
}

#[test]
fn unknown_terminal_cost_keeps_receipt_and_prevents_new_publication() {
    with_authorized_ap(|engine| {
        let mut storage = AccessPointAirtimeStorage::new(us(1000));
        let mut accounting = Accounting::new(&mut storage, us(100), |_, _| None);
        accounting.reserve_active(engine, group()).unwrap();
        accounting.publish_active();
        assert_eq!(
            accounting.complete_active(work(2)),
            Err(AccessPointAirtimeError::UnpricedWork)
        );
        accounting.cancel_active_preparation().unwrap();
        assert_eq!(accounting.balance(AccessPointAirtimePeer::Group), Some(0));
        assert_eq!(
            accounting.unresolved_work(),
            Some((AccessPointAirtimePeer::Group, work(2)))
        );
        assert_eq!(
            accounting.reserve_active(engine, group()),
            Err(AccessPointAirtimeError::TransactionBusy)
        );
    });
}

#[test]
fn ordinary_rejection_refunds_but_publication_requires_terminal_receipt() {
    with_authorized_ap(|engine| {
        let mut storage = AccessPointAirtimeStorage::new(us(1000));
        let mut packets = AccessPointTxStorage::<()>::new();
        let mut tx = AccessPointNetworkTx::<
            PinnedTxFrame<'_, embassy_sync::blocking_mutex::raw::NoopRawMutex, 64, 16, 8, 1>,
            (),
        >::new_with_airtime_accounting(
            &mut packets, None, &mut storage, us(100), tariff
        );
        tx.airtime
            .as_mut()
            .unwrap()
            .reserve_active(engine, group())
            .unwrap();
        assert_eq!(
            tx.ordinary_airtime_result(Ok(WifiTxProgress::Complete)),
            Ok(WifiTxProgress::Complete)
        );
        assert_eq!(
            tx.airtime_balance_micros(AccessPointAirtimePeer::Group),
            Some(1000)
        );
        tx.airtime
            .as_mut()
            .unwrap()
            .reserve_active(engine, group())
            .unwrap();
        assert_eq!(
            tx.ordinary_airtime_result(Ok(WifiTxProgress::Pending)),
            Ok(WifiTxProgress::Pending)
        );
        tx.airtime
            .as_mut()
            .unwrap()
            .cancel_active_preparation()
            .unwrap();
        assert_eq!(
            tx.airtime_balance_micros(AccessPointAirtimePeer::Group),
            Some(0)
        );
        tx.airtime
            .as_mut()
            .unwrap()
            .complete_active(work(1))
            .unwrap();
        assert_eq!(
            tx.airtime_balance_micros(AccessPointAirtimePeer::Group),
            Some(400)
        );
    });
}

#[test]
fn unicast_and_group_use_independent_accounts() {
    with_authorized_ap(|engine| {
        let mut storage = AccessPointAirtimeStorage::new(us(1000));
        let mut accounting = Accounting::new(&mut storage, us(100), tariff);
        let identity = engine.admit_downlink([4; 6]).unwrap().identity();
        accounting
            .reserve_active(engine, ApTxFlowKey::associated(identity))
            .unwrap();
        accounting.publish_active();
        accounting.reserve_standby(engine, group()).unwrap();
        accounting.complete_active(work(2)).unwrap();
        assert_eq!(
            accounting.balance(AccessPointAirtimePeer::Unicast(identity)),
            Some(-200)
        );
        assert_eq!(accounting.balance(AccessPointAirtimePeer::Group), Some(0));
        accounting.cancel_standby().unwrap();
        assert_eq!(
            accounting.balance(AccessPointAirtimePeer::Group),
            Some(1000)
        );
    });
}

mod hardware;

mod runtime;

mod admission;

mod peers;
mod selection;
