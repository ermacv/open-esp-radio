//! A single-copy convenience for ownership and pressure fixtures.
#![allow(
    clippy::panic,
    reason = "the fixture asserts that its prepared copy is readable"
)]

use core::future::Future;
use core::task::{Context, Poll, Waker};

pub(crate) fn copied(receiving: &mut crate::udp::Receiving<'_>) -> crate::wifi::Received {
    let mut prefix = core::pin::pin!(receiving.prefix(|_| panic!("fixture expected a copy")));
    match prefix
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(Some(datagram)) => datagram,
        _ => panic!("the fixture's copied datagram is already queued"),
    }
}
