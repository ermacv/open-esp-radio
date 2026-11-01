#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "a test fails loudly on a broken expectation"
)]

extern crate std;

use super::*;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};
use core::{cell::Cell, pin::pin};
use oer_crypto::{
    Aes128Key, KeyData,
    block::{Aes128Blocks, BlockCipher, BlockKeyUnwrap},
    conformance::{RFC3394_4_1_CIPHERTEXT, RFC3394_4_1_KEK, RFC3394_4_1_KEY_DATA},
};
use oer_memory::{HostContext, Never};
use std::{boxed::Box, rc::Rc};

const N: usize = 32;
type Service = AesService<HostContext, N>;

/// RustCrypto's AES-128 as the engine's block primitive.
struct Rust;
struct Keyed(aes::Aes128);
impl BlockCipher for Keyed {
    fn encrypt_block(&mut self, block: &mut [u8; 16]) {
        self.0.encrypt_block(GenericArray::from_mut_slice(block));
    }
    fn decrypt_block(&mut self, block: &mut [u8; 16]) {
        self.0.decrypt_block(GenericArray::from_mut_slice(block));
    }
}
impl Aes128Blocks for Rust {
    type Keyed<'a> = Keyed;
    fn load_key(&mut self, key: &Aes128Key) -> Keyed {
        Keyed(aes::Aes128::new(GenericArray::from_slice(key.as_bytes())))
    }
}

/// An engine whose operations finish only once the test opens its gate, and
/// which counts the requests it ran.
struct Gated {
    blocks: BlockKeyUnwrap<Rust>,
    gate: Rc<Cell<bool>>,
    ran: Rc<Cell<u32>>,
}

struct GatedOperation {
    engine: Option<Gated>,
    request: Option<KeyUnwrapRequest<N>>,
}

impl Future for GatedOperation {
    type Output = KeyUnwrapDone<Gated, N>;
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let Some(engine) = this.engine.as_mut() else {
            return Poll::Pending;
        };
        if !engine.gate.get() {
            return Poll::Pending;
        }
        let Some(request) = this.request.take() else {
            return Poll::Pending;
        };
        engine.ran.set(engine.ran.get() + 1);
        let response = engine.blocks.unwrap_now(request);
        match this.engine.take() {
            Some(engine) => Poll::Ready(KeyUnwrapDone { engine, response }),
            None => Poll::Pending,
        }
    }
}

impl KeyUnwrap<N> for Gated {
    type Operation = GatedOperation;
    fn unwrap_key(self, request: KeyUnwrapRequest<N>) -> GatedOperation {
        GatedOperation {
            engine: Some(self),
            request: Some(request),
        }
    }
}

fn engine() -> (Gated, Rc<Cell<bool>>, Rc<Cell<u32>>) {
    let gate = Rc::new(Cell::new(true));
    let ran = Rc::new(Cell::new(0));
    (
        Gated {
            blocks: BlockKeyUnwrap::new(Rust),
            gate: gate.clone(),
            ran: ran.clone(),
        },
        gate,
        ran,
    )
}

fn request(ciphertext: &[u8]) -> AesRequest<N> {
    AesRequest::KeyUnwrap(KeyUnwrapRequest::new(
        Aes128Key::copy_from(&RFC3394_4_1_KEK),
        KeyData::try_copy(ciphertext).expect("fits"),
    ))
}

fn plaintext(response: AesResponse<N>) -> Result<std::vec::Vec<u8>, oer_crypto::KeyUnwrapError> {
    match response {
        AesResponse::KeyUnwrap(response) => response.plaintext().map(<[u8]>::to_vec),
    }
}

fn poll<F: Future + ?Sized>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

fn service() -> &'static Service {
    seated::<2>()
}

/// A service of `C` seats, both leaked for the test.
fn seated<const C: usize>() -> &'static Service {
    let seats: &'static AesSeats<HostContext, N, C> = Box::leak(Box::new(AesSeats::new()));
    Box::leak(Box::new(Service::new(seats)))
}

