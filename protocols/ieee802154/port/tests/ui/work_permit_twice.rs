#![no_std]
use oer_ieee802154_port::{
    Interface, RequestId,
    owner::{WorkPermit, WorkPermits},
};
use oer_radio_port::PermitPool;
pub fn admit<'q, 'id, K, P: PermitPool<'id, K>>(
    pool: &WorkPermits<'id, K, P>,
    permit: WorkPermit<'q, 'id, K>,
    interface: Interface,
    id: RequestId,
) {
    let _ = pool.admit(interface, id, permit);
    #[cfg(ui_reject)]
    let _ = pool.admit(interface, id, permit); // ui-error: E0382
}
