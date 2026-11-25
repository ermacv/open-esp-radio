//! The shape of a one-octet code: any octet, some of them named.

macro_rules! octet_code {
    (
        $module:ident;
        $(#[$meta:meta])*
        $name:ident {
            $($(#[$value_meta:meta])* $value:ident = $number:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        ///
        /// A number is no code until it is named one:
        ///
        /// ```compile_fail,E0308
        #[doc = concat!("use oer_protocols_ieee80211_wire::", stringify!($module), "::", stringify!($name), ";")]
        #[doc = concat!("let code: ", stringify!($name), " = 0;")]
        /// ```
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[repr(transparent)]
        pub struct $name(u8);

        impl $name {
            $($(#[$value_meta])* pub const $value: Self = Self($number);)+

            /// The code `value`, a named one or not.
            pub const fn new(value: u8) -> Self {
                Self(value)
            }

            /// Its octet.
            pub const fn value(self) -> u8 {
                self.0
            }
        }
    };
}

pub(crate) use octet_code;
