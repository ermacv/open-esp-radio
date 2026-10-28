//! Names whose byte budgets are proved at compilation, including when a
//! runtime match selects one of several static reasons.

use crate::note::MAX_NAME;

/// A static text source. [`Name::from_static`] checks this associated
/// constant in an inline const block; no runtime string can enter a name.
pub trait StaticName {
    const TEXT: &'static str;
}

/// A static name of at most `BYTES` UTF-8 bytes (never more than 64).
/// The budget is part of the type so a note can const-check its aggregate.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Name<const BYTES: usize = MAX_NAME>(&'static str);

impl<const BYTES: usize> Name<BYTES> {
    /// Constructs a name from a compile-time source. Even a runtime call
    /// performs its checks at monomorphization, never on the hardware.
    pub const fn from_static<T: StaticName>() -> Self {
        const {
            if BYTES > MAX_NAME || T::TEXT.len() > BYTES {
                panic!("a diagnostic name fits its budget and MAX_NAME");
            }
            Self(T::TEXT)
        }
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// A literal/constant name, with its exact byte budget, or an explicit
/// common budget for the branches of a reason-producing match.
#[macro_export]
macro_rules! name {
    ($text:expr) => { $crate::name!($text; { $text.len() }) };
    ($text:expr; $budget:expr) => {{
        struct Text;
        impl $crate::text::StaticName for Text {
            const TEXT: &'static str = $text;
        }
        const { $crate::Name::<$budget>::from_static::<Text>() }
    }};
}
