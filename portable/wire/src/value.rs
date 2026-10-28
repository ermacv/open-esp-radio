//! Unsigned values whose width is admitted once, before field insertion.

macro_rules! unsigned {
    ($name:ident, $raw:ty) => {
        /// A value of exactly `WIDTH` bits, with no invalid public constructor.
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[repr(transparent)]
        pub struct $name<const WIDTH: usize>($raw);

        impl<const WIDTH: usize> Default for $name<WIDTH> {
            fn default() -> Self {
                Self::ZERO
            }
        }

        impl<const WIDTH: usize> $name<WIDTH> {
            pub const ZERO: Self = Self::constant::<0>();

            /// Admit a dynamic value without discarding any bit.
            pub const fn new(value: $raw) -> Option<Self> {
                const {
                    if !(WIDTH > 0 && WIDTH <= <$raw>::BITS as usize) {
                        panic!("invalid wire field geometry or value");
                    }
                }
                if value <= const { <$raw>::MAX >> (<$raw>::BITS as usize - WIDTH) } {
                    Some(Self(value))
                } else {
                    None
                }
            }

            /// Admit a literal at compile time, including in runtime callers.
            pub const fn constant<const VALUE: $raw>() -> Self {
                const {
                    if !(WIDTH > 0 && WIDTH <= <$raw>::BITS as usize) {
                        panic!("invalid wire field geometry or value");
                    }
                    if !(VALUE <= <$raw>::MAX >> (<$raw>::BITS as usize - WIDTH)) {
                        panic!("invalid wire field geometry or value");
                    }
                }
                Self(VALUE)
            }

            /// Extract the low `WIDTH` bits. Callers choose this only when the
            /// wire format defines truncation, or the source is a decoded field.
            pub const fn from_low_bits(value: $raw) -> Self {
                const {
                    if !(WIDTH > 0 && WIDTH <= <$raw>::BITS as usize) {
                        panic!("invalid wire field geometry or value");
                    }
                }
                Self(value & const { <$raw>::MAX >> (<$raw>::BITS as usize - WIDTH) })
            }

            /// Decode a field whose const geometry proves this exact width.
            ///
            /// ```compile_fail,E0080
            /// // A five-bit extraction cannot be a four-bit proof.
            /// let _ = oer_wire::U8::<4>::from_field::<0, 5, true, 1>(&[0xff]);
            /// ```
            #[doc(hidden)]
            pub const fn from_field<
                const START: usize,
                const END: usize,
                const LITTLE: bool,
                const N: usize,
            >(
                bytes: &[u8; N],
            ) -> Self {
                const {
                    if !(START < END && END - START == WIDTH) {
                        panic!("invalid wire field geometry or value");
                    }
                    if !(WIDTH <= <$raw>::BITS as usize) {
                        panic!("invalid wire field geometry or value");
                    }
                }
                // Extraction returns exactly END-START bits; the const proof
                // makes this narrowing lossless, without a second mask.
                Self(crate::__private::get::<START, END, LITTLE, N>(bytes) as $raw)
            }

            pub const fn get(self) -> $raw {
                self.0
            }

            /// Widen a proof without validating the same value again.
            pub const fn widen<const NEW_WIDTH: usize>(self) -> $name<NEW_WIDTH> {
                const {
                    if !(NEW_WIDTH >= WIDTH && NEW_WIDTH <= <$raw>::BITS as usize) {
                        panic!("invalid wire field geometry or value");
                    }
                }
                $name(self.0)
            }
        }
    };
}

unsigned!(U8, u8);
unsigned!(U16, u16);
unsigned!(U32, u32);
unsigned!(U64, u64);

/// The admitted length of a prefix in an array of `N` octets.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct PrefixLength<const N: usize>(usize);

impl<const N: usize> PrefixLength<N> {
    pub const ZERO: Self = Self(0);

    pub const fn new(length: usize) -> Option<Self> {
        if length <= N {
            Some(Self(length))
        } else {
            None
        }
    }

    pub const fn constant<const LENGTH: usize>() -> Self {
        const {
            if !(LENGTH <= N) {
                panic!("invalid wire field geometry or value");
            }
        }
        Self(LENGTH)
    }

    pub const fn get(self) -> usize {
        self.0
    }

    /// The constructor is the bounds proof. Trimming an iterator's unused
    /// tail forms the exact slice without a panicking split or re-admission.
    pub fn borrow(self, bytes: &[u8; N]) -> &[u8] {
        let mut prefix = bytes.iter();
        let mut unused = self.0..N;
        if unused.next().is_some() {
            prefix.nth_back(unused.len());
        }
        prefix.as_slice()
    }

    /// The same admitted prefix, exclusively borrowed for writing.
    pub fn borrow_mut(self, bytes: &mut [u8; N]) -> &mut [u8] {
        let mut prefix = bytes.iter_mut();
        let mut unused = self.0..N;
        if unused.next().is_some() {
            prefix.nth_back(unused.len());
        }
        prefix.into_slice()
    }
}
