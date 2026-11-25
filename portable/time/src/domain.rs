//! Checked time of one named clock, in its epoch.
//!
//! A clock's domain `D` is an empty type its owner declares. Instants of
//! different domains cannot be compared or subtracted. The owner validates
//! any additional interface, instance or generation identity. Raw
//! microseconds of one domain establish no relation to another's; conversion
//! belongs to the clock relations.
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

/// An instant of the clock of domain `D`, in microseconds of its epoch.
///
/// The coordinate alone establishes no relation to another clock. The owner validates interface, instance and
/// generation identity where needed.
///
/// Ordering and elapsed-time arithmetic require the same domain, without
/// requiring the domain marker itself to implement `Copy` or `Ord`:
///
/// ```
/// use oer_portable_time::{DomainDuration, DomainInstant};
///
/// enum Radio {}
/// let earlier = DomainInstant::<Radio>::from_micros(10);
/// let later = DomainInstant::<Radio>::from_micros(30);
/// assert!(earlier < later);
/// assert_eq!(later.checked_duration_since(earlier), Some(DomainDuration::from_micros(20)));
/// ```
///
/// Different domains cannot be ordered:
///
/// ```compile_fail,E0308
/// use oer_portable_time::DomainInstant;
///
/// enum FirstRadio {}
/// enum SecondRadio {}
/// let first = DomainInstant::<FirstRadio>::from_micros(10);
/// let second = DomainInstant::<SecondRadio>::from_micros(30);
/// let _ = first < second;
/// ```
#[repr(transparent)]
pub struct DomainInstant<D> {
    micros: u64,
    domain: PhantomData<fn() -> D>,
}

impl<D> DomainInstant<D> {
    /// The epoch of the domain's clock.
    pub const EPOCH: Self = Self::from_micros(0);

    /// Construct a coordinate from microseconds in the domain's epoch.
    pub const fn from_micros(micros: u64) -> Self {
        Self {
            micros,
            domain: PhantomData,
        }
    }

    /// The coordinate in microseconds, for formula or representation
    /// boundaries.
    pub const fn as_micros(self) -> u64 {
        self.micros
    }

    /// The instant `duration` later, or `None` on overflow.
    pub const fn checked_add(self, duration: DomainDuration<D>) -> Option<Self> {
        match self.micros.checked_add(duration.micros) {
            Some(micros) => Some(Self::from_micros(micros)),
            None => None,
        }
    }

    /// The instant `duration` earlier, or `None` before epoch zero.
    pub const fn checked_sub(self, duration: DomainDuration<D>) -> Option<Self> {
        match self.micros.checked_sub(duration.micros) {
            Some(micros) => Some(Self::from_micros(micros)),
            None => None,
        }
    }

    /// The span from `earlier` to `self`, or `None` when `earlier` is later.
    /// Equal instants yield a zero duration.
    ///
    /// Subtraction across domains is rejected:
    ///
    /// ```compile_fail,E0308
    /// use oer_portable_time::DomainInstant;
    ///
    /// enum FirstRadio {}
    /// enum SecondRadio {}
    /// let earlier = DomainInstant::<FirstRadio>::from_micros(10);
    /// let later = DomainInstant::<SecondRadio>::from_micros(30);
    /// let _ = later.checked_duration_since(earlier);
    /// ```
    pub const fn checked_duration_since(self, earlier: Self) -> Option<DomainDuration<D>> {
        match self.micros.checked_sub(earlier.micros) {
            Some(micros) => Some(DomainDuration::from_micros(micros)),
            None => None,
        }
    }
}

impl<D> Clone for DomainInstant<D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D> Copy for DomainInstant<D> {}

impl<D> PartialEq for DomainInstant<D> {
    fn eq(&self, other: &Self) -> bool {
        self.micros == other.micros
    }
}

impl<D> Eq for DomainInstant<D> {}

impl<D> PartialOrd for DomainInstant<D> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<D> Ord for DomainInstant<D> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.micros.cmp(&other.micros)
    }
}

impl<D> Hash for DomainInstant<D> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.micros.hash(state);
    }
}

impl<D> fmt::Debug for DomainInstant<D> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("DomainInstant")
            .field(&self.micros)
            .finish()
    }
}

