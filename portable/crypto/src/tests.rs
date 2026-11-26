use crate::{
    Aes128Key, KeyData, KeyDataCapacityError, KeyUnwrapError, KeyUnwrapRequest, KeyWrapRequest,
    block::{Aes128Engine, Block, BlockEngine, EngineStopped, unwrap_now, wrap_now},
    conformance::{self, RFC3394_4_1_CIPHERTEXT, RFC3394_4_1_KEK, RFC3394_4_1_KEY_DATA},
    rfc3394::{self, KeyWrapError},
    test_engine,
};
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};
use zeroize::ZeroizeOnDrop;

/// RustCrypto AES-128 as the synchronous block primitive.
struct Rust;
struct RustKeyed(aes::Aes128);

impl BlockEngine for RustKeyed {
    type Stop = core::convert::Infallible;
    fn encrypt(&mut self, block: &mut Block) -> Result<(), Self::Stop> {
        let mut array = GenericArray::from(*block);
        self.0.encrypt_block(&mut array);
        *block = array.into();
        Ok(())
    }
    fn decrypt(&mut self, block: &mut Block) -> Result<(), Self::Stop> {
        let mut array = GenericArray::from(*block);
        self.0.decrypt_block(&mut array);
        *block = array.into();
        Ok(())
    }
}

impl Aes128Engine for Rust {
    type Stop = core::convert::Infallible;
    type Keyed<'a> = RustKeyed;
    fn load(&mut self, key: &Aes128Key) -> Result<RustKeyed, Self::Stop> {
        Ok(RustKeyed(aes::Aes128::new(&GenericArray::from(
            *key.as_bytes(),
        ))))
    }
}

/// The software engine's key, loaded: it cannot refuse.
fn rust_keyed(key: &Aes128Key) -> RustKeyed {
    let Ok(keyed) = Rust.load(key);
    keyed
}

#[test]
fn the_synchronous_engine_passes_the_rfc3394_conformance_cases() {
    conformance::key_unwrap_rfc3394::<_, 32>(&mut Rust);
    conformance::key_unwrap_rfc3394::<_, 512>(&mut Rust);
}

fn wrap_request<const N: usize>(plaintext: &[u8]) -> KeyWrapRequest<N> {
    KeyWrapRequest::new(
        Aes128Key::copy_from(&RFC3394_4_1_KEK),
        KeyData::try_copy(plaintext).unwrap(),
    )
}

#[test]
fn wrap_produces_the_rfc3394_vector_and_unwrap_reverses_longer_key_data() {
    let response = wrap_now(&mut Rust, wrap_request::<32>(&RFC3394_4_1_KEY_DATA));
    assert_eq!(response.wrapped(), Ok(&RFC3394_4_1_CIPHERTEXT[..]));

    let plain: [u8; 40] = core::array::from_fn(|index| index as u8 ^ 0x5a);
    let wrapped = wrap_now(&mut Rust, wrap_request::<48>(&plain))
        .into_wrapped()
        .unwrap();
    assert_eq!(wrapped.as_bytes().len(), 48);
    let response = unwrap_now(
        &mut Rust,
        KeyUnwrapRequest::new(Aes128Key::copy_from(&RFC3394_4_1_KEK), wrapped),
    );
    assert_eq!(response.plaintext(), Ok(&plain[..]));
}

#[test]
fn wrap_refuses_data_outside_rfc3394_and_its_capacity() {
    assert_eq!(
        wrap_now(&mut Rust, wrap_request::<32>(&[0; 8])).wrapped(),
        Err(KeyWrapError::InvalidLength)
    );
    assert_eq!(
        wrap_now(&mut Rust, wrap_request::<32>(&[0; 17])).wrapped(),
        Err(KeyWrapError::InvalidLength)
    );
    assert_eq!(
        wrap_now(&mut Rust, wrap_request::<23>(&[0; 16])).wrapped(),
        Err(KeyWrapError::CapacityExceeded)
    );
    // Twenty-four octets of capacity hold sixteen and their register.
    assert!(
        wrap_now(&mut Rust, wrap_request::<24>(&[0; 16]))
            .wrapped()
            .is_ok()
    );
}

