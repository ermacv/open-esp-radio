//! H4 on USB Serial/JTAG. All FIFO waits yield; P1 owns no radio registers.
use super::*;
use core::{future::poll_fn, ops::ControlFlow};

/// The host's console.
pub(crate) type Console = crate::usb::Console;
/// The host's entropy source.
pub(crate) type Rng = oer_esp32s31_hal::platform::Entropy<EmbassyClock>;
use oer_bluetooth_hci::{
    HciControllerResponse,
    bt_hci::{
        PacketKind,
        cmd::{Cmd, Opcode, OpcodeGroup, le::LeRand},
    },
};
use oer_bluetooth_le_controller::dtm::{self, Accepted, Entropy};
use oer_bluetooth_le_port::TxPower;
use oer_esp32s31_bluetooth_driver::port::{DtmCaps, Fault};
use oer_firmware_apps_b0::{
    h4::Commands,
    rx::{self, Inbox, Received},
    tx,
};
use oer_radio_port as rp;
use oer_time::Timer;

type Id = bridge::slot::Id;
type Backend = bridge::Backend<Id>;
type Reset = dtm::Reset<rp::Root<Id>>;
const STOP: Opcode = Opcode::new(OpcodeGroup::new(63), 0);

struct RandomValue(Option<[u8; 8]>);
impl Entropy for RandomValue {
    fn random(&mut self) -> Result<[u8; 8], dtm::EntropyUnavailable> {
        self.0.take().ok_or(dtm::EntropyUnavailable)
    }
}
struct Io {
    console: Console,
    /// The console's pads, held so that nothing switches them off.
    _pads: oer_esp32s31_hal::usb_serial_jtag::UsbPads,
    inbox: Inbox,
    rng: Rng,
    commands: Commands,
    entropy: RandomValue,
}
impl Io {
    async fn receive(&mut self) -> Result<Received, Fault> {
        rx::receive(
            &mut self.console,
            &mut self.inbox,
            &mut self.commands,
            &EmbassyClock,
        )
        .await
        .map_err(|_| Fault::Hardware)
    }
    async fn entropy(&mut self) -> Result<(), Fault> {
        self.entropy.0 = None;
        // Waiting out the read gap on the timer before the second read
        // leaves the owner's settle nothing to wait for: this path never
        // spins.
        let low = self.rng.random().to_le_bytes();
        EmbassyClock
            .wait_for(oer_esp32s31_hal::platform::READ_GAP)
            .await
            .map_err(|_| Fault::Hardware)?;
        let high = self.rng.random().to_le_bytes();
        self.entropy.0 = Some([
            low[0], low[1], low[2], low[3], high[0], high[1], high[2], high[3],
        ]);
        Ok(())
    }
    async fn send(&mut self, response: &impl HciControllerResponse) -> Result<(), Fault> {
        tx::send(&mut self.console, response)
            .await
            .map_err(|_| Fault::Hardware)
    }
}
enum Disposition {
    Reset(Reset),
    Stop,
}
struct Body<'a> {
    io: &'a mut Io,
    reset: Option<Reset>,
    power: TxPower,
}
impl rp::PortBody<Id, Backend> for Body<'_> {
    type Output = Result<Disposition, Fault>;
    async fn run<'a, 'p>(
        self,
        owner: rp::Owner<'a, 'p, Id, Backend, rp::Disabled>,
        resources: <Backend as rp::PortBackend<Id>>::Resources<'p>,
    ) -> rp::Ended<Id, Backend, Self::Output>
    where
        Backend: 'p,
        Id: 'p,
        'p: 'a,
    {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(failed) => return rp::Ended::fail_stop(failed.fail_stop(), Err(Fault::Hardware)),
        };
        let [seat, _, _] = resources.seats;
        let reset = match self.reset {
            Some(reset) => match reset.after_install(&resources.session) {
                Ok(response) => Some(response),
                Err(_) => return rp::Ended::fail_stop(owner.fail_stop(), Err(Fault::Accounting)),
            },
            None => None,
        };
        let mut service =
            dtm::Service::<_, DtmCaps>::new(resources.session, seat, Some(self.power));
        let outcome = async {
            if let Some(response) = reset {
                self.io.send(&response).await?;
            }
            loop {
                if matches!(self.io.receive().await?, Received::Framing) {
                    self.io.send(&HardwareError([0x10, 1, 0xff])).await?;
                    continue;
                }
                let (stop, random) = {
                    let packet = self.io.commands.packet().map_err(|_| Fault::Hardware)?;
                    (
                        packet.opcode() == STOP && packet.parameters().is_empty(),
                        packet.opcode() == LeRand::OPCODE && packet.parameters().is_empty(),
                    )
                };
                if stop {
                    self.io.commands.consume().map_err(|_| Fault::Accounting)?;
                    return Ok(Disposition::Stop);
                }
                if random {
                    match owner.now().await {
                        Ok(Ok(_)) => {}
                        Err(poisoned) => return Err(poisoned.cause),
                        Ok(Err(_)) => return Err(Fault::ClockBusy),
                    }
                    self.io.entropy().await?;
                }
                let accepted = service
                    .accept(
                        self.io.commands.packet().map_err(|_| Fault::Hardware)?,
                        &mut self.io.entropy,
                    )
                    .map_err(|_| Fault::Accounting)?;
                self.io.commands.consume().map_err(|_| Fault::Accounting)?;
                match accepted {
                    Accepted::Reset(reset) => return Ok(Disposition::Reset(reset)),
                    Accepted::Response => {}
                    Accepted::Port => {
                        let submitted = {
                            let phase = owner.phase();
                            service.submit(&*phase).await.map_err(|error| match error {
                                dtm::SubmitError::Backend(poisoned) => poisoned.cause,
                                _ => Fault::Accounting,
                            })?
                        };
                        if submitted == dtm::Submitted::AwaitTerminal {
                            // Exercise the independent physical latch before
                            // removing this command's mandatory terminal.
                            match owner.now().await {
                                Ok(Ok(_)) => {}
                                Err(poisoned) => return Err(poisoned.cause),
                                Ok(Err(_)) => return Err(Fault::ClockBusy),
                            }
                            let mut phase = owner.phase();
                            service
                                .complete(&mut *phase)
                                .await
                                .map_err(|_| Fault::Hardware)?;
                        }
                    }
                }
                let response = service.response().ok_or(Fault::Accounting)?;
                self.io.send(response).await?;
                let _ = service.take_response();
            }
        }
        .await;
        match outcome {
            Ok(disposition) => {
                let mut sink = |_| ControlFlow::Continue(());
                match owner.disable(&mut sink).await {
                    Ok(owner) => rp::Ended::uninstall(owner, (), Ok(disposition)),
                    Err(owner) => rp::Ended::fail_stop(owner.fail_stop(), Err(Fault::Hardware)),
                }
            }
            Err(fault) => {
                let stopped = tx::fail_stop(
                    || owner.fail_stop(),
                    &mut self.io.console,
                    &HardwareError([0x10, 1, fault.reason_code()]),
                )
                .await;
                rp::Ended::fail_stop(stopped, Err(fault))
            }
        }
    }
}
struct StopComplete;
struct HardwareError([u8; 3]);
impl HciControllerResponse for HardwareError {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}
impl HciControllerResponse for StopComplete {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }
    fn as_bytes(&self) -> &[u8] {
        &[0x0e, 4, 1, 0, 0xfc, 0]
    }
}
pub async fn serve(
    console: Console,
    pads: oer_esp32s31_hal::usb_serial_jtag::UsbPads,
    rng: Rng,
    channel: bridge::FrontendChannel,
    power: TxPower,
) {
    let mut io = Io {
        console,
        _pads: pads,
        inbox: Inbox::new(),
        rng,
        commands: Commands::new(),
        entropy: RandomValue(None),
    };
    let mut poster = channel.0;
    let reports = channel.1;
    let mut joined = None;
    let mut reset = None;
    loop {
        let mut request = poster.post(joined.take());
        let (next, frontend) = poll_fn(|cx| request.poll_answer(cx)).await;
        poster = next;
        let frontend = match frontend {
            Ok(frontend) => frontend,
            Err(fault) => {
                tx::fail_stop(
                    || (),
                    &mut io.console,
                    &HardwareError([0x10, 1, fault.reason_code()]),
                )
                .await;
                core::future::pending::<()>().await;
                return;
            }
        };
        match rp::run_port(
            frontend,
            Body {
                io: &mut io,
                reset: reset.take(),
                power,
            },
        )
        .await
        {
            rp::PortEnd::Uninstalled {
                joined: proof,
                output: Ok(Disposition::Reset(token)),
            } => {
                report_shutdown(&mut io, &reports).await;
                joined = Some(proof);
                reset = Some(token);
            }
            rp::PortEnd::Uninstalled {
                output: Ok(Disposition::Stop),
                ..
            } => {
                report_shutdown(&mut io, &reports).await;
                let _ = io.send(&StopComplete).await;
                return;
            }
            _ => core::future::pending::<()>().await,
        }
    }
}

async fn report_shutdown(io: &mut Io, reports: &bridge::StopReports) {
    use oer_firmware_apps_b0::shutdown_observation::ShutdownObservation;
    let pending = match reports.try_receive() {
        Ok(pending) => pending,
        Err(_) => {
            tx::fail_stop(
                || (),
                &mut io.console,
                &HardwareError([0x10, 1, Fault::Accounting.reason_code()]),
            )
            .await;
            oer_firmware_apps_b0::handoff::park_failure((), || ()).await
        }
    };
    let report = ShutdownObservation::new(pending);
    if io.send(&report).await.is_err() {
        oer_firmware_apps_b0::handoff::park_failure(report, || ()).await;
    }
}
