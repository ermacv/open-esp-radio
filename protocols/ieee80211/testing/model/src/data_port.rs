//! The model as a data port ([`DataPort`]): the radio one actor owns.
//!
//! [`ModelPort`] owns the model's driver state and its queues by value and
//! runs every operation synchronously in the caller's call. Installations
//! come from [`Installations`]: each mint's tokens carry the `'id`
//! brand with the slot identity, and the installation's receipt gives the
//! port its private conversion authority. [`ModelCommands`] is the
//! lifecycle's issuer of the installation commands.
//!
//! Each call takes the instant its caller read once and the model's steps
//! use it as their time; no call reads the image clock or wakes its
//! caller. [`DataPort::poll_facts`] registers the actor with the air,
//! whose world (received frames, a released result, advanced time) wakes
//! it. The port's deadline is recomputed when its queues change and kept.
//!
//! A command whose barrier is not reached comes back `Held`: a station
//! configuration while a data arena or a station MPDU is on the air, a
//! key removal while an admitted attempt still uses its key, a group
//! mutation while the air holds group commands. A held command that waits
//! past [`MODEL_HELD_COMMAND_BOUND`] did not run: the port undoes what its
//! hold began and refuses it `Expired` with every owner.

use super::*;
use core::task::Context;
use oer_protocols_ieee80211_contracts_port::Ieee80211Instant;
use oer_protocols_ieee80211_contracts_port::{
    AccessPointCapabilities, AccessPointCommand, AccessPointCommandKind, AccessPointCommands,
    AccessPointPairwiseIssue, AccessPointPort, AccessPointProgress, AccessPointReceive,
    AccessPointRefusal, AccessPointSubmission, AccessPointTypes, BeaconParts, BeaconSubmission,
    TsfControl, first_stale,
};
use oer_protocols_ieee80211_contracts_port::{
    AccessPointCommandFailed, AdmittedTxParts, CleanupRefused, CommandFailed, CommandKind,
    CommandProgress, CommandRefusal, CommandTypes, DataPort, Facts, Input, LendRefusal,
    LendRefused, PairwiseIssue, PortAnswer, PortArena, PortCommand, PortCommandFailed,
    PortCommands, PortCompleted, PortRxBuffer, PortRxPull, PortSink, Removed, RemovedKey, RxPull,
    StartFailure, StartRefused, StationSubmission, StopCause, StopObservations, StopUnconfirmed,
    SubmitRefusal, TxFacts, Unconsumed, UnconsumedRefused, Unkeeping, Unkept,
    arena::{
        Arena, ArenaId, ArenaPublication, Completion, IntakeRefused, PublishError, Settlement,
        TxArena,
    },
};
use oer_protocols_radio_port::{Installation, Installations, Minted, RemintRefused};

/// The model's backend type of the composition `(Id, C, X)`.
type Backend<Id, C, X> = ModelBackend<Id, C, X>;

/// How long a held command may wait at its barrier, in model time, before
/// the port names it stalled: past the arena's abort and the publication
/// bound.
pub const MODEL_HELD_COMMAND_BOUND: oer_portable_time::Duration =
    oer_portable_time::Duration::from_millis(20);

/// How long a stop may wait for the work on the air, in model time.
pub const MODEL_STOP_BOUND: oer_portable_time::Duration =
    oer_portable_time::Duration::from_millis(20);

/// A key's activity before a removal held at its barrier changed it.
struct HeldKey {
    key: RawKeyIdentity,
    active: bool,
    rx_active: bool,
}

impl HeldKey {
    fn of(key: &RawKeyIdentity) -> Self {
        Self {
            key: key.clone(),
            active: key.0.active.get(),
            rx_active: key.0.rx_active.get(),
        }
    }

    /// The removal did not run: the key is as it was.
    fn restore(self) {
        self.key.0.active.set(self.active);
        self.key.0.rx_active.set(self.rx_active);
    }
}

/// A pairwise installation the issuer accepted.
pub struct ModelPairwiseInstall<B: ModelComposition> {
    spp: oer_protocols_ieee80211_wire::security::Spp,
    /// The installation that issued the association.
    association: u32,
    key: RawKeyIdentity,
    material: Ccmp128Key,
    installed: OwnedInstalledPairwise<'static, B>,
}

/// A group installation the issuer accepted.
pub struct ModelGroupInstall<B: ModelComposition>(GroupInstallationCommand<'static, B>);

impl<'id, B: ModelComposition + 'id> CommandTypes<'id> for ModelTypes<B> {
    type PairwiseInstall = ModelPairwiseInstall<B>;
    type GroupInstall = ModelGroupInstall<B>;
}

/// An access-point pairwise installation the issuer accepted: the station's
/// payload does not fit an access-point command.
pub struct ModelAccessPointPairwiseInstall<B: ModelComposition>(ModelPairwiseInstall<B>);

/// An access-point group installation the issuer accepted.
pub struct ModelAccessPointGroupInstall<B: ModelComposition>(ModelGroupInstall<B>);

/// The access point's installations run on the station's implementation,
/// in payload types of their own.
impl<'id, B: ModelComposition + 'id> AccessPointTypes<'id> for ModelTypes<B> {
    type AccessPointPairwiseInstall = ModelAccessPointPairwiseInstall<B>;
    type AccessPointGroupInstall = ModelAccessPointGroupInstall<B>;
}

/// The lifecycle's issuer of the model port's installation commands.
pub struct ModelCommands<
    'id,
    Id: SlotIdentity,
    C: Ieee80211Capabilities = ModelCaps,
    X: ModelContext = (),
> {
    air: Rc<ModelAir>,
    loan: BodyLoan<'id, Id, Backend<Id, C, X>>,
}

impl<'id, Id: SlotIdentity + 'id, C: Ieee80211Capabilities + 'id, X: ModelContext + 'id>
    PortCommands<'id> for ModelCommands<'id, Id, C, X>
{
    type Types = ModelTypes<Backend<Id, C, X>>;
    type Caps = C;

    fn install_pairwise(
        &mut self,
        association: &ModelAssociation<'id, Backend<Id, C, X>, Station>,
        slot: PairwiseKeySlot<
            'id,
            Station,
            MODEL_KEY_SLOTS,
            MODEL_KEY_SLOTS,
            <Station as ModelRole>::AssociationPath<Id>,
        >,
        _peer: IndividualAddress,
        key: Ccmp128Key,
    ) -> PairwiseIssue<'id, Self::Types, C> {
        let identity = RawKeyIdentity::new(
            slot.index(),
            slot.generation(),
            0,
            VifRole::Station,
            &self.air,
        );
        let issued = KeyIdentity {
            owned: identity.clone(),
            loan: PhantomData,
        };
        let install = ModelPairwiseInstall {
            spp: association.owner.spp(),
            association: association.owner.generation(),
            key: identity.clone(),
            material: key,
            installed: Station::pairwise_installed(ModelPairwiseKey {
                identity,
                slot: slot.into_owned(&self.loan),
            }),
        };
        (PortCommand::InstallPairwise(install), issued)
    }

    fn install_group(
        &mut self,
        association: &ModelAssociation<'id, Backend<Id, C, X>, Station>,
        permission: StationGroupPermission<'id, Backend<Id, C, X>>,
        slot: GroupKeySlot<
            'id,
            Station,
            MODEL_KEY_SLOTS,
            MODEL_KEY_SLOTS,
            <Station as ModelRole>::AssociationPath<Id>,
        >,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    ) -> PortCommand<'id, Self::Types, C> {
        PortCommand::InstallGroup(ModelGroupInstall(Station::installation(
            PendingGroupInstall {
                spp: association.owner.spp(),
                association: association.owner.generation(),
                slot: slot.into_owned(&self.loan),
                permission: permission.into_owned(&self.loan),
                key_id,
                key,
            },
        )))
    }
}

