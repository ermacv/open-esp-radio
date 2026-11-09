//! What a sink refused, which the port keeps whole until it is offered
//! again ([`PortSink`]).
//!
//! A terminal waits in the place of its token: its ticket, the gate, its
//! interface's configure permission, its pairwise key slot or its group
//! permission. Each token exists once in an installation and a terminal
//! carries it until its sink takes it, so no two kept terminals name one
//! place and the places never run out. A refused receive unit waits in a
//! place of its own, counted among the caller's held units, of which there
//! are at most [`RX_HELD`]. One sequence orders both: a relative number,
//! below the count of what the store keeps, which leaving renumbers.

use super::*;
use oer_protocols_ieee80211_contracts_port::{
    Configured, InstalledGroup, InstalledPairwise, Removed, RemovedKey, TxCompleted,
};

/// Places of single-MPDU terminals, one per ticket.
const TICKET_PLACES: usize = TX_TICKETS;
/// The gate's place.
const GATE_PLACE: usize = TICKET_PLACES;
/// The configure permissions' places: the station's, then the access
/// point's.
const CONFIGURE_PLACES: usize = GATE_PLACE + 1;
/// The station's pairwise key slots' places.
const STATION_PAIRWISE_PLACES: usize = CONFIGURE_PLACES + 2;
/// The access point's pairwise key slots' places.
const ACCESS_POINT_PAIRWISE_PLACES: usize =
    STATION_PAIRWISE_PLACES + crate::lower_mac::association::STATION_PAIRWISE_SLOTS;
/// The station's group permissions' places.
const STATION_GROUP_PLACES: usize =
    ACCESS_POINT_PAIRWISE_PLACES + crate::lower_mac::association::ACCESS_POINT_PAIRWISE_SLOTS;
/// The access point's group permissions' places.
const ACCESS_POINT_GROUP_PLACES: usize = STATION_GROUP_PLACES + 1;
/// Every terminal place.
const TERMINAL_PLACES: usize = ACCESS_POINT_GROUP_PLACES + 1;

const _: () = {
    if TERMINAL_PLACES + RX_HELD > u8::MAX as usize {
        panic!("a kept item's sequence fits its byte");
    }
};

