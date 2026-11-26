use crate::{
    Aes128Key,
    block::{Aes128Engine, Block, BlockEngine, EngineStopped},
    conformance::{self, RFC3394_4_1_CIPHERTEXT, RFC3394_4_1_KEK, RFC3394_4_1_KEY_DATA},
    rfc3394::{KeyUnwrapError, KeyWrapError, unwrap, wrap},
    test_engine,
};
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};

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

#[test]
fn the_synchronous_engine_passes_the_rfc3394_conformance_cases() {
    conformance::key_unwrap_rfc3394(&mut Rust);
}

fn kek() -> Aes128Key {
    Aes128Key::copy_from(&RFC3394_4_1_KEK)
}

/// A buffer of a semiblock of room, then `plain`.
fn room_and<const N: usize>(plain: &[u8]) -> [u8; N] {
    let mut buffer = [0xc3; N];
    buffer[8..].copy_from_slice(plain);
    buffer
}

#[test]
fn wrap_produces_the_rfc3394_vector_and_unwrap_reverses_longer_key_data() {
    let mut buffer = room_and::<24>(&RFC3394_4_1_KEY_DATA);
    assert_eq!(wrap(&mut Rust, &kek(), &mut buffer), Ok(()));
    assert_eq!(buffer, RFC3394_4_1_CIPHERTEXT);

    let plain: [u8; 40] = core::array::from_fn(|index| index as u8 ^ 0x5a);
    let mut buffer = room_and::<48>(&plain);
    assert_eq!(wrap(&mut Rust, &kek(), &mut buffer), Ok(()));
    let plaintext = unwrap(&mut Rust, &kek(), &mut buffer).unwrap();
    assert_eq!(plaintext, &plain);
    assert_eq!(
        buffer[..8],
        [0xa6; 8],
        "the first semiblock holds the initial value"
    );
}

#[test]
fn wrap_refuses_data_outside_rfc3394_and_leaves_it() {
    for length in [0, 8, 16, 17, 25] {
        let mut buffer = std::vec![0x3c; length];
        assert_eq!(
            wrap(&mut Rust, &kek(), &mut buffer),
            Err(KeyWrapError::InvalidLength),
            "{length}"
        );
        assert!(buffer.iter().all(|byte| *byte == 0x3c), "{length}");
    }
    // Twenty-four octets hold a semiblock of room and sixteen of key data.
    assert_eq!(wrap(&mut Rust, &kek(), &mut [0; 24]), Ok(()));
}

/// An engine over [`test_engine::Engine`]: it counts its loads, refuses its
/// key with `refuse_key`, and its keyed view refuses every block from
/// `refuse_from` on.
struct Counted {
    loads: usize,
    refuse_key: bool,
    refuse_from: usize,
}

impl Counted {
    /// Blocks from `refuse_from` on are refused.
    const fn refusing_from(refuse_from: usize) -> Self {
        Self {
            loads: 0,
            refuse_key: false,
            refuse_from,
        }
    }

    /// The key is refused.
    const fn refusing_the_key() -> Self {
        Self {
            loads: 0,
            refuse_key: true,
            refuse_from: usize::MAX,
        }
    }
}

impl Aes128Engine for Counted {
    type Stop = EngineStopped;
    type Keyed<'a> = test_engine::Engine;
    fn load(&mut self, key: &Aes128Key) -> Result<test_engine::Engine, EngineStopped> {
        self.loads += 1;
        if self.refuse_key {
            return Err(EngineStopped);
        }
        Ok(test_engine::Engine::refusing_from(
            key.as_bytes(),
            self.refuse_from,
        ))
    }
}

/// A length refusal loads no key and leaves the buffer; a stop ends the
/// wrap with the engine's stop and the whole buffer cleared.
#[test]
fn a_refused_wrap_loads_no_key_or_clears_its_buffer() {
    let mut engine = Counted::refusing_from(usize::MAX);
    let mut buffer = [0xa5; 16];
    assert_eq!(
        wrap(&mut engine, &kek(), &mut buffer),
        Err(KeyWrapError::InvalidLength)
    );
    assert_eq!(engine.loads, 0);
    assert_eq!(buffer, [0xa5; 16]);
    for refuse_from in [0, 1, 5, 11] {
        let mut buffer = room_and::<24>(&RFC3394_4_1_KEY_DATA);
        assert_eq!(
            wrap(
                &mut Counted::refusing_from(refuse_from),
                &kek(),
                &mut buffer
            ),
            Err(KeyWrapError::Engine(test_engine::REFUSAL)),
            "{refuse_from}"
        );
        assert_eq!(buffer, [0; 24], "{refuse_from}");
    }
    let mut buffer = room_and::<24>(&RFC3394_4_1_KEY_DATA);
    assert_eq!(
        wrap(&mut Counted::refusing_the_key(), &kek(), &mut buffer),
        Err(KeyWrapError::Engine(test_engine::REFUSAL))
    );
    assert_eq!(buffer, [0; 24]);
    // Twelve blocks (two semiblocks, six passes) wrap the vector.
    let mut buffer = room_and::<24>(&RFC3394_4_1_KEY_DATA);
    assert_eq!(
        wrap(&mut Counted::refusing_from(12), &kek(), &mut buffer),
        Ok(())
    );
    assert_eq!(buffer, RFC3394_4_1_CIPHERTEXT);
}