impl<'id, Id: SlotIdentity + 'id, C: Ieee80211Capabilities + 'id, X: ModelContext + 'id>
    AccessPointCommands<'id> for ModelCommands<'id, Id, C, X>
{
    type Types = ModelTypes<Backend<Id, C, X>>;
    type Caps = C;

    fn install_access_point_pairwise(
        &mut self,
        association: &ModelAssociation<'id, Backend<Id, C, X>, AccessPoint>,
        slot: PairwiseKeySlot<
            'id,
            AccessPoint,
            MODEL_KEY_SLOTS,
            MODEL_KEY_SLOTS,
            <AccessPoint as ModelRole>::AssociationPath<Id>,
        >,
        _peer: IndividualAddress,
        key: Ccmp128Key,
    ) -> AccessPointPairwiseIssue<'id, Self::Types, C> {
        let identity = RawKeyIdentity::new(
            slot.index(),
            slot.generation(),
            0,
            VifRole::AccessPoint,
            &self.air,
        );
        let issued = KeyIdentity {
            owned: identity.clone(),
            loan: PhantomData,
        };
        let install = ModelPairwiseInstall {
            spp: association.owner.spp(),
            association: association.owner.generation(),
            key: identity.clone(),
            material: key,
            installed: AccessPoint::pairwise_installed(ModelPairwiseKey {
                identity,
                slot: slot.into_owned(&self.loan),
            }),
        };
        (
            AccessPointCommand::InstallPairwise(ModelAccessPointPairwiseInstall(install)),
            issued,
        )
    }

    fn install_access_point_group(
        &mut self,
        association: &ModelAssociation<'id, Backend<Id, C, X>, AccessPoint>,
        permission: oer_protocols_ieee80211_contracts_port::IssuedSlot<
            'id,
            oer_protocols_radio_port::Slots<ModelGroupPermission<AccessPoint>, 1>,
            <AccessPoint as ModelRole>::GroupPermissionPath<Id>,
        >,
        slot: GroupKeySlot<
            'id,
            AccessPoint,
            MODEL_KEY_SLOTS,
            MODEL_KEY_SLOTS,
            <AccessPoint as ModelRole>::AssociationPath<Id>,
        >,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    ) -> AccessPointCommand<'id, Self::Types, C> {
        AccessPointCommand::InstallGroup(ModelAccessPointGroupInstall(ModelGroupInstall(
            AccessPoint::installation(PendingGroupInstall {
                spp: association.owner.spp(),
                association: association.owner.generation(),
                slot: slot.into_owned(&self.loan),
                permission: permission.into_owned(&self.loan),
                key_id,
                key,
            }),
        )))
    }
}

/// The call during which the model's air quarantined the installation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelPortCall {
    /// [`DataPort::service_tx`].
    ServiceTx,
    /// [`DataPort::service_arena`].
    ServiceArena,
    /// [`DataPort::receive`] or a receive owner's return.
    Receive,
    /// [`DataPort::command`].
    Command,
    /// [`DataPort::poll_stop`].
    Stop,
}

/// The generations of the installation tokens a station command names:
/// its permission, association and key slots, as their issuer recorded
/// them.
fn station_generations<'id, B: ModelComposition + 'id, C: Ieee80211Capabilities>(
    command: &PortCommand<'id, ModelTypes<B>, C>,
) -> [Option<u32>; 3] {
    let group = |install: &GroupInstallationCommand<'_, B>| match install {
        GroupInstallationCommand::Station(pending) => [
            Some(pending.association),
            Some(pending.slot.generation()),
            Some(pending.permission.generation()),
        ],
        GroupInstallationCommand::AccessPoint(pending) => [
            Some(pending.association),
            Some(pending.slot.generation()),
            Some(pending.permission.generation()),
        ],
    };
    match command {
        PortCommand::InstallPairwise(install) => [
            Some(install.association),
            Some(install.key.0.generation),
            None,
        ],
        PortCommand::InstallGroup(ModelGroupInstall(install)) => group(install),
        PortCommand::ReplaceGroup {
            permission,
            installed,
            ..
        }
        | PortCommand::SelectGroup {
            permission,
            installed,
        } => [
            Some(permission.generation()),
            Some(installed.slot.generation()),
            None,
        ],
        PortCommand::RemoveGroup { permission, key } => [
            Some(permission.generation()),
            Some(key.slot.generation()),
            None,
        ],
        PortCommand::RemovePairwise { association, key } => [
            Some(association.owner.generation()),
            Some(key.slot.generation()),
            None,
        ],
        PortCommand::Configure { permission, .. } => [Some(permission.generation()), None, None],
        PortCommand::CloseGate(_)
        | PortCommand::SetEdca(_)
        | PortCommand::SetBssColor(_)
        | PortCommand::SetHe(_) => [None, None, None],
    }
}

/// The model's completion event in [`TxFacts`].
const MODEL_TX_COMPLETION: u32 = 1;

/// What the model port held when it faulted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ModelPortObservation {
    /// Single MPDUs published to the modeled hardware.
    pub published: u8,
    /// Single MPDUs admitted and not yet published.
    pub queued: u8,
    /// A data arena was on the air.
    pub arena_on_air: bool,
    /// Received units out with the caller.
    pub rx_out: usize,
    /// Received units lent to the network.
    pub lent: usize,
    /// Key removals waiting for their last use.
    pub removals: u8,
}

/// A broken contract of the model port's own state, named with what it
/// observed. The port keeps every owner it holds; every later call that can
/// fail reports the same fault.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelPortFault {
    /// The air quarantined the installation during `call`: an injected
    /// poison, an answer of the wrong shape or a broken model invariant.
    Quarantined {
        /// The call that found it.
        call: ModelPortCall,
        /// What the port held.
        observation: ModelPortObservation,
    },
    /// The station's data arenas quarantined themselves.
    Arena {
        /// The arenas' own fault, when they recorded one.
        fault: Option<arena::ModelArenaFault>,
        /// The arena on the air then.
        on_air: Option<ArenaId>,
    },
}

/// An access-point command of the model port of `(Id, C, X)`.
type ModelAccessPointCommand<'id, Id, C, X> =
    AccessPointCommand<'id, ModelTypes<Backend<Id, C, X>>, C>;

/// The progress of an access-point command of the model port of `(Id, C, X)`.
type ModelAccessPointProgress<'id, Id, C, X> =
    AccessPointProgress<'id, ModelTypes<Backend<Id, C, X>>, C>;

/// A station attempt the model port accepted: the backend's own owners.
pub type ModelAdmitted<Id, C, X> = AdmittedTxParts<
    ModelContextOf<Backend<Id, C, X>>,
    Slot<'static, oer_protocols_radio_port::Slots<ModelTickets, MODEL_TICKETS>, ModelPool<Id, 1>>,
    ModelMpdu<'static, Backend<Id, C, X>>,
    ModelTxExecution<Backend<Id, C, X>>,
    C,
>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Radio {
    Stopped,
    Running,
    /// A fault was taken back: nothing new goes out until the stop.
    Recovering,
    Stopping {
        since: Instant,
    },
}

/// The time of one port call: the model's steps read it as their clock.
struct Turn<'a>(&'a ModelAir);
impl<'a> Turn<'a> {
    fn at(air: &'a ModelAir, now: Instant) -> Self {
        air.turn.set(Some(now));
        Self(air)
    }
}
impl Drop for Turn<'_> {
    fn drop(&mut self) {
        self.0.turn.set(None);
    }
}

/// A fresh installation of the model port: the port the actor owns, the
/// lifecycle's issuer and its resources.
pub struct ModelInstall<
    'id,
    Id: SlotIdentity + 'id,
    C: Ieee80211Capabilities + 'id = ModelCaps,
    X: ModelContext + 'id = (),
> {
    /// The port.
    pub port: ModelPort<'id, Id, C, X>,
    /// The issuer of its installation commands.
    pub commands: ModelCommands<'id, Id, C, X>,
    /// The installation's tokens.
    pub resources: ModelResources<'id, Backend<Id, C, X>>,
}

/// Why [`ModelPort::reinstall`] did not reinstall; the port comes back.
#[derive(Debug, Eq, PartialEq)]
pub enum ModelReinstallRefused {
    /// The radio runs, or work, a terminal or a received unit is still out.
    NotRetired,
    /// The installation authority refused the receipt.
    Receipt,
    /// The port faulted.
    Faulted,
}

/// The model's radio as a data port, owned by one actor.
pub struct ModelPort<
    'id,
    Id: SlotIdentity + 'id,
    C: Ieee80211Capabilities + 'id = ModelCaps,
    X: ModelContext + 'id = (),
