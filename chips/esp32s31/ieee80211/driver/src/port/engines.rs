//! The S31 data port: the radio as the station's datapath actor owns it.
//!
//! [`Esp32s31Port`] owns the driver's state, the device and the settings
//! by value; one task calls it in its turns and nothing else borrows it.
//! Every call takes the turn's `now`, never reads the clock and never
//! wakes its caller: what must run again is in the returned value or in
//! the cached [`DataPort::deadline`], which each call that changes a queue
//! recomputes. The driver's steps are shared with the slot-loan path
//! through [`DriverEnv`]: here they post to the turn's [`PortSink`].
//!
//! A broken contract of the port's own state is a named [`Fault`] that
//! keeps its observations; the port keeps every owner it holds and
//! answers each later call with the same fault.

use super::driver::{self, Driver, DriverEnv, Pending, Sink, core_views, queue, settle_lent};
use super::keys::{GroupCommand, Installation, KeyIdentity, PortKeys, RawKeyIdentity, Role};
use super::*;
use crate::lower_mac::arena::{ArenaFault, ArenaPort};
use oer_protocols_ieee80211_contracts_port::arena::{
    Arena, ArenaId, ArenaPublication, Completion, IntakeRefused, PublishError, PublishRefused,
    Settlement, TxArena,
};
use oer_protocols_ieee80211_contracts_port::{
    CommandProgress, CommandRefusal, CommandTypes, DataPort, Facts, PairwiseIssue, PortArena,
    PortCommand, PortCommands, PortCompleted, PortRxBuffer, PortRxPull, PortSink, RxPull,
    StartRefused, StationSubmission, StopCause, StopObservations, StopUnconfirmed, SubmitRefusal,
    TxFacts,
};
use oer_protocols_radio_port::{Installation as Receipt, Installations, Minted};

/// The port's private conversion authority for its `'static` brand.
type Loan<'id, Id, D, const B: usize, O> = BodyLoan<'id, Id, Esp32s31Backend<Id, D, B, O>>;

/// The data port's command of the phase types `Types<Id, D, B, O>`.
type Cmd<'id, Id, D, const B: usize, O> = PortCommand<'id, Types<Id, D, B, O>, Esp32s31Caps<B>>;

/// The events one [`DataPort::poll_facts`] takes at most: a transmit word
/// and a TBTT, each sampled once, with room for a fact raised meanwhile.
const FACT_POLLS: usize = 4;

/// The ring pulls one [`DataPort::receive`] makes at most: a unit no
/// receive rule admits goes back to the ring and the pull takes the next.
const RECEIVE_STEPS: usize = 8;

/// How soon a group mutation held at its receive cut runs again: the
/// device's own drain poll interval.
const CUT_RETRY: Duration = crate::rx::receive::DRAIN_POLL;

/// An accepted pairwise installation: its identity, its slot and peer
/// installation and the key material. Only [`Esp32s31Commands`] makes one.
pub struct PairwiseInstall<Id: SlotIdentity> {
    identity: RawKeyIdentity,
    installation: Installation<'static, Id>,
    material: Ccmp128Key,
}

/// An accepted group installation. Only [`Esp32s31Commands`] makes one.
pub struct GroupInstall<Id: SlotIdentity>(GroupCommand<'static, Id>);

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations> CommandTypes<'id>
    for Types<Id, D, B, O>
{
    type PairwiseInstall = PairwiseInstall<Id>;
    type GroupInstall = GroupInstall<Id>;
}

/// The lifecycle's issuer of the port's installation commands. It mints
/// each pairwise identity from its own serials, prefixed by the
/// installation's generation, so no two installations share one.
pub struct Esp32s31Commands<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    key_live: &'static [KeyActivity; 20],
    serials: Serials,
    loan: Loan<'id, Id, D, B, O>,
}

/// Key installation serials of one installation: its generation in the
/// high word, a count in the low word.
#[derive(Clone, Copy, Debug)]
struct Serials {
    generation: u32,
    next: u32,
}

impl Serials {
    const fn new(generation: u32) -> Self {
        Self {
            generation,
            next: 1,
        }
    }

    /// The next serial. After 2^32 - 1 installations in one installation
    /// the count stays at its last value; no installation reaches it.
    fn take(&mut self) -> u64 {
        let serial = (u64::from(self.generation) << 32) | u64::from(self.next);
        self.next = self.next.saturating_add(1);
        serial
    }
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations> PortCommands<'id>
    for Esp32s31Commands<'id, Id, D, B, O>
{
    type Types = Types<Id, D, B, O>;
    type Caps = Esp32s31Caps<B>;

    fn install_pairwise(
        &mut self,
        association: &<PortKeys<Id, Station> as KeyTypes<'id>>::Association,
        slot: <PortKeys<Id, Station> as KeyTypes<'id>>::PairwiseSlot,
        peer: IndividualAddress,
        key: Ccmp128Key,
    ) -> PairwiseIssue<'id, Self::Types, Self::Caps> {
        let identity = <Station as Role<'id, Id>>::identity(
            association,
            &slot,
            self.serials.take(),
            self.key_live,
        );
        let installation =
            <Station as Role<'id, Id>>::pairwise(association, slot, peer).into_owned(&self.loan);
        (
            PortCommand::InstallPairwise(PairwiseInstall {
                identity,
                installation,
                material: key,
            }),
            KeyIdentity {
                owned: identity,
                brand: PhantomData,
            },
        )
    }

    fn install_group(
        &mut self,
        association: &<PortKeys<Id, Station> as KeyTypes<'id>>::Association,
        permission: <PortKeys<Id, Station> as KeyTypes<'id>>::GroupPermission,
        slot: <PortKeys<Id, Station> as KeyTypes<'id>>::GroupSlot,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    ) -> PortCommand<'id, Self::Types, Self::Caps> {
        PortCommand::InstallGroup(GroupInstall(
            <Station as Role<'id, Id>>::group(association, slot, permission, key_id, key)
                .into_owned(&self.loan),
        ))
    }
}

/// Where the radio is between its stop and its run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PortState {
    Stopped,
    Starting { since: Instant },
    Running,
    Stopping { since: Instant },
}

/// The station's settings, owned by the port and applied in its turn.
struct PortSettings {
    /// The station's and the access point's EDCA tables.
    edca: [EdcaParameterSet; 2],
    edca_dirty: bool,
    color: Option<BssColor>,
    /// The receive Block Ack agreements the caller wants installed.
    block_acks: [Option<core_mac::RxBlockAckValues>; 8],
    block_acks_dirty: bool,
}

impl PortSettings {
    const fn new() -> Self {
        Self {
            edca: [EdcaParameterSet::STANDARD_DEFAULT; 2],
            edca_dirty: true,
            color: None,
            block_acks: [None; 8],
            block_acks_dirty: false,
        }
    }

    fn dirty(&self) -> bool {
        self.edca_dirty || self.color.is_some() || self.block_acks_dirty
    }
}

/// The driver state of one port, placed by its owner: the first
/// installation takes it, and every later one reuses it in place.
pub struct PortDriver<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>(
    Driver<'static, Id, D, B, O>,
);

impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> PortDriver<Id, D, B, O> {
    /// A driver with no installation.
    pub const fn vacant() -> Self {
        Self(Driver::vacant())
    }
}

/// Why [`Esp32s31Port::reinstall`] installed nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReinstallRefused {
    /// The port runs, faulted, or holds admitted work, a received unit or
    /// a transmit allocation: it stays as it was.
    NotRetired,
    /// The next installation's TSF epoch repeats the current one.
    SameEpoch,
    /// The port's channel names no Wi-Fi channel.
    ChannelUnset,
    /// The receipt is gone or names another installation, or the
    /// generations are used up: the port stays as it was.
    Receipt,
    /// The retired driver kept its transmitter or a descriptor: the port
    /// keeps its fault and the new receipt.
    Resources,
}

/// One installation's port, its command issuer and its resources.
pub struct PortInstallation<
    'id,
    Id: SlotIdentity + 'id,
    D: Device<B>,
    const B: usize,
    O: Observations,
> {
    /// The radio, for the datapath actor.
    pub port: Esp32s31Port<'id, Id, D, B, O>,
    /// The installation commands' issuer, for the lifecycle.
    pub commands: Esp32s31Commands<'id, Id, D, B, O>,
    /// The installation's tokens, under the brand its tasks share.
    pub resources: Resources<'id, Id, D, B, O>,
}

