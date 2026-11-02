use super::*;

fn configuration(gateway: Option<[u8; 4]>) -> Ipv4Configuration {
    Ipv4Configuration {
        address: [10, 231, 0, 23],
        prefix_length: 24,
        gateway,
    }
}

#[test]
fn nothing_published_reads_as_a_running_service_without_link_or_address() {
    assert_eq!(
        Published::new().read(),
        Ok(Status {
            link: LinkSnapshot::DOWN,
            ipv4: Ipv4Status::Unconfigured,
            lifecycle: Lifecycle::Running,
        })
    );
}

#[test]
fn every_status_reads_back_whole() {
    let errors = [
        ServiceError::InvalidPrefixLength(40),
        ServiceError::EndpointAlreadySplit,
        ServiceError::RxWaiterUnavailable,
        ServiceError::InterfaceTableFull,
        ServiceError::UnsupportedMedium,
        ServiceError::HardwareAddressMismatch,
        ServiceError::AddressNotUnicast,
        ServiceError::AddressTableFull,
        ServiceError::GatewayNotUnicast,
        ServiceError::RouteTableFull,
        ServiceError::DhcpMediumMismatch,
        ServiceError::ContextAccessRefused,
    ];
    let lifecycles = [Lifecycle::Running, Lifecycle::Stopped]
        .into_iter()
        .chain(errors.map(Lifecycle::Failed));
    let published = Published::new();
    for lifecycle in lifecycles {
        for ipv4 in [
            Ipv4Status::Unconfigured,
            Ipv4Status::Lease(configuration(Some([10, 231, 0, 1]))),
            Ipv4Status::Lease(configuration(None)),
            Ipv4Status::Static(configuration(Some([10, 231, 0, 1]))),
        ] {
            for link in [
                LinkSnapshot::DOWN,
                LinkSnapshot {
                    state: LinkState::Up,
                    epoch: u64::MAX - 1,
                },
            ] {
                let status = Status {
                    link,
                    ipv4,
                    lifecycle,
                };
                published.publish(status);
                assert_eq!(published.read(), Ok(status));
            }
        }
    }
}

#[test]
fn a_read_during_a_write_or_of_no_publication_is_refused() {
    let published = Published::new();
    published.sequence.store(1, Ordering::Relaxed);
    assert_eq!(published.read(), Err(Concurrent));
    published.sequence.store(2, Ordering::Relaxed);
    published.words[3].store(3 << 1, Ordering::Relaxed);
    assert_eq!(published.read(), Err(Concurrent), "no IPv4 kind three");
    published.words[3].store(FAILED << LIFECYCLE_SHIFT, Ordering::Relaxed);
    assert_eq!(published.read(), Err(Concurrent), "no failure code zero");
}
