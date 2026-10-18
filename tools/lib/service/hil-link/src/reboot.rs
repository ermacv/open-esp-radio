//! Explicit, one-shot expected reboot for terminal hardware-fault scenarios.
//! Ordinary captures continue to reject every boot change.
use super::*;

pub(super) struct ExpectedReboot {
    old: u64,
    started: Instant,
    earliest: Instant,
    deadline: Instant,
}
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct RebootObservation {
    pub old_boot: u64,
    pub new_boot: u64,
    pub elapsed_millis: u64,
}
impl ProtocolState {
    pub(super) fn accept_expected_reboot(
        &mut self,
        message: &super::Received,
        now: Instant,
    ) -> bool {
        let Some(expected) = self.expected_reboot.as_ref() else {
            return false;
        };
        if self.failure.is_some()
            || self.health.failure.is_some()
            || self.health.boot_id != Some(expected.old)
            || expected.old == message.boot_id
            || message.boot_id == 0
            || message.message_sequence != 0
            || !message.is::<oer_hil_protocol::base::Hello>()
            || now < expected.earliest
            || now > expected.deadline
        {
            return false;
        }
        let expected = self.expected_reboot.take().unwrap();
        self.observed_reboot = Some(RebootObservation {
            old_boot: expected.old,
            new_boot: message.boot_id,
            elapsed_millis: now.duration_since(expected.started).as_millis() as u64,
        });
        true
    }
}
impl SerialCapture {
    /// Accept exactly one fresh Hello in the declared window. This authorizes
    /// an observation only, sends no reset and never clears an earlier failure.
    pub fn expect_reboot(&self, earliest: Duration, latest: Duration) -> Result<()> {
        self.check_link()?;
        let mut state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if earliest >= latest || state.expected_reboot.is_some() || state.observed_reboot.is_some()
        {
            return Err("invalid or repeated expected reboot".into());
        }
        let old = state
            .health
            .boot_id
            .ok_or("expected reboot requires live boot identity")?;
        let started = Instant::now();
        state.expected_reboot = Some(ExpectedReboot {
            old,
            started,
            earliest: started + earliest,
            deadline: started + latest,
        });
        Ok(())
    }
    /// Accept the end of the target's stream as its departure from USB, which
    /// the next request asks for: the capture then closes without a failure.
    pub fn expect_detach(&self) -> Result<()> {
        self.check_link()?;
        self.protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .expected_detach = true;
        Ok(())
    }

    pub fn wait_expected_reboot(&self) -> Result<RebootObservation> {
        let mut state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            oer_process::check_cancelled()?;
            state.check()?;
            if let Some(observed) = state.observed_reboot {
                return Ok(observed);
            }
            let deadline = state
                .expected_reboot
                .as_ref()
                .ok_or("reboot not armed")?
                .deadline;
            if Instant::now() >= deadline {
                state.expected_reboot = None;
                return Err("expected reboot deadline exceeded".into());
            }
            (state, _) = self
                .protocol
                .changed
                .wait_timeout(state, deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::super::capture::test_support;
    use super::*;
    #[test]
    fn reboot_expectation_is_timed_single_use_and_cannot_erase_failure() {
        let now = Instant::now();
        let make = || {
            let mut state = ProtocolState::default();
            state.health.observe(
                &test_support::received(test_support::hello(1, 0)),
                DecodeCounters::default(),
            );
            state.expected_reboot = Some(ExpectedReboot {
                old: 1,
                started: now,
                earliest: now + Duration::from_secs(1),
                deadline: now + Duration::from_secs(2),
            });
            state
        };
        let hello = test_support::received(test_support::hello(2, 0));
        assert!(!make().accept_expected_reboot(&hello, now));
        assert!(!make().accept_expected_reboot(&hello, now + Duration::from_secs(3)));
        let mut state = make();
        state.fail(LinkError::protocol("earlier failure"));
        assert!(!state.accept_expected_reboot(&hello, now + Duration::from_secs(1)));
        let mut state = make();
        state.health.fail("wire failure".into());
        assert!(!state.accept_expected_reboot(&hello, now + Duration::from_secs(1)));
        let mut state = make();
        assert!(state.accept_expected_reboot(&hello, now + Duration::from_secs(1)));
        assert!(!state.accept_expected_reboot(&hello, now + Duration::from_secs(1)));
        assert!(state.observed_reboot.is_some());
    }
}