/// A length refusal loads no key; a refused block ends the wrap with the
/// engine's error and the buffer cleared.
#[test]
fn a_refused_wrap_loads_no_key_or_clears_its_buffer() {
    let mut loaded = false;
    let response =
        rfc3394::wrap::<test_engine::Engine, 32>(KeyData::try_copy(&[0xa5; 8]).unwrap(), || {
            loaded = true;
            Ok(test_engine::Engine::new(&RFC3394_4_1_KEK))
        });
    assert_eq!(response.wrapped(), Err(KeyWrapError::InvalidLength));
    assert!(!loaded);
    assert!(erased(&response));
    for refuse_from in [0, 1, 5, 11] {
        let response = rfc3394::wrap::<test_engine::Engine, 32>(
            KeyData::try_copy(&RFC3394_4_1_KEY_DATA).unwrap(),
            || {
                Ok(test_engine::Engine::refusing_from(
                    &RFC3394_4_1_KEK,
                    refuse_from,
                ))
            },
        );
        assert_eq!(
            response.wrapped(),
            Err(KeyWrapError::Engine(test_engine::REFUSAL)),
            "{refuse_from}"
        );
        assert!(erased(&response), "{refuse_from}");
        assert_eq!(
            response.into_wrapped().err(),
            Some(KeyWrapError::Engine(test_engine::REFUSAL))
        );
    }
    let response = rfc3394::wrap::<test_engine::Engine, 32>(
        KeyData::try_copy(&RFC3394_4_1_KEY_DATA).unwrap(),
        || Err(test_engine::REFUSAL),
    );
    assert_eq!(
        response.wrapped(),
        Err(KeyWrapError::Engine(test_engine::REFUSAL))
    );
    assert!(erased(&response));
    // Twelve blocks (two semiblocks, six passes) wrap the vector.
    let response = rfc3394::wrap::<test_engine::Engine, 32>(
        KeyData::try_copy(&RFC3394_4_1_KEY_DATA).unwrap(),
        || Ok(test_engine::Engine::refusing_from(&RFC3394_4_1_KEK, 12)),
    );
    assert_eq!(response.wrapped(), Ok(&RFC3394_4_1_CIPHERTEXT[..]));
}

#[test]
fn refused_unwraps_return_the_cleared_buffer() {
    let mut engine = Rust;
    let mut changed = RFC3394_4_1_CIPHERTEXT;
    changed[0] ^= 0x80;
    let response = unwrap_now(
        &mut engine,
        KeyUnwrapRequest::new(
            Aes128Key::copy_from(&RFC3394_4_1_KEK),
            KeyData::<32>::try_copy(&changed).unwrap(),
        ),
    );
    assert_eq!(
        response.into_plaintext().err(),
        Some(KeyUnwrapError::IntegrityCheckFailed)
    );
    let response = unwrap_now(
        &mut engine,
        KeyUnwrapRequest::new(
            Aes128Key::copy_from(&RFC3394_4_1_KEK),
            KeyData::<32>::try_copy(&[]).unwrap(),
        ),
    );
    assert_eq!(response.plaintext(), Err(KeyUnwrapError::InvalidLength));
}