#[test]
fn two_clients_share_one_engine_and_each_gets_its_own_response() {
    let service = service();
    let (engine, _, ran) = engine();
    let mut first = service.client().expect("a seat");
    let mut second = service.client().expect("a seat");
    assert!(matches!(service.client(), Err(Exhausted)));
    let mut serve = pin!(service.serve(engine));
    let mut a = pin!(first.execute(request(&RFC3394_4_1_CIPHERTEXT)));
    let mut changed = RFC3394_4_1_CIPHERTEXT;
    changed[0] ^= 0x80;
    let mut b = pin!(second.execute(request(&changed)));
    assert!(poll(a.as_mut()).is_pending());
    assert!(poll(b.as_mut()).is_pending());
    // One request at a time: the service takes and finishes both in turn.
    assert!(poll(serve.as_mut()).is_pending());
    assert_eq!(ran.get(), 2);
    let Poll::Ready(Ok(a)) = poll(a.as_mut()) else {
        panic!("the first response is ready");
    };
    assert_eq!(plaintext(a), Ok(RFC3394_4_1_KEY_DATA.to_vec()));
    let Poll::Ready(Ok(b)) = poll(b.as_mut()) else {
        panic!("the second response is ready");
    };
    assert_eq!(
        plaintext(b),
        Err(oer_crypto::KeyUnwrapError::IntegrityCheckFailed)
    );
}

#[test]
fn a_wait_dropped_before_the_service_took_it_withdraws_the_request() {
    let service = service();
    let (engine, _, ran) = engine();
    let mut client = service.client().expect("a seat");
    {
        let mut wait = pin!(client.execute(request(&RFC3394_4_1_CIPHERTEXT)));
        assert!(poll(wait.as_mut()).is_pending());
    }
    let mut serve = pin!(service.serve(engine));
    assert!(poll(serve.as_mut()).is_pending());
    assert_eq!(ran.get(), 0, "the withdrawn request never ran");
    // The seat is idle: the next request runs.
    let mut wait = pin!(client.execute(request(&RFC3394_4_1_CIPHERTEXT)));
    assert!(poll(wait.as_mut()).is_pending());
    assert!(poll(serve.as_mut()).is_pending());
    assert!(matches!(poll(wait.as_mut()), Poll::Ready(Ok(_))));
    assert_eq!(ran.get(), 1);
}

#[test]
fn a_wait_dropped_while_its_request_runs_lets_the_service_finish_and_keep_the_engine() {
    let service = service();
    let (engine, gate, ran) = engine();
    gate.set(false);
    let mut client = service.client().expect("a seat");
    let mut serve = pin!(service.serve(engine));
    {
        let mut wait = pin!(client.execute(request(&RFC3394_4_1_CIPHERTEXT)));
        assert!(poll(wait.as_mut()).is_pending());
        assert!(poll(serve.as_mut()).is_pending(), "the request runs");
    }
    // The next request waits for the discarded one.
    let mut next = pin!(client.execute(request(&RFC3394_4_1_CIPHERTEXT)));
    assert!(poll(next.as_mut()).is_pending());
    gate.set(true);
    assert!(poll(serve.as_mut()).is_pending());
    assert_eq!(ran.get(), 1, "the discarded request finished");
    assert!(poll(next.as_mut()).is_pending(), "its response was erased");
    assert!(poll(serve.as_mut()).is_pending());
    assert_eq!(ran.get(), 2);
    let Poll::Ready(Ok(response)) = poll(next.as_mut()) else {
        panic!("the next response is ready");
    };
    assert_eq!(plaintext(response), Ok(RFC3394_4_1_KEY_DATA.to_vec()));
}

#[test]
fn a_client_dropped_while_its_request_runs_frees_its_seat_once_the_service_finished() {
    let service = service();
    let (engine, gate, _) = engine();
    gate.set(false);
    let mut serve = pin!(service.serve(engine));
    let mut first = service.client().expect("a seat");
    let _second = service.client().expect("a seat");
    {
        let mut wait = pin!(first.execute(request(&RFC3394_4_1_CIPHERTEXT)));
        assert!(poll(wait.as_mut()).is_pending());
        assert!(poll(serve.as_mut()).is_pending());
    }
    drop(first);
    assert!(
        matches!(service.client(), Err(Exhausted)),
        "the seat stays taken while its request runs"
    );
    gate.set(true);
    assert!(poll(serve.as_mut()).is_pending());
    assert!(service.client().is_ok(), "the finished seat is vacant");
}