/// A non-negative span of the domain `D`, from zero through `u64::MAX`
/// microseconds.
///
/// A span belongs to the domain whose instants it separates, as
/// [`DomainInstant`] does: it adds only to an instant of that domain and
/// combines only with spans of that domain. A conversion exists only where a
/// clock relation between the two domains is defined. Checked arithmetic
/// rejects unrepresentable results instead of wrapping or clamping.
///
/// ```
/// use oer_portable_time::{DomainDuration, DomainInstant};
///
/// enum Radio {}
/// let start = DomainInstant::<Radio>::from_micros(10);
/// let span = DomainDuration::<Radio>::from_micros(20);
/// assert_eq!(start.checked_add(span), Some(DomainInstant::from_micros(30)));
/// ```
///
/// A span of one domain does not move an instant of another:
///
/// ```compile_fail,E0308
/// use oer_portable_time::{DomainDuration, DomainInstant};
///
/// enum FirstRadio {}
/// enum SecondRadio {}
/// let start = DomainInstant::<FirstRadio>::from_micros(10);
/// let _ = start.checked_add(DomainDuration::<SecondRadio>::from_micros(20));
/// ```
///
/// Nor do spans of two domains combine:
///
/// ```compile_fail,E0308
/// use oer_portable_time::DomainDuration;
///
/// enum FirstRadio {}
/// enum SecondRadio {}
/// let first = DomainDuration::<FirstRadio>::from_micros(10);
/// let _ = first.checked_add(DomainDuration::<SecondRadio>::from_micros(20));
/// ```
#[repr(transparent)]
pub struct DomainDuration<D> {
    micros: u64,
    domain: PhantomData<fn() -> D>,
}

impl<D> DomainDuration<D> {
    /// The empty span.
    pub const ZERO: Self = Self::from_micros(0);

    /// Construct a duration from microseconds.
    pub const fn from_micros(micros: u64) -> Self {
        Self {
            micros,
            domain: PhantomData,
        }
    }

    /// Construct a duration from milliseconds; every `u32` fits.
    pub const fn from_millis(millis: u32) -> Self {
        // A `u32` of milliseconds is below 2^42 microseconds.
        Self::from_micros((millis as u64).saturating_mul(1_000))
    }

    /// Construct a duration from seconds; every `u32` fits.
    pub const fn from_secs(secs: u32) -> Self {
        // A `u32` of seconds is below 2^52 microseconds.
        Self::from_micros((secs as u64).saturating_mul(1_000_000))
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

impl<D> Clone for DomainDuration<D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D> Copy for DomainDuration<D> {}

impl<D> Default for DomainDuration<D> {
    fn default() -> Self {
        Self::ZERO
    }
}

impl<D> PartialEq for DomainDuration<D> {
    fn eq(&self, other: &Self) -> bool {
        self.micros == other.micros
    }
}

impl<D> Eq for DomainDuration<D> {}

impl<D> PartialOrd for DomainDuration<D> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<D> Ord for DomainDuration<D> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.micros.cmp(&other.micros)
    }
}

impl<D> Hash for DomainDuration<D> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.micros.hash(state);
    }
}

impl<D> fmt::Debug for DomainDuration<D> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("DomainDuration")
            .field(&self.micros)
            .finish()
    }
}

/// A span of the domain `D` of at least one microsecond: the duration
/// of a window that cannot be empty.
#[repr(transparent)]
pub struct NonZeroDomainDuration<D> {
    micros: NonZeroU64,
    domain: PhantomData<fn() -> D>,
}

impl<D> NonZeroDomainDuration<D> {
    /// Construct a duration from a non-zero count of microseconds.
    pub const fn from_micros(micros: NonZeroU64) -> Self {
        Self {
            micros,
            domain: PhantomData,
        }
    }

    /// The span, or `None` when it is zero.
    pub const fn new(duration: DomainDuration<D>) -> Option<Self> {
        match NonZeroU64::new(duration.micros) {
            Some(micros) => Some(Self::from_micros(micros)),
            None => None,
        }
    }

    /// The span as a possibly-zero duration.
    pub const fn get(self) -> DomainDuration<D> {
        DomainDuration::from_micros(self.micros.get())
    }

    /// The sum, or `None` past `u64::MAX` microseconds.
    pub const fn checked_add(self, other: DomainDuration<D>) -> Option<Self> {
        match self.micros.checked_add(other.micros) {
            Some(micros) => Some(Self::from_micros(micros)),
            None => None,
        }
    }
}

