//! The lifecycle's commands to a data port, and their issuer.
//!
//! The station's lifecycle never holds the port. It sends each key,
//! receive-configuration, gate or setting operation as one [`PortCommand`]
//! to the task that owns the port, which hands it to
//! [`DataPort::command`](crate::DataPort::command) in the order it came.
//! A command carries every owner its operation consumes; its end returns
//! them in the port's sink ([`PortSink`](crate::PortSink)) or with the
//! command itself ([`CommandProgress`](crate::CommandProgress)).
//!
//! An installation names an identity the lifecycle keeps before the port
//! runs it, so only the backend's issuer ([`PortCommands`]) makes an
//! installation command: it mints the identity and the backend's own
//! payload, which no other code constructs.
//!
//! A command moves its owners once and is never copied:
//!
//! ```compile_fail,E0599
//! use oer_protocols_ieee80211_contracts_port::{CommandTypes, Ieee80211Capabilities, PortCommand};
//! fn copy<'id, T: CommandTypes<'id>, K: Ieee80211Capabilities>(
//!     command: &PortCommand<'id, T, K>,
//! ) -> PortCommand<'id, T, K> {
//!     command.clone()
//! }
//! ```

use oer_protocols_ieee80211_contracts_security::Ccmp128Key;
use oer_protocols_ieee80211_wire::{address::IndividualAddress, ccmp::GroupKeyId};

use crate::{
    BssColor, ConfigurePermission, GateOpen, Ieee80211Capabilities, KeyTypes, PortTypes, Station,
    StationReceive, SupportedEdca, he::StationHe,
};

/// The station's keys of the phase types `T`.
type StationKeys<'id, T> = <T as PortTypes<'id>>::StationKeys;

/// A pairwise installation command and the identity its installed key
/// will carry.
pub type PairwiseIssue<'id, T, K> = (
    PortCommand<'id, T, K>,
    <StationKeys<'id, T> as KeyTypes<'id>>::Identity,
);

/// The phase types of a backend that runs [`PortCommand`]s: the payloads
/// only its issuer makes.
pub trait CommandTypes<'id>: PortTypes<'id> {
    /// An accepted pairwise installation: its vacant slot, peer, key
    /// material and the identity its installed key will carry.
    type PairwiseInstall;
    /// An accepted group installation: its vacant slot, the interface's
    /// group permission, its key ID and material.
    type GroupInstall;
}

