//! One installation of the port: the pools it mints, the tokens its
//! lifecycle and datapath actor take, and the port's backend identity.
use super::*;
use radio::{SlotRetire, SlotStopped, SlotWakes};

pub(super) type PortCoreOwner<'id, Id, D, const B: usize> =
    crate::lower_mac::CoreOwner<Core<'id, Id, D, B>>;

/// The complete pool layout one installation mints.
pub type InstallationPools = (
    CorePools<TX_TICKETS>,
    ArbiterPool,
    TransmitPool<Tickets, TX_TICKETS>,
    radio::Slots<Buffers, TX_BUFFERS>,
    radio::Slots<ConnectionEpochKind, 1>,
    radio::Slots<GroupPermission<Station>, 1>,
    radio::Slots<GroupPermission<AccessPoint>, 1>,
    association::BlockAckSlots<BlockAcks, 8>,
    InterfaceCommandPool<Station>,
    InterfaceCommandPool<AccessPoint>,
);

/// The tokens of one installation, under the brand its tasks share.
pub struct Resources<'p, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    pub tickets: [Slot<'p, Tickets, Part<Id, 2>>; TX_TICKETS],
    pub terminal_access: TerminalAccess<'p, Tickets, Part<Id, 2>>,
    pub buffers: [Buffer<'p, Id, D, B, O>; TX_BUFFERS],
    pub connection_epoch: Slot<'p, ConnectionEpochKind, Part<Id, 4>>,
    pub station_association: core_mac::association::VacantStationAssociation<'p, CorePath<Id>>,
    pub access_point_association:
        core_mac::association::VacantAccessPointAssociation<'p, CorePath<Id>>,
    pub station_commands: InterfacePermissions<'p, Station, Part<Id, 8>>,
    pub access_point_commands: InterfacePermissions<'p, AccessPoint, Part<Id, 9>>,
    pub block_acks: [association::BlockAckSlot<'p, BlockAcks, 8, Part<Id, 7>>; 8],
    pub station_group_permission: keys::StaPermission<'p, Id>,
    pub access_point_group_permission: keys::ApPermission<'p, Id>,
    pub gate: GateOpen<'p, Part<Id, 1>>,
    pub arbiter_authority: ArbiterAuthority<'p, Part<Id, 1>>,
}
/// The tokens of one installation a station's lifecycle takes: all but the
/// receive Block Ack slots, whose agreements the datapath actor keeps, and
/// the access point's, which a station-only composition leaves unused.
pub struct StationTokens<'p, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    pub tickets: [Slot<'p, Tickets, Part<Id, 2>>; TX_TICKETS],
    pub terminal_access: TerminalAccess<'p, Tickets, Part<Id, 2>>,
    pub buffers: [Buffer<'p, Id, D, B, O>; TX_BUFFERS],
    pub connection_epoch: Slot<'p, ConnectionEpochKind, Part<Id, 4>>,
    pub station_association: core_mac::association::VacantStationAssociation<'p, CorePath<Id>>,
    pub station_commands: InterfacePermissions<'p, Station, Part<Id, 8>>,
    pub station_group_permission: keys::StaPermission<'p, Id>,
    pub gate: GateOpen<'p, Part<Id, 1>>,
    pub arbiter_authority: ArbiterAuthority<'p, Part<Id, 1>>,
}
/// An installation's receive Block Ack slots.
pub type BlockAckSlots<'p, Id> = [association::BlockAckSlot<'p, BlockAcks, 8, Part<Id, 7>>; 8];
impl<'p, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    Resources<'p, Id, D, B, O>
{
    /// Split the installation's tokens for a station-only composition: the
    /// receive Block Ack slots and the lifecycle's tokens. The access
    /// point's tokens drop unused.
    pub fn into_station(self) -> (BlockAckSlots<'p, Id>, StationTokens<'p, Id, D, B, O>) {
        let Self {
            tickets,
            terminal_access,
            buffers,
            connection_epoch,
            station_association,
            access_point_association: _,
            station_commands,
            access_point_commands: _,
            block_acks,
            station_group_permission,
            access_point_group_permission: _,
            gate,
            arbiter_authority,
        } = self;
        (
            block_acks,
            StationTokens {
                tickets,
                terminal_access,
                buffers,
                connection_epoch,
                station_association,
                station_commands,
                station_group_permission,
                gate,
                arbiter_authority,
            },
        )
    }
}

/// The port's backend identity: the tag of its loan conversions between
/// the driver's `'static` owners and an installation's brand, and the
/// owner its sealed admissions name. It has no value: no slot registers
/// it, and none of its slot operations can run.
pub struct Esp32s31Backend<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>(
    Infallible,
    PhantomData<fn(Id) -> (Id, D, O)>,
);

impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> SlotOwner<Id>
    for Esp32s31Backend<Id, D, B, O>
{
    type Pools = InstallationPools;
    type Queues = ();
    const VACANT_QUEUES: Self::Queues = ();
    type Vacant = Infallible;
    type Frontend<'s>
        = Infallible
    where
        Self: 's,
        Id: 's;
    type Invocation<'s>
        = Infallible
    where
        Self: 's,
        Id: 's;
    fn prepare_queues(&mut self, _: &mut (), _: radio::Mint<'static, Self::Pools, Id>) {
        match self.0 {}
    }
    fn split<'s>(
        _: &'s mut (),
        _: SlotWakes<'s, Id>,
    ) -> Result<(Self::Frontend<'s>, Self::Invocation<'s>), oer_protocols_radio_port::SlotInitError>
    where
        Self: 's,
        Id: 's,
    {
        Err(oer_protocols_radio_port::SlotInitError::MissingResource(
            "the S31 port registers no slot",
        ))
    }
    fn run<'s>(
        &mut self,
        _: &mut Self::Invocation<'s>,
        _: SlotRetire<'_, Id, Self>,
    ) -> impl Future<Output = SlotStopped<Id, Self>>
    where
        Self: 's,
        Id: 's,
    {
        let never = self.0;
        async move { match never {} }
    }
    fn finish<'s>(&mut self, _: &mut Self::Invocation<'s>) -> Self::Vacant
    where
        Self: 's,
        Id: 's,
    {
        match self.0 {}
    }
}
