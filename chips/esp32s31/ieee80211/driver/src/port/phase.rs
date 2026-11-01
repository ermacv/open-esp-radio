use super::keys::{self, PortKeys, Role};
use super::*;
#[cfg(oer_debug_control)]
use oer_ieee80211_wire::qos::WmmAccessCategory;

pub struct Esp32s31Setup<'a, 'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    pub(super) loan: BodyLoan<'id, Id, Esp32s31Backend<Id, D, B, O>>,
    pub(super) held: &'a FrontHeld<'id, Id, D, B, O>,
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> LowerMacSetup
    for Esp32s31Setup<'_, '_, Id, D, B, O>
{
    type Caps = Esp32s31Caps<B>;
    fn set_channel(&mut self, channel: SupportedChannel<Self::Caps>) {
        self.held.publishing.set(true);
        let published = self.held.channel.set(channel.channel());
        self.held.publishing.set(false);
        match published {
            // P1 alone writes the channel; its own view follows the store.
            Ok(()) => self.held.tuned.set(channel.channel()),
            Err(fault) => self.held.storage.poison(Fault::Setting(fault)),
        }
        self.held.storage.driver.wake();
    }
}

pub struct Esp32s31Phase<'p, 'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    pub(super) loan: BodyLoan<'id, Id, Esp32s31Backend<Id, D, B, O>>,
    pub(super) held: &'p FrontHeld<'id, Id, D, B, O>,
    pub(super) addresses: [IndividualAddress; 2],
}
#[cfg(oer_debug_control)]
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    Esp32s31Phase<'_, '_, Id, D, B, O>
{
    /// Reject the next GTK replacement's first publication. Rollback writes
    /// still reach the actual hardware. Only diagnostic images expose this.
    pub fn debug_reject_group_replacement(&mut self) {
        self.held.reject_group_replacement.set(true);
    }

    /// Hold this AC before it transfers DMA ownership to the real transmitter.
    pub fn debug_hold_publication(&mut self, category: WmmAccessCategory, hold: bool) {
        let bit = 1
            << LegacyTxQueue::from_access_category(category)
                .hardware_index()
                .get();
        let mask = self.held.publication_hold.get();
        self.held
            .publication_hold
            .set(if hold { mask | bit } else { mask & !bit });
        self.held.storage.driver.wake();
    }

    /// The image holds the completion facts in its static IRQ signal. Prevent
    /// the ordinary deadline service from reading the same result meanwhile.
    pub fn debug_hold_completion(&mut self, hold: bool) {
        self.held.completion_hold.set(hold);
        self.held.storage.driver.wake();
    }
}
impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations> radio::PortPhase
    for Esp32s31Phase<'_, 'id, Id, D, B, O>
{
    type Event = Event<'id, Id, D, B, O>;
    type Fault = Fault;
    fn next_event(
        &mut self,
    ) -> impl Future<Output = Result<Result<Self::Event, radio::EventsLost>, radio::EventEnd<Fault>>> + '_
    {
        poll_fn(|cx| {
            self.held.storage.body.register(cx.waker());
            if let Some(event) = self.held.events.try_receive().ok() {
                if matches!(event, owned::OwnedEvent::Received { .. }) {
                    self.held.storage.took_received();
                }
                match event.lend(&self.loan, self.held.completions) {
                    Some(event) => return Poll::Ready(Ok(Ok(event))),
                    // A parked terminal's place held none.
                    None => self.held.storage.poison(Fault::Capacity),
                }
            }
            if let Some(cause) = self.held.storage.poisoned.get() {
                return Poll::Ready(Err(radio::EventEnd::Quarantined { cause }));
            }
            if self.held.loss.replace(false) {
                return Poll::Ready(Ok(Err(radio::EventsLost)));
            }
            Poll::Pending
        })
    }
}
impl<'p, 'id: 'p, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
    LowerMacPhase<'p, 'id> for Esp32s31Phase<'p, 'id, Id, D, B, O>
{
    type Caps = Esp32s31Caps<B>;
    type Types = Types<Id, D, B, O>;
    type Station<'v>
        = Esp32s31Interface<'p, 'v, 'id, Id, D, B, O, Station>
    where
        Self: 'v;
    type AccessPoint<'v>
        = Esp32s31Interface<'p, 'v, 'id, Id, D, B, O, AccessPoint>
    where
        Self: 'v;
    fn channel(&self) -> Channel {
        self.held.tuned.get()
    }
    fn recycle(&mut self, buffer: ReceiveBuffer<'id, Id, D::Receive>) {
        if let Err(buffer) = self
            .held
            .recycled
            .try_send(buffer.into_owned())
            .map_err(|error| error.0)
        {
            core::mem::forget(buffer);
            self.held.storage.poison(Fault::Capacity);
        }
        self.held.storage.driver.wake();
    }
    fn lend(
        &mut self,
        buffer: ReceiveBuffer<'id, Id, D::Receive>,
    ) -> Result<
        super::LentReceive<D::Receive, D::Lent>,
        oer_ieee80211_contracts_port::LendRefused<ReceiveBuffer<'id, Id, D::Receive>>,
    > {
        let shared = self.held.storage;
        buffer.lend_to(&shared.network, D::LENT_RX, shared.check().is_err())
    }
    fn poll_lent_returned(&mut self, cx: &mut core::task::Context<'_>) -> core::task::Poll<()> {
        self.held.storage.network.poll_returned(cx)
    }
    fn lent(&self) -> usize {
        self.held.storage.network.held()
    }
    fn lend_room(&self) -> bool {
        self.held.storage.network.held() < D::LENT_RX
    }
    fn poll_lend_room(&mut self, cx: &mut core::task::Context<'_>) -> core::task::Poll<()> {
        self.held.storage.network.poll_room(cx, D::LENT_RX)
    }
    fn close_gate(&mut self, gate: GateOpen<'id, Part<Id, 1>>) {
        self.held
            .command(driver::Command::CloseGate(gate.into_owned(&self.loan)));
    }
    fn open_gate(&mut self, gate: GateClosed<'id, Part<Id, 1>>) -> GateOpen<'id, Part<Id, 1>> {
        let gate = self.held.gate.open(gate.into_owned(&self.loan));
        self.held.command(driver::Command::OpenGate);
        gate.lend(&self.loan)
    }
    fn station(&mut self) -> Self::Station<'_> {
        Esp32s31Interface {
            held: self.held,
            loan: self.loan,
            address: self.addresses[0],
            proof: PhantomData,
        }
    }
    fn access_point(&mut self) -> Self::AccessPoint<'_> {
        Esp32s31Interface {
            held: self.held,
            loan: self.loan,
            address: self.addresses[1],
            proof: PhantomData,
        }
    }
}

