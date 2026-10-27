use std::boxed::Box;

use oer_esp32s31_hal::ieee80211::dma::{
    rx_dma::model::RxWalkerModel,
    rx_ring::RxDmaArenaState,
    space::{HOST_ARENA_BASE, HostDma},
};

use oer_time_virtual::VirtualClock;

use super::*;
use crate::rx::{
    counters::WifiRxReport,
    frontier::ReceiveFrontier,
    transaction::Publisher,
    upper::{QueueSink, RxFrameEnds},
};

type HostStatic =
    WifiRxStatic<32, 1_664, 1_664, 32, oer_espressif_interrupt_table::signal::host::Host, HostDma>;

#[test]
fn the_zero_bytes_bind_with_the_storage_placed_inside_the_static() {
    let placed: &'static mut HostStatic = Box::leak(Box::new(bytemuck::Zeroable::zeroed()));
    let start = core::ptr::from_ref(&*placed).addr();
    let end = start + core::mem::size_of::<HostStatic>();

    let (parts, mut stop, frames) = WifiRxArena::bind(
        oer_esp32s31_hal::dma_static::Placed::host(placed),
        crate::rx::arena::WifiIrqSignals {
            mac: oer_espressif_interrupt_table::signal::host::signal(),
            power: oer_espressif_interrupt_table::signal::host::signal(),
        },
    )
    .expect("a zeroed static binds");

    let descriptors = parts.rx().descriptors().as_ptr().addr();
    assert!(descriptors >= start && descriptors < end);
    assert_eq!(parts.descriptor_base(), HOST_ARENA_BASE);
    assert_eq!(parts.rx().lifecycle_state(), RxDmaArenaState::Reusable);
    assert_eq!(parts.counters().report(), WifiRxReport::default());
    assert!(!parts.requests().is_pending());
    assert_eq!(parts.stage().claimed_slots(), 0);
    // The staged-frame queue is empty and as deep as the staging pool.
    let RxFrameEnds {
        sender,
        receiver: _,
    } = frames;
    let clock: &'static VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let sink = QueueSink::new(sender, clock);
    assert_eq!(Publisher::<'_, 1_664, 32>::free_capacity(&sink), 32);
    assert!(stop.try_exit().is_none());
    assert!(!parts.control.stop_requested());
    assert_eq!(
        (parts.irq.has_pending(), parts.power.has_pending()),
        (Ok(false), Ok(false))
    );
    // Every buffer address is laid out inside the arena's address space.
    for (index, &address) in parts.rx().buffer_addresses().iter().enumerate() {
        assert_ne!(address, 0, "buffer {index}");
    }

    // The zeroed descriptors and addresses make a ring.
    let mut walker = RxWalkerModel::new();
    let rx_storage = parts.rx();
    let frontier = ReceiveFrontier::<32, 1_664>::from_prepared(
        parts
            .rx
            .prepare_ring(&mut walker)
            .unwrap()
            .core_ready_for_test()
            .publish(&mut walker)
            .unwrap(),
    );
    assert_eq!(
        frontier.phase(),
        crate::rx::frontier::RxFrontierPhase::Prepared
    );
    assert_eq!(rx_storage.lifecycle_state(), RxDmaArenaState::Prepared);

    stop.request_stop();
    assert!(parts.control.stop_requested());

    // The line lands in the arena's own slot.
    let cell: &'static oer_memory::HandlerCell<
        WifiIrqLine<oer_espressif_interrupt_table::signal::host::Host>,
    > = Box::leak(Box::new(oer_memory::HandlerCell::new()));
    let Ok(installed) = parts.line.install(cell) else {
        panic!("an empty cell takes the line");
    };
    assert_eq!(installed.cell().with(|_| ()), Some(()));
}
