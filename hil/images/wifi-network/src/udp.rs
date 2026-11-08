//! The load server's UDP over the network core: a socket for each direction,
//! as each is an affine end, kept across runs. The first run's start names the
//! address, which configures the network; a later run names the same.

use core::cell::{Cell, RefCell, RefMut};

use heapless::Vec;
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

/// Blocks no run could give back. A full lane can keep refusing across runs, so
/// no one run's end bounds them; the pool does, as a block is affine and this
/// has room for all of it.
type Parked = RefCell<Vec<NetworkBlock, { wifi::BLOCKS }>>;

/// Give `block` back, or park it for the next run: a full lane or a stop
/// refuses it, and a run's end has no time to wait.
fn home(socket: &mut Socket, parked: &Parked, block: NetworkBlock) {
    if let Err(Refused { block, .. }) = socket.release(block)
        && let Ok(mut parked) = parked.try_borrow_mut()
    {
        // Room for every block there is.
        let _kept = parked.push(block);
    }
}

/// The network the load server opens its runs on.
pub(crate) struct Network {
    network: RefCell<wifi::Network>,
    sockets: RefCell<Option<[Socket; wifi::SOCKETS]>>,
    address: Cell<Option<oer_hil_services_load::StaticAddress>>,
    /// Datagrams the sockets delivered: this core counts no frames.
    delivered: Cell<u32>,
    /// Blocks an ended run could not give back. They outlive its adapters,
    /// which a deadline cancels at any await.
    parked: Parked,
    /// Where the network core's turns are kept ([`Turns`]), once the group
    /// gave it. The chip counts them; the host's tests of this facade have
    /// none.
    #[cfg(target_arch = "riscv32")]
    turns: Cell<Option<&'static mut Turns>>,
}

impl Network {
    pub(crate) const fn new(network: wifi::Network) -> Self {
        Self {
            network: RefCell::new(network),
            sockets: RefCell::new(None),
            address: Cell::new(None),
            delivered: Cell::new(0),
            parked: RefCell::new(Vec::new()),
            #[cfg(target_arch = "riscv32")]
            turns: Cell::new(None),
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
        parked: &net.parked,
    })
}

/// The datagrams the sockets delivered.
pub(crate) fn received_frames(net: &Network) -> u32 {
    net.delivered.get()
}

/// No window of the network core's turns.
#[cfg(target_arch = "riscv32")]
const TURNS_NONE: oer_hil_services_load::NetworkTurns = oer_hil_services_load::NetworkTurns {
    available: false,
    turns: 0,
    reserves: 0,
    releases: 0,
    sends: 0,
    closes: 0,
    budget_spent: 0,
    budget_turns: 0,
    intake_full: 0,
    accepted: 0,
    accepted_by_turn: [0; oer_hil_services_load::ACCEPTED_BINS],
    publish_chose: 0,
    publish_refused: 0,
    publish_budget: 0,
    publish_deferred: 0,
    publish_fault: 0,
    blocks_out: 0,
    credits_out: 0,
    yields: 0,
    yields_with_work: 0,
    waits_pending: 0,
    waits_ready: 0,
};

/// The network core's turns at the open window's start, and the last closed
/// window, kept until the next one closes: in a static the group takes once,
/// as neither a run's futures, whose polls would copy them, nor the group's
/// stack should hold them.
#[cfg(target_arch = "riscv32")]
pub(crate) struct Turns {
    start: oer_hil_services_load::NetworkTurns,
    window: oer_hil_services_load::NetworkTurns,
}

/// The one place of the turns.
#[cfg(target_arch = "riscv32")]
pub(crate) static TURNS: static_cell::ConstStaticCell<Turns> =
    static_cell::ConstStaticCell::new(Turns {
        start: TURNS_NONE,
        window: TURNS_NONE,
    });

#[cfg(target_arch = "riscv32")]
impl Network {
    /// Keep the network core's turns in `turns`.
    pub(crate) fn keep_turns(&self, turns: &'static mut Turns) {
        self.turns.set(Some(turns));
    }

