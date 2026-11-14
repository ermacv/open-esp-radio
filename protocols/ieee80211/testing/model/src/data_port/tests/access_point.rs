//! The access-point role on the model's one port: the shared conformance
//! cases (`oer_protocols_ieee80211_testing_conformance::access_point`) over
//! the model, and the helpers the station's stale-token cases share.

use super::*;
use oer_protocols_ieee80211_contracts_port::{
    AccessPointCommand, AccessPointCommands, AccessPointPort, AccessPointReceive, AttemptKey,
    BeaconAttempt, BeaconPermission, BeaconStamp, BeaconTimestamp, Configured, GroupAddress,
    InstalledGroup, InstalledPairwise, NumberedKey, ReceiveFilter, Removed, RemovedKey, SingleEnd,
    SupportedFilter, SupportedTsfControl, TsfControl,
};
use oer_protocols_ieee80211_wire::ccmp::CcmpPacketNumber;

type ApCommand<'s> = AccessPointCommand<'s, Types<'s>, ModelCaps>;
type ApPairwise<'s> = ModelPairwiseKey<'s, B<'s>, AccessPoint>;
pub(super) type StaPairwise<'s> = ModelPairwiseKey<'s, B<'s>, Station>;
pub(super) type Beacon<'s> = BeaconPermission<'s, ModelBeaconPath<B<'s>>>;
pub(super) type Admitted<'s> = ModelAdmitted<LocalId<'s>, ModelCaps, ()>;

/// How a single MPDU's terminal ended, as these cases tell them apart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum End {
    Delivered,
    Withdrawn(WithdrawCause),
    Other,
}

/// One transmission terminal the sink held, its ticket and buffer back with
/// the harness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Ended {
    /// A single MPDU of the ticket.
    Frame(u8, End),
    /// The beacon of the ticket; its permission is back too.
    Beacon(u8, End),
}

/// Take the transmission terminals out of the sink, returning tickets and
/// buffers to the harness and a beacon's permission to `beacon`. Every
/// other terminal stays in the sink, in its order.
pub(super) fn settle<'s>(harness: &mut Harness<'s>, beacon: &mut Option<Beacon<'s>>) -> Vec<Ended> {
    let terminals = core::mem::take(&mut harness.sink.terminals);
    let mut ended = Vec::new();
    for terminal in terminals {
        match terminal {
            LowerMacTerminal::TxCompleted(TxCompleted::Single(completed)) => {
                let (ticket, end) = give_back(harness, completed);
                ended.push(Ended::Frame(ticket, end));
            }
            LowerMacTerminal::TxCompleted(TxCompleted::Beacon {
                completed,
                permission,
            }) => {
                assert!(
                    beacon.replace(permission).is_none(),
                    "one beacon permission per installation"
                );
                let (ticket, end) = give_back(harness, completed);
                ended.push(Ended::Beacon(ticket, end));
            }
            other => harness.sink.terminals.push(other),
        }
    }
    ended
}

pub(super) fn give_back<'s>(
    harness: &mut Harness<'s>,
    completed: oer_protocols_ieee80211_contracts_port::SingleCompleted<'s, Types<'s>>,
) -> (u8, End) {
    let TerminalParts {
        ticket,
        payload,
        outcome,
        ..
    } = completed.into_parts(&mut harness.terminal_access);
    let index = ticket.index();
    harness.tickets.push(ticket);
    let (buffer, _) = payload.into_parts();
    let mut buffer = match buffer {
        ModelAnyBuffer::Small(buffer) => buffer,
        ModelAnyBuffer::SmallSent(sent) => {
            <Types<'s> as TxBufferPool<'s, ModelSmall>>::renew(sent).0
        }
        _ => panic!("these cases submit small buffers"),
    };
    buffer.renew_msdu().unwrap();
    harness.small.push(buffer);
    let end = match outcome {
        TxOutcome::Ended(SingleEnd::Delivered { .. }) => End::Delivered,
        TxOutcome::Withdrawn(cause) => End::Withdrawn(cause),
        _ => End::Other,
    };
    (index, end)
}

