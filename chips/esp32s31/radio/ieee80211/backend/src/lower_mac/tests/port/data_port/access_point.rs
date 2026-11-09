//! The S31 port serves the access-point role: the shared access-point
//! conformance cases over the model device, its register model observing
//! the access point's interface and its publications.
use super::lifecycle::reinstall;
use super::*;
use crate::lower_mac::tests::{AccessPointObserved, PEER, queue_of};
use oer_chips_esp32s31_hal::radio::ieee80211::registers::tx::MacLegacyTxProgram;
use oer_chips_esp32s31_hal::types::MacInterface;
use oer_protocols_ieee80211_contracts_port::{AccessPointCommand, CommandFailed};
use oer_protocols_ieee80211_testing_conformance::access_point::{
    self as ap_cases, AccessPointHarness, End as CaseEnd, Ended as CaseEnded, Interface, Issued,
    Receive, Sent as CaseSent, Terminal as CaseTerminal, Violation as CaseViolation,
};
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;

type ApKeys<Id> = PortKeys<Id, AccessPoint>;
type StaKeys<Id> = PortKeys<Id, Station>;
type ApCommand<'id, Id> = AccessPointCommand<'id, Family<Id>, Caps>;
type ApConfigure<'id, Id> =
    ConfigurePermission<'id, AccessPoint, radio::Child<radio::Root<Id>, 9>, Tag>;
type Beacon<'id, Id> = BeaconPermission<'id, radio::Child<radio::Root<Id>, 10>>;
type Admitted<'id, Id> = <Port<'id, Id> as DataPort<'id>>::Admitted;

/// The access point's tokens an installation minted, as the fixture keeps
/// them.
pub(super) struct ApResources<'id, Id: radio::SlotIdentity + 'id> {
    pub(super) association: crate::lower_mac::association::VacantAccessPointAssociation<
        'id,
        radio::Child<radio::Root<Id>, 0>,
    >,
    pub(super) configure: ApConfigure<'id, Id>,
    pub(super) group_permission: <ApKeys<Id> as KeyTypes<'id>>::GroupPermission,
    pub(super) beacon: Beacon<'id, Id>,
}

/// The access point's tokens of one installation between the cases' calls.
struct ApTokens<'id, Id: radio::SlotIdentity + 'id> {
    configure: Option<ApConfigure<'id, Id>>,
    association: Option<<ApKeys<Id> as KeyTypes<'id>>::Association>,
    pairwise: Vec<<ApKeys<Id> as KeyTypes<'id>>::PairwiseSlot>,
    group: Vec<<ApKeys<Id> as KeyTypes<'id>>::GroupSlot>,
    group_permission: Option<<ApKeys<Id> as KeyTypes<'id>>::GroupPermission>,
    beacon: Option<Beacon<'id, Id>>,
    pairwise_key: Option<<ApKeys<Id> as KeyTypes<'id>>::PairwiseKey>,
    group_key: Option<<ApKeys<Id> as KeyTypes<'id>>::GroupKey>,
    number: u64,
}

impl<'id, Id: radio::SlotIdentity + 'id> ApTokens<'id, Id> {
    fn new(resources: ApResources<'id, Id>) -> Self {
        let (start, bank) = resources.association.into_parts();
        let (pairwise, group) = bank.into_parts();
        Self {
            configure: Some(resources.configure),
            association: Some(
                start.begin(oer_protocols_ieee80211_wire::security::Spp::NotNegotiated),
            ),
            pairwise: pairwise.into_iter().collect(),
            group: group.into_iter().collect(),
            group_permission: Some(resources.group_permission),
            beacon: Some(resources.beacon),
            pairwise_key: None,
            group_key: None,
            number: 0,
        }
    }

    fn none() -> Self {
        Self {
            configure: None,
            association: None,
            pairwise: Vec::new(),
            group: Vec::new(),
            group_permission: None,
            beacon: None,
            pairwise_key: None,
            group_key: None,
            number: 0,
        }
    }
}

/// The S31 port as the access-point conformance cases call it.
/// The station's association and its installed pairwise key.
type StationKey<'id, Id> = (
    <StaKeys<Id> as KeyTypes<'id>>::Association,
    <StaKeys<Id> as KeyTypes<'id>>::PairwiseKey,
);

