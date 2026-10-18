use std::boxed::Box;

use oer_esp32s31_ieee80211_dma::{
    rx_dma::model::RxWalkerModel,
    rx_ring::RxDmaArenaState,
    space::{HOST_ARENA_BASE, HostDma},
};
use oer_esp32s31_ieee80211_mac::irq::model::{MacInterruptModel, PowerInterruptModel};

use super::*;
use crate::rx::{counters::WifiRxReport, frontier::ReceiveFrontier};

type HostEpoch = ActiveEpoch<Moderated, MacInterruptModel, PowerInterruptModel>;
type HostArena = WifiRxArena<32, 1_664, 1_668, 32, HostDma, HostEpoch>;

#[test]
fn arena_zero_bytes_bind_and_descriptors_do_not_start_the_arena() {
    let arena: &'static mut HostArena = Box::leak(Box::new(bytemuck::Zeroable::zeroed()));
    let start = core::ptr::from_ref(&*arena).addr();
    let end = start + core::mem::size_of::<HostArena>();

    let (parts, mut stop) = arena.bind().expect("a zeroed arena binds");

    let descriptors = parts.rx().descriptors().as_ptr().addr();
    assert!(descriptors > start && descriptors < end);
    assert_eq!(parts.descriptor_base(), HOST_ARENA_BASE);
    assert_eq!(parts.rx().lifecycle_state(), RxDmaArenaState::Reusable);
    assert_eq!(parts.counters().report(), WifiRxReport::default());
    assert!(!parts.requests().is_pending());
    assert_eq!(parts.stage().claimed_slots(), 0);
    assert!(stop.try_exit().is_none());
    assert!(!parts.control.stop_requested());
    assert!(!parts.irq.has_pending() && !parts.power.has_pending());
    // Every buffer address is laid out inside the arena's address space.
    for (index, &address) in parts.buffer_addresses.iter().enumerate() {
        assert_ne!(address, 0, "buffer {index}");
    }

    // The zeroed descriptors and addresses make a ring.
    let mut walker = RxWalkerModel::new();
    let frontier = ReceiveFrontier::<32, 1_664>::prepare_initial(
        &mut walker,
        parts.rx,
        parts.descriptor_base,
        parts.buffer_addresses,
    );
    assert!(frontier.is_ok());
    assert_eq!(parts.rx().lifecycle_state(), RxDmaArenaState::Prepared);

    stop.request_stop();
    assert!(parts.control.stop_requested());

    // The line lands in the arena's own slot.
    let cell: &'static oer_memory::HandlerCell<WifiIrqLine<HostEpoch>> =
        Box::leak(Box::new(oer_memory::HandlerCell::new()));
    let epoch = ActiveEpoch::for_host_test(MacInterruptModel::new(), PowerInterruptModel::new());
    let Ok(installed) = parts.line.install(epoch, cell) else {
        panic!("an empty cell takes the line");
    };
    assert_eq!(installed.cell().with(|line| line.masked()), Some(false));
}