/// The access point's unicast data frame to the peer, under `key` when
/// given, on `category`.
fn access_point_frame<'s>(
    harness: &mut Harness<'s>,
    key: Option<(&ApPairwise<'s>, u64)>,
    category: WmmAccessCategory,
) -> Admitted<'s> {
    let ticket = harness.tickets.pop().unwrap();
    let mut buffer = harness.small.pop().unwrap();
    buffer.bytes_mut().unwrap().fill(0);
    let rate = harness
        .port
        .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
        .unwrap();
    let header = HeaderSpec::new(
        FrameControl::new(if key.is_some() { 0x4208 } else { 0x0208 }).unwrap(),
        0,
        IndividualAddress::new(PEER).unwrap(),
        ACCESS_POINT,
        ACCESS_POINT,
        0,
        None,
        None,
        None,
    )
    .and_then(AckHeader::new)
    .unwrap();
    let frame = TxFrame::Individual {
        frame: SizedFrame::new(header, MpduLength::new(40).unwrap()).unwrap(),
        plan: RetryPlan::new(RetryStage {
            rate,
            transmissions: NonZeroU8::new(2).unwrap(),
            protection: Protection::None,
        }),
    };
    let frame = ClassFrame::<ModelSmall, _>::new(frame).ok().unwrap();
    let source = prepare_model(
        TxSource::Fresh {
            buffer,
            frame,
            body: None,
            protection: match key {
                Some((key, number)) => TxProtection::Ccmp(NumberedKey {
                    key: AttemptKey::Pairwise(key),
                    packet_number: CcmpPacketNumber::new(number).unwrap(),
                }),
                None => TxProtection::Open,
            },
        }
        .check()
        .ok()
        .unwrap(),
    )
    .ok()
    .unwrap();
    let attempt = Attempt {
        context: (),
        ticket,
        source,
        access_category: category,
        protection: Protection::None,
        power: SupportedPower::new(TxPower::Calibrated).unwrap(),
        coex_operation: TxOperation::Data,
    }
    .admit();
    harness.port.admit_access_point(attempt)
}

/// The access point's broadcast Beacon under `permission`, fresh until
/// `deadline`.
fn beacon<'s>(
    harness: &mut Harness<'s>,
    permission: Beacon<'s>,
    deadline: Instant,
) -> Admitted<'s> {
    let ticket = harness.tickets.pop().unwrap();
    let mut buffer = harness.small.pop().unwrap();
    buffer.bytes_mut().unwrap().fill(0);
    let rate = harness
        .port
        .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
        .unwrap();
    let header = HeaderSpec::new(
        FrameControl::new(0x0080).unwrap(),
        0,
        GroupAddress::new([0xff; 6]).unwrap(),
        ACCESS_POINT,
        ACCESS_POINT,
        0,
        None,
        None,
        None,
    )
    .unwrap();
    let frame = TxFrame::Group {
        frame: SizedFrame::new(header, MpduLength::new(40).unwrap()).unwrap(),
        rate,
    };
    let frame = ClassFrame::<ModelSmall, _>::new(frame).ok().unwrap();
    let source = prepare_model(
        TxSource::Fresh {
            buffer,
            frame,
            body: None,
            protection: TxProtection::Open,
        }
        .check()
        .ok()
        .unwrap(),
    )
    .ok()
    .unwrap();
    let sealed = BeaconAttempt {
        context: (),
        ticket,
        source,
        power: SupportedPower::new(TxPower::Calibrated).unwrap(),
        permission,
        stamp: BeaconStamp::new(BeaconTimestamp::Transmitter).unwrap(),
        deadline,
    }
    .admit();
    harness.port.admit_beacon(sealed)
}

pub(super) fn submit<'s>(harness: &mut Harness<'s>, attempt: Admitted<'s>) -> u8 {
    let ticket = attempt.operation.ticket().index();
    let now = harness.clock.now();
    harness
        .port
        .submit(attempt, now, &mut harness.sink)
        .unwrap()
        .ok()
        .unwrap();
    ticket
}

fn ap_configure<'s>(
    permission: ConfigurePermission<
        's,
        AccessPoint,
        oer_protocols_radio_port::Child<ModelPool<LocalId<'s>, 9>, 2>,
    >,
    receive: AccessPointReceive<ModelCaps>,
    tsf: Option<TsfControl>,
) -> ApCommand<'s> {
    AccessPointCommand::Configure {
        permission,
        receive,
        tsf: tsf.map(|control| SupportedTsfControl::new(control).unwrap()),
    }
}

