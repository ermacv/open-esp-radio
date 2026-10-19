//! Checked time in a radio owner's domain and epoch.
//!
//! Each owner names its domain `D`, an empty type its package declares.
//! Instants of different domains cannot be compared or subtracted. The
//! owner validates any additional interface, instance or generation identity.
//! Raw radio microseconds establish no relation to image-monotonic or
//! wall-clock time; conversion belongs to the existing clock relations.
//!
//! Instants and non-negative durations span the full `u64` microsecond
//! range. Checked arithmetic returns `None` for an unrepresentable result.
//! A window also requires a non-zero duration and a representable exclusive
//! endpoint.

use core::{
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    num::NonZeroU64,
};

/// A microsecond coordinate in the radio domain and epoch owned by `D`.
///
/// The coordinate alone establishes neither an image-monotonic nor a
/// wall-clock relationship. The owner validates interface, instance and
/// generation identity where needed.
///
/// Ordering and elapsed-time arithmetic require the same domain, without
/// requiring the domain marker itself to implement `Copy` or `Ord`:
///
/// ```
/// use crate::{RadioDuration, RadioInstant};
///
/// enum Radio {}
/// let earlier = RadioInstant::<Radio>::from_micros(10);
/// let later = RadioInstant::<Radio>::from_micros(30);
/// assert!(earlier < later);
/// assert_eq!(later.checked_duration_since(earlier), Some(RadioDuration::from_micros(20)));
/// ```
///
/// Different domains cannot be ordered:
///
/// ```compile_fail,E0308
/// use crate::RadioInstant;
///
/// enum FirstRadio {}
/// enum SecondRadio {}
/// let first = RadioInstant::<FirstRadio>::from_micros(10);
/// let second = RadioInstant::<SecondRadio>::from_micros(30);
/// let _ = first < second;
/// ```
#[repr(transparent)]
pub struct RadioInstant<D> {
    micros: u64,
    domain: PhantomData<fn() -> D>,
}

impl<D> RadioInstant<D> {
    /// Construct a coordinate from microseconds in this radio epoch.
    pub const fn from_micros(micros: u64) -> Self {
        Self {
            micros,
            domain: PhantomData,
        }
    }

    /// The radio coordinate in microseconds, for formula or representation
    /// boundaries.
    pub const fn as_micros(self) -> u64 {
        self.micros
    }

    /// The instant `duration` later, or `None` on overflow.
    pub const fn checked_add(self, duration: RadioDuration<D>) -> Option<Self> {
        match self.micros.checked_add(duration.micros) {
            Some(micros) => Some(Self::from_micros(micros)),
            None => None,
        }
    }

    /// The instant `duration` earlier, or `None` before epoch zero.
    pub const fn checked_sub(self, duration: RadioDuration<D>) -> Option<Self> {
        match self.micros.checked_sub(duration.micros) {
            Some(micros) => Some(Self::from_micros(micros)),
            None => None,
        }
    }

    /// The span from `earlier` to `self`, or `None` when `earlier` is later.
    /// Equal instants yield a zero duration.
    ///
    /// Subtraction across radio domains is rejected:
    ///
    /// ```compile_fail,E0308
    /// use crate::RadioInstant;
    ///
    /// enum FirstRadio {}
    /// enum SecondRadio {}
    /// let earlier = RadioInstant::<FirstRadio>::from_micros(10);
    /// let later = RadioInstant::<SecondRadio>::from_micros(30);
    /// let _ = later.checked_duration_since(earlier);
    /// ```
    pub const fn checked_duration_since(self, earlier: Self) -> Option<RadioDuration<D>> {
        match self.micros.checked_sub(earlier.micros) {
            Some(micros) => Some(RadioDuration::from_micros(micros)),
            None => None,
        }
    }
}

impl<D> Clone for RadioInstant<D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D> Copy for RadioInstant<D> {}

impl<D> PartialEq for RadioInstant<D> {
    fn eq(&self, other: &Self) -> bool {
        self.micros == other.micros
    }
}

impl<D> Eq for RadioInstant<D> {}

impl<D> PartialOrd for RadioInstant<D> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<D> Ord for RadioInstant<D> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.micros.cmp(&other.micros)
    }
}