> {
    air: Rc<ModelAir>,
    held: ModelHeld<'static, Backend<Id, C, X>>,
    driver: Driver<'static, Backend<Id, C, X>>,
    installation: Installation<Id, ModelPools>,
    loan: BodyLoan<'id, Id, Backend<Id, C, X>>,
    channel: Channel,
    addresses: [IndividualAddress; 2],
    radio: Radio,
    deadline: Option<Instant>,
    held_since: Option<(CommandKind, Instant)>,
    /// The activity of the key a held removal names, as it was before its
    /// first hold: an expired removal restores it.
    held_key: Option<HeldKey>,
    /// The access point's held command, on its own bound: it holds no
    /// station command.
    access_point_held_since: Option<(AccessPointCommandKind, Instant)>,
    /// The access point's [`Self::held_key`].
    access_point_held_key: Option<HeldKey>,
    fault: Option<ModelPortFault>,
    /// What a sink refused, oldest first: offered again before anything
    /// newer ([`PortSink`]).
    kept: alloc::collections::VecDeque<Unkept<'id, ModelTypes<Backend<Id, C, X>>>>,
    /// The one item this call could not keep: returned at its end.
    unkept: Option<Unkept<'id, ModelTypes<Backend<Id, C, X>>>>,
}

/// Where a test makes the model's next call stop on an item its sink
/// refused ([`ModelAir::unkeep_next`](crate::ModelAir::unkeep_next)).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelUnkeep {
    /// The next refused item of a drain is not kept: the call returns it
    /// beside its answer, and what was posted after it stays posted.
    AtDrain,
    /// The next command or submission drains what was posted before it
    /// accepts its input; an item refused there comes back beside the
    /// input, whole.
    BeforeAcceptance,
}

/// Offer `item` to `sink`; refused, it comes back.
fn offer<'id, T: PortTypes<'id>>(
    sink: &mut impl PortSink<'id, T>,
    item: Unkept<'id, T>,
) -> Result<(), Unkept<'id, T>> {
    match item {
        Unkept::Terminal(terminal) => sink.terminal(terminal).map_err(Unkept::Terminal),
        Unkept::Received(frame, meta) => sink
            .received(frame, meta)
            .map_err(|(frame, meta)| Unkept::Received(frame, meta)),
    }
}

