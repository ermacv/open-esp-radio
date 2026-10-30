//! The station's SA Query procedure (IEEE 802.11-2020 11.13).
//!
//! Under management frame protection an unprotected Deauthentication or
//! Disassociation for a class 2 or 3 frame may be forged. The station then
//! asks its access point with protected SA Query Requests whether the
//! association still holds: a matching protected Response ends the procedure
//! and keeps the association, and no Response within the timeout ends the
//! association.
//!
//! The vendor arms a one-shot 1024 ms timeout and a periodic 200 ms retry
//! timer when the procedure starts, draws the first transaction identifier
//! from its random source modulo 65525 and increments it with every further
//! Request. A Response matches any Request sent in the current procedure. A
//! retry after ten Requests also ends the association; with these timers the
//! timeout comes first.
//!
//! SOURCE(esp32s31): complete pinned `libnet80211.a[ieee80211_sta.o]::
//! sta_try_sa_query_process`, `sta_sa_query_process_timeout`,
//! `sta_try_sa_query`, `sta_sa_query_timeout` and
//! `libnet80211.a[ieee80211.o]::ieee80211_recv_sa_query_resp`.

use oer_time::{Duration, Instant};

/// Period of the vendor's retry timer.
const RETRY_INTERVAL: Duration = Duration::from_millis(200);
/// The vendor's one-shot procedure timeout.
const TIMEOUT: Duration = Duration::from_millis(1_024);
/// Requests after which the next retry ends the association.
const REQUEST_LIMIT: u16 = 10;
/// Modulus the vendor reduces its random first transaction identifier by.
const TRANSACTION_MODULUS: u32 = 65_525;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ActiveSaQuery {
    /// Transaction identifier of the latest Request.
    transaction: u16,
    /// Requests sent in this procedure.
    requests: u16,
    /// No further retry is due before the already checked final timeout.
    next_retry: Option<Instant>,
    timeout: Instant,
}

/// What the procedure asks of its owner at one instant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaQueryStep {
    Idle,
    /// Send one SA Query Request with this transaction identifier.
    Request([u8; 2]),
    /// The access point did not confirm the association.
    TimedOut,
}

/// The procedure cannot schedule its complete response window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SaQueryDeadlineOverflow;

/// The SA Query procedure of one association.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StationSaQuery {
    active: Option<ActiveSaQuery>,
}

impl StationSaQuery {
    pub const fn new() -> Self {
        Self { active: None }
    }

    /// Whether a procedure runs.
    pub const fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Start the procedure for one unprotected disconnect and return the
    /// first Request's transaction identifier. A disconnect during a running
    /// procedure is ignored, as the vendor ignores it.
    /// Overflow leaves the procedure unchanged and requires its caller to
    /// terminate the association: PMF cannot leave this check outstanding.
    pub fn start(
        &mut self,
        now: Instant,
        random: u32,
    ) -> Result<Option<[u8; 2]>, SaQueryDeadlineOverflow> {
        if self.active.is_some() {
            return Ok(None);
        }
        let timeout = now.checked_add(TIMEOUT).ok_or(SaQueryDeadlineOverflow)?;
        let next_retry = now
            .checked_add(RETRY_INTERVAL)
            .ok_or(SaQueryDeadlineOverflow)?;
        let transaction = (random % TRANSACTION_MODULUS) as u16;
        self.active = Some(ActiveSaQuery {
            transaction,
            requests: 1,
            next_retry: Some(next_retry),
            timeout,
        });
        Ok(Some(transaction.to_le_bytes()))
    }

    /// Match one protected SA Query Response. A Response to a Request of the
    /// running procedure ends it.
    pub fn response(&mut self, transaction: [u8; 2]) -> bool {
        let Some(active) = self.active else {
            return false;
        };
        let received = i32::from(u16::from_le_bytes(transaction));
        let latest = i32::from(active.transaction);
        let first = latest.saturating_sub(i32::from(active.requests));
        if received <= first || received > latest {
            return false;
        }
        self.active = None;
        true
    }

    /// Whether the procedure's timeout passed. The timeout ends the
    /// association even while frames may not leave.
    pub fn timed_out(&self, now: Instant) -> bool {
        self.active.is_some_and(|active| now >= active.timeout)
    }

    /// When the procedure ends the association unless the peer answers.
    pub fn timeout(&self) -> Option<Instant> {
        self.active.map(|active| active.timeout)
    }

    /// Advance the procedure to `now`.
    pub fn step(&mut self, now: Instant) -> SaQueryStep {
        let Some(active) = self.active.as_mut() else {
            return SaQueryStep::Idle;
        };
        if now >= active.timeout {
            self.active = None;
            return SaQueryStep::TimedOut;
        }
        let Some(next_retry) = active.next_retry else {
            return SaQueryStep::Idle;
        };
        if now < next_retry {
            return SaQueryStep::Idle;
        }
        if active.requests == REQUEST_LIMIT {
            self.active = None;
            return SaQueryStep::TimedOut;
        }
        // A retry beyond the terminal timeout is not part of this procedure.
        // Its overflow cannot turn the checked terminal into an endless wait.
        active.next_retry = next_retry
            .checked_add(RETRY_INTERVAL)
            .filter(|retry| *retry < active.timeout);
        active.transaction = active.transaction.wrapping_add(1);
        active.requests = active.requests.saturating_add(1);
        SaQueryStep::Request(active.transaction.to_le_bytes())
    }

    /// The instant the procedure next needs its owner.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.active
            .map(|active| active.next_retry.unwrap_or(active.timeout))
    }
}

#[cfg(test)]
mod tests;
