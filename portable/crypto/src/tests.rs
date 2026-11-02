use crate::{
    Aes128Key, KeyData, KeyDataCapacityError, KeyUnwrap, KeyUnwrapDone, KeyUnwrapError,
    KeyUnwrapRequest, KeyUnwrapResponse,
    block::{Aes128Blocks, Block, BlockCipher, BlockKeyUnwrap},
    conformance::{self, RFC3394_4_1_CIPHERTEXT, RFC3394_4_1_KEK, RFC3394_4_1_KEY_DATA},
    rfc3394::{KeyWrapError, wrap},
};
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use zeroize::ZeroizeOnDrop;

/// RustCrypto AES-128 as the synchronous block primitive.
struct Rust;
struct RustKeyed(aes::Aes128);

impl BlockCipher for RustKeyed {
    fn encrypt_block(&mut self, block: &mut Block) {
        let mut array = GenericArray::from(*block);
        self.0.encrypt_block(&mut array);
        *block = array.into();
    }
    fn decrypt_block(&mut self, block: &mut Block) {
        let mut array = GenericArray::from(*block);
        self.0.decrypt_block(&mut array);
        *block = array.into();
    }
}

impl Aes128Blocks for Rust {
    type Keyed<'a> = RustKeyed;
    fn load_key(&mut self, key: &Aes128Key) -> RustKeyed {
        RustKeyed(aes::Aes128::new(&GenericArray::from(*key.as_bytes())))
    }
}

/// An engine whose operation is pending on its first poll and unwraps on the
/// next, as a service client's would.
struct Deferred;

struct DeferredOperation {
    yielded: bool,
    request: Option<KeyUnwrapRequest<64>>,
}

impl Future for DeferredOperation {
    type Output = KeyUnwrapDone<Deferred, 64>;
    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if !this.yielded {
            this.yielded = true;
            context.waker().wake_by_ref();
            return Poll::Pending;
        }
        match this.request.take() {
            Some(request) => Poll::Ready(KeyUnwrapDone {
                engine: Deferred,
                response: crate::block::unwrap_now(&mut Rust, request),
            }),
            None => Poll::Pending,
        }
    }
}

impl KeyUnwrap<64> for Deferred {
    type Operation = DeferredOperation;
    fn unwrap_key(self, request: KeyUnwrapRequest<64>) -> DeferredOperation {
        DeferredOperation {
            yielded: false,
            request: Some(request),
        }
    }
}

#[test]
fn the_synchronous_engine_passes_the_rfc3394_conformance_cases() {
    let engine = BlockKeyUnwrap::new(Rust);
    let engine = conformance::key_unwrap_rfc3394::<_, 32>(engine, || {
        panic!("a synchronous operation is ready on its first poll")
    });
    let _ = conformance::key_unwrap_rfc3394::<_, 512>(engine, || {
        panic!("a synchronous operation is ready on its first poll")
    });
}

#[test]
fn a_pending_engine_passes_the_same_cases_through_its_driver() {
    let mut drives = 0;
    let _ = conformance::key_unwrap_rfc3394(Deferred, || drives += 1);
    // Each of the five conformance requests is pending once.
    assert_eq!(drives, 5);
}

#[test]
fn wrap_produces_the_rfc3394_vector_and_unwrap_reverses_longer_key_data() {
    let mut cipher = Rust.load_key(&Aes128Key::copy_from(&RFC3394_4_1_KEK));
    let wrapped = wrap::<32>(&mut cipher, &RFC3394_4_1_KEY_DATA).unwrap();
    assert_eq!(wrapped.as_bytes(), RFC3394_4_1_CIPHERTEXT);

    let plain: [u8; 40] = core::array::from_fn(|index| index as u8 ^ 0x5a);
    let wrapped = wrap::<48>(&mut cipher, &plain).unwrap();
    assert_eq!(wrapped.as_bytes().len(), 48);
    let response = BlockKeyUnwrap::new(Rust).unwrap_now(KeyUnwrapRequest::new(
        Aes128Key::copy_from(&RFC3394_4_1_KEK),
        wrapped,
    ));
    assert_eq!(response.plaintext(), Ok(&plain[..]));
}

