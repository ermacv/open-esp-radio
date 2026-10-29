use crate::{
    Aes128Key, KeyData, KeyDataCapacityError, KeyUnwrap, KeyUnwrapDone, KeyUnwrapError,
    KeyUnwrapRequest, KeyUnwrapResponse,
    block::{Aes128Blocks, Block, BlockCipher, BlockKeyUnwrap},
    conformance::{self, RFC3394_4_1_CIPHERTEXT, RFC3394_4_1_KEK, RFC3394_4_1_KEY_DATA},
    rfc3394::{KeyWrapError, Rfc3394Step, Rfc3394Unwrap, wrap},
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

/// An engine that performs one block per poll: the machine is resumable.
struct Stepping;

#[expect(
    clippy::large_enum_variant,
    reason = "a test operation holds its machine in place"
)]
enum SteppingOperation {
    Running(Rfc3394Step<64>, RustKeyed),
    Done(KeyUnwrapResponse<64>),
    Taken,
}

impl Future for SteppingOperation {
    type Output = KeyUnwrapDone<Stepping, 64>;
    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match core::mem::replace(this, Self::Taken) {
            Self::Running(step, mut cipher) => match step {
                Rfc3394Step::Block { mut input, pending } => {
                    cipher.decrypt_block(input.as_mut_bytes());
                    *this = Self::Running(pending.complete(Ok(input)), cipher);
                    context.waker().wake_by_ref();
                    Poll::Pending
                }
                Rfc3394Step::Complete(response) => Poll::Ready(KeyUnwrapDone {
                    engine: Stepping,
                    response,
                }),
            },
            Self::Done(response) => Poll::Ready(KeyUnwrapDone {
                engine: Stepping,
                response,
            }),
            Self::Taken => Poll::Pending,
        }
    }
}

impl KeyUnwrap<64> for Stepping {
    type Operation = SteppingOperation;
    fn unwrap_key(self, request: KeyUnwrapRequest<64>) -> SteppingOperation {
        let (kek, wrapped) = request.into_parts();
        match Rfc3394Unwrap::start(wrapped) {
            Ok(machine) => SteppingOperation::Running(machine.next_block(), Rust.load_key(&kek)),
            Err(response) => SteppingOperation::Done(response),
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
fn an_engine_yielding_between_blocks_passes_the_same_cases() {
    let mut drives = 0;
    let _ = conformance::key_unwrap_rfc3394(Stepping, || drives += 1);
    // §4.1 is two semiblocks: six passes of two blocks, one block per poll,
    // for the two valid runs and the changed ciphertext. Length refusals
    // complete at once.
    assert_eq!(drives, 3 * 12);
}

#[test]
fn the_machine_unwraps_one_block_per_step() {
    let wrapped = KeyData::<32>::try_copy(&RFC3394_4_1_CIPHERTEXT).unwrap();
    let mut step = Rfc3394Unwrap::start(wrapped).ok().unwrap().next_block();
    let mut cipher = Rust.load_key(&Aes128Key::copy_from(&RFC3394_4_1_KEK));
    let mut steps = 0;
    let response = loop {
        step = match step {
            Rfc3394Step::Block { mut input, pending } => {
                steps += 1;
                cipher.decrypt_block(input.as_mut_bytes());
                pending.complete(Ok(input))
            }
            Rfc3394Step::Complete(response) => break response,
        };
    };
    assert_eq!(steps, 12);
    assert_eq!(response.plaintext(), Ok(&RFC3394_4_1_KEY_DATA[..]));
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

#[test]
fn an_engine_error_terminates_the_mode_without_exposing_key_data() {
    use crate::block::{BlockError, CipherBlock};
    let wrapped = KeyData::<32>::try_copy(&RFC3394_4_1_CIPHERTEXT).unwrap();
    let machine = Rfc3394Unwrap::start(wrapped).ok().unwrap();
    let Rfc3394Step::Block { input, pending } = machine.next_block() else {
        panic!("first block")
    };
    let mut expected = [0; 16];
    expected[..8].copy_from_slice(&RFC3394_4_1_CIPHERTEXT[..8]);
    expected[7] ^= 12; // n*j+i = 2*5+2, big endian.
    expected[8..].copy_from_slice(&RFC3394_4_1_CIPHERTEXT[16..]);
    assert_eq!(input.as_bytes(), &expected);
    drop(input);
    let error = BlockError::Timeout { observation: 1 };
    let Rfc3394Step::Complete(response) = pending.complete(Err(error)) else {
        panic!("error is terminal")
    };
    assert_eq!(response.plaintext(), Err(KeyUnwrapError::Engine(error)));
    assert_eq!(response.data.len, 0);
    assert!(response.data.bytes.iter().all(|byte| *byte == 0));
    fn erased<T: ZeroizeOnDrop>() {}
    erased::<CipherBlock>();
}
