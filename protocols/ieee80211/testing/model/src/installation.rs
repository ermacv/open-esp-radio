//! The model's installation: its composition tag, the pools one mint
//! issues and the driver queues and owned resources made from them.

use super::*;

type CompositionIdentity<Id, C, X> = PhantomData<fn(Id, C, X) -> (Id, C, X)>;

/// The model's composition tag: the slot identity, capabilities and
/// contexts of one model port, and the tag of its loan conversions between
/// the port's `'static` owners and an installation's brand. It has no
/// value.
pub struct ModelBackend<
    Id: SlotIdentity,
    C: Ieee80211Capabilities = ModelCaps,
    X: ModelContext = (),
>(core::convert::Infallible, CompositionIdentity<Id, C, X>);

impl<Id: SlotIdentity, C: Ieee80211Capabilities, X: ModelContext> ModelComposition
    for ModelBackend<Id, C, X>
{
    type Identity = Id;
    type Caps = C;
    type Contexts = X;
}

/// The installation's pool layout, which its one mint issues.
pub type ModelPools = (
    oer_protocols_ieee80211_contracts_port::ArbiterPool,
    oer_protocols_ieee80211_contracts_port::TransmitPool<ModelTickets, MODEL_TICKETS>,
    oer_protocols_radio_port::Slots<ModelSmall, MODEL_SMALL_BUFFERS>,
    oer_protocols_radio_port::Slots<ModelLarge, MODEL_LARGE_BUFFERS>,
    oer_protocols_radio_port::Slots<ConnectionEpochKind, 1>,
    ModelAssociationPool<Station>,
    ModelAssociationPool<AccessPoint>,
    ModelBlockAckPool,
    oer_protocols_ieee80211_contracts_port::IssuedSlots<ModelGroupPermission<Station>, 1>,
    (
        oer_protocols_ieee80211_contracts_port::IssuedSlots<ModelGroupPermission<AccessPoint>, 1>,
        InterfaceCommandPool<Station>,
        InterfaceCommandPool<AccessPoint>,
        oer_protocols_ieee80211_contracts_port::BeaconPool,
    ),
);

/// One installation's driver queues and owned resources from its mint of
/// the installation `generation`, with the air reset for it.
pub(super) fn installation_parts<B: ModelComposition>(
    air: &Rc<ModelAir>,
    mint: Mint<'static, ModelPools, B::Identity>,
    generation: u32,
) -> (ModelHeld<'static, B>, ModelResources<'static, B>) {
    let (
        (gate, gate_issuer, arbiter_authority),
        (tickets, terminal_access),
        small,
        large,
        [epoch_slot],
        station_vacant,
        access_point_vacant,
        block_acks,
        station_group_permission,
        (access_point_group_permission, station_commands, access_point_commands, beacon),
    ) = mint.into_pools();
    // Every token names this installation: a later one refuses it.
    let [station_group_permission] = station_group_permission.issue(generation);
    let [access_point_group_permission] = access_point_group_permission.issue(generation);
    let station_commands = station_commands.issue(generation);
    let access_point_commands = access_point_commands.issue(generation);
    let (station_association, station_bank) = station_vacant.issue(generation).into_parts();
    let (station_pairwise, station_group) = station_bank.into_parts();
    let (access_point_association, access_point_bank) =
        access_point_vacant.issue(generation).into_parts();
    let (access_point_pairwise, access_point_group) = access_point_bank.into_parts();
    air.stream.set(ModelStream::Running);
    air.receive_agreements.take().clear();
    air.associations.set(InterfaceAssociations::default());
    let network = alloc::sync::Arc::<ModelNetwork>::default();
    air.rx_network.set(Some(network.clone()));
    let held = ModelHeld {
        air: air.clone(),
        // Every received unit the caller may hold can queue as an event,
        // up to the most an air's limits allow.
        events: Bounded::new(MODEL_TERMINALS + MODEL_RX_HELD_MAX, "event"),
        quarantined: Bounded::new(1, "answer shape rejection"),
        rx_out: Cell::new(0),
        network,
        gate_issuer,
        draining: Cell::new(false),
        arenas: core::cell::RefCell::new(arena::StationArenas::new()),
        gate_closed: Cell::new(false),
        configuring_station: Cell::new(false),
    };
    let resources = ModelResources {
        shared: ModelShared {
            tickets: tickets.into_iter().collect(),
            terminal_access,
            small: small
                .into_iter()
                .map(|slot| ModelBuffer {
                    slot: Some(slot),
                    loan: None,
                    memory: Box::new([0; ModelSmall::CAPACITY]),
                    admitted_at: None,
                    admitted: None,
                })
                .collect(),
            large: large
                .into_iter()
                .map(|slot| ModelBuffer {
                    slot: Some(slot),
                    loan: None,
                    memory: Box::new([0; ModelLarge::CAPACITY]),
                    admitted_at: None,
                    admitted: None,
                })
                .collect(),
            block_acks,
            gate,
            arbiter_authority,
        },
        station: ModelStationPermissions {
            association: station_association,
            commands: station_commands,
            connection_epoch: epoch_slot,
            pairwise: station_pairwise,
            group: station_group,
            group_permission: station_group_permission,
        },
        access_point: ModelAccessPointPermissions {
            association: access_point_association,
            commands: access_point_commands,
            pairwise: access_point_pairwise,
            group: access_point_group,
            group_permission: access_point_group_permission,
            beacon: beacon.issue(generation),
        },
    };
    (held, resources)
}
