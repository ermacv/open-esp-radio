extern crate std;

use super::*;
use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, Waker};

fn poll_once<F: Future>(future: F) -> Poll<F::Output> {
    pin!(future).poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn the_second_end_comes_only_after_the_application_end_split_the_channel() {
    static CHANNEL: Channel<u8, u16, 2> = Channel::new();
    assert_eq!(CHANNEL.take_second().err(), Some(ChannelError::NotSplit));
    let application = CHANNEL.take_application();
    assert!(application.is_ok());
    assert_eq!(CHANNEL.take_application().err(), Some(ChannelError::Taken));
    assert!(CHANNEL.take_second().is_ok());
    assert_eq!(CHANNEL.take_second().err(), Some(ChannelError::Taken));
}

#[test]
fn each_end_receives_what_the_other_sent_in_order() {
    static CHANNEL: Channel<u8, u16, 2> = Channel::new();
    let mut application = CHANNEL.take_application().unwrap();
    let mut second = CHANNEL.take_second().unwrap();

    assert_eq!(poll_once(second.receive()), Poll::Pending);
    assert_eq!(poll_once(application.send(1)), Poll::Ready(()));
    assert_eq!(application.try_send(2), Ok(()));
    assert_eq!(application.try_send(3), Err(3), "two deep");
    assert_eq!(poll_once(application.send(3)), Poll::Pending);
    assert_eq!(poll_once(second.receive()), Poll::Ready(1));
    assert_eq!(second.try_receive(), Some(2));
    assert_eq!(second.try_receive(), None);

    assert_eq!(poll_once(second.send(300)), Poll::Ready(()));
    assert_eq!(poll_once(application.receive()), Poll::Ready(300));
}