impl<D> Hash for RadioInstant<D> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.micros.hash(state);
    }
}

impl<D> fmt::Debug for RadioInstant<D> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("RadioInstant")
            .field(&self.micros)
            .finish()
    }
}

/// A non-negative span of the radio domain `D`, from zero through
/// `u64::MAX` microseconds.
///
/// A span belongs to the domain whose instants it separates, as
/// [`RadioInstant`] does: it adds only to an instant of that domain and
/// combines only with spans of that domain. It is not a monotonic
/// [`Duration`](crate::Duration); a conversion exists only where a clock
/// relation between the two domains is defined. Arithmetic rejects
/// unrepresentable results instead of wrapping or clamping.
///
/// ```
/// use crate::{RadioDuration, RadioInstant};
///
/// enum Radio {}
/// let start = RadioInstant::<Radio>::from_micros(10);
/// let span = RadioDuration::<Radio>::from_micros(20);
/// assert_eq!(start.checked_add(span), Some(RadioInstant::from_micros(30)));
/// ```
///
/// A span of one domain does not move an instant of another:
///
/// ```compile_fail,E0308
/// use crate::{RadioDuration, RadioInstant};
///
/// enum FirstRadio {}
/// enum SecondRadio {}
/// let start = RadioInstant::<FirstRadio>::from_micros(10);
/// let _ = start.checked_add(RadioDuration::<SecondRadio>::from_micros(20));
/// ```
///
/// Nor do spans of two domains combine:
///
/// ```compile_fail,E0308
/// use crate::RadioDuration;
///
/// enum FirstRadio {}
/// enum SecondRadio {}
/// let first = RadioDuration::<FirstRadio>::from_micros(10);
/// let _ = first.checked_add(RadioDuration::<SecondRadio>::from_micros(20));
/// ```
#[repr(transparent)]
pub struct RadioDuration<D> {
    micros: u64,
    domain: PhantomData<fn() -> D>,
}

impl<D> RadioDuration<D> {
    /// The empty span.
    pub const ZERO: Self = Self::from_micros(0);

    /// Construct a duration from microseconds.
    pub const fn from_micros(micros: u64) -> Self {
        Self {
            micros,
            domain: PhantomData,
        }
    }

    /// Microseconds, for formula or representation boundaries.
    pub const fn as_micros(self) -> u64 {
        self.micros
    }

    /// The sum, or `None` past `u64::MAX` microseconds.
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        match self.micros.checked_add(other.micros) {
            Some(micros) => Some(Self::from_micros(micros)),
            None => None,
        }
    }

    /// The difference, or `None` when `other` is longer.
    pub const fn checked_sub(self, other: Self) -> Option<Self> {
        match self.micros.checked_sub(other.micros) {
            Some(micros) => Some(Self::from_micros(micros)),
            None => None,
        }
    }

    /// The span repeated `count` times, or `None` past `u64::MAX` microseconds.
    pub const fn checked_mul(self, count: u64) -> Option<Self> {
        match self.micros.checked_mul(count) {
            Some(micros) => Some(Self::from_micros(micros)),
            None => None,
        }
    }

    /// What remains of this span after whole `period`s, or `None` for an
    /// empty `period`.
    pub const fn checked_rem(self, period: Self) -> Option<Self> {
        match self.micros.checked_rem(period.micros) {
            Some(micros) => Some(Self::from_micros(micros)),
            None => None,
        }
    }
}

impl<D> Clone for RadioDuration<D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D> Copy for RadioDuration<D> {}

impl<D> Default for RadioDuration<D> {
    fn default() -> Self {
        Self::ZERO
    }
}

impl<D> PartialEq for RadioDuration<D> {
    fn eq(&self, other: &Self) -> bool {
        self.micros == other.micros
    }
}

impl<D> Eq for RadioDuration<D> {}

impl<D> PartialOrd for RadioDuration<D> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<D> Ord for RadioDuration<D> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.micros.cmp(&other.micros)
    }
}

impl<D> Hash for RadioDuration<D> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.micros.hash(state);
    }
}

impl<D> fmt::Debug for RadioDuration<D> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("RadioDuration")
            .field(&self.micros)
            .finish()
    }
}