/// The S31 radio of one installation as a [`DataPort`].
pub struct Esp32s31Port<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
{
    device: &'static mut D,
    shared: &'static PortSharedStorage<B, D::Lent>,
    driver: &'id mut Driver<'static, Id, D, B, O>,
    settings: PortSettings,
    state: PortState,
    gate: GateIssuer<'static, Part<Id, 1>>,
    loan: Loan<'id, Id, D, B, O>,
    /// The installation's receipt; the next installation takes it.
    receipt: Option<Receipt<Id, InstallationPools>>,
    channel: Channel,
    addresses: [IndividualAddress; 2],
    config: LowerMacConfig,
    coex: &'static D::Coex,
    /// The received units the caller holds.
    rx_out: Cell<usize>,
    /// Group installation serials.
    serials: Serials,
    expired: u32,
    /// The turn instant of the last call.
    now: Instant,
    /// The cached deadline, recomputed whenever a queue changes.
    deadline: Option<Instant>,
    /// The receive ring's continuation.
    rx_later: Option<Instant>,
    /// A group mutation held at its barrier since this instant.
    group_since: Option<Instant>,
    /// The kind of a command the caller holds at its barrier, which keeps
    /// later single MPDUs from publishing as the command queue did.
    holding: Option<oer_protocols_ieee80211_contracts_port::CommandKind>,
    /// A held receive cut runs again at the device's drain poll.
    cut_retry: bool,
    /// A held receive cut delivered a unit: more may follow at once.
    cut_due: bool,
    /// The caller opened the gate; the next turn writes it.
    reopen: bool,
    /// The stopping port served one turn, which withdrew its queues.
    stop_served: bool,
    /// The core disabled for the stop.
    core_disabled: bool,
    /// The first contract fault: every later call answers with it.
    fault: Option<Fault>,
    /// A command the fault kept with the quarantine.
    retained: Option<Cmd<'id, Id, D, B, O>>,
}

/// The port's environment for one shared driver step.
struct PortEnv<'a, 'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations, S> {
    stopping: bool,
    edca: &'a [EdcaParameterSet; 2],
    expired: &'a mut u32,
    serials: &'a mut Serials,
    rx_out: &'a Cell<usize>,
    shared: &'static PortSharedStorage<B, D::Lent>,
    loan: Loan<'id, Id, D, B, O>,
    sink: &'a mut S,
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations, S>
    DriverEnv<'static, Id, D, B, O> for PortEnv<'_, 'id, Id, D, B, O, S>
where
    S: PortSink<'id, Types<Id, D, B, O>>,
{
    fn stopping(&self) -> bool {
        self.stopping
    }
    fn lifetime(
        &self,
        vif: usize,
        category: oer_protocols_ieee80211_wire::qos::WmmAccessCategory,
    ) -> Duration {
        self.edca.get(vif).map_or(Duration::ZERO, |table| {
            table.access_category(category).msdu_lifetime
        })
    }
    fn count_expired(&mut self, msdus: u32) {
        *self.expired = self.expired.saturating_add(msdus);
    }
    fn tx_completed(&mut self, _: u8, completed: owned::OwnedTxCompleted<'static, Id, D, B, O>) {
        self.sink.terminal(completed.lend(&self.loan));
    }
    fn tbtt(&mut self, event: TbttEvent) {
        self.sink.tbtt(event);
    }
    fn received(
        &mut self,
        frame: ReceiveBuffer<'static, Id, D::Receive>,
        meta: Esp32s31RxMeta,
    ) -> Result<(), Fault> {
        self.sink.received(frame.lend(), meta);
        Ok(())
    }
    fn serial(&mut self) -> u64 {
        self.serials.take()
    }
    fn shared(&self) -> &'static PortSharedStorage<B, D::Lent> {
        self.shared
    }
    fn rx_out(&self) -> &Cell<usize> {
        self.rx_out
    }
    #[cfg(oer_debug_control)]
    fn reject_group_replacement(&mut self) -> bool {
        false
    }
}

/// The parts and the mint an installation refused, both unchanged.
pub type InstallRefused<Id, D, const B: usize, O> =
    (PortParts<D, B, O>, Minted<Id, InstallationPools>);

/// The driver, the device and the environment of one shared step.
type Step<'a, 'id, Id, D, const B: usize, O, S> = (
    &'a mut Driver<'static, Id, D, B, O>,
    &'a mut D,
    PortEnv<'a, 'id, Id, D, B, O, S>,
);

