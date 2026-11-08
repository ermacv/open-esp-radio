//! The join: scan, authentication, association, the four-way handshake,
//! the open port and the DHCP lease.

use crate::simulation::{Setup, simulate};

#[test]
fn a_station_joins_and_leases_its_address() {
    struct Net;
    let report = simulate::<Net>(Setup::default(), |world| {
        Box::pin(async move {
            let configuration = world.join().await;
            println!("{configuration:?} at {:?}", world.now());
            assert!(configuration.ipv4.is_some());
        })
    });
    let ap = report.access_point.borrow();
    println!("{:?}", ap.events);
    println!("{:?}", ap.wired.dhcp.seen);
    println!("{:?}", report.heard.notices.borrow());
    println!(
        "{:?} {:?} {:?}",
        report.lifecycle, report.actor, report.ended_at
    );
    drop(ap);
    report.assert_stopped_with_every_owner();
}
