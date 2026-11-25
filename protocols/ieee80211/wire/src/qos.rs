//! Portable IEEE 802.11 priority values.
//!
//! The UP-to-AC mapping is the standard's (IEEE 802.11-2020 Table 10-1). The
//! WMM vendor element parser lives in `extensions::wmm`; the upper MAC owns
//! admission and queue selection using these values, and the Ethernet bridge
//! classifies network frames into them.
//! Checked priorities and categories select cells of exact eight- and
//! four-cell arrays. The selector retains the bounded value and needs no
//! numeric indexing or repeat validation in a caller.

/// Standard WMM access-category identifier carried in ACI bits 5..6.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum WmmAccessCategory {
    #[default]
    BestEffort = 0,
    Background = 1,
    Video = 2,
    Voice = 3,
}

impl WmmAccessCategory {
    /// Every category, in the order of its ACI (`ALL[c as usize] == c`).
    pub const ALL: [Self; 4] = [Self::BestEffort, Self::Background, Self::Video, Self::Voice];

    pub const fn from_aci(aci: u8) -> Option<Self> {
        match aci {
            0 => Some(Self::BestEffort),
            1 => Some(Self::Background),
            2 => Some(Self::Video),
            3 => Some(Self::Voice),
            _ => None,
        }
    }

    /// Select a cell of the exact four-category array without a numeric index.
    pub const fn select<T>(self, entries: &[T; 4]) -> &T {
        let [be, bk, vi, vo] = entries;
        match self {
            Self::BestEffort => be,
            Self::Background => bk,
            Self::Video => vi,
            Self::Voice => vo,
        }
    }
    pub const fn select_mut<T>(self, entries: &mut [T; 4]) -> &mut T {
        let [be, bk, vi, vo] = entries;
        match self {
            Self::BestEffort => be,
            Self::Background => bk,
            Self::Video => vi,
            Self::Voice => vo,
        }
    }
}

/// A traffic identifier, 0 to 15: a user priority's, or a traffic stream's
/// from 8 on.
///
/// SOURCE: IEEE Std 802.11-2020, 9.2.4.5.2 (TID subfield).
///
/// A number is no identifier until it is checked:
///
/// ```compile_fail,E0308
/// use oer_protocols_ieee80211_wire::qos::Tid;
/// let tid: Tid = 16;
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Tid(oer_portable_wire::U8<4>);

impl Tid {
    /// The width of its field.
    pub const BITS: u32 = 4;

    /// `tid`, when it is at most 15.
    pub const fn new(tid: u8) -> Option<Self> {
        match oer_portable_wire::U8::new(tid) {
            Some(tid) => Some(Self(tid)),
            None => None,
        }
    }

    /// The identifier `VALUE`, checked at compile time.
    pub const fn constant<const VALUE: u8>() -> Self {
        Self(oer_portable_wire::U8::constant::<VALUE>())
    }

    /// The identifier.
    pub const fn get(self) -> u8 {
        self.0.get()
    }

    /// The user priority this identifier names; `None` for a traffic
    /// stream's.
    pub const fn user_priority(self) -> Option<WmmUserPriority> {
        WmmUserPriority::new(self.0.get())
    }

    /// Select the cell of this identifier among the exact sixteen,
    /// without a numeric index.
    pub const fn select<T>(self, entries: &[T; 16]) -> &T {
        let [a, b, c, d, e, f, g, h, i, j, k, l, m, n, o, p] = entries;
        match self.0.get() {
            0 => a,
            1 => b,
            2 => c,
            3 => d,
            4 => e,
            5 => f,
            6 => g,
            7 => h,
            8 => i,
            9 => j,
            10 => k,
            11 => l,
            12 => m,
            13 => n,
            14 => o,
            // A value of four bits that is none of the above is the last.
            _ => p,
        }
    }

    /// Select the mutable cell of this identifier among the exact sixteen,
    /// without a numeric index.
    pub const fn select_mut<T>(self, entries: &mut [T; 16]) -> &mut T {
        let [a, b, c, d, e, f, g, h, i, j, k, l, m, n, o, p] = entries;
        match self.0.get() {
            0 => a,
            1 => b,
            2 => c,
            3 => d,
            4 => e,
            5 => f,
            6 => g,
            7 => h,
            8 => i,
            9 => j,
            10 => k,
            11 => l,
            12 => m,
            13 => n,
            14 => o,
            // A value of four bits that is none of the above is the last.
            _ => p,
        }
    }

    /// Decode a field of exactly an identifier's width.
    #[doc(hidden)]
    pub const fn from_field<
        const START: usize,
        const END: usize,
        const LITTLE: bool,
        const N: usize,
    >(
        bytes: &[u8; N],
    ) -> Self {
        Self(oer_portable_wire::U8::from_field::<START, END, LITTLE, N>(
            bytes,
        ))
    }