/// The transmit interrupt word of a device wake; a deadline wake has none.
fn events(wake: WifiTxWake) -> u32 {
    match wake {
        WifiTxWake::Interrupt { events } => events,
        WifiTxWake::Deadline => 0,
    }
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
    Esp32s31Port<'id, Id, D, B, O>
{
    /// Install the port on its first minted installation.
    ///
    /// # Errors
    ///
    /// The parts and the mint, unchanged, when the parts do not describe a
    /// port this chip runs (as [`Esp32s31Backend::new`] checks).
    #[inline(never)]
    pub fn install(
        parts: PortParts<D, B, O>,
        minted: Minted<Id, InstallationPools>,
        driver: &'id mut PortDriver<Id, D, B, O>,
    ) -> Result<PortInstallation<'id, Id, D, B, O>, InstallRefused<Id, D, B, O>> {
        let Ok(station) = IndividualAddress::for_interface(parts.config.station_address) else {
            return Err((parts, minted));
        };
        if parts.config.publication_timeout > PUBLICATION_BOUND
            || parts.config.publication_timeout == Duration::ZERO
            || IndividualAddress::for_interface(parts.access_point_address.get()).is_err()
            || !Esp32s31Caps::<B>::CAPS
                .supports_channel(Channel::from_wifi_channel(parts.config.channel))
            || B < crate::ordinary_tx::TX_METADATA_SIZE
                + 24
                + crate::ordinary_tx::TX_CCMP_MIC_SIZE
                + crate::ordinary_tx::TX_FCS_SIZE
            || usize::from(parts.bank) > TX_BUFFERS
            || !parts.descriptors.idle()
            || !driver.0.retired()
            || parts.storage.shared.poisoned.get().is_some()
            || !parts.storage.shared.returned.none_out()
        {
            return Err((parts, minted));
        }
        let PortParts {
            storage,
            transmitter,
            pool,
            bank,
            descriptors,
            config,
            access_point_address,
            coex,
            observations,
        } = parts;
        let PortStorage { device, shared, .. } = storage;
        let device: &'static mut D = device.get_mut();
        let shared: &'static mut PortSharedStorage<B, D::Lent> = shared;
        // The port takes each of its pool buffers at its place. A buffer
        // the pool does not give, or a place already held, quarantines the
        // installation with its allocation kept out of the bank.
        let mut seeded = true;
        for index in 0..TX_BUFFERS {
            seeded &= u8::try_from(index).ok().is_some_and(|place| {
                place >= bank
                    || claim(pool, place)
                        .is_some_and(|memory| shared.returned.seed(index, memory).is_ok())
            });
        }
        let shared: &'static PortSharedStorage<B, D::Lent> = shared;
        let mut installation = Self::assemble(Assembly {
            device,
            shared,
            driver: &mut driver.0,
            transmitter,
            descriptors,
            observations,
            config,
            coex,
            addresses: [station, access_point_address],
            minted,
        });
        if !seeded {
            installation.port.fault = Some(Fault::InstallationResources);
        }
        Ok(installation)
    }

    /// Build an installation's port over its device, from its mint.
    #[inline(never)]
    fn assemble(parts: Assembly<'id, Id, D, B, O>) -> PortInstallation<'id, Id, D, B, O> {
        let Assembly {
            device,
            shared,
            driver,
            transmitter,
            descriptors,
            observations,
            config,
            coex,
            addresses,
            minted,
        } = parts;
        let now = device.timer().now();
        let Fresh {
            issuer,
            loan,
            receipt,
            generation,
            prepared,
            commands,
            resources,
        } = Fresh::mint(
            &mut *driver,
            shared,
            (transmitter, descriptors, observations),
            config,
            coex,
            minted,
            now,
        );
        let port = Self {
            device,
            shared,
            driver,
            settings: PortSettings::new(),
            state: PortState::Stopped,
            gate: issuer,
            loan,
            receipt: Some(receipt),
            channel: Channel::from_wifi_channel(config.channel),
            addresses,
            config,
            coex,
            rx_out: Cell::new(0),
            serials: Serials::new(generation),
            expired: 0,
            now: Instant::from_micros(0),
            deadline: None,
            rx_later: None,
            group_since: None,
            holding: None,
            cut_retry: false,
            cut_due: false,
            reopen: false,
            stop_served: false,
            core_disabled: false,
            fault: prepared.err(),
            retained: None,
        };
        PortInstallation {
            port,
            commands,
            resources,
        }
    }

    /// Install the next installation in place, on the same device, driver
    /// and allocations, with a distinct TSF epoch, once the stopped port
    /// passed its retire checks: no admitted work, no received unit held,
    /// every transmit allocation home. Units the network still holds stay
    /// counted by the lend credit and rejoin the new ring when it drops
    /// them; the reinstall neither waits for them nor resets the chip.
    ///
    /// # Errors
    ///
    /// Why nothing was installed; every refusal but
    /// [`ReinstallRefused::Resources`] leaves the port as it was.
    #[inline(never)]
    pub fn reinstall(
        &mut self,
        installations: &mut Installations<Id, InstallationPools>,
        tsf_epoch: u32,
    ) -> Result<
        (
            Esp32s31Commands<'id, Id, D, B, O>,
            Resources<'id, Id, D, B, O>,
        ),
        ReinstallRefused,
    > {
        if !self.retirable() {
            return Err(ReinstallRefused::NotRetired);
        }
        if tsf_epoch == self.config.tsf_epoch {
            return Err(ReinstallRefused::SameEpoch);
        }
        let Some(channel) = self.channel.wifi_channel() else {
            return Err(ReinstallRefused::ChannelUnset);
        };
        let Some(receipt) = self.receipt.take() else {
            return Err(ReinstallRefused::Receipt);
        };
        let minted = match installations.remint(receipt) {
            Ok(minted) => minted,
            Err(
                oer_protocols_radio_port::RemintRefused::Foreign(receipt)
                | oer_protocols_radio_port::RemintRefused::Exhausted(receipt),
            ) => {
                self.receipt = Some(receipt);
                return Err(ReinstallRefused::Receipt);
            }
        };
        let Some(reusable) = self.retire_driver() else {
            // The mint drops unissued; its receipt names the current
            // installation, which keeps the fault.
            self.receipt = Some(minted.installation);
            self.record(Fault::InstallationResources);
            return Err(ReinstallRefused::Resources);
        };
        self.config.channel = channel;
        self.config.tsf_epoch = tsf_epoch;
        let now = self.device.timer().now();
        let Fresh {
            issuer,
            loan,
            receipt,
            generation,
            prepared,
            commands,
            resources,
        } = Fresh::mint(
            &mut *self.driver,
            self.shared,
            reusable,
            self.config,
            self.coex,
            minted,
            now,
        );
        self.settings = PortSettings::new();
        self.state = PortState::Stopped;
        self.gate = issuer;
        self.loan = loan;
        self.receipt = Some(receipt);
        self.channel = Channel::from_wifi_channel(channel);
        self.rx_out.set(0);
        self.serials = Serials::new(generation);
        self.expired = 0;
        self.now = Instant::from_micros(0);
        self.deadline = None;
        self.rx_later = None;
        self.group_since = None;
        self.holding = None;
        self.cut_retry = false;
        self.cut_due = false;
        self.reopen = false;
        self.stop_served = false;
        self.core_disabled = false;
        self.fault = prepared.err();
        self.retained = None;
        Ok((commands, resources))
    }

    /// The MSDUs that expired before publication in this installation.
    pub fn expired_msdus(&self) -> u32 {
        self.expired
    }

    /// The installation's first contract fault.
    pub fn fault(&self) -> Option<Fault> {
        self.fault
    }

    /// Whether the stopped installation passes its retire checks: no
    /// admitted work, no received unit held, every transmit allocation
    /// home. Units the network still holds stay counted by the lend
    /// credit, which the next installation keeps.
    fn retirable(&mut self) -> bool {
        // Units the caller lent leave its count here, as at its next pull.
        if let Err(fault) = settle_lent(&self.shared.network, &self.rx_out, self.device) {
            self.record(fault);
            return false;
        }
        self.state == PortState::Stopped
            && self.fault.is_none()
            && self.rx_out.get() == 0
            && self.shared.returned.none_out()
            && self.driver.queues.iter().all(|queue| queue.is_empty())
            && self.driver.running.iter().all(Option::is_none)
    }

    /// Retire the driver's installation and detach the device: the
    /// reusable owners, unless the driver kept one.
    fn retire_driver(&mut self) -> Option<Reusable<D, B, O>> {
        let reusable = self.driver.retire();
        for activity in &self.shared.key_live {
            activity.set(0);
        }
        self.device.detach();
        reusable
    }

    /// End the stopped port's installations after the retire checks: its
    /// device, for the owner's release. The port then holds no installation
    /// a later call could run.
    pub fn release(&mut self) -> Option<&mut D> {
        if !self.retirable() {
            return None;
        }
        let _reusable = self.retire_driver();
        Some(&mut *self.device)
    }

    fn check(&self) -> Result<(), Fault> {
        match self.fault {
            Some(fault) => Err(fault),
            // A received buffer dropped unrecycled marks its allocation lost.
            None => match self.shared.poisoned.get() {
                Some(fault) => Err(fault),
                None => Ok(()),
            },
        }
    }

    /// Record the first contract fault; later ones keep the first.
    fn record(&mut self, fault: Fault) -> Fault {
        *self.fault.get_or_insert(fault)
    }

    /// Keep a command with the quarantine of a fault.
    fn retain(&mut self, command: Cmd<'id, Id, D, B, O>) {
        match self.retained {
            None => self.retained = Some(command),
            // A later command stays quarantined with the first.
            Some(_) => {
                let _quarantined = core::mem::ManuallyDrop::new(command);
            }
        }
    }

    fn running(&self) -> bool {
        self.state == PortState::Running
    }

    fn stopping(&self) -> bool {
        matches!(self.state, PortState::Stopping { .. })
    }

    /// The driver, the device and the environment of one shared step.
    fn parts<'a, S>(&'a mut self, sink: &'a mut S) -> Step<'a, 'id, Id, D, B, O, S> {
        let stopping = self.stopping();
        (
            &mut self.driver,
            &mut *self.device,
            PortEnv {
                stopping,
                edca: &self.settings.edca,
                expired: &mut self.expired,
                serials: &mut self.serials,
                rx_out: &self.rx_out,
                shared: self.shared,
                loan: self.loan,
                sink,
            },
        )
    }

    /// Recompute the cached deadline from the queues as they are now.
    fn recompute_deadline(&mut self) {
        let mut at: Option<Instant> = None;
        let mut add = |instant: Option<Instant>| {
            if let Some(instant) = instant {
                at = Some(at.map_or(instant, |at| at.min(instant)));
            }
        };
        add(core_views::deadline::<Id, D, B>(self.driver.core.view()));
        add(self.device.next_deadline());
        add(self.rx_later);
        match self.state {
            PortState::Starting { since } | PortState::Stopping { since } => {
                add(Some(since.saturating_add(COMMAND_BOUND)));
            }
            PortState::Stopped | PortState::Running => {}
        }
        // Work a turn owes at once: a stop's withdrawals, an opened gate,
        // settings to apply.
        if (self.stopping() && !self.stop_served)
            || self.reopen
            || self.cut_due
            || self.settings.dirty()
        {
            add(Some(self.now));
        }
        if let Some(since) = self.group_since {
            add(Some(since.saturating_add(COMMAND_BOUND)));
        }
        if self.cut_retry {
            add(Some(self.now.saturating_add(CUT_RETRY)));
        }
        if !core_views::configuring::<Id, D, B>(self.driver.core.view()) {
            for pending in self.driver.queues.iter().flat_map(|queue| queue.iter()) {
                let payload = pending.operation.payload();
                if let (Some(admitted), Some(table)) = (
                    payload.buffer.admission,
                    self.settings.edca.get(driver::vif(payload.role)),
                ) {
                    add(Some(admitted.saturating_add(
                        table.access_category(pending.category).msdu_lifetime,
                    )));
                }
            }
        }
        add(self
            .driver
            .arena
            .as_ref()
            .and_then(driver::DataArenas::deadline));
        self.deadline = at;
    }

    /// Apply the settings the caller changed: EDCA, BSS color and the
    /// receive Block Ack agreements.
    fn apply_settings(&mut self) -> Result<(), Fault> {
        let driver = &mut self.driver;
        if self.settings.edca_dirty {
            let [station, _] = self.settings.edca;
            let table =
                SupportedEdca::<Esp32s31Caps<B>>::new(station).ok_or(Fault::EdcaUnsupported)?;
            let mut view = driver
                .core
                .view()
                .commands()
                .ok_or(Fault::CoreViewUnavailable(CoreOp::Command))?;
            let _installed = view
                .set_checked_edca(VifRole::Station, table)
                .map_err(|_| Fault::EdcaUnsupported)?;
            self.settings.edca_dirty = false;
        }
        if let Some(color) = self.settings.color {
            let mut view = driver
                .core
                .view()
                .commands()
                .ok_or(Fault::CoreViewUnavailable(CoreOp::Command))?;
            view.set_checked_he_bss_color(color);
            self.settings.color = None;
        }
        if self.settings.block_acks_dirty {
            for (index, old) in driver.applied_ba.iter_mut().enumerate() {
                let wanted = self.settings.block_acks.get(index).copied().flatten();
                if *old == wanted {
                    continue;
                }
                if let Some(old) = old.take() {
                    let mut view = driver
                        .core
                        .view()
                        .commands()
                        .ok_or(Fault::CoreViewUnavailable(CoreOp::Command))?;
                    view.end_checked_rx_block_ack(
                        self.device.hardware()?,
                        old.role,
                        old.peer,
                        old.tid,
                    )?;
                }
                if let Some(agreement) = wanted {
                    let mut view = driver
                        .core
                        .view()
                        .commands()
                        .ok_or(Fault::CoreViewUnavailable(CoreOp::Command))?;
                    view.start_checked_rx_block_ack(self.device.hardware()?, agreement)?;
                }
                *old = wanted;
            }
            self.settings.block_acks_dirty = false;
        }
        Ok(())
    }

    /// Whether a command the caller holds at its barrier keeps single
    /// MPDUs from publishing; a removal withdraws only its key's attempts.
    fn holding_blocks(&self) -> bool {
        use oer_protocols_ieee80211_contracts_port::CommandKind as Kind;
        matches!(
            self.holding,
            Some(Kind::InstallGroup | Kind::ReplaceGroup | Kind::SelectGroup | Kind::Configure)
        )
    }

    /// End expired queued MPDUs and publish each free queue's head.
    fn advance_queues<S>(&mut self, sink: &mut S, now: Instant) -> Result<(), Fault>
    where
        S: PortSink<'id, Types<Id, D, B, O>>,
    {
        let blocked = self.stopping() || self.holding_blocks();
        let (driver, device, mut env) = self.parts(sink);
        for index in 0..driver.queues.len() {
            // A published MPDU or arena holds the queue.
            let busy = driver.running.get(index).is_none_or(Option::is_some)
                || driver.free.get(index).is_none_or(Option::is_none);
            if busy {
                continue;
            }
            // Expiry continues behind a gate or a barrier.
            while driver
                .queues
                .get(index)
                .and_then(heapless::Deque::front)
                .is_some_and(|pending| Driver::<Id, D, B, O>::expired(&env, pending, now))
            {
                let Some(pending) = driver
                    .queues
                    .get_mut(index)
                    .and_then(heapless::Deque::pop_front)
                else {
                    break;
                };
                driver.finish(&mut env, pending, TxOutcome::Expired { msdus: 1 });
            }
            let held = blocked
                || driver.closed
                || driver.closing.is_some()
                || core_views::configuring::<Id, D, B>(driver.core.view())
                || driver
                    .queues
                    .get(index)
                    .and_then(heapless::Deque::front)
                    .is_some_and(|pending| driver.blocked(pending));
            if held {
                continue;
            }
            if let Some(pending) = driver
                .queues
                .get_mut(index)
                .and_then(heapless::Deque::pop_front)
            {
                driver.publish(device, index, pending)?;
            }
        }
        Ok(())
    }

    /// Move a station configuration on: the core's barrier, then the
    /// withdrawal of the station's queued attempts, then its terminal.
    fn advance_configuration<S>(&mut self, sink: &mut S) -> Result<(), Fault>
    where
        S: PortSink<'id, Types<Id, D, B, O>>,
    {
        if self.driver.configuring.is_none() {
            return Ok(());
        }
        // A station configuration waits for the station's published arena.
        if self.driver.arena_on_air() {
            return Ok(());
        }
        let stopping = self.stopping();
        let loan = self.loan;
        let (driver, device, mut env) = self.parts(sink);
        if core_views::configuring::<Id, D, B>(driver.core.view()) {
            let mut steps = Sink::default();
            let done = core_views::advance_configuration::<Id, D, B>(
                driver.core.view(),
                device.hardware()?,
                &mut steps,
            );
            driver.collect(&mut env, &mut steps, stopping)?;
            if !done {
                return Ok(());
            }
        }
        if driver.withdraw(&mut env)
            && let Some(permission) = driver.configuring.take()
        {
            env.sink
                .terminal(owned::OwnedTerminal::Configured(permission).lend(&loan));
        }
        Ok(())
    }

    /// Close the gate once the published arena and the core's MPDUs ended.
    fn close_quiet_gate<S>(&mut self, sink: &mut S) -> Result<(), Fault>
    where
        S: PortSink<'id, Types<Id, D, B, O>>,
    {
        if self.driver.closing.is_some()
            && !self.driver.arena_on_air()
            && !core_views::configuring::<Id, D, B>(self.driver.core.view())
            && core_views::close::<Id, D, B>(self.driver.core.view(), self.device.hardware()?)
            && let Some(gate) = self.driver.closing.take()
        {
            let gate = self.gate.close(gate);
            self.driver.closed = true;
            sink.terminal(LowerMacTerminal::GateClosed(gate.lend(&self.loan)));
        }
        Ok(())
    }

    /// Write an opened gate to the core, in the turn after the caller
    /// opened it.
    fn write_open_gate<S>(&mut self, sink: &mut S) -> Result<(), Fault>
    where
        S: PortSink<'id, Types<Id, D, B, O>>,
    {
        if !self.reopen {
            return Ok(());
        }
        let stopping = self.stopping();
        let (driver, device, mut env) = self.parts(sink);
        let mut steps = Sink::default();
        core_views::open::<Id, D, B>(driver.core.view(), device.hardware()?, &mut steps)?;
        driver.closed = false;
        driver.collect(&mut env, &mut steps, stopping)?;
        self.reopen = false;
        Ok(())
    }

    /// Disable the core of a stopping port once nothing is on the air.
    fn disable_core<S>(&mut self, sink: &mut S) -> Result<(), Fault>
    where
        S: PortSink<'id, Types<Id, D, B, O>>,
    {
        if self.core_disabled
            || self.driver.arena_on_air()
            || self.driver.configuring.is_some()
            || self.driver.closing.is_some()
            || core_views::configuring::<Id, D, B>(self.driver.core.view())
        {
            return Ok(());
        }
        let (driver, device, mut env) = self.parts(sink);
        let mut steps = Sink::default();
        core_views::disable::<Id, D, B>(driver.core.view(), device.hardware()?, &mut steps);
        driver.collect(&mut env, &mut steps, true)?;
        let mut steps = Sink::default();
        let disabled = core_views::finish_disable::<Id, D, B>(driver.core.view(), &mut steps);
        driver.collect(&mut env, &mut steps, true)?;
        self.core_disabled = disabled;
        Ok(())
    }

    /// One transmit turn: the facts' completions, a TBTT, the barriers
    /// they released, expiry and publication.
    fn turn<S>(
        &mut self,
        tx: TxFacts,
        tbtt: bool,
        now: Instant,
        sink: &mut S,
    ) -> Result<Option<PortCompleted<'id, Self>>, Fault>
    where
        S: PortSink<'id, Types<Id, D, B, O>>,
    {
        self.write_open_gate(sink)?;
        self.apply_settings()?;
        let stopping = self.stopping();
        let wake = match tx.bits() {
            0 => WifiTxWake::Deadline,
            events => WifiTxWake::Interrupt { events },
        };
        let done = {
            let (driver, device, mut env) = self.parts(sink);
            let done = driver.service_tx(&mut env, device, wake, stopping, now)?;
            if tbtt {
                let mut steps = Sink::default();
                if !core_views::configuring::<Id, D, B>(driver.core.view()) {
                    driver
                        .core
                        .view()
                        .commands()
                        .ok_or(Fault::CoreViewUnavailable(CoreOp::Tbtt))?
                        .station_tbtt(device.hardware()?, &mut steps);
                }
                driver.collect(&mut env, &mut steps, stopping)?;
            }
            done
        };
        self.advance_configuration(sink)?;
        self.close_quiet_gate(sink)?;
        self.advance_queues(sink, now)?;
        if stopping {
            self.disable_core(sink)?;
            self.stop_served = true;
        }
        Ok(done)
    }

    /// Run a group mutation behind its barrier and receive cut, or hold it.
    fn group<S>(
        &mut self,
        command: Cmd<'id, Id, D, B, O>,
        now: Instant,
        sink: &mut S,
    ) -> Result<CommandProgress<'id, Types<Id, D, B, O>, Esp32s31Caps<B>>, Fault>
    where
        S: PortSink<'id, Types<Id, D, B, O>>,
    {
        let since = *self.group_since.get_or_insert(now);
        if now.saturating_duration_since(since) >= COMMAND_BOUND {
            let kind = command.kind();
            self.retain(command);
            return Err(self.record(Fault::CommandExpired(kind)));
        }
        // The installed key the mutation uses, which no published attempt
        // may name; a removal also withdraws its key's queued attempts.
        let (uses, removal) = match &command {
            PortCommand::ReplaceGroup { installed, .. }
            | PortCommand::SelectGroup { installed, .. } => (Some(installed.identity), None),
            PortCommand::RemoveGroup { key, .. } => (Some(key.identity), Some(key.identity)),
            _ => (None, None),
        };
        if let Some(identity) = removal {
            identity.activity.set(0);
            self.driver.barrier = Some(identity);
        }
        let in_use = self
            .driver
            .running
            .iter()
            .flatten()
            .any(|running| uses.is_some() && running.operation.payload().key == uses);
        let drained = {
            let (driver, _, mut env) = self.parts(sink);
            driver.withdraw(&mut env)
        };
        if in_use || !drained {
            self.holding = Some(command.kind());
            return Ok(CommandProgress::Held(command));
        }
        let room = RX_HELD.saturating_sub(self.rx_out.get());
        let waker = core::task::Waker::noop();
        let cut = self
            .device
            .poll_rx_cut(room, &mut Context::from_waker(waker));
        match cut {
            Poll::Ready(Ok(Some(unit))) => {
                // A unit captured before the mutation precedes its terminal.
                let (driver, device, mut env) = self.parts(sink);
                driver.receive(&mut env, device, unit)?;
                self.cut_retry = false;
                self.cut_due = true;
                self.holding = Some(command.kind());
                Ok(CommandProgress::Held(command))
            }
            Poll::Pending => {
                self.cut_retry = true;
                self.cut_due = false;
                self.holding = Some(command.kind());
                Ok(CommandProgress::Held(command))
            }
            Poll::Ready(Err(cause)) => {
                self.retain(command);
                Err(self.record(cause))
            }
            Poll::Ready(Ok(None)) => {
                let owned = match command {
                    PortCommand::InstallGroup(GroupInstall(command)) => command,
                    PortCommand::ReplaceGroup {
                        permission,
                        installed,
                        key_id,
                        key,
                    } => GroupCommand::Replace {
                        permission: self.loan.reclaim_slot(permission),
                        key: installed.into_owned(),
                        key_id,
                        material: key,
                    },
                    PortCommand::SelectGroup {
                        permission,
                        installed,
                    } => GroupCommand::Select {
                        permission: self.loan.reclaim_slot(permission),
                        key: installed.into_owned(),
                    },
                    PortCommand::RemoveGroup { permission, key } => GroupCommand::Remove {
                        identity: <Station as Role<'id, Id>>::group_identity(key),
                        removal: <Station as Role<'id, Id>>::remove_group(permission)
                            .into_owned(&self.loan),
                    },
                    other => return Ok(CommandProgress::Held(other)),
                };
                let loan = self.loan;
                let terminal = {
                    let (driver, device, mut env) = self.parts(sink);
                    driver.group(&mut env, device, since, owned)?
                };
                if let Err(cause) = self.device.finish_rx_cut() {
                    // The terminal stays with the quarantine.
                    let _quarantined = core::mem::ManuallyDrop::new(terminal);
                    return Err(self.record(cause));
                }
                self.group_since = None;
                self.cut_retry = false;
                self.cut_due = false;
                self.holding = None;
                self.driver.barrier = None;
                sink.terminal(terminal.lend(&loan));
                Ok(CommandProgress::Done)
            }
        }
    }

    /// Run one command of a running port.
    fn run_command<S>(
        &mut self,
        command: Cmd<'id, Id, D, B, O>,
        now: Instant,
        sink: &mut S,
    ) -> Result<CommandProgress<'id, Types<Id, D, B, O>, Esp32s31Caps<B>>, Fault>
    where
        S: PortSink<'id, Types<Id, D, B, O>>,
    {
        match command {
            PortCommand::SetEdca(parameters) => {
                let [station, _] = &mut self.settings.edca;
                *station = parameters.get();
                self.settings.edca_dirty = true;
                self.apply_settings()?;
                Ok(CommandProgress::Done)
            }
            PortCommand::SetBssColor(color) => {
                self.settings.color = Some(color);
                self.apply_settings()?;
                Ok(CommandProgress::Done)
            }
            PortCommand::CloseGate(gate) => {
                if self.driver.closing.is_some() || self.driver.closed {
                    return Ok(CommandProgress::Refused(
                        PortCommand::CloseGate(gate),
                        CommandRefusal::GateClosed,
                    ));
                }
                core_views::hold::<Id, D, B>(self.driver.core.view());
                self.driver.closing = Some(gate.into_owned(&self.loan));
                {
                    let (driver, _, mut env) = self.parts(sink);
                    driver.withdraw(&mut env);
                }
                self.close_quiet_gate(sink)?;
                Ok(CommandProgress::Done)
            }
            PortCommand::Configure {
                permission,
                receive,
            } => {
                if self.driver.configuring.is_some() {
                    return Ok(CommandProgress::Refused(
                        PortCommand::Configure {
                            permission,
                            receive,
                        },
                        CommandRefusal::Configuring,
                    ));
                }
                // A configuration waits for the station's published arena.
                if self.driver.arena_on_air() {
                    self.holding =
                        Some(oer_protocols_ieee80211_contracts_port::CommandKind::Configure);
                    return Ok(CommandProgress::Held(PortCommand::Configure {
                        permission,
                        receive,
                    }));
                }
                let (bssid, filter) = match receive {
                    StationReceive::Off => (None, ReceiveFilter::NONE),
                    StationReceive::OtherBssManagement => {
                        (None, ReceiveFilter::OTHER_BSS_MANAGEMENT)
                    }
                    StationReceive::Bss { bssid, filter } => (Some(bssid.get()), filter.get()),
                };
                core_views::configure::<Id, D, B>(
                    self.driver.core.view(),
                    CoreVif {
                        role: VifRole::Station,
                        address: self.addresses[0].get(),
                        bssid,
                        receive: filter,
                    },
                )?;
                self.holding = None;
                self.driver.configuring = Some(owned::OwnedConfigured::Station(
                    permission.into_owned(&self.loan),
                ));
                self.advance_configuration(sink)?;
                Ok(CommandProgress::Done)
            }
            PortCommand::InstallPairwise(PairwiseInstall {
                identity,
                installation,
                material,
            }) => {
                self.driver
                    .install(self.device, identity, installation, &material)?;
                identity.activity.set(identity.serial);
                sink.terminal(
                    owned::OwnedTerminal::PairwiseKeyInstalled(owned::OwnedInstalledPairwise::new(
                        identity.role,
                        identity,
                    ))
                    .lend(&self.loan),
                );
                Ok(CommandProgress::Done)
            }
            PortCommand::RemovePairwise { association, key } => {
                let identity =
                    <PortKeys<Id, Station> as KeyTypes<'id>>::pairwise_identity(&key).owned;
                identity.activity.set(0);
                self.driver.barrier = Some(identity);
                let drained = {
                    let (driver, _, mut env) = self.parts(sink);
                    driver.withdraw(&mut env)
                };
                if !drained {
                    self.holding =
                        Some(oer_protocols_ieee80211_contracts_port::CommandKind::RemovePairwise);
                    return Ok(CommandProgress::Held(PortCommand::RemovePairwise {
                        association,
                        key,
                    }));
                }
                let removed = match self.driver.remove(self.device, identity) {
                    Ok(removed) => removed,
                    Err(fault) => {
                        self.retain(PortCommand::RemovePairwise { association, key });
                        return Err(fault);
                    }
                };
                let removal =
                    <Station as Role<'id, Id>>::removal(association).into_owned(&self.loan);
                let terminal = self.driver.removed_terminal(removal, removed)?;
                self.driver.barrier = None;
                self.holding = None;
                sink.terminal(owned::OwnedTerminal::KeyRemoved(terminal).lend(&self.loan));
                Ok(CommandProgress::Done)
            }
            group @ (PortCommand::InstallGroup(_)
            | PortCommand::ReplaceGroup { .. }
            | PortCommand::SelectGroup { .. }
            | PortCommand::RemoveGroup { .. }) => self.group(group, now, sink),
        }
    }
}

