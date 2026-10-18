use core::net::Ipv4Addr;

use crate::network::{Stack, UdpStorage, new_udp};
use edge_dhcp::{
    Options, Packet,
    server::{Server, ServerOptions},
};
use embassy_time::Instant;
use static_cell::{ConstStaticCell, StaticCell};

const SERVER_ADDRESS: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 1);
const LEASE_START: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 100);
const LEASE_END: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 114);
const DHCP_SERVER_PORT: u16 = 67;
const DHCP_CLIENT_PORT: u16 = 68;
const DATAGRAM_CAPACITY: usize = 768;

type DhcpServer = Server<fn() -> u64, 15>;

static UDP_STORAGE: ConstStaticCell<UdpStorage> = ConstStaticCell::new(UdpStorage::new());
static SERVER: StaticCell<DhcpServer> = StaticCell::new();
// DHCP packets are deliberately static: keeping two maximum-sized messages
// out of the async state machine prevents a large executor stack frame.
static REQUEST: StaticCell<[u8; DATAGRAM_CAPACITY]> = StaticCell::new();
static REPLY: StaticCell<[u8; DATAGRAM_CAPACITY]> = StaticCell::new();

fn now_seconds() -> u64 {
    Instant::now().as_secs()
}

pub async fn run(stack: Stack<'static>) -> ! {
    let mut server = Server::new(now_seconds as fn() -> u64, SERVER_ADDRESS);
    server.range_start = LEASE_START;
    server.range_end = LEASE_END;
    let server = SERVER.init(server);
    let mut gateway = [Ipv4Addr::UNSPECIFIED];
    let options = ServerOptions::new(SERVER_ADDRESS, Some(&mut gateway));
    let mut socket = new_udp(stack, UDP_STORAGE.take());
    socket
        .bind(
            DHCP_SERVER_PORT,
            embassy_net::wire::ListenSocketAddr::UNSPECIFIED,
        )
        .expect("DHCP port must be free");
    let request_buffer = REQUEST.init_with(|| [0; DATAGRAM_CAPACITY]);
    let reply_buffer = REPLY.init_with(|| [0; DATAGRAM_CAPACITY]);

    loop {
        let Ok((length, _remote)) = socket.recv_from(request_buffer).await else {
            continue;
        };
        let Ok(request) = Packet::decode(&request_buffer[..length]) else {
            continue;
        };
        let mut option_buffer = Options::buf();
        let Some(reply) = server.handle_request(&mut option_buffer, &options, &request) else {
            continue;
        };
        let Ok(reply) = reply.encode(reply_buffer) else {
            continue;
        };
        let destination = if !request.giaddr.is_unspecified() {
            (request.giaddr, DHCP_SERVER_PORT)
        } else if !request.ciaddr.is_unspecified() && !request.broadcast {
            (request.ciaddr, DHCP_CLIENT_PORT)
        } else {
            (Ipv4Addr::BROADCAST, DHCP_CLIENT_PORT)
        };
        let _ = socket.send_to(reply, destination).await;
    }
}
