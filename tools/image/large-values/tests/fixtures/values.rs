#![no_std]

use core::hint::black_box;

#[derive(Clone, Copy)]
pub struct CopyBig(pub [u8; 8192]);
pub struct Owned(pub [u8; 8192]);

#[inline(never)]
pub fn local(seed: u8) {
    let mut value = [seed; 8192];
    black_box(&mut value);
}

#[inline(never)]
pub fn consume_copy(value: CopyBig) {
    black_box(&value);
}

#[inline(never)]
pub fn consume_move(value: Owned) {
    black_box(&value);
}

#[inline(never)]
pub fn direct_copy(value: CopyBig) {
    consume_copy(value);
    black_box(&value);
}

#[inline(never)]
pub fn direct_move(value: Owned) {
    consume_move(value);
}

#[inline(never)]
pub fn indirect_copy(function: fn(CopyBig), value: CopyBig) {
    function(value);
    black_box(&value);
}

#[inline(never)]
pub fn indirect_move(function: fn(Owned), value: Owned) {
    function(value);
}

#[inline(never)]
pub fn return_value(seed: u8) -> Owned {
    Owned([seed; 8192])
}

#[inline(never)]
pub fn call_destination(seed: u8) {
    let value = return_value(seed);
    black_box(&value);
}

#[inline(never)]
pub fn generic<T: Copy>(value: &T) {
    let copied = *value;
    black_box(&copied);
}

#[inline(never)]
pub fn instantiate(value: &CopyBig) {
    generic(value);
}

#[inline(never)]
pub fn instantiate_again(value: &CopyBig) {
    generic(value);
}

pub async fn future(value: Owned) {
    core::future::pending::<()>().await;
    black_box(&value);
}

pub fn poll_future(value: Owned) {
    use core::future::Future;
    let mut future = core::pin::pin!(future(value));
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    black_box(future.as_mut().poll(&mut context));
}

static STATIC_VALUE: [u8; 8192] = [7; 8192];

#[inline(never)]
pub fn static_reference() {
    black_box(&STATIC_VALUE);
}

#[inline(never)]
pub fn at_limit(seed: u8) {
    let mut value = [seed; 4096];
    black_box(&mut value);
}

macro_rules! large_local {
    () => {{
        let mut value = [0u8; 8192];
        black_box(&mut value);
    }};
}

#[inline(never)]
pub fn macro_local() {
    large_local!();
}
