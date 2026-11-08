//! One installation of the port: the pools it mints, the tokens its
//! lifecycle and datapath actor take, and the port's backend identity.
use super::*;

pub(super) type PortCoreOwner<'id, Id, D, const B: usize> =
    crate::lower_mac::CoreOwner<Core<'id, Id, D, B>>;

/// The complete pool layout one installation mints.
pub type InstallationPools = (
    CorePools<TX_TICKETS>,
    ArbiterPool,
    TransmitPool<Tickets, TX_TICKETS>,
    radio::Slots<Buffers, TX_BUFFERS>,
    radio::Slots<ConnectionEpochKind, 1>,
    IssuedSlots<GroupPermission<Station>, 1>,
    IssuedSlots<GroupPermission<AccessPoint>, 1>,
    association::BlockAckSlots<BlockAcks, 8>,
    InterfaceCommandPool<Station>,
    InterfaceCommandPool<AccessPoint>,
    BeaconPool,
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
    pub access_point_beacon: BeaconPermission<'p, Part<Id, 10>>,
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
            access_point_beacon: _,
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

/// The port's backend tag: the tag of its loan conversions between the
/// driver's `'static` owners and an installation's brand, and the owner
/// its sealed admissions name. It has no value.
pub struct Esp32s31Backend<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>(
    Infallible,
    PhantomData<fn(Id) -> (Id, D, O)>,
);
