//! H4 on USB Serial/JTAG. All FIFO waits yield; P1 owns no radio registers.
use super::*;
use core::{future::poll_fn, ops::ControlFlow};

/// The host's console.
pub(crate) type Console = crate::usb::Console;
/// The host's entropy source.
pub(crate) type Rng = oer_esp32s31_hal::peripherals::entropy::Entropy<EmbassyClock>;
use oer_bluetooth_hci::{
    CommandCompleteStatusEvent, HardwareErrorEvent, HciControllerResponse, Opcode, OpcodeGroup,
    Status, VendorShutdownObservationEvent,
};
use oer_bluetooth_le_controller::dtm::{self, Entropy};
use oer_bluetooth_le_port::{LePhase, TxPower};
use oer_bluetooth_le_runtime::host::{self, Disposition, Transport};
use oer_esp32s31_bluetooth_driver::port::{DtmCaps, Fault};
use oer_firmware_apps_b0::{
    h4::Commands,
    rx::{self, Inbox},
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
    _pads: oer_esp32s31_hal::peripherals::usb_serial_jtag::UsbPads,
    inbox: Inbox,
    rng: Rng,
    commands: Commands,
    entropy: RandomValue,
}
impl Transport for Io {
    type Error = Fault;
    type Entropy = RandomValue;
    async fn receive(&mut self) -> Result<host::Received, Fault> {
        rx::receive(
            &mut self.console,
            &mut self.inbox,
            &mut self.commands,
            &EmbassyClock,
        )
        .await
        .map(|received| match received {
            rx::Received::Command => host::Received::Command,
            rx::Received::Framing => host::Received::Framing,
        })
        .map_err(|_| Fault::Hardware)
    }
    fn command(
        &mut self,
    ) -> Result<(oer_bluetooth_hci::HciCommandPacket<'_>, &mut RandomValue), Fault> {
        Ok((
            self.commands.packet().map_err(|_| Fault::Hardware)?,
            &mut self.entropy,
        ))
    }
    fn consume(&mut self) -> Result<(), Fault> {
        self.commands.consume().map_err(|_| Fault::Accounting)
    }
    async fn entropy(&mut self) -> Result<(), Fault> {
        self.entropy.0 = None;
        // Waiting out the read gap on the timer before the second read
        // leaves the owner's settle nothing to wait for: this path never
        // spins.
        let low = self.rng.random().to_le_bytes();
        EmbassyClock
            .wait_for(oer_esp32s31_hal::peripherals::entropy::READ_GAP)
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
struct Installed<'o, 'a, 'p: 'a>(&'o mut rp::Owner<'a, 'p, Id, Backend, rp::Enabled>);
impl<'p> host::ControllerPort<'p, rp::Root<Id>> for Installed<'_, '_, 'p> {
    type Caps = DtmCaps;
    type Fault = Fault;
    fn phase(
        &mut self,
    ) -> impl core::ops::DerefMut<
        Target: Sized + LePhase<'p, rp::Root<Id>, (), Caps = DtmCaps, Fault = Fault>,
    > {
        self.0.phase()
    }
    async fn latch(&mut self) -> rp::PortResult<(), rp::ClockError, Fault> {
        self.0.now().await.map(|result| result.map(|_| ()))
    }
}
struct Body<'a> {
    io: &'a mut Io,
    reset: Option<Reset>,
    power: TxPower,
}
impl rp::PortBody<Id, Backend> for Body<'_> {
    type Output = Result<Disposition<rp::Root<Id>>, Fault>;
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
        let outcome = host::serve(
            &mut Installed(&mut owner),
            &mut service,
            self.io,
            reset,
            STOP,
        )
        .await
        .map_err(|error| match error {
            host::Error::Transport(fault) | host::Error::Backend(fault) => fault,
            host::Error::ClockBusy => Fault::ClockBusy,
            host::Error::Accounting => Fault::Accounting,
            host::Error::Terminal(_) => Fault::Hardware,
        });
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
                    &HardwareErrorEvent::new(fault.reason_code()),
                )
                .await;
                rp::Ended::fail_stop(stopped, Err(fault))
            }
        }
    }
}
pub async fn serve(
    console: Console,
    pads: oer_esp32s31_hal::peripherals::usb_serial_jtag::UsbPads,
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
        let (next, frontend) =
            poll_fn(|cx| request.poll_answer(cx)).await.unwrap_or_else(
                oer_espressif_staged_runtime::panic::reset_request::<
                    oer_esp32s31_hal::platform::Port,
                    _,
                >,
            );
        poster = next;
        let frontend = match frontend {
            Ok(frontend) => frontend,
            Err(fault) => {
                tx::fail_stop(
                    || (),
                    &mut io.console,
                    &HardwareErrorEvent::new(fault.reason_code()),
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
                let _ = io
                    .send(&CommandCompleteStatusEvent::new(STOP, Status::SUCCESS))
                    .await;
                return;
            }
            _ => core::future::pending::<()>().await,
        }
    }
}

async fn report_shutdown(io: &mut Io, reports: &bridge::StopReports) {
    let pending = match reports.try_receive() {
        Ok(pending) => pending,
        Err(_) => {
            tx::fail_stop(
                || (),
                &mut io.console,
                &HardwareErrorEvent::new(Fault::Accounting.reason_code()),
            )
            .await;
            oer_firmware_apps_b0::handoff::park_failure((), || ()).await
        }
    };
    let report = VendorShutdownObservationEvent::new(pending);
    if io.send(&report).await.is_err() {
        oer_firmware_apps_b0::handoff::park_failure(report, || ()).await;
    }
}
