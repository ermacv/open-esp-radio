//! Concrete factory owns the actual generative tokens, with no erased call.
use super::*;
use oer_esp32s31_bluetooth_driver::radio::ControllerFactory;
type FactoryError = (ArenaError, ClockedOwner, Placed<Arena>);
pub struct Factory<'p, 'e> {
    arena: Placed<Arena>,
    pool: PoolScope<'p>,
    epoch: EpochScope<'e>,
}
impl<'p, 'e> ControllerFactory<Memory<'p, 'e>> for Factory<'p, 'e> {
    type Error = FactoryError;
    fn build(
        self,
        clocked: ClockedOwner,
    ) -> Result<(Engine<Memory<'p, 'e>>, InterruptSetupOwner), FactoryError> {
        Engine::new::<ChipDma>(
            clocked,
            self.arena,
            self.pool,
            self.epoch,
            SchedulerAllocationConfig::new(0, 0, 0).expect("DTM-only capacities"),
        )
    }
}
pub type Life<'p, 'e> =
    ControllerLifecycle<Factory<'p, 'e>, FactoryError, X, Memory<'p, 'e>, irq::Bell, 1>;
pub fn new<'p, 'e>(
    arena: Placed<Arena>,
    pool: PoolScope<'p>,
    epoch: EpochScope<'e>,
    gate: irq::Gate,
    address: [u8; 6],
) -> Life<'p, 'e> {
    ControllerLifecycle::new(
        Factory { arena, pool, epoch },
        ControllerRuntimeResources::new(&irq::CELLS),
        &irq::CELLS,
        gate,
        oer_bluetooth_hci::BluetoothPublicDeviceAddress::from_canonical_bytes(address),
    )
}
