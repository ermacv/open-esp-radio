//! The station's group table: its installed group keys, by place, and the
//! places a pending operation reserved.

use super::{Ccmp128Key, GroupKeyId, Pool};
use oer_protocols_ieee80211_core_upper_mac::ccmp::CcmpRxReplayState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum Index {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_evicts_the_oldest_non_current_installation() {
        let mut keys = GroupKeys::new();
        let id = GroupKeyId::new(1).unwrap();
        for value in 0..3 {
            let place = keys.reserve().unwrap();
            keys.install(
                Installed {
                    place,
                    key_id: id,
                    material: Ccmp128Key::new(&[value; 16]),
                    replay: Replay::Retained(
                        CcmpRxReplayState::from_receive_sequence([0; 8]).unwrap(),
                    ),
                },
                GroupRole::Current,
            );
        }
        // A selection round trip does not renew the installation's age.
        let selected = keys.take_matching(id, &[0; 16]).unwrap();
        keys.restore(selected, GroupRole::Current);
        let selected = keys.take_matching(id, &[2; 16]).unwrap();
        keys.restore(selected, GroupRole::Current);
        let old = keys.replace().unwrap();
        assert_eq!(old.material.as_bytes(), &[0; 16]);
        assert_eq!(keys.current_id(), Some(id));
        keys.release(old.place, &mut ());
        // The next replacement takes the remaining non-current one.
        let next = keys.replace().unwrap();
        assert_eq!(next.material.as_bytes(), &[1; 16]);
        keys.release(next.place, &mut ());
        let single = keys.replace().unwrap();
        assert_eq!(single.material.as_bytes(), &[2; 16]);
        keys.install(single, GroupRole::Current);
        let last = keys.take_any().unwrap();
        keys.release(last.place, &mut ());
        assert_eq!(keys.vacant.len(), 8);
    }
}

impl Index {
    const ALL: [Self; 8] = [
        Self::A,
        Self::B,
        Self::C,
        Self::D,
        Self::E,
        Self::F,
        Self::G,
        Self::H,
    ];

    fn select<T>(self, values: &[T; 8]) -> &T {
        let [a, b, c, d, e, f, g, h] = values;
        match self {
            Self::A => a,
            Self::B => b,
            Self::C => c,
            Self::D => d,
            Self::E => e,
            Self::F => f,
            Self::G => g,
            Self::H => h,
        }
    }

    fn select_mut<T>(self, values: &mut [T; 8]) -> &mut T {
        let [a, b, c, d, e, f, g, h] = values;
        match self {
            Self::A => a,
            Self::B => b,
            Self::C => c,
            Self::D => d,
            Self::E => e,
            Self::F => f,
            Self::G => g,
            Self::H => h,
        }
    }
}

/// A place of the station's group table: which of its records a group key
/// operation names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaGroupPlace(Index);

impl StaGroupPlace {
    /// The place's index in the table, from zero.
    pub const fn index(self) -> u8 {
        self.0 as u8
    }
}

pub(super) struct Installed {
    pub(super) place: StaGroupPlace,
    pub(super) key_id: GroupKeyId,
    pub(super) material: Ccmp128Key,
    pub(super) replay: Replay,
}

/// An active record lends its frontier to the association RX epoch. A retained
/// record owns the exact frontier taken back at the next selection boundary.
#[derive(Debug)]
pub(super) enum Replay {
    Active,
    Retained(CcmpRxReplayState),
}

/// Whether an installed group key becomes the current one, which receives,
/// or stays beside it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GroupRole {
    Current,
    Other,
}

/// How a group key comes into the table: a new installation, or a selection
/// of one it holds already, which keeps its installation age.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GroupArrival {
    Installed,
    Selected,
}

pub(super) struct GroupKeys {
    entries: [Option<Installed>; 8],
    vacant: Pool<StaGroupPlace>,
    /// Oldest first, without a wrapping or saturating installation counter.
    order: [Index; 8],
    current: Option<Index>,
}

