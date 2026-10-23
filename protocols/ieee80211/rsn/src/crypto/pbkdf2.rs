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
    password: [u8; PSK_PASSPHRASE_MAX_LEN],
    password_len: usize,
    ssid: [u8; PSK_SSID_MAX_LEN],
    ssid_len: usize,
    previous: [u8; 20],
    sum: [u8; 20],
    output: [u8; 32],
    block: u32,
    round: u32,
}

impl PmkDerivation {
    #[expect(
        clippy::indexing_slicing,
        reason = "input lengths are validated against the fixed backing arrays"
    )]
    pub fn new(password: &[u8], ssid: &[u8]) -> Result<Self, PskDerivationError> {
        if !(PSK_PASSPHRASE_MIN_LEN..=PSK_PASSPHRASE_MAX_LEN).contains(&password.len()) {
            return Err(PskDerivationError::InvalidPassphraseLength);
        }
        if ssid.is_empty() || ssid.len() > PSK_SSID_MAX_LEN {
            return Err(PskDerivationError::InvalidSsidLength);
        }
        let mut state = State {
            password: [0; PSK_PASSPHRASE_MAX_LEN],
            password_len: password.len(),
            ssid: [0; PSK_SSID_MAX_LEN],
            ssid_len: ssid.len(),
            previous: [0; 20],
            sum: [0; 20],
            output: [0; 32],
            block: 1,
            round: 0,
        };
        state.password[..password.len()].copy_from_slice(password);
        state.ssid[..ssid.len()].copy_from_slice(ssid);
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
    #[expect(
        clippy::indexing_slicing,
        reason = "private lengths are validated on construction; PBKDF2 has exactly two fixed output blocks"
    )]
    fn advance(&mut self, hmac: impl FnOnce(&[u8], &[u8], &[u8]) -> [u8; 20]) -> Option<Pmk> {
        // FnOnce makes the one-HMAC step budget structural, also when a
        // block finishes. No second block is started in the same step.
        self.previous = if self.round == 0 {
            hmac(
                &self.password[..self.password_len],
                &self.ssid[..self.ssid_len],
                &self.block.to_be_bytes(),
            )
        } else {
            hmac(&self.password[..self.password_len], &self.previous, &[])
        };
        for (sum, byte) in self.sum.iter_mut().zip(self.previous) {
            *sum ^= byte;
        }
        self.round += 1;
        if self.round != PSK_PBKDF2_ITERATIONS {
            return None;
        }
        if self.block == 1 {
            self.output[..20].copy_from_slice(&self.sum);
            self.block = 2;
            self.round = 0;
            self.sum.zeroize();
            self.previous.zeroize();
            None
        } else {
            self.output[20..].copy_from_slice(&self.sum[..12]);
            Some(Pmk(self.output))
        }
    }
}

#[expect(clippy::expect_used, reason = "HMAC accepts every passphrase length")]
fn hmac_sha1(password: &[u8], first: &[u8], second: &[u8]) -> [u8; 20] {
    let mut mac = Hmac::<Sha1>::new_from_slice(password).expect("HMAC accepts the password");
    mac.update(first);
    mac.update(second);
    let mut digest = mac.finalize().into_bytes();
    let mut result = [0; 20];
    result.copy_from_slice(&digest);
    digest.zeroize();
    result
}

#[cfg(test)]
mod tests {
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
            assert_eq!(state.password, [0; 63]);
            assert_eq!(state.ssid, [0; 32]);
            assert_eq!(state.previous, [0; 20]);
            assert_eq!(state.sum, [0; 20]);
            assert_eq!(state.output, [0; 32]);
            assert_eq!(
                (state.password_len, state.ssid_len, state.block, state.round),
                (0, 0, 0, 0)
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
