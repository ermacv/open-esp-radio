//! Growth of ordinary collections admitted in a [`WorkingMemory`] before it
//! is made. A structure that outlives its working memory (an analysis a
//! caller keeps after its budget ends) cannot hold a reservation of its own;
//! its builder admits every allocation through a [`Ledger`] the caller holds
//! while the structure counts against the operation.
//!
//! As the working memory's contract says, what is admitted is the requested
//! allocations: a vector's capacity, a map entry's key and value, a string's
//! bytes; never the allocator's or a map's own bookkeeping.
use crate::*;
use std::collections::{BTreeMap, BTreeSet};

/// The admitted bytes of the structures one builder keeps.
pub struct Ledger<'m> {
    memory: &'m WorkingMemory,
    reserved: MemoryReservation<'m>,
}

fn refused() -> Error {
    Error::new(ErrorCode::ResourceLimited, "admitted allocation refused")
}

impl<'m> Ledger<'m> {
    pub fn new(memory: &'m WorkingMemory, control: &mut dyn RunControl) -> Result<Self> {
        Ok(Self {
            memory,
            reserved: memory.reserve(0, control.position())?,
        })
    }

    /// The working memory the ledger admits in.
    pub fn memory(&self) -> &'m WorkingMemory {
        self.memory
    }

    /// Admit `bytes` more.
    pub fn admit(&mut self, bytes: usize, control: &mut dyn RunControl) -> Result<()> {
        self.reserved.grow(bytes as u64, control.position())
    }

    /// Push `value` onto `vector`, admitting a grown capacity before the
    /// vector reallocates, with the old buffer admitted while both exist.
    pub fn push<T>(
        &mut self,
        vector: &mut Vec<T>,
        value: T,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        if vector.len() == vector.capacity() {
            let more = vector.capacity().max(4);
            let old = vector.capacity() * size_of::<T>();
            let _both = self.memory.reserve(old as u64, control.position())?;
            self.admit(more * size_of::<T>(), control)?;
            vector.try_reserve_exact(more).map_err(|_| refused())?;
        }
        vector.push(value);
        Ok(())
    }

    /// Insert `key` into `map`, admitting the entry when it is new; the
    /// value it replaces, if any.
    pub fn insert<K: Ord, V>(
        &mut self,
        map: &mut BTreeMap<K, V>,
        key: K,
        value: V,
        control: &mut dyn RunControl,
    ) -> Result<Option<V>> {
        if !map.contains_key(&key) {
            self.admit(size_of::<K>() + size_of::<V>(), control)?;
        }
        Ok(map.insert(key, value))
    }

    /// The entry of `key` in `map`, inserted with `value()` and admitted when
    /// it is new.
    pub fn entry<'a, K: Ord, V>(
        &mut self,
        map: &'a mut BTreeMap<K, V>,
        key: K,
        value: impl FnOnce() -> V,
        control: &mut dyn RunControl,
    ) -> Result<&'a mut V> {
        if !map.contains_key(&key) {
            self.admit(size_of::<K>() + size_of::<V>(), control)?;
        }
        Ok(map.entry(key).or_insert_with(value))
    }

    /// Insert `value` into `set`, admitting it when it is new.
    pub fn add<T: Ord>(
        &mut self,
        set: &mut BTreeSet<T>,
        value: T,
        control: &mut dyn RunControl,
    ) -> Result<bool> {
        if set.contains(&value) {
            return Ok(false);
        }
        self.admit(size_of::<T>(), control)?;
        Ok(set.insert(value))
    }

    /// An owned copy of `text`, admitted before it is made.
    pub fn string(&mut self, text: &str, control: &mut dyn RunControl) -> Result<String> {
        self.admit(text.len(), control)?;
        let mut out = String::new();
        out.try_reserve_exact(text.len()).map_err(|_| refused())?;
        out.push_str(text);
        Ok(out)
    }

    /// The bytes admitted so far.
    pub fn bytes(&self) -> u64 {
        self.reserved.bytes()
    }

    /// The reservation of everything admitted, for the caller to hold while
    /// the structure lives.
    pub fn into_reservation(self) -> MemoryReservation<'m> {
        self.reserved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control() -> impl FnMut() -> Result<()> {
        || Ok(())
    }

    #[test]
    fn a_push_admits_the_new_capacity_and_the_old_buffer_while_both_exist() {
        let memory = WorkingMemory::new(1 << 20).unwrap();
        let mut ledger = Ledger::new(&memory, &mut control()).unwrap();
        let mut values: Vec<u64> = Vec::new();
        for value in 0..5 {
            ledger.push(&mut values, value, &mut control()).unwrap();
        }
        // Four, then four more: eight words admitted, and while growing to
        // eight the old four were admitted beside them.
        assert_eq!(values.capacity(), 8);
        assert_eq!(ledger.bytes(), 8 * 8);
        assert_eq!(memory.used(), 8 * 8);
        assert_eq!(memory.peak(), 8 * 8 + 4 * 8);
    }

    #[test]
    fn a_refused_push_leaves_the_vector_as_it_was() {
        let memory = WorkingMemory::new(4 * 8 + 4 * 8).unwrap();
        let mut ledger = Ledger::new(&memory, &mut control()).unwrap();
        let mut values: Vec<u64> = Vec::new();
        for value in 0..4 {
            ledger.push(&mut values, value, &mut control()).unwrap();
        }
        let error = ledger.push(&mut values, 4, &mut control()).unwrap_err();
        assert_eq!(error.code, ErrorCode::ResourceLimited);
        assert_eq!((values.len(), values.capacity()), (4, 4));
        assert_eq!(ledger.bytes(), 4 * 8);
    }

    #[test]
    fn a_key_or_member_already_present_admits_nothing_more() {
        let memory = WorkingMemory::new(1 << 20).unwrap();
        let mut ledger = Ledger::new(&memory, &mut control()).unwrap();
        let mut map = BTreeMap::new();
        ledger.insert(&mut map, 1u32, 2u64, &mut control()).unwrap();
        let entry = ledger.bytes();
        assert_eq!(entry, (size_of::<u32>() + size_of::<u64>()) as u64);
        assert_eq!(
            ledger.insert(&mut map, 1, 3, &mut control()).unwrap(),
            Some(2)
        );
        *ledger.entry(&mut map, 1, || 0, &mut control()).unwrap() += 1;
        assert_eq!(ledger.bytes(), entry);
        let mut set = BTreeSet::new();
        assert!(ledger.add(&mut set, 7u32, &mut control()).unwrap());
        assert!(!ledger.add(&mut set, 7u32, &mut control()).unwrap());
        assert_eq!(ledger.bytes(), entry + size_of::<u32>() as u64);
    }

    #[test]
    fn a_string_admits_its_bytes_and_everything_is_released_at_once() {
        let memory = WorkingMemory::new(1 << 20).unwrap();
        let mut ledger = Ledger::new(&memory, &mut control()).unwrap();
        let text = ledger.string("waker", &mut control()).unwrap();
        assert_eq!((text.as_str(), ledger.bytes()), ("waker", 5));
        let reservation = ledger.into_reservation();
        assert_eq!(memory.used(), 5);
        drop(reservation);
        assert_eq!(memory.used(), 0);
        let mut ledger = Ledger::new(&memory, &mut control()).unwrap();
        ledger.admit(3, &mut control()).unwrap();
        drop(ledger);
        assert_eq!(memory.used(), 0);
    }
}