struct ApFixture<'a, 'id, Id: radio::SlotIdentity + 'id> {
    fixture: Option<Fixture<'id, Id>>,
    installations: &'a mut Installations<Id, InstallationPools>,
    current: ApTokens<'id, Id>,
    earlier: ApTokens<'id, Id>,
    /// The current installation's generation: 0, then 1 after a reinstall.
    generation: u32,
    station_key: Option<StationKey<'id, Id>>,
    station_number: u64,
    /// The register model's view of the access point's interface.
    interface: Rc<RefCell<AccessPointObserved>>,
    /// Every legacy publication's program, in order.
    published: Rc<RefCell<Vec<MacLegacyTxProgram>>>,
    /// The key id the next group installation names.
    group_key_id: u8,
}

/// The header of a data MPDU from `transmitter` to `receiver` in the BSS
/// of `bssid`, with the frame control `control`.
fn header(
    control: u16,
    receiver: [u8; 6],
    transmitter: [u8; 6],
    bssid: [u8; 6],
) -> HeaderSpec<IndividualAddress> {
    HeaderSpec::new(
        FrameControl::new(control).expect("control"),
        0,
        IndividualAddress::new(receiver).expect("receiver"),
        transmitter,
        bssid,
        0,
        None,
        None,
        None,
    )
    .expect("header")
}

