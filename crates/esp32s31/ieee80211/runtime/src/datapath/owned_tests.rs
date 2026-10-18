use core::task::{Context, Poll};

use oer_ieee80211_mac::data::EthernetFrameParts;
use oer_network_interface::{NetworkInterfaceId, RxEnqueueError};

use super::network::{DatapathNetworkRx, DatapathNetworkRxEndpoints, DatapathNetworkRxSet};

#[derive(Default)]
struct Endpoint {
    frames: usize,
}

impl DatapathNetworkRx for Endpoint {
    fn queue_len(&self) -> usize {
        self.frames
    }

    fn try_send(&mut self, _frame: &[u8]) -> Result<(), RxEnqueueError> {
        self.frames += 1;
        Ok(())
    }

    fn try_send_parts(&mut self, _frame: EthernetFrameParts<'_>) -> Result<(), RxEnqueueError> {
        self.frames += 1;
        Ok(())
    }

    fn poll_ready(&mut self, _context: &mut Context<'_>) -> Poll<()> {
        Poll::Ready(())
    }

    fn try_send_observed(
        &mut self,
        frame: &[u8],
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        before_publish();
        self.try_send(frame)
    }

    fn try_send_parts_observed(
        &mut self,
        frame: EthernetFrameParts<'_>,
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        before_publish();
        self.try_send_parts(frame)
    }
}

#[test]
fn addressed_rx_endpoints_never_fall_back_to_the_primary_interface() {
    let first = NetworkInterfaceId::new(0);
    let second = NetworkInterfaceId::new(1);
    let unknown = NetworkInterfaceId::new(2);
    let mut endpoints =
        DatapathNetworkRxEndpoints::new(first, Endpoint::default(), second, Endpoint::default());

    endpoints
        .get_mut(second)
        .unwrap()
        .try_send(&[0; 14])
        .unwrap();
    assert!(endpoints.get_mut(unknown).is_none());

    let (first_endpoint, second_endpoint) = endpoints.into_parts();
    assert_eq!(first_endpoint.frames, 0);
    assert_eq!(second_endpoint.frames, 1);
}

#[test]
fn pair_borrow_respects_requested_interface_order() {
    let first = NetworkInterfaceId::new(0);
    let second = NetworkInterfaceId::new(1);
    let mut endpoints =
        DatapathNetworkRxEndpoints::new(first, Endpoint::default(), second, Endpoint::default());

    let (second_endpoint, first_endpoint) = endpoints.pair_mut(second, first).unwrap();
    second_endpoint.try_send(&[0; 14]).unwrap();
    first_endpoint.try_send(&[0; 14]).unwrap();
    first_endpoint.try_send(&[0; 14]).unwrap();

    let (first_endpoint, second_endpoint) = endpoints.into_parts();
    assert_eq!(first_endpoint.frames, 2);
    assert_eq!(second_endpoint.frames, 1);
}
