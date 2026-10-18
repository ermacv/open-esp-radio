//! One TCP echo session and its acknowledged close/reset boundary.

use crate::network::TcpSocket;
use embassy_time::{Duration, with_timeout};

const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) enum Completion {
    Closed,
    Aborted,
    ResetUnconfirmed,
}

// A session owns protocol-independent byte echo and the close/abort decision.
// The concrete socket supplies the bounded transport drain.
trait Connection {
    async fn read(&mut self, bytes: &mut [u8]) -> Result<usize, ()>;
    async fn write(&mut self, bytes: &[u8]) -> Result<usize, ()>;
    fn close(&mut self);
    fn abort(&mut self);
    async fn drain(&mut self) -> bool;
}

impl Connection for TcpSocket<'_, '_> {
    async fn read(&mut self, bytes: &mut [u8]) -> Result<usize, ()> {
        TcpSocket::read(self, bytes).await.map_err(|_| ())
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<usize, ()> {
        TcpSocket::write(self, bytes).await.map_err(|_| ())
    }

    fn close(&mut self) {
        TcpSocket::close(self);
    }

    fn abort(&mut self) {
        TcpSocket::abort(self);
    }

    async fn drain(&mut self) -> bool {
        matches!(with_timeout(DRAIN_TIMEOUT, self.flush()).await, Ok(Ok(())))
    }
}

pub(super) async fn serve(socket: &mut TcpSocket<'_, '_>, packet: &mut [u8]) -> Completion {
    session(socket, packet).await
}

async fn session(socket: &mut impl Connection, packet: &mut [u8]) -> Completion {
    let graceful = loop {
        let length = match socket.read(packet).await {
            Ok(0) => break true,
            Ok(length) => length,
            Err(()) => break false,
        };
        let mut written = 0;
        while written < length {
            match socket.write(&packet[written..length]).await {
                Ok(0) | Err(()) => break,
                Ok(count) => written += count,
            }
        }
        if written != length {
            break false;
        }
    };
    if graceful {
        // A peer FIN closes only its sending half. Our queued echo must still
        // be delivered and acknowledged before this socket is reused.
        socket.close();
        if socket.drain().await {
            return Completion::Closed;
        }
    }
    socket.abort();
    if socket.drain().await {
        Completion::Aborted
    } else {
        Completion::ResetUnconfirmed
    }
}

#[cfg(test)]
mod tests;