#[test]
fn wrap_refuses_data_outside_rfc3394_and_its_capacity() {
    let mut cipher = Rust.load_key(&Aes128Key::copy_from(&RFC3394_4_1_KEK));
    assert_eq!(
        wrap::<32>(&mut cipher, &[0; 8]).err(),
        Some(KeyWrapError::InvalidLength)
    );
    assert_eq!(
        wrap::<32>(&mut cipher, &[0; 17]).err(),
        Some(KeyWrapError::InvalidLength)
    );
    assert_eq!(
        wrap::<23>(&mut cipher, &[0; 16]).err(),
        Some(KeyWrapError::CapacityExceeded)
    );
}

#[test]
fn refused_unwraps_return_the_cleared_buffer() {
    let mut engine = BlockKeyUnwrap::new(Rust);
    let mut changed = RFC3394_4_1_CIPHERTEXT;
    changed[0] ^= 0x80;
    let response = engine.unwrap_now(KeyUnwrapRequest::new(
        Aes128Key::copy_from(&RFC3394_4_1_KEK),
        KeyData::<32>::try_copy(&changed).unwrap(),
    ));
    assert_eq!(
        response.into_plaintext().err(),
        Some(KeyUnwrapError::IntegrityCheckFailed)
    );
    let response = engine.unwrap_now(KeyUnwrapRequest::new(
        Aes128Key::copy_from(&RFC3394_4_1_KEK),
        KeyData::<32>::try_copy(&[]).unwrap(),
    ));
    assert_eq!(response.plaintext(), Err(KeyUnwrapError::InvalidLength));
}

#[test]
fn an_engine_that_ran_nothing_returns_an_empty_engine_refusal() {
    let response = KeyUnwrapResponse::<32>::engine_failed(crate::block::BlockError::Unavailable);
    assert_eq!(
        response.plaintext(),
        Err(KeyUnwrapError::Engine(
            crate::block::BlockError::Unavailable
        ))
    );
}

/// A hardware-like engine: RustCrypto blocks until it refuses, either the
/// key or the block numbered `refuse_block`.
struct Refusing {
    refuse_key: Option<crate::block::BlockError>,
    refuse_block: usize,
    blocks: usize,
}
struct RefusingKeyed<'a> {
    engine: &'a mut Refusing,
    cipher: RustKeyed,
}
impl crate::block::BlockEngine for RefusingKeyed<'_> {
    fn encrypt(&mut self, block: &mut Block) -> Result<(), crate::block::BlockError> {
        self.decrypt(block)
    }
    fn decrypt(&mut self, block: &mut Block) -> Result<(), crate::block::BlockError> {
        self.engine.blocks += 1;
        if self.engine.blocks == self.engine.refuse_block {
            return Err(crate::block::BlockError::Timeout { observation: 1 });
        }
        self.cipher.decrypt_block(block);
        Ok(())
    }
}
impl crate::block::Aes128Engine for Refusing {
    type Keyed<'a> = RefusingKeyed<'a>;
    fn load(&mut self, key: &Aes128Key) -> Result<RefusingKeyed<'_>, crate::block::BlockError> {
        if let Some(error) = self.refuse_key {
            return Err(error);
        }
        let cipher = Rust.load_key(key);
        Ok(RefusingKeyed {
            engine: self,
            cipher,
        })
    }
}

fn rfc3394_request() -> KeyUnwrapRequest<32> {
    KeyUnwrapRequest::new(
        Aes128Key::copy_from(&RFC3394_4_1_KEK),
        KeyData::try_copy(&RFC3394_4_1_CIPHERTEXT).unwrap(),
    )
}

