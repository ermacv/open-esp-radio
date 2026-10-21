use super::keys::{self, PortKeys, Role};
use super::*;
#[cfg(oer_debug_control)]
use oer_ieee80211_wire::qos::WmmAccessCategory;

pub struct Esp32s31Setup<'a, 'id, D: Device<B>, const B: usize> {
    pub(super) backend: &'a mut Esp32s31Backend<D, B>,
    pub(super) held: &'a Held<'id, D, B>,
}
impl<D: Device<B>, const B: usize> LowerMacSetup for Esp32s31Setup<'_, '_, D, B> {
    type Caps = Esp32s31Caps<B>;
    fn set_channel(&mut self, channel: SupportedChannel<Self::Caps>) {
        self.backend.channel = channel.channel();
        self.held.channel.set(channel.channel());
        self.held.storage.driver.wake();
    }
}

pub struct Esp32s31Phase<'p, 'id, D: Device<B>, const B: usize> {
    pub(super) held: &'p Held<'id, D, B>,
    pub(super) addresses: [IndividualAddress; 2],
}
#[cfg(oer_debug_control)]
impl<D: Device<B>, const B: usize> Esp32s31Phase<'_, '_, D, B> {
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
impl<'id, D: Device<B>, const B: usize> radio::PortPhase for Esp32s31Phase<'_, 'id, D, B> {
    type Event = Event<'id, D, B>;
    type Fault = Fault;
    fn next_event(
        &mut self,
    ) -> impl Future<Output = Result<Result<Self::Event, radio::EventsLost>, radio::EventEnd<Fault>>> + '_
    {
        poll_fn(|cx| {
            self.held.storage.body.register(cx.waker());
            if let Some(event) = self.held.events.pop() {
                return Poll::Ready(Ok(Ok(event)));
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
impl<'p, 'id: 'p, D: Device<B>, const B: usize> LowerMacPhase<'p, 'id>
    for Esp32s31Phase<'p, 'id, D, B>
{
    type Caps = Esp32s31Caps<B>;
    type Types = Types<D, B>;
    type Station<'v>
        = Esp32s31Interface<'p, 'v, 'id, D, B, Station>
    where
        Self: 'v;
    type AccessPoint<'v>
        = Esp32s31Interface<'p, 'v, 'id, D, B, AccessPoint>
    where
        Self: 'v;
    fn channel(&self) -> Channel {
        self.held.channel.get()
    }
    fn recycle(&mut self, buffer: ReceiveBuffer<D::Receive>) {
        if let Err(buffer) = self.held.recycled.push(buffer) {
            core::mem::forget(buffer);
            self.held.storage.poison(Fault::Capacity);
        }
        self.held.storage.driver.wake();
    }
    fn close_gate(&mut self, gate: GateOpen<'id, Part<'id, 1>>) {
        self.held.command(driver::Command::CloseGate(gate));
    }
    fn open_gate(&mut self, gate: GateClosed<'id, Part<'id, 1>>) -> GateOpen<'id, Part<'id, 1>> {
        let gate = self.held.gate.borrow_mut().open(gate);
        self.held.command(driver::Command::OpenGate);
        gate
    }
    fn station(&mut self) -> Self::Station<'_> {
        Esp32s31Interface {
            held: self.held,
            address: self.addresses[0],
            proof: PhantomData,
        }
    }
    fn access_point(&mut self) -> Self::AccessPoint<'_> {
        Esp32s31Interface {
            held: self.held,
            address: self.addresses[1],
            proof: PhantomData,
        }
    }
}

pub struct Esp32s31Interface<'p, 'v, 'id, D: Device<B>, const B: usize, R> {
    held: &'v Held<'id, D, B>,
    address: IndividualAddress,
    proof: PhantomData<fn(&'p ()) -> (&'p (), R)>,
}
impl<'id, D: Device<B>, const B: usize> Held<'id, D, B> {
    pub(super) fn serial(&self) -> u64 {
        let serial = self.storage.identity.get();
        match serial.checked_add(1) {
            Some(next) => self.storage.identity.set(next),
            None => self.storage.poison(Fault::Capacity),
        }
        serial
    }
}

impl<'p, 'v, 'id: 'p, D: Device<B>, const B: usize, R> InterfacePhase<'p, 'id>
    for Esp32s31Interface<'p, 'v, 'id, D, B, R>
where
    R: Role<'id> + RoleKeys<'id, Types<D, B>, Keys = PortKeys<R>>,
    PortKeys<R>: KeyTypes<'id, Identity = keys::KeyIdentity>,
{
    type Caps = Esp32s31Caps<B>;
    type Types = Types<D, B>;
    type Role = R;
    fn address(&self) -> IndividualAddress {
        self.address
    }
    fn submit<'k>(
        &mut self,
        mut attempt: InterfaceSubmission<'k, 'p, 'id, Self>,
    ) -> Result<(), radio::Poisoned<Fault>> {
        // Admission happens on this call, before the P2 queue is serviced.
        if attempt.operation.execute().mpdu.buffer.admission.is_none() {
            let mut device = self.held.storage.device.borrow_mut();
            match device.radio_now() {
                Ok(now) => {
                    attempt.operation.execute().mpdu.buffer.admission =
                        Some((now, device.timer().now()))
                }
                Err(_) => self.held.storage.poison(Fault::Clock),
            }
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
        let mut edca = self.held.edca.get();
        edca[if R::ROLE == VifRole::Station { 0 } else { 1 }] = parameters.get();
        self.held.edca.set(edca);
        self.held.dirty_edca.set(true);
        self.held.storage.driver.wake();
        parameters
    }
    fn install_pairwise_key(
        &mut self,
        association: &<PortKeys<R> as KeyTypes<'id>>::Association,
        slot: <PortKeys<R> as KeyTypes<'id>>::PairwiseSlot,
        peer: IndividualAddress,
        key: Ccmp128Key,
    ) -> <PortKeys<R> as KeyTypes<'id>>::PairwiseKey {
        let identity = R::identity(
            association,
            &slot,
            self.held.serial(),
            &self.held.storage.key_live,
        );
        identity.activity.set(identity.serial);
        let installation = R::pairwise(association, slot, peer);
        self.held.command(driver::Command::Install {
            identity,
            installation,
            material: key,
        });
        R::pairwise_key(identity)
    }
    fn install_group_key(
        &mut self,
        association: &<PortKeys<R> as KeyTypes<'id>>::Association,
        permission: <PortKeys<R> as KeyTypes<'id>>::GroupPermission,
        slot: <PortKeys<R> as KeyTypes<'id>>::GroupSlot,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    ) {
        self.held.command(driver::Command::Group {
            at: self.held.storage.device.borrow().timer().now(),
            command: R::group(association, slot, permission, key_id, key),
        });
    }
    fn remove_pairwise_key(
        &mut self,
        association: <PortKeys<R> as KeyTypes<'id>>::Association,
        key: <PortKeys<R> as KeyTypes<'id>>::PairwiseKey,
    ) {
        let identity = <PortKeys<R> as KeyTypes<'id>>::pairwise_identity(&key);
        identity.activity.set(0);
        self.held.command(driver::Command::Remove {
            identity,
            removal: R::removal(association),
        });
    }
    fn remove_group_key(
        &mut self,
        permission: <PortKeys<R> as KeyTypes<'id>>::GroupPermission,
        key: <PortKeys<R> as KeyTypes<'id>>::GroupKey,
    ) {
        let identity = R::group_identity(key);
        identity.activity.set(0);
        self.held.command(driver::Command::Group {
            at: self.held.storage.device.borrow().timer().now(),
            command: keys::GroupCommand::Remove {
                identity,
                removal: R::remove_group(permission),
            },
        });
    }
    fn add_block_ack(
        &mut self,
        slot: PhaseBlockAckSlot<'id, Self::Types>,
        agreement: SupportedBlockAck<Self::Caps>,
    ) -> ReceiveAgreement<'id> {
        self.set_block_ack(slot.index(), Some(agreement));
        ReceiveAgreement { slot }
    }
    fn replace_block_ack(
        &mut self,
        running: ReceiveAgreement<'id>,
        agreement: SupportedBlockAck<Self::Caps>,
    ) -> ReceiveAgreement<'id> {
        self.set_block_ack(running.slot.index(), Some(agreement));
        running
    }
    fn remove_block_ack(
        &mut self,
        agreement: ReceiveAgreement<'id>,
    ) -> PhaseBlockAckSlot<'id, Self::Types> {
        self.set_block_ack(agreement.slot.index(), None);
        agreement.slot
    }
}
impl<'id, D: Device<B>, const B: usize, R: InterfaceRole> Esp32s31Interface<'_, '_, 'id, D, B, R> {
    fn set_block_ack(&self, index: u8, agreement: Option<SupportedBlockAck<Esp32s31Caps<B>>>) {
        self.held.block_acks.borrow_mut()[usize::from(index)] = agreement.map(|agreement| {
            let (peer, tid, start_sequence, window) = agreement.get();
            core_mac::RxBlockAckValues {
                role: R::ROLE,
                peer: peer.get(),
                tid: tid.get(),
                start_sequence,
                window: window as u8,
            }
        });
        self.held.dirty_ba.set(true);
        self.held.storage.driver.wake();
    }
}
impl<'p, 'v, 'id: 'p, D: Device<B>, const B: usize> StationPhase<'p, 'id>
    for Esp32s31Interface<'p, 'v, 'id, D, B, Station>
{
    fn replace_group_key(
        &mut self,
        permission: keys::StaPermission<'id>,
        installed: GroupKey<'id, Station>,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    ) {
        self.held.command(driver::Command::Group {
            at: self.held.storage.device.borrow().timer().now(),
            command: keys::GroupCommand::Replace {
                permission,
                key: installed,
                key_id,
                material: key,
            },
        });
    }
    fn select_group_key(
        &mut self,
        permission: keys::StaPermission<'id>,
        installed: GroupKey<'id, Station>,
    ) {
        self.held.command(driver::Command::Group {
            at: self.held.storage.device.borrow().timer().now(),
            command: keys::GroupCommand::Select {
                permission,
                key: installed,
            },
        });
    }
    fn configure(
        &mut self,
        permission: ConfigurePermission<'id, Station, Part<'id, 8>, D::ConfigureContext<'id>>,
        receive: StationReceive<Self::Caps>,
    ) {
        let (bssid, filter) = match receive {
            StationReceive::Off => (None, ReceiveFilter::NONE),
            StationReceive::OtherBssManagement => (None, ReceiveFilter::OTHER_BSS_MANAGEMENT),
            StationReceive::Bss { bssid, filter } => (Some(bssid.get()), filter.get()),
        };
        self.held.command(driver::Command::Configure {
            permission: Configured::Station(permission),
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
impl<'p, 'v, 'id: 'p, D: Device<B>, const B: usize> AccessPointPhase<'p, 'id>
    for Esp32s31Interface<'p, 'v, 'id, D, B, AccessPoint>
{
    fn configure(
        &mut self,
        permission: ConfigurePermission<'id, AccessPoint, Part<'id, 9>, D::ConfigureContext<'id>>,
        receive: SupportedFilter<Self::Caps, AccessPoint>,
    ) {
        self.held.command(driver::Command::Configure {
            permission: Configured::AccessPoint(permission),
            config: CoreVif {
                role: VifRole::AccessPoint,
                address: self.address.get(),
                bssid: Some(self.address.get()),
                receive: receive.get(),
            },
        });
    }
}
