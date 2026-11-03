//! The station's two data arenas in the driver: built over the A-MPDU
//! chains at preparation, served in the driver's turn on a transmit edge
//! and at their deadline, and lent to the station for one synchronous step
//! through [`StationArena`] (`StationPhase::data_arena`). A published
//! arena's completion goes to the station as its transmission terminal; the
//! station settles it through the same view.
use super::*;
use crate::lower_mac::arena::{ArenaFault, ArenaPort, BlockAckAnswer, Esp32s31TxArena};
use oer_protocols_ieee80211_contracts_port::arena::{
    Arena, ArenaId, ArenaPublication, Completion, IntakeRefused, PublishError, PublishRefused,
    Settlement, TxArena,
};

/// The station's data arenas of an installation over its two chains.
pub(in crate::port) type DataArenas<'id, Id, const B: usize> =
    Esp32s31TxArena<'id, 'static, TxBacking<B>, CorePath<Id>, TX_AMPDU_SUBFRAMES>;

/// One step's arena port of an installation.
type Port<'a, 'id, Id, D, const B: usize, O> = ArenaPort<
    'a,
    'id,
    'static,
    <D as Device<B>>::Power,
    <D as Device<B>>::Entropy,
    <D as Device<B>>::Timer,
    <D as Device<B>>::Coex,
    <D as Device<B>>::Hardware,
    Sink<'id, Id, B>,
    O,
    TxBacking<B>,
    Esp32s31Caps<B>,
    B,
    CorePath<Id>,
    TX_TICKETS,
    TX_AMPDU_SUBFRAMES,
>;

/// The station's installed pairwise transmit key the arenas admit: the
/// newest active one. A key whose removal began is no longer active.
fn station_pairwise<'a, 'id, Id: SlotIdentity>(
    keys: &'a [Option<(RawKeyIdentity, Installed<'id, Id>)>],
) -> Option<&'a KeyHandle<'id, CorePath<Id>>> {
    keys.iter()
        .flatten()
        .filter_map(|(identity, installed)| match installed {
            Installed::Transmit(key) if identity.role == VifRole::Station && identity.active() => {
                Some((identity.serial, key))
            }
            _ => None,
        })
        .max_by_key(|(serial, _)| *serial)
        .map(|(_, key)| key)
}

impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    Driver<'id, Id, D, B, O>
{
    /// Whether an arena is on the air.
    pub(super) fn arena_on_air(&self) -> bool {
        self.arena
            .as_ref()
            .is_some_and(|arena| arena.on_air().is_some())
    }

    /// Whether the station's pending pairwise key removal waits for an
    /// arena's key right: a frame it encrypted is on the air, awaits
    /// settlement or is retained.
    pub(super) fn arena_holds_removed_key(&self) -> bool {
        matches!(
            &self.pending_command,
            Some(Command::Remove { identity, .. }) if identity.role == VifRole::Station
        ) && self
            .arena
            .as_ref()
            .is_some_and(DataArenas::<Id, B>::holds_key_right)
    }

    /// Whether the station may not publish an arena now: the port is not
    /// running, the gate is closed or closing, a configuration is pending,
    /// or a barrier command holds the queues as it holds the MPDUs.
    fn publication_blocked(&mut self, held: &FrontHeld<'_, Id, D, B, O>) -> bool {
        !held.enabled.get()
            || !held.want_enabled.get()
            || self.closed
            || self.closing.is_some()
            || self.configuring.is_some()
            || core_views::configuring::<Id, D, B>(self.core.view())
            || match &self.pending_command {
                Some(Command::Remove { .. })
                | Some(Command::Group {
                    command: GroupCommand::Remove { .. },
                    ..
                }) => false,
                Some(_) => true,
                None => false,
            }
    }

    /// Offer one TX edge to the arena on the air; its completion goes to
    /// the station. `true` when its timeout abort settled, so the MPDUs'
    /// latched timeouts are offered again.
    pub(super) fn service_arena(
        &mut self,
        held: &Held<'_, 'id, Id, D, B, O>,
        device: &mut D,
        wake: WifiTxWake,
    ) -> Result<bool, Fault> {
        let Some(arena) = self.arena.as_mut() else {
            return Ok(false);
        };
        if arena.on_air().is_none() {
            return Ok(false);
        }
        let was_settling = arena.settling();
        let now = device.timer().now();
        let hardware = device.hardware()?;
        let core = self.core.view().arena_core().ok_or(Fault::Capacity)?;
        let observations = self.observations.as_mut().ok_or(Fault::Capacity)?;
        let pairwise = station_pairwise(&self.keys);
        let mut sink = Sink::default();
        let (done, settled) = {
            let mut port: Port<'_, 'id, Id, D, B, O> = ArenaPort::new(
                arena,
                core,
                hardware,
                &mut self.free,
                VifRole::Station,
                pairwise,
                &mut sink,
                observations,
            );
            let done = port.service(wake, now).map_err(Fault::Arena)?;
            (done, was_settling && !port.arenas.settling())
        };
        if let Some(fault) = sink.fault() {
            return Err(fault);
        }
        // An arena releases its key right only at the station's settlement.
        if !sink.removed.is_empty() || !sink.completed.is_empty() {
            return Err(Fault::Capacity);
        }
        if let Some(done) = done {
            held.arena_completed(done);
        }
        Ok(settled)
    }
}