fn bss() -> AccessPointReceive<ModelCaps> {
    AccessPointReceive::Bss {
        filter: SupportedFilter::new(ReceiveFilter::BSS_MEMBER).unwrap(),
    }
}

/// Install the station's pairwise key and take it from its terminal.
pub(super) fn station_key<'s>(
    harness: &mut Harness<'s>,
) -> (ModelAssociation<'s, B<'s>, Station>, StaPairwise<'s>) {
    let (start, _, _, [slot, _]) = harness.group.take().unwrap();
    let association = ModelKeys::<B<'s>, Station>::begin_association(
        start,
        oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
    );
    let (install, _) = harness.commands.install_pairwise(
        &association,
        slot,
        IndividualAddress::new(PEER).unwrap(),
        Ccmp128Key::new(&[5; 16]),
    );
    assert!(matches!(harness.command(install), CommandProgress::Done));
    let Some(LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::Station(key))) =
        harness.sink.terminals.pop()
    else {
        panic!("the station key's terminal");
    };
    (association, key)
}

/// An unprotected station data frame on `category`.
fn station_frame_on<'s>(harness: &mut Harness<'s>, category: WmmAccessCategory) -> Admitted<'s> {
    let ticket = harness.tickets.pop().unwrap();
    let mut buffer = harness.small.pop().unwrap();
    buffer.bytes_mut().unwrap().fill(0);
    let rate = harness
        .port
        .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
        .unwrap();
    let header = HeaderSpec::new(
        FrameControl::new(0x0108).unwrap(),
        0,
        IndividualAddress::new(PEER).unwrap(),
        STATION,
        PEER,
        0,
        None,
        None,
        None,
    )
    .and_then(AckHeader::new)
    .unwrap();
    let frame = TxFrame::Individual {
        frame: SizedFrame::new(header, MpduLength::new(40).unwrap()).unwrap(),
        plan: RetryPlan::new(RetryStage {
            rate,
            transmissions: NonZeroU8::new(2).unwrap(),
            protection: Protection::None,
        }),
    };
    let frame = ClassFrame::<ModelSmall, _>::new(frame).ok().unwrap();
    let source = prepare_model(
        TxSource::Fresh {
            buffer,
            frame,
            body: None,
            protection: TxProtection::Open,
        }
        .check()
        .ok()
        .unwrap(),
    )
    .ok()
    .unwrap();
    let attempt = Attempt {
        context: (),
        ticket,
        source,
        access_category: category,
        protection: Protection::None,
        power: SupportedPower::new(TxPower::Calibrated).unwrap(),
        coex_operation: TxOperation::Data,
    }
    .admit();
    harness.port.admit(attempt)
}

/// A station data frame under `key`, on the station's queue.
pub(super) fn station_frame<'s>(
    harness: &mut Harness<'s>,
    key: &StaPairwise<'s>,
    number: u64,
) -> Admitted<'s> {
    let ticket = harness.tickets.pop().unwrap();
    let buffer = harness.small.pop().unwrap();
    harness.admitted_under_number(ticket, buffer, key, number)
}

/// Stop the started port of `harness` and reinstall it: the next
/// installation's port and resources, with the earlier installation's
/// tokens still with the caller.
pub(super) fn reinstall<'s>(mut harness: Harness<'s>) -> (Harness<'s>, ModelResources<'s, B<'s>>) {
    let waker = harness.waker();
    let now = harness.clock.now();
    assert!(matches!(
        harness
            .port
            .poll_stop(&mut Context::from_waker(&waker), now),
        Poll::Ready(Ok(()))
    ));
    let Harness {
        clock,
        air,
        port,
        mut installations,
        commands: _,
        sink,
        tickets,
        small,
        terminal_access,
        configure,
        gate,
        group,
        arenas,
        counter,
        access_point,
        station,
    } = harness;
    let Ok(ModelInstall {
        port,
        commands,
        resources,
    }) = port.reinstall(&mut installations)
    else {
        panic!("a retired port reinstalls");
    };
    assert_eq!(port.generation(), 1);
    (
        Harness {
            clock,
            air,
            installations,
            port,
            commands,
            sink,
            tickets,
            small,
            terminal_access,
            configure,
            gate,
            group,
            arenas,
            counter,
            access_point,
            station,
        },
        resources,
    )
}

