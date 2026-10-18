use oer::systems::esp32s31::embassy::wifi::WifiDevice;

use static_cell::StaticCell;
pub async fn run(device: WifiDevice, seed: u64) -> ! {
    use oer::systems::esp32s31::embassy::wifi::{WifiNetworkRunner, WifiStackResources};
    static STORAGE: StaticCell<WifiStackResources<'static>> = StaticCell::new();
    let (iface, runner) =
        WifiNetworkRunner::new(device, STORAGE.init(WifiStackResources::new()), seed);
    iface
        .set_dhcpv4(Some(Default::default()))
        .expect("the Wi-Fi interface is Ethernet");
    let application = async {
        iface.wait_config_v4_up().await;
        esp_println::println!("open-radio: IPv4 ready {:?}", iface.ip_addrs());
        super::echo(iface.stack()).await
    };
    embassy_futures::join::join(application, runner.run()).await;
    unreachable!()
}
