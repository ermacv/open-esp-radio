use oer_esp32s31_wifi::RX_DEPTH;
#[cfg(test)]
mod tests {
    use super::RX_DEPTH;
    extern crate std;
    use self::std::boxed::Box;
    use oer_embassy_net::{NetworkInterfaceId, OwnedEndpointResources};
    use oer_ieee80211_sta::data::EthernetFrame;
    use oer_ieee80211_wire::data::EthernetFrameParts;
    use oer_memory::{HostContext, WakingSpscQueue};
    use xarxa_driver::{PacketPool, PacketPoolStorage};

    #[test]
    fn a_service_batch_survives_both_rx_handoffs_before_the_stack_runs() {
        let storage = Box::leak(Box::new(PacketPoolStorage::<RX_DEPTH>::new()));
        let pool = Box::leak(Box::new(PacketPool::new(storage))).allocator();
        let endpoint = Box::leak(Box::new(
            OwnedEndpointResources::<HostContext, RX_DEPTH, 4>::new(),
        ));
        let destination = [2, 0, 0, 0, 0, 1];
        let source = [2, 0, 0, 0, 0, 2];
        let (mut device, mut radio) = endpoint
            .split(NetworkInterfaceId::new(0), destination, pool)
            .expect("endpoint split");
        radio.link_controller().set_link_up(true);
        let mut queue = WakingSpscQueue::<EthernetFrame, RX_DEPTH>::new();
        let (sender, receiver) = queue.split();

        // The service may accept eight frames in one poll. Neither the bridge
        // nor the IP stack has run when the last frame enters this queue.
        for marker in 0..8 {
            sender
                .try_send(
                    EthernetFrame::from_parts(EthernetFrameParts {
                        destination,
                        source,
                        ether_type: 0x0800,
                        payload: &[marker],
                    })
                    .unwrap(),
                )
                .unwrap();
        }
        while let Some(frame) = receiver.try_receive() {
            let p = frame.parts();
            radio
                .rx_publisher()
                .try_send_parts(p.destination, p.source, p.ether_type, p.payload)
                .unwrap();
        }
        for marker in 0..8 {
            let packet = device.receive().unwrap();
            assert_eq!(&packet[..6], &destination);
            assert_eq!(&packet[6..12], &source);
            assert_eq!(&packet[12..14], &[0x08, 0x00]);
            assert_eq!(&packet[14..], &[marker]);
        }
        assert!(device.receive().is_none());
        let returned: std::vec::Vec<_> = (0..RX_DEPTH).map(|_| pool.try_alloc().unwrap()).collect();
        assert!(pool.try_alloc().is_none());
        drop(returned);
    }
}