use oer_protocols_ieee80211_testing_conformance::access_point::{
    self as ap_cases, AccessPointHarness, End as CaseEnd, Ended as CaseEnded, Interface, Issued,
    Receive, Sent as CaseSent, Terminal as CaseTerminal, Violation as CaseViolation,
};

type ApConfigure<'s> = ConfigurePermission<
    's,
    AccessPoint,
    oer_protocols_radio_port::Child<ModelPool<LocalId<'s>, 9>, 2>,
>;
type ApStart<'s> = oer_protocols_ieee80211_contracts_port::association::AssociationStart<
    's,
    AccessPoint,
    MODEL_KEY_SLOTS,
    MODEL_KEY_SLOTS,
    ModelPool<LocalId<'s>, 6>,
>;
type ApAssociation<'s> = ModelAssociation<'s, B<'s>, AccessPoint>;
type ApPairwiseSlot<'s> =
    PairwiseKeySlot<'s, AccessPoint, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, ModelPool<LocalId<'s>, 6>>;
type ApGroupSlot<'s> =
    GroupKeySlot<'s, AccessPoint, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, ModelPool<LocalId<'s>, 6>>;
type ApGroupPermission<'s> = oer_protocols_ieee80211_contracts_port::IssuedSlot<
    's,
    oer_protocols_radio_port::Slots<ModelGroupPermission<AccessPoint>, 1>,
    oer_protocols_radio_port::Child<ModelPool<LocalId<'s>, 9>, 0>,
>;
type ApGroupKey<'s> = ModelGroupKey<'s, B<'s>, AccessPoint>;

/// The access point's tokens of one installation, as the conformance
/// harness holds them between calls.
struct ApTokens<'s> {
    configure: Option<ApConfigure<'s>>,
    start: Option<ApStart<'s>>,
    association: Option<ApAssociation<'s>>,
    pairwise: Vec<ApPairwiseSlot<'s>>,
    group: Vec<ApGroupSlot<'s>>,
    group_permission: Option<ApGroupPermission<'s>>,
    beacon: Option<Beacon<'s>>,
    pairwise_key: Option<ApPairwise<'s>>,
    group_key: Option<ApGroupKey<'s>>,
    number: u64,
}

impl<'s> ApTokens<'s> {
    fn new(permissions: ModelAccessPointPermissions<'s, B<'s>>) -> Self {
        Self {
            configure: Some(permissions.commands.configure),
            start: Some(permissions.association),
            association: None,
            pairwise: permissions.pairwise.into_iter().collect(),
            group: permissions.group.into_iter().collect(),
            group_permission: Some(permissions.group_permission),
            beacon: Some(permissions.beacon),
            pairwise_key: None,
            group_key: None,
            number: 0,
        }
    }

    fn none() -> Self {
        Self {
            configure: None,
            start: None,
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

    /// The association, begun on first use.
    fn association(&mut self) -> Option<&ApAssociation<'s>> {
        if self.association.is_none() {
            let start = self.start.take()?;
            self.association = Some(ModelKeys::<B<'s>, AccessPoint>::begin_association(
                start,
                oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
            ));
        }
        self.association.as_ref()
    }
}

/// The model's port as the access-point conformance cases call it.
struct ApHarness<'s> {
    harness: Option<Harness<'s>>,
    current: ApTokens<'s>,
    earlier: ApTokens<'s>,
    station_key: Option<(ModelAssociation<'s, B<'s>, Station>, StaPairwise<'s>)>,
    station_number: u64,
}

impl<'s> ApHarness<'s> {
    fn new(mut harness: Harness<'s>) -> Self {
        let permissions = harness.access_point.take().unwrap();
        Self {
            harness: Some(harness),
            current: ApTokens::new(permissions),
            earlier: ApTokens::none(),
            station_key: None,
            station_number: 0,
        }
    }

