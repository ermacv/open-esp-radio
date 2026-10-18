//! AP transaction ownership of an optional airtime ledger.
//!
//! Selection is optionally driven by deficits. An explicit caller model prices terminal
//! work; this module never substitutes a guessed charge for an unknown cost.

use core::num::NonZeroU32;

use oer_esp32s31_ieee80211_ap::ampdu::ApAmpduBudget;

use oer_esp32s31_ieee80211_mac::tx::HtRate;

use super::*;

pub use oer_ieee80211_datapath::airtime::{AirtimeAction, AirtimeObservation};

use oer_ieee80211_datapath::airtime::{
    AirtimeCandidate, AirtimeCompletion, AirtimeError, AirtimeInFlight, AirtimeReservation,
    AirtimeScheduler, AirtimeStorage,
};

use oer_ieee80211_softmac::{MacTxWork, tx_cost::PpduTiming};

/// Group traffic has one BSS account, separate from associated unicast peers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointAirtimePeer {
    Unicast(ApAssociationIdentity),
    Group,
}

/// Current peers, one group account and two overlapping retired generations.
pub type AccessPointAirtimeStorage =
    AirtimeStorage<AccessPointAirtimePeer, { AP_MAX_CLIENTS + 3 }, 2>;

/// An explicit model, not a claim of measured airtime. `None` retains the
/// completion and fails closed. The callback runs after hardware detach.
pub type AccessPointAirtimeCost = fn(AccessPointAirtimePeer, &MacTxWork) -> Option<NonZeroU32>;

/// Explicit compressed-BlockAck PHY assumption for prospective HT admission.
/// Return `None` when unknown. The model must remain stable for a reservation;
/// it is not an observation of the peer's actual response or internal retries.
pub type AccessPointBlockAckTiming = fn(AccessPointAirtimePeer) -> Option<PpduTiming>;

/// Keep model and HT admission identical while comparing destination policies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointAirtimeSelection {
    RoundRobin,
    Deficit,
}