#[test]
fn a_client_is_a_key_unwrap_engine_that_comes_back_with_its_response() {
    let service = service();
    let (engine, _, _) = engine();
    let client = service.client().expect("a seat");
    let mut serve = pin!(service.serve(engine));
    let mut operation = pin!(client.unwrap_key(KeyUnwrapRequest::new(
        Aes128Key::copy_from(&RFC3394_4_1_KEK),
        KeyData::try_copy(&RFC3394_4_1_CIPHERTEXT).expect("fits"),
    )));
    assert!(poll(operation.as_mut()).is_pending());
    assert!(poll(serve.as_mut()).is_pending());
    let Poll::Ready(done) = poll(operation.as_mut()) else {
        panic!("the unwrap is done");
    };
    assert_eq!(done.response.plaintext(), Ok(&RFC3394_4_1_KEY_DATA[..]));
    // The client came back and serves its next request.
    let mut again = pin!(done.engine.unwrap_key(KeyUnwrapRequest::new(
        Aes128Key::copy_from(&RFC3394_4_1_KEK),
        KeyData::try_copy(&RFC3394_4_1_CIPHERTEXT).expect("fits"),
    )));
    assert!(poll(again.as_mut()).is_pending());
    assert!(poll(serve.as_mut()).is_pending());
    assert!(poll(again.as_mut()).is_ready());
}

#[test]
fn outside_its_context_a_client_is_issued_but_nothing_is_served() {
    let seats: &'static AesSeats<Never, N, 1> = Box::leak(Box::new(AesSeats::new()));
    let service = Box::leak(Box::new(AesService::new(seats)));
    // A seat is claimed without the context: a boot hands clients out.
    let mut client = service.client().expect("a seat");
    let mut wait = pin!(client.execute(request(&RFC3394_4_1_CIPHERTEXT)));
    assert!(matches!(
        poll(wait.as_mut()),
        Poll::Ready(Err(AesError::Foreign))
    ));
    let (engine, _, ran) = engine();
    let mut serve = pin!(service.serve(engine));
    assert_eq!(poll(serve.as_mut()), Poll::Ready(AesError::Foreign));
    assert_eq!(ran.get(), 0);
}

#[test]
fn a_dropped_idle_client_frees_its_seat_at_once() {
    let service = service();
    let first = service.client().expect("a seat");
    let _second = service.client().expect("a seat");
    assert!(matches!(service.client(), Err(Exhausted)));
    drop(first);
    assert!(service.client().is_ok());
}

/// A ciphertext whose unwrap fails its integrity check.
fn corrupted() -> [u8; 24] {
    let mut changed = RFC3394_4_1_CIPHERTEXT;
    changed[0] ^= 0x80;
    changed
}

// A wait that ended without its response, outside its context (`Foreign`)
// or leaked, leaves its seat as `forget` does here.
#[test]
fn a_later_request_never_takes_the_response_of_a_wait_that_ended_without_it() {
    let service = service();
    let (engine, _, ran) = engine();
    let mut client = service.client().expect("a seat");
    let mut serve = pin!(service.serve(engine));
    let mut first = Box::pin(client.execute(request(&RFC3394_4_1_CIPHERTEXT)));
    assert!(poll(first.as_mut()).is_pending());
    core::mem::forget(first);
    assert!(poll(serve.as_mut()).is_pending());
    assert_eq!(ran.get(), 1, "the first request ran and left its response");
    // The corrupted request gets its own integrity failure, not the
    // first one's plaintext.
    let mut second = pin!(client.execute(request(&corrupted())));
    assert!(poll(second.as_mut()).is_pending());
    assert!(poll(serve.as_mut()).is_pending());
    assert_eq!(ran.get(), 2);
    let Poll::Ready(Ok(response)) = poll(second.as_mut()) else {
        panic!("the second response is ready");
    };
    assert_eq!(
        plaintext(response),
        Err(oer_crypto::KeyUnwrapError::IntegrityCheckFailed)
    );
}