/// One operation of the station's lifecycle on the data port.
///
/// The port runs one command at a time, in the order the lifecycle sent
/// them. A command whose barrier is not reached yet (a configuration while
/// a data arena is on air, a removal before the attempts that use its key
/// are drained, a group mutation before its receive cut) comes back as
/// [`CommandProgress::Held`](crate::CommandProgress::Held) and runs again
/// in the same order on a later call; one that the port cannot run in its
/// state comes back refused, with every owner unchanged.
pub enum PortCommand<'id, T: CommandTypes<'id>, K: Ieee80211Capabilities> {
    /// Install a pairwise key; its terminal
    /// ([`LowerMacTerminal::PairwiseKeyInstalled`](crate::LowerMacTerminal::PairwiseKeyInstalled))
    /// returns the installed key.
    InstallPairwise(T::PairwiseInstall),
    /// Install a group key; its terminal is the receive boundary of the new
    /// installation
    /// ([`LowerMacTerminal::GroupKeyInstalled`](crate::LowerMacTerminal::GroupKeyInstalled)).
    InstallGroup(T::GroupInstall),
    /// Replace the installed group key in its slot.
    ReplaceGroup {
        /// The interface's group permission.
        permission: <StationKeys<'id, T> as KeyTypes<'id>>::GroupPermission,
        /// The installed key the replacement consumes.
        installed: <StationKeys<'id, T> as KeyTypes<'id>>::GroupKey,
        /// The replacement's wire KeyID.
        key_id: GroupKeyId,
        /// The replacement's material.
        key: Ccmp128Key,
    },
    /// Select a retained group installation, without writing material.
    SelectGroup {
        /// The interface's group permission.
        permission: <StationKeys<'id, T> as KeyTypes<'id>>::GroupPermission,
        /// The retained installation to select.
        installed: <StationKeys<'id, T> as KeyTypes<'id>>::GroupKey,
    },
    /// Remove the pairwise key once no admitted attempt uses it.
    RemovePairwise {
        /// The association owner the removal returns.
        association: <StationKeys<'id, T> as KeyTypes<'id>>::Association,
        /// The installed key.
        key: <StationKeys<'id, T> as KeyTypes<'id>>::PairwiseKey,
    },
    /// Remove a group key.
    RemoveGroup {
        /// The interface's group permission.
        permission: <StationKeys<'id, T> as KeyTypes<'id>>::GroupPermission,
        /// The installed key.
        key: <StationKeys<'id, T> as KeyTypes<'id>>::GroupKey,
    },
    /// Set what the station receives once none of its frames is in flight.
    Configure {
        /// The station's unique configure permission, which the terminal
        /// returns.
        permission:
            ConfigurePermission<'id, Station, T::StationCommandPath, T::StationConfigureContext>,
        /// What it receives.
        receive: StationReceive<K>,
    },
    /// Close publication in every queue; its terminal is
    /// [`LowerMacTerminal::GateClosed`](crate::LowerMacTerminal::GateClosed).
    CloseGate(GateOpen<'id, T::GatePath>),
    /// The station's EDCA parameters, from the next publication on.
    SetEdca(SupportedEdca<K>),
    /// The BSS color of the station's HE BSS.
    SetBssColor(BssColor),
    /// The station interface's HE association, from the next publication
    /// on: installed after the Association Response and before the link's
    /// data, [`StationHe::Off`] once the association ends. It also sets the
    /// color the station's HE transmissions carry, as
    /// [`Self::SetBssColor`] does.
    SetHe(StationHe),
}

impl<'id, T: CommandTypes<'id>, K: Ieee80211Capabilities> PortCommand<'id, T, K> {
    /// The command's name, for a refusal's or a barrier's observation.
    pub const fn kind(&self) -> CommandKind {
        match self {
            Self::InstallPairwise(_) => CommandKind::InstallPairwise,
            Self::InstallGroup(_) => CommandKind::InstallGroup,
            Self::ReplaceGroup { .. } => CommandKind::ReplaceGroup,
            Self::SelectGroup { .. } => CommandKind::SelectGroup,
            Self::RemovePairwise { .. } => CommandKind::RemovePairwise,
            Self::RemoveGroup { .. } => CommandKind::RemoveGroup,
            Self::Configure { .. } => CommandKind::Configure,
            Self::CloseGate(_) => CommandKind::CloseGate,
            Self::SetEdca(_) => CommandKind::SetEdca,
            Self::SetBssColor(_) => CommandKind::SetBssColor,
            Self::SetHe(_) => CommandKind::SetHe,
        }
    }
}

/// The kind of a [`PortCommand`], without its owners.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CommandKind {
    /// [`PortCommand::InstallPairwise`].
    InstallPairwise,
    /// [`PortCommand::InstallGroup`].
    InstallGroup,
    /// [`PortCommand::ReplaceGroup`].
    ReplaceGroup,
    /// [`PortCommand::SelectGroup`].
    SelectGroup,
    /// [`PortCommand::RemovePairwise`].
    RemovePairwise,
    /// [`PortCommand::RemoveGroup`].
    RemoveGroup,
    /// [`PortCommand::Configure`].
    Configure,
    /// [`PortCommand::CloseGate`].
    CloseGate,
    /// [`PortCommand::SetEdca`].
    SetEdca,
    /// [`PortCommand::SetBssColor`].
    SetBssColor,
    /// [`PortCommand::SetHe`].
    SetHe,
}

/// The lifecycle's issuer of the commands only the backend makes: it
/// mints each installation's identity and payload, which the lifecycle
/// sends as a [`PortCommand`]. The backend creates its issuer with the
/// installation, beside the port.
pub trait PortCommands<'id> {
    /// The backend's phase types.
    type Types: CommandTypes<'id>;
    /// The backend's capabilities marker.
    type Caps: Ieee80211Capabilities;

    /// Install `key` for frames with `peer` in its vacant `slot`. Returns
    /// the command and the identity its installed key will carry: the
    /// terminal's key is this installation's when its identity equals it.
    fn install_pairwise(
        &mut self,
        association: &<StationKeys<'id, Self::Types> as KeyTypes<'id>>::Association,
        slot: <StationKeys<'id, Self::Types> as KeyTypes<'id>>::PairwiseSlot,
        peer: IndividualAddress,
        key: Ccmp128Key,
    ) -> PairwiseIssue<'id, Self::Types, Self::Caps>;

    /// Install a group key in its vacant `slot`, consuming the interface's
    /// group permission until the installation's terminal.
    fn install_group(
        &mut self,
        association: &<StationKeys<'id, Self::Types> as KeyTypes<'id>>::Association,
        permission: <StationKeys<'id, Self::Types> as KeyTypes<'id>>::GroupPermission,
        slot: <StationKeys<'id, Self::Types> as KeyTypes<'id>>::GroupSlot,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    ) -> PortCommand<'id, Self::Types, Self::Caps>;
}
