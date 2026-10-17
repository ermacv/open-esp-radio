//! Backend granularity of the association's SPP A-MSDU setting.
//!
//! Every association has one immutable decision. A backend may share its
//! physical setting across an interface; that constraint is declared by a
//! type, never discovered by checking a chip name. Interface-wide APs can
//! advertise SPP only with a uniform mode for all clients. Their current
//! non-SPP profile explicitly refuses a peer that requires SPP.

use super::{Spp, SppCapability, SppError};

mod sealed {
    pub trait Scope {}
}

/// Granularity a backend can enforce independently.
pub trait SppScope: sealed::Scope + 'static {
    /// Statically selected AP admission policy for this hardware granularity.
    fn admit_ap(local: SppCapability, peer: SppCapability) -> Result<Spp, SppError>;
}

/// One hardware SPP setting shared by every association of an interface.
pub enum Interface {}

/// Independent SPP settings for the associations of one interface.
pub enum Association {}

impl sealed::Scope for Interface {}
impl sealed::Scope for Association {}

impl SppScope for Interface {
    fn admit_ap(local: SppCapability, peer: SppCapability) -> Result<Spp, SppError> {
        match local {
            SppCapability::Capable => Err(SppError::InterfaceRequiresUniformMode),
            SppCapability::NotCapable | SppCapability::Required => local.negotiate(peer),
        }
    }
}

impl SppScope for Association {
    fn admit_ap(local: SppCapability, peer: SppCapability) -> Result<Spp, SppError> {
        local.negotiate(peer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_scope_never_admits_mixed_modes() {
        for peer in [
            SppCapability::NotCapable,
            SppCapability::Capable,
            SppCapability::Required,
        ] {
            assert_eq!(
                Interface::admit_ap(SppCapability::Capable, peer),
                Err(SppError::InterfaceRequiresUniformMode)
            );
        }
        assert_eq!(
            Interface::admit_ap(SppCapability::Required, SppCapability::NotCapable),
            Err(SppError::LocalRequiresSupport)
        );
        assert_eq!(
            Interface::admit_ap(SppCapability::Required, SppCapability::Capable),
            Ok(Spp::Negotiated)
        );
        assert_eq!(
            Association::admit_ap(SppCapability::Capable, SppCapability::NotCapable),
            Ok(Spp::NotNegotiated)
        );
        assert_eq!(
            Association::admit_ap(SppCapability::Capable, SppCapability::Capable),
            Ok(Spp::Negotiated)
        );
    }
}