/// The station's data arenas, lent for one synchronous step of the phase
/// (`StationPhase::data_arena`). The driver serves the published arena's
/// hardware: [`TxArena::complete`] here observes nothing, and the arena's
/// end reaches the station as its transmission terminal. It carries its
/// phase's `'p` and its installation's `'id`: it publishes only rates that
/// phase checked.
pub struct StationArena<
    'a,
    'p,
    'id,
    Id: SlotIdentity,
    D: Device<B>,
    const B: usize,
    O: Observations,
> {
    pub(in crate::port) held: &'a FrontHeld<'id, Id, D, B, O>,
    pub(in crate::port) phase: PhantomData<fn(&'p ()) -> &'p ()>,
}

impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    StationArena<'_, '_, '_, Id, D, B, O>
{
    /// The driver's state, which only the driver's own turn borrows.
    fn driver(&self) -> Option<core::cell::RefMut<'_, Driver<'static, Id, D, B, O>>> {
        self.held.driver.try_borrow_mut().ok()
    }

    /// Quarantine the port for `fault`.
    fn fault(&self, fault: ArenaFault) -> Fault {
        let fault = Fault::Arena(fault);
        self.held.storage.poison(fault);
        fault
    }

    /// Run `step` over one step's arena port; an arena fault quarantines
    /// the port, as a key removal the arenas must not finish does.
    fn with_port<R>(
        &self,
        step: impl for<'x> FnOnce(&mut Port<'x, 'static, Id, D, B, O>) -> Result<R, ArenaFault>,
    ) -> Result<R, Fault> {
        let mut driver = self.driver().ok_or(Fault::HardwareUnavailable)?;
        let mut device = self.held.port.device()?;
        let hardware = device.hardware()?;
        let driver = &mut *driver;
        let arena = driver.arena.as_mut().ok_or(Fault::Capacity)?;
        let core = driver.core.view().arena_core().ok_or(Fault::Capacity)?;
        let observations = driver.observations.as_mut().ok_or(Fault::Capacity)?;
        let pairwise = station_pairwise(&driver.keys);
        let mut sink = Sink::default();
        let result = {
            let mut port: Port<'_, 'static, Id, D, B, O> = ArenaPort::new(
                arena,
                core,
                hardware,
                &mut driver.free,
                VifRole::Station,
                pairwise,
                &mut sink,
                observations,
            );
            step(&mut port).map_err(|fault| self.fault(fault))
        };
        let contract = sink.fault().or_else(|| {
            (!sink.removed.is_empty() || !sink.completed.is_empty()).then_some(Fault::Capacity)
        });
        if let Some(fault) = contract {
            self.held.storage.poison(fault);
            return Err(fault);
        }
        result
    }
}

impl<'p, 'id: 'p, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> TxArena<'p, 'id>
    for StationArena<'_, 'p, 'id, Id, D, B, O>
{
    type Caps = Esp32s31Caps<B>;
    type Memory = TxBacking<B>;
    type Events = WifiTxWake;
    type Response = BlockAckAnswer;
    type Denial = Infallible;
    type Fault = Fault;

    fn claim_arenas(&mut self) -> bool {
        self.driver()
            .and_then(|mut driver| driver.arena.as_mut().map(DataArenas::<Id, B>::claim))
            .unwrap_or(false)
    }

    fn capacity(&self) -> u8 {
        DataArenas::<Id, B>::CAPACITY
    }

    fn push(
        &mut self,
        arena: &Arena,
        memory: TxBacking<B>,
        write: impl FnOnce(&mut [u8]) -> Option<u16>,
    ) -> Result<(), (TxBacking<B>, IntakeRefused)> {
        let Some(mut driver) = self.driver() else {
            return Err((memory, IntakeRefused::Busy));
        };
        let Some(arenas) = driver.arena.as_mut() else {
            return Err((memory, IntakeRefused::Busy));
        };
        let pushed = arenas.push(arena.id(), memory, write);
        if let (Err((_, IntakeRefused::Quarantined)), Some(fault)) = (&pushed, arenas.fault()) {
            self.held.storage.poison(Fault::Arena(fault));
        }
        pushed
    }

    fn len(&self, arena: ArenaId) -> u8 {
        self.driver()
            .and_then(|driver| driver.arena.as_ref().map(|arenas| arenas.len(arena)))
            .unwrap_or(0)
    }

    fn on_air(&self) -> Option<ArenaId> {
        self.driver()?.arena.as_ref()?.on_air()
    }

    fn publish(
        &mut self,
        publication: ArenaPublication<'p, 'id, Esp32s31Caps<B>>,
        now: Instant,
    ) -> Result<(), PublishError<'p, 'id, Esp32s31Caps<B>, Fault>> {
        let blocked = self
            .driver()
            .is_none_or(|mut driver| driver.publication_blocked(self.held));
        if blocked {
            return Err(PublishError::Refused {
                publication,
                reason: PublishRefused::Busy,
            });
        }
        let mut publication = Some(publication);
        let published = self.with_port(|port| {
            let Some(publication) = publication.take() else {
                return Ok(Ok(()));
            };
            match port.publish(publication, now) {
                Ok(()) => Ok(Ok(())),
                Err(PublishError::Refused {
                    publication,
                    reason,
                }) => Ok(Err((publication, reason))),
                Err(PublishError::Fault(fault)) => Err(fault),
            }
        });
        match published {
            Ok(Ok(())) => {
                // The driver serves it from now on, on its new deadline.
                self.held.storage.driver.wake();
                Ok(())
            }
            Ok(Err((publication, reason))) => Err(PublishError::Refused {
                publication,
                reason,
            }),
            Err(fault) => match publication.take() {
                // The port did not see the publication: a refusal returns it.
                Some(publication) if fault == Fault::HardwareUnavailable => {
                    Err(PublishError::Refused {
                        publication,
                        reason: PublishRefused::Busy,
                    })
                }
                _ => Err(PublishError::Fault(fault)),
            },
        }
    }

    fn complete(
        &mut self,
        _: WifiTxWake,
        _: Instant,
    ) -> Result<Option<Completion<BlockAckAnswer, Infallible>>, Fault> {
        // The driver's turn serves the published arena.
        Ok(None)
    }

    fn settle(
        &mut self,
        settlement: Settlement,
        returned: impl FnMut(TxBacking<B>),
    ) -> Result<(), Fault> {
        self.with_port(|port| port.settle(settlement, returned))
    }

    fn withdraw(&mut self, arena: &Arena, returned: impl FnMut(TxBacking<B>)) -> Result<(), Fault> {
        self.with_port(|port| port.withdraw(arena, returned))
    }

    fn deadline(&self) -> Option<Instant> {
        self.driver()?.arena.as_ref()?.deadline()
    }
}