impl<'id, Id: SlotIdentity + 'id, C: Ieee80211Capabilities + 'id, X: ModelContext + 'id>
    ModelPort<'id, Id, C, X>
{
    /// The first installation of `installations` on a new modeled world, and
    /// the air it plays on. `None` once the first installation was minted.
    pub fn install(
        installations: &mut Installations<Id, ModelPools>,
        channel: Channel,
        addresses: [IndividualAddress; 2],
        clock: Rc<dyn Clock>,
    ) -> Option<(ModelInstall<'id, Id, C, X>, Rc<ModelAir>)> {
        let air = Rc::new(ModelAir::new(clock));
        let minted = installations.first()?;
        let network = alloc::sync::Arc::default();
        Some((
            Self::from_mint(minted, channel, addresses, air.clone(), network),
            air,
        ))
    }

    fn from_mint(
        minted: Minted<Id, ModelPools>,
        channel: Channel,
        addresses: [IndividualAddress; 2],
        air: Rc<ModelAir>,
        network: alloc::sync::Arc<ModelNetwork>,
    ) -> ModelInstall<'id, Id, C, X> {
        let Minted { mint, installation } = minted;
        let loan: BodyLoan<'id, Id, Backend<Id, C, X>> = installation.loan();
        let (mut held, resources) =
            installation::installation_parts(&air, mint, installation.generation());
        // Units the network still holds count against this installation's
        // lend limit until it drops them.
        held.network = network;
        ModelInstall {
            port: Self {
                air: air.clone(),
                held,
                driver: Driver::new(),
                installation,
                loan,
                channel,
                addresses,
                radio: Radio::Stopped,
                deadline: None,
                held_since: None,
                held_key: None,
                access_point_held_since: None,
                access_point_held_key: None,
                kept: alloc::collections::VecDeque::new(),
                unkept: None,
                fault: None,
            },
            commands: ModelCommands { air, loan },
            resources: resources.lend(&loan),
        }
    }

    /// The next installation on the same modeled world, after the retire
    /// checks: the radio stopped, no work, terminal or received unit out.
    /// Units the network holds stay lent; their credit carries over.
    ///
    /// # Errors
    ///
    /// The port, unchanged, with why it did not reinstall.
    pub fn reinstall(
        self,
        installations: &mut Installations<Id, ModelPools>,
    ) -> Result<ModelInstall<'id, Id, C, X>, Box<(Self, ModelReinstallRefused)>> {
        if let Err(refused) = self.retirable() {
            return Err(Box::new((self, refused)));
        }
        let Self {
            air,
            held,
            mut driver,
            installation,
            channel,
            addresses,
            ..
        } = self;
        match installations.remint(installation) {
            Ok(minted) => {
                // The installed keys end with their installation.
                for key in driver.keys.drain(..) {
                    air.remove_key_material(&key);
                }
                Ok(Self::from_mint(
                    minted,
                    channel,
                    addresses,
                    air,
                    held.network.clone(),
                ))
            }
            Err(RemintRefused::Foreign(installation) | RemintRefused::Exhausted(installation)) => {
                let loan: BodyLoan<'id, Id, Backend<Id, C, X>> = installation.loan();
                Err(Box::new((
                    Self {
                        air,
                        held,
                        driver,
                        installation,
                        loan,
                        channel,
                        addresses,
                        radio: Radio::Stopped,
                        deadline: None,
                        held_since: None,
                        held_key: None,
                        access_point_held_since: None,
                        access_point_held_key: None,
                        kept: alloc::collections::VecDeque::new(),
                        unkept: None,
                        fault: None,
                    },
                    ModelReinstallRefused::Receipt,
                )))
            }
        }
    }

    /// The retire checks [`Self::reinstall`] makes, without reinstalling:
    /// no fault, the radio stopped, no work, terminal or received unit out.
    ///
    /// # Errors
    ///
    /// Why the port does not retire.
    pub fn retirable(&self) -> Result<(), ModelReinstallRefused> {
        if self.fault.is_some() || self.air.failed() {
            return Err(ModelReinstallRefused::Faulted);
        }
        let retired = self.radio == Radio::Stopped
            && self.idle()
            && self.held.events.is_empty()
            && self.kept.is_empty()
            && self.held.rx_out.get() == 0
            && self.driver.removing.is_empty();
        if retired {
            Ok(())
        } else {
            Err(ModelReinstallRefused::NotRetired)
        }
    }

    /// The air this port plays on.
    pub fn air(&self) -> &Rc<ModelAir> {
        &self.air
    }

    /// The installation's generation.
    pub fn generation(&self) -> u32 {
        self.installation.generation()
    }

    /// The fault the port recorded, if any.
    pub const fn fault(&self) -> Option<ModelPortFault> {
        self.fault
    }

    fn observation(&self) -> ModelPortObservation {
        let count = |n: usize| u8::try_from(n).unwrap_or(u8::MAX);
        ModelPortObservation {
            published: count(
                self.driver
                    .queues
                    .iter()
                    .filter(|queue| queue.air.is_some())
                    .count(),
            ),
            queued: count(self.driver.queues.iter().map(|q| q.waiting.len()).sum()),
            arena_on_air: self.arena_on_air(),
            rx_out: self.held.rx_out.get(),
            lent: self.held.network.held(),
            removals: count(self.driver.removing.len()),
        }
    }

    fn arena_on_air(&self) -> bool {
        self.held
            .arenas
            .try_borrow()
            .map_or(true, |arenas| arenas.on_air().is_some())
    }

    /// Whether no single MPDU is queued or published and no arena is on air.
    fn idle(&self) -> bool {
        self.driver
            .queues
            .iter()
            .all(|queue| queue.air.is_none() && queue.waiting.is_empty())
            && !self.arena_on_air()
    }

    /// Record `fault` once and report the first one.
    fn fail(&mut self, fault: ModelPortFault) -> ModelPortFault {
        *self.fault.get_or_insert(fault)
    }

    /// The recorded fault, or the air's quarantine found during `call`.
    fn check(&mut self, call: ModelPortCall) -> Result<(), ModelPortFault> {
        if let Some(fault) = self.fault {
            return Err(fault);
        }
        if self.air.failed() {
            let observation = self.observation();
            let fault = ModelPortFault::Quarantined { call, observation };
            return Err(self.fail(fault));
        }
        Ok(())
    }

    /// Move every terminal and received unit the model's steps posted to
    /// `sink`, in the order they were posted, behind what a sink refused
    /// before. After a confirmed stop what the port keeps leaves only
    /// through [`DataPort::take_unconsumed`], so nothing newer passes it.
    fn drain(&mut self, sink: &mut impl PortSink<'id, ModelTypes<Backend<Id, C, X>>>) {
        if self.radio == Radio::Stopped && !self.kept.is_empty() {
            self.keep_posted();
            return;
        }
        if self.offer_kept(sink) == Unconsumed::Kept {
            self.keep_posted();
            return;
        }
        while self.unkept.is_none()
            && let Some(event) = self.held.take_event()
        {
            if let Err(refused) = offer(sink, self.lend_event(event)) {
                if self.air.take_unkeep(ModelUnkeep::AtDrain) {
                    // The call's one unkept item: nothing more is taken,
                    // what was posted after it stays posted.
                    self.unkept = Some(refused);
                    return;
                }
                // Everything posted after it waits behind it.
                self.kept.push_back(refused);
                self.keep_posted();
                return;
            }
        }
    }

    /// The call's answer `answer`, beside its one unkept item, if any.
    fn answer<I, R>(&mut self, answer: R) -> PortAnswer<'id, ModelTypes<Backend<Id, C, X>>, I, R> {
        match self.unkept.take() {
            Some(unkept) => Err(Unkeeping {
                unkept,
                input: Input::Accepted(answer),
            }),
            None => Ok(answer),
        }
    }

    /// Before a command or a submission accepts its input: the drain a
    /// test asked for, and the item it could not keep.
    fn unkept_before_acceptance(
        &mut self,
        sink: &mut impl PortSink<'id, ModelTypes<Backend<Id, C, X>>>,
    ) -> Option<Unkept<'id, ModelTypes<Backend<Id, C, X>>>> {
        if !self.air.take_unkeep(ModelUnkeep::BeforeAcceptance) {
            return None;
        }
        self.air.unkeep_next(ModelUnkeep::AtDrain);
        self.drain(sink);
        // An item the drain did take leaves the request spent.
        let _spent = self.air.take_unkeep(ModelUnkeep::AtDrain);
        self.unkept.take()
    }

    /// A posted event in the caller's brand.
    fn lend_event(
        &self,
        event: OwnedLowerMacEvent<'static, Backend<Id, C, X>>,
    ) -> Unkept<'id, ModelTypes<Backend<Id, C, X>>> {
        match event {
            OwnedLowerMacEvent::Terminal(terminal) => Unkept::Terminal(terminal.lend(&self.loan)),
            OwnedLowerMacEvent::Received { frame, meta } => {
                Unkept::Received(frame.lend(&self.loan), meta)
            }
        }
    }

    /// Keep every posted event behind what the port keeps, in order: a
    /// posted event makes the port's deadline due, a kept one does not.
    fn keep_posted(&mut self) {
        while let Some(event) = self.held.take_event() {
            let kept = self.lend_event(event);
            self.kept.push_back(kept);
        }
    }

    /// Offer what the port keeps to `sink`, oldest first, until it refuses
    /// one again.
    fn offer_kept(
        &mut self,
        sink: &mut impl PortSink<'id, ModelTypes<Backend<Id, C, X>>>,
    ) -> Unconsumed {
        while let Some(item) = self.kept.pop_front() {
            if let Err(item) = offer(sink, item) {
                self.kept.push_front(item);
                return Unconsumed::Kept;
            }
        }
        Unconsumed::None
    }

    /// End each queued single MPDU whose lifetime passed at `now`.
    fn expire(&mut self, now: Instant) {
        for queue in &mut self.driver.queues {
            while let Some(work) = queue.waiting.front() {
                let lifetime = self
                    .air
                    .edca(work.role)
                    .access_category(work.queue)
                    .msdu_lifetime;
                if !work.expired(now, lifetime) {
                    break;
                }
                let Some(work) = queue.waiting.pop_front() else {
                    break;
                };
                self.air.expired(1);
                terminal(&self.held, *work, TxOutcome::Expired { msdus: 1 });
            }
        }
    }

    /// End a refused attempt at once, withdrawn and never published: its
    /// terminal, with every owner, goes to `sink`.
    fn withdraw(
        &mut self,
        attempt: ModelAdmitted<Id, C, X>,
        reason: SubmitRefusal,
        cause: WithdrawCause,
        sink: &mut impl PortSink<'id, ModelTypes<Backend<Id, C, X>>>,
    ) -> SubmitRefusal {
        self.held.post(OwnedLowerMacEvent::Terminal(
            OwnedLowerMacTerminal::TxCompleted(OwnedTxCompleted::of(
                attempt.operation.complete(TxOutcome::Withdrawn(cause)),
                attempt.execution.beacon,
            )),
        ));
        self.drain(sink);
        reason
    }

    /// Close a closing gate once nothing it waits for is on the air.
    fn close_gate(&mut self) {
        if !self.idle() {
            return;
        }
        self.driver.gate = match core::mem::replace(&mut self.driver.gate, DriverGate::Closed) {
            DriverGate::Closing(gate) => {
                let closed = self.held.gate_issuer.close(gate);
                self.held.post(OwnedLowerMacEvent::Terminal(
                    OwnedLowerMacTerminal::GateClosed(closed),
                ));
                DriverGate::Closed
            }
            state => state,
        };
    }

    /// Serve the published data arena at `now`; its end returns here.
    fn serve_arena(
        &mut self,
        now: Instant,
    ) -> Result<Option<PortCompleted<'id, Self>>, ModelPortFault> {
        let results = self.air.held_results.get();
        let abort = self.held.draining.get();
        let ticket = self.arena_ticket();
        let progress = ticket.map(|ticket| self.air.air_progress(ticket));
        let on_air = progress == Some(AirProgress::Pending);
        let collided = progress == Some(AirProgress::Collided);
        let served = self.held.arenas.try_borrow_mut().map(|mut arenas| {
            arenas.serve::<C>(
                now,
                |category| *category.select(&results) || on_air,
                abort,
                collided,
            )
        });
        let Ok(served) = served else {
            return Err(self.fail(ModelPortFault::Arena {
                fault: None,
                on_air: None,
            }));
        };
        match served {
            Ok(Some((done, _))) => {
                if let Some(ticket) = ticket {
                    self.air.forget_air(ticket);
                }
                Ok(Some(done))
            }
            Ok(None) => Ok(None),
            Err(fault) => {
                let on_air = self.held.arenas.try_borrow().ok().and_then(|a| a.on_air());
                Err(self.fail(ModelPortFault::Arena {
                    fault: Some(fault),
                    on_air,
                }))
            }
        }
    }

    /// The exchange of the data arena on a timed air.
    fn arena_ticket(&self) -> Option<AirTicket> {
        self.held
            .arenas
            .try_borrow()
            .ok()
            .and_then(|arenas| arenas.on_air_ticket())
    }

    /// Whether the air still owes the data arena its end.
    fn arena_waits_for_air(&self) -> bool {
        self.arena_ticket()
            .is_some_and(|ticket| self.air.air_progress(ticket) == AirProgress::Pending)
    }

    /// Whether the single MPDUs have nothing to serve but ends the air owes.
    fn singles_wait_for_air(&self) -> bool {
        self.driver.queues.iter().all(|queue| {
            queue.waiting.is_empty()
                && queue.air.as_ref().is_none_or(|publication| {
                    publication
                        .air
                        .is_some_and(|ticket| self.air.air_progress(ticket) == AirProgress::Pending)
                })
        })
    }

    /// Recompute the cached deadline after the queues changed at `now`.
    fn refresh_deadline(&mut self, now: Instant) {
        let mut deadline: Option<Instant> = None;
        let mut due = |at: Instant| {
            deadline = Some(deadline.map_or(at, |earlier| earlier.min(at)));
        };
        if !self.held.events.is_empty() {
            due(now);
        }
        let busy = self.air.held.get();
        let results = self.air.held_results.get();
        let arena_waits = self.arena_waits_for_air();
        for ((queue, busy), result_held) in self.driver.queues.iter().zip(busy).zip(results) {
            if let Some(publication) = &queue.air {
                if publication
                    .air
                    .is_some_and(|ticket| self.air.air_progress(ticket) == AirProgress::Pending)
                {
                    // The air's end wakes the actor.
                } else if result_held {
                    due(Instant::from_micros(publication.started)
                        .saturating_add(MODEL_PUBLICATION_BOUND));
                } else {
                    due(now);
                }
            } else if let Some(work) = queue.waiting.front() {
                if busy {
                    let lifetime = self
                        .air
                        .edca(work.role)
                        .access_category(work.queue)
                        .msdu_lifetime;
                    if let Some(at) = work.operation.payload().admitted_monotonic() {
                        due(at.saturating_add(lifetime));
                    }
                    if let Some(deadline) = work.deadline {
                        due(deadline);
                    }
                } else {
                    due(now);
                }
            }
        }
        if let Ok(arenas) = self.held.arenas.try_borrow()
            && let Some(at) = arenas.due(
                now,
                |category| *category.select(&results) || arena_waits,
                self.held.draining.get(),
            )
        {
            due(at);
        }
        if let Radio::Stopping { since } = self.radio {
            due(since.saturating_add(MODEL_STOP_BOUND));
        }
        for since in [
            self.held_since.map(|(_, since)| since),
            self.access_point_held_since.map(|(_, since)| since),
        ]
        .into_iter()
        .flatten()
        {
            due(since.saturating_add(MODEL_HELD_COMMAND_BOUND));
        }
        self.deadline = deadline;
    }

    /// Take a pending removal of `key` back out of the driver, to hold its
    /// command; `None` once it finished.
    fn unremove(
        &mut self,
        key: &RawKeyIdentity,
    ) -> Option<OwnedRemoved<'static, Backend<Id, C, X>>> {
        let index = self
            .driver
            .removing
            .iter()
            .position(|removal| &removal.key == key)?;
        Some(self.driver.removing.remove(index).removed)
    }

    /// Run one command.
    fn run(
        &mut self,
        command: PortCommand<'id, ModelTypes<Backend<Id, C, X>>, C>,
        now: Instant,
    ) -> CommandProgress<'id, ModelTypes<Backend<Id, C, X>>, C> {
        let group_held = self.air.group_held.get() && !self.held.draining.get();
        match command {
            PortCommand::SetEdca(parameters) => {
                self.air
                    .edca
                    .set_parameters(VifRole::Station, parameters.get());
                CommandProgress::Done
            }
            PortCommand::SetBssColor(_) => CommandProgress::Done,
            PortCommand::SetHe(he @ oer_protocols_ieee80211_contracts_port::StationHe::He20(_))
                if self.air.take_he_association_refusal() =>
            {
                CommandProgress::Refused(PortCommand::SetHe(he), CommandRefusal::Busy)
            }
            PortCommand::SetHe(he) => {
                self.air.set_he(he);
                CommandProgress::Done
            }
            PortCommand::CloseGate(gate) => {
                if !matches!(self.driver.gate, DriverGate::Open) {
                    return CommandProgress::Refused(
                        PortCommand::CloseGate(gate),
                        CommandRefusal::GateClosed,
                    );
                }
                self.held.gate_closed.set(true);
                self.driver.gate = DriverGate::Closing(gate.into_owned(&self.loan));
                self.driver.publish(&self.held);
                self.close_gate();
                CommandProgress::Done
            }
            PortCommand::Configure {
                permission,
                receive,
            } => {
                let station_on_air = |driver: &Driver<'static, Backend<Id, C, X>>| {
                    driver.queues.iter().any(|queue| {
                        queue
                            .air
                            .as_ref()
                            .is_some_and(|publication| publication.work.role() == VifRole::Station)
                    })
                };
                if !self.arena_on_air() {
                    self.driver
                        .publish_before(&self.held, |role| role == VifRole::Station);
                }
                if self.arena_on_air() || station_on_air(&self.driver) {
                    self.held.configuring_station.set(true);
                    return CommandProgress::Held(PortCommand::Configure {
                        permission,
                        receive,
                    });
                }
                self.held.configuring_station.set(false);
                self.held.post(OwnedLowerMacEvent::Terminal(
                    OwnedLowerMacTerminal::Configured(OwnedConfigured::Station(
                        permission.into_owned(&self.loan),
                    )),
                ));
                CommandProgress::Done
            }
            PortCommand::InstallPairwise(install) => {
                self.install_pairwise_key(install);
                CommandProgress::Done
            }
            PortCommand::InstallGroup(install) if group_held => {
                CommandProgress::Held(PortCommand::InstallGroup(install))
            }
            PortCommand::InstallGroup(ModelGroupInstall(install)) => {
                self.driver.group_change(
                    &self.held,
                    now.as_micros(),
                    GroupChange::Install(install),
                );
                CommandProgress::Done
            }
            command @ (PortCommand::ReplaceGroup { .. } | PortCommand::SelectGroup { .. })
                if group_held =>
            {
                CommandProgress::Held(command)
            }
            PortCommand::ReplaceGroup {
                permission,
                installed,
                key_id,
                key,
            } => {
                self.driver.group_change(
                    &self.held,
                    now.as_micros(),
                    GroupChange::Replace {
                        permission: permission.into_owned(&self.loan),
                        installed: installed.into_owned(&self.loan),
                        key_id,
                        key,
                    },
                );
                CommandProgress::Done
            }
            PortCommand::SelectGroup {
                permission,
                installed,
            } => {
                self.driver.group_change(
                    &self.held,
                    now.as_micros(),
                    GroupChange::Select {
                        permission: permission.into_owned(&self.loan),
                        installed: installed.into_owned(&self.loan),
                    },
                );
                CommandProgress::Done
            }
            command @ PortCommand::RemoveGroup { .. } if group_held => {
                CommandProgress::Held(command)
            }
            PortCommand::RemoveGroup { permission, key } => {
                let identity = key.identity.clone();
                self.driver.group_change(
                    &self.held,
                    now.as_micros(),
                    GroupChange::Remove {
                        key: identity.clone(),
                        removed: OwnedRemoved::retain(
                            Removed::Station(RemovedKey::Group {
                                slot: key.slot,
                                permission,
                            }),
                            &self.loan,
                        ),
                    },
                );
                self.driver.publish(&self.held);
                self.driver.finish_removals(&self.held);
                match self
                    .unremove(&identity)
                    .map(|removed| removed.lend(&self.loan))
                {
                    Some(Removed::Station(RemovedKey::Group { slot, permission })) => {
                        CommandProgress::Held(PortCommand::RemoveGroup {
                            permission,
                            key: ModelGroupKey { identity, slot },
                        })
                    }
                    Some(other) => {
                        self.restore_removal(identity, OwnedRemoved::retain(other, &self.loan));
                        CommandProgress::Done
                    }
                    None => CommandProgress::Done,
                }
            }
            PortCommand::RemovePairwise { association, key } => {
                let ModelPairwiseKey { identity, slot } = key;
                identity.0.active.set(false);
                self.driver.removing.push(Removal {
                    key: identity.clone(),
                    removed: OwnedRemoved::retain(
                        Removed::Station(RemovedKey::Pairwise { slot, association }),
                        &self.loan,
                    ),
                    group_admitted_at: None,
                });
                self.driver.publish(&self.held);
                self.driver.finish_removals(&self.held);
                match self
                    .unremove(&identity)
                    .map(|removed| removed.lend(&self.loan))
                {
                    Some(Removed::Station(RemovedKey::Pairwise { slot, association })) => {
                        CommandProgress::Held(PortCommand::RemovePairwise {
                            association,
                            key: ModelPairwiseKey { identity, slot },
                        })
                    }
                    Some(other) => {
                        self.restore_removal(identity, OwnedRemoved::retain(other, &self.loan));
                        CommandProgress::Done
                    }
                    None => CommandProgress::Done,
                }
            }
        }
    }

    /// A removal of another shape stays the driver's; its terminal comes
    /// from a later service.
    fn restore_removal(
        &mut self,
        key: RawKeyIdentity,
        removed: OwnedRemoved<'static, Backend<Id, C, X>>,
    ) {
        self.driver.removing.push(Removal {
            key,
            removed,
            group_admitted_at: None,
        });
    }

    fn stop_observations(&self) -> StopObservations {
        let observation = self.observation();
        StopObservations {
            arena_on_air: observation.arena_on_air,
            published: observation.published,
            queued: observation.queued,
            configuring: self.held.configuring_station.get(),
        }
    }
}

