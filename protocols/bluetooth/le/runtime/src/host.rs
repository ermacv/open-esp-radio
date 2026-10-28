//! One-credit HCI service loop over a composition-owned transport and port.

use oer_bluetooth_hci::{
    HciCommandPacket, HciControllerResponse,
    bt_hci::{
        PacketKind,
        cmd::{Cmd, Opcode, le::LeRand},
    },
};
use oer_bluetooth_le_controller::dtm::{
    Accepted, Entropy, Reset, Response, Service, SubmitError, Submitted,
};
use oer_bluetooth_le_port::{LeCapabilities, LePhase};
use oer_radio_port::{ClockError, PoolPath, PortResult};

/// A complete command or a framing failure reported by the transport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Received {
    Command,
    Framing,
}

/// Composition-owned HCI framing, output and entropy. Futures bind no executor.
pub trait Transport {
    type Error;
    type Entropy: Entropy;
    /// Wait for a complete command, retaining any trailing input bytes.
    fn receive(&mut self) -> impl core::future::Future<Output = Result<Received, Self::Error>>;
    /// Borrow the complete command and the separately owned entropy value.
    fn command(&mut self) -> Result<(HciCommandPacket<'_>, &mut Self::Entropy), Self::Error>;
    /// Consume the command only after the core has accepted it.
    fn consume(&mut self) -> Result<(), Self::Error>;
    /// Prepare entropy after the independent physical clock latch.
    fn entropy(&mut self) -> impl core::future::Future<Output = Result<(), Self::Error>>;
    /// Confirm delivery of one immutable HCI packet.
    fn send(
        &mut self,
        response: &impl HciControllerResponse,
    ) -> impl core::future::Future<Output = Result<(), Self::Error>>;
}

/// Sequential access to the composition's enabled port owner. A phase loan
/// ends before the independent physical latch, as it does in B0.
pub trait ControllerPort<'p, P: PoolPath> {
    type Caps: LeCapabilities;
    type Fault;
    fn phase(
        &mut self,
    ) -> impl core::ops::DerefMut<
        Target: Sized + LePhase<'p, P, (), Caps = Self::Caps, Fault = Self::Fault>,
    >;
    fn latch(
        &mut self,
    ) -> impl core::future::Future<Output = PortResult<(), ClockError, Self::Fault>>;
}

/// Leave the service body; the composition disables and uninstalls the port.
pub enum Disposition<P: PoolPath> {
    Reset(Reset<P>),
    Stop,
}

/// Failure observations returned to the image's shutdown/fail-stop policy.
#[derive(Debug)]
pub enum Error<E, F> {
    Transport(E),
    Backend(F),
    ClockBusy,
    Accounting,
    Terminal(oer_bluetooth_le_controller::dtm::Fault),
}

struct FramingError;
impl HciControllerResponse for FramingError {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }
    fn as_bytes(&self) -> &[u8] {
        &[0x10, 1, 0xff]
    }
}

/// Serve commands in B0's order: read, optional latch/entropy, accept/consume,
/// submit, physical latch, terminal, send, then release the command credit.
/// Installation, disable/uninstall and failure policy remain in the image.
pub async fn serve<'p, P, R, T>(
    port: &mut R,
    service: &mut Service<'p, P, R::Caps>,
    io: &mut T,
    reset: Option<Response>,
    stop: Opcode,
) -> Result<Disposition<P>, Error<T::Error, R::Fault>>
where
    P: PoolPath,
    R: ControllerPort<'p, P>,
    T: Transport,
{
    if let Some(response) = reset {
        io.send(&response).await.map_err(Error::Transport)?;
    }
    loop {
        if io.receive().await.map_err(Error::Transport)? == Received::Framing {
            io.send(&FramingError).await.map_err(Error::Transport)?;
            continue;
        }
        let (stopping, random) = {
            let (packet, _) = io.command().map_err(Error::Transport)?;
            (
                packet.opcode() == stop && packet.parameters().is_empty(),
                packet.opcode() == LeRand::OPCODE && packet.parameters().is_empty(),
            )
        };
        if stopping {
            io.consume().map_err(Error::Transport)?;
            return Ok(Disposition::Stop);
        }
        if random {
            match port.latch().await {
                Ok(Ok(_)) => {}
                Err(poisoned) => return Err(Error::Backend(poisoned.cause)),
                Ok(Err(_)) => return Err(Error::ClockBusy),
            }
            io.entropy().await.map_err(Error::Transport)?;
        }
        let (packet, entropy) = io.command().map_err(Error::Transport)?;
        let accepted = service
            .accept(packet, entropy)
            .map_err(|_| Error::Accounting)?;
        io.consume().map_err(Error::Transport)?;
        match accepted {
            Accepted::Reset(reset) => return Ok(Disposition::Reset(reset)),
            Accepted::Response => {}
            Accepted::Port => {
                let submitted = {
                    let phase = port.phase();
                    crate::dtm::submit(service, &*phase)
                        .await
                        .map_err(|error| match error {
                            SubmitError::Backend(poisoned) => Error::Backend(poisoned.cause),
                            SubmitError::NoCommand
                            | SubmitError::Poisoned
                            | SubmitError::Receipt
                            | SubmitError::InvalidState { .. } => Error::Accounting,
                        })?
                };
                if submitted == Submitted::AwaitTerminal {
                    match port.latch().await {
                        Ok(Ok(_)) => {}
                        Err(poisoned) => return Err(Error::Backend(poisoned.cause)),
                        Ok(Err(_)) => return Err(Error::ClockBusy),
                    }
                    let mut phase = port.phase();
                    crate::dtm::complete(service, &mut *phase)
                        .await
                        .map_err(Error::Terminal)?;
                }
            }
        }
        let response = service.response().ok_or(Error::Accounting)?;
        io.send(response).await.map_err(Error::Transport)?;
        let _ = service.take_response();
    }
}

#[cfg(test)]
mod tests;