impl GroupKeys {
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.iter().flatten().count()
    }

    pub(super) fn new() -> Self {
        Self {
            entries: [const { None }; 8],
            vacant: Pool::new(Index::ALL.map(StaGroupPlace)),
            order: Index::ALL,
            current: None,
        }
    }

    /// Whether no place is installed or reserved.
    pub(super) fn is_empty(&self) -> bool {
        self.vacant.len() == Index::ALL.len()
    }

    /// The places installed or reserved.
    pub(super) fn occupied(&self) -> usize {
        Index::ALL.len().saturating_sub(self.vacant.len())
    }

    pub(super) fn reserve(&mut self) -> Option<StaGroupPlace> {
        self.vacant.take()
    }

    pub(super) fn install(&mut self, entry: Installed, role: GroupRole) {
        let index = entry.place.0;
        self.restore(entry, role);
        let mut carry = index;
        for position in self.order.iter_mut().rev() {
            if *position == index {
                *position = carry;
                break;
            }
            core::mem::swap(position, &mut carry);
        }
    }

    /// Selection restores an existing record without changing its physical
    /// installation age, used to choose the oldest non-current eviction.
    pub(super) fn restore(&mut self, entry: Installed, role: GroupRole) {
        let index = entry.place.0;
        *index.select_mut(&mut self.entries) = Some(entry);
        if role == GroupRole::Current {
            self.current = Some(index);
        }
    }

    /// The installation of `key_id` whose lane a new installation took
    /// keeps the frontier it had, `previous`, for its next selection.
    pub(super) fn retain(&mut self, key_id: GroupKeyId, previous: CcmpRxReplayState) {
        if let Some(old) = self
            .entries
            .iter_mut()
            .flatten()
            .find(|old| old.key_id == key_id && matches!(old.replay, Replay::Active))
        {
            old.replay = Replay::Retained(previous);
        }
        // No old retained record means the old physical installation was
        // replaced; its frontier legitimately ends with that installation.
    }

    pub(super) fn release(
        &mut self,
        place: StaGroupPlace,
        events: &mut impl oer_portable_events::EventSink<crate::events::StaEvent>,
    ) {
        self.vacant.put(place, events);
    }

    pub(super) fn has_active(&self, id: GroupKeyId) -> bool {
        self.entries
            .iter()
            .flatten()
            .any(|entry| entry.key_id == id && matches!(entry.replay, Replay::Active))
    }

    pub(super) fn current_material(&self, id: GroupKeyId, material: &[u8; 16]) -> bool {
        self.current
            .and_then(|index| index.select(&self.entries).as_ref())
            .is_some_and(|entry| entry.key_id == id && entry.material.as_bytes() == material)
    }

    pub(super) fn take_matching(
        &mut self,
        id: GroupKeyId,
        material: &[u8; 16],
    ) -> Option<Installed> {
        self.entries
            .iter_mut()
            .find(|entry| {
                entry.as_ref().is_some_and(|entry| {
                    entry.key_id == id && entry.material.as_bytes() == material
                })
            })?
            .take()
    }

    pub(super) fn current_id(&self) -> Option<GroupKeyId> {
        self.current
            .and_then(|index| index.select(&self.entries).as_ref())
            .map(|entry| entry.key_id)
    }

    #[cfg(test)]
    pub(super) fn same_material(&self, id: GroupKeyId, material: &[u8; 16]) -> bool {
        self.entries
            .iter()
            .flatten()
            .any(|entry| entry.key_id == id && entry.material.as_bytes() == material)
    }

    fn oldest_retiring(&self) -> Option<Index> {
        self.order
            .iter()
            .copied()
            .find(|index| Some(*index) != self.current && index.select(&self.entries).is_some())
    }

    pub(super) fn take_any(&mut self) -> Option<Installed> {
        let index = self.oldest_retiring().or(self.current)?;
        self.take(index)
    }

    fn take(&mut self, index: Index) -> Option<Installed> {
        let entry = index.select_mut(&mut self.entries).take()?;
        if self.current == Some(index) {
            self.current = None;
        }
        // The removal keeps this record's place until its end.
        Some(entry)
    }

    /// When no vacant slot remains, replace the oldest non-current record,
    /// or the current one when it is the only installed owner. Selection
    /// leaves the installation's age unchanged.
    pub(super) fn replace(&mut self) -> Option<Installed> {
        let index = self.oldest_retiring().or(self.current)?;
        index.select_mut(&mut self.entries).take()
    }
}
