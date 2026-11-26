//! The core's one space of numbers: every socket, command, neighbor
//! request, echo reply and DHCP message of a brand takes the next number of
//! one counter, which the brand carries from one core to the next.
//!
//! A number is unique for its brand's life, so a handle whose number is no
//! longer its place's names nothing.

/// A number of the core's counter: only [`Serials::next`] gives one, and a
/// place that holds none yet holds [`Serial::UNGIVEN`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct Serial(u64);

impl Serial {
    /// The number of a place that has held nothing: the counter never gives
    /// it, as its first number is the one after it.
    pub(crate) const UNGIVEN: Self = Self(0);
}

/// The counter: the last number it gave.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Serials(Serial);

impl Serials {
    /// A brand's counter before its first number.
    pub(crate) const fn new() -> Self {
        Self(Serial::UNGIVEN)
    }

    /// The next number. The counter wraps after 2^64 numbers, which no
    /// brand's life reaches.
    pub(crate) const fn next(&mut self) -> Serial {
        self.0 = Serial(self.0.0.wrapping_add(1));
        self.0
    }

    /// The next number, given only when `take` accepts it: a refusal leaves
    /// the counter as it was.
    pub(crate) fn next_if(&mut self, take: impl FnOnce(Serial) -> bool) -> bool {
        let next = Serial(self.0.0.wrapping_add(1));
        let taken = take(next);
        if taken {
            self.0 = next;
        }
        taken
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_counter_gives_each_number_once_and_a_refusal_takes_none() {
        let mut serials = Serials::new();
        let first = serials.next();
        assert_ne!(first, Serial::UNGIVEN);
        let mut offered = None;
        assert!(!serials.next_if(|serial| {
            offered = Some(serial);
            false
        }));
        let second = serials.next();
        assert_eq!(offered, Some(second));
        assert!(second > first);
        assert!(serials.next_if(|serial| serial > second));
        assert!(serials.next() > second);
    }
}
