//! Installed group owners and reserved record places. A pending installation
//! owns its vacant place, so its return cannot overflow a retirement pool.

use super::{Ccmp128Key, CcmpRxReplayEpoch, GroupKeyId, Pool};
use oer_ieee80211_mac::ccmp::CcmpRxReplayState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
                    key: value,
                    replay: Replay::Retained(
                        CcmpRxReplayState::from_receive_sequence([0; 8]).unwrap(),
                    ),
                },
                true,
            );
        }
        // A selection round trip does not renew the installation's age.
        let selected = keys.take_matching(id, &[0; 16]).unwrap();
        keys.restore(selected, true);
        let selected = keys.take_matching(id, &[2; 16]).unwrap();
        keys.restore(selected, true);
        let old = keys.replace().unwrap();
        assert_eq!(old.key, 0);
        assert_eq!(keys.current_id(), Some(id));
        keys.release(old.place);
        let next = keys.take_retiring().unwrap();
        assert_eq!(next.key, 1);
        keys.release(next.place);
        let single = keys.replace().unwrap();
        assert_eq!(single.key, 2);
        keys.install(single, true);
        let last = keys.take_any().unwrap();
        keys.release(last.place);
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

/// One vacant record; minted privately once and moved through each install.
#[derive(Debug)]
pub(super) struct GroupPlace(Index);

pub(super) struct Installed<Key> {
    pub(super) place: GroupPlace,
    pub(super) key_id: GroupKeyId,
    pub(super) material: Ccmp128Key,
    pub(super) key: Key,
    pub(super) replay: Replay,
}

/// An active record lends its frontier to the association RX epoch. A retained
/// record owns the exact frontier taken back at the next selection boundary.
#[derive(Debug)]
pub(super) enum Replay {
    Active,
    Retained(CcmpRxReplayState),
}

pub(super) struct GroupKeys<Key> {
    entries: [Option<Installed<Key>>; 8],
    vacant: Pool<GroupPlace>,
    /// Oldest first, without a wrapping or saturating installation counter.
    order: [Index; 8],
    current: Option<Index>,
}

impl<Key> GroupKeys<Key> {
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.iter().flatten().count()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn owners(&self) -> impl Iterator<Item = &Key> {
        self.entries.iter().flatten().map(|entry| &entry.key)
    }

    pub(super) fn new() -> Self {
        Self {
            entries: [const { None }; 8],
            vacant: Pool::new(Index::ALL.map(GroupPlace)),
            order: Index::ALL,
            current: None,
        }
    }

    pub(super) fn reserve(&mut self) -> Option<GroupPlace> {
        self.vacant.take()
    }

    pub(super) fn install(&mut self, entry: Installed<Key>, current: bool) {
        let index = entry.place.0;
        self.restore(entry, current);
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
    pub(super) fn restore(&mut self, entry: Installed<Key>, current: bool) {
        let index = entry.place.0;
        *index.select_mut(&mut self.entries) = Some(entry);
        if current {
            self.current = Some(index);
        }
    }

    pub(super) fn activate(&mut self, entry: &mut Installed<Key>, replay: &mut CcmpRxReplayEpoch) {
        let Replay::Retained(next) = core::mem::replace(&mut entry.replay, Replay::Active) else {
            // It was already RX-active under this ID; selecting it changes
            // neither counters nor the per-ID replay frontier.
            return;
        };
        if let Some(previous) = replay.exchange_group(entry.key_id, next)
            && let Some(old) = self
                .entries
                .iter_mut()
                .flatten()
                .find(|old| old.key_id == entry.key_id && matches!(old.replay, Replay::Active))
        {
            old.replay = Replay::Retained(previous);
        }
        // No old retained record means the old physical installation was
        // replaced; its frontier legitimately ends with that installation.
    }

    pub(super) fn release(&mut self, place: GroupPlace) {
        self.vacant.put(place);
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
    ) -> Option<Installed<Key>> {
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

    pub(super) fn take_retiring(&mut self) -> Option<Installed<Key>> {
        self.take(self.oldest_retiring()?)
    }

    pub(super) fn take_any(&mut self) -> Option<Installed<Key>> {
        let index = self.oldest_retiring().or(self.current)?;
        self.take(index)
    }

    fn take(&mut self, index: Index) -> Option<Installed<Key>> {
        let entry = index.select_mut(&mut self.entries).take()?;
        if self.current == Some(index) {
            self.current = None;
        }
        // The removal token keeps this record place until its terminal.
        Some(entry)
    }

    /// When no vacant slot remains, replace the oldest non-current record,
    /// or the current one when it is the only installed owner. Selection
    /// leaves the installation's age unchanged.
    pub(super) fn replace(&mut self) -> Option<Installed<Key>> {
        let index = self.oldest_retiring().or(self.current)?;
        index.select_mut(&mut self.entries).take()
    }
}
