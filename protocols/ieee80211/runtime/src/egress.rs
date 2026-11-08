//! The airtime policy of a network's egress over one radio.
//!
//! Peers share the air by [`airtime`](oer_protocols_ieee80211_core_upper_mac::airtime)
//! deficits, keyed by peer and never by flow, so a peer's flows take turns within
//! its share. What the radio knows of a peer stays behind [`PeerAir`], and what
//! the air charged or which association ended comes back in a [`RadioNotice`]:
//! the network sees neither.

use core::num::NonZeroU32;

use oer_protocols_ieee80211_core_upper_mac::airtime::{
    AirtimeCandidate, AirtimeError, AirtimeInFlight, AirtimeReservation, AirtimeScheduler,
    AirtimeStorage,
};
use oer_protocols_network_port::{Candidate, Class, Egress, Ready};

/// What the radio knows of the peers.
pub trait PeerAir {
    /// An association: a peer with its generation, never a flow.
    type Key: Copy + Eq;

    /// The account `ready` spends and its least useful grant, or `None` while
    /// this work may not go: its peer sleeps or has left.
    fn candidate(&self, ready: &Ready) -> Option<AirtimeCandidate<Self::Key>>;

    /// The most `ready` can cost, every retry at the lowest rate included. A
    /// frame whose cost is not known is charged this, so no peer gains by not
    /// knowing it. It holds only as far as this is a true ceiling.
    fn ceiling(&self, ready: &Ready) -> NonZeroU32;
}

/// How a frame ended, as a radio port's completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Exchange<T> {
    pub ticket: T,
    /// What the air cost, or `None` when that is not known, which charges the
    /// frame's ceiling or its reservation, whichever is more.
    pub charged: Option<NonZeroU32>,
}

/// What a radio port tells the policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioNotice<T, K> {
    /// A frame the port took has ended.
    Ended(Exchange<T>),
    /// An association has ended. Each of its frames still at the port
    /// settles here, once, at its ceiling or its reservation if that is
    /// more, and its account goes at once. The frames' buffers stay the
    /// port's until they come back; each one's later [`Self::Ended`] only
    /// closes its ticket, with no second charge.
    Left(K),
    /// The port took the frame and lost it with its memory before the
    /// radio's end of it: no completion follows, and whether it reached the
    /// air is not known. Its reservation settles here at its ceiling, or its
    /// reservation if that is more. This settles the account only: it
    /// returns no memory and permits no reuse of the lost buffer.
    Lost(T),
}

/// Why the policy cannot go on. The scheduler keeps a charge it could not
/// settle, so nothing is refunded by a fault.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AirtimeFault {
    Airtime(AirtimeError),
    /// More frames in flight than reservations: an accepted frame has no place.
    NoPlace,
    /// A completion names no frame in flight.
    UnknownTicket,
}

impl From<AirtimeError> for AirtimeFault {
    fn from(error: AirtimeError) -> Self {
        Self::Airtime(error)
    }
}

/// A choice until the port answers it.
#[derive(Debug)]
pub struct Held<'o, K> {
    reservation: AirtimeReservation<'o, K>,
    flow: Option<u8>,
    ceiling: NonZeroU32,
}

/// A frame the port holds.
enum Flight<'o, T, K> {
    /// Its reservation is open, for its completion to settle.
    Open {
        ticket: T,
        flight: AirtimeInFlight<'o, K>,
        ceiling: NonZeroU32,
    },
    /// Its association left and its reservation settled at its ceiling: only
    /// its buffer's return is owed, which closes the ticket and charges
    /// nothing.
    Closed { ticket: T },
}

impl<T, K> Flight<'_, T, K> {
    const fn ticket(&self) -> &T {
        match self {
            Self::Open { ticket, .. } | Self::Closed { ticket } => ticket,
        }
    }
}

