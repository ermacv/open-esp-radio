//! Expected terminal identities, independent of their arrival order.
//!
//! A ticket's public index is `u8`, so four words cover its entire domain
//! without an allocation or a fallible capacity increase. The other four
//! operations each consume a unique affine permission. This is accounting
//! of a backend's external promises, not another owner of those permissions.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingTerminal {
    Transmit(u8),
    Control(ControlTerminal),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlTerminal {
    Configure,
    PairwiseRemoval,
    GroupChange,
    GateClosed,
}

impl ControlTerminal {
    const fn mask(self) -> u8 {
        match self {
            Self::Configure => 1,
            Self::PairwiseRemoval => 2,
            Self::GroupChange => 4,
            Self::GateClosed => 8,
        }
    }
}

#[derive(Default)]
pub(super) struct Admissions {
    transmit: [u64; 4],
    control: u8,
}

impl Admissions {
    fn place(&mut self, index: u8) -> (&mut u64, u64) {
        let [first, second, third, fourth] = &mut self.transmit;
        let word = match index {
            0..=63 => first,
            64..=127 => second,
            128..=191 => third,
            192..=255 => fourth,
        };
        (word, 1_u64.rotate_left(u32::from(index)))
    }

    pub(super) fn insert(&mut self, operation: PendingTerminal) {
        match operation {
            PendingTerminal::Transmit(index) => {
                let (word, mask) = self.place(index);
                *word |= mask;
            }
            PendingTerminal::Control(operation) => self.control |= operation.mask(),
        }
    }

    pub(super) fn remove(&mut self, operation: PendingTerminal) -> bool {
        match operation {
            PendingTerminal::Transmit(index) => {
                let (word, mask) = self.place(index);
                let present = *word & mask != 0;
                *word &= !mask;
                present
            }
            PendingTerminal::Control(operation) => {
                let mask = operation.mask();
                let present = self.control & mask != 0;
                self.control &= !mask;
                present
            }
        }
    }

    pub(super) fn first(&self) -> Option<PendingTerminal> {
        for (offset, word) in [0_u8, 64, 128, 192].into_iter().zip(&self.transmit) {
            if *word != 0 {
                return Some(PendingTerminal::Transmit(
                    offset | word.trailing_zeros() as u8,
                ));
            }
        }
        [
            ControlTerminal::Configure,
            ControlTerminal::PairwiseRemoval,
            ControlTerminal::GroupChange,
            ControlTerminal::GateClosed,
        ]
        .into_iter()
        .find(|operation| self.control & operation.mask() != 0)
        .map(PendingTerminal::Control)
    }
}