#[test]
fn refused_unwraps_clear_the_buffer() {
    let mut changed = RFC3394_4_1_CIPHERTEXT;
    changed[0] ^= 0x80;
    assert_eq!(
        unwrap(&mut Rust, &kek(), &mut changed).err(),
        Some(KeyUnwrapError::IntegrityCheckFailed)
    );
    assert_eq!(changed, [0; 24]);
    assert_eq!(
        unwrap(&mut Rust, &kek(), &mut []).err(),
        Some(KeyUnwrapError::InvalidLength)
    );
}

#[test]
fn a_fallible_engine_unwraps_in_place_and_its_stops_are_typed_engine_failures() {
    let mut engine = Counted::refusing_from(usize::MAX);
    let mut buffer = RFC3394_4_1_CIPHERTEXT;
    assert_eq!(
        unwrap(&mut engine, &kek(), &mut buffer).map(|plaintext| &*plaintext),
        Ok(&RFC3394_4_1_KEY_DATA[..])
    );

    let mut buffer = RFC3394_4_1_CIPHERTEXT;
    assert_eq!(
        unwrap(&mut Counted::refusing_from(3), &kek(), &mut buffer).err(),
        Some(KeyUnwrapError::Engine(EngineStopped))
    );
    assert_eq!(buffer, [0; 24]);

    let mut engine = Counted::refusing_the_key();
    let mut buffer = RFC3394_4_1_CIPHERTEXT;
    assert_eq!(
        unwrap(&mut engine, &kek(), &mut buffer).err(),
        Some(KeyUnwrapError::Engine(EngineStopped))
    );
    assert_eq!(engine.loads, 1);
    assert_eq!(buffer, [0; 24]);
}

#[test]
fn keys_erase_themselves_when_dropped() {
    fn erased<T: zeroize::ZeroizeOnDrop>() {}
    erased::<Aes128Key>();
}

/// An engine whose keyed view records the first block it is given and
/// refuses it.
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
impl Aes128Engine for RecordingRefusal {
    type Stop = EngineStopped;
    type Keyed<'a> = &'a mut RecordingRefusal;
    fn load(&mut self, _key: &Aes128Key) -> Result<&mut RecordingRefusal, EngineStopped> {
        Ok(self)
    }
}

#[test]
fn an_engine_stop_terminates_the_mode_without_exposing_key_data() {
    let mut buffer = RFC3394_4_1_CIPHERTEXT;
    let mut engine = RecordingRefusal { first: None };
    assert_eq!(
        unwrap(&mut engine, &kek(), &mut buffer).err(),
        Some(KeyUnwrapError::Engine(EngineStopped))
    );
    let mut expected = [0; 16];
    expected[..8].copy_from_slice(&RFC3394_4_1_CIPHERTEXT[..8]);
    expected[7] ^= 12; // n*j+i = 2*5+2, big endian.
    expected[8..].copy_from_slice(&RFC3394_4_1_CIPHERTEXT[16..]);
    assert_eq!(engine.first, Some(expected));
    assert_eq!(buffer, [0; 24]);
}

#[test]
fn a_length_refusal_loads_no_key() {
    let mut engine = Counted::refusing_from(usize::MAX);
    let mut buffer = [0; 16];
    buffer.copy_from_slice(&RFC3394_4_1_CIPHERTEXT[..16]);
    assert_eq!(
        unwrap(&mut engine, &kek(), &mut buffer).err(),
        Some(KeyUnwrapError::InvalidLength)
    );
    assert_eq!(engine.loads, 0);
    assert_eq!(
        buffer[..],
        RFC3394_4_1_CIPHERTEXT[..16],
        "a length refusal leaves the buffer"
    );
}

/// An engine whose blocks come back as they went: not AES.
struct Identity;
impl BlockEngine for Identity {
    type Stop = core::convert::Infallible;
    fn encrypt(&mut self, _: &mut Block) -> Result<(), Self::Stop> {
        Ok(())
    }
    fn decrypt(&mut self, _: &mut Block) -> Result<(), Self::Stop> {
        Ok(())
    }
}
impl Aes128Engine for Identity {
    type Stop = core::convert::Infallible;
    type Keyed<'a> = Identity;
    fn load(&mut self, _: &Aes128Key) -> Result<Identity, Self::Stop> {
        Ok(Identity)
    }
}

/// The conformance cases fail an engine that breaks them.
#[test]
#[should_panic(expected = "RFC 3394 §4.1 unwraps")]
fn an_engine_that_is_not_aes_fails_the_conformance_cases() {
    conformance::key_unwrap_rfc3394(&mut Identity);
}