/// One installation's fresh owners, minted onto a retired driver.
struct Fresh<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations> {
    issuer: GateIssuer<'static, Part<Id, 1>>,
    loan: Loan<'id, Id, D, B, O>,
    receipt: Receipt<Id, InstallationPools>,
    generation: u32,
    prepared: Result<(), Fault>,
    commands: Esp32s31Commands<'id, Id, D, B, O>,
    resources: Resources<'id, Id, D, B, O>,
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
    Fresh<'id, Id, D, B, O>
{
    /// Mint the installation `minted` onto `driver`, which holds no
    /// installation, with the reusable owners and the configuration.
    #[inline(never)]
    fn mint(
        driver: &mut Driver<'static, Id, D, B, O>,
        shared: &'static PortSharedStorage<B, D::Lent>,
        reusable: Reusable<D, B, O>,
        config: LowerMacConfig,
        coex: &'static D::Coex,
        minted: Minted<Id, InstallationPools>,
        now: Instant,
    ) -> Self {
        let (transmitter, descriptors, observations) = reusable;
        let Minted { mint, installation } = minted;
        let loan: Loan<'id, Id, D, B, O> = installation.loan();
        let (
            core,
            (gate, issuer, arbiter_authority),
            (tickets, terminal_access),
            buffers,
            [connection_epoch],
            [station_group_permission],
            [access_point_group_permission],
            block_acks,
            station_commands,
            access_point_commands,
        ) = mint.into_pools();
        let CoreResources {
            authority,
            tickets: core_tickets,
            station_association,
            access_point_association,
            queues,
        } = core;
        let core = LowerMacCore::new(authority, transmitter, config, coex);
        // The caller checked every allocation home: each place takes its own.
        let buffers = buffers.map(|place| {
            let pair = shared.returned.take(usize::from(place.index()));
            Buffer {
                place,
                dma: buffer::ReturnableBuffer::taken(pair),
                loan: None,
                admission: None,
            }
        });
        let prepared = driver.prepare(core, core_tickets, queues, descriptors, observations, now);
        // The lend credit outlives the previous installation: units the
        // network still holds stay counted and rejoin this ring.
        shared.network.reset();
        let generation = installation.generation();
        let resources = Resources {
            tickets: tickets.map(|slot| loan.lend_slot(slot)),
            terminal_access: terminal_access.lend(&loan),
            buffers: buffers.map(|buffer| buffer.lend(&loan)),
            connection_epoch: loan.lend_slot(connection_epoch),
            station_association: station_association.lend(&loan),
            access_point_association: access_point_association.lend(&loan),
            station_commands: station_commands.lend(&loan),
            access_point_commands: access_point_commands.lend(&loan),
            block_acks: block_acks.map(|slot| slot.lend(&loan)),
            station_group_permission: loan.lend_slot(station_group_permission),
            access_point_group_permission: loan.lend_slot(access_point_group_permission),
            gate: gate.lend(&loan),
            arbiter_authority: arbiter_authority.lend(&loan),
        };
        Self {
            issuer,
            loan,
            receipt: installation,
            generation,
            prepared,
            commands: Esp32s31Commands {
                key_live: &shared.key_live,
                serials: Serials::new(generation),
                loan,
            },
            resources,
        }
    }
}

/// What [`Esp32s31Port::assemble`] builds an installation from.
struct Assembly<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    device: &'static mut D,
    shared: &'static PortSharedStorage<B, D::Lent>,
    driver: &'id mut Driver<'static, Id, D, B, O>,
    transmitter: OrdinaryTxIdleOwner<'static, D::Power, D::Entropy, D::Timer, B>,
    descriptors: TxDescriptors<B>,
    observations: O,
    config: LowerMacConfig,
    coex: &'static D::Coex,
    addresses: [IndividualAddress; 2],
    minted: Minted<Id, InstallationPools>,
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations> DataPort<'id>
    for Esp32s31Port<'id, Id, D, B, O>
{
    type Caps = Esp32s31Caps<B>;
    type Types = Types<Id, D, B, O>;
    type Fault = Fault;
    type Admitted = AdmittedTxParts<
        D::OwnedContext<Id>,
        Slot<'static, Tickets, Part<Id, 2>>,
        Mpdu<'static, Id, D, B, O>,
        buffer::Execution,
        Esp32s31Caps<B>,
    >;
    type Arena<'t>
        = Esp32s31Arena<'t, 'id, Id, D, B, O>
    where
        Self: 't,
        'id: 't;

    fn channel(&self) -> Channel {
        self.channel
    }

    fn address(&self) -> IndividualAddress {
        self.addresses[0]
    }

    fn poll_facts(&mut self, cx: &mut Context<'_>) -> Facts {
        let mut facts = Facts::default();
        if self.check().is_err() {
            // The next service answers with the fault.
            facts.tx = Some(TxFacts::DUE);
            return facts;
        }
        if !matches!(self.state, PortState::Running | PortState::Stopping { .. }) {
            return facts;
        }
        for _ in 0..FACT_POLLS {
            match self.device.poll_event(cx) {
                Poll::Ready(Ok(DeviceEvent::Tx(wake))) => {
                    let sampled = TxFacts::events(events(wake));
                    facts.tx = Some(facts.tx.map_or(sampled, |tx| tx.union(sampled)));
                }
                Poll::Ready(Ok(DeviceEvent::Tbtt)) => facts.tbtt = true,
                Poll::Ready(Err(cause)) => {
                    self.record(cause);
                    facts.tx = Some(TxFacts::DUE);
                    return facts;
                }
                Poll::Pending => break,
            }
        }
        match self.device.poll_rx_work(cx) {
            Ok(work) => facts.rx = work,
            Err(cause) => {
                self.record(cause);
                facts.tx = Some(TxFacts::DUE);
            }
        }
        // A unit the network returned rearms the ring at the next pull.
        if self.shared.network.poll_returns(cx) {
            facts.rx = true;
        }
        facts
    }

    fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    fn service_tx(
        &mut self,
        tx: TxFacts,
        tbtt: bool,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> Result<Option<PortCompleted<'id, Self>>, Self::Fault> {
        self.check()?;
        self.now = now;
        if !matches!(self.state, PortState::Running | PortState::Stopping { .. }) {
            return Ok(None);
        }
        let done = self
            .turn(tx, tbtt, now, sink)
            .map_err(|fault| self.record(fault));
        self.recompute_deadline();
        done
    }

    fn arena(&mut self) -> Self::Arena<'_> {
        Esp32s31Arena { port: self }
    }