/// Airtime between peers, turns between one peer's flows, and control first.
pub struct AirtimeEgress<'o, A: PeerAir, T, const PEERS: usize, const RESERVATIONS: usize> {
    scheduler: AirtimeScheduler<'o, A::Key, PEERS, RESERVATIONS>,
    air: A,
    flights: [Option<Flight<'o, T, A::Key>>; RESERVATIONS],
    /// Each peer's last served flow.
    flows: [Option<(A::Key, u8)>; PEERS],
}

impl<'o, A: PeerAir, T, const PEERS: usize, const RESERVATIONS: usize>
    AirtimeEgress<'o, A, T, PEERS, RESERVATIONS>
{
    pub fn new(storage: &'o mut AirtimeStorage<A::Key, PEERS, RESERVATIONS>, air: A) -> Self {
        Self {
            scheduler: storage.scheduler(),
            air,
            flights: [const { None }; RESERVATIONS],
            flows: [const { None }; PEERS],
        }
    }

    pub const fn air(&self) -> &A {
        &self.air
    }

    pub const fn air_mut(&mut self) -> &mut A {
        &mut self.air
    }

    /// A peer's modelled credit; negative is debt.
    pub fn balance_micros(&self, key: A::Key) -> Option<i64> {
        self.scheduler.balance_micros(key)
    }

    /// Frames the port still holds, settled or not: one per buffer it has
    /// not given back.
    pub fn in_flight(&self) -> usize {
        self.flights.iter().flatten().count()
    }

    /// Frames whose reservation is still open.
    pub fn unsettled(&self) -> usize {
        self.flights
            .iter()
            .flatten()
            .filter(|flight| matches!(flight, Flight::Open { .. }))
            .count()
    }

    /// End an association: each of its frames still at the port settles now
    /// at its ceiling, or its reservation if that is more, and the account
    /// goes. The frames' tickets stay until their buffers come back.
    ///
    /// # Errors
    /// [`AirtimeFault`]: a settlement the ledger refused.
    pub fn retire(&mut self, key: A::Key) -> Result<(), AirtimeFault> {
        for slot in &mut self.flights {
            let closes = matches!(slot, Some(Flight::Open { flight, .. }) if flight.key() == key);
            if !closes {
                continue;
            }
            let Some(Flight::Open {
                ticket,
                flight,
                ceiling,
            }) = slot.take()
            else {
                continue;
            };
            *slot = Some(Flight::Closed { ticket });
            let charged = ceiling.max(flight.reserved_micros());
            self.scheduler
                .settle(flight.completed(()), charged, &mut ())
                .map_err(|(error, _)| AirtimeFault::from(error))?;
        }
        self.scheduler.retire(key);
        for slot in &mut self.flows {
            if slot.is_some_and(|(peer, _)| peer == key) {
                *slot = None;
            }
        }
        Ok(())
    }

    fn last_flow(&self, key: A::Key) -> Option<u8> {
        self.flows
            .iter()
            .flatten()
            .find(|(peer, _)| *peer == key)
            .map(|(_, flow)| *flow)
    }

    fn remember(&mut self, key: A::Key, flow: u8) {
        let place = self
            .flows
            .iter()
            .position(|slot| slot.is_some_and(|(peer, _)| peer == key))
            .or_else(|| self.flows.iter().position(Option::is_none))
            .and_then(|place| self.flows.get_mut(place));
        if let Some(slot) = place {
            *slot = Some((key, flow));
        }
    }

    /// The work of `key` next in turn: control first, then the flow after the
    /// last served one.
    fn pick<C: Candidate>(&self, ready: impl Iterator<Item = C>, key: A::Key) -> Option<C> {
        let last = self.last_flow(key);
        ready
            .filter(|work| {
                self.air
                    .candidate(work.ready())
                    .is_some_and(|candidate| candidate.key == key)
            })
            .min_by_key(|work| turn(work.ready(), last))
    }

    /// Hold `flight` for its completion.
    fn fly(&mut self, flight: Flight<'o, T, A::Key>) -> Result<(), AirtimeFault> {
        let slot = self
            .flights
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(AirtimeFault::NoPlace)?;
        *slot = Some(flight);
        Ok(())
    }

    /// Settle `flight` at `charged`.
    fn settle(
        &mut self,
        flight: AirtimeInFlight<'o, A::Key>,
        charged: NonZeroU32,
    ) -> Result<(), AirtimeFault> {
        self.scheduler
            .settle(flight.completed(()), charged, &mut ())
            .map_err(|(error, _)| error.into())
            .map(|()| ())
    }
}

