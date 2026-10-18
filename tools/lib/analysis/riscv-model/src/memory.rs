//! Explicit capacity admission. Reservations account for requested allocations,
//! not allocator bookkeeping, stacks, library internals or resident process pages.
use crate::*;
use std::{
    cell::Cell,
    ops::{Deref, DerefMut},
};

pub const DEFAULT_WORKING_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingMemoryObservation {
    pub limit_bytes: u64,
    pub reserved_bytes: u64,
    pub peak_reserved_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryFailure {
    pub requested_bytes: u64,
    pub available_bytes: u64,
    pub limit_bytes: u64,
    pub phase: RunPhase,
    pub input: Option<u64>,
    pub member: Option<u64>,
}

/// One operation's allocation authority. No global allocator or implicit growth.
pub struct WorkingMemory {
    limit: u64,
    used: Cell<u64>,
    peak: Cell<u64>,
    phase: Cell<RunPhase>,
}
impl WorkingMemory {
    pub fn new(limit: u64) -> Result<Self> {
        if limit == 0 {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "working memory must be positive",
            ));
        }
        Ok(Self {
            limit,
            used: Cell::new(0),
            peak: Cell::new(0),
            phase: Cell::new(RunPhase::Starting),
        })
    }
    fn observe_phase(&self, phase: RunPhase) {
        self.phase.set(phase);
    }
    pub fn used(&self) -> u64 {
        self.used.get()
    }
    pub fn peak(&self) -> u64 {
        self.peak.get()
    }
    pub fn observation(&self) -> WorkingMemoryObservation {
        WorkingMemoryObservation {
            limit_bytes: self.limit,
            reserved_bytes: self.used(),
            peak_reserved_bytes: self.peak(),
        }
    }
    pub fn reserve(&self, bytes: u64, position: RunPosition) -> Result<MemoryReservation<'_>> {
        self.observe_phase(position.phase);
        let available = self.limit - self.used.get();
        if bytes > available {
            let mut error = Error::new(
                ErrorCode::ResourceLimited,
                format!(
                    "working memory exhausted: requested {bytes} bytes; available {available} bytes; phase {:?}; input {:?}; member {:?}",
                    position.phase, position.input, position.member
                ),
            );
            error.memory = Some(MemoryFailure {
                requested_bytes: bytes,
                available_bytes: available,
                limit_bytes: self.limit,
                phase: position.phase,
                input: position.input,
                member: position.member,
            });
            return Err(error);
        }
        let used = self.used.get() + bytes;
        self.used.set(used);
        self.peak.set(self.peak.get().max(used));
        self.observe_phase(position.phase);
        Ok(MemoryReservation {
            memory: self,
            bytes,
        })
    }
    /// Initialized, fallibly allocated scratch. Its capacity and lifetime remain
    /// attached to the reservation; dropping on any path returns the capacity.
    pub fn bytes(&self, size: usize, position: RunPosition) -> Result<ScratchBytes<'_>> {
        let reservation = self.reserve(size as u64, position)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(size).map_err(|_| {
            Error::new(
                ErrorCode::ResourceLimited,
                "host allocation refused working buffer",
            )
        })?;
        bytes.resize(size, 0);
        Ok(ScratchBytes {
            bytes,
            _reservation: reservation,
        })
    }
}

pub struct MemoryReservation<'a> {
    memory: &'a WorkingMemory,
    bytes: u64,
}
impl MemoryReservation<'_> {
    /// Admit `bytes` more before the allocation they account for is made:
    /// a structure that grows entry by entry holds one reservation.
    pub fn grow(&mut self, bytes: u64, position: RunPosition) -> Result<()> {
        let mut more = self.memory.reserve(bytes, position)?;
        // The admitted bytes move into this reservation.
        self.bytes += std::mem::take(&mut more.bytes);
        Ok(())
    }
    /// The bytes this reservation admits.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}
impl Drop for MemoryReservation<'_> {
    fn drop(&mut self) {
        self.memory.used.set(self.memory.used.get() - self.bytes);
        self.memory.observe_phase(self.memory.phase.get());
    }
}

/// Scratch cannot be detached from its accounting or outlive its operation.
///
/// ```compile_fail
/// use blobray_domain::{WorkingMemory, RunPosition};
/// let bytes = { let memory = WorkingMemory::new(8).unwrap();
///     memory.bytes(8, RunPosition::default()).unwrap() };
/// println!("{}", bytes.len());
/// ```
pub struct ScratchBytes<'a> {
    bytes: Vec<u8>,
    _reservation: MemoryReservation<'a>,
}
impl Deref for ScratchBytes<'_> {
    type Target = [u8];
    #[inline]
    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}
