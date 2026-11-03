//! Successive installations of one port whose tasks share one brand.
//!
//! A slot loan ([`crate::PortSlot`]) mints each installation's capacity
//! under a generative body brand, which two `'static` tasks cannot share.
//! [`Installations`] instead mints every installation of one claimed slot
//! identity under the `'static` brand: the identity `Id` keeps one slot's
//! tokens apart from another's, and an affine [`Installation`] receipt keeps
//! one installation's capacity apart from the next one's.
//!
//! - The authority is claimed once, from the slot's [`SlotClaim`].
//! - [`Installations::first`] mints the first installation, once.
//! - [`Installations::remint`] mints the next one only in exchange for the
//!   receipt of the current one. The backend holds that receipt for the
//!   whole installation and gives it up only after its retire checks proved
//!   every token, buffer and key of the installation back; a token kept past
//!   them belongs to an installation that no longer mints, and the backend's
//!   checks refuse it at run time.
//!
//! A receipt cannot be copied, and only these two operations make one:
//!
//! ```compile_fail,E0599
//! use oer_protocols_radio_port::{Installation, PoolSet, SlotIdentity};
//! fn copy<Id: SlotIdentity, P: PoolSet>(receipt: &Installation<Id, P>) -> Installation<Id, P> {
//!     receipt.clone()
//! }
//! ```
//!
//! ```compile_fail,E0451
//! use core::marker::PhantomData;
//! use oer_protocols_radio_port::{Installation, PoolSet, SlotIdentity};
//! fn forge<Id: SlotIdentity, P: PoolSet>() -> Installation<Id, P> {
//!     Installation { generation: 0, identity: PhantomData }
//! }
//! ```

use core::marker::PhantomData;

use crate::{BodyLoan, Mint, PoolSet, SlotClaim, SlotIdentity, SlotOwner};

/// The invariant identity and layout an installation carries.
type Brand<Id, P> = PhantomData<fn(Id, P) -> (Id, P)>;

/// The minting authority of every installation of one slot identity `Id`
/// with the pool layout `P`.
pub struct Installations<Id: SlotIdentity, P: PoolSet> {
    /// The generation of the installation the current receipt names.
    generation: u32,
    /// Whether the first installation was minted.
    issued: bool,
    identity: Brand<Id, P>,
}

/// The affine receipt of one minted installation. The backend holds it for
/// the whole installation and returns it to [`Installations::remint`] only
/// after its retire checks.
#[must_use = "only the current installation's receipt mints the next one"]
pub struct Installation<Id: SlotIdentity, P: PoolSet> {
    generation: u32,
    identity: Brand<Id, P>,
}

/// One installation's capacity and its receipt.
#[must_use = "an installation's capacity is minted once"]
pub struct Minted<Id: SlotIdentity, P: PoolSet> {
    /// The installation's complete pool set, under the `'static` brand.
    pub mint: Mint<'static, P, Id>,
    /// The receipt the backend keeps until its retire checks pass.
    pub installation: Installation<Id, P>,
}

/// Why [`Installations::remint`] minted nothing; the receipt comes back
/// unchanged.
pub enum RemintRefused<Id: SlotIdentity, P: PoolSet> {
    /// The receipt names an installation other than the current one.
    Foreign(Installation<Id, P>),
    /// The generations of this identity are used up.
    Exhausted(Installation<Id, P>),
}

impl<Id: SlotIdentity, P: PoolSet> core::fmt::Debug for RemintRefused<Id, P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Foreign(receipt) => f.debug_tuple("Foreign").field(&receipt.generation).finish(),
            Self::Exhausted(receipt) => f
                .debug_tuple("Exhausted")
                .field(&receipt.generation)
                .finish(),
        }
    }
}

impl<Id: SlotIdentity, P: PoolSet> Installations<Id, P> {
    /// Claim the minting authority of the slot `claim` names, consuming it:
    /// no slot loan exists for this identity afterwards.
    pub const fn claim(claim: SlotClaim<Id>) -> Self {
        let SlotClaim { identity: _ } = claim;
        Self {
            generation: 0,
            issued: false,
            identity: PhantomData,
        }
    }

    /// Mint the first installation, once; `None` once it was minted.
    pub fn first(&mut self) -> Option<Minted<Id, P>> {
        if self.issued {
            return None;
        }
        self.issued = true;
        Some(self.minted())
    }

    /// Mint the installation after the one `retired` names.
    ///
    /// # Errors
    ///
    /// The receipt, unchanged, when it is not the current installation's or
    /// no generation is left.
    pub fn remint(
        &mut self,
        retired: Installation<Id, P>,
    ) -> Result<Minted<Id, P>, RemintRefused<Id, P>> {
        if !self.issued || retired.generation != self.generation {
            return Err(RemintRefused::Foreign(retired));
        }
        let Some(next) = self.generation.checked_add(1) else {
            return Err(RemintRefused::Exhausted(retired));
        };
        let Installation { .. } = retired;
        self.generation = next;
        Ok(self.minted())
    }

    /// The generation of the current installation.
    pub const fn generation(&self) -> u32 {
        self.generation
    }

    fn minted(&self) -> Minted<Id, P> {
        Minted {
            mint: Mint::new(),
            installation: Installation {
                generation: self.generation,
                identity: PhantomData,
            },
        }
    }
}

impl<Id: SlotIdentity, P: PoolSet> Installation<Id, P> {
    /// The installation's generation, distinct from every other
    /// installation of its identity.
    pub const fn generation(&self) -> u32 {
        self.generation
    }

    /// The backend's private conversion authority for this installation's
    /// `'static` brand. Its conversions keep each token's identity, path,
    /// kind and index; only the holder of the receipt makes one.
    pub const fn loan<S: SlotOwner<Id>>(&self) -> BodyLoan<'static, Id, S> {
        BodyLoan::new()
    }
}

impl<Id: SlotIdentity, P: PoolSet> core::fmt::Debug for Installation<Id, P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Installation")
            .field("generation", &self.generation)
            .finish()
    }
}

#[cfg(test)]
mod tests;
