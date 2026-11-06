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
//! past [`MODEL_HELD_COMMAND_BOUND`] is a named fault, and the port keeps
//! its owners.

use super::*;
use core::task::Context;
use oer_protocols_ieee80211_contracts_port::Ieee80211Instant;
use oer_protocols_ieee80211_contracts_port::{
    AdmittedTxParts, CommandKind, CommandProgress, CommandRefusal, CommandTypes, DataPort, Facts,
    LendRefusal, LendRefused, PairwiseIssue, PortArena, PortCommand, PortCommands, PortCompleted,
    PortRxBuffer, PortRxPull, PortSink, Removed, RemovedKey, RxPull, StartRefused,
    StationSubmission, StopCause, StopObservations, StopUnconfirmed, SubmitRefusal, TxFacts,
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

/// A pairwise installation the issuer accepted.
pub struct ModelPairwiseInstall<B: ModelComposition> {
    spp: oer_protocols_ieee80211_wire::security::Spp,
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
        let identity = RawKeyIdentity::new(slot.index(), 0, VifRole::Station, &self.air);
        let issued = KeyIdentity {
            owned: identity.clone(),
            loan: PhantomData,
        };
        let install = ModelPairwiseInstall {
            spp: association.owner.spp(),
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
                slot: slot.into_owned(&self.loan),
                permission: self.loan.reclaim_slot(permission),
                key_id,
                key,
            },
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
    /// A held command passed [`MODEL_HELD_COMMAND_BOUND`]; the port keeps it.
    CommandStalled {
        /// The held command.
        kind: CommandKind,
        /// When the port first held it.
        since: Instant,
    },
}

/// A command of the model port of `(Id, C, X)`.
type ModelCommand<'id, Id, C, X> = PortCommand<'id, ModelTypes<Backend<Id, C, X>>, C>;

/// A station attempt the model port accepted: the backend's own owners.
pub type ModelAdmitted<Id, C, X> = AdmittedTxParts<
    ModelContextOf<Backend<Id, C, X>>,
    Slot<'static, ModelTickets, ModelPool<Id, 1>>,
    ModelMpdu<'static, Backend<Id, C, X>>,
    ModelTxExecution,
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
    retained: Vec<ModelCommand<'id, Id, C, X>>,
    fault: Option<ModelPortFault>,
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
        let (mut held, resources) = installation::installation_parts(&air, mint);
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
                retained: Vec::new(),
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
        if self.fault.is_some() || self.air.failed() {
            return Err(Box::new((self, ModelReinstallRefused::Faulted)));
        }
        let retired = self.radio == Radio::Stopped
            && self.idle()
            && self.held.events.is_empty()
            && self.held.rx_out.get() == 0
            && self.driver.removing.is_empty();
        if !retired {
            return Err(Box::new((self, ModelReinstallRefused::NotRetired)));
        }
        let Self {
            air,
            held,
            mut driver,
            installation,
            channel,
            addresses,
            retained,
            ..
        } = self;
        match installations.remint(installation) {
            Ok(minted) => {
                // The installed keys end with their installation.
                for key in driver.keys.drain(..) {
                    air.remove_key_material(&key);
                }
                drop(retained);
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
                        retained,
                        fault: None,
                    },
                    ModelReinstallRefused::Receipt,
                )))
            }
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
    /// `sink`, in the order they were posted.
    fn drain(&mut self, sink: &mut impl PortSink<'id, ModelTypes<Backend<Id, C, X>>>) {
        while let Some(event) = self.held.take_event() {
            match event {
                OwnedLowerMacEvent::Terminal(terminal) => sink.terminal(terminal.lend(&self.loan)),
                OwnedLowerMacEvent::Received { frame, meta } => {
                    sink.received(frame.lend(&self.loan), meta);
                }
            }
        }
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
                if !work.operation.payload().expired(now, lifetime) {
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
        sink: &mut impl PortSink<'id, ModelTypes<Backend<Id, C, X>>>,
    ) -> SubmitRefusal {
        self.held.post(OwnedLowerMacEvent::Terminal(
            OwnedLowerMacTerminal::TxCompleted(OwnedTxCompleted::Single(
                attempt
                    .operation
                    .complete(TxOutcome::Withdrawn(WithdrawCause::Disabled)),
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
        let served =
            self.held.arenas.try_borrow_mut().map(|mut arenas| {
                arenas.serve::<C>(now, |category| *category.select(&results), abort)
            });
        let Ok(served) = served else {
            return Err(self.fail(ModelPortFault::Arena {
                fault: None,
                on_air: None,
            }));
        };
        match served {
            Ok(Some((done, _))) => Ok(Some(done)),
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
        for ((queue, busy), result_held) in self.driver.queues.iter().zip(busy).zip(results) {
            if let Some(publication) = &queue.air {
                if result_held {
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
                } else {
                    due(now);
                }
            }
        }
        if let Ok(arenas) = self.held.arenas.try_borrow()
            && let Some(at) = arenas.due(
                now,
                |category| *category.select(&results),
                self.held.draining.get(),
            )
        {
            due(at);
        }
        if let Radio::Stopping { since } = self.radio {
            due(since.saturating_add(MODEL_STOP_BOUND));
        }
        if let Some((_, since)) = self.held_since {
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
                let ModelPairwiseInstall {
                    spp,
                    key,
                    material,
                    installed,
                } = install;
                self.air.apply_association(key.0.role, spp);
                self.air.install_material(&key, material);
                self.driver.keys.push(key);
                self.held.post(OwnedLowerMacEvent::Terminal(
                    OwnedLowerMacTerminal::PairwiseKeyInstalled(installed),
                ));
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
                        permission: self.loan.reclaim_slot(permission),
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
                        permission: self.loan.reclaim_slot(permission),
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
        model_ampdu_capabilities()
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
        let tx = !self.idle() || !self.held.events.is_empty();
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
            && !self.air.held_results.get().contains(&true);
        Facts {
            tx: tx.then_some(if completion {
                TxFacts::events(MODEL_TX_COMPLETION)
            } else {
                TxFacts::DUE
            }),
            tbtt: false,
            rx: self.radio == Radio::Running
                && (!self.air.rx.is_empty() || group_ready || streaming),
        }
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
        _: bool,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> Result<Option<PortCompleted<'id, Self>>, ModelPortFault> {
        self.check(ModelPortCall::ServiceTx)?;
        let air = self.air.clone();
        let _turn = Turn::at(&air, now);
        self.expire(now);
        self.driver.publish(&self.held);
        let done = self.serve_arena(now)?;
        self.close_gate();
        if !self.driver.removing.is_empty() {
            self.driver.finish_removals(&self.held);
        }
        self.drain(sink);
        self.check(ModelPortCall::ServiceTx)?;
        self.refresh_deadline(now);
        Ok(done)
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
    ) -> Result<(), SubmitRefusal> {
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
            return Err(self.withdraw(attempt, reason, sink));
        }
        let air = self.air.clone();
        let _turn = Turn::at(&air, now);
        let mut attempt = attempt;
        let mpdu = attempt.operation.execute().mpdu;
        mpdu.admit(now);
        if mpdu.project_admission(&air).is_err() {
            return Err(self.withdraw(attempt, SubmitRefusal::NotEnabled, sink));
        }
        let ModelTxExecution { plan, key } = attempt.execution;
        let prepared = Box::new(Prepared {
            operation: attempt.operation,
            role: VifRole::Station,
            queue: attempt.access_category,
            plan,
            key,
        });
        attempt
            .access_category
            .select_mut(&mut self.driver.queues)
            .waiting
            .push_back(prepared);
        self.refresh_deadline(now);
        Ok(())
    }

    fn receive(&mut self, room: u8, now: Instant) -> Result<PortRxPull<'id, Self>, ModelPortFault> {
        self.check(ModelPortCall::Receive)?;
        if room == 0 || self.radio != Radio::Running || self.held.rx_out.get() >= MODEL_RX_EVENTS {
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

    fn recycle(&mut self, unit: PortRxBuffer<'id, Self>) {
        let mut owner = unit.owner;
        owner.recycled = true;
        drop(owner);
        // A return without a charge breaks the model's count: the air
        // quarantines, and the next fallible call reports it.
        self.held.release_rx();
    }

    fn lend(
        &mut self,
        unit: PortRxBuffer<'id, Self>,
    ) -> Result<ModelLentRx, LendRefused<PortRxBuffer<'id, Self>>> {
        let reason = if self.fault.is_some() || self.air.failed() {
            Some(LendRefusal::Stopping)
        } else if self.held.network.held() >= MODEL_RX_EVENTS {
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
        self.held.release_rx();
        Ok(ModelLentRx {
            bytes,
            mpdu_len,
            network: self.held.network.clone(),
        })
    }

    fn lend_room(&self) -> bool {
        self.held.network.held() < MODEL_RX_EVENTS
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
    ) -> Result<CommandProgress<'id, Self::Types, C>, ModelPortFault> {
        if let Err(fault) = self.check(ModelPortCall::Command) {
            self.retained.push(command);
            return Err(fault);
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
        let progress = self.run(command, now);
        self.drain(sink);
        if let Err(fault) = self.check(ModelPortCall::Command) {
            if let CommandProgress::Held(command) | CommandProgress::Refused(command, _) = progress
            {
                self.retained.push(command);
            }
            return Err(fault);
        }
        let progress = match progress {
            CommandProgress::Held(command) => {
                let since = match self.held_since {
                    Some((held, since)) if held == kind => since,
                    _ => now,
                };
                if now.saturating_duration_since(since) >= MODEL_HELD_COMMAND_BOUND {
                    self.retained.push(command);
                    return Err(self.fail(ModelPortFault::CommandStalled { kind, since }));
                }
                self.held_since = Some((kind, since));
                CommandProgress::Held(command)
            }
            done => {
                self.held_since = None;
                done
            }
        };
        self.refresh_deadline(now);
        Ok(progress)
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
                now
            }
            Radio::Stopping { since } => since,
        };
        self.radio = Radio::Stopping { since };
        let air = self.air.clone();
        let _turn = Turn::at(&air, now);
        self.held.draining.set(true);
        self.driver.publish(&self.held);
        if self.idle() && self.held.events.is_empty() {
            self.held.draining.set(false);
            self.radio = Radio::Stopped;
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

/// What the model aggregates: up to [`MODEL_AMPDU_SUBFRAMES`] subframes at
/// HT and HE rates.
pub(crate) fn model_ampdu_capabilities() -> AmpduCapabilities {
    AmpduCapabilities {
        max_subframes: MODEL_AMPDU_SUBFRAMES as u16,
        formats: PhyFormatSet::HT.union(PhyFormatSet::HE),
        max_length: &MODEL_AMPDU_LENGTHS,
    }
}

#[cfg(test)]
mod tests;