    fn admit<'k, 'p>(&self, attempt: StationSubmission<'k, 'p, 'id, Self>) -> Self::Admitted {
        attempt.into_parts(&self.loan)
    }

    fn submit(
        &mut self,
        mut attempt: Self::Admitted,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> Result<(), SubmitRefusal> {
        let reason = match self.state {
            PortState::Running if self.check().is_ok() => None,
            PortState::Stopping { .. } => Some(SubmitRefusal::Stopping),
            _ => Some(SubmitRefusal::NotEnabled),
        };
        if let Some(reason) = reason {
            // The attempt ends at once, withdrawn and never published: its
            // terminal returns every owner.
            let completed = owned::OwnedTxCompleted::Single(attempt.operation.complete(
                oer_protocols_ieee80211_contracts_port::TxOutcome::Withdrawn(
                    oer_protocols_ieee80211_contracts_port::WithdrawCause::Disabled,
                ),
            ));
            sink.terminal(completed.lend(&self.loan));
            return Err(reason);
        }
        self.now = now;
        // Admission happens on this call: the MSDU ages from here.
        let buffer = &mut attempt.operation.execute().mpdu.buffer;
        if buffer.admission.is_none() {
            buffer.admission = Some(now);
        }
        let index = queue(attempt.access_category);
        let pending = Pending {
            operation: attempt.operation,
            plan: attempt.execution.plan,
            category: attempt.access_category,
            protection: attempt.protection,
            power: attempt.power.get(),
            coex: attempt.coex_operation,
            stage: 0,
            stage_tries: 0,
            transmissions: 0,
            publications: 0,
            air_rate: None,
        };
        let Some(queued) = self.driver.queues.get_mut(index) else {
            // The attempt stays in custody with the quarantine.
            core::mem::forget(pending);
            self.record(Fault::QueueFull { queue: 0 });
            return Ok(());
        };
        if let Err(pending) = queued.push_back(pending) {
            // A queue holds at most every ticket; the attempt stays in
            // custody with the quarantine.
            core::mem::forget(pending);
            self.record(Fault::QueueFull {
                queue: u8::try_from(index).unwrap_or(u8::MAX),
            });
            return Ok(());
        }
        // A free queue publishes its head at once: nothing ends here.
        let driver = &mut self.driver;
        let free = driver.running.get(index).is_some_and(Option::is_none)
            && driver.free.get(index).is_some_and(Option::is_some);
        let alone = driver.queues.get(index).is_some_and(|queued| {
            queued.len() == 1 && queued.front().is_some_and(|p| !driver.blocked(p))
        });
        let publishable = free
            && alone
            && !self.holding_blocks()
            && !self.driver.closed
            && self.driver.closing.is_none()
            && !core_views::configuring::<Id, D, B>(self.driver.core.view());
        if publishable
            && let Some(pending) = self
                .driver
                .queues
                .get_mut(index)
                .and_then(heapless::Deque::pop_front)
            && let Err(fault) = self.driver.publish(self.device, index, pending)
        {
            self.record(fault);
        }
        self.recompute_deadline();
        Ok(())
    }

    fn receive(&mut self, room: u8, now: Instant) -> Result<PortRxPull<'id, Self>, Self::Fault> {
        self.check()?;
        self.now = now;
        if !self.running() {
            return Ok(RxPull::Empty);
        }
        settle_lent(&self.shared.network, &self.rx_out, self.device)
            .map_err(|fault| self.record(fault))?;
        for _ in 0..RECEIVE_STEPS {
            let room = usize::from(room).min(RX_HELD.saturating_sub(self.rx_out.get()));
            let pulled = self
                .device
                .pull_unit(room, now)
                .map_err(|fault| self.record(fault))?;
            match pulled {
                DevicePull::Unit(unit) => {
                    let admitted = self
                        .driver
                        .admit(self.device, &self.rx_out, self.shared, unit)
                        .map_err(|fault| self.record(fault))?;
                    if let Some((frame, meta)) = admitted {
                        return Ok(RxPull::Unit {
                            frame: frame.lend(),
                            meta,
                        });
                    }
                }
                DevicePull::Again => {}
                DevicePull::Empty => {
                    self.rx_later = None;
                    self.recompute_deadline();
                    return Ok(RxPull::Empty);
                }
                DevicePull::Later(at) => {
                    self.rx_later = Some(at);
                    self.recompute_deadline();
                    return Ok(RxPull::Later(at));
                }
            }
        }
        // Ring work is left beyond this call's steps.
        Ok(RxPull::Later(now))
    }