#[test]
fn a_fallible_engine_unwraps_in_place_and_its_refusals_are_typed_engine_failures() {
    let mut engine = Refusing {
        refuse_key: None,
        refuse_block: usize::MAX,
        blocks: 0,
    };
    let response = crate::block::unwrap_now(&mut engine, rfc3394_request());
    assert_eq!(response.plaintext(), Ok(&RFC3394_4_1_KEY_DATA[..]));
    assert_eq!(engine.blocks, 12, "two 64-bit blocks: six rounds of two");

    let mut engine = Refusing {
        refuse_key: None,
        refuse_block: 3,
        blocks: 0,
    };
    let response = crate::block::unwrap_now(&mut engine, rfc3394_request());
    assert_eq!(
        response.plaintext(),
        Err(KeyUnwrapError::Engine(crate::block::BlockError::Timeout {
            observation: 1
        }))
    );
    assert_eq!(engine.blocks, 3, "no block after the refused one");

    let mut engine = BlockKeyUnwrap::new(Refusing {
        refuse_key: Some(crate::block::BlockError::InvalidState { observation: 2 }),
        refuse_block: usize::MAX,
        blocks: 0,
    });
    let response = engine.unwrap_now(rfc3394_request());
    assert_eq!(
        response.plaintext(),
        Err(KeyUnwrapError::Engine(
            crate::block::BlockError::InvalidState { observation: 2 }
        ))
    );
    assert_eq!(engine.into_blocks().blocks, 0);
}

#[test]
fn key_data_refuses_more_than_its_capacity() {
    assert_eq!(
        KeyData::<8>::try_copy(&[0; 9]).err(),
        Some(KeyDataCapacityError)
    );
    assert_eq!(KeyData::<8>::try_copy(&[7; 8]).unwrap().as_bytes(), [7; 8]);
}

#[test]
fn keys_and_key_data_erase_themselves_when_dropped() {
    fn erased<T: ZeroizeOnDrop>() {}
    erased::<Aes128Key>();
    erased::<KeyData<32>>();
}

/// An engine that records the first block it is given and refuses it.
struct RecordingRefusal {
    first: Option<Block>,
}
impl crate::block::BlockEngine for &mut RecordingRefusal {
    fn encrypt(&mut self, block: &mut Block) -> Result<(), crate::block::BlockError> {
        self.decrypt(block)
    }
    fn decrypt(&mut self, block: &mut Block) -> Result<(), crate::block::BlockError> {
        self.first = Some(*block);
        Err(crate::block::BlockError::Timeout { observation: 1 })
    }
}

#[test]
fn an_engine_error_terminates_the_mode_without_exposing_key_data() {
    use crate::block::BlockError;
    let wrapped = KeyData::<32>::try_copy(&RFC3394_4_1_CIPHERTEXT).unwrap();
    let mut engine = RecordingRefusal { first: None };
    let response = crate::rfc3394::unwrap(wrapped, || Ok(&mut engine));
    let mut expected = [0; 16];
    expected[..8].copy_from_slice(&RFC3394_4_1_CIPHERTEXT[..8]);
    expected[7] ^= 12; // n*j+i = 2*5+2, big endian.
    expected[8..].copy_from_slice(&RFC3394_4_1_CIPHERTEXT[16..]);
    assert_eq!(engine.first, Some(expected));
    let error = BlockError::Timeout { observation: 1 };
    assert_eq!(response.plaintext(), Err(KeyUnwrapError::Engine(error)));
    assert_eq!(response.data.len, 0);
    assert!(response.data.bytes.iter().all(|byte| *byte == 0));
}

#[test]
fn a_length_refusal_loads_no_key() {
    let wrapped = KeyData::<32>::try_copy(&RFC3394_4_1_CIPHERTEXT[..16]).unwrap();
    let mut loads = 0;
    let response = crate::rfc3394::unwrap(wrapped, || {
        loads += 1;
        Ok(Rust.load_key(&Aes128Key::copy_from(&RFC3394_4_1_KEK)))
    });
    assert_eq!(response.plaintext(), Err(KeyUnwrapError::InvalidLength));
    assert_eq!(loads, 0);
}
