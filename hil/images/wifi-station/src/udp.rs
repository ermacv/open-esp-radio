//! The load server's UDP over the IP service: one socket for both directions.

use core::net::Ipv4Addr;

use oer::wifi::embassy_net::TryError;
use oer::wifi::embassy_net::udp::{RecvPacket, UdpMetadata, UdpSocket};
use oer::wifi::embassy_net::wire::{ListenSocketAddr, SocketAddr};
use oer::wifi::{IpDevice, Ipv4Status, Net};
use oer_hil_services_load::header::Header;
use oer_hil_services_load::{LoadError, Start};

use crate::RX;

type Socket = UdpSocket<'static, IpDevice<RX>>;

/// The network the load server opens its runs on.
pub(crate) type Network = Net<RX>;

/// Where a datagram goes.
#[derive(Clone, Copy)]
pub(crate) struct Peer {
    pub address: [u8; 4],
    pub port: u16,
}

impl From<Peer> for SocketAddr {
    fn from(peer: Peer) -> Self {
        Self::from((Ipv4Addr::from(peer.address), peer.port))
    }
}

/// What became of a datagram offered now.
pub(crate) enum Attempt {
    Taken,
    /// No room: the datagram can wait for it.
    NoRoom,
    /// The datagram is lost.
    Lost,
}

/// A run's socket, bound to `start.port` of the station's address.
pub(crate) struct Run<'a> {
    socket: Socket,
    network: core::marker::PhantomData<&'a Network>,
}

/// A run's socket, on the address the station leased.
pub(crate) fn open<'a>(net: &'a Network, start: &Start) -> Result<Run<'a>, LoadError> {
    // This stack leases its address, and never takes the host's.
    if start.address.is_some() {
        return Err(LoadError::InvalidParameters);
    }
    let address = match net.status().ipv4 {
        Ipv4Status::Lease(configuration) | Ipv4Status::Static(configuration) => {
            configuration.address
        }
        Ipv4Status::Unconfigured => return Err(LoadError::NoAddress),
    };
    let mut socket = UdpSocket::new(net.stack()).map_err(|_| LoadError::Bind)?;
    let local = SocketAddr::from((Ipv4Addr::from(address), start.port));
    socket
        .bind(local, ListenSocketAddr::UNSPECIFIED)
        .map_err(|_| LoadError::Bind)?;
    Ok(Run {
        socket,
        network: core::marker::PhantomData,
    })
}

/// The frames the IP stack took.
pub(crate) fn received_frames(net: &Network) -> u32 {
    net.stack().statistics().rx_frames
}

/// Open a window of the network core's turns: this image runs the IP stack,
/// which has none.
pub(crate) fn open_turns(_: &Network) {}

/// Close the window of the network core's turns: none here.
pub(crate) fn close_turns(_: &Network) {}

/// The last closed window of the network core's turns: none counted here.
pub(crate) fn network_window(_: &Network) -> oer_hil_services_load::NetworkTurns {
    oer_hil_services_load::NetworkTurns::default()
}

impl Run<'_> {
    pub(crate) fn split(&mut self) -> (Receiving<'_>, Sending<'_>) {
        (
            Receiving {
                socket: &self.socket,
                closed: false,
            },
            Sending {
                socket: &self.socket,
            },
        )
    }
}

/// The receiving side of a run.
pub(crate) struct Receiving<'a> {
    socket: &'a Socket,
    closed: bool,
}

impl<'a> Receiving<'a> {
    /// Wait for a datagram. A closed socket receives nothing more.
    pub(crate) async fn ready(&mut self) {
        if self.closed {
            core::future::pending::<()>().await;
        }
        self.socket.wait_recv_ready().await;
    }

    /// The next datagram here.
    pub(crate) fn take(&mut self) -> Option<Packet<'a>> {
        match self.socket.try_recv() {
            Ok(packet) => Some(Packet {
                packet,
                socket: self.socket,
            }),
            Err(TryError::WouldBlock) => None,
            // Only a closed socket refuses.
            Err(TryError::Other(_)) => {
                self.closed = true;
                None
            }
        }
    }
}

/// A received datagram.
pub(crate) struct Packet<'a> {
    packet: RecvPacket,
    socket: &'a Socket,
}

impl Packet<'_> {
    pub(crate) fn payload(&self) -> &[u8] {
        self.packet.payload()
    }

    pub(crate) fn payload_mut(&mut self) -> &mut [u8] {
        self.packet.payload_mut()
    }

    /// Send the payload back where it came from; whether the socket took it.
    pub(crate) fn reply(self) -> bool {
        let remote: UdpMetadata = self.packet.meta().remote_addr.into();
        self.socket
            .try_send_to(self.packet.payload(), remote)
            .is_ok()
    }
}

/// The sending side of a run.
pub(crate) struct Sending<'a> {
    socket: &'a Socket,
}

impl Sending<'_> {
    /// Offer `size` octets of the datagram `header` begins, without waiting:
    /// the whole payload is written in place
    /// ([`Header::write_datagram`]).
    pub(crate) fn try_send(&mut self, size: usize, peer: Peer, header: Header) -> Attempt {
        match self
            .socket
            .try_send_to_with(size, SocketAddr::from(peer), |payload| {
                header.write_datagram(payload)
            }) {
            Ok(Some(())) => Attempt::Taken,
            Err(TryError::WouldBlock) => Attempt::NoRoom,
            Ok(None) | Err(TryError::Other(_)) => Attempt::Lost,
        }
    }

    /// As [`Self::try_send`], once the socket has room; whether it took it.
    pub(crate) async fn send(&mut self, size: usize, peer: Peer, header: Header) -> bool {
        matches!(
            self.socket
                .send_to_with(size, SocketAddr::from(peer), |payload| (
                    size,
                    header.write_datagram(payload)
                ))
                .await,
            Ok(Some(()))
        )
    }
}
