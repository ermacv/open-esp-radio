//! The S31 data port: the radio as the station's datapath actor owns it.
//!
//! [`Esp32s31Port`] owns the driver's state, the device and the settings
//! by value; one task calls it in its turns and nothing else borrows it.
//! Every call takes the turn's `now`, never reads the clock and never
//! wakes its caller: what must run again is in the returned value or in
//! the cached [`DataPort::deadline`], which each call that changes a queue
//! recomputes. The driver's steps post through [`DriverEnv`] to the
//! turn's [`PortSink`].
//!
//! A broken contract of the port's own state is a named [`Fault`] that
//! keeps its observations; the port keeps every owner it holds and
//! answers each later call with the same fault.

use super::driver::{self, Driver, DriverEnv, Pending, Sink, core_views, queue, settle_lent};
use super::keys::{
    AccessPointGroupCommand, AccessPointGroupInstallation, GroupInstallation, Installation,
    KeyIdentity, PortKeys, RawKeyIdentity, Role, StationGroupCommand, StationGroupInstallation,
};
use super::*;
use crate::lower_mac::arena::{ArenaFault, ArenaPort};
use oer_protocols_ieee80211_contracts_port::arena::{
    Arena, ArenaId, ArenaPublication, Completion, IntakeRefused, PublishError, PublishRefused,
    Settlement, TxArena,
};
use oer_protocols_ieee80211_contracts_port::{
    CommandFailed, CommandProgress, CommandRefusal, CommandTypes, DataPort, Facts, Input,
    PairwiseIssue, PortAnswer, PortArena, PortCommand, PortCommandFailed, PortCommands,
    PortCompleted, PortRxBuffer, PortRxPull, PortSink, ReceiveAccounting, ReceiveOperation,
    RecycleRefused, RxPull, StartFailure, StartRefused, StationSubmission, StopCause,
    StopObservations, StopUnconfirmed, SubmitAnswer, SubmitHalt, SubmitRefusal, TxFacts,
    Unconsumed, UnconsumedRefused, Unkeeping,
};
use oer_protocols_radio_port::{Installation as Receipt, Installations, Minted};

/// The port's private conversion authority for its `'static` brand.
type Loan<'id, Id, D, const B: usize, O> = BodyLoan<'id, Id, Esp32s31Backend<Id, D, B, O>>;

/// The data port's command over its types `Types<Id, D, B, O>`.
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
    /// The installations that issued its association and its slot.
    issued: [u32; 2],
}

/// An accepted group installation. Only [`Esp32s31Commands`] makes one.
pub struct GroupInstall<Id: SlotIdentity> {
    install: StationGroupInstallation<'static, Id>,
    /// The installation that issued its association.
    association: u32,
}

impl<Id: SlotIdentity> GroupInstall<Id> {
    /// The slot and permission indices of the installation.
    #[cfg(test)]
    pub(crate) fn owners_for_test(&self) -> (u8, u8) {
        (self.install.slot.index(), self.install.permission.index())
    }
}

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
        let issued = [association.generation(), slot.generation()];
        let installation =
            <Station as Role<'id, Id>>::pairwise(association, slot, peer).into_owned(&self.loan);
        (
            PortCommand::InstallPairwise(PairwiseInstall {
                identity,
                installation,
                material: key,
                issued,
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
        PortCommand::InstallGroup(GroupInstall {
            association: association.generation(),
            install: GroupInstallation {
                spp: association.spp(),
                slot,
                permission,
                key_id,
                material: key,
            }
            .into_owned(&self.loan),
        })
    }
}

/// The generations of the installation tokens a station command names:
/// its permission, association, key slot or installed key.
fn station_generations<
    'id,
    Id: SlotIdentity + 'id,
    D: Device<B>,
    const B: usize,
    O: Observations,
>(
    command: &Cmd<'id, Id, D, B, O>,
) -> [Option<u32>; 3] {
    match command {
        PortCommand::InstallPairwise(install) => {
            let [association, slot] = install.issued;
            [Some(association), Some(slot), None]
        }
        PortCommand::InstallGroup(install) => [
            Some(install.association),
            Some(install.install.slot.generation()),
            Some(install.install.permission.generation()),
        ],
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
            Some(installed.identity.generation()),
            None,
        ],
        PortCommand::RemoveGroup { permission, key } => [
            Some(permission.generation()),
            Some(key.identity.generation()),
            None,
        ],
        PortCommand::RemovePairwise { association, key } => [
            Some(association.generation()),
            Some(key.identity.generation()),
            None,
        ],
        PortCommand::Configure { permission, .. } => [Some(permission.generation()), None, None],
        PortCommand::CloseGate(_)
        | PortCommand::SetEdca(_)
        | PortCommand::SetBssColor(_)
        | PortCommand::SetHe(_) => [None, None, None],
    }
}

/// Where the radio is between its stop and its run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PortState {
    Stopped,
    Starting {
        since: Instant,
    },
    Running,
    /// A fault ended the run: nothing new goes out, and what is out comes
    /// home by the same service a stop uses. The stop's bound starts with
    /// the stop.
    Recovering,
    Stopping {
        since: Instant,
    },
    /// A start the device began failed: its receive path stops before the
    /// refusal is answered, within the command bound from `since`.
    RollingBack {
        since: Instant,
        refused: StartFailure<Fault>,
    },
}

/// The station's settings, owned by the port and applied in its turn.
struct PortSettings {
    /// The station's and the access point's EDCA tables.
    edca: [EdcaParameterSet; 2],
    edca_dirty: bool,
    color: Option<BssColor>,
    /// The station interface's HE association to install or end.
    he: Option<oer_protocols_ieee80211_contracts_port::StationHe>,
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
            he: None,
            block_acks: [None; 8],
            block_acks_dirty: false,
        }
    }

    fn dirty(&self) -> bool {
        self.edca_dirty || self.color.is_some() || self.he.is_some() || self.block_acks_dirty
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
    /// Whose group mutation holds the receive cut.
    group_mutator: Option<Mutator>,
    /// The kind of an access-point command the caller holds at its barrier.
    access_point_holding: Option<oer_protocols_ieee80211_contracts_port::AccessPointCommandKind>,
    /// An access-point configuration held at its barrier since this
    /// instant.
    access_point_since: Option<Instant>,
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
    /// The caller's waker from its last [`DataPort::poll_facts`]: a held
    /// receive cut registers the caller's own task with the receive session,
    /// so the image keeps one waker vtable.
    waker: Option<core::task::Waker>,
    /// The first contract fault: every later call answers with it.
    fault: Option<Fault>,
    /// What a sink refused, kept whole until a sink takes it.
    unconsumed: unconsumed::Store<'id, Id, D, B, O>,
    /// The current call's one place for an item it could neither give its
    /// sink nor keep, and the call's right to fill it: returned beside the
    /// call's answer, empty between calls.
    unkept: unconsumed::CallPlace<'id, Id, D, B, O>,
}

/// How far the S31 port took a station command.
type CmdProgress<'id, Id, D, const B: usize, O> =
    CommandProgress<'id, Types<Id, D, B, O>, Esp32s31Caps<B>>;

/// A failed station command of the S31 port.
type CmdFailed<'id, Id, D, const B: usize, O> = CommandFailed<Cmd<'id, Id, D, B, O>, Fault>;

/// Whose group mutation runs: the station's or the access point's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Mutator {
    Station(oer_protocols_ieee80211_contracts_port::CommandKind),
    AccessPoint(oer_protocols_ieee80211_contracts_port::AccessPointCommandKind),
}

/// How far one group mutation's step took it.
pub(super) enum GroupStep<C> {
    /// It ran; its terminal is in the sink.
    Done,
    /// It waits at its barrier or receive cut.
    Held(C),
    /// It waited past its bound and did not run; what its barrier began is
    /// undone and the caller refuses it with every owner.
    Expired(C),
    /// It failed before it took the command's owners: the caller returns
    /// the command beside the fault.
    Failed(C, Fault),
}

/// A group mutation's receive cut: polled under the call's right, or
/// waiting without one.
enum Cut<P> {
    Polled(P, unconsumed::Vacancy),
    Waiting,
}

mod access_point;
pub use access_point::{AccessPointGroupInstall, AccessPointPairwiseInstall};

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
    /// What the sink refused, which the port keeps.
    unconsumed: &'a mut unconsumed::Store<'id, Id, D, B, O>,
    /// The call's one place for an item it could not keep, and its right.
    unkept: &'a mut unconsumed::CallPlace<'id, Id, D, B, O>,
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations, S>
    PortEnv<'_, 'id, Id, D, B, O, S>
