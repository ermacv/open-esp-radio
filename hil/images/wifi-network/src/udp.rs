//! The load server's UDP over the network core: a socket for each direction,
//! as each is an affine end, kept across runs. The first run's start names the
//! address, which configures the network; a later run names the same.

use core::cell::{Cell, RefCell, RefMut};

use oer::wifi::{self, NetworkAddress, NetworkBlock, Received, Refusal, Refused, Remote, Socket};
use oer_hil_services_load::{LoadError, Start};

/// Where a datagram goes.
#[derive(Clone, Copy)]
pub(crate) struct Peer {
    pub address: [u8; 4],
    pub port: u16,
}

impl From<Peer> for Remote {
    fn from(peer: Peer) -> Self {
        Self {
            address: peer.address,
            port: peer.port,
        }
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

/// The network the load server opens its runs on.
pub(crate) struct Network {
    network: RefCell<wifi::Network>,
    sockets: RefCell<Option<[Socket; wifi::SOCKETS]>>,
    address: Cell<Option<oer_hil_services_load::StaticAddress>>,
    /// Datagrams the sockets delivered: this core counts no frames.
    delivered: Cell<u32>,
}

impl Network {
    pub(crate) const fn new(network: wifi::Network) -> Self {
        Self {
            network: RefCell::new(network),
            sockets: RefCell::new(None),
            address: Cell::new(None),
            delivered: Cell::new(0),
        }
    }
}

/// A run's sockets, on the address its start names.
pub(crate) fn open<'a>(net: &'a Network, start: &Start) -> Result<Run<'a>, LoadError> {
    let address = start.address.ok_or(LoadError::NoAddress)?;
    match net.address.get() {
        Some(configured) if configured == address => {}
        Some(_) => return Err(LoadError::InvalidParameters),
        None => {
            let ports = [start.port, start.port.wrapping_add(1)];
            let mut network = net.network.try_borrow_mut().map_err(|_| LoadError::Busy)?;
            let sockets = network
                .configure(
                    NetworkAddress {
                        address: address.address,
                        prefix: address.prefix,
                        gateway: Some(address.gateway),
                    },
                    ports,
                )
                .map_err(|_| LoadError::Bind)?;
            let [receiving, mut sending] = sockets;
            // A batch of sends takes no turn of the service for each block.
            sending.reserve_ahead(wifi::RESERVE_AHEAD);
            let mut held = net.sockets.try_borrow_mut().map_err(|_| LoadError::Busy)?;
            *held = Some([receiving, sending]);
            net.address.set(Some(address));
        }
    }
    let held = net.sockets.try_borrow_mut().map_err(|_| LoadError::Busy)?;
    let sockets = RefMut::filter_map(held, Option::as_mut).map_err(|_| LoadError::Bind)?;
    Ok(Run {
        sockets,
        delivered: &net.delivered,
    })
}

/// The datagrams the sockets delivered.
pub(crate) fn received_frames(net: &Network) -> u32 {
    net.delivered.get()
}

/// A run's sockets, the network's again when it ends.
pub(crate) struct Run<'a> {
    sockets: RefMut<'a, [Socket; wifi::SOCKETS]>,
    delivered: &'a Cell<u32>,
}

impl Run<'_> {
    pub(crate) fn split(&mut self) -> (Receiving<'_>, Sending<'_>) {
        let [receiving, sending] = &mut *self.sockets;
        (
            Receiving {
                socket: receiving,
                delivered: self.delivered,
                next: None,
                returning: None,
                closed: false,
            },
            Sending {
                socket: sending,
                held: None,
            },
        )
    }
}

/// The receiving side of a run.
pub(crate) struct Receiving<'a> {
    socket: &'a mut Socket,
    delivered: &'a Cell<u32>,
    /// A datagram [`Self::ready`] found.
    next: Option<Received>,
    /// A block that found the socket's lane full.
    returning: Option<NetworkBlock>,
    closed: bool,
}

impl<'a> Receiving<'a> {
    /// Wait for a datagram. A closed socket receives nothing more. A
    /// cancelled wait loses nothing: the socket's ends keep what they hold.
    pub(crate) async fn ready(&mut self) {
        if self.closed {
            core::future::pending::<()>().await;
        }
        if self.returning.is_some() {
            if self.socket.room().await.is_err() {
                self.closed = true;
                return;
            }
            self.release();
        }
        match self.socket.receive().await {
            Ok(datagram) => self.next = Some(datagram),
            Err(_) => self.closed = true,
        }
    }

