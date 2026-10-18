use super::*;
#[cfg(feature = "owned-network")]
pub const UDP_RX_QUEUE_DEPTH: usize = xarxa_owned::config::UDP_RX_QUEUE_COUNT;
pub struct UdpStorage<const RX: usize, const TX: usize>;
impl<const RX: usize, const TX: usize> Default for UdpStorage<RX, TX> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const RX: usize, const TX: usize> UdpStorage<RX, TX> {
    pub const fn new() -> Self {
        Self
    }
}
pub fn new_udp<'a, const RX: usize, const TX: usize>(
    stack: Stack<'a>,
    _storage: &'a mut UdpStorage<RX, TX>,
) -> UdpSocket<'a> {
    #[cfg(feature = "owned-network")]
    UdpSocket::new(stack).expect("a HIL UDP socket slot must be free")
}
pub async fn recv_from_with<R>(
    socket: &mut UdpSocket<'_>,
    f: impl FnOnce(&[u8], embassy_net::udp::UdpMetadata) -> R,
) -> Result<R, embassy_net::udp::RecvError> {
    socket.recv_from_with(f).await
}
pub fn listen(stack: Stack<'_>, port: u16) -> embassy_net::tcp::TcpListener<'_> {
    #[cfg(feature = "owned-network")]
    let mut listener =
        embassy_net::tcp::TcpListener::new(stack).expect("a HIL TCP listener slot must be free");
    listener.listen(port).expect("HIL TCP port must be free");
    listener
}
pub async fn accept(
    listener: &mut embassy_net::tcp::TcpListener<'_>,
    socket: &mut TcpSocket<'_, '_>,
) -> Result<(), embassy_net::tcp::AcceptError> {
    #[cfg(feature = "owned-network")]
    socket.accept(listener.accept().await?).await
}
