//! Application-owned stack setup and one UDP echo workload across contracts.
mod embassy;
pub use embassy::run;

use crate::embassy_net::{Stack, udp::UdpSocket};
use static_cell::StaticCell;

async fn echo(stack: Stack<'static>) -> ! {
    #[cfg(feature = "owned-network")]
    let mut udp = UdpSocket::new(stack).expect("a UDP socket slot is free");
    static PAYLOAD: StaticCell<[u8; 1472]> = StaticCell::new();
    let payload = PAYLOAD.init_with(|| [0; 1472]);
    udp.bind(
        4321,
        crate::embassy_net::wire::ListenSocketAddr::UNSPECIFIED,
    )
    .expect("UDP echo port is available");
    loop {
        match udp.recv_from(payload).await {
            Ok((length, metadata)) => {
                if let Err(error) = udp.send_to(&payload[..length], metadata.remote_addr).await {
                    esp_println::println!("open-radio: UDP echo send failed: {:?}", error);
                }
            }
            Err(error) => esp_println::println!("open-radio: UDP echo receive failed: {:?}", error),
        }
    }
}
