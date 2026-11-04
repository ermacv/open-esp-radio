//! Expected terminal identities, independent of their arrival order.
//!
//! A ticket's public index is `u8`, so four words cover its entire domain
//! without an allocation or a fallible capacity increase. The control
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
    PairwiseInstall,
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
            Self::PairwiseInstall => 16,
        }
    }
}

#[derive(Default)]
pub(crate) struct Admissions {
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

    pub(crate) fn insert(&mut self, operation: PendingTerminal) {
        match operation {
            PendingTerminal::Transmit(index) => {
                let (word, mask) = self.place(index);
                *word |= mask;
            }
            PendingTerminal::Control(operation) => self.control |= operation.mask(),
        }
    }

    pub(crate) fn remove(&mut self, operation: PendingTerminal) -> bool {
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
}
