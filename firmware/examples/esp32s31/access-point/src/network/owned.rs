use crate::embassy_net;

pub use embassy_net::{
    Stack,
    tcp::{TcpListener, TcpSocket},
    udp::UdpSocket,
};
pub struct UdpStorage;
impl UdpStorage {
    pub const fn new() -> Self {
        Self
    }
}

pub fn new_udp(stack: Stack<'static>, _storage: &'static mut UdpStorage) -> UdpSocket<'static> {
    UdpSocket::new(stack).expect("a UDP socket slot must be free")
}
pub fn new_tcp<'a>(
    stack: Stack<'static>,
    rx: &'a mut [u8],
    tx: &'a mut [u8],
) -> TcpSocket<'a, 'static> {
    TcpSocket::new(stack, rx, tx).expect("a TCP socket slot must be free")
}
pub fn listen(stack: Stack<'static>, port: u16) -> TcpListener<'static> {
    let mut listener = TcpListener::new(stack).expect("a TCP listener slot must be free");
    listener.listen(port).expect("TCP echo port must be free");
    listener
}
pub async fn accept(
    listener: &mut TcpListener<'static>,
    socket: &mut TcpSocket<'_, 'static>,
) -> Result<(), ()> {
    let token = listener.accept().await.map_err(|_| ())?;
    socket.accept(token).await.map_err(|_| ())
}
pub fn remote_endpoint(meta: embassy_net::udp::UdpMetadata) -> embassy_net::wire::SocketAddr {
    meta.remote_addr
}

#[cfg(target_arch = "riscv32")]
pub async fn run<F: core::future::Future>(
    device: oer::systems::esp32s31::embassy::wifi::WifiDevice,
    seed: u64,
    application: impl FnOnce(Stack<'static>) -> F,
) -> ! {
    use embassy_net::wire::{IpCidr, Ipv4Addr, Ipv4Cidr};

    use oer::systems::esp32s31::embassy::wifi::{WifiNetworkRunner, WifiStackResources};
    static STORAGE: static_cell::StaticCell<WifiStackResources<'static>> =
        static_cell::StaticCell::new();
    let (iface, runner) =
        WifiNetworkRunner::new(device, STORAGE.init(WifiStackResources::new()), seed);
    iface
        .add_ip_addr(IpCidr::V4(Ipv4Cidr::new(Ipv4Addr::new(192, 168, 4, 1), 24)))
        .expect("the access point address fits");
    embassy_futures::join::join(application(iface.stack()), runner.run()).await;
    unreachable!()
}

impl Default for UdpStorage {
    fn default() -> Self {
        Self::new()
    }
}
