//! One grant survives selection and the choice of physical publication form.
use super::*;

#[test]
fn selected_heads_fill_two_reservations_without_allocating_on_handoff() {
    with_authorized_ap(|engine| {
        let mut storage = AccessPointAirtimeStorage::new(us(1000));
        let mut accounting = Accounting::new(&mut storage, us(100), tariff);
        accounting.select(engine, group()).unwrap();
        accounting.reserve_active(engine, group()).unwrap();
        accounting.publish_active();
        accounting.select(engine, group()).unwrap();
        accounting.reserve_standby(engine, group()).unwrap();
        assert_eq!(
            accounting.select(engine, group()),
            Err(AccessPointAirtimeError::TransactionBusy)
        );
        accounting.complete_active(work(2)).unwrap();
        assert_eq!(
            accounting.balance(AccessPointAirtimePeer::Group),
            Some(-200)
        );
        // The pair does not fit HT. Keep the successor's original 1000-us grant.
        accounting.defer_standby().unwrap();
        accounting.reserve_active(engine, group()).unwrap();
        accounting.publish_active();
        accounting.complete_active(work(1)).unwrap();
        assert_eq!(accounting.balance(AccessPointAirtimePeer::Group), Some(200));
    });
}

#[test]
fn another_peer_cannot_consume_a_selected_heads_grant() {
    with_authorized_ap(|engine| {
        let mut storage = AccessPointAirtimeStorage::new(us(1000));
        let mut accounting = Accounting::new(&mut storage, us(100), tariff);
        let peer = engine.admit_downlink([4; 6]).unwrap().identity();
        let key = ApTxFlowKey::associated(peer);
        accounting.select(engine, key).unwrap();
        assert_eq!(
            accounting.reserve_active(engine, group()),
            Err(AccessPointAirtimeError::SelectedPeerChanged)
        );
        assert_eq!(
            accounting.reserve_standby(engine, group()),
            Err(AccessPointAirtimeError::SelectedPeerChanged)
        );
        accounting.reserve_active(engine, key).unwrap();
        accounting.publish_active();
        accounting.complete_active(work(1)).unwrap();
        assert_eq!(
            accounting.balance(AccessPointAirtimePeer::Unicast(peer)),
            Some(400)
        );
        assert_eq!(accounting.balance(AccessPointAirtimePeer::Group), None);
    });
}

#[test]
fn stale_generation_cannot_cancel_a_new_selection() {
    with_authorized_ap(|engine| {
        let mut storage = AccessPointAirtimeStorage::new(us(1000));
        let mut accounting = Accounting::new(&mut storage, us(100), tariff);
        let identity = engine.admit_downlink([4; 6]).unwrap().identity();
        let different = super::super::super::ApAssociationIdentity::new(
            identity.address(),
            identity.association_id(),
            identity.association_epoch() + 1,
        )
        .unwrap();
        let key = ApTxFlowKey::associated(identity);
        accounting.select(engine, key).unwrap();
        accounting
            .cancel_selection_for(ApTxFlowKey::associated(different))
            .unwrap();
        assert_eq!(
            accounting.require_selection_idle(),
            Err(AccessPointAirtimeError::TransactionBusy)
        );
        accounting.cancel_selection_for(key).unwrap();
        accounting.require_selection_idle().unwrap();
        assert_eq!(
            accounting.balance(AccessPointAirtimePeer::Unicast(identity)),
            Some(1000)
        );
    });
}

#[test]
fn selection_cancellation_never_refunds_a_built_standby() {
    with_authorized_ap(|engine| {
        let mut storage = AccessPointAirtimeStorage::new(us(1000));
        let mut accounting = Accounting::new(&mut storage, us(100), tariff);
        accounting.select(engine, group()).unwrap();
        accounting.reserve_standby(engine, group()).unwrap();
        accounting.cancel_selection().unwrap();
        assert_eq!(accounting.balance(AccessPointAirtimePeer::Group), Some(0));
        assert_eq!(
            accounting.require_selection_idle(),
            Err(AccessPointAirtimeError::TransactionBusy)
        );
        accounting.cancel_standby().unwrap();
        assert_eq!(
            accounting.balance(AccessPointAirtimePeer::Group),
            Some(1000)
        );
    });
}
