//! Continue composition teardown only after the MAC returned its owners.

use oer_espressif_ieee802154_engine::engine::Ieee802154Engine;
use oer_espressif_ieee802154_runtime::{
    Ieee802154RuntimeParts, MacStoppedParts, RuntimeQueue, Uninstalled,
};
use oer_radio_port::Quarantined;

/// The two MAC outcomes that permit clocks and PHY to be released.
pub(crate) enum StoppedMac<'s, H, S> {
    Reusable(Ieee802154RuntimeParts<'s, H>),
    TerminalsQuarantined {
        parts: MacStoppedParts<'s, H>,
        terminals: Quarantined<S>,
    },
}

pub(crate) enum Released<P, S> {
    Reusable { parts: P, queue: RuntimeQueue },
    TerminalsQuarantined { parts: P, terminals: Quarantined<S> },
}

impl<'s, H, S> StoppedMac<'s, H, S> {
    pub(crate) fn take(
        parts: Uninstalled<Ieee802154RuntimeParts<'s, H>, MacStoppedParts<'s, H>, S>,
    ) -> Result<Self, Quarantined<S>> {
        match parts {
            Uninstalled::Reusable(parts) => Ok(Self::Reusable(parts)),
            Uninstalled::TerminalsQuarantined { parts, terminals } => {
                Ok(Self::TerminalsQuarantined { parts, terminals })
            }
            Uninstalled::Quarantined(receipt) => Err(receipt),
        }
    }

    /// A refused coexistence withdrawal leaves exactly the original owners.
    pub(crate) fn into_uninstalled(
        self,
    ) -> Uninstalled<Ieee802154RuntimeParts<'s, H>, MacStoppedParts<'s, H>, S> {
        match self {
            Self::Reusable(parts) => Uninstalled::Reusable(parts),
            Self::TerminalsQuarantined { parts, terminals } => {
                Uninstalled::TerminalsQuarantined { parts, terminals }
            }
        }
    }

    /// Both stopped outcomes execute the same remaining hardware teardown.
    /// Terminal retention controls only whether the old queue returns.
    pub(crate) async fn release<P, E>(
        self,
        teardown: impl AsyncFnOnce(Ieee802154Engine<'s>, H) -> Result<P, E>,
    ) -> Result<Released<P, S>, E> {
        match self {
            Self::Reusable(parts) => {
                let (engine, hardware, queue) = parts.into_parts();
                let parts = teardown(engine, hardware).await?;
                Ok(Released::Reusable { parts, queue })
            }
            Self::TerminalsQuarantined { parts, terminals } => {
                let (engine, hardware) = parts.into_parts();
                let parts = teardown(engine, hardware).await?;
                Ok(Released::TerminalsQuarantined { parts, terminals })
            }
        }
    }
}

#[cfg(test)]
mod tests;
