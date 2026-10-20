//! Receive reorder banks with caller-selected geometry and interface identity.

use super::{RxReorderBuffer, RxReorderError, RxReorderRelease};
use oer_ieee80211_wire::sequence::SequenceNumber;

/// Identity of one receive ordering space, independent of hardware registers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxReorderIdentity<I> {
    pub bank: u8,
    pub interface: I,
    pub peer: [u8; 6],
    pub tid: u8,
}

/// The sequence window admitted by the receive agreement owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxReorderAgreement<I> {
    pub identity: RxReorderIdentity<I>,
    pub window: u16,
    pub starting_sequence: SequenceNumber,
}

impl<I: Copy> RxReorderAgreement<I> {
    pub const fn identity(self) -> RxReorderIdentity<I> {
        self.identity
    }
}

/// Receive agreement edges delivered to the independent reorder owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxReorderCommand<I> {
    Start(RxReorderAgreement<I>),
    Stop(RxReorderIdentity<I>),
    StopInterface(I),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxReorderCommandError<I> {
    Full(RxReorderCommand<I>),
}

/// Role-neutral owner of a bounded set of software reorder sequence spaces.
///
/// Hardware-bank identity and its reorder state cannot be updated separately.
/// Frame backing and gap timers deliberately remain outside this type because
/// they belong to the executor/integration memory policy.
pub struct RxReorderBanks<I: Copy + Eq, const BANKS: usize, const WINDOW: usize, const SLOTS: usize>
{
    identities: [Option<RxReorderIdentity<I>>; BANKS],
    states: [Option<RxReorderBuffer<WINDOW, SLOTS>>; BANKS],
}

impl<I: Copy + Eq, const BANKS: usize, const WINDOW: usize, const SLOTS: usize>
    RxReorderBanks<I, BANKS, WINDOW, SLOTS>
{
    pub const fn new() -> Self {
        Self {
            identities: [None; BANKS],
            states: [const { None }; BANKS],
        }
    }

    #[inline(always)]
    pub fn find(&self, interface: I, peer: [u8; 6], tid: u8) -> Option<usize> {
        self.identities
            .iter()
            .enumerate()
            .find_map(|(bank, identity)| {
                identity
                    .is_some_and(|identity| {
                        identity.interface == interface
                            && identity.peer == peer
                            && identity.tid == tid
                    })
                    .then_some(bank)
            })
    }

    pub fn identity(&self, bank: usize) -> Option<RxReorderIdentity<I>> {
        self.identities.get(bank).copied().flatten()
    }

    pub fn state(&self, bank: usize) -> Option<&RxReorderBuffer<WINDOW, SLOTS>> {
        self.states.get(bank).and_then(Option::as_ref)
    }

    pub fn state_mut(&mut self, bank: usize) -> Option<&mut RxReorderBuffer<WINDOW, SLOTS>> {
        self.states.get_mut(bank).and_then(Option::as_mut)
    }

    /// Replace the sequence space assigned to the negotiated physical bank.
    /// Any frames retained by the previous generation are returned to their
    /// integration owner before the new identity becomes observable.
    pub fn start(
        &mut self,
        agreement: RxReorderAgreement<I>,
    ) -> Result<Option<RxReorderRelease<WINDOW>>, RxReorderError> {
        let bank = usize::from(agreement.identity.bank);
        let state = RxReorderBuffer::new(agreement.starting_sequence, agreement.window)?;
        let (Some(identity), Some(destination)) =
            (self.identities.get_mut(bank), self.states.get_mut(bank))
        else {
            return Err(RxReorderError::InvalidBank(agreement.identity.bank));
        };
        let released = destination.take().map(|mut previous| previous.stop());
        *identity = Some(agreement.identity());
        *destination = Some(state);
        Ok(released)
    }

    /// Stop only the identified peer/TID on its interface. A teardown for a
    /// different identity cannot stop a replacement occupying the bank.
    pub fn stop(&mut self, identity: RxReorderIdentity<I>) -> Option<RxReorderRelease<WINDOW>> {
        let bank = usize::from(identity.bank);
        if self.identity(bank) != Some(identity) {
            return None;
        }
        self.stop_bank(bank)
    }

    pub fn stop_bank(&mut self, bank: usize) -> Option<RxReorderRelease<WINDOW>> {
        *self.identities.get_mut(bank)? = None;
        self.states
            .get_mut(bank)?
            .take()
            .map(|mut state| state.stop())
    }

    pub fn occupied(&self) -> u32 {
        self.states
            .iter()
            .flatten()
            .map(RxReorderBuffer::occupied)
            .sum()
    }
}

impl<I: Copy + Eq, const BANKS: usize, const WINDOW: usize, const SLOTS: usize> Default
    for RxReorderBanks<I, BANKS, WINDOW, SLOTS>
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_ack::RxReorderMpdu;
    use oer_ieee80211_port::VifRole;

    fn agreement(interface: VifRole, peer: u8, bank: u8) -> RxReorderAgreement<VifRole> {
        RxReorderAgreement {
            identity: RxReorderIdentity {
                bank,
                interface,
                peer: [peer; 6],
                tid: 0,
            },
            window: 8,
            starting_sequence: SequenceNumber::new(10).unwrap(),
        }
    }

    #[test]
    fn replacement_returns_retained_frames_and_teardown_checks_interface_and_peer() {
        let mut banks = RxReorderBanks::<_, 2, 64, 34>::new();
        let old = agreement(VifRole::Station, 2, 0);
        let replacement = agreement(VifRole::AccessPoint, 2, 0);
        banks.start(old).unwrap();
        let frame = RxReorderMpdu {
            sequence: SequenceNumber::new(12).unwrap(),
            slot: 3,
        };
        assert!(banks.state_mut(0).unwrap().ingest(frame).unwrap().buffered);
        let returned = banks.start(replacement).unwrap().unwrap();
        assert_eq!(returned.iter().collect::<std::vec::Vec<_>>(), [frame]);
        assert!(banks.stop(old.identity).is_none());
        assert_eq!(banks.find(VifRole::Station, [2; 6], 0), None);
        assert_eq!(banks.find(VifRole::AccessPoint, [2; 6], 0), Some(0));
        assert!(banks.stop(replacement.identity).is_some());
        assert_eq!(banks.occupied(), 0);
    }

    #[test]
    fn an_external_bank_index_never_panics_or_changes_another_window() {
        let mut banks = RxReorderBanks::<_, 2, 64, 34>::new();
        let active = agreement(VifRole::Station, 2, 0);
        banks.start(active).unwrap();
        assert!(matches!(
            banks.start(agreement(VifRole::AccessPoint, 4, 255)),
            Err(RxReorderError::InvalidBank(255))
        ));
        assert!(banks.state(usize::MAX).is_none());
        assert!(banks.state_mut(usize::MAX).is_none());
        assert!(banks.identity(usize::MAX).is_none());
        assert!(banks.stop_bank(usize::MAX).is_none());
        assert_eq!(banks.identity(0), Some(active.identity));
    }
}
