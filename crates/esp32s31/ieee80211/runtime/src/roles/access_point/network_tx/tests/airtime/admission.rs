//! Actual AP prefix admission from active/standby grants and explicit PHY assumptions.

use core::pin::pin;

use crate::datapath::PinnedTxFrame;

use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use oer_esp32s31_ieee80211_ap::ampdu::{ApAmpduBudget, ApAmpduTx};

use oer_esp32s31_ieee80211_mac::tx::{
    HtChannelWidth, HtGuardInterval, HtMcs, HtRate, LegacyRate, TxPhyRate,
    ampdu::{HtAmpduTxResources, HtAmpduTxStorage, RetainedAmpduDmaStorage},
};

use oer_ieee80211_softmac::tx_cost::PpduTiming;

use super::{super::super::AccessPointDatapathError, *};

fn block_ack(_: AccessPointAirtimePeer) -> Option<PpduTiming> {
    TxPhyRate::Legacy(LegacyRate::Ofdm24M).ppdu_timing()
}
fn rate(mcs: HtMcs) -> HtRate {
    HtRate::new(mcs, HtGuardInterval::Long800Ns, HtChannelWidth::Mhz20)
}
fn with_budget(rate: HtRate, test: impl FnOnce(ApAmpduBudget)) {
    let mut metadata = pin!(HtAmpduTxStorage::<32, 0>::new());
    let mut retention =
        RetainedAmpduDmaStorage::<PinnedTxFrame<'_, NoopRawMutex, 1514, 64, 16, 32>, 32>::new();
    let ampdu = ApAmpduTx::new(
        HtAmpduTxResources::new_model(metadata.as_mut()).unwrap(),
        &mut retention,
        u16::MAX,
    )
    .unwrap();
    test(ampdu.length_budget(rate).unwrap());
}
fn count(mut budget: ApAmpduBudget) -> usize {
    (0..32)
        .take_while(|_| budget.admit_ethernet_len(1514).unwrap())
        .count()
}

#[test]
fn a_slower_phy_fits_fewer_frames_in_the_same_grant() {
    with_authorized_ap(|engine| {
        let mut storage = AccessPointAirtimeStorage::new(us(5000));
        let mut accounting =
            Accounting::new(&mut storage, us(100), tariff).with_aggregate_admission(block_ack);
        accounting.reserve_active(engine, group()).unwrap();
        let mut frames = [0; 2];
        for (index, rate) in [rate(HtMcs::Mcs0), rate(HtMcs::Mcs7)]
            .into_iter()
            .enumerate()
        {
            with_budget(rate, |mut budget| {
                accounting.cap_active_aggregate(rate, &mut budget).unwrap();
                frames[index] = count(budget);
            });
        }
        assert!(frames[0] >= 2 && frames[1] > frames[0] * 4, "{frames:?}");
        accounting.cancel_active_preparation().unwrap();
    });
}

#[test]
fn standby_uses_its_own_grant_after_active_completion_changes_balance() {
    with_authorized_ap(|engine| {
        let mut storage = AccessPointAirtimeStorage::new(us(5000));
        let mut accounting =
            Accounting::new(&mut storage, us(100), tariff).with_aggregate_admission(block_ack);
        accounting.reserve_active(engine, group()).unwrap();
        accounting.publish_active();
        accounting.reserve_standby(engine, group()).unwrap();
        let rate = rate(HtMcs::Mcs0);
        let mut before = 0;
        with_budget(rate, |mut budget| {
            accounting.cap_standby_aggregate(rate, &mut budget).unwrap();
            before = count(budget);
        });
        accounting.complete_active(work(20)).unwrap();
        assert!(accounting.balance(AccessPointAirtimePeer::Group).unwrap() < 0);
        with_budget(rate, |mut budget| {
            accounting.cap_standby_aggregate(rate, &mut budget).unwrap();
            assert_eq!(
                count(budget),
                before,
                "the published exchange cannot spend the successor's grant"
            );
        });
        accounting.cancel_standby().unwrap();
    });
}

#[test]
fn unknown_response_is_an_error_not_unlimited_admission() {
    with_authorized_ap(|engine| {
        let mut storage = AccessPointAirtimeStorage::new(us(5000));
        let mut accounting =
            Accounting::new(&mut storage, us(100), tariff).with_aggregate_admission(|_| None);
        accounting.reserve_active(engine, group()).unwrap();
        with_budget(rate(HtMcs::Mcs7), |mut budget| {
            assert_eq!(
                accounting.cap_active_aggregate(rate(HtMcs::Mcs7), &mut budget),
                Err(AccessPointDatapathError::Airtime(
                    AccessPointAirtimeError::UnpricedAggregate
                ))
            );
            assert!(
                budget.admit_ethernet_len(1514).unwrap(),
                "failure leaves the prospective prefix untouched"
            );
        });
        accounting.cancel_active_preparation().unwrap();
        assert_eq!(
            accounting.balance(AccessPointAirtimePeer::Group),
            Some(5000)
        );
    });
}

#[test]
fn published_grant_cannot_authorize_a_new_initial_aggregate() {
    with_authorized_ap(|engine| {
        let mut storage = AccessPointAirtimeStorage::new(us(5000));
        let mut accounting =
            Accounting::new(&mut storage, us(100), tariff).with_aggregate_admission(block_ack);
        accounting.reserve_active(engine, group()).unwrap();
        accounting.publish_active();
        with_budget(rate(HtMcs::Mcs7), |mut budget| {
            assert_eq!(
                accounting.cap_active_aggregate(rate(HtMcs::Mcs7), &mut budget),
                Err(AccessPointDatapathError::Airtime(
                    AccessPointAirtimeError::TransactionBusy
                ))
            );
        });
        accounting.complete_active(work(1)).unwrap();
    });
}

#[test]
fn too_small_grant_closes_ht_admission_instead_of_waiting_for_time() {
    with_authorized_ap(|engine| {
        let mut storage = AccessPointAirtimeStorage::new(us(1));
        let mut accounting =
            Accounting::new(&mut storage, us(1), tariff).with_aggregate_admission(block_ack);
        accounting.reserve_active(engine, group()).unwrap();
        with_budget(rate(HtMcs::Mcs7), |mut budget| {
            accounting
                .cap_active_aggregate(rate(HtMcs::Mcs7), &mut budget)
                .unwrap();
            assert_eq!(count(budget), 0);
        });
        accounting.cancel_active_preparation().unwrap();
    });
}
