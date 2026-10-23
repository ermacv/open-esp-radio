//! H4 on USB Serial/JTAG. All FIFO waits yield; P1 owns no radio registers.
use super::*;
use core::{future::poll_fn, ops::ControlFlow};
use esp_hal::{peripherals::RNG, usb::usb_serial_jtag::UsbSerialJtag};
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
use oer_firmware_apps_b0::h4::Commands;
use oer_radio_port as rp;
use oer_time::{Duration, Timer};

type Id = bridge::slot::Id;
type Backend = bridge::Backend<Id>;
type Reset = dtm::Reset<rp::Root<Id>>;
const STOP: Opcode = Opcode::new(OpcodeGroup::new(63), 0);
const POLL: Duration = Duration::from_millis(1);

struct RandomValue(Option<[u8; 8]>);
impl Entropy for RandomValue {
    fn random(&mut self) -> Result<[u8; 8], dtm::EntropyUnavailable> {
        self.0.take().ok_or(dtm::EntropyUnavailable)
    }
}
struct Io {
    console: UsbSerialJtag<'static, esp_hal::Blocking>,
    rng: RNG<'static>,
    commands: Commands,
    entropy: RandomValue,
    sent: usize,
}
impl Io {
    async fn receive(&mut self) -> Result<(), Fault> {
        while !self.commands.complete() {
            match self.console.read_byte() {
                Ok(byte) => {
                    self.commands.push(byte).map_err(|_| Fault::Hardware)?;
                }
                Err(nb::Error::WouldBlock) => EmbassyClock
                    .wait_for(POLL)
                    .await
                    .map_err(|_| Fault::Hardware)?,
                Err(nb::Error::Other(_)) => return Err(Fault::Hardware),
            }
        }
        Ok(())
    }
    async fn entropy(&mut self) -> Result<(), Fault> {
        self.entropy.0 = None;
        // SOURCE: pinned esp-hal rng::ll reads CRC_SYNC_DATA on S31 and
        // separates words by >=16 APB cycles. The RF-enabled owner is live
        // here. One microsecond exceeds that spacing at this board's APB;
        // unlike RngOwner::read, this owned PAC path never spins.
        let low = self
            .rng
            .register_block()
            .crc_sync_data()
            .read()
            .bits()
            .to_le_bytes();
        EmbassyClock
            .wait_for(Duration::from_micros(1))
            .await
            .map_err(|_| Fault::Hardware)?;
        let high = self
            .rng
            .register_block()
            .crc_sync_data()
            .read()
            .bits()
            .to_le_bytes();
        self.entropy.0 = Some([
            low[0], low[1], low[2], low[3], high[0], high[1], high[2], high[3],
        ]);
        Ok(())
    }
    async fn send(&mut self, response: &impl HciControllerResponse) -> Result<(), Fault> {
        let body = response.as_bytes();
        while self.sent <= body.len() {
            let byte = if self.sent == 0 {
                response.kind() as u8
            } else {
                *body.get(self.sent - 1).ok_or(Fault::Accounting)?
            };
            match self.console.write_byte_nb(byte) {
                Ok(()) => self.sent += 1,
                Err(nb::Error::WouldBlock) => EmbassyClock
                    .wait_for(POLL)
                    .await
                    .map_err(|_| Fault::Hardware)?,
                Err(nb::Error::Other(_)) => return Err(Fault::Hardware),
            }
        }
        loop {
            match self.console.flush_tx_nb() {
                Ok(()) => {
                    self.sent = 0;
                    return Ok(());
                }
                Err(nb::Error::WouldBlock) => EmbassyClock
                    .wait_for(POLL)
                    .await
                    .map_err(|_| Fault::Hardware)?,
                Err(nb::Error::Other(_)) => return Err(Fault::Hardware),
            }
        }
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
                self.io.receive().await?;
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
                        _ => return Err(Fault::Hardware),
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
                        {
                            let phase = owner.phase();
                            service.submit(&*phase).await.map_err(|_| Fault::Hardware)?;
                        }
                        // Exercise the independent physical latch before
                        // removing this command's mandatory terminal.
                        match owner.now().await {
                            Ok(Ok(_)) => {}
                            _ => return Err(Fault::Hardware),
                        }
                        let mut phase = owner.phase();
                        service
                            .complete(&mut *phase)
                            .await
                            .map_err(|_| Fault::Hardware)?;
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
            Err(fault) => rp::Ended::fail_stop(owner.fail_stop(), Err(fault)),
        }
    }
}
struct StopComplete;
impl HciControllerResponse for StopComplete {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }
    fn as_bytes(&self) -> &[u8] {
        &[0x0e, 4, 1, 0, 0xfc, 0]
    }
}
#[embassy_executor::task]
pub async fn serve(
    console: UsbSerialJtag<'static, esp_hal::Blocking>,
    rng: RNG<'static>,
    channel: bridge::FrontendChannel,
    power: TxPower,
) {
    let mut io = Io {
        console,
        rng,
        commands: Commands::new(),
        entropy: RandomValue(None),
        sent: 0,
    };
    let mut poster = channel.0;
    let mut joined = None;
    let mut reset = None;
    loop {
        let mut request = poster.post(joined.take());
        let (next, frontend) = poll_fn(|cx| request.poll_answer(cx)).await;
        poster = next;
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
                joined = Some(proof);
                reset = Some(token);
            }
            rp::PortEnd::Uninstalled {
                output: Ok(Disposition::Stop),
                ..
            } => {
                let _ = io.send(&StopComplete).await;
                return;
            }
            _ => core::future::pending::<()>().await,
        }
    }
}
