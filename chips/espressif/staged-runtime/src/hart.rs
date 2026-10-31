//! Hart-local execution identity and the interval before interrupt admission.

use core::marker::PhantomData;

/// Execution identity minted by the runtime handoff. It cannot be copied,
/// sent to another hart or shared with a concurrent context.
///
/// ```compile_fail
/// use oer_espressif_staged_runtime::{Hart, BOOT_HART};
/// fn send<T: Send>() {}
/// send::<Hart<BOOT_HART>>();
/// ```
/// ```compile_fail
/// use oer_espressif_staged_runtime::{Hart, BOOT_HART};
/// fn sync<T: Sync>() {}
/// sync::<Hart<BOOT_HART>>();
/// ```
#[derive(Debug)]
pub struct Hart<const H: u8> {
    _local: PhantomData<*mut ()>,
}

impl<const H: u8> Hart<H> {
    /// The architectural identity, admitted against the chip geometry.
    pub const fn id(&self) -> u8 {
        H
    }
}

/// The hart's unique authority to install its inactive interrupt stack.
/// Only the boot hart's handoff or a secondary hart's `SecondaryHandoff`
/// constructs it; interrupt admission consumes it.
///
/// ```compile_fail
/// use oer_espressif_staged_runtime::{PreAdmission, BOOT_HART};
/// let admission = PreAdmission::<BOOT_HART>::new();
/// ```
/// A secondary hart's authority comes only from its root's
/// `SecondaryHandoff` (`secondary_entry!`):
/// ```compile_fail
/// use oer_espressif_staged_runtime::PreAdmission;
/// let admission = PreAdmission::<1>::new();
/// ```
/// ```compile_fail
/// use oer_espressif_staged_runtime::Handoff;
/// fn cannot_keep_authority<P>(mut handoff: Handoff<P>) {
///     let admission = handoff.pre_admission();
///     drop(handoff);
///     let _ = admission.hart();
/// }
/// ```
#[derive(Debug)]
pub struct PreAdmission<const H: u8> {
    hart: Hart<H>,
}

impl<const H: u8> PreAdmission<H> {
    /// Called only while wrapping the boot entry's handoff proof or minting a
    /// secondary hart's `SecondaryHandoff`.
    pub(crate) const fn new() -> Self {
        const { assert!((H as usize) < oer_espressif_platform_port::HARTS) };
        Self {
            hart: Hart {
                _local: PhantomData,
            },
        }
    }

    /// This admission's local execution identity.
    pub const fn hart(&self) -> &Hart<H> {
        &self.hart
    }

    /// End stack-install authority while retaining the local execution identity.
    pub(crate) const fn into_hart(self) -> Hart<H> {
        self.hart
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_keeps_the_facts_defined_local_identity_without_storage() {
        let admission = PreAdmission::<{ crate::BOOT_HART }>::new();
        assert_eq!(admission.hart().id(), crate::BOOT_HART);
        assert_eq!(core::mem::size_of_val(&admission), 0);
        let hart = admission.into_hart();
        assert_eq!(hart.id(), crate::BOOT_HART);
    }
}