/// How far `ready` stands from its turn after flow `last`.
fn turn(ready: &Ready, last: Option<u8>) -> (bool, u8) {
    let flow = ready.flow.unwrap_or(0);
    let distance = last.map_or(flow, |last| flow.wrapping_sub(last).wrapping_sub(1));
    (ready.class != Class::Control, distance)
}

impl<'o, A: PeerAir, T: Eq, const PEERS: usize, const RESERVATIONS: usize>
    Egress<T, RadioNotice<T, A::Key>> for AirtimeEgress<'o, A, T, PEERS, RESERVATIONS>
{
    type Reservation = Held<'o, A::Key>;
    type Fault = AirtimeFault;

    fn choose<C: Candidate>(
        &mut self,
        ready: impl Iterator<Item = C> + Clone,
    ) -> Result<Option<(C, Self::Reservation)>, AirtimeFault> {
        let air = &self.air;
        let candidates = ready.clone().filter_map(|work| air.candidate(work.ready()));
        let reservation = match self.scheduler.reserve(candidates, &mut ()) {
            Ok(reservation) => reservation,
            // A frame's completion makes room, and it wakes the service.
            Err(AirtimeError::ReservationCapacity) => None,
            Err(error) => return Err(error.into()),
        };
        let Some(reservation) = reservation else {
            return Ok(None);
        };
        match self.pick(ready, reservation.key()) {
            Some(work) => {
                let flow = work.ready().flow;
                let ceiling = self.air.ceiling(work.ready());
                let held = Held {
                    reservation,
                    flow,
                    ceiling,
                };
                Ok(Some((work, held)))
            }
            // The candidates were this work, so one of them holds the key.
            None => match self.scheduler.cancel(reservation, &mut ()) {
                Ok(()) => Ok(None),
                Err((error, _)) => Err(error.into()),
            },
        }
    }

    fn accepted(&mut self, held: Held<'o, A::Key>, ticket: T) -> Result<(), AirtimeFault> {
        let Held {
            reservation,
            flow,
            ceiling,
        } = held;
        if let Some(flow) = flow {
            self.remember(reservation.key(), flow);
        }
        self.fly(Flight::Open {
            ticket,
            flight: reservation.published(),
            ceiling,
        })
    }

    fn refused(&mut self, held: Held<'o, A::Key>) -> Result<(), AirtimeFault> {
        self.scheduler
            .cancel(held.reservation, &mut ())
            .map_err(|(error, _)| error.into())
    }

    fn unknown(&mut self, held: Held<'o, A::Key>) -> Result<(), AirtimeFault> {
        let flight = held.reservation.published();
        let charged = held.ceiling.max(flight.reserved_micros());
        self.settle(flight, charged)
    }

    fn notice(&mut self, notice: RadioNotice<T, A::Key>) -> Result<(), AirtimeFault> {
        let exchange = match notice {
            RadioNotice::Ended(exchange) => exchange,
            RadioNotice::Lost(ticket) => Exchange {
                ticket,
                charged: None,
            },
            RadioNotice::Left(key) => return self.retire(key),
        };
        let flight = self
            .flights
            .iter_mut()
            .find(|slot| {
                slot.as_ref()
                    .is_some_and(|flight| *flight.ticket() == exchange.ticket)
            })
            .and_then(Option::take);
        match flight {
            Some(Flight::Open {
                flight, ceiling, ..
            }) => {
                let charged = exchange
                    .charged
                    .unwrap_or_else(|| ceiling.max(flight.reserved_micros()));
                self.settle(flight, charged)
            }
            // Settled when its association left: its return closes it.
            Some(Flight::Closed { .. }) => Ok(()),
            None => Err(AirtimeFault::UnknownTicket),
        }
    }
}

#[cfg(test)]
mod tests;
