//! Opaque worker storage is scoped apart from task-pool const evaluation.
use super::*;
pub(super) async fn run(boot: &'static mut Boot) {
    WORKER.take().future(|| worker(boot)).await;
}
type Worker = impl core::future::Future<Output = ()>;
static WORKER: static_cell::ConstStaticCell<oer_memory::StaticPlace<Worker>> =
    static_cell::ConstStaticCell::new(oer_memory::StaticPlace::new());
#[define_opaque(Worker)]
fn worker(boot: &'static mut Boot) -> Worker {
    async move {
        let timer: &'static irq::TimerPort = boot.timer.take().expect("one P2 timer handover");
        loop {
            let (command, reply) = bridge::command().await;
            if command.operation != rpc::Operation::Enable
                || boot.port_epoch.checked_add(1) != Some(command.epoch)
            {
                bridge::answer(command, reply, Err(Fault::Accounting));
                hold(boot).await;
            }
            boot.port_epoch = command.epoch;
            boot.reply = Some((command, reply));
            {
                let mut pool = core::pin::pin!(with_pool_async(async |pool| {
                    let mut epoch = core::pin::pin!(with_epoch_async(async |epoch| {
                        let mut installed =
                            core::pin::pin!(installed::run(boot, pool, epoch, timer));
                        installed.as_mut().await;
                    }));
                    epoch.as_mut().await;
                }));
                pool.as_mut().await;
            }
            // Every scoped future and its old generative authority ended;
            // the actual arena and cold owners are back in Boot.
            bridge::complete(&mut boot.reply, Ok(rpc::Completion::Done));
        }
    }
}
