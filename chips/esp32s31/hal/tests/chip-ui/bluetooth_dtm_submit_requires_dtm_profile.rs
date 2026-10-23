#![no_std]
use oer_esp32s31_hal::bluetooth::engine::{
    ArenaMemory, Engine, SchedulerRoleInstance,
    scheduler::interrupt::SchedulerDoorbell,
};

pub fn dtm<'pool, 'epoch, D: SchedulerDoorbell>(
    engine: &mut Engine<ArenaMemory<'pool, 'epoch, 0, 0, 0, 0, 0, 0, 1>>,
    instance: &SchedulerRoleInstance<'pool, 'epoch>,
    doorbell: &D,
) {
    let _ = engine.submit_dtm(instance, doorbell);
}

pub fn shared<'pool, 'epoch, D: SchedulerDoorbell>(
    engine: &mut Engine<ArenaMemory<'pool, 'epoch, 1, 1, 1, 1, 2, 2, 4>>,
    instance: &SchedulerRoleInstance<'pool, 'epoch>,
    doorbell: &D,
) {
    let _ = engine.submit(instance, 0, doorbell);
    #[cfg(ui_reject)]
    let _ = engine.submit_dtm(instance, doorbell); // ui-error: E0599
}
