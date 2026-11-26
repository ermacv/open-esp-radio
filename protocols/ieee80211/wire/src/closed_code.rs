//! The shape of a closed code of this wire: every value it admits named,
//! the rest refused.

/// A code whose named values are listed once: the enum, its decoder and its
/// encoder into a field of `$width` bits.
macro_rules! closed_code {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident: $width:literal, $error:path => $error_type:ty {
            $($(#[$variant_meta:meta])* $variant:ident = $value:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        $vis enum $name {
            $($(#[$variant_meta])* $variant),+
        }

        impl $name {
            const fn from_wire(value: u8) -> Result<Self, $error_type> {
                match value {
                    $($value => Ok(Self::$variant),)+
                    _ => Err($error),
                }
            }

            /// The code as its field holds it.
            pub const fn wire(self) -> oer_portable_wire::U8<$width> {
                match self {
                    $(Self::$variant => oer_portable_wire::U8::constant::<$value>(),)+
                }
            }
        }
    };
}

pub(crate) use closed_code;
