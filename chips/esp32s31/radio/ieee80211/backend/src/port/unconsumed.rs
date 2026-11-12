//! What a sink refused, which the port keeps whole until it is offered
//! again ([`PortSink`]).
//!
//! A terminal waits in the place of its token, in that token's own pool:
//! its ticket's, the gate's, its interface's configure permission's, its
//! pairwise key slot's or its group permission's. Each pool's places are
//! [`radio::Places`] of the pool's size, indexed by the token's
//! [`radio::Place`], which is below that size by its type. A bounded place
//! does not prove that the place is free: each token exists once in an
//! installation and a terminal carries it until its sink takes it, so no
//! two kept terminals name one place, but a put into a taken place returns
//! the item whole. A refused receive unit waits in a place of its own,
//! counted among the caller's held units, of which there are at most
//! [`RX_HELD`]. One sequence orders both: a relative number, below the
//! count of what the store keeps, which leaving renumbers. A re-offer the
//! sink refuses goes back to the place it was taken from.

use super::*;
use oer_protocols_ieee80211_contracts_port::{
    Configured, InstalledGroup, InstalledPairwise, Removed, RemovedKey, TxCompleted,
};

use crate::lower_mac::association::{ACCESS_POINT_PAIRWISE_SLOTS, STATION_PAIRWISE_SLOTS};

/// Every place of the store: the terminal pools' and the units'.
const PLACES: usize =
    TX_TICKETS + 1 + 2 + STATION_PAIRWISE_SLOTS + ACCESS_POINT_PAIRWISE_SLOTS + 1 + 1 + RX_HELD;

const _: () = {
    if PLACES > u8::MAX as usize {
        panic!("a kept item's sequence fits its byte");
    }
};

/// One kept item, as the sink gets it.
pub(super) enum Item<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations> {
    Terminal(Terminal<'id, Id, D, B, O>),
    Received(ReceiveBuffer<'id, Id, D::Receive>, Esp32s31RxMeta),
}

/// The right to hand out one item in a port call: the call's one place
/// for an item it could neither give its sink nor keep is empty while the
/// right exists. A producer obtains it before it takes the owner an item
/// carries ([`CallPlace::vacancy`]); without it the owner stays where it
/// is, with the driver or the caller. Handing out the item consumes the
/// right: given or kept, it comes back; held, it ends in the place.
///
/// It is created only where the place is known empty: once with the empty
/// place ([`CallPlace::new`]), and again only by a [`CallPlace::settle`]
/// that emptied the place. It is neither `Clone` nor `Copy`: one exists per
/// place, so a held item is never overwritten and no drop branch exists
/// for a further item.
#[must_use = "a dropped right is the port's permanent fault (`Fault::RightLost`)"]
pub(super) struct Vacancy(());

/// The one place of the port's calls for an item a call could neither give
/// its sink nor keep, and the right to fill it ([`Vacancy`]). The item goes
/// back beside the call's answer ([`Self::settle`]).
pub(super) struct CallPlace<
    'id,
    Id: SlotIdentity + 'id,
    D: Device<B>,
    const B: usize,
    O: Observations,
> {
    item: Option<Item<'id, Id, D, B, O>>,
    vacancy: Option<Vacancy>,
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
    CallPlace<'id, Id, D, B, O>
{
    /// The empty place, with its first right.
    pub(super) const fn new() -> Self {
        Self {
            item: None,
            vacancy: Some(Vacancy(())),
        }
    }

    /// The right to hand out one item, while the place has it: none while
    /// the place holds an item or a producer holds the right.
    pub(super) fn vacancy(&mut self) -> Option<Vacancy> {
        self.vacancy.take()
    }

    /// Give back a right its producer did not use.
    pub(super) fn vacate(&mut self, vacancy: Vacancy) {
        self.vacancy = Some(vacancy);
    }

    /// Hold `item`: the right ends here. The place is empty, as the right
    /// exists only while it is.
    fn hold(&mut self, vacancy: Vacancy, item: Item<'id, Id, D, B, O>) {
        let Vacancy(()) = vacancy;
        self.item = Some(item);
    }

    /// Whether the place lost its right: it holds no item and no right, as
    /// a producer dropped it. No settle makes another.
    pub(super) const fn right_lost(&self) -> bool {
        self.item.is_none() && self.vacancy.is_none()
    }

    /// Whether the call stopped on an item it could not keep: it takes no
    /// further owner, and what it has not taken stays in its place.
    pub(super) const fn halted(&self) -> bool {
        self.item.is_some()
    }

    /// The end of the call: its held item, once. Taking it frees the place,
    /// and only that creates the next right; a place that held nothing
    /// keeps the right its producers gave back.
    pub(super) fn settle(&mut self) -> Option<Item<'id, Id, D, B, O>> {
        let item = self.item.take()?;
        self.vacancy = Some(Vacancy(()));
        Some(item)
    }

    /// Drop the place's right, as a producer that lost it would.
    #[cfg(test)]
    pub(super) fn lose_right_for_test(&mut self) {
        self.vacancy = None;
    }
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
    Item<'id, Id, D, B, O>
{
    /// The item as the contract returns it.
    pub(super) fn into_unkept(
        self,
    ) -> oer_protocols_ieee80211_contracts_port::Unkept<'id, Types<Id, D, B, O>> {
        match self {
            Self::Terminal(terminal) => {
                oer_protocols_ieee80211_contracts_port::Unkept::Terminal(terminal)
            }
            Self::Received(frame, meta) => {
                oer_protocols_ieee80211_contracts_port::Unkept::Received(frame, meta)
            }
        }
    }
}

/// A kept terminal with its sequence.
type KeptTerminal<'id, Id, D, const B: usize, O> = (u8, Terminal<'id, Id, D, B, O>);
/// A kept unit with its sequence.
type KeptUnit<'id, Id, D, const B: usize> = (
    u8,
    ReceiveBuffer<'id, Id, <D as Device<B>>::Receive>,
    Esp32s31RxMeta,
);