impl<D> Clone for NonZeroDomainDuration<D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D> Copy for NonZeroDomainDuration<D> {}

impl<D> PartialEq for NonZeroDomainDuration<D> {
    fn eq(&self, other: &Self) -> bool {
        self.micros == other.micros
    }
}

impl<D> Eq for NonZeroDomainDuration<D> {}

impl<D> PartialOrd for NonZeroDomainDuration<D> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<D> Ord for NonZeroDomainDuration<D> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.micros.cmp(&other.micros)
    }
}

impl<D> Hash for NonZeroDomainDuration<D> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.micros.hash(state);
    }
}

impl<D> fmt::Debug for NonZeroDomainDuration<D> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("NonZeroDomainDuration")
            .field(&self.micros)
            .finish()
    }
}

/// The image's monotonic clock: the one clock every executor task waits on.
pub enum Monotonic {}

/// A point on the image's monotonic time, in microseconds since the epoch
/// of its [`Clock`](crate::Clock).
///
/// An instant of another domain is not monotonic time:
///
/// ```compile_fail,E0308
/// use oer_portable_time::{DomainInstant, Instant};
///
/// enum Radio {}
/// let _: Instant = DomainInstant::<Radio>::from_micros(10);
/// ```
pub type Instant = DomainInstant<Monotonic>;

/// A non-negative span of monotonic time, in microseconds.
pub type Duration = DomainDuration<Monotonic>;

/// What only a deadline may use. The other domains' arithmetic never
/// saturates:
///
/// ```compile_fail,E0599
/// use oer_portable_time::{DomainDuration, DomainInstant};
///
/// enum Radio {}
/// let start = DomainInstant::<Radio>::from_micros(10);
/// let _ = start.saturating_add(DomainDuration::from_micros(20));
/// ```
impl DomainInstant<Monotonic> {
    /// The deadline `duration` later, or the end of representable time when
    /// it lies past it: such a deadline never comes, and `u64` microseconds
    /// outlast any image. Use it for deadlines, never to measure time.
    pub const fn saturating_add(self, duration: Duration) -> Self {
        Self::from_micros(self.micros.saturating_add(duration.micros))
    }

    /// The time from `earlier` to `self`, zero when `earlier` is later.
    ///
    /// ```compile_fail,E0599
    /// use oer_portable_time::DomainInstant;
    ///
    /// enum Radio {}
    /// let start = DomainInstant::<Radio>::from_micros(10);
    /// let _ = start.saturating_duration_since(start);
    /// ```
    pub const fn saturating_duration_since(self, earlier: Self) -> Duration {
        Duration::from_micros(self.micros.saturating_sub(earlier.micros))
    }
}