where
    S: PortSink<'id, Types<Id, D, B, O>>,
{
    /// Give `item` to the sink behind what the port keeps, under the
    /// call's right `vacancy`.
    fn deliver(&mut self, vacancy: unconsumed::Vacancy, item: unconsumed::Item<'id, Id, D, B, O>) {
        self.unconsumed
            .deliver(self.sink, self.unkept, vacancy, item);
    }
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations, S>
    DriverEnv<'static, Id, D, B, O> for PortEnv<'_, 'id, Id, D, B, O, S>
where
    S: PortSink<'id, Types<Id, D, B, O>>,
{
    type Vacancy = unconsumed::Vacancy;
    fn vacancy(&mut self) -> Option<unconsumed::Vacancy> {
        self.unkept.vacancy()
    }
    fn vacate(&mut self, vacancy: unconsumed::Vacancy) {
        self.unkept.vacate(vacancy);
    }
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
    fn tx_completed(
        &mut self,
        vacancy: unconsumed::Vacancy,
        _: u8,
        completed: brand::OwnedTxCompleted<'static, Id, D, B, O>,
    ) {
        let terminal = completed.lend(&self.loan);
        self.deliver(vacancy, unconsumed::Item::Terminal(terminal));
    }
    fn tbtt(&mut self, event: TbttEvent) {
        self.sink.tbtt(event);
    }
    fn received(
        &mut self,
        vacancy: unconsumed::Vacancy,
        frame: ReceiveBuffer<'static, Id, D::Receive>,
        meta: Esp32s31RxMeta,
    ) -> Result<(), Fault> {
        self.deliver(vacancy, unconsumed::Item::Received(frame.lend(), meta));
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
    fn halted(&self) -> bool {
        self.unkept.halted()
    }
    #[cfg(oer_debug_control)]
    fn reject_group_replacement(&mut self) -> bool {
        false
    }
}

/// The parts, the mint and the driver an installation refused, all
/// unchanged: nothing was installed.
pub type InstallRefused<'id, Id, D, const B: usize, O> = (
    PortParts<D, B, O>,
    Minted<Id, InstallationPools>,
    &'id mut PortDriver<Id, D, B, O>,
);

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
    /// port this chip runs: an address, publication bound, channel, buffer
    /// size or bank it does not take, a busy descriptor, a driver that holds
    /// work, a lost receive buffer or a transmit allocation still out.
    #[inline(never)]
    #[allow(
        clippy::result_large_err,
        reason = "the refusal returns every input by value, unchanged"
    )]
    pub fn install(
        parts: PortParts<D, B, O>,
        minted: Minted<Id, InstallationPools>,
        driver: &'id mut PortDriver<Id, D, B, O>,
    ) -> Result<PortInstallation<'id, Id, D, B, O>, InstallRefused<'id, Id, D, B, O>> {
        let Ok(station) = IndividualAddress::for_interface(parts.config.station_address) else {
            return Err((parts, minted, driver));
        };
        if parts.config.publication_timeout > PUBLICATION_BOUND
            || parts.config.publication_timeout == Duration::ZERO
            || IndividualAddress::for_interface(parts.access_point_address.get()).is_err()
            || !Esp32s31Caps::<B>::CAPS
                .supports_channel(Channel::from_ghz2_4_channel(parts.config.channel))
            || B < crate::ordinary_tx::TX_METADATA_SIZE
                + 24
                + crate::ordinary_tx::TX_CCMP_MIC_SIZE
                + crate::ordinary_tx::TX_FCS_SIZE
            || usize::from(parts.bank) > TX_BUFFERS
            || !parts.descriptors.idle()
            || !driver.0.retired()
            || parts.shared.lost()
            || !parts.shared.returned.none_out()
        {
            return Err((parts, minted, driver));
        }
        let PortParts {
            device,
            shared,
            transmitter,
            pool,
            bank,
            descriptors,
            config,
            access_point_address,
            coex,
            observations,
        } = parts;
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
            channel: Channel::from_ghz2_4_channel(config.channel),
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
            group_mutator: None,
            access_point_holding: None,
            access_point_since: None,
            holding: None,
            cut_retry: false,
            cut_due: false,
            reopen: false,
            stop_served: false,
            core_disabled: false,
            waker: None,
            fault: prepared.err(),
            unconsumed: unconsumed::Store::new(),
            unkept: unconsumed::CallPlace::new(),
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
        let Some(channel) = self.channel.ghz2_4_channel() else {
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
        );
        self.settings = PortSettings::new();
        self.state = PortState::Stopped;
        self.gate = issuer;
        self.loan = loan;
        self.receipt = Some(receipt);
        self.channel = Channel::from_ghz2_4_channel(channel);
        self.rx_out.set(0);
        self.serials = Serials::new(generation);
        self.expired = 0;
        self.now = Instant::from_micros(0);
        self.deadline = None;
        self.rx_later = None;
        self.forget_held();
        self.reopen = false;
        self.stop_served = false;
        self.core_disabled = false;
        self.fault = prepared.err();
        Ok((commands, resources))
    }

    /// The MSDUs that expired before publication in this installation.
    pub fn expired_msdus(&self) -> u32 {
        self.expired
    }

    /// The installation's first contract fault, or the loss of a received
    /// buffer, which every later call answers with.
    pub fn fault(&self) -> Option<Fault> {
        self.check().err()
    }

    /// Refuse the next publication past its buffer's attachment.
    #[cfg(test)]
    pub(crate) fn refuse_values_for_test(&mut self) {
        self.driver.refuse_values_for_test = true;
    }

    /// Refuse the next step of `refusal`'s kind, which the counts exclude.
    #[cfg(test)]
    pub(crate) fn refuse_for_test(&mut self, refusal: super::driver::TestRefusal) {
        self.driver.refuse_for_test = Some(refusal);
    }

    /// The place of `key` among the driver's installed keys.
    #[cfg(test)]
    pub(crate) fn key_place_for_test<R>(
        &self,
        key: &super::PairwiseKey<'id, Id, R>,
    ) -> Option<usize> {
        self.driver.keys.iter().position(|entry| {
            entry
                .as_ref()
                .and_then(super::keys::KeyPlace::installed)
                .is_some_and(|(identity, _)| *identity == key.identity)
        })
    }

    /// The tickets in the driver's hand and the keys' uses in the core.
    #[cfg(test)]
    pub(crate) fn admissions_for_test(&mut self) -> (usize, Option<u32>) {
        let tickets = self.driver.tickets.at_home() as usize;
        let uses = match self.driver.core.view() {
            CoreView::Open(core) => Some(core.key_uses_for_test()),
            _ => None,
        };
        (tickets, uses)
    }

    /// Forget every installed key, so that a publication names a missing one.
    #[cfg(test)]
    pub(crate) fn uninstall_keys_for_test(&mut self) {
        for key in &mut self.driver.keys {
            drop(key.take());
        }
    }

    /// Count `units` lost beyond the buffers the caller holds, which the
    /// held count excludes.
    #[cfg(test)]
    pub(crate) fn lose_for_test(&self, units: usize) {
        self.shared
            .lost
            .fetch_add(units, core::sync::atomic::Ordering::AcqRel);
    }

    /// Admit a use of the first installed transmit key outside any
    /// publication, held by the test.
    #[cfg(test)]
    pub(crate) fn hold_key_use_for_test(
        &mut self,
    ) -> Option<crate::lower_mac::TxAdmission<'static, CorePath<Id>, TX_TICKETS>> {
        self.driver.hold_key_use_for_test()
    }

    /// Return the use [`Self::hold_key_use_for_test`] admitted, as an
    /// unpublished admission returns.
    #[cfg(test)]
    pub(crate) fn return_key_use_for_test(
        &mut self,
        admission: crate::lower_mac::TxAdmission<'static, CorePath<Id>, TX_TICKETS>,
    ) {
        let hardware = self.device.hardware().expect("registers");
        self.driver.return_key_use_for_test(hardware, admission);
        self.recompute_deadline();
    }

    /// Whether a removal its command left waits in the port.
    #[cfg(test)]
    pub(crate) fn removal_waits_for_test(&self) -> bool {
        self.driver.removal_waits()
    }

    /// Set the received units the caller holds: a count a unit did not
    /// join.
    #[cfg(test)]
    pub(crate) fn set_held_for_test(&self, held: usize) {
        self.rx_out.set(held);
    }

    /// The received units the caller holds.
    #[cfg(test)]
    pub(crate) fn held_for_test(&self) -> usize {
        self.rx_out.get()
    }

    /// Report `fault` as the port would.
    #[cfg(test)]
    pub(crate) fn fault_for_test(&mut self, fault: Fault) {
        self.record(fault);
    }

    /// The slot and permission of a rejected group replacement the driver
    /// keeps with its quarantine, and its previous and attempted KeyIDs
    /// with whether the rollback restored the previous entry.
    #[cfg(test)]
    pub(crate) fn retained_group(&self) -> (Option<(u8, u8)>, Option<(u8, u8, bool)>) {
        (
            self.shared.retained_group.get(),
            self.shared.rolled_back.get(),
        )
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
            && !self.driver.removal_waits()
            && self.driver.retained_group.is_none()
            && self.unconsumed.is_empty()
            && self.shared.returned.none_out()
            && self.driver.queues.iter().all(|queue| queue.is_empty())
            && self.driver.running.iter().all(Option::is_none)
            && self.driver.arenas_returnable()
    }

    /// End every removal its command left waiting whose key the core
    /// cleared since: each terminal, taken from the core place its key's
    /// table place names, returns the rights waiting there. The key is not
    /// cleared again.
    fn deliver_removals(
        &mut self,
        sink: &mut impl PortSink<'id, Types<Id, D, B, O>>,
    ) -> Result<(), Fault> {
        for place in 0..self.driver.keys.len() {
            // Each removal ends under the call's right, obtained before its
            // terminal leaves: without it the rest wait in their places for
            // the next call.
            let Some(vacancy) = self.unkept.vacancy() else {
                break;
            };
            match self.driver.finished_removal(place) {
                Some(terminal) => self.unconsumed.deliver(
                    sink,
                    &mut self.unkept,
                    vacancy,
                    unconsumed::Item::Terminal(
                        brand::OwnedTerminal::KeyRemoved(terminal).lend(&self.loan),
                    ),
                ),
                None => self.unkept.vacate(vacancy),
            }
        }
        Ok(())
    }

    /// The stopped port's cleanup of a station group replacement whose
    /// rollback failed: its removal's terminal returns the slot and the
    /// permission once the key table reads the entry cleared.
    fn clean_retained_group(
        &mut self,
        sink: &mut impl PortSink<'id, Types<Id, D, B, O>>,
    ) -> Result<(), CleanupRefused<Fault>> {
        // The removal's terminal leaves under the call's right: without it
        // the replacement stays retained and the cleanup is not finished.
        let Some(vacancy) = self.unkept.vacancy() else {
            return Ok(());
        };
        match self.driver.clean_retained_group(self.device) {
            Ok(Some(terminal)) => self.unconsumed.deliver(
                sink,
                &mut self.unkept,
                vacancy,
                unconsumed::Item::Terminal(
                    brand::OwnedTerminal::KeyRemoved(terminal).lend(&self.loan),
                ),
            ),
            Ok(None) => self.unkept.vacate(vacancy),
            Err(refused) => {
                self.unkept.vacate(vacancy);
                return Err(refused);
            }
        }
        Ok(())
    }

    /// Forget the command the caller held at its barrier: its admission
    /// instant, its hold on single MPDUs, its receive cut and its removal
    /// barrier. A stop refuses the held command back to the caller
    /// unchanged; it runs again, from its own new admission, once the port
    /// runs again.
    fn forget_held(&mut self) {
        self.group_since = None;
        self.group_mutator = None;
        self.holding = None;
        self.access_point_holding = None;
        self.access_point_since = None;
        self.driver.access_point_configuring = false;
        self.cut_retry = false;
        self.cut_due = false;
        self.driver.barrier = None;
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

    /// Whether the port confirmed its radio's stop: its last stop ended
    /// confirmed and nothing started it again.
    pub fn stop_confirmed(&self) -> bool {
        self.state == PortState::Stopped
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
            None if self.shared.lost() => Err(Fault::ReceiveLost),
            None => Ok(()),
        }
    }

    /// Offer what a sink refused again, oldest first, while the radio has
    /// not confirmed a stop; after a confirmed stop only
    /// [`DataPort::take_unconsumed`] does.
    /// Refuse the next `count` keeps of the store, as a taken place does.
    #[cfg(test)]
    pub(crate) fn refuse_keeps_for_test(&mut self, count: usize) {
        self.unconsumed.refuse_keeps = count;
    }

    /// Lose the call place's right, as a producer that dropped it would.
    #[cfg(test)]
    pub(crate) fn lose_right_for_test(&mut self) {
        self.unkept.lose_right_for_test();
    }

    /// Mark the radio's stop confirmed, as a stop whose core disable left
    /// completions with the driver ends.
    #[cfg(test)]
    pub(crate) fn confirm_stop_for_test(&mut self) {
        self.state = PortState::Stopped;
        self.recompute_deadline();
    }

    /// How many attempts wait in the queues, and how many completions a
    /// halted call left with the driver.
    #[cfg(test)]
    pub(crate) fn waiting_for_test(&self) -> (usize, usize) {
        (
            self.driver.queues.iter().map(heapless::Deque::len).sum(),
            self.driver.uncollected_for_test(),
        )
    }

    /// Whether this call stopped on an item it could not keep: it takes no
    /// further owner ([`unconsumed::CallPlace::halted`]).
    fn halted(&self) -> bool {
        self.unkept.halted()
    }

    fn reoffer(&mut self, sink: &mut impl PortSink<'id, Types<Id, D, B, O>>) {
        if self.state != PortState::Stopped {
            let _kept: Unconsumed = self.unconsumed.offer(sink);
        }
    }

    /// Record the first contract fault; later ones keep the first.
    fn record(&mut self, fault: Fault) -> Fault {
        *self.fault.get_or_insert(fault)
    }

    fn running(&self) -> bool {
        self.state == PortState::Running
    }

    fn stopping(&self) -> bool {
        matches!(
            self.state,
            PortState::Stopping { .. } | PortState::Recovering
        )
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
                unconsumed: &mut self.unconsumed,
                unkept: &mut self.unkept,
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
            PortState::Starting { since }
            | PortState::Stopping { since }
            | PortState::RollingBack { since, .. } => {
                add(Some(since.saturating_add(COMMAND_BOUND)));
            }
            PortState::Stopped | PortState::Running | PortState::Recovering => {}
        }
        // Settings wait for a port that runs: the start applies them, so `now`
        // here would hide what the start itself waits for, and a stopping
        // port applies none.
        let settings_due = self.settings.dirty() && self.running();
        // A removal the core finished after its command waits for a
        // service to end it.
        if self.driver.removal_ready() {
            add(Some(self.now));
        }
        // Work a turn owes at once: a stop's withdrawals, an opened gate,
        // settings to apply.
        if (self.stopping() && !self.stop_served) || self.reopen || self.cut_due || settings_due {
            add(Some(self.now));
        }
        if let Some(since) = self.group_since {
            add(Some(since.saturating_add(COMMAND_BOUND)));
        }
        if let Some(since) = self.access_point_since {
            add(Some(since.saturating_add(COMMAND_BOUND)));
        }
        if self.cut_retry {
            add(Some(self.now.saturating_add(CUT_RETRY)));
        }
        if !core_views::configuring::<Id, D, B>(self.driver.core.view()) {
            for pending in self.driver.queues.iter().flat_map(|queue| queue.iter()) {
                let payload = pending.operation.payload();
                if let (Some(admitted), Some(table)) = (
                    payload.buffer.admission(),
                    self.settings.edca.get(driver::vif(payload.role)),
                ) {
                    add(Some(admitted.saturating_add(
                        table.access_category(pending.category).msdu_lifetime,
                    )));
                }
            }
        }
        // A beacon's deadline holds behind a busy queue and a barrier too.
        for pending in self.driver.queues.iter().flat_map(|queue| queue.iter()) {
            add(pending.deadline());
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
        if self.settings.dirty()
            && let Some(arena) = driver.arena.as_mut()
        {
            arena.forget_program();
        }
        // A configuration lends no commands: what is dirty stays so for the
        // service after it.
        let Some(mut view) = driver.core.view().commands() else {
            return Ok(());
        };
        if self.settings.edca_dirty {
            let [station, _] = self.settings.edca;
            let table =
                SupportedEdca::<Esp32s31Caps<B>>::new(station).ok_or(Fault::EdcaUnsupported)?;
            let _installed = view
                .set_checked_edca(VifRole::Station, table)
                .map_err(|_| Fault::EdcaUnsupported)?;
            self.settings.edca_dirty = false;
        }
        if let Some(color) = self.settings.color {
            view.set_checked_he_bss_color(color);
            self.settings.color = None;
        }
        if let Some(he) = self.settings.he {
            view.set_checked_he(self.device.hardware()?, he)
                .map_err(|crate::lower_mac::HeAssociationRange| Fault::HeAssociationRange)?;
            self.settings.he = None;
        }
        if self.settings.block_acks_dirty {
            // The registers before any agreement changes: a device that holds
            // none leaves every applied record as it was.
            let hardware = self.device.hardware()?;
            for (index, applied) in driver.applied_ba.iter_mut().enumerate() {
                let wanted = self.settings.block_acks.get(index).copied().flatten();
                if *applied == wanted {
                    continue;
                }
                if let Some(old) = *applied {
                    view.end_checked_rx_block_ack(&mut *hardware, old.role, old.peer, old.tid)?;
                    // Ended: the record follows the bank.
                    *applied = None;
                }
                if let Some(agreement) = wanted {
                    view.start_checked_rx_block_ack(&mut *hardware, agreement)?;
                }
                *applied = wanted;
            }
            self.settings.block_acks_dirty = false;
        }
        Ok(())
    }

    /// Whether a station command the caller holds at its barrier keeps the
    /// station's single MPDUs from publishing; a removal withdraws only its
    /// key's attempts. The access point's MPDUs are not ordered behind the
    /// station's commands.
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
        let stopping = self.stopping();
        let holding = self.holding_blocks();
        let (driver, device, mut env) = self.parts(sink);
        for index in 0..driver.queues.len() {
            // A halted call ends and publishes nothing more.
            if env.halted() {
                break;
            }
            // A published MPDU or arena holds the queue.
            let busy = driver.running.get(index).is_none_or(Option::is_some)
                || !driver.free.is_home_at(index);
            if busy {
                // A beacon past its deadline ends behind a busy queue too:
                // the next TBTT's beacon replaces it.
                driver.expire_beacons(&mut env, index, now);
                continue;
            }
            // Expiry continues behind a gate or a barrier. Each attempt
            // leaves under the call's right, obtained before it leaves.
            while driver
                .queues
                .get(index)
                .and_then(heapless::Deque::front)
                .is_some_and(|pending| Driver::<Id, D, B, O>::expired(&env, pending, now))
            {
                let Some(vacancy) = env.vacancy() else {
                    break;
                };
                let Some(pending) = driver
                    .queues
                    .get_mut(index)
                    .and_then(heapless::Deque::pop_front)
                else {
                    env.vacate(vacancy);
                    break;
                };
                driver.finish(&mut env, vacancy, pending, TxOutcome::Expired { msdus: 1 });
            }
            let held = stopping
                || (holding
                    && driver
                        .queues
                        .get(index)
                        .and_then(heapless::Deque::front)
                        .is_some_and(|pending| {
                            pending.operation.payload().role == VifRole::Station
                        }))
                || driver.closed
                || driver.closing.is_some()
                || driver
                    .queues
                    .get(index)
                    .and_then(heapless::Deque::front)
                    .is_none_or(|pending| {
                        core_views::configuring_role::<Id, D, B>(driver.core.view())
                            .is_some_and(|role| role == pending.operation.payload().role)
                    })
                || driver
                    .queues
                    .get(index)
                    .and_then(heapless::Deque::front)
                    .is_some_and(|pending| driver.blocked(pending));
            if held {
                continue;
            }
            // A publication takes the call's right before its attempt
            // leaves the queue: a refused one may end there.
            let Some(vacancy) = env.vacancy() else {
                break;
            };
            match driver
                .queues
                .get_mut(index)
                .and_then(heapless::Deque::pop_front)
            {
                Some(pending) => driver.publish(device, &mut env, vacancy, index, pending)?,
                None => env.vacate(vacancy),
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
            let mut steps = driver.completion_sink();
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
        // The terminal leaves under the call's right, obtained before the
        // permission leaves the driver.
        if driver.withdraw(&mut env)
            && driver.configuring.is_some()
            && let Some(vacancy) = env.vacancy()
        {
            match driver.configuring.take() {
                Some(permission) => env.deliver(
                    vacancy,
                    unconsumed::Item::Terminal(
                        brand::OwnedTerminal::Configured(permission).lend(&loan),
                    ),
                ),
                None => env.vacate(vacancy),
            }
        }
        Ok(())
    }

    /// Close the gate once the published arena and the core's MPDUs ended.
    fn close_quiet_gate<S>(&mut self, sink: &mut S) -> Result<(), Fault>
    where
        S: PortSink<'id, Types<Id, D, B, O>>,
    {
        if self.driver.closing.is_none()
            || self.driver.arena_on_air()
            || core_views::configuring::<Id, D, B>(self.driver.core.view())
        {
            return Ok(());
        }
        // The gate's terminal leaves under the call's right, obtained
        // before the core closes it.
        let Some(vacancy) = self.unkept.vacancy() else {
            return Ok(());
        };
        let hardware = match self.device.hardware() {
            Ok(hardware) => hardware,
            Err(fault) => {
                self.unkept.vacate(vacancy);
                return Err(fault);
            }
        };
        if core_views::close::<Id, D, B>(self.driver.core.view(), hardware)
            && let Some(gate) = self.driver.closing.take()
        {
            let gate = self.gate.close(gate);
            self.driver.closed = true;
            self.unconsumed.deliver(
                sink,
                &mut self.unkept,
                vacancy,
                unconsumed::Item::Terminal(LowerMacTerminal::GateClosed(gate.lend(&self.loan))),
            );
        } else {
            self.unkept.vacate(vacancy);
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
        let mut steps = driver.completion_sink();
        let opened =
            core_views::open::<Id, D, B>(driver.core.view(), device.hardware()?, &mut steps);
        if opened.is_ok() {
            driver.closed = false;
        }
        // What the open's steps completed goes home before its fault.
        let collected = driver.collect(&mut env, &mut steps, stopping);
        opened?;
        collected?;
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
        let mut steps = driver.completion_sink();
        core_views::disable::<Id, D, B>(driver.core.view(), device.hardware()?, &mut steps);
        driver.collect(&mut env, &mut steps, true)?;
        let mut steps = driver.completion_sink();
        let disabled = core_views::finish_disable::<Id, D, B>(driver.core.view(), &mut steps);
        driver.collect(&mut env, &mut steps, true)?;
        if disabled {
            // The stopped MAC receives nothing for the access point and
            // runs no TSF of it.
            driver
                .core
                .view()
                .commands()
                .ok_or(Fault::CoreViewUnavailable(CoreOp::Configure))?
                .release_access_point(device.hardware()?);
        }
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
        // Settings wait for a port that runs: a stopping one applies none,
        // and the next start applies what is still dirty.
        if self.running() {
            self.apply_settings()?;
        }
        let stopping = self.stopping();
        let wake = match tx.bits() {
            0 => WifiTxWake::Deadline,
            events => WifiTxWake::Interrupt { events },
        };
        let done = {
            let (driver, device, mut env) = self.parts(sink);
            let done =
                driver.service_tx(&mut env, device, wake, tx.is_arena_ended(), stopping, now)?;
            if tbtt {
                let mut steps = driver.completion_sink();
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
        // A halted call takes nothing more: the next call goes on from
        // here, with the arena's end returned now.
        if self.halted() {
            return Ok(done);
        }
        self.advance_configuration(sink)?;
        if self.halted() {
            return Ok(done);
        }
        if self.driver.access_point_configuring {
            // The access point's held configuration ends its attempts that
            // waited behind its frame on the air.
            let (driver, _, mut env) = self.parts(sink);
            driver.withdraw(&mut env);
        }
        self.close_quiet_gate(sink)?;
        if self.halted() {
            return Ok(done);
        }
        self.advance_queues(sink, now)?;
        if stopping && !self.halted() {
            self.disable_core(sink)?;
            self.stop_served = true;
        }
        Ok(done)
    }

    /// Run a station group mutation behind its barrier and receive cut, or
    /// hold it.
    #[allow(
        clippy::result_large_err,
        reason = "the failure returns the command by value, as a hold does"
    )]
    fn group<S>(
        &mut self,
        command: Cmd<'id, Id, D, B, O>,
        now: Instant,
        sink: &mut S,
    ) -> Result<CmdProgress<'id, Id, D, B, O>, CmdFailed<'id, Id, D, B, O>>
    where
        S: PortSink<'id, Types<Id, D, B, O>>,
    {
        let kind = command.kind();
        // The installed key the mutation uses, which no published attempt
        // may name; a removal also withdraws its key's queued attempts.
        let (uses, removal) = match &command {
            PortCommand::ReplaceGroup { installed, .. }
            | PortCommand::SelectGroup { installed, .. } => (Some(installed.identity), None),
            PortCommand::RemoveGroup { key, .. } => (Some(key.identity), Some(key.identity)),
            _ => (None, None),
        };
        let step = self
            .group_mutation(
                command,
                Mutator::Station(kind),
                uses,
                removal,
                now,
                sink,
                |command, loan| match command {
                    PortCommand::InstallGroup(GroupInstall { install, .. }) => {
                        Ok(StationGroupCommand::Install(install))
                    }
                    PortCommand::ReplaceGroup {
                        permission,
                        installed,
                        key_id,
                        key,
                    } => Ok(StationGroupCommand::Replace {
                        permission: permission.into_owned(loan),
                        key: installed.into_owned(),
                        key_id,
                        material: key,
                    }),
                    PortCommand::SelectGroup {
                        permission,
                        installed,
                    } => Ok(StationGroupCommand::Select {
                        permission: permission.into_owned(loan),
                        key: installed.into_owned(),
                    }),
                    PortCommand::RemoveGroup { permission, key } => {
                        Ok(StationGroupCommand::Remove {
                            permission: permission.into_owned(loan),
                            key: key.into_owned(),
                        })
                    }
                    other => Err(other),
                },
                // The inverse of the conversion above: a command the driver
                // refused goes back to the caller as it came.
                |command, loan, association| match command {
                    StationGroupCommand::Install(install) => {
                        PortCommand::InstallGroup(GroupInstall {
                            install,
                            association,
                        })
                    }
                    StationGroupCommand::Replace {
                        permission,
                        key,
                        key_id,
                        material,
                    } => PortCommand::ReplaceGroup {
                        permission: permission.lend(loan),
                        installed: key.lend(),
                        key_id,
                        key: material,
                    },
                    StationGroupCommand::Select { permission, key } => PortCommand::SelectGroup {
                        permission: permission.lend(loan),
                        installed: key.lend(),
                    },
                    StationGroupCommand::Remove { permission, key } => PortCommand::RemoveGroup {
                        permission: permission.lend(loan),
                        key: key.lend(),
                    },
                },
            )
            .map_err(CommandFailed::Taken)?;
        match step {
            GroupStep::Done => Ok(CommandProgress::Done),
            GroupStep::Held(command) => Ok(CommandProgress::Held(command)),
            GroupStep::Expired(command) => {
                Ok(CommandProgress::Refused(command, CommandRefusal::Expired))
            }
            GroupStep::Failed(command, fault) => Err(CommandFailed::Returned { command, fault }),
        }
    }

    /// Run a group mutation of either role behind its barrier and receive
    /// cut, or hold it. One mutation runs at a time: they share the receive
    /// cut. `uses` is the installed key no published attempt may name;
    /// `removal` the key whose queued attempts end withdrawn.
    #[allow(clippy::too_many_arguments, reason = "one step of one mutation")]
    pub(super) fn group_mutation<S, C, G>(
        &mut self,
        command: C,
        mutator: Mutator,
        uses: Option<RawKeyIdentity>,
        removal: Option<RawKeyIdentity>,
        now: Instant,
        sink: &mut S,
        into_owned: impl FnOnce(C, &Loan<'id, Id, D, B, O>) -> Result<G, C>,
        restore: impl FnOnce(G, &Loan<'id, Id, D, B, O>, u32) -> C,
    ) -> Result<GroupStep<C>, Fault>
    where
        S: PortSink<'id, Types<Id, D, B, O>>,
        G: driver::GroupMutation<'static, Id, D, B, O>,
    {
        // The other role's mutation holds the receive cut.
        if self.group_mutator.is_some_and(|running| running != mutator) {
            return Ok(GroupStep::Held(command));
        }
        self.group_mutator = Some(mutator);
        let since = *self.group_since.get_or_insert(now);
        if now.saturating_duration_since(since) >= COMMAND_BOUND {
            return self.expire_mutation(command, mutator, removal);
        }
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
        if in_use || !drained || self.halted() {
            // A publication has its own bound and the air decides how long
            // it takes: the command's counts from the last one that held it.
            self.group_since = Some(now);
            self.hold_mutation(mutator);
            return Ok(GroupStep::Held(command));
        }
        let room = RX_HELD.saturating_sub(self.rx_out.get());
        // Before the caller's first poll of the facts there is no task to
        // register: the cut runs again at the device's drain poll. The cut
        // runs under the call's right, obtained before it hands out a unit
        // and before the command's owners are taken: without it the cut
        // takes no unit and runs again at the next call.
        let cut = match self.unkept.vacancy() {
            Some(vacancy) => match self.waker.as_ref() {
                Some(waker) => Cut::Polled(
                    self.device
                        .poll_rx_cut(room, &mut Context::from_waker(waker)),
                    vacancy,
                ),
                None => {
                    self.unkept.vacate(vacancy);
                    Cut::Waiting
                }
            },
            None => Cut::Waiting,
        };
        let (cut, vacancy) = match cut {
            Cut::Waiting => return Ok(self.cut_waits(room, mutator, now, command)),
            Cut::Polled(Poll::Pending, vacancy) => {
                self.unkept.vacate(vacancy);
                return Ok(self.cut_waits(room, mutator, now, command));
            }
            Cut::Polled(Poll::Ready(cut), vacancy) => (cut, vacancy),
        };
        match cut {
            Ok(Some(unit)) => {
                // A unit captured before the mutation precedes its terminal.
                let (driver, device, mut env) = self.parts(sink);
                if let Err(fault) = driver.receive(&mut env, vacancy, device, unit) {
                    // The mutation has not reached the key table: it comes
                    // back beside the fault.
                    return Ok(GroupStep::Failed(command, fault));
                }
                self.cut_retry = false;
                self.cut_due = true;
                self.hold_mutation(mutator);
                Ok(GroupStep::Held(command))
            }
            Err(cause) => {
                self.unkept.vacate(vacancy);
                Ok(GroupStep::Failed(command, cause))
            }
            Ok(None) => {
                // What the driver's step needs besides the command's own
                // owners, checked before it takes them: the registers, the
                // core's command view, the key it names, or a free table
                // place for an installation. Refused, the command comes
                // back whole and the frozen receive path resumes.
                if let Err(fault) = self.group_ready(mutator, uses) {
                    self.unkept.vacate(vacancy);
                    self.unfreeze_mutation(mutator, removal);
                    return Ok(GroupStep::Failed(command, fault));
                }
                let loan = self.loan;
                let owned = match into_owned(command, &loan) {
                    Ok(owned) => owned,
                    Err(other) => {
                        self.unkept.vacate(vacancy);
                        return Ok(GroupStep::Held(other));
                    }
                };
                let terminal = {
                    let (driver, device, mut env) = self.parts(sink);
                    owned.run(driver, &mut env, device)
                };
                let terminal = match terminal {
                    Ok(terminal) => terminal,
                    Err(driver::GroupFailed::Returned(command, fault)) => {
                        // The driver's step took none of the command's
                        // owners: the command comes back whole, an
                        // installation with its installation's generation
                        // (the command's: its stale check passed).
                        self.unkept.vacate(vacancy);
                        self.unfreeze_mutation(mutator, removal);
                        let generation = self.serials.generation;
                        return Ok(GroupStep::Failed(
                            restore(command, &loan, generation),
                            fault,
                        ));
                    }
                    Err(driver::GroupFailed::Retained(fault)) => {
                        // The mutation failed with its fault; the receive
                        // path it froze resumes, so a failed command leaves
                        // no frozen receive behind. The first fault stays
                        // the port's.
                        self.unkept.vacate(vacancy);
                        let fault = self.record(fault);
                        if let Err(cause) = self.device.finish_rx_cut() {
                            self.record(cause);
                        }
                        self.group_since = None;
                        self.group_mutator = None;
                        self.cut_retry = false;
                        self.cut_due = false;
                        return Err(fault);
                    }
                };
                if let Err(cause) = self.device.finish_rx_cut() {
                    // The mutation ran: its terminal says so, and the fault
                    // of the receive path's resume follows it.
                    match terminal {
                        Some(terminal) => self.unconsumed.deliver(
                            sink,
                            &mut self.unkept,
                            vacancy,
                            unconsumed::Item::Terminal(terminal.lend(&loan)),
                        ),
                        None => self.unkept.vacate(vacancy),
                    }
                    return Err(self.record(cause));
                }
                self.group_since = None;
                self.group_mutator = None;
                self.cut_retry = false;
                self.cut_due = false;
                match mutator {
                    Mutator::Station(_) => self.holding = None,
                    Mutator::AccessPoint(_) => self.access_point_holding = None,
                }
                self.driver.barrier = None;
                // A removal whose key's last use is out ends at a later
                // service ([`Self::deliver_removals`]).
                match terminal {
                    Some(terminal) => self.unconsumed.deliver(
                        sink,
                        &mut self.unkept,
                        vacancy,
                        unconsumed::Item::Terminal(terminal.lend(&loan)),
                    ),
                    None => self.unkept.vacate(vacancy),
                }
                Ok(GroupStep::Done)
            }
        }
    }

    /// A group mutation whose receive cut waits: the caller holds all the
    /// units the cut could hand it, or the call has no right to take one.
    /// As with a publication, the units decide how long this takes, and
    /// the command's bound counts from the last they held it.
    fn cut_waits<C>(
        &mut self,
        room: usize,
        mutator: Mutator,
        now: Instant,
        command: C,
    ) -> GroupStep<C> {
        if room == 0 || self.device.cut_held_by_units() {
            self.group_since = Some(now);
        }
        self.cut_retry = true;
        self.cut_due = false;
        self.hold_mutation(mutator);
        GroupStep::Held(command)
    }

    /// Whether the driver's step of a group mutation has what it needs
    /// besides the command: the registers, the core's command view, the
    /// installed key `uses` names, or a free table place when it names
    /// none (an installation).
    fn group_ready(&mut self, mutator: Mutator, uses: Option<RawKeyIdentity>) -> Result<(), Fault> {
        self.device.hardware()?;
        if self.driver.core.view().commands().is_none() {
            return Err(Fault::CoreViewUnavailable(CoreOp::Command));
        }
        let op = match mutator {
            Mutator::Station(oer_protocols_ieee80211_contracts_port::CommandKind::SelectGroup) => {
                KeyOp::Select
            }
            Mutator::Station(oer_protocols_ieee80211_contracts_port::CommandKind::ReplaceGroup) => {
                KeyOp::Replace
            }
            _ => KeyOp::Remove,
        };
        // A station mutation names the station's group key, an access
        // point's the access point's.
        let group = match mutator {
            Mutator::Station(_) => core_mac::association::KeySlotKind::StationGroup,
            Mutator::AccessPoint(_) => core_mac::association::KeySlotKind::AccessPointGroup,
        };
        match uses.map(|identity| self.driver.installed_kind(identity)) {
            Some(None) => Err(Fault::KeySlotMissing(op)),
            Some(Some(kind)) if kind != group => match op {
                KeyOp::Remove => Err(Fault::RemovalMismatch),
                _ => Err(Fault::KeySlotMissing(op)),
            },
            Some(Some(_)) => Ok(()),
            None if self.driver.keys.iter().any(Option::is_none) => Ok(()),
            None => Err(Fault::KeySlotMissing(KeyOp::Install)),
        }
    }

    /// Undo a group mutation's hold that did not reach the key table: its
    /// frozen receive path resumes (a resume that fails is the port's
    /// fault), its removal's key is active again and its barrier lifts.
    fn unfreeze_mutation(&mut self, mutator: Mutator, removal: Option<RawKeyIdentity>) {
        if let Err(cause) = self.device.finish_rx_cut() {
            self.record(cause);
        }
        if let Some(identity) = removal {
            identity.activity.set(identity.serial);
            if self.driver.barrier == Some(identity) {
                self.driver.barrier = None;
            }
        }
        self.group_since = None;
        self.group_mutator = None;
        self.cut_retry = false;
        self.cut_due = false;
        match mutator {
            Mutator::Station(_) => self.holding = None,
            Mutator::AccessPoint(_) => self.access_point_holding = None,
        }
    }

    /// End a group mutation past its bound. It never reached the key table
    /// (`Driver::group` runs only once its cut drained), so its outcome is
    /// known: not run. What its hold began is undone: a cut it polled ends
    /// once a freeze already begun completes, then the removal's key is
    /// active again and its barrier lifts. Attempts the barrier withdrew
    /// keep their terminals.
    fn expire_mutation<C>(
        &mut self,
        command: C,
        mutator: Mutator,
        removal: Option<RawKeyIdentity>,
    ) -> Result<GroupStep<C>, Fault> {
        // A polled cut: `poll_rx_cut` runs only with the caller's waker.
        if self.cut_retry || self.cut_due {
            let ended = match self.waker.as_ref() {
                Some(waker) => self
                    .device
                    .poll_abandon_rx_cut(&mut Context::from_waker(waker)),
                None => Poll::Ready(Ok(())),
            };
            match ended {
                Poll::Pending => {
                    self.hold_mutation(mutator);
                    return Ok(GroupStep::Held(command));
                }
                Poll::Ready(Err(cause)) => return Ok(GroupStep::Failed(command, cause)),
                Poll::Ready(Ok(())) => {}
            }
        }
        if let Some(identity) = removal {
            // The key is installed: its removal did not run.
            identity.activity.set(identity.serial);
            if self.driver.barrier == Some(identity) {
                self.driver.barrier = None;
            }
        }
        self.group_since = None;
        self.group_mutator = None;
        self.cut_retry = false;
        self.cut_due = false;
        match mutator {
            Mutator::Station(_) => self.holding = None,
            Mutator::AccessPoint(_) => self.access_point_holding = None,
        }
        Ok(GroupStep::Expired(command))
    }

    /// Mark the mutation held at its barrier.
    fn hold_mutation(&mut self, mutator: Mutator) {
        match mutator {
            Mutator::Station(kind) => self.holding = Some(kind),
            Mutator::AccessPoint(kind) => self.access_point_holding = Some(kind),
        }
    }

    /// Run one command of a running port. A fault before the command's
    /// owners moved into the port returns the command beside it.
    #[allow(
        clippy::result_large_err,
        reason = "the failure returns the command by value, as a hold does"
    )]
    fn run_command<S>(
        &mut self,
        command: Cmd<'id, Id, D, B, O>,
        now: Instant,
        sink: &mut S,
    ) -> Result<CmdProgress<'id, Id, D, B, O>, CmdFailed<'id, Id, D, B, O>>
    where
        S: PortSink<'id, Types<Id, D, B, O>>,
    {
        match command {
            PortCommand::SetEdca(parameters) => {
                let [station, _] = &mut self.settings.edca;
                *station = parameters.get();
                self.settings.edca_dirty = true;
                self.apply_settings().map_err(CommandFailed::Taken)?;
                Ok(CommandProgress::Done)
            }
            PortCommand::SetBssColor(color) => {
                self.settings.color = Some(color);
                self.apply_settings().map_err(CommandFailed::Taken)?;
                Ok(CommandProgress::Done)
            }
            PortCommand::SetHe(he) => {
                self.settings.he = Some(he);
                self.apply_settings().map_err(CommandFailed::Taken)?;
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
                self.close_quiet_gate(sink).map_err(CommandFailed::Taken)?;
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
                if let Err(fault) = core_views::configure::<Id, D, B>(
                    self.driver.core.view(),
                    CoreVif {
                        role: VifRole::Station,
                        address: self.addresses[0].get(),
                        bssid,
                        receive: filter,
                    },
                ) {
                    return Err(CommandFailed::Returned {
                        command: PortCommand::Configure {
                            permission,
                            receive,
                        },
                        fault,
                    });
                }
                self.holding = None;
                self.driver.configuring = Some(brand::OwnedConfigured::Station(
                    permission.into_owned(&self.loan),
                ));
                self.advance_configuration(sink)
                    .map_err(CommandFailed::Taken)?;
                Ok(CommandProgress::Done)
            }
            PortCommand::InstallPairwise(PairwiseInstall {
                identity,
                installation,
                material,
                issued,
            }) => {
                // The terminal leaves under the call's right, obtained
                // before the core takes the slot: without it the command
                // waits for the next call, whole.
                let Some(vacancy) = self.unkept.vacancy() else {
                    return Ok(CommandProgress::Held(PortCommand::InstallPairwise(
                        PairwiseInstall {
                            identity,
                            installation,
                            material,
                            issued,
                        },
                    )));
                };
                // Refused, the installation comes back whole with its
                // command: the core did not take its slot.
                if let Err((installation, fault)) =
                    self.driver
                        .install(self.device, identity, installation, &material)
                {
                    self.unkept.vacate(vacancy);
                    return Err(CommandFailed::Returned {
                        command: PortCommand::InstallPairwise(PairwiseInstall {
                            identity,
                            installation,
                            material,
                            issued,
                        }),
                        fault,
                    });
                }
                identity.activity.set(identity.serial);
                self.unconsumed.deliver(
                    sink,
                    &mut self.unkept,
                    vacancy,
                    unconsumed::Item::Terminal(
                        brand::OwnedTerminal::PairwiseKeyInstalled(
                            brand::OwnedInstalledPairwise::new(identity.role, identity),
                        )
                        .lend(&self.loan),
                    ),
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
                // The terminal leaves under the call's right, obtained
                // before the core removes the key.
                let vacancy = match self.unkept.vacancy() {
                    Some(vacancy) if drained => vacancy,
                    vacancy => {
                        if let Some(vacancy) = vacancy {
                            self.unkept.vacate(vacancy);
                        }
                        self.holding = Some(
                            oer_protocols_ieee80211_contracts_port::CommandKind::RemovePairwise,
                        );
                        return Ok(CommandProgress::Held(PortCommand::RemovePairwise {
                            association,
                            key,
                        }));
                    }
                };
                let removed = match self
                    .driver
                    .remove::<core_mac::association::StationAssociation<'static, CorePath<Id>>>(
                        self.device,
                        identity,
                    ) {
                    Ok(removed) => removed,
                    Err(fault) => {
                        self.unkept.vacate(vacancy);
                        return Err(CommandFailed::Returned {
                            command: PortCommand::RemovePairwise { association, key },
                            fault,
                        });
                    }
                };
                let terminal = removed.settle(association.into_owned(&self.loan));
                self.driver.barrier = None;
                self.holding = None;
                // A removal whose key's last use is out ends at a later
                // service ([`Self::deliver_removals`]).
                match terminal {
                    Some(terminal) => self.unconsumed.deliver(
                        sink,
                        &mut self.unkept,
                        vacancy,
                        unconsumed::Item::Terminal(
                            brand::OwnedTerminal::KeyRemoved(terminal).lend(&self.loan),
                        ),
                    ),
                    None => self.unkept.vacate(vacancy),
                }
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
            station_group_permission,
            access_point_group_permission,
            block_acks,
            station_commands,
            access_point_commands,
            beacon,
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
        let prepared = driver.prepare(core, core_tickets, queues, descriptors, observations);
        // The lend credit outlives the previous installation: units the
        // network still holds stay counted and rejoin this ring.
        shared.network.reset();
        let generation = installation.generation();
        let resources = Resources {
            tickets: tickets.map(|slot| loan.lend_slot(slot)),
            terminal_access: terminal_access.lend(&loan),
            buffers: buffers.map(|buffer| buffer.lend(&loan)),
            connection_epoch: loan.lend_slot(connection_epoch),
            // Every token names this installation: a later one refuses it.
            station_association: station_association.issue(generation).lend(&loan),
            access_point_association: access_point_association.issue(generation).lend(&loan),
            station_commands: station_commands.issue(generation).lend(&loan),
            access_point_commands: access_point_commands.issue(generation).lend(&loan),
            block_acks: block_acks.map(|slot| slot.lend(&loan)),
            station_group_permission: {
                let [permission] = station_group_permission.issue(generation);
                permission.lend(&loan)
            },
            access_point_group_permission: {
                let [permission] = access_point_group_permission.issue(generation);
                permission.lend(&loan)
            },
            access_point_beacon: beacon.issue(generation).lend(&loan),
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

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
    Esp32s31Port<'id, Id, D, B, O>
{
    /// The answer of a producing call, beside the one item it could not
    /// keep, if any: the call accepted its input, and its own answer
    /// stands. A held item leaves the place here, which makes the next
    /// right ([`unconsumed::CallPlace::settle`]).
    fn answer<I, R>(&mut self, answer: R) -> PortAnswer<'id, Types<Id, D, B, O>, I, R> {
        // A call that ends with an empty place and no right lost it: the
        // port records its permanent fault, and its next calls answer it.
        if self.unkept.right_lost() {
            self.record(Fault::RightLost);
        }
        match self.unkept.settle() {
            Some(item) => Err(Unkeeping {
                unkept: item.into_unkept(),
                input: Input::Accepted(answer),
            }),
            None => Ok(answer),
        }
    }

    fn served_tx(
        &mut self,
        tx: TxFacts,
        tbtt: bool,
        now: Instant,
        sink: &mut impl PortSink<'id, <Self as DataPort<'id>>::Types>,
    ) -> Result<Option<PortCompleted<'id, Self>>, <Self as DataPort<'id>>::Fault> {
        self.reoffer(sink);
        self.check()?;
        self.now = now;
        if !matches!(
            self.state,
            PortState::Running | PortState::Recovering | PortState::Stopping { .. }
        ) {
            // A stopped port ends the removals the stop finished. A
            // replacement whose rollback failed waits for the lifecycle
            // owner's cleanup ([`DataPort::clean_stopped`]), not for this
            // service.
            let delivered = self
                .deliver_removals(sink)
                .map_err(|fault| self.record(fault));
            self.recompute_deadline();
            return delivered.map(|()| None);
        }
        let done = self
            .turn(tx, tbtt, now, sink)
            .and_then(|done| self.deliver_removals(sink).map(|()| done))
            .map_err(|fault| self.record(fault));
        self.recompute_deadline();
        done
    }

    /// Take `attempt` under the submission's right `vacancy`, which covers
    /// the attempt's own terminal: its refusal, or its publication at once.
    fn submitted(
        &mut self,
        mut attempt: <Self as DataPort<'id>>::Admitted,
        vacancy: unconsumed::Vacancy,
        now: Instant,
        sink: &mut impl PortSink<'id, <Self as DataPort<'id>>::Types>,
    ) -> Result<(), SubmitRefusal> {
        self.reoffer(sink);
        let reason = match self.state {
            PortState::Running if self.check().is_ok() => None,
            PortState::Stopping { .. } | PortState::Recovering => Some(SubmitRefusal::Stopping),
            _ => Some(SubmitRefusal::NotEnabled),
        };
        let installation = self.serials.generation;
        // An attempt under a key or a beacon permission of an earlier
        // installation is refused for good; it is checked before the
        // radio's state.
        let stale = attempt
            .operation
            .payload()
            .key
            .as_ref()
            .map(RawKeyIdentity::generation)
            .into_iter()
            .chain(
                attempt
                    .execution
                    .beacon
                    .as_ref()
                    .map(|(permission, _)| permission.generation()),
            )
            .find(|issued| *issued != installation);
        let refusal = match (stale, reason) {
            (Some(issued), _) => Some((
                SubmitRefusal::StalePermission {
                    issued,
                    installation,
                },
                oer_protocols_ieee80211_contracts_port::WithdrawCause::StalePermission,
            )),
            (None, Some(reason)) => Some((
                reason,
                oer_protocols_ieee80211_contracts_port::WithdrawCause::Disabled,
            )),
            (None, None) => None,
        };
        if let Some((reason, cause)) = refusal {
            // The attempt ends at once, withdrawn and never published: its
            // terminal returns every owner.
            let completed = attempt.operation.complete(
                oer_protocols_ieee80211_contracts_port::TxOutcome::Withdrawn(cause),
                // Never published: nothing reached the air.
                oer_protocols_ieee80211_contracts_port::TxEstimate::NONE,
            );
            let completed = match attempt.execution.beacon {
                Some((permission, _)) => brand::OwnedTxCompleted::Beacon {
                    completed,
                    permission,
                },
                None => brand::OwnedTxCompleted::Single(completed),
            };
            self.unconsumed.deliver(
                sink,
                &mut self.unkept,
                vacancy,
                unconsumed::Item::Terminal(completed.lend(&self.loan)),
            );
            return Err(reason);
        }
        self.now = now;
        // Admission happens on this call: the MSDU ages from here.
        attempt.operation.execute().mpdu.buffer.admit(now);
        let index = queue(attempt.access_category);
        let attempt_role = attempt.operation.payload().role;
        let tries_left = attempt
            .execution
            .plan
            .first()
            .transmissions
            .get()
            .saturating_sub(1);
        let pending = Pending {
            operation: attempt.operation,
            plan: attempt.execution.plan,
            category: attempt.access_category,
            protection: attempt.protection,
            power: attempt.power.get(),
            coex: attempt.coex_operation,
            tries_left,
            published: false,
            air: None,
            try_work: None,
            cost: TxEstimate::NONE,
            beacon: attempt.execution.beacon,
        };
        // A queue holds every ticket's attempt (`QUEUE_DEPTH`), and this
        // attempt holds one: the push has room.
        if let Err(pending) = self.driver.enqueue(index, pending, false) {
            // Refused all the same: the attempt ends withdrawn with every
            // owner, and the fault stops the port.
            self.record(Fault::QueueFull {
                queue: u8::try_from(index).unwrap_or(u8::MAX),
            });
            let (driver, _, mut env) = self.parts(sink);
            driver.finish(
                &mut env,
                vacancy,
                pending,
                oer_protocols_ieee80211_contracts_port::TxOutcome::Withdrawn(
                    oer_protocols_ieee80211_contracts_port::WithdrawCause::Disabled,
                ),
            );
            return Ok(());
        }
        // A free queue publishes its head at once: nothing ends here.
        let driver = &mut self.driver;
        let free =
            driver.running.get(index).is_some_and(Option::is_none) && driver.free.is_home_at(index);
        let alone = driver.queues.get(index).is_some_and(|queued| {
            queued.len() == 1 && queued.front().is_some_and(|p| !driver.blocked(p))
        });
        let role = attempt_role;
        let publishable = free
            && alone
            && !(self.holding_blocks() && role == VifRole::Station)
            && !self.driver.closed
            && self.driver.closing.is_none()
            && core_views::configuring_role::<Id, D, B>(self.driver.core.view()) != Some(role);
        if publishable {
            let (driver, device, mut env) = self.parts(sink);
            let published = match driver
                .queues
                .get_mut(index)
                .and_then(heapless::Deque::pop_front)
            {
                Some(pending) => driver.publish(device, &mut env, vacancy, index, pending),
                None => {
                    env.vacate(vacancy);
                    Ok(())
                }
            };
            if let Err(fault) = published {
                self.record(fault);
            }
        } else {
            self.unkept.vacate(vacancy);
        }
        self.recompute_deadline();
        Ok(())
    }

    #[allow(
        clippy::result_large_err,
        reason = "the failure returns the command by value, as the trait's call does"
    )]
    fn ran(
        &mut self,
        command: PortCommand<'id, <Self as DataPort<'id>>::Types, <Self as DataPort<'id>>::Caps>,
        now: Instant,
        sink: &mut impl PortSink<'id, <Self as DataPort<'id>>::Types>,
    ) -> Result<
        CommandProgress<'id, <Self as DataPort<'id>>::Types, <Self as DataPort<'id>>::Caps>,
        PortCommandFailed<'id, Self>,
    > {
        self.reoffer(sink);
        // A faulted port keeps no command: it comes back beside the fault.
        if let Err(fault) = self.check() {
            return Err(CommandFailed::Returned { command, fault });
        }
        self.now = now;
        if let Some(arena) = self.driver.arena.as_mut() {
            arena.forget_program();
        }
        // A token of an earlier installation is refused for good, before
        // the radio's own state is asked.
        let installation = self.serials.generation;
        if let Some(issued) = station_generations::<Id, D, B, O>(&command)
            .into_iter()
            .flatten()
            .find(|issued| *issued != installation)
        {
            return Ok(CommandProgress::Refused(
                command,
                CommandRefusal::StalePermission {
                    issued,
                    installation,
                },
            ));
        }
        if !self.running() {
            return Ok(CommandProgress::Refused(
                command,
                CommandRefusal::NotEnabled,
            ));
        }
        let progress = self.run_command(command, now, sink);
        let progress = progress.map_err(|failed| failed.map_fault(|fault| self.record(fault)));
        self.recompute_deadline();
        progress
    }

    fn cleaned(
        &mut self,
        sink: &mut impl PortSink<'id, <Self as DataPort<'id>>::Types>,
    ) -> Result<(), CleanupRefused<<Self as DataPort<'id>>::Fault>> {
        if self.state != PortState::Stopped {
            return Err(CleanupRefused::NotStopped);
        }
        self.check().map_err(CleanupRefused::Faulted)?;
        // Completions a halted call left with the driver go home first,
        // each under the place's right: software only, no hardware call. A
        // refused one fills the place and the rest stay with the driver.
        let (driver, _device, mut env) = self.parts(sink);
        let mut none = driver.completion_sink();
        if let Err(fault) = driver.collect(&mut env, &mut none, true) {
            return Err(CleanupRefused::Faulted(self.record(fault)));
        }
        // The cleanup's result is the owner's, never the port's recorded
        // fault: a later call goes on from where it stands, writing nothing
        // it wrote before.
        let cleaned = self.clean_retained_group(sink);
        self.recompute_deadline();
        cleaned
    }
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations> DataPort<'id>
    for Esp32s31Port<'id, Id, D, B, O>
{
    type Caps = Esp32s31Caps<B>;
    type Types = Types<Id, D, B, O>;
    type Fault = Fault;
    type Admitted = AdmittedTxParts<
        D::OwnedContext<Id>,
        Slot<'static, radio::Slots<Tickets, TX_TICKETS>, Part<Id, 2>>,
        Mpdu<'static, Id, D, B, O>,
        buffer::Execution<Id>,
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
        if !self
            .waker
            .as_ref()
            .is_some_and(|waker| waker.will_wake(cx.waker()))
        {
            self.waker = Some(cx.waker().clone());
        }
        let mut facts = Facts::default();
        if self.check().is_err() {
            // The next service answers with the fault.
            facts.tx = Some(TxFacts::DUE);
            return facts;
        }
        if !matches!(
            self.state,
            PortState::Running | PortState::Recovering | PortState::Stopping { .. }
        ) {
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
        // A unit the network returned rearms the ring at the next pull. The
        // return is taken here, so it is one fact: a caller that cannot pull
        // now (the network holds all it may) does not hear it again.
        let quiet = self.device.returns_quiet(
            self.shared.network.held(),
            self.shared.network.returned_unseen(),
        );
        if self.shared.network.poll_returns(cx, quiet) && self.shared.network.take_returned() {
            self.device.returned();
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
    ) -> PortAnswer<'id, Self::Types, (), Result<Option<PortCompleted<'id, Self>>, Self::Fault>>
    {
        let served = self.served_tx(tx, tbtt, now, sink);
        self.answer(served)
    }

    fn service_arena(
        &mut self,
        tx: TxFacts,
        now: Instant,
    ) -> Result<Option<PortCompleted<'id, Self>>, Self::Fault> {
        self.check()?;
        if tx.bits() != crate::mac::irq::EVENT_TX_COMPLETE
            || !matches!(self.state, PortState::Running)
            || self.driver.arena_abort_settling()
        {
            return Ok(None);
        }
        self.now = now;
        let wake = WifiTxWake::Interrupt { events: tx.bits() };
        self.driver
            .service_arena(&mut self.device, wake, now)
            .map(|(done, _)| done)
            .map_err(|fault| self.record(fault))
    }

    fn arena(&mut self) -> Self::Arena<'_> {
        Esp32s31Arena { port: self }
    }

    fn admit<'k, 'p>(&self, attempt: StationSubmission<'k, 'p, 'id, Self>) -> Self::Admitted {
        attempt.into_parts(&self.loan)
    }

    fn submit(
        &mut self,
        attempt: Self::Admitted,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> SubmitAnswer<'id, Self::Types, Self::Admitted> {
        // The place's right covers the attempt's own terminal: what the
        // port kept is offered again and stays in its own place when
        // refused. Without the right the port takes nothing of the attempt.
        let Some(vacancy) = self.unkept.vacancy() else {
            self.record(Fault::RightLost);
            return Err(SubmitHalt::NotAccepted(attempt));
        };
        let submitted = self.submitted(attempt, vacancy, now, sink);
        self.answer(submitted).map_err(SubmitHalt::Unkeeping)
    }

    fn receive(&mut self, room: u8, now: Instant) -> Result<PortRxPull<'id, Self>, Self::Fault> {
        self.check()?;
        self.now = now;
        if !self.running() {
            return Ok(RxPull::Empty);
        }
        settle_lent(&self.shared.network, &self.rx_out, self.device)
            .map_err(|fault| self.record(fault))?;
        // This pull serves the continuation the last one named: a passed
        // one no longer bounds the deadline, which would otherwise ask for
        // a transmit service at every turn.
        if self.rx_later.is_some_and(|at| at <= now) {
            self.rx_later = None;
            self.recompute_deadline();
        }
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

    fn recycle(
        &mut self,
        unit: PortRxBuffer<'id, Self>,
    ) -> Result<(), RecycleRefused<PortRxBuffer<'id, Self>>> {
        // The count first: a refused unit is not touched.
        let held = self.rx_out.get();
        let Some(after) = held.checked_sub(1) else {
            return Err(RecycleRefused {
                unit,
                fault: ReceiveAccounting {
                    operation: ReceiveOperation::Recycle,
                    held,
                    requested: 1,
                },
            });
        };
        self.rx_out.set(after);
        self.device.recycle(unit.into_unit());
        Ok(())
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
        slot: PortBlockAckSlot<'id, Self::Types>,
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
    ) -> PortBlockAckSlot<'id, Self::Types> {
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
    ) -> PortAnswer<
        'id,
        Self::Types,
        PortCommand<'id, Self::Types, Self::Caps>,
        Result<CommandProgress<'id, Self::Types, Self::Caps>, PortCommandFailed<'id, Self>>,
    > {
        let ran = self.ran(command, now, sink);
        self.answer(ran)
    }

    fn poll_start(
        &mut self,
        cx: &mut Context<'_>,
        channel: SupportedChannel<Self::Caps>,
        now: Instant,
    ) -> Poll<Result<(), StartRefused<Fault>>> {
        self.now = now;
        let since = match self.state {
            PortState::Running | PortState::Recovering | PortState::Stopping { .. } => {
                return Poll::Ready(Err(StartRefused::Running));
            }
            PortState::Stopped => {
                self.state = PortState::Starting { since: now };
                now
            }
            PortState::Starting { since } => since,
            PortState::RollingBack { since, refused } => {
                return self.poll_roll_back(cx, since, refused, now);
            }
        };
        if self.check().is_err() {
            self.state = PortState::Stopped;
            return Poll::Ready(Err(StartRefused::Device));
        }
        if now.saturating_duration_since(since) >= COMMAND_BOUND {
            // What the device began of the start stops first.
            return self.roll_back_start(cx, StartFailure::Deadline, now);
        }
        let Some(wifi) = channel.channel().ghz2_4_channel() else {
            self.state = PortState::Stopped;
            return Poll::Ready(Err(StartRefused::Retune(Fault::ChannelUnsupported)));
        };
        let started = match self.device.poll_start(cx, wifi) {
            Poll::Pending => {
                self.recompute_deadline();
                return Poll::Pending;
            }
            Poll::Ready(started) => started,
        };
        // A device that refused its start and one that started before a
        // later step failed both stop what they began before the refusal
        // is answered: the device is never left started.
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
        let refused = match tuned {
            Ok(()) => {
                self.state = PortState::Running;
                self.channel = channel.channel();
                self.stop_served = false;
                self.core_disabled = false;
                self.recompute_deadline();
                return Poll::Ready(Ok(()));
            }
            Err(cause @ (Fault::Retune(_) | Fault::ChannelNotPermitted)) => {
                StartFailure::Retune(cause)
            }
            Err(_) => StartFailure::Device,
        };
        self.roll_back_start(cx, refused, now)
    }

    fn poll_stop(
        &mut self,
        cx: &mut Context<'_>,
        now: Instant,
    ) -> Poll<Result<(), StopUnconfirmed<Fault>>> {
        self.now = now;
        let since = match self.state {
            PortState::Stopped => return Poll::Ready(Ok(())),
            PortState::Starting { .. } | PortState::RollingBack { .. } | PortState::Running => {
                self.state = PortState::Stopping { since: now };
                self.stop_served = false;
                // A stopping port refuses the held command back to its
                // caller: nothing of it stays to run against the restart.
                self.forget_held();
                self.recompute_deadline();
                now
            }
            PortState::Recovering => {
                self.state = PortState::Stopping { since: now };
                self.recompute_deadline();
                now
            }
            PortState::Stopping { since } => since,
        };
        let unconfirmed = |port: &mut Self, cause: StopCause<Fault>| {
            Poll::Ready(Err(StopUnconfirmed {
                cause,
                observations: port.stop_observations(),
            }))
        };
        if let Err(fault) = self.check() {
            return unconfirmed(self, StopCause::Device(fault));
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
            // A mask the line's handler has not confirmed within its bound
            // is a stop not confirmed yet: the handler may still serve it,
            // and the next poll confirms then. Only another failure of the
            // device's stop is its fault.
            Poll::Ready(Err(Fault::Receive(crate::rx::control::WifiRxCause::LineRefused))) => {
                unconfirmed(self, StopCause::Deadline)
            }
            Poll::Ready(Err(fault)) => unconfirmed(self, StopCause::Device(fault)),
            Poll::Pending if expired => unconfirmed(self, StopCause::Deadline),
            Poll::Pending => {
                // The device may have asked to look again before the bound.
                self.recompute_deadline();
                Poll::Pending
            }
        }
    }

    fn recover_for_stop(&mut self) -> bool {
        // A faulted port keeps no command: the call that faulted returned
        // it to its caller.
        // Units of buffers dropped unrecycled are back in the ring: they
        // leave the held count here, and the loss stays the port's cause
        // until this recovery.
        //
        // The held count is the installation's live buffers, plus its
        // lends not yet settled, plus its losses not yet settled: a buffer
        // joins it once at its admission and leaves it once, by its
        // recycle, its lend's settlement or its loss's settlement here; a
        // loss is counted only by a buffer the count holds. A retirement
        // needs the count at zero, so a reinstallation starts with no
        // buffer, lend or loss of the previous one. The losses read here
        // are therefore within the held count, and the settlement below
        // subtracts exactly them, so a loss recorded meanwhile stays.
        let lost = self.shared.lost_units();
        if lost > 0 {
            let Some(held) = self.rx_out.get().checked_sub(lost) else {
                // Excluded by the count above. Nothing is settled: the
                // held and lost counts and the port's cause stay as they
                // were, and no recovery follows a failed settlement.
                return false;
            };
            self.rx_out.set(held);
            self.shared.settle_lost(lost);
            self.device.returned();
            self.record(Fault::ReceiveLost);
        }
        let recoverable = self.fault.is_some_and(|fault| fault.leaves_owners_whole());
        if recoverable {
            self.fault = None;
            if matches!(self.state, PortState::Starting { .. } | PortState::Running) {
                self.state = PortState::Recovering;
                self.stop_served = false;
                self.forget_held();
                self.recompute_deadline();
            }
        }
        recoverable
    }

    fn clean_stopped(
        &mut self,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> PortAnswer<'id, Self::Types, (), Result<(), CleanupRefused<Self::Fault>>> {
        let cleaned = self.cleaned(sink);
        self.answer(cleaned)
    }

    fn take_unconsumed(
        &mut self,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> Result<Unconsumed, UnconsumedRefused> {
        if self.state != PortState::Stopped {
            return Err(UnconsumedRefused::NotStopped);
        }
        Ok(self.unconsumed.offer(sink))
    }

    fn sample_clock(&mut self) -> Result<Ieee80211Instant, ClockError> {
        self.device.radio_now()
    }
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
    Esp32s31Port<'id, Id, D, B, O>
{
    /// Begin to undo a failed start: the device stops its receive path
    /// before `refused` is answered. A start that enabled the core (its
    /// settings failed after the tune) is not undone here: the port is
    /// stopping, as after [`DataPort::poll_stop`]'s first call, and the
    /// answer is [`StartRefused::Unstopped`] with the start's cause.
    fn roll_back_start(
        &mut self,
        cx: &mut Context<'_>,
        refused: StartFailure<Fault>,
        now: Instant,
    ) -> Poll<Result<(), StartRefused<Fault>>> {
        if !core_views::disabled::<Id, D, B>(self.driver.core.view()) {
            // The stop's own service disables the core.
            self.state = PortState::Stopping { since: now };
            self.stop_served = false;
            self.core_disabled = false;
            self.recompute_deadline();
            return Poll::Ready(Err(StartRefused::Unstopped {
                cause: refused,
                stop: StopUnconfirmed {
                    cause: StopCause::CoreEnabled,
                    observations: self.stop_observations(),
                },
            }));
        }
        self.state = PortState::RollingBack {
            since: now,
            refused,
        };
        self.poll_roll_back(cx, now, refused, now)
    }

    /// Undo a failed start: once the device confirms its receive path
    /// paused (at once for one that never started it) the port is stopped
    /// and the start's refusal is answered. A device that refuses that
    /// pause, or does not confirm it within the command bound, leaves the
    /// radio not stopped: the answer is [`StartRefused::Unstopped`] with
    /// the start's cause and the stop's, and the port is stopping, so
    /// [`DataPort::poll_stop`] goes on with it and a release refuses until
    /// it confirms.
    fn poll_roll_back(
        &mut self,
        cx: &mut Context<'_>,
        since: Instant,
        refused: StartFailure<Fault>,
        now: Instant,
    ) -> Poll<Result<(), StartRefused<Fault>>> {
        let unstopped = |port: &mut Self, cause: StopCause<Fault>| {
            port.state = PortState::Stopping { since };
            Err(StartRefused::Unstopped {
                cause: refused,
                stop: StopUnconfirmed {
                    cause,
                    observations: port.stop_observations(),
                },
            })
        };
        let result = match self.device.poll_stop(cx) {
            Poll::Ready(Ok(())) => {
                self.state = PortState::Stopped;
                Err(refused.into())
            }
            Poll::Ready(Err(fault)) => unstopped(self, StopCause::Device(fault)),
            Poll::Pending if now.saturating_duration_since(since) >= COMMAND_BOUND => {
                unstopped(self, StopCause::Deadline)
            }
            Poll::Pending => {
                self.recompute_deadline();
                return Poll::Pending;
            }
        };
        self.recompute_deadline();
        Poll::Ready(result)
    }

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
        // The arena core hands its port nothing: a receipt or a completion
        // it finishes waits with the core for the next ordinary service.
        let mut sink = Sink::refusing();
        let result = {
            let mut arena_port: driver::arena::Port<'_, 'static, Id, D, B, O> = ArenaPort::new(
                arena,
                core,
                hardware,
                &mut driver.free.bank,
                VifRole::Station,
                pairwise,
                &mut sink,
                observations,
            );
            step(&mut arena_port)
        };
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

    fn event_waiting(&self) -> bool {
        self.port.device.event_waiting()
    }

    fn publish(
        &mut self,
        publication: ArenaPublication<'t, Esp32s31Caps<B>>,
        now: Instant,
    ) -> Result<(), PublishError<'t, Esp32s31Caps<B>, Fault>> {
        if self.publication_blocked() {
            return Err(PublishError::Refused {
                publication,
                reason: PublishRefused::Busy,
            });
        }
        self.port.now = now;
        let mut publication = Some(publication);
        let published = self.with_port(|port| {
            let Some(taken) = publication.take() else {
                return Ok(Ok(()));
            };
            match port.publish(taken, now) {
                Ok(()) => Ok(Ok(())),
                Err(PublishError::Refused {
                    publication,
                    reason,
                }) => Ok(Err((publication, reason))),
                Err(PublishError::FaultBeforeTaking {
                    publication: back,
                    fault,
                }) => {
                    publication = Some(back);
                    Err(fault)
                }
                Err(PublishError::Fault(fault)) => Err(fault),
            }
        });
        match published {
            Ok(Ok(())) => Ok(()),
            Ok(Err((publication, reason))) => Err(PublishError::Refused {
                publication,
                reason,
            }),
            Err(fault) => Err(match publication.take() {
                // The port did not see the publication.
                Some(publication) => PublishError::FaultBeforeTaking { publication, fault },
                None => PublishError::Fault(fault),
            }),
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
