//! The shape of a one-octet code of a wire: any octet, some of them named.

/// Declares a one-octet code: any octet, some of them named.
///
/// The literal before the type is the path of its module, for the type's own
/// example in its documentation.
///
/// ```
/// # fn main() {}
/// oer_portable_wire::octet_code! { "example";
///     /// A kind.
///     Kind {
///         /// The first.
///         FIRST = 1,
///     }
/// }
/// const _: () = {
///     if Kind::FIRST.value() != 1 || Kind::new(7).value() != 7 {
///         panic!("a code is any octet")
///     }
/// };
/// ```
///
/// A value is the octet it is named:
///
/// ```compile_fail,E0080
/// # fn main() {}
/// oer_portable_wire::octet_code! { "example";
///     /// A kind.
///     Kind {
///         /// The first.
///         FIRST = 1,
///     }
/// }
/// const _: () = {
///     if Kind::FIRST.value() == 1 {
///         panic!("a named code has its octet")
///     }
/// };
/// ```
#[macro_export]
macro_rules! octet_code {
    (
        $path:literal;
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
        #[doc = concat!("use ", $path, "::", stringify!($name), ";")]
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
