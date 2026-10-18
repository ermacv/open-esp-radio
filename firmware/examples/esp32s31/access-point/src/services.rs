use crate::network::{Stack, UdpStorage, accept, listen, new_tcp, new_udp, remote_endpoint};
use static_cell::{ConstStaticCell, StaticCell};

mod tcp;

pub const ECHO_PORT: u16 = 7;

static UDP_STORAGE: ConstStaticCell<UdpStorage> = ConstStaticCell::new(UdpStorage::new());
static UDP_PACKET: StaticCell<[u8; 1472]> = StaticCell::new();
// TCP windows are static because frame-sized arrays inside this eternal task
// would unnecessarily enlarge the executor future.
static TCP_RX: StaticCell<[u8; 4096]> = StaticCell::new();
static TCP_TX: StaticCell<[u8; 4096]> = StaticCell::new();
static TCP_PACKET: StaticCell<[u8; 1460]> = StaticCell::new();

pub async fn udp_echo(stack: Stack<'static>) -> ! {
    let mut socket = new_udp(stack, UDP_STORAGE.take());
    socket
        .bind(ECHO_PORT, embassy_net::wire::ListenSocketAddr::UNSPECIFIED)
        .expect("UDP echo port must be free");
    let packet = UDP_PACKET.init_with(|| [0; 1472]);
    loop {
        let Ok((length, remote)) = socket.recv_from(packet).await else {
            continue;
        };
        let _ = socket
            .send_to(&packet[..length], remote_endpoint(remote))
            .await;
    }
}

pub async fn tcp_echo(stack: Stack<'static>) -> ! {
    let rx = TCP_RX.init_with(|| [0; 4096]);
    let tx = TCP_TX.init_with(|| [0; 4096]);
    let packet = TCP_PACKET.init_with(|| [0; 1460]);
    let mut socket = new_tcp(stack, rx, tx);
    let mut listener = listen(stack, ECHO_PORT);
    loop {
        if accept(&mut listener, &mut socket).await.is_err() {
            continue;
        }
        if let tcp::Completion::ResetUnconfirmed = tcp::serve(&mut socket, packet).await {
            // The peer did not acknowledge close and reset could not drain.
            // Retire that socket after the bounded recovery attempt; a fresh
            // socket prevents the next connection inheriting its TCP state.
            drop(socket);
            socket = new_tcp(stack, rx, tx);
        }
    }
}
