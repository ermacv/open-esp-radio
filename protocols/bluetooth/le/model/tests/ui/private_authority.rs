use oer_bluetooth_le_model::{ModelPhase, Resources};
use oer_bluetooth_le_port::{Command, DtmCommand, LeOperationId, LePhase, Receive, Request, Supported, TestPhy};
use oer_bluetooth_le_wire::{TestChannel};
use oer_radio_port::{PortContext, SlotIdentity};
pub async fn submit<'p, Id: SlotIdentity + 'p>(phase: &ModelPhase<'_, 'p, Id, ()>, resources: Resources<'p, Id>) {
    #[cfg(ui_reject)]
    let _loan = phase.view.loan; // ui-error: E0616
    #[cfg(not(ui_reject))]
    {
        let Resources { seats: [mut seat, ..], session } = resources;
        let capacity = phase.take(&mut seat).await;
        let config = Supported::new(Receive { channel: TestChannel::new(19).expect("test channel"), phy: TestPhy::Le1M }).expect("supported test RX");
        drop(phase.submit(Request { id: LeOperationId(1), capacity, command: Command::DirectTest(DtmCommand::Receive(session, config)), context: PortContext::new(()) }));
    }
}