/// One kept item, as the sink gets it.
pub(super) enum Item<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations> {
    Terminal(Terminal<'id, Id, D, B, O>),
    Received(ReceiveBuffer<'id, Id, D::Receive>, Esp32s31RxMeta),
}

/// The port's store of what its sink refused.
pub(super) struct Store<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
{
    terminals: [Option<(u8, Terminal<'id, Id, D, B, O>)>; TERMINAL_PLACES],
    units: [Option<(u8, ReceiveBuffer<'id, Id, D::Receive>, Esp32s31RxMeta)>; RX_HELD],
    /// How many items the store keeps: every kept item's sequence is below.
    count: u8,
    /// A place the store found taken or missing: the token it names was
    /// not unique, which no installation issues. The item that found it
    /// could not be kept.
    broken: bool,
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
    Store<'id, Id, D, B, O>
{
    pub(super) const fn new() -> Self {
        Self {
            terminals: [const { None }; TERMINAL_PLACES],
            units: [const { None }; RX_HELD],
            count: 0,
            broken: false,
        }
    }

    /// Whether the store keeps nothing.
    pub(super) const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Whether a place was found taken or missing.
    pub(super) const fn broken(&self) -> bool {
        self.broken
    }

    /// Keep `item` behind everything kept.
    pub(super) fn keep(&mut self, item: Item<'id, Id, D, B, O>) {
        let sequence = self.count;
        if self.put(sequence, item) {
            self.count = self.count.saturating_add(1);
        }
    }

    /// Keep `item` ahead of everything kept: a refused re-offer.
    fn keep_first(&mut self, item: Item<'id, Id, D, B, O>) {
        for (sequence, _) in self.terminals.iter_mut().flatten() {
            *sequence = sequence.saturating_add(1);
        }
        for (sequence, _, _) in self.units.iter_mut().flatten() {
            *sequence = sequence.saturating_add(1);
        }
        if self.put(0, item) {
            self.count = self.count.saturating_add(1);
        } else {
            // Undo the renumbering: nothing was put first.
            self.renumber_after_take();
        }
    }

    /// Put `item` at its place with `sequence`; whether it was kept.
    fn put(&mut self, sequence: u8, item: Item<'id, Id, D, B, O>) -> bool {
        match item {
            Item::Terminal(terminal) => {
                let place = place(&terminal).and_then(|place| self.terminals.get_mut(place));
                match place {
                    Some(place @ None) => {
                        *place = Some((sequence, terminal));
                        true
                    }
                    // Excluded by the token's uniqueness: the terminal has
                    // no place, and only the fault remains of it.
                    _ => {
                        self.broken = true;
                        false
                    }
                }
            }
            Item::Received(frame, meta) => {
                match self.units.iter_mut().find(|unit| unit.is_none()) {
                    Some(place) => {
                        *place = Some((sequence, frame, meta));
                        true
                    }
                    // Excluded by the caller's held count, which counts every
                    // kept unit and is at most `RX_HELD`.
                    None => {
                        self.broken = true;
                        false
                    }
                }
            }
        }
    }

    /// Take the oldest kept item, renumbering the rest.
    fn take_first(&mut self) -> Option<Item<'id, Id, D, B, O>> {
        let item = if let Some(place) = self
            .terminals
            .iter_mut()
            .find(|place| matches!(place, Some((0, _))))
        {
            place.take().map(|(_, terminal)| Item::Terminal(terminal))
        } else if let Some(place) = self
            .units
            .iter_mut()
            .find(|place| matches!(place, Some((0, _, _))))
        {
            place
                .take()
                .map(|(_, frame, meta)| Item::Received(frame, meta))
        } else {
            None
        }?;
        self.count = self.count.saturating_sub(1);
        self.renumber_after_take();
        Some(item)
    }

    /// One place less in front: every sequence moves down by one.
    fn renumber_after_take(&mut self) {
        for (sequence, _) in self.terminals.iter_mut().flatten() {
            *sequence = sequence.saturating_sub(1);
        }
        for (sequence, _, _) in self.units.iter_mut().flatten() {
            *sequence = sequence.saturating_sub(1);
        }
    }

    /// Give `item` to `sink` behind what the store keeps: kept when the
    /// store keeps anything, or when the sink refuses it.
    pub(super) fn deliver(
        &mut self,
        sink: &mut impl PortSink<'id, Types<Id, D, B, O>>,
        item: Item<'id, Id, D, B, O>,
    ) {
        if !self.is_empty() {
            self.keep(item);
        } else if let Err(item) = offer_one(sink, item) {
            self.keep(item);
        }
    }

    /// Offer what the store keeps to `sink`, oldest first, until the sink
    /// refuses one, which stays first.
    pub(super) fn offer(
        &mut self,
        sink: &mut impl PortSink<'id, Types<Id, D, B, O>>,
    ) -> Unconsumed {
        while let Some(item) = self.take_first() {
            if let Err(item) = offer_one(sink, item) {
                self.keep_first(item);
                return Unconsumed::Kept;
            }
        }
        Unconsumed::None
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

/// The place of `terminal`'s token.
fn place<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>(
    terminal: &Terminal<'id, Id, D, B, O>,
) -> Option<usize> {
    let (base, index) = match terminal {
        LowerMacTerminal::TxCompleted(
            TxCompleted::Single(completed) | TxCompleted::Beacon { completed, .. },
        ) => (0, completed.ticket_index()),
        LowerMacTerminal::GateClosed(_) => (GATE_PLACE, 0),
        LowerMacTerminal::Configured(Configured::Station(_)) => (CONFIGURE_PLACES, 0),
        LowerMacTerminal::Configured(Configured::AccessPoint(_)) => (CONFIGURE_PLACES, 1),
        LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::Station(key)) => {
            (STATION_PAIRWISE_PLACES, key.identity.slot)
        }
        LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::AccessPoint(key)) => {
            (ACCESS_POINT_PAIRWISE_PLACES, key.identity.slot)
        }
        LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Pairwise { slot, .. })) => {
            (STATION_PAIRWISE_PLACES, slot.index())
        }
        LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Pairwise {
            slot, ..
        })) => (ACCESS_POINT_PAIRWISE_PLACES, slot.index()),
        LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Group {
            permission, ..
        }))
        | LowerMacTerminal::GroupKeyReplaced { permission, .. }
        | LowerMacTerminal::GroupKeySelected { permission, .. }
        | LowerMacTerminal::GroupKeyInstalled(InstalledGroup::Station { permission, .. }) => {
            (STATION_GROUP_PLACES, permission.index())
        }
        LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Group {
            permission,
            ..
        }))
        | LowerMacTerminal::GroupKeyInstalled(InstalledGroup::AccessPoint { permission, .. }) => {
            (ACCESS_POINT_GROUP_PLACES, permission.index())
        }
    };
    let place = base.checked_add(usize::from(index))?;
    // A place of the next pool is not this token's.
    let end = match base {
        0 => TICKET_PLACES,
        GATE_PLACE => CONFIGURE_PLACES,
        CONFIGURE_PLACES => STATION_PAIRWISE_PLACES,
        STATION_PAIRWISE_PLACES => ACCESS_POINT_PAIRWISE_PLACES,
        ACCESS_POINT_PAIRWISE_PLACES => STATION_GROUP_PLACES,
        STATION_GROUP_PLACES => ACCESS_POINT_GROUP_PLACES,
        _ => TERMINAL_PLACES,
    };
    (place < end).then_some(place)
}
