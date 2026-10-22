use super::keys::{self, PortKeys, Role};
use super::*;
#[cfg(oer_debug_control)]
use oer_ieee80211_wire::qos::WmmAccessCategory;

pub struct Esp32s31Setup<'a, 'id, Id: SlotIdentity, D: Device<B>, const B: usize> {
    pub(super) loan: BodyLoan<'id, Id, Esp32s31Backend<Id, D, B>>,
    pub(super) held: &'a FrontHeld<'id, Id, D, B>,
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize> LowerMacSetup
    for Esp32s31Setup<'_, '_, Id, D, B>
{
    type Caps = Esp32s31Caps<B>;
    fn set_channel(&mut self, channel: SupportedChannel<Self::Caps>) {
        self.held.publishing.set(true);
        self.held.channel.set(channel.channel());
        self.held.publishing.set(false);
        self.held.storage.driver.wake();
    }
}

pub struct Esp32s31Phase<'p, 'id, Id: SlotIdentity, D: Device<B>, const B: usize> {
    pub(super) loan: BodyLoan<'id, Id, Esp32s31Backend<Id, D, B>>,
    pub(super) held: &'p FrontHeld<'id, Id, D, B>,
    pub(super) addresses: [IndividualAddress; 2],
}
#[cfg(oer_debug_control)]
impl<Id: SlotIdentity, D: Device<B>, const B: usize> Esp32s31Phase<'_, '_, Id, D, B> {
    /// Hold this AC before it transfers DMA ownership to the real transmitter.
    pub fn debug_hold_publication(&mut self, category: WmmAccessCategory, hold: bool) {
        let bit = 1 << LegacyTxQueue::from_access_category(category).hardware_index();
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
impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize> radio::PortPhase
    for Esp32s31Phase<'_, 'id, Id, D, B>
{
    type Event = Event<'id, Id, D, B>;
    type Fault = Fault;
    fn next_event(
        &mut self,
    ) -> impl Future<Output = Result<Result<Self::Event, radio::EventsLost>, radio::EventEnd<Fault>>> + '_
    {
        poll_fn(|cx| {
            self.held.storage.body.register(cx.waker());
            if let Some(event) = self.held.events.try_receive().ok() {
                return Poll::Ready(Ok(Ok(event.lend(&self.loan))));
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
impl<'p, 'id: 'p, Id: SlotIdentity + 'id, D: Device<B>, const B: usize> LowerMacPhase<'p, 'id>
    for Esp32s31Phase<'p, 'id, Id, D, B>
{
    type Caps = Esp32s31Caps<B>;
    type Types = Types<Id, D, B>;
    type Station<'v>
        = Esp32s31Interface<'p, 'v, 'id, Id, D, B, Station>
    where
        Self: 'v;
    type AccessPoint<'v>
        = Esp32s31Interface<'p, 'v, 'id, Id, D, B, AccessPoint>
    where
        Self: 'v;
    fn channel(&self) -> Channel {
        self.held.channel.get()
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

pub struct Esp32s31Interface<'p, 'v, 'id, Id: SlotIdentity, D: Device<B>, const B: usize, R> {
    loan: BodyLoan<'id, Id, Esp32s31Backend<Id, D, B>>,
    held: &'v FrontHeld<'id, Id, D, B>,
    address: IndividualAddress,
    proof: PhantomData<fn(&'p ()) -> (&'p (), R)>,
}

impl<'p, 'v, 'id: 'p, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, R>
    InterfacePhase<'p, 'id> for Esp32s31Interface<'p, 'v, 'id, Id, D, B, R>
where
    R: Role<'id, Id> + RoleKeys<'id, Types<Id, D, B>, Keys = PortKeys<Id, R>>,
    PortKeys<Id, R>: KeyTypes<'id, Identity = keys::KeyIdentity<'id, Id>>,
{
    type Caps = Esp32s31Caps<B>;
    type Types = Types<Id, D, B>;
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
        self.held.edca.set(
            if R::ROLE == VifRole::Station { 0 } else { 1 },
            parameters.get(),
        );
        self.held.dirty_edca.set(true);
        self.held.publishing.set(false);
        self.held.storage.driver.wake();
        parameters
    }
    fn install_pairwise_key(
        &mut self,
        association: &<PortKeys<Id, R> as KeyTypes<'id>>::Association,
        slot: <PortKeys<Id, R> as KeyTypes<'id>>::PairwiseSlot,
        peer: IndividualAddress,
        key: Ccmp128Key,
    ) -> <PortKeys<Id, R> as KeyTypes<'id>>::PairwiseKey {
        let identity = R::identity(
            association,
            &slot,
            self.held.serial(),
            &self.held.storage.key_live,
        );
        identity.activity.set(identity.serial);
        let installation = R::pairwise(association, slot, peer).into_owned(&self.loan);
        self.held.command(driver::Command::Install {
            identity,
            installation,
            material: key,
        });
        R::pairwise_key(identity)
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
impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, R: InterfaceRole>
    Esp32s31Interface<'_, '_, 'id, Id, D, B, R>
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
impl<'p, 'v, 'id: 'p, Id: SlotIdentity + 'id, D: Device<B>, const B: usize> StationPhase<'p, 'id>
    for Esp32s31Interface<'p, 'v, 'id, Id, D, B, Station>
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
impl<'p, 'v, 'id: 'p, Id: SlotIdentity + 'id, D: Device<B>, const B: usize>
    AccessPointPhase<'p, 'id> for Esp32s31Interface<'p, 'v, 'id, Id, D, B, AccessPoint>
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