impl<'a, 'id, Id: radio::SlotIdentity + 'id> ApFixture<'a, 'id, Id> {
    fn f(&mut self) -> &mut Fixture<'id, Id> {
        self.fixture.as_mut().expect("the fixture")
    }

    fn fr(&self) -> &Fixture<'id, Id> {
        self.fixture.as_ref().expect("the fixture")
    }

    fn tokens(&mut self, of: Issued) -> &mut ApTokens<'id, Id> {
        match of {
            Issued::Current => &mut self.current,
            Issued::Earlier => &mut self.earlier,
        }
    }

    /// The tokens of the installation `generation`.
    fn of(&mut self, generation: u32) -> &mut ApTokens<'id, Id> {
        if generation == self.generation {
            &mut self.current
        } else {
            &mut self.earlier
        }
    }

    /// An access-point data MPDU to the peer on `category`, under the
    /// pairwise key of `key` when given.
    fn access_point_mpdu(
        &mut self,
        key: Option<Issued>,
        category: WmmAccessCategory,
    ) -> Option<Admitted<'id, Id>> {
        let (number, pairwise) = match key {
            Some(of) => {
                let tokens = self.tokens(of);
                tokens.number += 1;
                (tokens.number, Some(tokens.pairwise_key.take()?))
            }
            None => (0, None),
        };
        let fixture = self.fixture.as_mut()?;
        let buffer = fixture.buffers.pop()?;
        let ticket = fixture.tickets.pop()?;
        let rate = fixture.port.rate(PhyRate::Legacy(LegacyRate::Ofdm6M))?;
        let control = if pairwise.is_some() { 0x4208 } else { 0x0208 };
        let frame = ClassFrame::new(TxFrame::Individual {
            frame: SizedFrame::new(
                AckHeader::new(header(control, PEER, ACCESS_POINT, ACCESS_POINT)).expect("ACK"),
                MpduLength::new(64).expect("length"),
            )
            .expect("frame"),
            plan: RetryPlan::new(RetryStage {
                rate,
                transmissions: core::num::NonZeroU8::MIN,
                protection: Protection::None,
            }),
        })
        .ok()?;
        let prepared = {
            let protection = match &pairwise {
                Some(key) => TxProtection::Ccmp(NumberedKey {
                    key: AttemptKey::Pairwise(key),
                    packet_number: oer_protocols_ieee80211_wire::ccmp::CcmpPacketNumber::new(
                        number,
                    )
                    .expect("PN"),
                }),
                None => TxProtection::Open,
            };
            let source = TxSource::Fresh {
                buffer,
                frame,
                body: None,
                protection,
            }
            .check()
            .ok()?;
            <Family<Id> as TxBufferPool<'id, BufferClass<512>>>::prepare::<AccessPoint, Caps>(
                source,
            )
            .ok()?
        };
        let attempt = Attempt {
            context: Tag(93, None),
            ticket,
            source: prepared,
            access_category: category,
            protection: Protection::None,
            power: SupportedPower::new(TxPower::Calibrated).expect("power"),
            coex_operation: TxOperation::Data,
        }
        .admit();
        let admitted = fixture.port.admit_access_point(attempt);
        if let (Some(of), Some(key)) = (key, pairwise) {
            self.tokens(of).pairwise_key = Some(key);
        }
        Some(admitted)
    }

    /// A station data MPDU to the BSS on `category`, under the station's
    /// installed key when `keyed`.
    fn station_mpdu(
        &mut self,
        category: WmmAccessCategory,
        keyed: bool,
    ) -> Option<Admitted<'id, Id>> {
        self.station_number += 1;
        let number = self.station_number;
        let installed = if keyed {
            Some(self.station_key.take()?)
        } else {
            None
        };
        let fixture = self.fixture.as_mut()?;
        let buffer = fixture.buffers.pop()?;
        let ticket = fixture.tickets.pop()?;
        let rate = fixture.port.rate(PhyRate::Legacy(LegacyRate::Ofdm6M))?;
        let control = if keyed { 0x4108 } else { 0x0108 };
        let frame = ClassFrame::new(TxFrame::Individual {
            frame: SizedFrame::new(
                AckHeader::new(header(control, BSSID, STATION, BSSID)).expect("ACK"),
                MpduLength::new(64).expect("length"),
            )
            .expect("frame"),
            plan: RetryPlan::new(RetryStage {
                rate,
                transmissions: core::num::NonZeroU8::MIN,
                protection: Protection::None,
            }),
        })
        .ok()?;
        let prepared = {
            let protection = match &installed {
                Some((_, key)) => TxProtection::Ccmp(NumberedKey {
                    key,
                    packet_number: oer_protocols_ieee80211_wire::ccmp::CcmpPacketNumber::new(
                        number,
                    )
                    .expect("PN"),
                }),
                None => TxProtection::Open,
            };
            let source = TxSource::Fresh {
                buffer,
                frame,
                body: None,
                protection,
            }
            .check()
            .ok()?;
            <Family<Id> as TxBufferPool<'id, BufferClass<512>>>::prepare::<Station, Caps>(source)
                .ok()?
        };
        let attempt = Attempt {
            context: Tag(94, None),
            ticket,
            source: prepared,
            access_category: category,
            protection: Protection::None,
            power: SupportedPower::new(TxPower::Calibrated).expect("power"),
            coex_operation: TxOperation::Data,
        }
        .admit();
        let admitted = fixture.port.admit(attempt);
        if let Some(installed) = installed {
            self.station_key = Some(installed);
        }
        Some(admitted)
    }

    /// A Beacon under the beacon permission `of`, fresh until `deadline`;
    /// none while the S31 declares no Timestamp rule.
    fn beacon_mpdu(&mut self, of: Issued, deadline: Instant) -> Option<Admitted<'id, Id>> {
        let permission = self.tokens(of).beacon.take()?;
        let fixture = self.fixture.as_mut()?;
        let buffer = fixture.buffers.pop()?;
        let ticket = fixture.tickets.pop()?;
        let rate = fixture.port.rate(PhyRate::Legacy(LegacyRate::Ofdm6M))?;
        let beacon_header = HeaderSpec::new(
            FrameControl::new(0x0080).expect("control"),
            0,
            GroupAddress::new([0xff; 6]).expect("broadcast"),
            ACCESS_POINT,
            ACCESS_POINT,
            0,
            None,
            None,
            None,
        )
        .expect("header");
        let frame = ClassFrame::new(TxFrame::Group {
            frame: SizedFrame::new(beacon_header, MpduLength::new(64).expect("length"))
                .expect("frame"),
            rate,
        })
        .ok()?;
        let source = TxSource::Fresh {
            buffer,
            frame,
            body: None,
            protection: TxProtection::Open,
        }
        .check()
        .ok()?;
        let prepared = <Family<Id> as TxBufferPool<'id, BufferClass<512>>>::prepare::<
            AccessPoint,
            Caps,
        >(source)
        .ok()?;
        let sealed = BeaconAttempt {
            context: Tag(95, None),
            ticket,
            source: prepared,
            power: SupportedPower::new(TxPower::Calibrated).expect("power"),
            permission,
            stamp: BeaconStamp::new(BeaconTimestamp::Caller)?,
            deadline,
        }
        .admit();
        Some(fixture.port.admit_beacon(sealed))
    }

    /// Return a terminal's ticket and buffer to the fixture: how it ended.
    fn give_back(&mut self, completed: SingleCompleted<'id, Family<Id>>) -> (u8, CaseEnd) {
        let fixture = self.f();
        let parts = completed.into_parts(fixture.access.as_mut().expect("terminal access"));
        let end = match parts.outcome {
            TxOutcome::Ended(SingleEnd::Delivered { .. }) => CaseEnd::Delivered,
            TxOutcome::Withdrawn(cause) => CaseEnd::Withdrawn(cause),
            TxOutcome::Expired { .. } => CaseEnd::Expired,
            _ => CaseEnd::Other,
        };
        let ticket = parts.ticket.index();
        fixture.tickets.push(parts.ticket);
        let (buffer, _) = parts.payload.into_parts();
        let mut buffer = match buffer {
            AnyBuffer::Fresh(buffer) => buffer,
            AnyBuffer::Sent(sent) => {
                <Family<Id> as TxBufferPool<'id, BufferClass<512>>>::renew(sent).0
            }
        };
        buffer.renew_msdu().expect("renewed");
        fixture.buffers.push(buffer);
        (ticket, end)
    }
}