/// The station's data arenas of a [`ModelPort`], lent for one turn.
pub struct ModelPortArena<'t, 'id, Id: SlotIdentity, C: Ieee80211Capabilities, X: ModelContext> {
    inner: arena::ModelDataArena<'t, 't, 'id, C, Backend<Id, C, X>>,
    channel: Channel,
    air: &'t ModelAir,
}

impl<'t, 'id: 't, Id: SlotIdentity + 'id, C: Ieee80211Capabilities + 'id, X: ModelContext + 'id>
    ModelPortArena<'t, 'id, Id, C, X>
{
    fn fault(&self) -> ModelPortFault {
        ModelPortFault::Arena {
            fault: self.inner.arena_fault(),
            on_air: self.inner.on_air(),
        }
    }
}

impl<'t, 'id: 't, Id: SlotIdentity + 'id, C: Ieee80211Capabilities + 'id, X: ModelContext + 'id>
    TxArena<'t, 'id> for ModelPortArena<'t, 'id, Id, C, X>
{
    type Caps = C;
    type Memory = ModelNetworkMemory;
    type Events = ();
    type Response = ModelBlockAckEvidence;
    type Denial = ModelAirDenied;
    type Fault = ModelPortFault;

    fn claim_arenas(&mut self) -> bool {
        self.inner.claim_arenas()
    }

    fn capacity(&self) -> u8 {
        self.inner.capacity()
    }

    fn push(
        &mut self,
        arena: &Arena,
        memory: ModelNetworkMemory,
        write: impl FnOnce(&mut [u8]) -> Option<u16>,
    ) -> Result<(), (ModelNetworkMemory, IntakeRefused)> {
        self.inner.push(arena, memory, write)
    }

    fn len(&self, arena: ArenaId) -> u8 {
        self.inner.len(arena)
    }

    fn on_air(&self) -> Option<ArenaId> {
        self.inner.on_air()
    }

    fn event_waiting(&self) -> bool {
        self.air.event_waiting.get()
    }

    fn publish(
        &mut self,
        publication: ArenaPublication<'t, C>,
        now: Instant,
    ) -> Result<(), PublishError<'t, C, ModelPortFault>> {
        match self.inner.publish(publication, now) {
            Ok(()) => Ok(()),
            Err(PublishError::Refused {
                publication,
                reason,
            }) => Err(PublishError::Refused {
                publication,
                reason,
            }),
            Err(PublishError::FaultBeforeTaking {
                publication,
                fault: ModelFault,
            }) => Err(PublishError::FaultBeforeTaking {
                publication,
                fault: self.fault(),
            }),
            Err(PublishError::Fault(ModelFault)) => Err(PublishError::Fault(self.fault())),
        }
    }

    fn complete(
        &mut self,
        (): (),
        now: Instant,
    ) -> Result<Option<Completion<ModelBlockAckEvidence, ModelAirDenied>>, ModelPortFault> {
        self.inner
            .complete((), now)
            .map_err(|ModelFault| self.fault())
    }

    fn settle(
        &mut self,
        settlement: Settlement,
        returned: impl FnMut(ModelNetworkMemory),
    ) -> Result<(), ModelPortFault> {
        self.inner
            .settle(settlement, returned)
            .map_err(|ModelFault| self.fault())
    }

    fn withdraw(
        &mut self,
        arena: &Arena,
        returned: impl FnMut(ModelNetworkMemory),
    ) -> Result<(), ModelPortFault> {
        self.inner
            .withdraw(arena, returned)
            .map_err(|ModelFault| self.fault())
    }

    fn deadline(&self) -> Option<Instant> {
        self.inner.deadline()
    }
}

