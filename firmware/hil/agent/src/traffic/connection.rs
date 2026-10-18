//! A ready event must follow the socket entering its accept state.
use core::{
    future::{Future, poll_fn},
    pin::pin,
    task::Poll,
};

pub async fn before_ready<E>(
    accept: impl Future<Output = Result<(), E>>,
    ready: impl Future<Output = ()>,
) -> Result<(), E> {
    let mut accept = pin!(accept);
    // Released Embassy starts listening on the first poll, whereas Xarxa has
    // a separate listener. Retain this same future across readiness publication
    // so cancellation or a second accept cannot reset an incoming connection.
    let first = poll_fn(|cx| Poll::Ready(accept.as_mut().poll(cx))).await;
    if let Poll::Ready(Err(error)) = first {
        return Err(error);
    }
    ready.await;
    match first {
        Poll::Ready(result) => result,
        Poll::Pending => accept.await,
    }
}
