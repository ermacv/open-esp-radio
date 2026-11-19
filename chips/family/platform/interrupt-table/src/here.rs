//! The witness that code runs as an entry's handler, on the entry's hart.
//!
//! The generated vector of an entry is the one place where "this code runs
//! as entry `E`, on `E`'s hart" holds by construction: the vector is
//! exported as `__oer_interrupt_h<HART>_l<LINE>`, which only hart `HART`'s
//! vector table names (generated from the chip facts), and the router binds
//! `E` only on `E::HART`. The vector's one entry point,
//! [`vector`](crate::__private::vector), lends the entry's part and a
//! [`Here<'_, E>`] for the call; root Clippy policy forbids every other call
//! of it (`disallowed-methods`), as it forbids a direct `EntryProof::new`.

use core::marker::PhantomData;

use oer_portable_riscv::OnHart;

use crate::Entry;

/// The witness that the code holding it runs as entry `E`'s handler, on
/// `E`'s hart, for one call of the handler: zero-sized, neither `Send` nor
/// `Sync`, and bounded by the call. Only the entry's vector makes one.
///
/// ```compile_fail
/// # oer_chips_family_platform_interrupt_table::__fake_table!();
/// fn send<T: Send>() {}
/// send::<oer_chips_family_platform_interrupt_table::Here<'static, TimerToken>>();
/// ```
/// ```compile_fail
/// # oer_chips_family_platform_interrupt_table::__fake_table!();
/// fn sync<T: Sync>() {}
/// sync::<oer_chips_family_platform_interrupt_table::Here<'static, TimerToken>>();
/// ```
/// ```compile_fail,E0451
/// # oer_chips_family_platform_interrupt_table::__fake_table!();
/// // No constructor outside the vector.
/// let _ = oer_chips_family_platform_interrupt_table::Here::<'static, TimerToken> {
///     _call: core::marker::PhantomData,
/// };
/// ```
pub struct Here<'call, E> {
    _call: PhantomData<(&'call (), *mut (), fn() -> E)>,
}

impl<E> core::fmt::Debug for Here<'_, E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Here")
    }
}

impl<E: Entry> Here<'_, E> {
    /// The witness of the call the vector makes.
    #[inline(always)]
    pub(crate) const fn new() -> Self {
        Self { _call: PhantomData }
    }

    /// The proof of the entry's hart, with the build's check that it is
    /// hart `H`:
    ///
    /// ```compile_fail,E0080
    /// # oer_chips_family_platform_interrupt_table::__fake_table!();
    /// // The radio's entry is hart 1's.
    /// fn on(
    ///     here: &oer_chips_family_platform_interrupt_table::Here<'_, RadioToken>,
    /// ) -> oer_portable_riscv::OnHart<0> {
    ///     here.on_hart::<0>()
    /// }
    /// # fn main() {
    /// #     let on: for<'a> fn(&oer_chips_family_platform_interrupt_table::Here<'a, RadioToken>) -> oer_portable_riscv::OnHart<0> = on;
    /// #     core::hint::black_box(on);
    /// # }
    /// ```
    #[inline(always)]
    pub fn on_hart<const H: u8>(&self) -> OnHart<H> {
        const {
            if E::HART != H {
                panic!("an entry's witness proves the entry's own hart")
            }
        };
        #[allow(
            unsafe_code,
            reason = "the promise of an entry's vector, which runs on its hart by construction"
        )]
        // SAFETY: a `Here<'_, E>` exists only for one call that the entry's
        // vector makes: (1) the vector's one entry point, `vector`, is called
        // only from the generated vector of `E`, a rule root Clippy policy
        // enforces as it does for `EntryProof::new` (`disallowed-methods`);
        // (2) that vector is exported as `__oer_interrupt_h<E::HART>_l<LINE>`,
        // which only hart `E::HART`'s vector table names, generated from the
        // chip facts; (3) the router binds `E` only on hart `E::HART`
        // (checked at compile time), so the vector runs only there. The
        // build checked `E::HART == H` above, and the proof, neither `Send`
        // nor `Sync`, stays on that hart.
        unsafe {
            OnHart::assume()
        }
    }
}
