use super::*;

struct StoppedRx(u8);

impl StoppedStaRx for StoppedRx {
    type Preconnected = (u8,);
    type Persistent = u16;

    fn split_for_reconnect(self) -> (Self::Preconnected, Self::Persistent) {
        ((self.0,), u16::from(self.0) + 100)
    }
}

#[test]
fn running_scan_round_trip_and_reconnect_preserve_every_owner() {
    let disconnected = DisconnectedStaEpoch::new("network", "hardware", StoppedRx(7), 8, 9);
    let scan = disconnected.into_running_scan_parts();
    assert_eq!(scan.hardware, "hardware");
    assert_eq!(scan.rx.0, 7);

    let disconnected = scan.retained.restore(scan.hardware, scan.rx);
    assert_eq!(disconnected.hardware(), &"hardware");
    assert_eq!(disconnected.rx().0, 7);

    let (network, reconnected) = disconnected.prepare_reconnect();
    assert_eq!(network, "network");
    let parts = reconnected.into_parts();
    assert_eq!(parts.hardware, "hardware");
    assert_eq!(parts.rx.0, 7);
    assert_eq!(parts.rx_resources, 107);
    assert_eq!(parts.aggregate_tx, 8);
    assert_eq!(parts.control, 9);
}