pub struct Esp32s31Interface<
    'p,
    'v,
    'id,
    Id: SlotIdentity,
    D: Device<B>,
    const B: usize,
    O: Observations,
    R,
> {
    loan: BodyLoan<'id, Id, Esp32s31Backend<Id, D, B, O>>,
    held: &'v FrontHeld<'id, Id, D, B, O>,
    address: IndividualAddress,
    proof: PhantomData<fn(&'p ()) -> (&'p (), R)>,
}

impl<'p, 'v, 'id: 'p, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations, R>
    InterfacePhase<'p, 'id> for Esp32s31Interface<'p, 'v, 'id, Id, D, B, O, R>
where
    R: Role<'id, Id> + RoleKeys<'id, Types<Id, D, B, O>, Keys = PortKeys<Id, R>>,
    PortKeys<Id, R>: KeyTypes<'id, Identity = keys::KeyIdentity<'id, Id>>,
{
    type Caps = Esp32s31Caps<B>;
    type Types = Types<Id, D, B, O>;
    type Role = R;
    fn address(&self) -> IndividualAddress {
        self.address
    }
    fn submit<'k>(
        &mut self,
        attempt: InterfaceSubmission<'k, 'p, 'id, Self>,
    ) -> Result<(), radio::Poisoned<Fault>> {
        let mut attempt = attempt.into_parts(&self.loan);
        // Admission happens on this call, before the P2 queue is serviced.
        let buffer = &mut attempt.operation.execute().mpdu.buffer;
        if buffer.admission.is_none() {
            buffer.admission = Some(self.held.timer.now());
        }
        self.held.command(driver::Command::Submit(driver::Pending {
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
        }));
        self.held.storage.check()
    }
    fn set_edca(&mut self, parameters: SupportedEdca<Self::Caps>) -> SupportedEdca<Self::Caps> {
        self.held.publishing.set(true);
        let published = self.held.edca.set(
            if R::ROLE == VifRole::Station { 0 } else { 1 },
            parameters.get(),
        );
        self.held.dirty_edca.set(true);
        self.held.publishing.set(false);
        // A supported table's lifetimes are at most 500 TU and fit their
        // words; the port is not served with a table it did not store.
        if let Err(fault) = published {
            self.held.storage.poison(Fault::Setting(fault));
        }
        self.held.storage.driver.wake();
        parameters
    }
    fn install_pairwise_key(
        &mut self,
        association: &<PortKeys<Id, R> as KeyTypes<'id>>::Association,
        slot: <PortKeys<Id, R> as KeyTypes<'id>>::PairwiseSlot,
        peer: IndividualAddress,
        key: Ccmp128Key,
    ) -> <PortKeys<Id, R> as KeyTypes<'id>>::Identity {
        // P2 activates the identity after the write and returns the key in
        // the installation terminal; until then no attempt can name it.
        let identity = R::identity(
            association,
            &slot,
            self.held.serial(),
            &self.held.storage.key_live,
        );
        let installation = R::pairwise(association, slot, peer).into_owned(&self.loan);
        self.held.command(driver::Command::Install {
            identity,
            installation,
            material: key,
        });
        KeyIdentity {
            owned: identity,
            brand: PhantomData,
        }
    }
    fn install_group_key(
        &mut self,
        association: &<PortKeys<Id, R> as KeyTypes<'id>>::Association,
        permission: <PortKeys<Id, R> as KeyTypes<'id>>::GroupPermission,
        slot: <PortKeys<Id, R> as KeyTypes<'id>>::GroupSlot,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    ) {
        self.held.command(driver::Command::Group {
            at: self.held.timer.now(),
            command: R::group(association, slot, permission, key_id, key).into_owned(&self.loan),
        });
    }
    fn remove_pairwise_key(
        &mut self,
        association: <PortKeys<Id, R> as KeyTypes<'id>>::Association,
        key: <PortKeys<Id, R> as KeyTypes<'id>>::PairwiseKey,
    ) {
        let identity = <PortKeys<Id, R> as KeyTypes<'id>>::pairwise_identity(&key).owned;
        identity.activity.set(0);
        self.held.command(driver::Command::Remove {
            identity,
            removal: R::removal(association).into_owned(&self.loan),
        });
    }
    fn remove_group_key(
        &mut self,
        permission: <PortKeys<Id, R> as KeyTypes<'id>>::GroupPermission,
        key: <PortKeys<Id, R> as KeyTypes<'id>>::GroupKey,
    ) {
        let identity = R::group_identity(key);
        identity.activity.set(0);
        self.held.command(driver::Command::Group {
            at: self.held.timer.now(),
            command: keys::GroupCommand::Remove {
                identity,
                removal: R::remove_group(permission).into_owned(&self.loan),
            },
        });
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
}
/// The access category of an aggregate's TID: the user priority's
/// (IEEE Std 802.11-2020, Table 10-1); best effort for a TID beyond them.
fn aggregate_category(tid: Tid) -> oer_ieee80211_wire::qos::WmmAccessCategory {
    oer_ieee80211_wire::qos::WmmUserPriority::new(tid.get()).map_or(
        oer_ieee80211_wire::qos::WmmAccessCategory::BestEffort,
        |priority| priority.access_category(),
    )
}