/// A span of the radio domain `D` of at least one microsecond: the duration
/// of a window that cannot be empty.
#[repr(transparent)]
pub struct NonZeroRadioDuration<D> {
    micros: NonZeroU64,
    domain: PhantomData<fn() -> D>,
}

impl<D> NonZeroRadioDuration<D> {
    /// Construct a duration from a non-zero count of microseconds.
    pub const fn from_micros(micros: NonZeroU64) -> Self {
        Self {
            micros,
            domain: PhantomData,
        }
    }

    /// The span, or `None` when it is zero.
    pub const fn new(duration: RadioDuration<D>) -> Option<Self> {
        match NonZeroU64::new(duration.micros) {
            Some(micros) => Some(Self::from_micros(micros)),
            None => None,
        }
    }

    /// The span as a possibly-zero duration.
    pub const fn get(self) -> RadioDuration<D> {
        RadioDuration::from_micros(self.micros.get())
    }

    /// The sum, or `None` past `u64::MAX` microseconds.
    pub const fn checked_add(self, other: RadioDuration<D>) -> Option<Self> {
        match self.micros.checked_add(other.micros) {
            Some(micros) => Some(Self::from_micros(micros)),
            None => None,
        }
    }
}

impl<D> Clone for NonZeroRadioDuration<D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D> Copy for NonZeroRadioDuration<D> {}

impl<D> PartialEq for NonZeroRadioDuration<D> {
    fn eq(&self, other: &Self) -> bool {
        self.micros == other.micros
    }
}

impl<D> Eq for NonZeroRadioDuration<D> {}

impl<D> PartialOrd for NonZeroRadioDuration<D> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<D> Ord for NonZeroRadioDuration<D> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.micros.cmp(&other.micros)
    }
}

impl<D> Hash for NonZeroRadioDuration<D> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.micros.hash(state);
    }
}

impl<D> fmt::Debug for NonZeroRadioDuration<D> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("NonZeroRadioDuration")
            .field(&self.micros)
            .finish()
    }
}

/// Why a window is not representable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowError {
    /// The window has no duration.
    Empty,
    /// The window ends past the representable epoch.
    Overflow,
}

/// A non-empty interval `[start, start + duration)` reserved for one event
/// on the clock of the port of domain `D`.
///
/// Construction validates the exclusive endpoint against the full radio
/// epoch. A valid window's [`end`](Self::end) is always representable. Its
/// duration is a span of the same domain:
///
/// ```compile_fail,E0308
/// use crate::{RadioDuration, RadioInstant, RadioWindow};
///
/// enum FirstRadio {}
/// enum SecondRadio {}
/// let _ = RadioWindow::new(
///     RadioInstant::<FirstRadio>::from_micros(10),
///     RadioDuration::<SecondRadio>::from_micros(20),
/// );
/// ```
pub struct RadioWindow<D> {
    start: RadioInstant<D>,
    duration: RadioDuration<D>,
}

impl<D> RadioWindow<D> {
    /// Reserve `duration` from `start`.
    ///
    /// Zero duration returns [`WindowError::Empty`]; an unrepresentable
    /// exclusive endpoint returns [`WindowError::Overflow`].
    pub const fn new(
        start: RadioInstant<D>,
        duration: RadioDuration<D>,
    ) -> Result<Self, WindowError> {
        if duration.micros == 0 {
            return Err(WindowError::Empty);
        }
        if start.checked_add(duration).is_none() {
            return Err(WindowError::Overflow);
        }
        Ok(Self { start, duration })
    }

    /// Reserve a non-zero `duration` from `start`, or `None` when the
    /// exclusive endpoint lies past the epoch: the only way such a window can
    /// fail.
    pub const fn nonempty(
        start: RadioInstant<D>,
        duration: NonZeroRadioDuration<D>,
    ) -> Option<Self> {
        match Self::new(start, duration.get()) {
            Ok(window) => Some(window),
            Err(_) => None,
        }
    }