    fn h(&mut self) -> &mut Harness<'s> {
        self.harness.as_mut().unwrap()
    }

    fn hr(&self) -> &Harness<'s> {
        self.harness.as_ref().unwrap()
    }

    fn tokens(&mut self, of: Issued) -> &mut ApTokens<'s> {
        match of {
            Issued::Current => &mut self.current,
            Issued::Earlier => &mut self.earlier,
        }
    }

    /// The tokens of the installation `generation`.
    fn of(&mut self, generation: u32) -> &mut ApTokens<'s> {
        if generation == self.hr().port.generation() {
            &mut self.current
        } else {
            &mut self.earlier
        }
    }
}

/// How a terminal's outcome reads in the conformance cases.
fn case_end(outcome: &ModelSingleOutcome) -> CaseEnd {
    match outcome {
        TxOutcome::Ended(SingleEnd::Delivered { .. }) => CaseEnd::Delivered,
        TxOutcome::Withdrawn(cause) => CaseEnd::Withdrawn(*cause),
        TxOutcome::Expired { .. } => CaseEnd::Expired,
        _ => CaseEnd::Other,
    }
}

impl<'s> ApHarness<'s> {
    fn give_back(
        &mut self,
        completed: oer_protocols_ieee80211_contracts_port::SingleCompleted<'s, Types<'s>>,
    ) -> (u8, CaseEnd) {
        let harness = self.h();
        let TerminalParts {
            ticket,
            payload,
            outcome,
            ..
        } = completed.into_parts(&mut harness.terminal_access);
        let end = case_end(&outcome);
        let index = ticket.index();
        harness.tickets.push(ticket);
        let (buffer, _) = payload.into_parts();
        let mut buffer = match buffer {
            ModelAnyBuffer::Small(buffer) => buffer,
            ModelAnyBuffer::SmallSent(sent) => {
                <Types<'s> as TxBufferPool<'s, ModelSmall>>::renew(sent).0
            }
            _ => panic!("these cases submit small buffers"),
        };
        buffer.renew_msdu().unwrap();
        harness.small.push(buffer);
        (index, end)
    }
}

