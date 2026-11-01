use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use oer_crypto::block::{Aes128Blocks, Block, BlockCipher, BlockKeyUnwrap};

use super::*;
use oer_hil_protocol::Message;

/// RustCrypto's AES-128 as the engine's block primitive.
struct Software;

struct Keyed(aes::Aes128);

impl BlockCipher for Keyed {
    fn encrypt_block(&mut self, block: &mut Block) {
        self.0.encrypt_block(block.into());
    }
    fn decrypt_block(&mut self, block: &mut Block) {
        self.0.decrypt_block(block.into());
    }
}

impl Aes128Blocks for Software {
    type Keyed<'a> = Keyed;
    fn load_key(&mut self, key: &Aes128Key) -> Keyed {
        Keyed(aes::Aes128::new(key.as_bytes().into()))
    }
}

fn engine() -> Engine<BlockKeyUnwrap<Software>> {
    Engine::new(BlockKeyUnwrap::new(Software))
}

fn hex(text: &str) -> heapless::Vec<u8, MAX_WRAPPED> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
        .collect()
}

const KEK: [u8; 16] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
];

fn unwrap(handler: &mut Engine<BlockKeyUnwrap<Software>>, wrapped: &str) -> Unwrapped {
    embassy_futures::block_on(handler.unwrap_key(UnwrapKey {
        kek: KEK,
        wrapped: hex(wrapped),
    }))
}

#[test]
fn an_engine_of_a_larger_capacity_unwraps_the_same_request() {
    let mut handler: Engine<BlockKeyUnwrap<Software>, 512> =
        Engine::new(BlockKeyUnwrap::new(Software));
    let answer = embassy_futures::block_on(handler.unwrap_key(UnwrapKey {
        kek: KEK,
        wrapped: hex("1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cfe5"),
    }));
    assert_eq!(
        answer,
        Unwrapped::Plaintext(hex("00112233445566778899aabbccddeeff"))
    );
}

#[test]
fn the_handler_unwraps_rfc3394_4_1_and_keeps_its_engine_for_the_next_request() {
    let mut handler = engine();
    for _ in 0..2 {
        assert_eq!(
            unwrap(
                &mut handler,
                "1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cfe5"
            ),
            Unwrapped::Plaintext(hex("00112233445566778899aabbccddeeff"))
        );
    }
}

#[test]
fn refusals_are_typed_answers() {
    let mut handler = engine();
    assert_eq!(
        unwrap(
            &mut handler,
            "1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cfe4"
        ),
        Unwrapped::Refused(UnwrapError::IntegrityCheckFailed)
    );
    assert_eq!(
        unwrap(&mut handler, "1fa68b0a8112b447aef34bd8"),
        Unwrapped::Refused(UnwrapError::InvalidLength)
    );
    assert_eq!(
        unwrap(
            &mut handler,
            "1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cf"
        ),
        Unwrapped::Refused(UnwrapError::InvalidLength)
    );
}

#[test]
fn the_service_serves_exactly_its_endpoint() {
    use oer_hil_agent::base::Service;
    assert_eq!(Request::REQUESTS, [UnwrapKey::KEY]);
    assert_eq!(Request::KEYS, [UnwrapKey::KEY, Unwrapped::KEY]);
}

#[test]
fn every_engine_failure_has_its_own_answer() {
    for (error, expected) in [
        (BlockError::Unavailable, UnwrapError::EngineUnavailable),
        (
            BlockError::Timeout { observation: 3 },
            UnwrapError::EngineTimeout { observation: 3 },
        ),
        (
            BlockError::InvalidState { observation: 4 },
            UnwrapError::EngineInvalidState { observation: 4 },
        ),
        (BlockError::TimeOverflow, UnwrapError::EngineTimeOverflow),
    ] {
        assert_eq!(UnwrapError::from(KeyUnwrapError::Engine(error)), expected);
    }
}
