#![no_std]
use oer_esp32s31_bluetooth_driver::interrupt::{
    CapturedWakeBatch, PrimaryInterruptCapture, PrimaryWakeFault, SchedulerWakeBatch,
};

pub fn classify(batch: CapturedWakeBatch) -> Result<SchedulerWakeBatch, PrimaryWakeFault> {
    batch.classify()
}

pub fn consume_capture(capture: PrimaryInterruptCapture) {
    #[cfg(not(ui_reject))]
    drop(capture);
    #[cfg(ui_reject)]
    consume_work(capture); // ui-error: E0308
}

pub fn consume_batch(batch: CapturedWakeBatch) {
    drop(batch);
    #[cfg(ui_reject)]
    drop(batch); // ui-error: E0382
}

fn consume_work(_: SchedulerWakeBatch) {}