impl DomainDuration<Monotonic> {
    /// The difference, zero when `other` is longer.
    ///
    /// ```compile_fail,E0599
    /// use oer_portable_time::DomainDuration;
    ///
    /// enum Radio {}
    /// let span = DomainDuration::<Radio>::from_micros(10);
    /// let _ = span.saturating_sub(span);
    /// ```
    pub const fn saturating_sub(self, other: Self) -> Self {
        Self::from_micros(self.micros.saturating_sub(other.micros))
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
/// on the clock of domain `D`.
///
/// Construction validates the exclusive endpoint against the full epoch. A valid window's [`end`](Self::end) is always representable. Its
/// duration is a span of the same domain:
///
/// ```compile_fail,E0308
/// use oer_portable_time::{DomainDuration, DomainInstant, DomainWindow};
///
/// enum FirstRadio {}
/// enum SecondRadio {}
/// let _ = DomainWindow::new(
///     DomainInstant::<FirstRadio>::from_micros(10),
///     DomainDuration::<SecondRadio>::from_micros(20),
/// );
/// ```
pub struct DomainWindow<D> {
    start: DomainInstant<D>,
    end: DomainInstant<D>,
}

impl<D> DomainWindow<D> {
    /// Reserve `duration` from `start`.
    ///
    /// Zero duration returns [`WindowError::Empty`]; an unrepresentable
    /// exclusive endpoint returns [`WindowError::Overflow`].
    pub const fn new(
        start: DomainInstant<D>,
        duration: DomainDuration<D>,
    ) -> Result<Self, WindowError> {
        if duration.micros == 0 {
            return Err(WindowError::Empty);
        }
        let end = match start.checked_add(duration) {
            Some(end) => end,
            None => return Err(WindowError::Overflow),
        };
        Ok(Self { start, end })
    }

    /// Reserve a non-zero `duration` from `start`, or `None` when the
    /// exclusive endpoint lies past the epoch: the only way such a window can
    /// fail.
    pub const fn nonempty(
        start: DomainInstant<D>,
        duration: NonZeroDomainDuration<D>,
    ) -> Option<Self> {
        match Self::new(start, duration.get()) {
            Ok(window) => Some(window),
            Err(_) => None,
        }
    }

    /// The same window, starting `lead` earlier and ending where it ends, or
    /// `None` when the earlier start precedes the epoch: the only way it can
    /// fail, since the end stays representable.
    pub const fn extended_before(self, lead: DomainDuration<D>) -> Option<Self> {
        match self.start.checked_sub(lead) {
            Some(start) => Some(Self {
                start,
                end: self.end,
            }),
            None => None,
        }
    }

    /// First reserved instant.
    pub const fn start(self) -> DomainInstant<D> {
        self.start
    }

    /// Reserved span. The end lies past the start, so it never wraps.
    pub const fn duration(self) -> DomainDuration<D> {
        DomainDuration::from_micros(self.end.micros.wrapping_sub(self.start.micros))
    }

    /// First instant after the window.
    pub const fn end(self) -> DomainInstant<D> {
        self.end
    }

    /// Whether the two windows share an instant. Touching windows do not.
    pub const fn overlaps(self, other: Self) -> bool {
        self.start.micros < other.end().micros && other.start.micros < self.end().micros
    }
}

impl<D> Clone for DomainWindow<D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D> Copy for DomainWindow<D> {}

impl<D> PartialEq for DomainWindow<D> {
    fn eq(&self, other: &Self) -> bool {
        self.start == other.start && self.end == other.end
    }
}

impl<D> Eq for DomainWindow<D> {}

impl<D> Hash for DomainWindow<D> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.start.hash(state);
        self.end.hash(state);
    }
}

