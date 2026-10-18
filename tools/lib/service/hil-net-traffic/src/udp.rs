//! Host UDP socket setup shared by throughput qualifiers.

use rustix::{
    io::Errno,
    net::{
        RecvFlags, recv,
        sockopt::{set_socket_recv_buffer_size, socket_recv_buffer_size},
    },
};
use std::{io, net::UdpSocket};

/// A saturated board TX stream can exceed 90 Mbit/s.  The Linux default
/// receive queue is commonly only 212,992 bytes, which is short enough to
/// overflow during an ordinary scheduler pause and falsely attribute host
/// loss to the radio driver.
pub const QUALIFICATION_RECEIVE_BUFFER_BYTES: usize = 4 * 1024 * 1024;

/// Request and read back a qualification-sized UDP receive queue.
///
/// Linux reports twice the requested `SO_RCVBUF` value because the returned
/// size includes kernel bookkeeping.  Callers retain the read-back value as
/// evidence instead of assuming that the request was accepted.
pub fn configure_qualification_receive_buffer(socket: &UdpSocket) -> io::Result<usize> {
    set_socket_recv_buffer_size(socket, QUALIFICATION_RECEIVE_BUFFER_BYTES)?;
    let actual = socket_recv_buffer_size(socket)?;
    if actual == 0 {
        return Err(io::Error::other("invalid SO_RCVBUF read-back"));
    }
    Ok(actual)
}

/// Linux exports the socket's cumulative drop count, including a final lost
/// packet with no subsequent ancillary message. Read before and after collection.
#[cfg(target_os = "linux")]
pub fn kernel_drops(socket: &UdpSocket) -> io::Result<Option<u32>> {
    oer_hil_fixture::linux_socket::dropped_packets(socket).map(Some)
}

#[cfg(not(target_os = "linux"))]
pub fn kernel_drops(_socket: &UdpSocket) -> io::Result<Option<u32>> {
    Ok(None)
}

/// Confirm the exact UDP flow before load can evict unresolved-neighbor packets.
/// A retry deadline is failure recovery, never an assumed readiness delay.
pub fn confirm_reverse_flow(socket: &UdpSocket, timeout: std::time::Duration) -> crate::Result<()> {
    use oer_hil_link::transport::events::{EventPoll, deadline_after};
    use oer_hil_protocol::network::UdpProbe;
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nonce = (SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos() as u64)
        .wrapping_add(NEXT.fetch_add(1, Ordering::Relaxed));
    let request = UdpProbe {
        nonce,
        response: false,
    }
    .encode();
    let expected = UdpProbe {
        nonce,
        response: true,
    };
    let mut poll = EventPoll::new()?;
    poll.register(socket, false)?;
    let deadline = deadline_after(timeout);
    let mut next_send = Instant::now();
    let mut packet = [0_u8; UdpProbe::LENGTH + 1];
    loop {
        oer_process::check_cancelled()?;
        let now = Instant::now();
        if now >= deadline {
            return Err(format!(
                "UDP reverse-flow probe to {} timed out without its matching response",
                socket.peer_addr()?
            )
            .into());
        }
        if now >= next_send {
            socket.send(&request)?;
            next_send = now + Duration::from_millis(250);
        }
        // Per-call nonblocking receive preserves the socket's later send mode.
        match recv(socket, &mut packet[..], RecvFlags::DONTWAIT) {
            Ok((length, _)) => {
                if UdpProbe::decode(&packet[..length]) == Some(expected) {
                    return Ok(());
                }
                continue;
            }
            Err(Errno::INTR) => continue,
            Err(Errno::WOULDBLOCK) => {}
            Err(error) => return Err(error.into()),
        }
        poll.wait(Some(next_send.min(deadline)))?;
    }
}

#[cfg(test)]
mod tests;
