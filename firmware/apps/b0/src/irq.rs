use super::*;
use oer_espressif_executor::{Doorbell, DoorbellAcknowledger, SameLevel};
use oer_memory::HandlerCell;
oer_espressif_staged_runtime::placed_static! {
    static ACK:HandlerCell<DoorbellAcknowledger<3>>=HandlerCell::new(),in critical_data "b0";
    static ACK_STORE:StaticCell<DoorbellAcknowledger<3>>=StaticCell::new(),in critical_data "b0";
}
static BELL: StaticCell<Doorbell<3>> = StaticCell::new();
pub fn initialize(soft: SoftwareInterrupt<'static, 3>, token: DoorbellToken) {
    let (bell, ack) = Doorbell::new(
        soft,
        token,
        SameLevel::<RadioAlarmToken, DoorbellToken>::new(),
    );
    let _ = ACK
        .install(ACK_STORE.init(ack))
        .ok()
        .expect("one radio doorbell acknowledger");
    BELL.init(bell);
    ACK.with(|ack| ack.enable())
        .expect("published doorbell")
        .expect("correct shared route");
}
pub fn doorbell() {
    let _ = ACK.with(|ack| ack.acknowledge());
}