impl<D> fmt::Debug for DomainWindow<D> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DomainWindow")
            .field("start", &self.start)
            .field("end", &self.end)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::{DomainDuration, DomainInstant, DomainWindow, NonZeroDomainDuration, WindowError};
    use core::num::NonZeroU64;

    /// A test port's domain.
    enum Port {}

    const fn instant(micros: u64) -> DomainInstant<Port> {
        DomainInstant::from_micros(micros)
    }

    const fn duration(micros: u64) -> DomainDuration<Port> {
        DomainDuration::from_micros(micros)
    }

    fn window(start: u64, micros: u64) -> DomainWindow<Port> {
        DomainWindow::new(instant(start), duration(micros)).unwrap()
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
    fn checked_arithmetic_is_available_in_const_contexts() {
        const SUM: Option<DomainDuration<Port>> = duration(10).checked_add(duration(20));
        const DIFFERENCE: Option<DomainDuration<Port>> = duration(20).checked_sub(duration(10));
        const PRODUCT: Option<DomainDuration<Port>> = duration(10).checked_mul(3);
        const LATER: Option<DomainInstant<Port>> = instant(10).checked_add(duration(20));
        const EARLIER: Option<DomainInstant<Port>> = instant(20).checked_sub(duration(10));
        const ELAPSED: Option<DomainDuration<Port>> =
            instant(u64::MAX).checked_duration_since(instant(0));
        const WINDOW: Result<DomainWindow<Port>, WindowError> =
            DomainWindow::new(instant(0), duration(u64::MAX));
        const END: DomainInstant<Port> = match WINDOW {
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
                DomainWindow::new(instant(start), duration(0)),
                Err(WindowError::Empty)
            );
        }
        for (start, micros) in [(u64::MAX, 1), (u64::MAX - 1, 2), (1, u64::MAX)] {
            assert_eq!(
                DomainWindow::new(instant(start), duration(micros)),
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
        assert_eq!(DomainDuration::<Port>::default(), DomainDuration::ZERO);
    }

    #[test]
    fn a_nonempty_window_fails_only_past_the_epoch() {
        enum D {}
        let one = NonZeroDomainDuration::from_micros(NonZeroU64::MIN);
        assert_eq!(
            NonZeroDomainDuration::new(DomainDuration::<D>::from_micros(0)),
            None
        );
        assert_eq!(
            NonZeroDomainDuration::new(DomainDuration::from_micros(1)),
            Some(one)
        );
        let last = DomainWindow::<D>::nonempty(DomainInstant::from_micros(u64::MAX - 1), one)
            .expect("ends at the last instant");
        assert_eq!(last.end().as_micros(), u64::MAX);
        assert!(DomainWindow::<D>::nonempty(DomainInstant::from_micros(u64::MAX), one).is_none());
        assert_eq!(
            one.checked_add(DomainDuration::from_micros(u64::MAX - 1))
                .map(|d| d.get().as_micros()),
            Some(u64::MAX)
        );
        assert_eq!(one.checked_add(DomainDuration::from_micros(u64::MAX)), None);
    }

    #[test]
    fn extending_a_window_before_keeps_its_end_and_fails_only_before_the_epoch() {
        enum D {}
        let whole = DomainWindow::<D>::new(
            DomainInstant::from_micros(10),
            DomainDuration::from_micros(u64::MAX - 10),
        )
        .unwrap();
        let extended = whole
            .extended_before(DomainDuration::from_micros(10))
            .expect("starts at the epoch's first instant");
        assert_eq!(extended.start().as_micros(), 0);
        assert_eq!(extended.end().as_micros(), u64::MAX);
        assert_eq!(extended.duration().as_micros(), u64::MAX);
        assert!(
            whole
                .extended_before(DomainDuration::from_micros(11))
                .is_none()
        );
        let same = whole
            .extended_before(DomainDuration::from_micros(0))
            .unwrap();
        assert_eq!(same.start().as_micros(), 10);
        assert_eq!(same.end().as_micros(), u64::MAX);
    }

    #[test]
    fn a_window_is_its_start_and_its_end() {
        use std::{
            collections::hash_map::DefaultHasher,
            hash::{Hash, Hasher},
        };
        let hash = |window: DomainWindow<Port>| {
            let mut hasher = DefaultHasher::new();
            window.hash(&mut hasher);
            hasher.finish()
        };
        let whole = window(10, 5);
        assert_eq!(whole, window(10, 5));
        assert_eq!(hash(whole), hash(window(10, 5)));
        // The same start, the same end, and neither.
        for other in [window(10, 6), window(9, 6), window(20, 5)] {
            assert_ne!(whole, other);
            assert_ne!(hash(whole), hash(other));
        }
    }

    #[test]
    fn a_value_prints_its_microseconds() {
        use std::format;
        assert_eq!(format!("{:?}", instant(7)), "DomainInstant(7)");
        assert_eq!(format!("{:?}", duration(8)), "DomainDuration(8)");
        assert_eq!(
            format!(
                "{:?}",
                NonZeroDomainDuration::<Port>::from_micros(NonZeroU64::MIN)
            ),
            "NonZeroDomainDuration(1)"
        );
        assert_eq!(
            format!("{:?}", window(1, 2)),
            "DomainWindow { start: DomainInstant(1), end: DomainInstant(3) }"
        );
    }

    #[test]
    #[expect(clippy::clone_on_copy, reason = "the clone is what is checked")]
    fn a_span_is_compared_hashed_and_cloned_by_its_microseconds() {
        use core::cmp::Ordering;
        use std::{
            collections::hash_map::DefaultHasher,
            hash::{Hash, Hasher},
        };
        fn hash<T: Hash>(value: T) -> u64 {
            let mut hasher = DefaultHasher::new();
            value.hash(&mut hasher);
            hasher.finish()
        }
        let nonzero = |micros| NonZeroDomainDuration::new(duration(micros)).unwrap();
        assert_eq!(duration(3).clone(), duration(3));
        assert_ne!(duration(3), duration(4));
        assert_eq!(duration(3).partial_cmp(&duration(4)), Some(Ordering::Less));
        assert_ne!(hash(duration(3)), hash(duration(4)));
        assert_ne!(nonzero(3), nonzero(4));
        assert_eq!(nonzero(4).partial_cmp(&nonzero(3)), Some(Ordering::Greater));
        assert_ne!(hash(nonzero(3)), hash(nonzero(4)));
    }
}