/// The place a kept item is in, for writing: a refused re-offer goes back
/// there.
enum Cell<'a, 'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations> {
    Terminal(&'a mut Option<KeptTerminal<'id, Id, D, B, O>>),
    Unit(&'a mut Option<KeptUnit<'id, Id, D, B>>),
}

/// The port's store of what its sink refused: one array of places per
/// token pool, as long as that pool.
pub(super) struct Store<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
{
    tickets: radio::Places<KeptTerminal<'id, Id, D, B, O>, TX_TICKETS>,
    gate: Option<KeptTerminal<'id, Id, D, B, O>>,
    station_configure: Option<KeptTerminal<'id, Id, D, B, O>>,
    access_point_configure: Option<KeptTerminal<'id, Id, D, B, O>>,
    station_pairwise: radio::Places<KeptTerminal<'id, Id, D, B, O>, STATION_PAIRWISE_SLOTS>,
    access_point_pairwise:
        radio::Places<KeptTerminal<'id, Id, D, B, O>, ACCESS_POINT_PAIRWISE_SLOTS>,
    station_group: radio::Places<KeptTerminal<'id, Id, D, B, O>, 1>,
    access_point_group: radio::Places<KeptTerminal<'id, Id, D, B, O>, 1>,
    units: [Option<KeptUnit<'id, Id, D, B>>; RX_HELD],
    /// How many items the store keeps: every kept item's sequence is below.
    count: u8,
    /// How many of the next puts a test refuses, as a taken place does.
    #[cfg(test)]
    pub(super) refuse_keeps: usize,
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
    Store<'id, Id, D, B, O>
{
    pub(super) const fn new() -> Self {
        Self {
            tickets: radio::Places::new(),
            gate: None,
            station_configure: None,
            access_point_configure: None,
            station_pairwise: radio::Places::new(),
            access_point_pairwise: radio::Places::new(),
            station_group: radio::Places::new(),
            access_point_group: radio::Places::new(),
            units: [const { None }; RX_HELD],
            count: 0,
            #[cfg(test)]
            refuse_keeps: 0,
        }
    }

    /// Whether the store keeps nothing.
    pub(super) const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Keep `item` behind everything kept.
    ///
    /// # Errors
    ///
    /// Its place holds an item, or every unit place does: `item`, whole.
    pub(super) fn keep(
        &mut self,
        item: Item<'id, Id, D, B, O>,
    ) -> Result<(), Item<'id, Id, D, B, O>> {
        let sequence = self.count;
        self.put(sequence, item)?;
        self.count = self.count.saturating_add(1);
        Ok(())
    }

    /// Put `item` at its own place with `sequence`.
    ///
    /// # Errors
    ///
    /// Its place holds an item, or every unit place does: `item`, whole.
    fn put(
        &mut self,
        sequence: u8,
        item: Item<'id, Id, D, B, O>,
    ) -> Result<(), Item<'id, Id, D, B, O>> {
        #[cfg(test)]
        if let Some(left) = self.refuse_keeps.checked_sub(1) {
            self.refuse_keeps = left;
            return Err(item);
        }
        match item {
            Item::Terminal(terminal) => self
                .put_terminal((sequence, terminal))
                .map_err(|(_, terminal)| Item::Terminal(terminal)),
            Item::Received(frame, meta) => {
                match self.units.iter_mut().find(|unit| unit.is_none()) {
                    Some(place) => {
                        *place = Some((sequence, frame, meta));
                        Ok(())
                    }
                    None => Err(Item::Received(frame, meta)),
                }
            }
        }
    }