    /// Give the block back, or keep it for [`Self::ready`].
    fn release(&mut self) {
        if let Some(block) = self.returning.take()
            && let Err(Refused { reason, block }) = self.socket.release(block)
        {
            // After a stop the pool's holder has what is left.
            self.closed = reason == Refusal::Closed;
            self.returning = Some(block);
        }
    }

    /// The next datagram here. One block at a time waits for the lane, so a
    /// waiting one holds the rest back until [`Self::ready`] gave it.
    pub(crate) fn take(&mut self) -> Option<Packet<'_, 'a>> {
        if self.returning.is_some() {
            self.release();
        }
        if self.returning.is_some() {
            return None;
        }
        let datagram = self.next.take().or_else(|| self.socket.try_receive())?;
        self.delivered.set(self.delivered.get().wrapping_add(1));
        Some(Packet {
            receiving: self,
            datagram: Some(datagram),
        })
    }
}

/// A received datagram; its block goes back when this ends.
pub(crate) struct Packet<'r, 'a> {
    receiving: &'r mut Receiving<'a>,
    /// `None` only once it went.
    datagram: Option<Received>,
}

impl Drop for Packet<'_, '_> {
    fn drop(&mut self) {
        if let Some(datagram) = self.datagram.take() {
            self.receiving.returning = Some(datagram.block);
            self.receiving.release();
        }
    }
}

impl Packet<'_, '_> {
    pub(crate) fn payload(&self) -> &[u8] {
        self.datagram
            .as_ref()
            .map_or(&[], |datagram| datagram.block.octets())
    }

    pub(crate) fn payload_mut(&mut self) -> &mut [u8] {
        let Some(datagram) = self.datagram.as_mut() else {
            return &mut [];
        };
        let length = datagram.block.octets().len();
        datagram.block.space().get_mut(..length).unwrap_or_default()
    }

    /// Send the payload back where it came from; whether the socket took it.
    pub(crate) fn reply(mut self) -> bool {
        let Some(datagram) = self.datagram.take() else {
            return false;
        };
        match self.receiving.socket.send(datagram.remote, datagram.block) {
            Ok(_result_given_up) => true,
            Err(Refused { block, .. }) => {
                self.receiving.returning = Some(block);
                false
            }
        }
    }
}

/// The sending side of a run.
pub(crate) struct Sending<'a> {
    socket: &'a mut Socket,
    /// A block reserved and not sent.
    held: Option<NetworkBlock>,
}

impl Sending<'_> {
    /// Offer `size` octets that `write` builds, without waiting.
    pub(crate) fn try_send(
        &mut self,
        size: usize,
        peer: Peer,
        write: impl FnOnce(&mut [u8]) -> Option<()>,
    ) -> Attempt {
        if self.held.is_none() {
            match self.socket.try_reserve() {
                Ok(Some(block)) => self.held = Some(block),
                Ok(None) => return Attempt::NoRoom,
                Err(_) => return Attempt::Lost,
            }
        }
        self.commit(size, peer, write)
    }

    /// Write the held block and hand it to the socket.
    fn commit(
        &mut self,
        size: usize,
        peer: Peer,
        write: impl FnOnce(&mut [u8]) -> Option<()>,
    ) -> Attempt {
        let Some(mut block) = self.held.take() else {
            return Attempt::NoRoom;
        };
        let written = block.space().get_mut(..size).and_then(write).is_some()
            && block.set_length(size).is_ok();
        if !written {
            self.held = Some(block);
            return Attempt::Lost;
        }
        match self.socket.send(Remote::from(peer), block) {
            Ok(_result_given_up) => Attempt::Taken,
            Err(Refused { reason, block }) => {
                self.held = Some(block);
                if reason == Refusal::Busy {
                    Attempt::NoRoom
                } else {
                    Attempt::Lost
                }
            }
        }
    }

    /// As [`Self::try_send`], once the socket has room; whether it took it.
    pub(crate) async fn send(
        &mut self,
        size: usize,
        peer: Peer,
        write: impl FnOnce(&mut [u8]) -> Option<()>,
    ) -> bool {
        if self.held.is_none() {
            match self.socket.reserve().await {
                Ok(block) => self.held = Some(block),
                Err(_) => return false,
            }
        }
        if self.socket.credit().await.is_err() {
            return false;
        }
        matches!(self.commit(size, peer, write), Attempt::Taken)
    }
}