impl<'a, 'id, Id: radio::SlotIdentity + 'id> AccessPointHarness<'id> for ApFixture<'a, 'id, Id> {
    type Port = Port<'id, Id>;
    type Sink = Outbox<'id, Id>;
    type Ends = Vec<CaseEnded>;
    type Sent = Vec<CaseSent>;

    fn parts(&mut self) -> (&mut Self::Port, &mut Self::Sink) {
        let fixture = self.f();
        (&mut fixture.port, &mut fixture.outbox)
    }
    fn now(&self) -> Instant {
        self.fr().now()
    }
    fn advance(&mut self, by: oer_portable_time::Duration) {
        DataPortHarness::advance(self.f(), by);
    }
    fn waker(&self) -> Waker {
        DataPortHarness::waker(self.fr())
    }
    fn channel(&self) -> SupportedChannel<Caps> {
        DataPortHarness::channel(self.fr())
    }
    fn settle(&mut self) {}
    fn reinstall(&mut self) -> Result<(), CaseViolation> {
        let mut fixture = self.fixture.take().ok_or(CaseViolation::Fixture)?;
        fixture.stop();
        let mut fixture =
            reinstall(fixture, self.installations).map_err(|_| CaseViolation::Fixture)?;
        let resources = fixture.access_point.take().ok_or(CaseViolation::Fixture)?;
        self.fixture = Some(fixture);
        self.earlier = core::mem::replace(&mut self.current, ApTokens::new(resources));
        self.generation += 1;
        self.station_key = None;
        Ok(())
    }
    fn command_bound(&self) -> oer_portable_time::Duration {
        crate::port::COMMAND_BOUND
    }
    fn publication_bound(&self) -> oer_portable_time::Duration {
        crate::port::PUBLICATION_BOUND
    }
    fn hold(&mut self, category: WmmAccessCategory, hold: bool) {
        self.f().observed.borrow_mut().held_queues[queue_of(category)] = hold;
    }
    fn hold_group_commands(&mut self, hold: bool) {
        self.f().observed.borrow_mut().hold_cut = hold;
    }
    fn access_point_frame(
        &mut self,
        key: Option<Issued>,
        category: WmmAccessCategory,
    ) -> Option<Admitted<'id, Id>> {
        self.access_point_mpdu(key, category)
    }
    fn station_frame(
        &mut self,
        category: WmmAccessCategory,
        keyed: bool,
    ) -> Option<Admitted<'id, Id>> {
        self.station_mpdu(category, keyed)
    }
    fn beacon(&mut self, of: Issued, deadline: Instant) -> Option<Admitted<'id, Id>> {
        self.beacon_mpdu(of, deadline)
    }
    fn ticket(attempt: &Admitted<'id, Id>) -> u8 {
        attempt.operation.ticket().index()
    }
    fn take_ends(&mut self) -> Vec<CaseEnded> {
        let terminals = core::mem::take(&mut self.f().outbox.terminals);
        let mut ended = Vec::new();
        for terminal in terminals {
            match terminal {
                LowerMacTerminal::TxCompleted(TxCompleted::Single(completed)) => {
                    let (ticket, end) = self.give_back(completed);
                    ended.push(CaseEnded::Frame(ticket, end));
                }
                LowerMacTerminal::TxCompleted(TxCompleted::Beacon {
                    completed,
                    permission,
                }) => {
                    let tokens = self.of(permission.generation());
                    assert!(
                        tokens.beacon.replace(permission).is_none(),
                        "one beacon permission per installation"
                    );
                    let (ticket, end) = self.give_back(completed);
                    ended.push(CaseEnded::Beacon(ticket, end));
                }
                other => self.f().outbox.terminals.push(other),
            }
        }
        ended
    }
    fn holds_beacon(&self, of: Issued) -> bool {
        match of {
            Issued::Current => self.current.beacon.is_some(),
            Issued::Earlier => self.earlier.beacon.is_some(),
        }
    }
    fn station_configure(&mut self) -> Option<PortCommand<'id, Family<Id>, Caps>> {
        DataPortHarness::configure(self.f())
    }
    fn take_station_configured(&mut self) -> bool {
        let fixture = self.f();
        let at = fixture.outbox.terminals.iter().position(|terminal| {
            matches!(
                terminal,
                LowerMacTerminal::Configured(Configured::Station(_))
            )
        });
        match at.map(|at| fixture.outbox.terminals.remove(at)) {
            Some(LowerMacTerminal::Configured(Configured::Station(permission))) => {
                fixture.configure = Some(permission);
                true
            }
            _ => false,
        }
    }
    fn install_station_key(&mut self) -> bool {
        let fixture = self.f();
        let (association, _group, slot) = fixture.association();
        let (install, _) = fixture.commands.install_pairwise(
            &association,
            slot,
            IndividualAddress::new(BSSID).expect("peer"),
            group_key(7),
        );
        if !matches!(fixture.run(install), Ok(CommandProgress::Done)) {
            return false;
        }
        let at = fixture.outbox.terminals.iter().position(|terminal| {
            matches!(
                terminal,
                LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::Station(_))
            )
        });
        match at.map(|at| fixture.outbox.terminals.remove(at)) {
            Some(LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::Station(key))) => {
                self.station_key = Some((association, key));
                true
            }
            _ => false,
        }
    }
    fn configure(
        &mut self,
        of: Issued,
        receive: Receive,
        tsf: Option<TsfControl>,
    ) -> Option<ApCommand<'id, Id>> {
        let permission = self.tokens(of).configure.take()?;
        let receive = match receive {
            Receive::Off => AccessPointReceive::Off,
            Receive::Bss => AccessPointReceive::Bss {
                filter: SupportedFilter::new(ReceiveFilter::BSS_MEMBER).expect("filter"),
            },
        };
        Some(AccessPointCommand::Configure {
            permission,
            receive,
            tsf: tsf.map(|control| SupportedTsfControl::new(control).expect("the S31 TSF")),
        })
    }
    fn install_pairwise(
        &mut self,
        association: Issued,
        slot: Issued,
    ) -> Option<ApCommand<'id, Id>> {
        let slot = self.tokens(slot).pairwise.pop()?;
        let association = match association {
            Issued::Current => self.current.association.as_ref()?,
            Issued::Earlier => self.earlier.association.as_ref()?,
        };
        let (install, _) = self
            .fixture
            .as_mut()?
            .commands
            .install_access_point_pairwise(
                association,
                slot,
                IndividualAddress::new(PEER).expect("peer"),
                group_key(8),
            );
        Some(install)
    }
    fn install_group(
        &mut self,
        association: Issued,
        permission: Issued,
        slot: Issued,
    ) -> Option<ApCommand<'id, Id>> {
        let slot = self.tokens(slot).group.pop()?;
        let permission = self.tokens(permission).group_permission.take()?;
        let association = match association {
            Issued::Current => self.current.association.as_ref()?,
            Issued::Earlier => self.earlier.association.as_ref()?,
        };
        Some(self.fixture.as_mut()?.commands.install_access_point_group(
            association,
            permission,
            slot,
            key_id(self.group_key_id),
            group_key(9),
        ))
    }
    fn remove_pairwise(&mut self, association: Issued, key: Issued) -> Option<ApCommand<'id, Id>> {
        let key = self.tokens(key).pairwise_key.take()?;
        let association = self.tokens(association).association.take()?;
        Some(AccessPointCommand::RemovePairwise { association, key })
    }
    fn remove_group(&mut self, permission: Issued, key: Issued) -> Option<ApCommand<'id, Id>> {
        let key = self.tokens(key).group_key.take()?;
        let permission = self.tokens(permission).group_permission.take()?;
        Some(AccessPointCommand::RemoveGroup { permission, key })
    }
    fn take_terminal(&mut self) -> Option<CaseTerminal> {
        let fixture = self.f();
        let position = fixture.outbox.terminals.iter().position(|terminal| {
            matches!(
                terminal,
                LowerMacTerminal::Configured(Configured::AccessPoint(_))
                    | LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::AccessPoint(_))
                    | LowerMacTerminal::GroupKeyInstalled(InstalledGroup::AccessPoint { .. })
                    | LowerMacTerminal::KeyRemoved(Removed::AccessPoint(_))
            )
        })?;
        match fixture.outbox.terminals.remove(position) {
            LowerMacTerminal::Configured(Configured::AccessPoint(permission)) => {
                let generation = permission.generation();
                self.of(generation).configure = Some(permission);
                Some(CaseTerminal::Configured(generation))
            }
            LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::AccessPoint(key)) => {
                let generation = key.generation();
                self.of(generation).pairwise_key = Some(key);
                Some(CaseTerminal::PairwiseInstalled(generation))
            }
            LowerMacTerminal::GroupKeyInstalled(InstalledGroup::AccessPoint {
                key,
                permission,
            }) => {
                let generation = permission.generation();
                let tokens = self.of(generation);
                tokens.group_key = Some(key);
                tokens.group_permission = Some(permission);
                Some(CaseTerminal::GroupInstalled(generation))
            }
            LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Pairwise {
                slot,
                association,
            })) => {
                let generation = association.generation();
                let tokens = self.of(generation);
                tokens.pairwise.push(slot);
                tokens.association = Some(association);
                Some(CaseTerminal::PairwiseRemoved(generation))
            }
            LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Group {
                slot,
                permission,
            })) => {
                let generation = permission.generation();
                let tokens = self.of(generation);
                tokens.group.push(slot);
                tokens.group_permission = Some(permission);
                Some(CaseTerminal::GroupRemoved(generation))
            }
            _ => None,
        }
    }
    fn restore(&mut self, command: ApCommand<'id, Id>) {
        match command {
            AccessPointCommand::Configure { permission, .. } => {
                let generation = permission.generation();
                self.of(generation).configure = Some(permission);
            }
            AccessPointCommand::RemovePairwise { association, key } => {
                let generation = association.generation();
                self.of(generation).association = Some(association);
                let generation = key.generation();
                self.of(generation).pairwise_key = Some(key);
            }
            AccessPointCommand::RemoveGroup { permission, key } => {
                let generation = permission.generation();
                self.of(generation).group_permission = Some(permission);
                let generation = key.generation();
                self.of(generation).group_key = Some(key);
            }
            // A refused installation's slot and material go with it: the
            // cases do not install from them again.
            AccessPointCommand::InstallPairwise(_) | AccessPointCommand::InstallGroup(_) => {}
        }
    }
    fn sent(&self) -> Vec<CaseSent> {
        self.published
            .borrow()
            .iter()
            .map(|program| CaseSent {
                role: match program.interface() {
                    MacInterface::AccessPoint => VifRole::AccessPoint,
                    _ => VifRole::Station,
                },
                protected: program.hardware_key_selector() != 0,
            })
            .collect()
    }
    fn interface(&self) -> Interface {
        let observed = *self.interface.borrow();
        Interface {
            receives: observed.policy.is_some(),
            tsf_running: observed.tsf_running,
        }
    }
    fn key_writes(&self) -> usize {
        self.fr().observed.borrow().key_writes
    }
    fn idle(&self) -> bool {
        let fixture = self.fr();
        fixture.tickets.len() == TX_TICKETS && fixture.buffers.len() == TX_BUFFERS
    }
}

