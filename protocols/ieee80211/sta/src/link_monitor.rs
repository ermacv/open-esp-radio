//! Executor- and chip-independent connected-station beacon-loss policy.
//!
//! This owner performs no MMIO and does not put the modem to sleep. RX supplies
//! typed beacon observations, while the runtime supplies monotonic time and
//! owns the resulting disconnect edge. Power-save policy can consume the same
//! TIM observation without importing vendor PM contexts or RTOS timers.

use oer_ieee80211_mac::station_beacon::{StaBeaconObservation, StaTimObservation};
use oer_time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaBeaconLossConfigError {
    ZeroInterval,
    ZeroMissLimit,
    ZeroTimeout,
    DeadlineOverflow,
}

/// Association-derived beacon-loss policy.
///
/// The connected station declares the link unreachable, and starts probing
/// the access point, when no beacon has arrived for `timeout`, as the
/// vendor's station beacon timeout does. `miss_limit` is the separate
/// consecutive-miss count the MAC's hardware beacon monitor uses while the
/// modem sleeps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaBeaconLossConfig {
    interval_tu: u16,
    miss_limit: u8,
    window: Duration,
}

impl StaBeaconLossConfig {
    pub const fn new(
        interval_tu: u16,
        miss_limit: u8,
        timeout: Duration,
    ) -> Result<Self, StaBeaconLossConfigError> {
        if interval_tu == 0 {
            return Err(StaBeaconLossConfigError::ZeroInterval);
        }
        if miss_limit == 0 {
            return Err(StaBeaconLossConfigError::ZeroMissLimit);
        }
        if timeout.as_micros() == 0 {
            return Err(StaBeaconLossConfigError::ZeroTimeout);
        }
        Ok(Self {
            interval_tu,
            miss_limit,
            window: timeout,
        })
    }

    pub const fn interval_tu(self) -> u16 {
        self.interval_tu
    }

    pub const fn miss_limit(self) -> u8 {
        self.miss_limit
    }

    /// How long the link may go without a beacon before it is probed.
    pub const fn window(self) -> Duration {
        self.window
    }
}

/// Finite beacon/TIM state owned by the connected executor task.
pub struct StaBeaconMonitor {
    config: StaBeaconLossConfig,
    deadline: Option<Instant>,
    last_observation: Option<StaBeaconObservation>,
    observed: u32,
}

impl StaBeaconMonitor {
    pub const fn new(config: StaBeaconLossConfig) -> Self {
        Self {
            config,
            deadline: None,
            last_observation: None,
            observed: 0,
        }
    }

    pub const fn config(&self) -> StaBeaconLossConfig {
        self.config
    }

    pub const fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub const fn last_observation(&self) -> Option<StaBeaconObservation> {
        self.last_observation
    }

    pub const fn last_tim(&self) -> Option<StaTimObservation> {
        match self.last_observation {
            Some(observation) => observation.tim,
            None => None,
        }
    }

    pub const fn observed(&self) -> u32 {
        self.observed
    }

    /// Arm from the association-complete edge before the first beacon arrives.
    pub fn arm(&mut self, now: Instant) -> Result<(), StaBeaconLossConfigError> {
        if self.deadline.is_none() {
            self.deadline = Some(
                now.checked_add(self.config.window)
                    .ok_or(StaBeaconLossConfigError::DeadlineOverflow)?,
            );
        }
        Ok(())
    }

    /// Refresh the absolute deadline from a beacon already authenticated by
    /// BSSID/address classification. The association's interval remains the
    /// policy source; an unprotected beacon cannot silently stretch it.
    pub fn observe(
        &mut self,
        now: Instant,
        observation: StaBeaconObservation,
    ) -> Result<(), StaBeaconLossConfigError> {
        self.deadline = Some(
            now.checked_add(self.config.window)
                .ok_or(StaBeaconLossConfigError::DeadlineOverflow)?,
        );
        self.last_observation = Some(observation);
        self.observed = self.observed.saturating_add(1);
        Ok(())
    }

    /// Refresh link reachability from a response already validated as coming
    /// from the associated BSSID.
    ///
    /// This deliberately does not fabricate a beacon observation or TIM:
    /// active-probe reachability and passive beacon state remain distinct.
    pub fn observe_reachability(&mut self, now: Instant) -> Result<(), StaBeaconLossConfigError> {
        self.deadline = Some(
            now.checked_add(self.config.window)
                .ok_or(StaBeaconLossConfigError::DeadlineOverflow)?,
        );
        Ok(())
    }