#[test]
fn a_later_request_withdraws_a_pending_one_and_waits_for_a_running_one() {
    let service = service();
    let (engine, gate, ran) = engine();
    let mut client = service.client().expect("a seat");
    let mut serve = pin!(service.serve(engine));
    // Pending: withdrawn and never run.
    let mut pending = Box::pin(client.execute(request(&RFC3394_4_1_CIPHERTEXT)));
    assert!(poll(pending.as_mut()).is_pending());
    core::mem::forget(pending);
    {
        let mut second = pin!(client.execute(request(&corrupted())));
        assert!(poll(second.as_mut()).is_pending());
        assert!(poll(serve.as_mut()).is_pending());
        assert_eq!(ran.get(), 1, "only the second request ran");
        let Poll::Ready(Ok(response)) = poll(second.as_mut()) else {
            panic!("the second response is ready");
        };
        assert!(plaintext(response).is_err());
    }
    // Running: discarded, then the next one runs.
    gate.set(false);
    let mut running = Box::pin(client.execute(request(&RFC3394_4_1_CIPHERTEXT)));
    assert!(poll(running.as_mut()).is_pending());
    assert!(poll(serve.as_mut()).is_pending(), "the request runs");
    core::mem::forget(running);
    let mut third = pin!(client.execute(request(&corrupted())));
    assert!(poll(third.as_mut()).is_pending());
    gate.set(true);
    assert!(poll(serve.as_mut()).is_pending());
    assert!(
        poll(third.as_mut()).is_pending(),
        "the discarded response was erased"
    );
    assert!(poll(serve.as_mut()).is_pending());
    let Poll::Ready(Ok(response)) = poll(third.as_mut()) else {
        panic!("the third response is ready");
    };
    assert_eq!(
        plaintext(response),
        Err(oer_crypto::KeyUnwrapError::IntegrityCheckFailed)
    );
}

#[test]
fn an_unwrap_never_takes_the_response_a_wait_left_in_its_seat() {
    let service = service();
    let (engine, _, _) = engine();
    let mut client = service.client().expect("a seat");
    let mut serve = pin!(service.serve(engine));
    let mut first = Box::pin(client.execute(request(&RFC3394_4_1_CIPHERTEXT)));
    assert!(poll(first.as_mut()).is_pending());
    core::mem::forget(first);
    assert!(poll(serve.as_mut()).is_pending());
    let mut operation = pin!(client.unwrap_key(KeyUnwrapRequest::new(
        Aes128Key::copy_from(&RFC3394_4_1_KEK),
        KeyData::try_copy(&corrupted()).expect("fits"),
    )));
    assert!(poll(operation.as_mut()).is_pending());
    assert!(poll(serve.as_mut()).is_pending());
    let Poll::Ready(done) = poll(operation.as_mut()) else {
        panic!("the unwrap is done");
    };
    assert_eq!(
        done.response.plaintext(),
        Err(oer_crypto::KeyUnwrapError::IntegrityCheckFailed)
    );
}

#[test]
fn the_seats_storage_sets_the_client_count_and_clients_of_any_count_share_one_type() {
    // Six seats, as an image with more clients than the default keeps.
    let six = seated::<6>();
    let clients: std::vec::Vec<AesClient<'static, HostContext, N>> =
        (0..6).map(|_| six.client().expect("a seat")).collect();
    assert!(matches!(six.client(), Err(Exhausted)));
    // A one-seat service's client has the same type.
    let one: AesClient<'static, HostContext, N> = seated::<1>().client().expect("a seat");
    drop(one);
    drop(clients);
    assert!(six.client().is_ok());
    // All 32 seats of the largest storage are claimable.
    let full = seated::<32>();
    let all: std::vec::Vec<_> = (0..32).map(|_| full.client().expect("a seat")).collect();
    assert!(matches!(full.client(), Err(Exhausted)));
    drop(all);
}

#[test]
fn a_request_on_the_last_of_six_seats_is_served() {
    let service = seated::<6>();
    let mut clients: std::vec::Vec<_> = (0..6).map(|_| service.client().expect("a seat")).collect();
    let last = clients.last_mut().expect("six clients");
    let mut wait = pin!(last.execute(request(&RFC3394_4_1_CIPHERTEXT)));
    assert!(poll(wait.as_mut()).is_pending());
    let (engine, _, ran) = engine();
    let mut serve = pin!(service.serve(engine));
    assert!(poll(serve.as_mut()).is_pending());
    match poll(wait.as_mut()) {
        Poll::Ready(Ok(response)) => {
            assert_eq!(
                plaintext(response).expect("unwrapped"),
                RFC3394_4_1_KEY_DATA
            )
        }
        _ => panic!("the sixth seat's request was not served"),
    }
    assert_eq!(ran.get(), 1);
}
