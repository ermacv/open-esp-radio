#![no_std]
use oer_protocols_ieee80211_contracts_port::*;
use oer_protocols_radio_port::{PoolPath, Slot};

pub fn admit<'c, 'id, T, M: PreparedTx<'id>, K, X, Path: PoolPath, Q: PoolPath>(
    checked: CurrentBeacon<'c, 'id, Q>,
    unchecked: BeaconPermission<'id, Q>,
    beacon: (X, Slot<'id, T, Path>, M, SupportedPower<K>, BeaconStamp<K>),
) where
    M::Types: PortTypes<'id, TicketKind = T, TicketPath = Path, Context = X>,
{
    let (context, ticket, source, power, stamp) = beacon;
    #[cfg(not(ui_reject))]
    let permission = { drop(unchecked); checked };
    #[cfg(ui_reject)]
    let permission = { drop(checked); unchecked };
    let _sealed = BeaconAttempt { context, ticket, source, power, coex_operation: TxOperation::Data, permission, stamp }.admit(); // ui-error: E0308
}
