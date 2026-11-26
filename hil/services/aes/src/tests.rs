use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use oer_portable_crypto::block::{Block, BlockEngine};

use super::*;
use oer_protocols_hil_protocol::Message;

/// RustCrypto's AES-128 as the engine's block primitive.
struct Software;

struct Keyed(aes::Aes128);

impl BlockEngine for Keyed {
    type Stop = EngineStopped;
    fn encrypt(&mut self, block: &mut Block) -> Result<(), EngineStopped> {
        self.0.encrypt_block(block.into());
        Ok(())
    }
    fn decrypt(&mut self, block: &mut Block) -> Result<(), EngineStopped> {
        self.0.decrypt_block(block.into());
        Ok(())
    }
}

impl Aes128Engine for Software {
    type Stop = EngineStopped;
    type Keyed<'a> = Keyed;
    fn load(&mut self, key: &Aes128Key) -> Result<Keyed, EngineStopped> {
        Ok(Keyed(aes::Aes128::new(key.as_bytes().into())))
    }
}

/// The software engine never stops: its record stays empty. The reader is a
/// closure, as the image's is: a generic parameter, called directly.
fn engine() -> Engine<Software, impl Fn(&Software) -> Option<EngineObservation>> {
    Engine::new(Software, |_: &Software| None)
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

fn unwrap<E, R>(handler: &mut Engine<E, R>, wrapped: &str) -> Unwrapped
where
    E: Aes128Engine<Stop = EngineStopped>,
    R: Fn(&E) -> Option<EngineObservation>,
{
    embassy_futures::block_on(handler.unwrap_key(UnwrapKey {
        kek: KEK,
        wrapped: hex(wrapped),
    }))
}

/// A hardware-like engine that refuses every key and records, as a driver
/// does, what it observed.
struct Refusing {
    observed: Option<EngineObservation>,
    last: Option<EngineObservation>,
}

struct Never;

impl BlockEngine for Never {
    type Stop = EngineStopped;
    fn encrypt(&mut self, _: &mut Block) -> Result<(), EngineStopped> {
        Err(EngineStopped)
    }
    fn decrypt(&mut self, _: &mut Block) -> Result<(), EngineStopped> {
        Err(EngineStopped)
    }
}

impl Aes128Engine for Refusing {
    type Stop = EngineStopped;
    type Keyed<'a> = Never;
    fn load(&mut self, _: &Aes128Key) -> Result<Never, EngineStopped> {
        self.last = self.observed;
        Err(EngineStopped)
    }
}

/// A stop answers what the engine's record holds, as the record reader
/// reads it, and the next request is served again.
#[test]
fn an_engine_stop_answers_its_record_and_serves_the_next_request() {
    for observed in [
        Some(EngineObservation::Timeout { observation: 3 }),
        Some(EngineObservation::InvalidState { observation: 5 }),
        None,
    ] {
        let mut handler = Engine::new(
            Refusing {
                observed,
                last: None,
            },
            |engine: &Refusing| engine.last,
        );
        for _ in 0..2 {
            assert_eq!(
                unwrap(
                    &mut handler,
                    "1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cfe5"
                ),
                Unwrapped::Refused(UnwrapError::EngineStopped { last: observed })
            );
        }
    }
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
    use oer_protocols_hil_agent::base::Service;
    assert_eq!(Request::REQUESTS, [UnwrapKey::KEY]);
    assert_eq!(Request::KEYS, [UnwrapKey::KEY, Unwrapped::KEY]);
}