    /// Replace the passive beacon deadline with one bounded active-probe
    /// response deadline selected by the chip control policy.
    pub fn wait_for_reachability(
        &mut self,
        now: Instant,
        timeout: Duration,
    ) -> Result<(), StaBeaconLossConfigError> {
        self.deadline = Some(
            now.checked_add(timeout)
                .ok_or(StaBeaconLossConfigError::DeadlineOverflow)?,
        );
        Ok(())
    }

    pub const fn expired(&self, now: Instant) -> bool {
        matches!(self.deadline, Some(deadline) if now.as_micros() >= deadline.as_micros())
    }
}

/// How a station probes the access point once its beacons stop: one probe
/// every `interval`, at most `attempts` probes before the link is lost; the
/// first `directed` probes are addressed to the access point and the rest
/// are broadcast.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaLinkProbePolicy {
    pub interval: Duration,
    pub attempts: u8,
    pub directed: u8,
}

/// What the link needs from its station now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaLinkAction {
    /// Send one probe, then report it with [`StaLinkMonitor::probe_sent`]:
    /// a Probe Request addressed to the access point when `directed`, a
    /// broadcast one otherwise.
    Probe { directed: bool },
    /// Neither beacons nor probe responses came back: the link is lost.
    Lost,
}

/// The connected station's link supervision: beacons keep the link, and
/// when they stop for the beacon window the station probes the access point
/// under a [`StaLinkProbePolicy`] before it declares the link lost.
///
/// Time enters as a value; the owner sends the probe and ends the link.
pub struct StaLinkMonitor {
    beacons: StaBeaconMonitor,
    probe: StaLinkProbePolicy,
    probes_sent: u8,
}

impl StaLinkMonitor {
    pub const fn new(loss: StaBeaconLossConfig, probe: StaLinkProbePolicy) -> Self {
        Self {
            beacons: StaBeaconMonitor::new(loss),
            probe,
            probes_sent: 0,
        }
    }

    /// The beacon state the link keeps: the last observation and its TIM.
    pub const fn beacons(&self) -> &StaBeaconMonitor {
        &self.beacons
    }

    pub const fn probe_policy(&self) -> StaLinkProbePolicy {
        self.probe
    }

    /// Probes sent since the last beacon or probe response.
    pub const fn probes_sent(&self) -> u8 {
        self.probes_sent
    }

    /// When [`Self::due`] next has something to report.
    pub const fn deadline(&self) -> Option<Instant> {
        self.beacons.deadline()
    }

    /// Start the beacon window from the association-complete edge, before
    /// the first beacon arrives; a running window is kept.
    pub fn arm(&mut self, now: Instant) -> Result<(), StaBeaconLossConfigError> {
        self.beacons.arm(now)
    }

    /// A beacon of the associated access point: the link is alive.
    pub fn observe_beacon(
        &mut self,
        now: Instant,
        observation: StaBeaconObservation,
    ) -> Result<(), StaBeaconLossConfigError> {
        self.probes_sent = 0;
        self.beacons.observe(now, observation)
    }

    /// A probe response from the associated access point: the link is
    /// reachable, without a beacon observation.
    pub fn observe_probe_response(&mut self, now: Instant) -> Result<(), StaBeaconLossConfigError> {
        self.probes_sent = 0;
        self.beacons.observe_reachability(now)
    }

    /// What the link needs at `now`: nothing while beacons or the last
    /// probe's response window keep it, a probe while attempts remain, and
    /// [`StaLinkAction::Lost`] after the last one went unanswered.
    pub const fn due(&self, now: Instant) -> Option<StaLinkAction> {
        if !self.beacons.expired(now) {
            None
        } else if self.probes_sent < self.probe.attempts {
            Some(StaLinkAction::Probe {
                directed: self.probes_sent < self.probe.directed,
            })
        } else {
            Some(StaLinkAction::Lost)
        }
    }

    /// The probe [`Self::due`] asked for left at `now`: its response window
    /// starts.
    pub fn probe_sent(&mut self, now: Instant) -> Result<(), StaBeaconLossConfigError> {
        self.beacons
            .wait_for_reachability(now, self.probe.interval)?;
        self.probes_sent = self.probes_sent.saturating_add(1);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
