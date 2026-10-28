//! The single-channel image still validates the arbiter's complete move budget.
use oer_ieee80211_arbiter::{
    ArbiterPolicy, ArbiterPolicyError, CheckedArbiterPolicy, NoReservation,
};
use oer_time::Duration;

/// Hold a complete StationService input batch before the network task runs.
/// Both RX handoffs need this capacity; a smaller queue loses a burst even
/// when the network consumer drains every time it is polled.
pub const RX_DEPTH: usize = 8;

/// Fit two publication/retune bounds and the minimum listen window. W3
/// performs no off-channel work once associated; this is the constructor's
/// valid absence budget, independent of the software beacon-loss deadline.
pub fn arbiter(
    publication_bound: Duration,
) -> Result<CheckedArbiterPolicy<NoReservation>, ArbiterPolicyError> {
    let mut policy = ArbiterPolicy::DEFAULT;
    let absence = publication_bound
        .as_micros()
        .checked_add(policy.retune.as_micros())
        .and_then(|one| one.checked_mul(2))
        .and_then(|moves| moves.checked_add(policy.min_listen.as_micros()))
        .ok_or(ArbiterPolicyError::AbsenceBudgetOverflow)?;
    policy.absence = Duration::from_micros(absence);
    policy.checked(publication_bound)
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use self::std::boxed::Box;
    use oer_embassy_net::{NetworkInterfaceId, NoopRawMutex, OwnedEndpointResources};
    use oer_ieee80211_sta::data::EthernetFrame;
    use oer_ieee80211_wire::data::EthernetFrameParts;
    use oer_memory::WakingSpscQueue;
    use xarxa_driver::{PacketPool, PacketPoolStorage};

    #[test]
    fn a_service_batch_survives_both_rx_handoffs_before_the_stack_runs() {
        let storage = Box::leak(Box::new(PacketPoolStorage::<RX_DEPTH>::new()));
        let pool = Box::leak(Box::new(PacketPool::new(storage))).allocator();
        let endpoint = Box::leak(Box::new(
            OwnedEndpointResources::<NoopRawMutex, RX_DEPTH, 4>::new(),
        ));
        let destination = [2, 0, 0, 0, 0, 1];
        let source = [2, 0, 0, 0, 0, 2];
        let (mut device, radio) = endpoint
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

    #[test]
    fn a_long_backend_publication_bound_is_valid_before_any_owner_moves() {
        let bound = Duration::from_millis(250);
        assert!(matches!(
            ArbiterPolicy::DEFAULT.checked::<NoReservation>(bound),
            Err(ArbiterPolicyError::AbsenceTooShort { .. })
        ));
        assert!(arbiter(bound).is_ok());
        assert!(arbiter(Duration::from_secs(1)).is_ok());
        assert!(matches!(
            arbiter(Duration::from_micros(u64::MAX)),
            Err(ArbiterPolicyError::AbsenceBudgetOverflow)
        ));
    }
}