impl DerefMut for ScratchBytes<'_> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn index_growth_admits_both_buffers_and_preserves_data_on_failure() {
        let memory = WorkingMemory::new(23).unwrap();
        let mut index = AdmittedVec::new(&memory);
        for n in 0u8..8 {
            index.push(n, RunPosition::default()).unwrap();
        }
        assert_eq!(
            index.push(8, RunPosition::default()).unwrap_err().code,
            ErrorCode::ResourceLimited
        );
        assert_eq!(&*index, &[0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(memory.used(), 8);
        drop(index);
        assert_eq!(memory.used(), 0);
        let memory = WorkingMemory::new(24).unwrap();
        let mut index = AdmittedVec::new(&memory);
        for n in 0u8..9 {
            index.push(n, RunPosition::default()).unwrap();
        }
        assert_eq!(memory.peak(), 24);
        assert_eq!(memory.used(), 16);
    }
    #[test]
    fn a_grown_reservation_admits_within_the_limit_and_releases_all_at_once() {
        let memory = WorkingMemory::new(16).unwrap();
        let mut reservation = memory.reserve(4, RunPosition::default()).unwrap();
        reservation.grow(8, RunPosition::default()).unwrap();
        assert_eq!((reservation.bytes(), memory.used()), (12, 12));
        let error = reservation.grow(5, RunPosition::default()).unwrap_err();
        assert_eq!(error.code, ErrorCode::ResourceLimited);
        assert_eq!(
            (reservation.bytes(), memory.used(), memory.peak()),
            (12, 12, 12)
        );
        drop(reservation);
        assert_eq!(memory.used(), 0);
    }
    #[test]
    fn overlapping_scopes_are_bounded_and_errors_release_capacity() {
        let memory = WorkingMemory::new(16).unwrap();
        let a = memory.bytes(10, RunPosition::default()).unwrap();
        assert!(memory.bytes(7, RunPosition::default()).is_err());
        {
            let _b = memory.bytes(6, RunPosition::default()).unwrap();
        }
        assert_eq!(memory.used(), 10);
        drop(a);
        assert_eq!(memory.used(), 0);
        assert_eq!(memory.peak(), 16);
    }
}

#[cfg(test)]
mod unwind_tests {
    use super::*;
    #[test]
    fn unwind_releases_scratch_before_next_scope() {
        let memory = WorkingMemory::new(8).unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _bytes = memory.bytes(8, RunPosition::default()).unwrap();
            panic!("phase failed");
        }));
        assert!(result.is_err());
        assert_eq!(memory.used(), 0);
        assert!(memory.bytes(8, RunPosition::default()).is_ok());
        assert_eq!(memory.used(), 0);
        assert_eq!(
            memory
                .reserve(u64::MAX, RunPosition::default())
                .err()
                .unwrap()
                .memory
                .unwrap()
                .requested_bytes,
            u64::MAX
        );
    }
}

/// Operation-local vector with fallible growth and explicit old/new overlap admission.
pub struct AdmittedVec<'a, T> {
    values: Vec<T>,
    memory: &'a WorkingMemory,
    capacity: Option<MemoryReservation<'a>>,
}
impl<'a, T> AdmittedVec<'a, T> {
    pub fn new(memory: &'a WorkingMemory) -> Self {
        Self {
            values: Vec::new(),
            memory,
            capacity: None,
        }
    }
    pub fn pop(&mut self) -> Option<T> {
        self.values.pop()
    }
    /// Keep the first `length` values; the admitted capacity stays.
    pub fn truncate(&mut self, length: usize) {
        self.values.truncate(length);
    }
    pub fn push(&mut self, value: T, position: RunPosition) -> Result<()> {
        if self.values.len() == self.values.capacity() {
            let count = self.values.len().saturating_mul(2).max(8);
            let bytes = count
                .checked_mul(std::mem::size_of::<T>())
                .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "index capacity overflow"))?;
            let reservation = self.memory.reserve(bytes as u64, position)?;
            let mut replacement = Vec::new();
            replacement
                .try_reserve_exact(count)
                .map_err(|_| Error::new(ErrorCode::ResourceLimited, "index allocation refused"))?;
            replacement.append(&mut self.values);
            self.values = replacement;
            self.capacity = Some(reservation);
        }
        self.values.push(value);
        Ok(())
    }
}
impl<T> Deref for AdmittedVec<'_, T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.values
    }
}
impl<T> DerefMut for AdmittedVec<'_, T> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.values
    }
}