/// Run `case` on a fresh installed fixture whose register model the
/// harness observes.
fn run_case(case: ap_cases::Case) -> Result<(), CaseViolation> {
    run_check(Check::Case(case))
}

/// A check of the suite: one of its cases, or a check for backends whose
/// command call never ends a publication.
#[derive(Clone, Copy, Debug)]
enum Check {
    Case(ap_cases::Case),
    ExpiredConfigure,
    ExpiredPairwiseRemoval,
    UnsupportedGroupKeyId,
    UnpublishableDuringStationConfigure,
    ReturnedAccessPointCommand,
}

fn run_check(check: Check) -> Result<(), CaseViolation> {
    block_on(radio::with_local_slot(async move |claim| {
        let hardware = crate::lower_mac::tests::Hardware::default();
        let interface = hardware.access_point.clone();
        let published = hardware.published.clone();
        let (parts, observed, timer) = installed_with_hardware(Vec::new(), hardware);
        let mut installations = Installations::<_, InstallationPools>::claim(claim);
        let minted: Minted<_, InstallationPools> = installations.first().expect("first mint");
        let driver = Box::leak(Box::new(PortDriver::vacant()));
        let installation = Port::install(parts, minted, driver).ok().expect("install");
        let mut fixture = Fixture::new(installation, observed, timer);
        let resources = fixture
            .access_point
            .take()
            .expect("the access point's tokens");
        let mut harness = ApFixture {
            fixture: Some(fixture),
            installations: &mut installations,
            current: ApTokens::new(resources),
            earlier: ApTokens::none(),
            generation: 0,
            station_key: None,
            station_number: 0,
            interface,
            published,
            group_key_id: 1,
        };
        match check {
            Check::Case(case) => ap_cases::check(&mut harness, case),
            Check::ExpiredConfigure => ap_cases::expired_configure(&mut harness),
            Check::ExpiredPairwiseRemoval => ap_cases::expired_pairwise_removal(&mut harness),
            Check::UnsupportedGroupKeyId => unsupported_group_key_id(&mut harness),
            Check::UnpublishableDuringStationConfigure => {
                unpublishable_during_station_configure(&mut harness)
            }
            Check::ReturnedAccessPointCommand => returned_access_point_command(&mut harness),
        }
    }))
}