impl<'p, 'v, 'id: 'p, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations, R>
    AmpduPhase<'p, 'id> for Esp32s31Interface<'p, 'v, 'id, Id, D, B, O, R>
where
    R: Role<'id, Id> + RoleKeys<'id, Types<Id, D, B, O>, Keys = PortKeys<Id, R>>,
    PortKeys<Id, R>: KeyTypes<'id, Identity = keys::KeyIdentity<'id, Id>>,
{
    fn ampdu_capabilities(&self) -> AmpduCapabilities {
        esp32s31_ampdu_capabilities(TX_AMPDU_SUBFRAMES)
    }
    fn aggregate<'k>(
        &mut self,
        receiver: IndividualAddress,
        tid: Tid,
        rate: AmpduRate<'p, 'id, Self::Caps>,
        protection: InterfaceProtection<'k, 'p, 'id, Self>,
    ) -> InterfaceAggregate<'k, 'p, 'id, Self, EmptyAggregate>
    where
        'id: 'k,
    {
        let key = match &protection {
            TxProtection::Open => None,
            TxProtection::Ccmp(key) => Some(R::tx_identity(key).owned),
        };
        // The builder checks its subframes under the TXOP limit P1 last
        // published for this role; P2 plans under the same value.
        let vif = if R::ROLE == VifRole::Station { 0 } else { 1 };
        let txop_limit_units_32_us = match self.held.edca.get(vif) {
            Some(table) => {
                table
                    .access_category(aggregate_category(tid))
                    .wmm
                    .txop_limit_units_32_us
            }
            // P1 alone publishes the table and is not publishing it now.
            None => {
                self.held.storage.poison(Fault::Capacity);
                0
            }
        };
        aggregate::Aggregate::new(aggregate::AggregateSetup {
            shared: self.held.storage,
            loan: self.loan,
            receiver,
            tid,
            rate,
            key,
            role: R::ROLE,
            txop_limit_units_32_us,
        })
    }
    fn submit_ampdu<'k>(
        &mut self,
        attempt: InterfaceAggregateSubmission<'k, 'p, 'id, Self>,
    ) -> Result<(), radio::Poisoned<Fault>>
    where
        'id: 'k,
    {
        let mut attempt = attempt.into_parts(&self.loan);
        // Admission happens on this call, before the P2 queue is serviced;
        // a returned subframe keeps its first admission.
        let oldest = attempt.operation.execute().admit(self.held.timer.now());
        self.held
            .command(driver::Command::SubmitAggregate(driver::PendingAggregate {
                category: aggregate_category(attempt.execution.tid),
                operation: attempt.operation,
                execution: attempt.execution,
                protection: attempt.protection,
                power: attempt.power.get(),
                min_mpdu_start_spacing: attempt.min_mpdu_start_spacing.get(),
                oldest,
            }));
        self.held.storage.check()
    }
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations, R: InterfaceRole>
    Esp32s31Interface<'_, '_, 'id, Id, D, B, O, R>
{
    fn set_block_ack(&self, index: u8, agreement: Option<SupportedBlockAck<Esp32s31Caps<B>>>) {
        self.held.publishing.set(true);
        self.held.block_acks.set(
            usize::from(index),
            agreement.map(|agreement| {
                let (peer, tid, start_sequence, window) = agreement.get();
                core_mac::RxBlockAckValues {
                    role: R::ROLE,
                    peer: peer.get(),
                    tid: tid.get(),
                    start_sequence,
                    window: window as u8,
                }
            }),
        );
        self.held.dirty_ba.set(true);
        self.held.publishing.set(false);
        self.held.storage.driver.wake();
    }
}
impl<'p, 'v, 'id: 'p, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
    StationPhase<'p, 'id> for Esp32s31Interface<'p, 'v, 'id, Id, D, B, O, Station>
{
    fn replace_group_key(
        &mut self,
        permission: keys::StaPermission<'id, Id>,
        installed: GroupKey<'id, Id, Station>,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    ) {
        self.held.command(driver::Command::Group {
            at: self.held.timer.now(),
            command: keys::GroupCommand::Replace {
                permission: self.loan.reclaim_slot(permission),
                key: installed.into_owned(),
                key_id,
                material: key,
            },
        });
    }
    fn select_group_key(
        &mut self,
        permission: keys::StaPermission<'id, Id>,
        installed: GroupKey<'id, Id, Station>,
    ) {
        self.held.command(driver::Command::Group {
            at: self.held.timer.now(),
            command: keys::GroupCommand::Select {
                permission: self.loan.reclaim_slot(permission),
                key: installed.into_owned(),
            },
        });
    }
    fn configure(
        &mut self,
        permission: ConfigurePermission<'id, Station, Part<Id, 8>, D::ConfigureContext<'id, Id>>,
        receive: StationReceive<Self::Caps>,
    ) {
        let (bssid, filter) = match receive {
            StationReceive::Off => (None, ReceiveFilter::NONE),
            StationReceive::OtherBssManagement => (None, ReceiveFilter::OTHER_BSS_MANAGEMENT),
            StationReceive::Bss { bssid, filter } => (Some(bssid.get()), filter.get()),
        };
        self.held.command(driver::Command::Configure {
            permission: owned::OwnedConfigured::Station(permission.into_owned(&self.loan)),
            config: CoreVif {
                role: VifRole::Station,
                address: self.address.get(),
                bssid,
                receive: filter,
            },
        });
    }
    fn set_bss_color(&mut self, color: BssColor) {
        self.held.color.set(Some(color));
        self.held.storage.driver.wake();
    }
}
impl<'p, 'v, 'id: 'p, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
    AccessPointPhase<'p, 'id> for Esp32s31Interface<'p, 'v, 'id, Id, D, B, O, AccessPoint>
{
    fn configure(
        &mut self,
        permission: ConfigurePermission<
            'id,
            AccessPoint,
            Part<Id, 9>,
            D::ConfigureContext<'id, Id>,
        >,
        receive: SupportedFilter<Self::Caps, AccessPoint>,
    ) {
        self.held.command(driver::Command::Configure {
            permission: owned::OwnedConfigured::AccessPoint(permission.into_owned(&self.loan)),
            config: CoreVif {
                role: VifRole::AccessPoint,
                address: self.address.get(),
                bssid: Some(self.address.get()),
                receive: receive.get(),
            },
        });
    }
}