/// Caller-supplied model for a standalone AP epoch. Storage remains with the
/// board owner, including after an unresolved terminal or detach failure.
#[derive(Clone, Copy)]
pub struct AccessPointAirtimeConfiguration {
    pub selection: AccessPointAirtimeSelection,
    pub quantum_micros: NonZeroU32,
    pub minimum_exchange_micros: NonZeroU32,
    pub cost: AccessPointAirtimeCost,
    pub block_ack_timing: AccessPointBlockAckTiming,
    /// Optional bounded observation of successful ledger transitions.
    pub observer: Option<fn(AirtimeObservation<AccessPointAirtimePeer>)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointAirtimeError {
    Ledger(AirtimeError),
    TransactionBusy,
    UnpricedWork,
    UnpricedAggregate,
    SelectedPeerChanged,
    SelectionStorageFull,
    DestinationQueuesRequired,
}

impl From<AccessPointAirtimeError> for AccessPointDatapathError {
    fn from(error: AccessPointAirtimeError) -> Self {
        Self::Airtime(error)
    }
}

type Reservation<'a> = AirtimeReservation<'a, AccessPointAirtimePeer>;
type Completion<'a> = AirtimeCompletion<'a, AccessPointAirtimePeer, MacTxWork>;

enum ActiveCharge<'a> {
    Preparing(Reservation<'a>),
    Published(AirtimeInFlight<'a, AccessPointAirtimePeer>),
    Completed(Completion<'a>),
}

struct SelectedCharge<'a> {
    key: ApTxFlowKey,
    reservation: Reservation<'a>,
}

enum NextCharge<'a> {
    Selected(SelectedCharge<'a>),
    Standby(SelectedCharge<'a>),
}

pub(super) struct Accounting<'a> {
    scheduler: AirtimeScheduler<'a, AccessPointAirtimePeer, { AP_MAX_CLIENTS + 3 }, 2>,
    minimum: NonZeroU32,
    cost: AccessPointAirtimeCost,
    block_ack_timing: Option<AccessPointBlockAckTiming>,
    active: Option<ActiveCharge<'a>>,
    next: Option<NextCharge<'a>>,
    peer_selection: bool,
}

impl<'a> Accounting<'a> {
    pub(super) fn new(
        storage: &'a mut AccessPointAirtimeStorage,
        minimum: NonZeroU32,
        cost: AccessPointAirtimeCost,
    ) -> Self {
        Self {
            scheduler: storage.scheduler(),
            minimum,
            cost,
            block_ack_timing: None,
            active: None,
            next: None,
            peer_selection: false,
        }
    }

    pub(super) fn with_aggregate_admission(mut self, timing: AccessPointBlockAckTiming) -> Self {
        self.block_ack_timing = Some(timing);
        self
    }

    pub(super) fn with_peer_selection(mut self) -> Self {
        self.peer_selection = true;
        self
    }

    pub(super) fn selects_peers(&self) -> bool {
        self.peer_selection
    }

    pub(super) fn needs_frontier_selection(&self) -> bool {
        self.peer_selection && self.next.is_none()
    }

    /// Reserve against all eligible peers before removing the winning head.
    /// The caller has already coalesced each peer's oldest ready source.
    pub(super) fn select_candidates(
        &mut self,
        engine: &ApEngine<'_>,
        keys: impl Iterator<Item = ApTxFlowKey> + Clone,
    ) -> Result<Option<ApTxFlowKey>, AccessPointAirtimeError> {
        self.require_selection_idle()?;
        self.scheduler.retire_where(|peer| match peer {
            AccessPointAirtimePeer::Unicast(identity) => !engine.association_is_current(identity),
            AccessPointAirtimePeer::Group => false,
        });
        let reservation = self
            .scheduler
            .reserve(keys.clone().filter_map(|key| {
                Self::peer_for_key(engine, key).map(|peer| AirtimeCandidate {
                    key: peer,
                    minimum_micros: self.minimum,
                })
            }))
            .map_err(AccessPointAirtimeError::Ledger)?;
        let Some(reservation) = reservation else {
            return Ok(None);
        };
        let key = keys
            .clone()
            .find(|key| Self::peer_for_key(engine, *key) == Some(reservation.key()))
            .expect("the reserved account belongs to an eligible head");
        self.next = Some(NextCharge::Selected(SelectedCharge { key, reservation }));
        Ok(Some(key))
    }

    pub(super) fn peer_for_key(
        engine: &ApEngine<'_>,
        key: ApTxFlowKey,
    ) -> Option<AccessPointAirtimePeer> {
        if let Some(identity) = key.association() {
            engine
                .association_is_current(identity)
                .then_some(AccessPointAirtimePeer::Unicast(identity))
        } else {
            (key.destination[0] & 1 != 0).then_some(AccessPointAirtimePeer::Group)
        }
    }

    fn cap_aggregate(
        &self,
        reservation: Option<&Reservation<'_>>,
        rate: HtRate,
        budget: &mut ApAmpduBudget,
    ) -> Result<(), AccessPointDatapathError> {
        let Some(timing) = self.block_ack_timing else {
            return Ok(());
        };
        let reservation = reservation.ok_or(AccessPointAirtimeError::TransactionBusy)?;
        let response =
            timing(reservation.key()).ok_or(AccessPointAirtimeError::UnpricedAggregate)?;
        let maximum = rate
            .ampdu_exchange_byte_limit(reservation.budget_micros().get(), response, u16::MAX)
            .map_or(0, core::num::NonZeroU16::get);
        budget
            .cap_bytes(maximum)
            .map_err(AccessPointDatapathError::Aggregate)
    }

    pub(super) fn cap_active_aggregate(
        &self,
        rate: HtRate,
        budget: &mut ApAmpduBudget,
    ) -> Result<(), AccessPointDatapathError> {
        let reservation = match &self.active {
            Some(ActiveCharge::Preparing(reservation)) => Some(reservation),
            _ => None,
        };
        self.cap_aggregate(reservation, rate, budget)
    }

    pub(super) fn cap_standby_aggregate(
        &self,
        rate: HtRate,
        budget: &mut ApAmpduBudget,
    ) -> Result<(), AccessPointDatapathError> {
        let reservation = match &self.next {
            Some(NextCharge::Standby(charge)) => Some(&charge.reservation),
            _ => None,
        };
        self.cap_aggregate(reservation, rate, budget)
    }

    fn reserve(
        &mut self,
        engine: &ApEngine<'_>,
        key: ApTxFlowKey,
    ) -> Result<Option<Reservation<'a>>, AccessPointAirtimeError> {
        self.scheduler.retire_where(|peer| match peer {
            AccessPointAirtimePeer::Unicast(identity) => !engine.association_is_current(identity),
            AccessPointAirtimePeer::Group => false,
        });
        let peer = if let Some(identity) = key.association() {
            if !engine.association_is_current(identity) {
                return Ok(None);
            }
            AccessPointAirtimePeer::Unicast(identity)
        } else if key.destination[0] & 1 != 0 {
            AccessPointAirtimePeer::Group
        } else {
            // Unknown/invalid unicast still follows ordinary protocol rejection.
            return Ok(None);
        };
        self.scheduler
            .reserve([AirtimeCandidate {
                key: peer,
                minimum_micros: self.minimum,
            }])
            .map_err(AccessPointAirtimeError::Ledger)
    }

    /// Adopt the selected head's grant, or reserve a direct/protocol publication.
    pub(super) fn reserve_active(
        &mut self,
        engine: &ApEngine<'_>,
        key: ApTxFlowKey,
    ) -> Result<(), AccessPointAirtimeError> {
        if self.active.is_some() {
            return Err(AccessPointAirtimeError::TransactionBusy);
        }
        let reservation = if matches!(self.next, Some(NextCharge::Selected(_))) {
            self.require_selected_key(key)?;
            let Some(NextCharge::Selected(charge)) = self.next.take() else {
                unreachable!()
            };
            Some(charge.reservation)
        } else {
            self.reserve(engine, key)?
        };
        self.active = reservation.map(ActiveCharge::Preparing);
        Ok(())
    }

    pub(super) fn require_selection_idle(&self) -> Result<(), AccessPointAirtimeError> {
        if self.next.is_some() || matches!(self.active, Some(ActiveCharge::Completed(_))) {
            Err(AccessPointAirtimeError::TransactionBusy)
        } else {
            Ok(())
        }
    }

    /// Bind the radio-admitted head once, before choosing its physical form.
    pub(super) fn select(
        &mut self,
        engine: &ApEngine<'_>,
        key: ApTxFlowKey,
    ) -> Result<(), AccessPointAirtimeError> {
        self.require_selection_idle()?;
        self.next = self
            .reserve(engine, key)?
            .map(|reservation| NextCharge::Selected(SelectedCharge { key, reservation }));
        Ok(())
    }

    fn require_selected_key(&self, key: ApTxFlowKey) -> Result<(), AccessPointAirtimeError> {
        match &self.next {
            Some(NextCharge::Selected(charge)) if charge.key == key => Ok(()),
            _ => Err(AccessPointAirtimeError::SelectedPeerChanged),
        }
    }

    /// Adopt the selected head's grant, or reserve an already retained frontier.
    pub(super) fn reserve_standby(
        &mut self,
        engine: &ApEngine<'_>,
        key: ApTxFlowKey,
    ) -> Result<(), AccessPointAirtimeError> {
        match &self.next {
            Some(NextCharge::Standby(_)) => return Err(AccessPointAirtimeError::TransactionBusy),
            Some(NextCharge::Selected(_)) => {
                self.require_selected_key(key)?;
                let Some(NextCharge::Selected(charge)) = self.next.take() else {
                    unreachable!()
                };
                self.next = Some(NextCharge::Standby(charge));
            }
            None => {
                self.next = self
                    .reserve(engine, key)?
                    .map(|reservation| NextCharge::Standby(SelectedCharge { key, reservation }));
            }
        }
        Ok(())
    }

    /// The initial pair did not fit; the same selected head now uses ordinary TX.
    /// This is only valid before physical standby construction begins.
    pub(super) fn defer_standby(&mut self) -> Result<(), AccessPointAirtimeError> {
        match self.next.take() {
            Some(NextCharge::Standby(charge)) => self.next = Some(NextCharge::Selected(charge)),
            None => {}
            other => {
                self.next = other;
                return Err(AccessPointAirtimeError::TransactionBusy);
            }
        }
        Ok(())
    }

    pub(super) fn publish_active(&mut self) {
        if let Some(charge) = self.active.take() {
            self.active = Some(match charge {
                ActiveCharge::Preparing(reservation) => {
                    ActiveCharge::Published(reservation.published())
                }
                _ => panic!("initial publication must own one unpublished airtime reservation"),
            });
        }
    }

    pub(super) fn require_active_idle(&self) -> Result<(), AccessPointAirtimeError> {
        if self.active.is_some() {
            Err(AccessPointAirtimeError::TransactionBusy)
        } else {
            Ok(())
        }
    }

    pub(super) fn publish_standby(&mut self) {
        assert!(
            self.active.is_none(),
            "standby publication requires terminal active accounting"
        );
        self.active = match self.next.take() {
            Some(NextCharge::Standby(charge)) => {
                Some(ActiveCharge::Published(charge.reservation.published()))
            }
            None => None,
            Some(NextCharge::Selected(_)) => {
                panic!("standby publication requires a built successor")
            }
        };
    }

    pub(super) fn cancel_active_preparation(&mut self) -> Result<(), AccessPointAirtimeError> {
        let Some(charge) = self.active.take() else {
            return Ok(());
        };
        match charge {
            ActiveCharge::Preparing(reservation) => match self.scheduler.cancel(reservation) {
                Ok(()) => Ok(()),
                Err((error, reservation)) => {
                    self.active = Some(ActiveCharge::Preparing(reservation));
                    Err(AccessPointAirtimeError::Ledger(error))
                }
            },
            // A service failure or cancelled wait is not terminal detach.
            _ => {
                self.active = Some(charge);
                Ok(())
            }
        }
    }

    pub(super) fn cancel_standby(&mut self) -> Result<(), AccessPointAirtimeError> {
        self.cancel_next_if(|charge| matches!(charge, NextCharge::Standby(_)))
    }

    pub(super) fn cancel_selection(&mut self) -> Result<(), AccessPointAirtimeError> {
        self.cancel_next_if(|charge| matches!(charge, NextCharge::Selected(_)))
    }

    pub(super) fn cancel_selection_for(
        &mut self,
        key: ApTxFlowKey,
    ) -> Result<(), AccessPointAirtimeError> {
        self.cancel_next_if(
            |charge| matches!(charge, NextCharge::Selected(selected) if selected.key == key),
        )
    }

    fn cancel_next_if(
        &mut self,
        cancel: impl FnOnce(&NextCharge<'_>) -> bool,
    ) -> Result<(), AccessPointAirtimeError> {
        if !self.next.as_ref().is_some_and(cancel) {
            return Ok(());
        }
        let next = self.next.take().expect("checked next charge");
        let (standby, charge) = match next {
            NextCharge::Selected(charge) => (false, charge),
            NextCharge::Standby(charge) => (true, charge),
        };
        match self.scheduler.cancel(charge.reservation) {
            Ok(()) => Ok(()),
            Err((error, reservation)) => {
                let charge = SelectedCharge {
                    key: charge.key,
                    reservation,
                };
                self.next = Some(if standby {
                    NextCharge::Standby(charge)
                } else {
                    NextCharge::Selected(charge)
                });
                Err(AccessPointAirtimeError::Ledger(error))
            }
        }
    }

    pub(super) fn complete_active(
        &mut self,
        work: MacTxWork,
    ) -> Result<(), AccessPointAirtimeError> {
        let Some(charge) = self.active.take() else {
            return Ok(());
        };
        let completion = match charge {
            ActiveCharge::Published(publication) => publication.completed(work),
            _ => {
                self.active = Some(charge);
                return Err(AccessPointAirtimeError::TransactionBusy);
            }
        };
        let Some(cost) = (self.cost)(completion.key(), completion.work()) else {
            self.active = Some(ActiveCharge::Completed(completion));
            return Err(AccessPointAirtimeError::UnpricedWork);
        };
        match self.scheduler.settle(completion, cost) {
            Ok(_) => Ok(()),
            Err((error, completion)) => {
                self.active = Some(ActiveCharge::Completed(completion));
                Err(AccessPointAirtimeError::Ledger(error))
            }
        }
    }

    pub(super) fn balance(&self, peer: AccessPointAirtimePeer) -> Option<i64> {
        self.scheduler.balance_micros(peer)
    }

    pub(super) fn unresolved_work(&self) -> Option<(AccessPointAirtimePeer, MacTxWork)> {
        match self.active.as_ref() {
            Some(ActiveCharge::Completed(completion)) => {
                Some((completion.key(), *completion.work()))
            }
            _ => None,
        }
    }
}

impl<B: StableDmaBacking, N> AccessPointNetworkTx<'_, B, N> {
    pub(super) fn ordinary_airtime_result(
        &mut self,
        result: Result<WifiTxProgress, AccessPointControlError>,
    ) -> Result<WifiTxProgress, AccessPointDatapathError> {
        if let Some(accounting) = self.airtime.as_mut() {
            match result {
                Ok(WifiTxProgress::Pending) => accounting.publish_active(),
                Ok(WifiTxProgress::Complete) | Err(_) => accounting.cancel_active_preparation()?,
            }
        }
        result.map_err(AccessPointDatapathError::Control)
    }
}