    /// Encode into a field of exactly an identifier's width.
    #[doc(hidden)]
    pub const fn to_field<
        const START: usize,
        const END: usize,
        const LITTLE: bool,
        const N: usize,
    >(
        self,
        bytes: &mut [u8; N],
    ) {
        self.0.to_field::<START, END, LITTLE, N>(bytes);
    }
}

/// Valid IEEE 802.1D / IEEE 802.11 user priority and QoS TID.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct WmmUserPriority(UserPriority);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
enum UserPriority {
    #[default]
    Up0 = 0,
    Up1 = 1,
    Up2 = 2,
    Up3 = 3,
    Up4 = 4,
    Up5 = 5,
    Up6 = 6,
    Up7 = 7,
}

impl WmmUserPriority {
    pub const UP0: Self = Self(UserPriority::Up0);
    pub const UP1: Self = Self(UserPriority::Up1);
    pub const UP2: Self = Self(UserPriority::Up2);
    pub const UP3: Self = Self(UserPriority::Up3);
    pub const UP4: Self = Self(UserPriority::Up4);
    pub const UP5: Self = Self(UserPriority::Up5);
    pub const UP6: Self = Self(UserPriority::Up6);
    pub const UP7: Self = Self(UserPriority::Up7);
    /// Every priority, in TID order.
    pub const ALL: [Self; 8] = [
        Self::UP0,
        Self::UP1,
        Self::UP2,
        Self::UP3,
        Self::UP4,
        Self::UP5,
        Self::UP6,
        Self::UP7,
    ];
    pub const fn new(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self(UserPriority::Up0)),
            1 => Some(Self(UserPriority::Up1)),
            2 => Some(Self(UserPriority::Up2)),
            3 => Some(Self(UserPriority::Up3)),
            4 => Some(Self(UserPriority::Up4)),
            5 => Some(Self(UserPriority::Up5)),
            6 => Some(Self(UserPriority::Up6)),
            7 => Some(Self(UserPriority::Up7)),
            _ => None,
        }
    }
    pub const fn value(self) -> u8 {
        self.wire().get()
    }
    /// The priority as its three-bit value; the QoS Control TID field is
    /// four bits wide (IEEE Std 802.11-2020, 9.2.4.5.2).
    pub const fn wire(self) -> oer_portable_wire::U8<3> {
        match self.0 {
            UserPriority::Up0 => oer_portable_wire::U8::constant::<0>(),
            UserPriority::Up1 => oer_portable_wire::U8::constant::<1>(),
            UserPriority::Up2 => oer_portable_wire::U8::constant::<2>(),
            UserPriority::Up3 => oer_portable_wire::U8::constant::<3>(),
            UserPriority::Up4 => oer_portable_wire::U8::constant::<4>(),
            UserPriority::Up5 => oer_portable_wire::U8::constant::<5>(),
            UserPriority::Up6 => oer_portable_wire::U8::constant::<6>(),
            UserPriority::Up7 => oer_portable_wire::U8::constant::<7>(),
        }
    }
    /// The traffic identifier of this priority's frames.
    ///
    /// SOURCE: IEEE Std 802.11-2020, 9.2.4.5.2 (TID subfield: the UP).
    pub const fn tid(self) -> Tid {
        Tid(self.wire().widen())
    }
    /// Select a cell of the exact eight-entry array by the closed identifier.
    pub const fn select<T>(self, entries: &[T; 8]) -> &T {
        let [a, b, c, d, e, f, g, h] = entries;
        match self.0 {
            UserPriority::Up0 => a,
            UserPriority::Up1 => b,
            UserPriority::Up2 => c,
            UserPriority::Up3 => d,
            UserPriority::Up4 => e,
            UserPriority::Up5 => f,
            UserPriority::Up6 => g,
            UserPriority::Up7 => h,
        }
    }
    /// Select a cell of the exact eight-entry array by the closed identifier.
    pub const fn select_mut<T>(self, entries: &mut [T; 8]) -> &mut T {
        let [a, b, c, d, e, f, g, h] = entries;
        match self.0 {
            UserPriority::Up0 => a,
            UserPriority::Up1 => b,
            UserPriority::Up2 => c,
            UserPriority::Up3 => d,
            UserPriority::Up4 => e,
            UserPriority::Up5 => f,
            UserPriority::Up6 => g,
            UserPriority::Up7 => h,
        }
    }
    pub const fn access_category(self) -> WmmAccessCategory {
        match self.0 {
            UserPriority::Up0 | UserPriority::Up3 => WmmAccessCategory::BestEffort,
            UserPriority::Up1 | UserPriority::Up2 => WmmAccessCategory::Background,
            UserPriority::Up4 | UserPriority::Up5 => WmmAccessCategory::Video,
            UserPriority::Up6 | UserPriority::Up7 => WmmAccessCategory::Voice,
        }
    }
}

#[cfg(test)]
mod tests;