#[test]
fn an_expired_configuration_and_pairwise_removal_undo_their_barriers_on_the_s31_port() {
    for check in [Check::ExpiredConfigure, Check::ExpiredPairwiseRemoval] {
        assert_eq!(run_check(check), Ok(()), "{check:?}");
    }
}

/// The cases that submit a beacon: the S31 declares no Timestamp rule, so
/// no beacon stamp exists and the fixture builds no beacon.
const BEACON_CASES: [ap_cases::Case; 6] = [
    ap_cases::Case::BeaconPermission,
    ap_cases::Case::StoppedBeacon,
    ap_cases::Case::BeaconExpires,
    ap_cases::Case::StaleBeacon,
    ap_cases::Case::StopsAlone,
    ap_cases::Case::SharedStop,
];

#[test]
fn the_s31_declares_no_beacon_timestamp_rule() {
    for source in [BeaconTimestamp::Caller, BeaconTimestamp::Transmitter] {
        assert!(BeaconStamp::<Caps>::new(source).is_none(), "{source:?}");
    }
}

#[test]
fn every_access_point_case_without_a_beacon_passes_on_the_s31_port() {
    let failed: Vec<_> = ap_cases::Case::ALL
        .into_iter()
        .map(|case| (case, run_case(case)))
        .filter(|(case, result)| {
            if BEACON_CASES.contains(case) {
                *result != Err(ap_cases::Violation::Fixture)
            } else {
                result.is_err()
            }
        })
        .collect();
    assert!(failed.is_empty(), "{failed:?}");
}

