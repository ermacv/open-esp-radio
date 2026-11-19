//! The proof that code runs on one hart.
//!
//! A hart's identity is the architecture's (`mhartid`). [`OnHart<H>`] is
//! made only where the hart is known by construction: [`OnHart::check`]
//! reads `mhartid` where a proof is taken once (a hart's start), and
//! [`OnHart::assume`] is the promise of code that runs on its hart by
//! construction (an interrupt entry's vector), where a read on every entry
//! would add a branch that cannot be taken.

use core::marker::PhantomData;

/// The proof that the code holding it runs on hart `H`. Zero-sized; neither
/// `Send` nor `Sync`, so it never leaves its hart and is never shared with
/// a context of another:
///
/// ```compile_fail
/// fn send<T: Send>() {}
/// send::<oer_portable_riscv::OnHart<0>>();
/// ```
/// ```compile_fail
/// fn sync<T: Sync>() {}
/// sync::<oer_portable_riscv::OnHart<0>>();
/// ```
///
/// No safe constructor makes it from nothing on the host:
///
/// ```compile_fail
/// let _ = oer_portable_riscv::OnHart::<0> { _here: core::marker::PhantomData };
/// ```
#[derive(Debug)]
pub struct OnHart<const H: u8> {
    _here: PhantomData<*mut ()>,
}

/// The hart the code runs on is another than the one asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OtherHart {
    /// `mhartid` as read.
    pub raw: usize,
}

impl<const H: u8> OnHart<H> {
    /// The proof, when `mhartid` reads `H`: for where a proof is taken once,
    /// as a hart starts.
    ///
    /// # Errors
    ///
    /// [`OtherHart`] with the identity read, when the code runs on another
    /// hart.
    #[cfg(target_arch = "riscv32")]
    pub fn check() -> Result<Self, OtherHart> {
        let raw = crate::hart_id();
        if raw == usize::from(H) {
            Ok(Self { _here: PhantomData })
        } else {
            Err(OtherHart { raw })
        }
    }

    /// The proof, by the caller's promise.
    ///
    /// # Safety
    ///
    /// The caller runs on hart `H` by construction. The proof's type keeps
    /// it there: neither `Send` nor `Sync`, it reaches no code of another
    /// hart.
    #[allow(
        unsafe_code,
        reason = "the promise of code that runs on its hart by construction"
    )]
    #[inline(always)]
    pub const unsafe fn assume() -> Self {
        Self { _here: PhantomData }
    }

    /// A proof for a host test, which has no `mhartid`: only with the
    /// `host-hart` feature, which only test builds enable, and never on
    /// the target.
    #[cfg(all(not(target_arch = "riscv32"), feature = "host-hart"))]
    pub const fn host() -> Self {
        Self { _here: PhantomData }
    }

    /// The hart the proof names.
    pub const fn hart(&self) -> u8 {
        H
    }
}

#[cfg(test)]
mod tests {
    use super::OnHart;

    #[test]
    fn a_proof_is_zero_sized() {
        assert_eq!(core::mem::size_of::<OnHart<0>>(), 0);
        assert_eq!(core::mem::size_of::<OnHart<1>>(), 0);
    }
}
