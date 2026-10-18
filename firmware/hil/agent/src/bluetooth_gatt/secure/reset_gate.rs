//! HIL-only reader suspension around the real final Reset command.
//!
//! No responses are synthesized or discarded. The sole shutdown reader is
//! gated before calling the real facade, so its command response stays in the
//! original transport. The application and old Host reader must already be
//! dropped; arming during bootstrap or with another concurrent reader is not
//! supported. Cancellation never releases the gate. There is no timer/reset
//! dependency, and the hardware runner must continue polling independently.
//! At the reached gate HIL may instead request a distinct terminal read error.
//! That error never consumes the queued response or releases a physical owner.
use bt_hci::{
    ControllerToHostPacket,
    cmd::{self, Cmd, controller_baseband::Reset},
    controller::{Controller, ControllerCmdAsync, ControllerCmdSync},
    data::{AclPacket, IsoPacket, SyncPacket},
};
use core::{
    cell::{Cell, RefCell},
    future::poll_fn,
    task::Poll,
};
use embassy_sync::waitqueue::WakerRegistration;
use embedded_io_async::{Error as IoError, ErrorKind, ErrorType};
use oer_hil_protocol::bluetooth::BluetoothGattResetReadGate as Phase;

pub struct Gate {
    phase: Cell<Phase>,
    waker: RefCell<WakerRegistration>,
}

impl Default for Gate {
    fn default() -> Self {
        Self::new()
    }
}

impl Gate {
    pub fn new() -> Self {
        Self {
            phase: Cell::new(Phase::Disabled),
            waker: RefCell::new(WakerRegistration::new()),
        }
    }
    pub fn phase(&self) -> Phase {
        self.phase.get()
    }
    pub fn arm(&self) -> bool {
        if self.phase.get() != Phase::Disabled {
            return false;
        }
        self.phase.set(Phase::Armed);
        true
    }
    pub fn release(&self) -> bool {
        if self.phase.get() != Phase::ReaderHeld {
            return false;
        }
        self.phase.set(Phase::Released);
        self.waker.borrow_mut().wake();
        true
    }
    pub fn fail_read(&self) -> bool {
        if self.phase.get() != Phase::ReaderHeld {
            return false;
        }
        self.phase.set(Phase::FailureRequested);
        self.waker.borrow_mut().wake();
        true
    }
    fn reset_entered(&self) {
        if self.phase.get() == Phase::Armed {
            self.phase.set(Phase::ResetEntered);
        }
    }
    async fn before_read(&self) -> Result<(), ()> {
        poll_fn(|cx| match self.phase.get() {
            Phase::ResetEntered | Phase::ReaderHeld => {
                self.phase.set(Phase::ReaderHeld);
                self.waker.borrow_mut().register(cx.waker());
                Poll::Pending
            }
            Phase::FailureRequested | Phase::ReadFailed => {
                self.phase.set(Phase::ReadFailed);
                Poll::Ready(Err(()))
            }
            _ => Poll::Ready(Ok(())),
        })
        .await
    }
}

/// Preserve real transport errors separately from the diagnostic failure.
#[derive(Debug)]
pub enum ReadError<E> {
    Transport(E),
    Injected,
}
impl<E: core::fmt::Debug> core::fmt::Display for ReadError<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl<E: core::fmt::Debug> core::error::Error for ReadError<E> {}
impl<E: IoError> IoError for ReadError<E> {
    fn kind(&self) -> ErrorKind {
        match self {
            Self::Transport(error) => error.kind(),
            Self::Injected => ErrorKind::Other,
        }
    }
}

fn command_error<E>(error: cmd::Error<E>) -> cmd::Error<ReadError<E>> {
    match error {
        cmd::Error::Io(error) => cmd::Error::Io(ReadError::Transport(error)),
        cmd::Error::Hci(error) => cmd::Error::Hci(error),
    }
}

/// Owns the original facade; the borrowed gate cannot outlive its application.
pub struct GatedController<'a, C> {
    pub inner: C,
    pub gate: &'a Gate,
}

impl<C: ErrorType> ErrorType for GatedController<'_, C> {
    type Error = ReadError<C::Error>;
}

impl<C: Controller> Controller for GatedController<'_, C> {
    type Buffer<'a> = C::Buffer<'a>;
    fn alloc_buf(&self) -> Result<Self::Buffer<'_>, Self::Error> {
        self.inner.alloc_buf().map_err(ReadError::Transport)
    }
    async fn read<'a>(
        &self,
        buf: &'a mut Self::Buffer<'_>,
    ) -> Result<ControllerToHostPacket<'a>, Self::Error> {
        self.gate
            .before_read()
            .await
            .map_err(|()| ReadError::Injected)?;
        self.inner.read(buf).await.map_err(ReadError::Transport)
    }
    async fn write_acl_data(&self, p: &AclPacket<'_>) -> Result<(), Self::Error> {
        self.inner
            .write_acl_data(p)
            .await
            .map_err(ReadError::Transport)
    }
    async fn write_sync_data(&self, p: &SyncPacket<'_>) -> Result<(), Self::Error> {
        self.inner
            .write_sync_data(p)
            .await
            .map_err(ReadError::Transport)
    }
    async fn write_iso_data(&self, p: &IsoPacket<'_>) -> Result<(), Self::Error> {
        self.inner
            .write_iso_data(p)
            .await
            .map_err(ReadError::Transport)
    }
}

impl<C, T> ControllerCmdSync<T> for GatedController<'_, C>
where
    C: ControllerCmdSync<T>,
    T: cmd::SyncCmd + ?Sized,
{
    async fn exec(&self, command: &T) -> Result<T::Return, cmd::Error<Self::Error>> {
        if T::OPCODE == Reset::OPCODE {
            self.gate.reset_entered();
        }
        self.inner.exec(command).await.map_err(command_error)
    }
}

impl<C, T> ControllerCmdAsync<T> for GatedController<'_, C>
where
    C: ControllerCmdAsync<T>,
    T: cmd::AsyncCmd + ?Sized,
{
    async fn exec(&self, command: &T) -> Result<(), cmd::Error<Self::Error>> {
        self.inner.exec(command).await.map_err(command_error)
    }
}