impl<'s> AccessPointHarness<'s> for ApHarness<'s> {
    type Port = Port<'s>;
    type Sink = Sink<'s>;
    type Ends = Vec<CaseEnded>;
    type Sent = Vec<CaseSent>;

    fn parts(&mut self) -> (&mut Port<'s>, &mut Sink<'s>) {
        let harness = self.h();
        (&mut harness.port, &mut harness.sink)
    }
    fn now(&self) -> Instant {
        self.hr().clock.now()
    }
    fn advance(&mut self, by: oer_portable_time::Duration) {
        DataPortHarness::advance(self.h(), by);
    }
    fn waker(&self) -> Waker {
        DataPortHarness::waker(self.hr())
    }
    fn channel(&self) -> SupportedChannel<ModelCaps> {
        DataPortHarness::channel(self.hr())
    }
    fn settle(&mut self) {}
    fn reinstall(&mut self) -> Result<(), CaseViolation> {
        let harness = self.harness.take().ok_or(CaseViolation::Fixture)?;
        let (harness, resources) = reinstall(harness);
        self.harness = Some(harness);
        self.earlier = core::mem::replace(&mut self.current, ApTokens::new(resources.access_point));
        Ok(())
    }
    fn command_bound(&self) -> oer_portable_time::Duration {
        MODEL_HELD_COMMAND_BOUND
    }
    fn publication_bound(&self) -> oer_portable_time::Duration {
        MODEL_PUBLICATION_BOUND
    }
    fn hold(&mut self, category: WmmAccessCategory, hold: bool) {
        self.h().air.hold_completion(category, hold);
    }
    fn hold_group_commands(&mut self, hold: bool) {
        self.h().air.hold_group_commands(hold);
    }
    fn access_point_frame(
        &mut self,
        key: Option<Issued>,
        category: WmmAccessCategory,
    ) -> Option<Admitted<'s>> {
        match key {
            None => Some(access_point_frame(self.h(), None, category)),
            Some(of) => {
                let tokens = self.tokens(of);
                tokens.number += 1;
                let number = tokens.number;
                let key = tokens.pairwise_key.take()?;
                let attempt = access_point_frame(self.h(), Some((&key, number)), category);
                self.tokens(of).pairwise_key = Some(key);
                Some(attempt)
            }
        }
    }
    fn station_frame(&mut self, category: WmmAccessCategory, keyed: bool) -> Option<Admitted<'s>> {
        if !keyed {
            return Some(station_frame_on(self.h(), category));
        }
        if category != CATEGORY {
            return None;
        }
        self.station_number += 1;
        let number = self.station_number;
        let (association, key) = self.station_key.take()?;
        let attempt = station_frame(self.h(), &key, number);
        self.station_key = Some((association, key));
        Some(attempt)
    }
    fn beacon(&mut self, of: Issued, deadline: Instant) -> Option<Admitted<'s>> {
        let permission = self.tokens(of).beacon.take()?;
        Some(beacon(self.h(), permission, deadline))
    }
    fn ticket(attempt: &Admitted<'s>) -> u8 {
        attempt.operation.ticket().index()
    }
    fn take_ends(&mut self) -> Vec<CaseEnded> {
        let terminals = core::mem::take(&mut self.h().sink.terminals);
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
                other => self.h().sink.terminals.push(other),
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
    fn station_configure(&mut self) -> Option<PortCommand<'s, Types<'s>, ModelCaps>> {
        DataPortHarness::configure(self.h())
    }
    fn take_station_configured(&mut self) -> bool {
        DataPortHarness::take_configured(self.h())
    }
    fn install_station_key(&mut self) -> bool {
        let installed = station_key(self.h());
        self.station_key = Some(installed);
        true
    }
    fn configure(
        &mut self,
        of: Issued,
        receive: Receive,
        tsf: Option<TsfControl>,
    ) -> Option<ApCommand<'s>> {
        let permission = self.tokens(of).configure.take()?;
        let receive = match receive {
            Receive::Off => AccessPointReceive::Off,
            Receive::Bss => bss(),
        };
        Some(ap_configure(permission, receive, tsf))
    }
    fn install_pairwise(&mut self, association: Issued, slot: Issued) -> Option<ApCommand<'s>> {
        let slot = self.tokens(slot).pairwise.pop()?;
        self.tokens(association).association()?;
        let harness = self.harness.as_mut()?;
        let association = match association {
            Issued::Current => self.current.association.as_ref()?,
            Issued::Earlier => self.earlier.association.as_ref()?,
        };
        let (install, _) = harness.commands.install_access_point_pairwise(
            association,
            slot,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key::new(&[8; 16]),
        );
        Some(install)
    }
    fn install_group(
        &mut self,
        association: Issued,
        permission: Issued,
        slot: Issued,
    ) -> Option<ApCommand<'s>> {
        let slot = self.tokens(slot).group.pop()?;
        let permission = self.tokens(permission).group_permission.take()?;
        self.tokens(association).association()?;
        let harness = self.harness.as_mut()?;
        let association = match association {
            Issued::Current => self.current.association.as_ref()?,
            Issued::Earlier => self.earlier.association.as_ref()?,
        };
        Some(harness.commands.install_access_point_group(
            association,
            permission,
            slot,
            GroupKeyId::new(1).unwrap(),
            Ccmp128Key::new(&[9; 16]),
        ))
    }
    fn remove_pairwise(&mut self, association: Issued, key: Issued) -> Option<ApCommand<'s>> {
        let key = self.tokens(key).pairwise_key.take()?;
        let association = self.tokens(association).association.take()?;
        Some(AccessPointCommand::RemovePairwise { association, key })
    }
    fn remove_group(&mut self, permission: Issued, key: Issued) -> Option<ApCommand<'s>> {
        let key = self.tokens(key).group_key.take()?;
        let permission = self.tokens(permission).group_permission.take()?;
        Some(AccessPointCommand::RemoveGroup { permission, key })
    }
    fn take_terminal(&mut self) -> Option<CaseTerminal> {
        let position = self.h().sink.terminals.iter().position(|terminal| {
            matches!(
                terminal,
                LowerMacTerminal::Configured(Configured::AccessPoint(_))
                    | LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::AccessPoint(_))
                    | LowerMacTerminal::GroupKeyInstalled(InstalledGroup::AccessPoint { .. })
                    | LowerMacTerminal::KeyRemoved(Removed::AccessPoint(_))
            )
        })?;
        match self.h().sink.terminals.remove(position) {
            LowerMacTerminal::Configured(Configured::AccessPoint(permission)) => {
                let generation = permission.generation();
                self.of(generation).configure = Some(permission);
                Some(CaseTerminal::Configured(generation))
            }
            LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::AccessPoint(key)) => {
                let generation = key.slot.generation();
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
                let generation = association.owner.generation();
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
    fn restore(&mut self, command: ApCommand<'s>) {
        match command {
            AccessPointCommand::Configure { permission, .. } => {
                let generation = permission.generation();
                self.of(generation).configure = Some(permission);
            }
            AccessPointCommand::RemovePairwise { association, key } => {
                let generation = association.owner.generation();
                self.of(generation).association = Some(association);
                let generation = key.slot.generation();
                self.of(generation).pairwise_key = Some(key);
            }
            AccessPointCommand::RemoveGroup { permission, key } => {
                let generation = permission.generation();
                self.of(generation).group_permission = Some(permission);
                let generation = key.slot.generation();
                self.of(generation).group_key = Some(key);
            }
            // A refused installation's slot and material go with it: the
            // cases do not install from them again.
            AccessPointCommand::InstallPairwise(_) | AccessPointCommand::InstallGroup(_) => {}
        }
    }
    fn sent(&self) -> Vec<CaseSent> {
        self.hr()
            .air
            .sent()
            .iter()
            .map(|frame| CaseSent {
                role: frame.role,
                protected: frame.key.is_some_and(|(_, live)| live),
            })
            .collect()
    }
    fn interface(&self) -> Interface {
        let interface = self.hr().air.access_point();
        Interface {
            receives: interface.receive.is_some(),
            tsf_running: matches!(interface.tsf, ModelAccessPointTsf::Running { .. }),
        }
    }
    fn key_writes(&self) -> usize {
        let harness = self.hr();
        harness.air.group_writes() + harness.port.driver.keys.len()
    }
    fn idle(&self) -> bool {
        let harness = self.hr();
        harness.tickets.len() == MODEL_TICKETS && harness.small.len() == MODEL_SMALL_BUFFERS
    }
}