/// The AP's group installation under key id 0, which the S31 AP key layout
/// has no entry for: refused with the command whole (its slot and
/// permission in it), before any key write or receive cut; the port runs
/// on and stops.
fn unsupported_group_key_id<Id: radio::SlotIdentity>(
    harness: &mut ApFixture<'_, '_, Id>,
) -> Result<(), CaseViolation> {
    ap_cases::start(harness)?;
    let writes = harness.key_writes();
    harness.group_key_id = 0;
    let install = harness.install_group(Issued::Current, Issued::Current, Issued::Current);
    let refused = match ap_cases::run(harness, install)? {
        AccessPointProgress::Refused(command, AccessPointRefusal::UnsupportedGroupKeyId) => command,
        _ => return Err(CaseViolation::Refusal),
    };
    // The command comes back whole, its slot and permission in it.
    if !matches!(refused, AccessPointCommand::InstallGroup(_)) {
        return Err(CaseViolation::Owners);
    }
    if harness.key_writes() != writes || harness.fr().port.fault().is_some() {
        return Err(CaseViolation::Fault);
    }
    // No receive cut began: an access-point frame goes out at once.
    let frame = harness.access_point_frame(None, ap_cases::ACCESS_POINT_QUEUE);
    let frame = ap_cases::submit(harness, frame)?;
    ap_cases::service(harness)?;
    if harness.take_ends() != [CaseEnded::Frame(frame, CaseEnd::Delivered)] {
        return Err(CaseViolation::Ends);
    }
    drop(refused);
    ap_cases::stop(harness)
}

