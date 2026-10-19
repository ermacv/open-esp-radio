//! Host socket admission measured independently of DUT delivery.

use crate::paced_udp::HostTransmission;
use serde::Serialize;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    NotAssessed,
    Met,
    UnderOffered,
    Unavailable,
}

#[derive(Serialize)]
pub struct Assessment {
    pub requested_bps: u64,
    /// Bytes accepted by host send calls divided by the requested workload window.
    pub accepted_bps_over_window: Option<u64>,
    /// Includes any time spent blocked in the final send, excludes terminal markers.
    pub accepted_bps_over_sender_elapsed: Option<u64>,
    pub minimum_percent: Option<u8>,
    pub status: Status,
}

impl Assessment {
    pub fn new(
        requested_bps: u64,
        duration: Duration,
        sent: Option<HostTransmission>,
        minimum_percent: Option<u8>,
    ) -> Self {
        let accepted_bps_over_window = sent.map(|sent| {
            u64::try_from(u128::from(sent.bytes) * 8_000_000_000 / duration.as_nanos().max(1))
                .unwrap_or(u64::MAX)
        });
        let accepted_bps_over_sender_elapsed = sent.map(HostTransmission::throughput_bps);
        let status = match (
            minimum_percent,
            accepted_bps_over_window,
            accepted_bps_over_sender_elapsed,
        ) {
            (_, None, _) => Status::Unavailable,
            (None, _, _) => Status::NotAssessed,
            (Some(minimum), Some(window), Some(elapsed)) => {
                if u128::from(window.min(elapsed)) * 100
                    >= u128::from(requested_bps) * u128::from(minimum)
                {
                    Status::Met
                } else {
                    Status::UnderOffered
                }
            }
            _ => Status::Unavailable,
        };
        Self {
            requested_bps,
            accepted_bps_over_window,
            accepted_bps_over_sender_elapsed,
            minimum_percent,
            status,
        }
    }

    pub fn validate(&self, flow: usize) -> crate::Result<()> {
        if self.minimum_percent.is_some()
            && matches!(self.status, Status::UnderOffered | Status::Unavailable)
        {
            return Err(format!(
                "AP UDP flow {flow} offered load not met: requested={} bps, host accepted over window={:?} bps, over sender elapsed={:?} bps, required={:?}%; this is not a DUT delivery verdict",
                self.requested_bps, self.accepted_bps_over_window, self.accepted_bps_over_sender_elapsed, self.minimum_percent,
            ).into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