    /// Lend the place of the turns to `use_turns`, if the group gave one.
    fn turns<R: Default>(&self, use_turns: impl FnOnce(&mut Turns) -> R) -> R {
        let Some(turns) = self.turns.take() else {
            return R::default();
        };
        let result = use_turns(turns);
        self.turns.set(Some(turns));
        result
    }
}

/// Open a window of the network core's turns.
#[cfg(target_arch = "riscv32")]
pub(crate) fn open_turns(net: &Network) {
    net.turns(|turns| turns.start = network_turns());
}

/// Close the open window of the network core's turns, which
/// [`network_window`] answers from here on.
#[cfg(target_arch = "riscv32")]
pub(crate) fn close_turns(net: &Network) {
    net.turns(|turns| {
        turns.window = oer_hil_services_load::NetworkTurns::window(&turns.start, &network_turns());
    });
}

/// The last closed window of the network core's turns; none before one
/// closed.
#[cfg(target_arch = "riscv32")]
pub(crate) fn network_window(net: &Network) -> oer_hil_services_load::NetworkTurns {
    net.turns(|turns| turns.window)
}

/// What the network core's turns did since boot, as the load service names
/// it: the chip's observation, which the host's tests of this facade lack.
#[cfg(target_arch = "riscv32")]
fn network_turns() -> oer_hil_services_load::NetworkTurns {
    let turns = oer::observe::network_turns();
    oer_hil_services_load::NetworkTurns {
        available: true,
        turns: turns.turns,
        reserves: turns.reserves,
        releases: turns.releases,
        sends: turns.sends,
        closes: turns.closes,
        budget_spent: turns.budget_spent,
        budget_turns: turns.budget_turns,
        intake_full: turns.intake_full,
        accepted: turns.accepted,
        accepted_by_turn: turns.accepted_by_turn,
        publish_chose: turns.publish_chose,
        publish_refused: turns.publish_refused,
        publish_budget: turns.publish_budget,
        publish_deferred: turns.publish_deferred,
        publish_fault: turns.publish_fault,
        blocks_out: turns.blocks_out,
        credits_out: turns.credits_out,
        yields: turns.yields,
        yields_with_work: turns.yields_with_work,
        waits_pending: turns.waits_pending,
        waits_ready: turns.waits_ready,
    }
}

/// A run's sockets, the network's again when it ends.
pub(crate) struct Run<'a> {
    sockets: RefMut<'a, [Socket; wifi::SOCKETS]>,
    delivered: &'a Cell<u32>,
    parked: &'a Parked,
}

impl Run<'_> {
    pub(crate) fn split(&mut self) -> (Receiving<'_>, Sending<'_>) {
        let [receiving, sending] = &mut *self.sockets;
        // What the last run could not give back: the lane has room by now.
        // One pass: a block refused again goes back behind the rest.
        let waiting = self.parked.try_borrow().map_or(0, |parked| parked.len());
        for _ in 0..waiting {
            let block = self
                .parked
                .try_borrow_mut()
                .ok()
                .and_then(|mut parked| parked.pop());
            if let Some(block) = block {
                home(receiving, self.parked, block);
            }
        }
        (
            Receiving {
                socket: receiving,
                delivered: self.delivered,
                parked: self.parked,
                next: None,
                returning: None,
                closed: false,
            },
            Sending {
                socket: sending,
                held: None,
                parked: self.parked,
            },
        )
    }
}

/// The receiving side of a run.
pub(crate) struct Receiving<'a> {
    socket: &'a mut Socket,
    delivered: &'a Cell<u32>,
    parked: &'a Parked,
    /// A datagram [`Self::ready`] found.
    next: Option<Received>,
    /// A block that found the socket's lane full.
    returning: Option<NetworkBlock>,
    closed: bool,
}

impl Drop for Receiving<'_> {
    fn drop(&mut self) {
        if let Some(datagram) = self.next.take() {
            home(self.socket, self.parked, datagram.block);
        }
        if let Some(block) = self.returning.take() {
            home(self.socket, self.parked, block);
        }
    }
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
    parked: &'a Parked,
}

impl Drop for Sending<'_> {
    fn drop(&mut self) {
        if let Some(block) = self.held.take() {
            home(self.socket, self.parked, block);
        }
    }
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