/// An access-point frame whose values the transmit path refuses while the
/// station's configuration waits at its barrier: the frame ends withdrawn
/// `Unpublishable` with its ticket and buffer, the port records no fault,
/// the configuration completes and the stop returns every owner.
fn unpublishable_during_station_configure<Id: radio::SlotIdentity>(
    harness: &mut ApFixture<'_, '_, Id>,
) -> Result<(), CaseViolation> {
    ap_cases::start(harness)?;
    // A station frame on the air holds the station's configuration.
    harness.hold(ap_cases::STATION_QUEUE, true);
    let first = harness.station_frame(ap_cases::STATION_QUEUE, false);
    let first = ap_cases::submit(harness, first)?;
    ap_cases::service(harness)?;
    let configure = harness.station_configure();
    let mut held = ap_cases::station_command(harness, configure)?;
    if harness.take_station_configured() {
        return Err(CaseViolation::Barrier);
    }
    let admissions = harness.f().port.admissions_for_test();
    harness.f().port.refuse_values_for_test();
    let frame = harness.access_point_frame(None, ap_cases::ACCESS_POINT_QUEUE);
    let frame = ap_cases::submit(harness, frame)?;
    if harness.take_ends()
        != [CaseEnded::Frame(
            frame,
            CaseEnd::Withdrawn(WithdrawCause::Unpublishable),
        )]
    {
        return Err(CaseViolation::Ends);
    }
    if harness.fr().port.fault().is_some() {
        return Err(CaseViolation::Fault);
    }
    if harness.f().port.admissions_for_test() != admissions {
        return Err(CaseViolation::Owners);
    }
    // The next access-point frame publishes beside the barrier.
    let next = harness.access_point_frame(None, ap_cases::ACCESS_POINT_QUEUE);
    let next = ap_cases::submit(harness, next)?;
    ap_cases::service(harness)?;
    if harness.take_ends() != [CaseEnded::Frame(next, CaseEnd::Delivered)] {
        return Err(CaseViolation::Ends);
    }
    harness.hold(ap_cases::STATION_QUEUE, false);
    let (mut first_ended, mut configured) = (false, false);
    for _ in 0..16 {
        ap_cases::service(harness)?;
        if held.is_some() {
            held = ap_cases::station_command(harness, held)?;
        }
        for end in harness.take_ends() {
            match end {
                CaseEnded::Frame(ticket, CaseEnd::Delivered) if ticket == first => {
                    first_ended = true;
                }
                _ => return Err(CaseViolation::Ends),
            }
        }
        configured |= held.is_none() && harness.take_station_configured();
        if first_ended && configured {
            break;
        }
    }
    if !(first_ended && configured) {
        return Err(CaseViolation::Barrier);
    }
    ap_cases::stop(harness)?;
    if !harness.idle() {
        return Err(CaseViolation::Owners);
    }
    Ok(())
}

#[test]
fn an_ap_group_key_id_without_a_hardware_entry_is_refused_with_its_owners_and_no_fault() {
    assert_eq!(run_check(Check::UnsupportedGroupKeyId), Ok(()));
}

#[test]
fn an_unpublishable_ap_frame_during_a_station_configuration_ends_withdrawn_with_its_owners() {
    assert_eq!(
        run_check(Check::UnpublishableDuringStationConfigure),
        Ok(())
    );
}

/// An access-point command offered to a port whose fault leaves every
/// owner whole comes back beside the fault: the port keeps no command, so
/// it recovers for a stop, and the stop returns every owner.
fn returned_access_point_command<Id: radio::SlotIdentity>(
    harness: &mut ApFixture<'_, '_, Id>,
) -> Result<(), CaseViolation> {
    ap_cases::start(harness)?;
    let fault = Fault::Unpublished(crate::port::Unpublished::Key);
    harness.f().port.fault_for_test(fault);
    let configure = harness
        .configure(Issued::Current, Receive::Bss, None)
        .ok_or(CaseViolation::Fixture)?;
    let now = AccessPointHarness::now(harness);
    let (port, sink) = AccessPointHarness::parts(harness);
    let command = match port.access_point_command(configure, now, sink) {
        Err(CommandFailed::Returned {
            command,
            fault: found,
        }) if found == fault => command,
        _ => return Err(CaseViolation::Fault),
    };
    if !matches!(command, AccessPointCommand::Configure { .. }) {
        return Err(CaseViolation::Owners);
    }
    AccessPointHarness::restore(harness, command);
    if !harness.f().port.recover_for_stop() || harness.f().port.fault().is_some() {
        return Err(CaseViolation::Owners);
    }
    ap_cases::stop(harness)?;
    if !harness.idle() {
        return Err(CaseViolation::Owners);
    }
    Ok(())
}

#[test]
fn an_access_point_command_offered_to_a_faulted_port_comes_back_and_the_port_recovers() {
    assert_eq!(run_check(Check::ReturnedAccessPointCommand), Ok(()));
}
