//! A backend for host domains that never acquire platform clocks.

use oer_esp32s31_hal::power::{
    PlatformClock, PlatformClockAbandonment, PlatformClockBackend, PlatformClockError,
};

pub(crate) struct TestClockBackend;

impl PlatformClockBackend for TestClockBackend {
    fn acquire(_: PlatformClock) -> Result<(), PlatformClockError> {
        panic!("host domain must not acquire platform clocks");
    }
    fn release(_: PlatformClock) -> Result<(), PlatformClockError> {
        panic!("host domain must not release platform clocks");
    }
    fn abandon(_: PlatformClock, _: PlatformClockAbandonment) {
        panic!("host domain must not abandon platform clocks");
    }
}

pub(crate) type ModelMemory =
    oer_esp32s31_hal::bluetooth::engine::ArenaMemory<'static, 'static, 0, 1, 2, 1, 4, 4, 8>;

pub(crate) fn engine(
    clocked: oer_esp32s31_hal::bluetooth::ClockedOwner,
) -> (
    oer_esp32s31_hal::bluetooth::engine::Engine<ModelMemory>,
    oer_esp32s31_hal::bluetooth::InterruptSetupOwner,
) {
    use bytemuck::Zeroable;
    use oer_esp32s31_hal::{bluetooth::engine::*, memory::dma_static::Placed};
    use std::boxed::Box;
    let arena = Box::leak(Box::new(BluetoothArena::<0, 1, 2, 1, 4, 4>::zeroed()));
    Engine::for_host_test::<HostDma<0x2f00_1000>>(
        clocked,
        Placed::host(arena),
        SchedulerAllocationConfig::new(1, 2, 0).expect("model capacities"),
    )
    .ok()
    .expect("model arena")
}