    /// The same window, starting `lead` earlier and ending where it ends, or
    /// `None` when the earlier start precedes the epoch: the only way it can
    /// fail, since the end stays representable.
    pub const fn extended_before(self, lead: RadioDuration<D>) -> Option<Self> {
        match self.start.checked_sub(lead) {
            // `start + duration` is representable, so `lead + duration`,
            // which ends at the same instant from an earlier start, is too.
            Some(start) => Some(Self {
                start,
                duration: RadioDuration::from_micros(self.duration.micros + lead.micros),
            }),
            None => None,
        }
    }

    /// First reserved instant.
    pub const fn start(self) -> RadioInstant<D> {
        self.start
    }

    /// Reserved span.
    pub const fn duration(self) -> RadioDuration<D> {
        self.duration
    }

    /// First instant after the window.
    pub const fn end(self) -> RadioInstant<D> {
        // `new` proved this endpoint representable; the fields are private.
        RadioInstant::from_micros(self.start.micros + self.duration.micros)
    }

    /// Whether the two windows share an instant. Touching windows do not.
    pub const fn overlaps(self, other: Self) -> bool {
        self.start.micros < other.end().micros && other.start.micros < self.end().micros
    }
}

impl<D> Clone for RadioWindow<D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D> Copy for RadioWindow<D> {}

impl<D> PartialEq for RadioWindow<D> {
    fn eq(&self, other: &Self) -> bool {
        self.start == other.start && self.duration == other.duration
    }
}

impl<D> Eq for RadioWindow<D> {}

impl<D> Hash for RadioWindow<D> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.start.hash(state);
        self.duration.hash(state);
    }
}