impl<'t, 'id: 't, Id: SlotIdentity + 'id, C: Ieee80211Capabilities + 'id, X: ModelContext + 'id>
    PortArena<'t, 'id> for ModelPortArena<'t, 'id, Id, C, X>
{
    fn channel(&self) -> Channel {
        self.channel
    }

    fn ampdu_capabilities(&self) -> AmpduCapabilities {
        model_ampdu_capabilities(self.air.arena_frames.get())
    }
}

impl<'id, Id: SlotIdentity + 'id, C: Ieee80211Capabilities + 'id, X: ModelContext + 'id>
    DataPort<'id> for ModelPort<'id, Id, C, X>
{
    type Caps = C;
    type Types = ModelTypes<Backend<Id, C, X>>;
    type Fault = ModelPortFault;
    type Admitted = ModelAdmitted<Id, C, X>;
    type Arena<'t>
        = ModelPortArena<'t, 'id, Id, C, X>
    where
        Self: 't,
        'id: 't;

    fn channel(&self) -> Channel {
        self.channel
    }

    fn address(&self) -> IndividualAddress {
        let [station, _] = self.addresses;
        station
    }

    fn poll_facts(&mut self, cx: &mut Context<'_>) -> Facts {
        self.air.actor.set(Some(cx.waker().clone()));
        let arena_waits = self.arena_waits_for_air();
        // Only the air's end moves work that waits for it: its wake brings
        // the next facts.
        let air_owes = self.singles_wait_for_air() && (arena_waits || !self.arena_on_air());
        let tx = (!self.idle() && !air_owes) || !self.held.events.is_empty();
        let group_ready = {
            let captured = self.air.group_rx.take_batch();
            let ready = captured
                .iter()
                .any(|received| received.stage == GroupRxStage::Ready);
            for received in captured {
                self.air.group_rx.push_back(received);
            }
            ready
        };
        let streaming = {
            let stream = self.air.rx_stream.take();
            let streaming = stream.is_some();
            self.air.rx_stream.set(stream);
            streaming
        };
        // A held result gives no completion yet; the model's arena ends at
        // once otherwise.
        let completion = self
            .held
            .arenas
            .try_borrow()
            .is_ok_and(|a| a.on_air().is_some())
            && !self.air.held_results.get().contains(&true)
            && !arena_waits;
        Facts {
            tx: tx.then_some(if completion {
                TxFacts::events(MODEL_TX_COMPLETION)
            } else {
                TxFacts::DUE
            }),
            tbtt: self.air.tbtt.take(),
            rx: self.radio == Radio::Running
                && (!self.air.rx.is_empty() || group_ready || streaming),
        }
    }

    fn take_unconsumed(
        &mut self,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> Result<Unconsumed, UnconsumedRefused> {
        if self.radio != Radio::Stopped {
            return Err(UnconsumedRefused::NotStopped);
        }
        self.keep_posted();
        Ok(self.offer_kept(sink))
    }

    /// The model's stop leaves no cleanup: a stopped port has finished.
    fn clean_stopped(
        &mut self,
        _sink: &mut impl PortSink<'id, Self::Types>,
    ) -> PortAnswer<
        'id,
        Self::Types,
        core::convert::Infallible,
        Result<(), CleanupRefused<Self::Fault>>,
    > {
        #[allow(
            clippy::redundant_closure_call,
            reason = "the call's own answer, then its unkept item"
        )]
        let answer = (|| -> Result<(), CleanupRefused<Self::Fault>> {
            if self.radio != Radio::Stopped {
                return Err(CleanupRefused::NotStopped);
            }
            Ok(())
        })();
        self.answer(answer)
    }

    fn deadline(&self) -> Option<Instant> {
        [self.deadline, self.air.kept_due.get()]
            .into_iter()
            .flatten()
            .min()
    }

    fn service_tx(
        &mut self,
        _: TxFacts,
        tbtt: bool,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> PortAnswer<
        'id,
        Self::Types,
        core::convert::Infallible,
        Result<Option<PortCompleted<'id, Self>>, ModelPortFault>,
    > {
        #[allow(
            clippy::redundant_closure_call,
            reason = "the call's own answer, then its unkept item"
        )]
        let answer = (|| -> Result<Option<PortCompleted<'id, Self>>, ModelPortFault> {
            self.check(ModelPortCall::ServiceTx)?;
            if tbtt {
                sink.tbtt(oer_protocols_ieee80211_contracts_port::TbttEvent {
                    tbtt: oer_protocols_ieee80211_contracts_port::VifTsf::new(
                        VifRole::Station,
                        oer_protocols_ieee80211_contracts_port::tsf::TsfInstant::from_micros(
                            now.as_micros(),
                        ),
                    ),
                });
            }
            let air = self.air.clone();
            let _turn = Turn::at(&air, now);
            self.expire(now);
            // A held configuration withdraws its interface's attempts queued
            // behind a frame on the air, as its call did.
            let station = self.held.configuring_station.get();
            let access_point = matches!(
                self.access_point_held_since,
                Some((AccessPointCommandKind::Configure, _))
            );
            self.driver.publish_before(&self.held, |role| match role {
                VifRole::Station => station,
                VifRole::AccessPoint => access_point,
            });
            let done = self.serve_arena(now)?;
            self.close_gate();
            if !self.driver.removing.is_empty() {
                self.driver.finish_removals(&self.held);
            }
            self.drain(sink);
            self.check(ModelPortCall::ServiceTx)?;
            self.refresh_deadline(now);
            Ok(done)
        })();
        self.answer(answer)
    }

    fn service_arena(
        &mut self,
        _: TxFacts,
        now: Instant,
    ) -> Result<Option<PortCompleted<'id, Self>>, ModelPortFault> {
        self.check(ModelPortCall::ServiceArena)?;
        let air = self.air.clone();
        let _turn = Turn::at(&air, now);
        let done = self.serve_arena(now)?;
        if done.is_some() {
            self.air
                .relayed
                .set(self.air.relayed.get().saturating_add(1));
        }
        Ok(done)
    }

    fn arena(&mut self) -> ModelPortArena<'_, 'id, Id, C, X> {
        ModelPortArena {
            inner: arena::ModelDataArena::new(&self.held),
            channel: self.channel,
            air: &self.air,
        }
    }

    fn admit<'k, 'p>(
        &self,
        attempt: StationSubmission<'k, 'p, 'id, Self>,
    ) -> ModelAdmitted<Id, C, X> {
        attempt.into_parts(&self.loan)
    }

    fn submit(
        &mut self,
        attempt: ModelAdmitted<Id, C, X>,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> PortAnswer<'id, Self::Types, ModelAdmitted<Id, C, X>, Result<(), SubmitRefusal>> {
        if let Some(unkept) = self.unkept_before_acceptance(sink) {
            return Err(Unkeeping {
                unkept,
                input: Input::NotAccepted(attempt),
            });
        }
        #[allow(
            clippy::redundant_closure_call,
            reason = "the call's own answer, then its unkept item"
        )]
        let answer = (|| -> Result<(), SubmitRefusal> {
            // An attempt under a key or beacon permission of an earlier
            // installation is refused for good, with its owners, before the
            // radio's own state is asked.
            {
                let installation = self.installation.generation();
                let named = [
                    attempt
                        .execution
                        .beacon
                        .as_ref()
                        .map(|beacon| beacon.generation()),
                    attempt
                        .execution
                        .key
                        .as_ref()
                        .map(|key| key.identity.0.generation),
                ];
                let named: Vec<u32> = named.into_iter().flatten().collect();
                if let Some(issued) = first_stale(installation, &named) {
                    return Err(self.withdraw(
                        attempt,
                        SubmitRefusal::StalePermission {
                            issued,
                            installation,
                        },
                        WithdrawCause::StalePermission,
                        sink,
                    ));
                }
            }
            let refusal = match self.radio {
                Radio::Stopped => Some(SubmitRefusal::NotEnabled),
                Radio::Stopping { .. } | Radio::Recovering => Some(SubmitRefusal::Stopping),
                Radio::Running if self.fault.is_some() || self.air.admission_closed() => {
                    Some(SubmitRefusal::Stopping)
                }
                // The admission instant projects into radio time; without the
                // relation the radio does not run for this attempt.
                Radio::Running if self.air.project_time(now).is_err() => {
                    Some(SubmitRefusal::NotEnabled)
                }
                Radio::Running => None,
            };
            if let Some(reason) = refusal {
                return Err(self.withdraw(attempt, reason, WithdrawCause::Disabled, sink));
            }
            let air = self.air.clone();
            let _turn = Turn::at(&air, now);
            let mut attempt = attempt;
            let mpdu = attempt.operation.execute().mpdu;
            mpdu.admit(now);
            if mpdu.project_admission(&air).is_err() {
                return Err(self.withdraw(
                    attempt,
                    SubmitRefusal::NotEnabled,
                    WithdrawCause::Disabled,
                    sink,
                ));
            }
            let ModelTxExecution {
                plan,
                key,
                role,
                beacon,
                deadline,
            } = attempt.execution;
            let prepared = Box::new(Prepared {
                operation: attempt.operation,
                role,
                queue: attempt.access_category,
                plan,
                key,
                beacon,
                deadline,
            });
            attempt
                .access_category
                .select_mut(&mut self.driver.queues)
                .waiting
                .push_back(prepared);
            self.refresh_deadline(now);
            Ok(())
        })();
        self.answer(answer)
    }

    fn receive(&mut self, room: u8, now: Instant) -> Result<PortRxPull<'id, Self>, ModelPortFault> {
        self.check(ModelPortCall::Receive)?;
        if self.air.tbtt_landing.take() {
            self.air.tbtt.set(true);
        }
        if room == 0
            || self.radio != Radio::Running
            || self.held.rx_out.get() >= self.air.rx_limits.get().held
        {
            return Ok(RxPull::Empty);
        }
        let air = self.air.clone();
        let _turn = Turn::at(&air, now);
        if let Some(at) = air.receive_later(now) {
            return Ok(RxPull::Later(at));
        }
        let unit = if let Some(unit) = air.rx.pop_front() {
            Some(unit)
        } else {
            let mut captured = air.group_rx.take_batch();
            let ready = captured
                .iter()
                .position(|received| received.stage == GroupRxStage::Ready)
                .and_then(|at| captured.remove(at));
            for received in captured {
                air.group_rx.push_back(received);
            }
            match ready {
                Some(received) => Some((received.frame, received.meta)),
                None => {
                    let stream = air.rx_stream.take();
                    air.rx_stream.set(stream.clone());
                    stream
                }
            }
        };
        let Some((bytes, meta)) = unit else {
            // A unit may land right after the ring was read, unannounced.
            air.land();
            return Ok(RxPull::Empty);
        };
        if !self.held.charge_rx() {
            self.check(ModelPortCall::Receive)?;
            return Ok(RxPull::Empty);
        }
        let owner = OwnedRxBuffer {
            identity: PhantomData,
            bytes,
            recycled: false,
            air: Rc::downgrade(&air),
        };
        Ok(RxPull::Unit {
            frame: owner.lend(&self.loan),
            meta,
        })
    }

    fn recycle(
        &mut self,
        unit: PortRxBuffer<'id, Self>,
    ) -> Result<(), oer_protocols_ieee80211_contracts_port::RecycleRefused<PortRxBuffer<'id, Self>>>
    {
        // The count first: a return without a charge, or one the air's
        // knob refuses, leaves the unit whole.
        if self.air.take_recycle_refusal() {
            let fault = oer_protocols_ieee80211_contracts_port::ReceiveAccounting {
                operation: oer_protocols_ieee80211_contracts_port::ReceiveOperation::Recycle,
                held: self.held.rx_held(),
                requested: 1,
            };
            return Err(oer_protocols_ieee80211_contracts_port::RecycleRefused { unit, fault });
        }
        if let Err(fault) = self.held.release_rx() {
            return Err(oer_protocols_ieee80211_contracts_port::RecycleRefused { unit, fault });
        }
        let mut owner = unit.owner;
        owner.recycled = true;
        drop(owner);
        Ok(())
    }

    fn lend(
        &mut self,
        unit: PortRxBuffer<'id, Self>,
    ) -> Result<ModelLentRx, LendRefused<PortRxBuffer<'id, Self>>> {
        let reason = if self.fault.is_some() || self.air.failed() {
            Some(LendRefusal::Stopping)
        } else if !self.lend_room() {
            Some(LendRefusal::Capacity)
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(LendRefused {
                buffer: unit,
                reason,
            });
        }
        let mut owner = unit.owner;
        owner.recycled = true;
        let mut bytes = core::mem::take(&mut owner.bytes);
        let mpdu_len = bytes.len();
        bytes.resize(mpdu_len.max(MODEL_RX_STORAGE), 0);
        self.held
            .network
            .held
            .fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        drop(owner);
        // A lend without a charge breaks the model's count: the air
        // quarantines, and the next fallible call reports it.
        if self.held.release_rx().is_err() {
            self.held.poison();
        }
        Ok(ModelLentRx {
            bytes,
            mpdu_len,
            network: self.held.network.clone(),
        })
    }

    fn lend_room(&self) -> bool {
        self.held.network.held() < self.air.rx_limits.get().lent
    }

    fn lent(&self) -> usize {
        self.held.network.held()
    }

    fn poll_lend_room(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.held.network.waker.register(cx.waker());
        if self.lend_room() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }

    fn poll_lent_returned(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.held.network.waker.register(cx.waker());
        if self.held.network.held() == 0 {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }

    fn add_block_ack(
        &mut self,
        slot: ModelBlockAckSlot<'id, Backend<Id, C, X>>,
        agreement: SupportedBlockAck<C>,
    ) -> ModelBlockAck<'id, Backend<Id, C, X>> {
        self.apply_block_ack(
            slot.index(),
            Some(ModelReceiveAgreement::checked(VifRole::Station, agreement)),
        );
        ModelBlockAck { slot }
    }

    fn replace_block_ack(
        &mut self,
        running: ModelBlockAck<'id, Backend<Id, C, X>>,
        agreement: SupportedBlockAck<C>,
    ) -> ModelBlockAck<'id, Backend<Id, C, X>> {
        self.apply_block_ack(
            running.slot.index(),
            Some(ModelReceiveAgreement::checked(VifRole::Station, agreement)),
        );
        running
    }

    fn remove_block_ack(
        &mut self,
        agreement: ModelBlockAck<'id, Backend<Id, C, X>>,
    ) -> ModelBlockAckSlot<'id, Backend<Id, C, X>> {
        self.apply_block_ack(agreement.slot.index(), None);
        agreement.slot
    }

    fn open_gate(
        &mut self,
        gate: GateClosed<'id, ModelPool<Id, 0>>,
    ) -> GateOpen<'id, ModelPool<Id, 0>> {
        let open = self.held.gate_issuer.open(gate.into_owned(&self.loan));
        self.held.gate_closed.set(false);
        self.driver.gate = DriverGate::Open;
        open.lend(&self.loan)
    }

    fn command(
        &mut self,
        command: PortCommand<'id, Self::Types, C>,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> PortAnswer<
        'id,
        Self::Types,
        PortCommand<'id, Self::Types, C>,
        Result<CommandProgress<'id, Self::Types, C>, PortCommandFailed<'id, Self>>,
    > {
        if let Some(unkept) = self.unkept_before_acceptance(sink) {
            return Err(Unkeeping {
                unkept,
                input: Input::NotAccepted(command),
            });
        }
        #[allow(
            clippy::redundant_closure_call,
            reason = "the call's own answer, then its unkept item"
        )]
        let answer =
            (|| -> Result<CommandProgress<'id, Self::Types, C>, PortCommandFailed<'id, Self>> {
                // A faulted port keeps no command: it comes back beside the fault.
                if let Err(fault) = self.check(ModelPortCall::Command) {
                    return Err(CommandFailed::Returned { command, fault });
                }
                // A token of an earlier installation is refused for good, before
                // the radio's own state is asked.
                let named: Vec<u32> = station_generations(&command)
                    .into_iter()
                    .flatten()
                    .collect();
                let installation = self.installation.generation();
                if let Some(issued) = first_stale(installation, &named) {
                    return Ok(CommandProgress::Refused(
                        command,
                        CommandRefusal::StalePermission {
                            issued,
                            installation,
                        },
                    ));
                }
                if self.radio != Radio::Running {
                    return Ok(CommandProgress::Refused(
                        command,
                        CommandRefusal::NotEnabled,
                    ));
                }
                let air = self.air.clone();
                let _turn = Turn::at(&air, now);
                let kind = command.kind();
                if self.held_since.is_none_or(|(held, _)| held != kind) {
                    self.held_key = match &command {
                        PortCommand::RemovePairwise { key, .. } => Some(HeldKey::of(&key.identity)),
                        PortCommand::RemoveGroup { key, .. } => Some(HeldKey::of(&key.identity)),
                        _ => None,
                    };
                }
                let progress = self.run(command, now);
                self.drain(sink);
                if let Err(fault) = self.check(ModelPortCall::Command) {
                    return Err(match progress {
                        CommandProgress::Held(command) | CommandProgress::Refused(command, _) => {
                            CommandFailed::Returned { command, fault }
                        }
                        CommandProgress::Done => CommandFailed::Taken(fault),
                    });
                }
                let progress = match progress {
                    CommandProgress::Held(command) => {
                        let since = match self.held_since {
                            Some((held, since)) if held == kind => since,
                            _ => now,
                        };
                        if now.saturating_duration_since(since) >= MODEL_HELD_COMMAND_BOUND {
                            // It did not run: what its hold began is undone.
                            self.held_since = None;
                            if let Some(key) = self.held_key.take() {
                                key.restore();
                            }
                            if kind == CommandKind::Configure {
                                self.held.configuring_station.set(false);
                            }
                            self.refresh_deadline(now);
                            return Ok(CommandProgress::Refused(command, CommandRefusal::Expired));
                        }
                        self.held_since = Some((kind, since));
                        CommandProgress::Held(command)
                    }
                    done => {
                        self.held_since = None;
                        self.held_key = None;
                        done
                    }
                };
                self.refresh_deadline(now);
                Ok(progress)
            })();
        self.answer(answer)
    }

    fn poll_start(
        &mut self,
        _: &mut Context<'_>,
        channel: SupportedChannel<C>,
        now: Instant,
    ) -> Poll<Result<(), StartRefused<ModelPortFault>>> {
        if self.radio != Radio::Stopped {
            return Poll::Ready(Err(StartRefused::Running));
        }
        if self.fault.is_some() || self.air.admission_closed() || self.air.take_enable_refusal() {
            return Poll::Ready(Err(StartRefused::Device));
        }
        if self.air.take_unstopped_enable() {
            // The MAC began the start: its stop begins now and goes on in
            // `poll_stop`.
            self.air
                .disables
                .set(self.air.disables.get().saturating_add(1));
            self.radio = Radio::Stopping { since: now };
            self.refresh_deadline(now);
            return Poll::Ready(Err(StartRefused::Unstopped {
                cause: StartFailure::Device,
                stop: StopUnconfirmed {
                    cause: StopCause::Device,
                    observations: self.stop_observations(),
                },
            }));
        }
        self.channel = channel.channel();
        self.radio = Radio::Running;
        self.refresh_deadline(now);
        Poll::Ready(Ok(()))
    }

    fn poll_stop(
        &mut self,
        cx: &mut Context<'_>,
        now: Instant,
    ) -> Poll<Result<(), StopUnconfirmed>> {
        if self.check(ModelPortCall::Stop).is_err() {
            return Poll::Ready(Err(StopUnconfirmed {
                cause: StopCause::Device,
                observations: self.stop_observations(),
            }));
        }
        let since = match self.radio {
            Radio::Stopped => return Poll::Ready(Ok(())),
            Radio::Running | Radio::Recovering => {
                self.air
                    .disables
                    .set(self.air.disables.get().saturating_add(1));
                // A stopping port refuses the held command back to its
                // caller: its bound runs again from its next admission.
                self.held_since = None;
                self.held_key = None;
                self.access_point_held_since = None;
                self.access_point_held_key = None;
                now
            }
            Radio::Stopping { since } => since,
        };
        self.radio = Radio::Stopping { since };
        let air = self.air.clone();
        let _turn = Turn::at(&air, now);
        self.held.draining.set(true);
        self.driver.publish(&self.held);
        if self.idle() && self.held.events.is_empty() && !self.air.stop_held.get() {
            self.held.draining.set(false);
            self.radio = Radio::Stopped;
            self.air.stop_confirmed();
            // The stopped MAC receives for no interface and runs no TSF.
            self.air
                .access_point
                .set(ModelAccessPointInterface::default());
            self.refresh_deadline(now);
            return Poll::Ready(Ok(()));
        }
        if now.saturating_duration_since(since) >= MODEL_STOP_BOUND {
            self.refresh_deadline(now);
            return Poll::Ready(Err(StopUnconfirmed {
                cause: StopCause::Deadline,
                observations: self.stop_observations(),
            }));
        }
        // The air's world wakes the actor; the stop's terminals and the
        // aborted arena come from the next service, which the deadline asks
        // for at once.
        self.air.actor.set(Some(cx.waker().clone()));
        self.refresh_deadline(now);
        Poll::Pending
    }

    fn recover_for_stop(&mut self) -> bool {
        let recovered = self.air.recover();
        if recovered {
            self.fault = None;
            if self.radio == Radio::Running {
                self.radio = Radio::Recovering;
                self.held.draining.set(true);
            }
        }
        recovered
    }

    fn sample_clock(&mut self) -> Result<Ieee80211Instant, ClockError> {
        self.air.radio_now()
    }
}

impl<'id, Id: SlotIdentity + 'id, C: Ieee80211Capabilities + 'id, X: ModelContext + 'id>
    ModelPort<'id, Id, C, X>
{
    fn apply_block_ack(&self, slot: u8, wanted: Option<ModelReceiveAgreement>) {
        let mut applied = self.air.receive_agreements.take();
        match wanted {
            Some(agreement) => {
                applied.insert(slot, agreement);
            }
            None => {
                applied.remove(&slot);
            }
        }
        self.air.receive_agreements.set(applied);
    }
}

/// What the model aggregates: up to `subframes` at HT and HE rates.
pub(crate) fn model_ampdu_capabilities(subframes: u8) -> AmpduCapabilities {
    AmpduCapabilities {
        max_subframes: subframes as u16,
        formats: PhyFormatSet::HT.union(PhyFormatSet::HE),
        max_length: &MODEL_AMPDU_LENGTHS,
    }
}

#[cfg(test)]
mod tests;

mod access_point;
