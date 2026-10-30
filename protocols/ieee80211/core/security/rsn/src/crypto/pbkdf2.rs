//! Bounded PBKDF2-HMAC-SHA1 work for the WPA2 passphrase mapping.

use super::*;

/// One owner of a pending PMK derivation. Each `step` evaluates at most one
/// HMAC; the caller schedules/yields between steps. Drop (including async
/// cancellation) clears the password, intermediate blocks and partial PMK.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct PmkDerivation {
    state: Option<State>,
}

#[derive(Zeroize, ZeroizeOnDrop)]
struct State {
    password: heapless::Vec<u8, PSK_PASSPHRASE_MAX_LEN, u8>,
    ssid: heapless::Vec<u8, PSK_SSID_MAX_LEN, u8>,
    previous: [u8; 20],
    sum: [u8; 20],
    output: [u8; 32],
    block: u32,
    round: Rounds,
}

/// A finite sequence of exactly one block's HMAC rounds.
struct Rounds(core::ops::Range<u32>);
impl Zeroize for Rounds {
    fn zeroize(&mut self) {
        self.0 = 0..0;
    }
}

impl PmkDerivation {
    pub fn new(password: &[u8], ssid: &[u8]) -> Result<Self, PskDerivationError> {
        if !(PSK_PASSPHRASE_MIN_LEN..=PSK_PASSPHRASE_MAX_LEN).contains(&password.len()) {
            return Err(PskDerivationError::InvalidPassphraseLength);
        }
        if ssid.is_empty() || ssid.len() > PSK_SSID_MAX_LEN {
            return Err(PskDerivationError::InvalidSsidLength);
        }
        let state = State {
            password: heapless::Vec::from_slice(password)
                .map_err(|_| PskDerivationError::InvalidPassphraseLength)?,
            ssid: heapless::Vec::from_slice(ssid)
                .map_err(|_| PskDerivationError::InvalidSsidLength)?,
            previous: [0; 20],
            sum: [0; 20],
            output: [0; 32],
            block: 1,
            round: Rounds(0..PSK_PBKDF2_ITERATIONS),
        };
        Ok(Self { state: Some(state) })
    }

    /// Returns the PMK once, after exactly 8192 steps. Afterwards this owner
    /// is erased and further steps do no work and return `None`.
    pub fn step(&mut self) -> Option<Pmk> {
        self.step_with(hmac_sha1)
    }

    fn step_with(&mut self, hmac: impl FnOnce(&[u8], &[u8], &[u8]) -> [u8; 20]) -> Option<Pmk> {
        let result = self.state.as_mut()?.advance(hmac);
        if result.is_some() {
            self.zeroize();
        }
        result
    }
}

impl State {
    fn advance(&mut self, hmac: impl FnOnce(&[u8], &[u8], &[u8]) -> [u8; 20]) -> Option<Pmk> {
        // FnOnce makes the one-HMAC step budget structural, also when a
        // block finishes. No second block is started in the same step.
        let round = self.round.0.next()?;
        self.previous = if round == 0 {
            hmac(
                self.password.as_slice(),
                self.ssid.as_slice(),
                &self.block.to_be_bytes(),
            )
        } else {
            hmac(self.password.as_slice(), &self.previous, &[])
        };
        for (sum, byte) in self.sum.iter_mut().zip(self.previous) {
            *sum ^= byte;
        }
        if !self.round.0.is_empty() {
            return None;
        }
        if self.block == 1 {
            for (target, source) in self.output.iter_mut().zip(&self.sum) {
                *target = *source;
            }
            self.block = 2;
            self.round = Rounds(0..PSK_PBKDF2_ITERATIONS);
            self.sum.zeroize();
            self.previous.zeroize();
            None
        } else {
            for (target, source) in self.output.iter_mut().skip(20).zip(&self.sum) {
                *target = *source;
            }
            Some(Pmk(self.output))
        }
    }
}

fn hmac_sha1(password: &[u8], first: &[u8], second: &[u8]) -> [u8; 20] {
    let mut mac = crate::mac::sha1(password);
    mac.update(first);
    mac.update(second);
    let mut digest = mac.finalize().into_bytes();
    let result = digest.into();
    digest.zeroize();
    result
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::disallowed_macros,
        clippy::disallowed_methods,
        clippy::arithmetic_side_effects,
        clippy::unreachable
    )]
    use super::*;

    #[test]
    fn rfc6070_and_previous_implementation_agree_one_hmac_per_step() {
        // RFC 6070 section 2, c=4096. WPA2 requests 32 bytes; the RFC's
        // 20-byte result is the prefix of the same PBKDF2 stream.
        let mut derive = PmkDerivation::new(b"password", b"salt").unwrap();
        for step in 1..=8192 {
            let mut calls = 0;
            let result = derive.step_with(|password, first, second| {
                calls += 1;
                hmac_sha1(password, first, second)
            });
            assert_eq!(calls, 1);
            if step < 8192 {
                assert!(result.is_none());
            } else {
                let result = result.unwrap();
                assert_eq!(
                    &result.0[..20],
                    &[
                        0x4b, 0x00, 0x79, 0x01, 0xb7, 0x65, 0x48, 0x9a, 0xbe, 0xad, 0x49, 0xd9,
                        0x26, 0xf7, 0x21, 0xd0, 0x65, 0xa4, 0x29, 0xc1
                    ]
                );
                let mut previous = [0; 32];
                ::pbkdf2::pbkdf2_hmac::<Sha1>(b"password", b"salt", 4096, &mut previous);
                assert_eq!(result.0, previous);
            }
        }
        assert!(derive.state.is_none());
        assert!(
            derive
                .step_with(|_, _, _| panic!("finished work invoked HMAC"))
                .is_none()
        );
    }

    #[test]
    fn cancellation_erases_password_and_all_partial_blocks() {
        fn erased_on_drop<T: ZeroizeOnDrop>() {}
        erased_on_drop::<PmkDerivation>();
        erased_on_drop::<State>();
        for steps in [0, 1, 4095, 4096, 4097, 8191] {
            let mut derive = PmkDerivation::new(&[0xa5; 63], &[0x55; 32]).unwrap();
            for _ in 0..steps {
                assert!(derive.step().is_none());
            }
            // Exercise the exact State::zeroize used by Drop, before its
            // storage is released; no access after drop is needed.
            let state = derive.state.as_mut().unwrap();
            state.zeroize();
            assert!(state.password.is_empty());
            assert!(state.ssid.is_empty());
            assert_eq!(state.previous, [0; 20]);
            assert_eq!(state.sum, [0; 20]);
            assert_eq!(state.output, [0; 32]);
            assert_eq!(
                (state.block, state.round.0.start, state.round.0.end),
                (0, 0, 0)
            );
            derive.zeroize();
            assert!(derive.state.is_none());
        }
    }

    #[test]
    fn invalid_inputs_are_rejected_before_retaining_secrets() {
        for length in [0, 7, 64] {
            assert!(matches!(
                PmkDerivation::new(&[0; 64][..length], b"ssid"),
                Err(PskDerivationError::InvalidPassphraseLength)
            ));
        }
        for length in [0, 33] {
            assert!(matches!(
                PmkDerivation::new(b"password", &[0; 33][..length]),
                Err(PskDerivationError::InvalidSsidLength)
            ));
        }
    }
}