    fn recycle(&mut self, unit: PortRxBuffer<'id, Self>) {
        let (unit, _) = unit.into_unit();
        self.device.recycle(unit);
        match self.rx_out.get().checked_sub(1) {
            Some(held) => self.rx_out.set(held),
            None => {
                self.record(Fault::ReceiveAccounting);
            }
        }
    }

    fn lend(
        &mut self,
        unit: PortRxBuffer<'id, Self>,
    ) -> Result<LentReceive<D::Receive, D::Lent>, LendRefused<PortRxBuffer<'id, Self>>> {
        let shared: &'static PortSharedStorage<B, D::Lent> = self.shared;
        unit.lend_to(
            &shared.network,
            D::LENT_RX,
            !self.running() || self.check().is_err(),
        )
    }

    fn lend_room(&self) -> bool {
        self.shared.network.held() < D::LENT_RX
    }

    fn lent(&self) -> usize {
        self.shared.network.held()
    }

    fn poll_lend_room(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.shared.network.poll_room(cx, D::LENT_RX)
    }

    fn poll_lent_returned(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.shared.network.poll_returned(cx)
    }

    fn add_block_ack(
        &mut self,
        slot: PhaseBlockAckSlot<'id, Self::Types>,
        agreement: SupportedBlockAck<Self::Caps>,
    ) -> ReceiveAgreement<'id, Id> {
        self.set_block_ack(slot.index(), Some(agreement));
        ReceiveAgreement { slot }
    }

    fn replace_block_ack(
        &mut self,
        running: ReceiveAgreement<'id, Id>,
        agreement: SupportedBlockAck<Self::Caps>,
    ) -> ReceiveAgreement<'id, Id> {
        self.set_block_ack(running.slot.index(), Some(agreement));
        running
    }

    fn remove_block_ack(
        &mut self,
        agreement: ReceiveAgreement<'id, Id>,
    ) -> PhaseBlockAckSlot<'id, Self::Types> {
        self.set_block_ack(agreement.slot.index(), None);
        agreement.slot
    }

    fn open_gate(&mut self, gate: GateClosed<'id, Part<Id, 1>>) -> GateOpen<'id, Part<Id, 1>> {
        let gate = self.gate.open(gate.into_owned(&self.loan));
        self.reopen = true;
        self.recompute_deadline();
        gate.lend(&self.loan)
    }

    fn command(
        &mut self,
        command: PortCommand<'id, Self::Types, Self::Caps>,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> Result<CommandProgress<'id, Self::Types, Self::Caps>, Self::Fault> {
        if let Err(fault) = self.check() {
            self.retain(command);
            return Err(fault);
        }
        self.now = now;
        if !self.running() {
            return Ok(CommandProgress::Refused(
                command,
                CommandRefusal::NotEnabled,
            ));
        }
        let progress = self.run_command(command, now, sink);
        let progress = progress.map_err(|fault| self.record(fault));
        self.recompute_deadline();
        progress
    }

    fn poll_start(
        &mut self,
        cx: &mut Context<'_>,
        channel: SupportedChannel<Self::Caps>,
        now: Instant,
    ) -> Poll<Result<(), StartRefused>> {
        self.now = now;
        let since = match self.state {
            PortState::Running | PortState::Stopping { .. } => {
                return Poll::Ready(Err(StartRefused::Running));
            }
            PortState::Stopped => {
                self.state = PortState::Starting { since: now };
                now
            }
            PortState::Starting { since } => since,
        };
        if self.check().is_err() {
            self.state = PortState::Stopped;
            return Poll::Ready(Err(StartRefused::Device));
        }
        if now.saturating_duration_since(since) >= COMMAND_BOUND {
            self.state = PortState::Stopped;
            self.recompute_deadline();
            return Poll::Ready(Err(StartRefused::Deadline));
        }
        let Some(wifi) = channel.channel().wifi_channel() else {
            self.state = PortState::Stopped;
            return Poll::Ready(Err(StartRefused::Retune));
        };
        let started = match self.device.poll_start(cx, wifi) {
            Poll::Pending => {
                self.recompute_deadline();
                return Poll::Pending;
            }
            Poll::Ready(started) => started,
        };
        let tuned = started.and_then(|()| {
            core_views::tune::<Id, D, B>(
                self.driver.core.view(),
                wifi,
                self.device.take_power_profile(),
            )
        });
        // The started core takes the station's settings before its first
        // publication.
        let tuned = tuned.and_then(|()| self.apply_settings());
        let result = match tuned {
            Ok(()) => {
                self.state = PortState::Running;
                self.channel = channel.channel();
                self.stop_served = false;
                self.core_disabled = false;
                Ok(())
            }
            Err(Fault::Retune(_) | Fault::ChannelNotPermitted) => {
                self.state = PortState::Stopped;
                Err(StartRefused::Retune)
            }
            Err(_) => {
                self.state = PortState::Stopped;
                Err(StartRefused::Device)
            }
        };
        self.recompute_deadline();
        Poll::Ready(result)
    }

    fn poll_stop(
        &mut self,
        cx: &mut Context<'_>,
        now: Instant,
    ) -> Poll<Result<(), StopUnconfirmed>> {
        self.now = now;
        let since = match self.state {
            PortState::Stopped => return Poll::Ready(Ok(())),
            PortState::Starting { .. } | PortState::Running => {
                self.state = PortState::Stopping { since: now };
                self.stop_served = false;
                self.recompute_deadline();
                now
            }
            PortState::Stopping { since } => since,
        };
        let unconfirmed = |port: &mut Self, cause: StopCause| {
            Poll::Ready(Err(StopUnconfirmed {
                cause,
                observations: port.stop_observations(),
            }))
        };
        if self.check().is_err() {
            return unconfirmed(self, StopCause::Device);
        }
        let expired = now.saturating_duration_since(since) >= COMMAND_BOUND;
        if !self.core_disabled {
            if expired {
                return unconfirmed(self, StopCause::Deadline);
            }
            // The next service withdraws the queues and disables the core.
            return Poll::Pending;
        }
        match self.device.poll_stop(cx) {
            Poll::Ready(Ok(())) => {
                self.state = PortState::Stopped;
                self.recompute_deadline();
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(_)) => unconfirmed(self, StopCause::Device),
            Poll::Pending if expired => unconfirmed(self, StopCause::Deadline),
            Poll::Pending => Poll::Pending,
        }
    }

    fn sample_clock(&mut self) -> Result<Ieee80211Instant, ClockError> {
        self.device.radio_now()
    }
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
    Esp32s31Port<'id, Id, D, B, O>
{
    /// What the port held when its stop gave up.
    fn stop_observations(&mut self) -> StopObservations {
        let count = |count: usize| u8::try_from(count).unwrap_or(u8::MAX);
        StopObservations {
            arena_on_air: self.driver.arena_on_air(),
            published: count(self.driver.running.iter().flatten().count()),
            queued: count(self.driver.queues.iter().map(heapless::Deque::len).sum()),
            configuring: self.driver.configuring.is_some(),
        }
    }

    fn set_block_ack(&mut self, index: u8, agreement: Option<SupportedBlockAck<Esp32s31Caps<B>>>) {
        if let Some(place) = self.settings.block_acks.get_mut(usize::from(index)) {
            *place = agreement.map(|agreement| {
                let (peer, tid, start_sequence, window) = agreement.get();
                core_mac::RxBlockAckValues {
                    role: VifRole::Station,
                    peer: peer.get(),
                    tid: tid.get(),
                    start_sequence,
                    window: u8::try_from(window as usize).unwrap_or(u8::MAX),
                }
            });
            self.settings.block_acks_dirty = true;
            self.recompute_deadline();
        }
    }
}

/// The station's data arenas of an [`Esp32s31Port`], lent for one turn.
/// The port serves the published arena in [`DataPort::service_tx`]:
/// [`TxArena::complete`] takes no events here.
pub struct Esp32s31Arena<
    't,
    'id,
    Id: SlotIdentity + 'id,
    D: Device<B>,
    const B: usize,
    O: Observations,
> {
    port: &'t mut Esp32s31Port<'id, Id, D, B, O>,
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
    Esp32s31Arena<'_, 'id, Id, D, B, O>
{
    /// Run `step` over one arena port; a fault quarantines the port.
    fn with_port<R>(
        &mut self,
        step: impl for<'x> FnOnce(
            &mut driver::arena::Port<'x, 'static, Id, D, B, O>,
        ) -> Result<R, ArenaFault>,
    ) -> Result<R, Fault> {
        let port = &mut *self.port;
        port.check()?;
        let hardware = port.device.hardware()?;
        let driver = &mut port.driver;
        let arena = driver.arena.as_mut().ok_or(Fault::InstallationResources)?;
        let core = driver
            .core
            .view()
            .arena_core()
            .ok_or(Fault::CoreViewUnavailable(CoreOp::Arena))?;
        let observations = driver
            .observations
            .as_mut()
            .ok_or(Fault::InstallationResources)?;
        let pairwise = driver::arena::station_pairwise(&driver.keys);
        let mut sink = Sink::default();
        let result = {
            let mut arena_port: driver::arena::Port<'_, 'static, Id, D, B, O> = ArenaPort::new(
                arena,
                core,
                hardware,
                &mut driver.free,
                VifRole::Station,
                pairwise,
                &mut sink,
                observations,
            );
            step(&mut arena_port)
        };
        let contract = sink.fault().or_else(|| {
            (!sink.removed.is_empty() || !sink.completed.is_empty())
                .then_some(Fault::ArenaCoreContract)
        });
        if let Some(fault) = contract {
            // The callback's owners stay with the sink's quarantine.
            core::mem::forget(sink);
            return Err(port.record(fault));
        }
        let result = result.map_err(|fault| port.record(Fault::Arena(fault)));
        port.recompute_deadline();
        result
    }

    /// Whether the station may not publish an arena now: the port does not
    /// run, the gate is closed or closing, or a configuration is pending.
    fn publication_blocked(&mut self) -> bool {
        let port = &mut *self.port;
        !port.running()
            || port.driver.closed
            || port.driver.closing.is_some()
            || port.driver.configuring.is_some()
            || port.holding_blocks()
            || core_views::configuring::<Id, D, B>(port.driver.core.view())
    }
}

impl<'t, 'id: 't, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
    TxArena<'t, 'id> for Esp32s31Arena<'t, 'id, Id, D, B, O>
{
    type Caps = Esp32s31Caps<B>;
    type Memory = TxBacking<B>;
    type Events = Infallible;
    type Response = BlockAckAnswer;
    type Denial = Infallible;
    type Fault = Fault;

    fn claim_arenas(&mut self) -> bool {
        self.port
            .driver
            .arena
            .as_mut()
            .is_some_and(driver::DataArenas::<Id, B>::claim)
    }

    fn capacity(&self) -> u8 {
        driver::DataArenas::<Id, B>::CAPACITY
    }

    fn push(
        &mut self,
        arena: &Arena,
        memory: TxBacking<B>,
        write: impl FnOnce(&mut [u8]) -> Option<u16>,
    ) -> Result<(), (TxBacking<B>, IntakeRefused)> {
        if self.port.check().is_err() {
            return Err((memory, IntakeRefused::Quarantined));
        }
        let Some(arenas) = self.port.driver.arena.as_mut() else {
            return Err((memory, IntakeRefused::Busy));
        };
        let pushed = arenas.push(arena.id(), memory, write);
        if let (Err((_, IntakeRefused::Quarantined)), Some(fault)) = (&pushed, arenas.fault()) {
            self.port.record(Fault::Arena(fault));
        }
        pushed
    }

    fn len(&self, arena: ArenaId) -> u8 {
        self.port
            .driver
            .arena
            .as_ref()
            .map_or(0, |arenas| arenas.len(arena))
    }

    fn on_air(&self) -> Option<ArenaId> {
        self.port.driver.arena.as_ref()?.on_air()
    }

    fn publish(
        &mut self,
        publication: ArenaPublication<'t, 'id, Esp32s31Caps<B>>,
        now: Instant,
    ) -> Result<(), PublishError<'t, 'id, Esp32s31Caps<B>, Fault>> {
        if self.publication_blocked() {
            return Err(PublishError::Refused {
                publication,
                reason: PublishRefused::Busy,
            });
        }
        self.port.now = now;
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
            Ok(Ok(())) => Ok(()),
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
        events: Infallible,
        _: Instant,
    ) -> Result<Option<Completion<BlockAckAnswer, Infallible>>, Fault> {
        match events {}
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
        self.port.driver.arena.as_ref()?.deadline()
    }
}

impl<'t, 'id: 't, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
    PortArena<'t, 'id> for Esp32s31Arena<'t, 'id, Id, D, B, O>
{
    fn channel(&self) -> Channel {
        self.port.channel
    }

    fn ampdu_capabilities(&self) -> AmpduCapabilities {
        esp32s31_ampdu_capabilities(TX_AMPDU_SUBFRAMES)
    }
}