#[test]
fn the_model_passes_every_access_point_case() {
    for case in ap_cases::Case::ALL {
        embassy_futures::block_on(with_local_slot(async |claim| {
            let mut harness = ApHarness::new(Harness::new(claim));
            assert_eq!(ap_cases::check(&mut harness, case), Ok(()), "{case:?}");
        }));
    }
}

/// The model's Timestamp rule (`Transmitter`): a beacon goes on the air with
/// the access point's TSF in its Timestamp field, the time since its
/// restart, while the buffer keeps what the caller wrote there (zero here).
#[test]
fn a_beacon_goes_on_the_model_s_air_with_the_access_point_s_tsf_and_its_buffer_as_written() {
    use oer_protocols_ieee80211_contracts_port::TsfControl;
    embassy_futures::block_on(with_local_slot(async |claim| {
        let mut harness = ApHarness::new(Harness::new(claim));
        ap_cases::start(&mut harness).unwrap();
        let configure = harness.configure(
            Issued::Current,
            ap_cases::Receive::Bss,
            Some(TsfControl::Restart),
        );
        ap_cases::done(&mut harness, configure).unwrap();
        let restarted = harness.now();
        harness.advance(Duration::from_micros(1_234));
        let deadline = harness.now().saturating_add(Duration::from_secs(1));
        let beacon = harness.beacon(Issued::Current, deadline);
        ap_cases::submit(&mut harness, beacon).unwrap();
        ap_cases::service(&mut harness).unwrap();
        let sent = harness.hr().air.sent();
        let beacon = sent
            .iter()
            .find(|sent| sent.frame.first() == Some(&0x80))
            .expect("the beacon went");
        let on_air = u64::from_le_bytes(beacon.frame[24..32].try_into().unwrap());
        assert_eq!(beacon.plaintext[24..32], [0; 8], "the buffer as written");
        assert!(on_air >= 1_234, "the access point's TSF: {on_air}");
        assert!(
            on_air
                <= harness
                    .now()
                    .saturating_duration_since(restarted)
                    .as_micros(),
            "no later than now: {on_air}"
        );
    }));
}