impl<D> fmt::Debug for RadioWindow<D> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RadioWindow")
            .field("start", &self.start)
            .field("duration", &self.duration)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{NonZeroRadioDuration, RadioDuration, RadioInstant, RadioWindow, WindowError};
    use core::num::NonZeroU64;

    /// A test port's domain.
    enum Port {}

    const fn instant(micros: u64) -> RadioInstant<Port> {
        RadioInstant::from_micros(micros)
    }

    const fn duration(micros: u64) -> RadioDuration<Port> {
        RadioDuration::from_micros(micros)
    }

    fn window(start: u64, micros: u64) -> RadioWindow<Port> {
        RadioWindow::new(instant(start), duration(micros)).unwrap()
    }

    #[test]
    fn durations_and_elapsed_spans_cover_the_full_unsigned_range() {
        for micros in [0, 1, 42, u32::MAX as u64, u32::MAX as u64 + 1, u64::MAX] {
            let span = duration(micros);
            let end = instant(micros);
            assert_eq!(span.as_micros(), micros);
            assert_eq!(instant(0).checked_add(span), Some(end));
            assert_eq!(end.checked_sub(span), Some(instant(0)));
            assert_eq!(end.checked_duration_since(instant(0)), Some(span));
            assert_eq!(end.checked_duration_since(end), Some(duration(0)));
            assert_eq!(end.checked_add(duration(0)), Some(end));
            assert_eq!(end.checked_sub(duration(0)), Some(end));
            if micros != 0 {
                assert_eq!(instant(0).checked_duration_since(end), None);
            }
        }
    }

    #[test]
    fn instant_arithmetic_rejects_the_first_result_outside_the_epoch() {
        assert_eq!(instant(100).checked_add(duration(50)), Some(instant(150)));
        assert_eq!(instant(100).checked_sub(duration(50)), Some(instant(50)));
        assert_eq!(
            instant(100).checked_duration_since(instant(40)),
            Some(duration(60))
        );
        assert_eq!(instant(100).checked_duration_since(instant(101)), None);
        assert_eq!(instant(100).checked_sub(duration(101)), None);
        assert_eq!(instant(0).checked_sub(duration(1)), None);
        assert_eq!(
            instant(u64::MAX - 1).checked_add(duration(1)),
            Some(instant(u64::MAX))
        );
        assert_eq!(instant(u64::MAX - 1).checked_add(duration(2)), None);
        assert_eq!(instant(u64::MAX).checked_add(duration(1)), None);
        assert_eq!(instant(1).checked_add(duration(u64::MAX)), None);
        assert_eq!(instant(u64::MAX - 1).checked_sub(duration(u64::MAX)), None);
    }

    #[test]
    fn duration_sums_accept_exact_limits_and_reject_overflow() {
        for (left, right, expected) in [
            (0, 0, Some(0)),
            (20, 30, Some(50)),
            (u32::MAX as u64, 1, Some(u32::MAX as u64 + 1)),
            (u64::MAX, 0, Some(u64::MAX)),
            (u64::MAX - 1, 1, Some(u64::MAX)),
            (u64::MAX - 1, 2, None),
            (u64::MAX, 1, None),
        ] {
            let expected = expected.map(duration);
            assert_eq!(duration(left).checked_add(duration(right)), expected);
            assert_eq!(duration(right).checked_add(duration(left)), expected);
        }
    }

    #[test]
    fn duration_differences_accept_zero_and_reject_underflow() {
        for (left, right, expected) in [
            (0, 0, Some(0)),
            (50, 20, Some(30)),
            (u32::MAX as u64 + 1, 1, Some(u32::MAX as u64)),
            (u64::MAX, 0, Some(u64::MAX)),
            (u64::MAX, u64::MAX, Some(0)),
            (0, 1, None),
            (u64::MAX - 1, u64::MAX, None),
        ] {
            assert_eq!(
                duration(left).checked_sub(duration(right)),
                expected.map(duration)
            );
        }
    }

    #[test]
    fn duration_products_accept_exact_limits_and_reject_the_next_product() {
        // (2^32 - 1) * (2^32 + 1) is exactly u64::MAX.
        let factor = u32::MAX as u64 + 2;
        for (micros, count, expected) in [
            (0, u64::MAX, Some(0)),
            (u64::MAX, 0, Some(0)),
            (u64::MAX, 1, Some(u64::MAX)),
            (7, 3, Some(21)),
            (u32::MAX as u64 + 1, 1, Some(u32::MAX as u64 + 1)),
            (1, u64::MAX, Some(u64::MAX)),
            (u32::MAX as u64, factor, Some(u64::MAX)),
            (u32::MAX as u64, factor + 1, None),
            (u64::MAX / 2, 2, Some(u64::MAX - 1)),
            (u64::MAX / 2 + 1, 2, None),
            (u64::MAX, 2, None),
        ] {
            assert_eq!(duration(micros).checked_mul(count), expected.map(duration));
        }
    }

    #[test]
    fn same_domain_ordering_needs_no_traits_on_the_domain_marker() {
        let earlier = instant(0);
        let later = instant(u64::MAX);
        let copied = later;
        assert_eq!(copied, later);
        assert!(earlier < later);
        assert_eq!(earlier.min(later), earlier);
        assert_eq!(earlier.max(later), later);
        assert_eq!(later.cmp(&copied), core::cmp::Ordering::Equal);
    }

    #[test]
    fn checked_radio_arithmetic_is_available_in_const_contexts() {
        const SUM: Option<RadioDuration<Port>> = duration(10).checked_add(duration(20));
        const DIFFERENCE: Option<RadioDuration<Port>> = duration(20).checked_sub(duration(10));
        const PRODUCT: Option<RadioDuration<Port>> = duration(10).checked_mul(3);
        const LATER: Option<RadioInstant<Port>> = instant(10).checked_add(duration(20));
        const EARLIER: Option<RadioInstant<Port>> = instant(20).checked_sub(duration(10));
        const ELAPSED: Option<RadioDuration<Port>> =
            instant(u64::MAX).checked_duration_since(instant(0));
        const WINDOW: Result<RadioWindow<Port>, WindowError> =
            RadioWindow::new(instant(0), duration(u64::MAX));
        const END: RadioInstant<Port> = match WINDOW {
            Ok(window) => window.end(),
            Err(_) => panic!("the full-epoch window is representable"),
        };
        assert_eq!(SUM, Some(duration(30)));
        assert_eq!(DIFFERENCE, Some(duration(10)));
        assert_eq!(PRODUCT, Some(duration(30)));
        assert_eq!(LATER, Some(instant(30)));
        assert_eq!(EARLIER, Some(instant(10)));
        assert_eq!(ELAPSED, Some(duration(u64::MAX)));
        assert_eq!(END, instant(u64::MAX));
    }

    #[test]
    fn windows_validate_duration_and_the_exclusive_endpoint() {
        for start in [0, 5, u64::MAX] {
            assert_eq!(
                RadioWindow::new(instant(start), duration(0)),
                Err(WindowError::Empty)
            );
        }
        for (start, micros) in [(u64::MAX, 1), (u64::MAX - 1, 2), (1, u64::MAX)] {
            assert_eq!(
                RadioWindow::new(instant(start), duration(micros)),
                Err(WindowError::Overflow)
            );
        }
        for micros in [1, u32::MAX as u64, u32::MAX as u64 + 1, u64::MAX] {
            let whole = window(0, micros);
            assert_eq!(whole.start(), instant(0));
            assert_eq!(whole.duration(), duration(micros));
            assert_eq!(whole.end(), instant(micros));
            let latest = window(u64::MAX - micros, micros);
            assert_eq!(latest.start(), instant(u64::MAX - micros));
            assert_eq!(latest.duration(), duration(micros));
            assert_eq!(latest.end(), instant(u64::MAX));
        }
    }

    #[test]
    fn half_open_overlap_is_symmetric_including_full_epoch_windows() {
        for (left, right, overlaps) in [
            (window(100, 50), window(100, 50), true),
            (window(100, 50), window(110, 10), true),
            (window(100, 50), window(149, 10), true),
            (window(100, 50), window(90, 20), true),
            (window(100, 50), window(150, 10), false),
            (window(100, 50), window(90, 10), false),
            (window(100, 50), window(151, 10), false),
            (window(0, u64::MAX), window(u64::MAX - 1, 1), true),
            (window(0, u64::MAX - 1), window(u64::MAX - 1, 1), false),
        ] {
            assert_eq!(left.overlaps(right), overlaps);
            assert_eq!(right.overlaps(left), overlaps);
        }
    }

    #[test]
    fn a_remainder_keeps_the_domain_and_refuses_an_empty_period() {
        assert_eq!(duration(25).checked_rem(duration(10)), Some(duration(5)));
        assert_eq!(duration(20).checked_rem(duration(10)), Some(duration(0)));
        assert_eq!(duration(5).checked_rem(duration(0)), None);
        assert_eq!(RadioDuration::<Port>::default(), RadioDuration::ZERO);
    }

    #[test]
    fn a_nonempty_window_fails_only_past_the_epoch() {
        enum D {}
        let one = NonZeroRadioDuration::from_micros(NonZeroU64::MIN);
        assert_eq!(
            NonZeroRadioDuration::new(RadioDuration::<D>::from_micros(0)),
            None
        );
        assert_eq!(
            NonZeroRadioDuration::new(RadioDuration::from_micros(1)),
            Some(one)
        );
        let last = RadioWindow::<D>::nonempty(RadioInstant::from_micros(u64::MAX - 1), one)
            .expect("ends at the last instant");
        assert_eq!(last.end().as_micros(), u64::MAX);
        assert!(RadioWindow::<D>::nonempty(RadioInstant::from_micros(u64::MAX), one).is_none());
        assert_eq!(
            one.checked_add(RadioDuration::from_micros(u64::MAX - 1))
                .map(|d| d.get().as_micros()),
            Some(u64::MAX)
        );
        assert_eq!(one.checked_add(RadioDuration::from_micros(u64::MAX)), None);
    }

    #[test]
    fn extending_a_window_before_keeps_its_end_and_fails_only_before_the_epoch() {
        enum D {}
        let whole = RadioWindow::<D>::new(
            RadioInstant::from_micros(10),
            RadioDuration::from_micros(u64::MAX - 10),
        )
        .unwrap();
        let extended = whole
            .extended_before(RadioDuration::from_micros(10))
            .expect("starts at the epoch's first instant");
        assert_eq!(extended.start().as_micros(), 0);
        assert_eq!(extended.end().as_micros(), u64::MAX);
        assert_eq!(extended.duration().as_micros(), u64::MAX);
        assert!(
            whole
                .extended_before(RadioDuration::from_micros(11))
                .is_none()
        );
        let same = whole
            .extended_before(RadioDuration::from_micros(0))
            .unwrap();
        assert_eq!(same.start().as_micros(), 10);
        assert_eq!(same.end().as_micros(), u64::MAX);
    }
}