/// A hardware-like engine: RustCrypto blocks until it refuses, either the
/// key or the block numbered `refuse_block`.
struct Refusing {
    refuse_key: bool,
    refuse_block: usize,
    blocks: usize,
}
struct RefusingKeyed<'a> {
    engine: &'a mut Refusing,
    cipher: RustKeyed,
}
impl BlockEngine for RefusingKeyed<'_> {
    type Stop = EngineStopped;
    fn encrypt(&mut self, block: &mut Block) -> Result<(), EngineStopped> {
        self.decrypt(block)
    }
    fn decrypt(&mut self, block: &mut Block) -> Result<(), EngineStopped> {
        self.engine.blocks += 1;
        if self.engine.blocks == self.engine.refuse_block {
            return Err(EngineStopped);
        }
        let Ok(()) = self.cipher.decrypt(block);
        Ok(())
    }
}
impl Aes128Engine for Refusing {
    type Stop = EngineStopped;
    type Keyed<'a> = RefusingKeyed<'a>;
    fn load(&mut self, key: &Aes128Key) -> Result<RefusingKeyed<'_>, EngineStopped> {
        if self.refuse_key {
            return Err(EngineStopped);
        }
        let cipher = rust_keyed(key);
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
        refuse_key: false,
        refuse_block: usize::MAX,
        blocks: 0,
    };
    let response = crate::block::unwrap_now(&mut engine, rfc3394_request());
    assert_eq!(response.plaintext(), Ok(&RFC3394_4_1_KEY_DATA[..]));
    assert_eq!(engine.blocks, 12, "two 64-bit blocks: six rounds of two");

    let mut engine = Refusing {
        refuse_key: false,
        refuse_block: 3,
        blocks: 0,
    };
    let response = crate::block::unwrap_now(&mut engine, rfc3394_request());
    assert_eq!(
        response.plaintext(),
        Err(KeyUnwrapError::Engine(EngineStopped))
    );
    assert_eq!(engine.blocks, 3, "no block after the refused one");

    let mut engine = Refusing {
        refuse_key: true,
        refuse_block: usize::MAX,
        blocks: 0,
    };
    let response = crate::block::unwrap_now(&mut engine, rfc3394_request());
    assert_eq!(
        response.plaintext(),
        Err(KeyUnwrapError::Engine(EngineStopped))
    );
    assert_eq!(engine.blocks, 0);
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
impl BlockEngine for &mut RecordingRefusal {
    type Stop = EngineStopped;
    fn encrypt(&mut self, block: &mut Block) -> Result<(), EngineStopped> {
        self.decrypt(block)
    }
    fn decrypt(&mut self, block: &mut Block) -> Result<(), EngineStopped> {
        self.first = Some(*block);
        Err(EngineStopped)
    }
}

#[test]
fn an_engine_error_terminates_the_mode_without_exposing_key_data() {
    let wrapped = KeyData::<32>::try_copy(&RFC3394_4_1_CIPHERTEXT).unwrap();
    let mut engine = RecordingRefusal { first: None };
    let response = crate::rfc3394::unwrap(wrapped, || Ok(&mut engine));
    let mut expected = [0; 16];
    expected[..8].copy_from_slice(&RFC3394_4_1_CIPHERTEXT[..8]);
    expected[7] ^= 12; // n*j+i = 2*5+2, big endian.
    expected[8..].copy_from_slice(&RFC3394_4_1_CIPHERTEXT[16..]);
    assert_eq!(engine.first, Some(expected));
    assert_eq!(
        response.plaintext(),
        Err(KeyUnwrapError::Engine(EngineStopped))
    );
    assert_eq!(response.data.len, 0);
    assert!(response.data.bytes.iter().all(|byte| *byte == 0));
}

#[test]
fn a_length_refusal_loads_no_key() {
    let wrapped = KeyData::<32>::try_copy(&RFC3394_4_1_CIPHERTEXT[..16]).unwrap();
    let mut loads = 0;
    let response = crate::rfc3394::unwrap(wrapped, || {
        loads += 1;
        Rust.load(&Aes128Key::copy_from(&RFC3394_4_1_KEK))
    });
    assert_eq!(response.plaintext(), Err(KeyUnwrapError::InvalidLength));
    assert_eq!(loads, 0);
}

/// Whether a response's whole buffer, beyond its length too, is zero.
fn erased<const N: usize, S>(response: &crate::KeyWrapResponse<N, S>) -> bool {
    response.data.len == 0 && response.data.bytes.iter().all(|byte| *byte == 0)
}