    /// Put `kept` in the place of its terminal's token.
    fn put_terminal(
        &mut self,
        kept: KeptTerminal<'id, Id, D, B, O>,
    ) -> Result<(), KeptTerminal<'id, Id, D, B, O>> {
        let single = |place: &mut Option<KeptTerminal<'id, Id, D, B, O>>, kept| match place {
            None => {
                *place = Some(kept);
                Ok(())
            }
            Some(_) => Err(kept),
        };
        match &kept.1 {
            LowerMacTerminal::TxCompleted(
                TxCompleted::Single(completed) | TxCompleted::Beacon { completed, .. },
            ) => {
                let place = completed.ticket_place();
                self.tickets.put(place, kept)
            }
            LowerMacTerminal::GateClosed(_) => single(&mut self.gate, kept),
            LowerMacTerminal::Configured(Configured::Station(_)) => {
                single(&mut self.station_configure, kept)
            }
            LowerMacTerminal::Configured(Configured::AccessPoint(_)) => {
                single(&mut self.access_point_configure, kept)
            }
            LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::Station(key)) => {
                match key.identity.slot {
                    keys::SlotPlace::StationPairwise(place) => {
                        self.station_pairwise.put(place, kept)
                    }
                    // A station key names a station pairwise slot.
                    _ => Err(kept),
                }
            }
            LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::AccessPoint(key)) => {
                match key.identity.slot {
                    keys::SlotPlace::AccessPointPairwise(place) => {
                        self.access_point_pairwise.put(place, kept)
                    }
                    // An access point key names its pairwise slot.
                    _ => Err(kept),
                }
            }
            LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Pairwise {
                slot, ..
            })) => {
                let place = slot.place();
                self.station_pairwise.put(place, kept)
            }
            LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Pairwise {
                slot,
                ..
            })) => {
                let place = slot.place();
                self.access_point_pairwise.put(place, kept)
            }
            LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Group {
                permission,
                ..
            }))
            | LowerMacTerminal::GroupKeyReplaced { permission, .. }
            | LowerMacTerminal::GroupKeySelected { permission, .. }
            | LowerMacTerminal::GroupKeyInstalled(InstalledGroup::Station { permission, .. }) => {
                let place = permission.place();
                self.station_group.put(place, kept)
            }
            LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Group {
                permission,
                ..
            }))
            | LowerMacTerminal::GroupKeyInstalled(InstalledGroup::AccessPoint {
                permission, ..
            }) => {
                let place = permission.place();
                self.access_point_group.put(place, kept)
            }
        }
    }

    /// The place of the oldest kept item.
    fn first(&mut self) -> Option<Cell<'_, 'id, Id, D, B, O>> {
        let oldest = |kept: &KeptTerminal<'id, Id, D, B, O>| kept.0 == 0;
        let single =
            |place: &mut Option<KeptTerminal<'id, Id, D, B, O>>| place.as_ref().is_some_and(oldest);
        if let Some(cell) = self.tickets.find_mut(oldest) {
            return Some(Cell::Terminal(cell));
        }
        if single(&mut self.gate) {
            return Some(Cell::Terminal(&mut self.gate));
        }
        if single(&mut self.station_configure) {
            return Some(Cell::Terminal(&mut self.station_configure));
        }
        if single(&mut self.access_point_configure) {
            return Some(Cell::Terminal(&mut self.access_point_configure));
        }
        let pool = [
            &mut self.station_pairwise as &mut dyn FindOldest<'id, Id, D, B, O>,
            &mut self.access_point_pairwise,
            &mut self.station_group,
            &mut self.access_point_group,
        ]
        .into_iter()
        .find_map(|places| places.oldest());
        if let Some(cell) = pool {
            return Some(Cell::Terminal(cell));
        }
        self.units
            .iter_mut()
            .find(|unit| matches!(unit, Some((0, _, _))))
            .map(Cell::Unit)
    }

    /// Every kept item's sequence, for writing.
    fn sequences(&mut self) -> impl Iterator<Item = &mut u8> {
        self.tickets
            .iter_mut()
            .chain(self.gate.iter_mut())
            .chain(self.station_configure.iter_mut())
            .chain(self.access_point_configure.iter_mut())
            .chain(self.station_pairwise.iter_mut())
            .chain(self.access_point_pairwise.iter_mut())
            .chain(self.station_group.iter_mut())
            .chain(self.access_point_group.iter_mut())
            .map(|(sequence, _)| sequence)
            .chain(
                self.units
                    .iter_mut()
                    .flatten()
                    .map(|(sequence, _, _)| sequence),
            )
    }

    /// The oldest item left: every sequence moves down by one.
    fn left_first(&mut self) {
        self.count = self.count.saturating_sub(1);
        for sequence in self.sequences() {
            *sequence = sequence.saturating_sub(1);
        }
    }

    /// Give `item` to `sink` behind what the store keeps: kept when the
    /// store keeps anything, or when the sink refuses it. The call's right
    /// `vacancy`, obtained before the item's owner was taken, comes back
    /// once the sink or the store took the item; an item neither takes
    /// fills the call's place and the right ends there, so the call takes
    /// no further owner.
    pub(super) fn deliver(
        &mut self,
        sink: &mut impl PortSink<'id, Types<Id, D, B, O>>,
        place: &mut CallPlace<'id, Id, D, B, O>,
        vacancy: Vacancy,
        item: Item<'id, Id, D, B, O>,
    ) {
        let item = if self.is_empty() {
            match offer_one(sink, item) {
                Ok(()) => return place.vacate(vacancy),
                Err(item) => item,
            }
        } else {
            item
        };
        match self.keep(item) {
            Ok(()) => place.vacate(vacancy),
            Err(item) => place.hold(vacancy, item),
        }
    }

    /// Offer what the store keeps to `sink`, oldest first, until the sink
    /// refuses one, which goes back to its own place and stays first.
    pub(super) fn offer(
        &mut self,
        sink: &mut impl PortSink<'id, Types<Id, D, B, O>>,
    ) -> Unconsumed {
        loop {
            // An empty store has no first item: no place is searched.
            if self.is_empty() {
                return Unconsumed::None;
            }
            let refused = match self.first() {
                None => return Unconsumed::None,
                Some(Cell::Terminal(cell)) => match cell.take() {
                    Some((sequence, terminal)) => match sink.terminal(terminal) {
                        Ok(()) => false,
                        Err(terminal) => {
                            *cell = Some((sequence, terminal));
                            true
                        }
                    },
                    None => return Unconsumed::None,
                },
                Some(Cell::Unit(cell)) => match cell.take() {
                    Some((sequence, frame, meta)) => match sink.received(frame, meta) {
                        Ok(()) => false,
                        Err((frame, meta)) => {
                            *cell = Some((sequence, frame, meta));
                            true
                        }
                    },
                    None => return Unconsumed::None,
                },
            };
            if refused {
                return Unconsumed::Kept;
            }
            self.left_first();
        }
    }
}

/// A pool's places, searched for the oldest kept terminal.
trait FindOldest<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations> {
    fn oldest(&mut self) -> Option<&mut Option<KeptTerminal<'id, Id, D, B, O>>>;
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations, const N: usize>
    FindOldest<'id, Id, D, B, O> for radio::Places<KeptTerminal<'id, Id, D, B, O>, N>
{
    fn oldest(&mut self) -> Option<&mut Option<KeptTerminal<'id, Id, D, B, O>>> {
        self.find_mut(|kept| kept.0 == 0)
    }
}

/// Offer `item` to `sink`; refused, it comes back.
pub(super) fn offer_one<
    'id,
    Id: SlotIdentity + 'id,
    D: Device<B>,
    const B: usize,
    O: Observations,
>(
    sink: &mut impl PortSink<'id, Types<Id, D, B, O>>,
    item: Item<'id, Id, D, B, O>,
) -> Result<(), Item<'id, Id, D, B, O>> {
    match item {
        Item::Terminal(terminal) => sink.terminal(terminal).map_err(Item::Terminal),
        Item::Received(frame, meta) => sink
            .received(frame, meta)
            .map_err(|(frame, meta)| Item::Received(frame, meta)),
    }
}
