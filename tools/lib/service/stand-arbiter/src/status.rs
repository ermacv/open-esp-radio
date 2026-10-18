//! A snapshot of the stand for `cargo stand queue`.

use std::{fmt::Write as _, time::Duration};

use serde::Serialize;

use oer_stand_journal::{BoardEvent, device_label, latest};

use crate::{
    Arbiter, LeaseRecord, balance, estimate::format_duration, grant::signed_duration, queue,
};

const RECENT_LEASES: usize = 5;

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    /// Leases held now; leases on disjoint resources run in parallel.
    pub holders: Vec<HolderStatus>,
    /// In service order: by owner balance, then arrival.
    pub queue: Vec<QueuedStatus>,
    /// Every recently active owner's balance, highest first.
    pub balances: Vec<BalanceStatus>,
    /// Registered, attached or journaled boards.
    pub devices: Vec<DeviceStatus>,
    /// The newest event of every startup-artifact host file.
    pub startup_artifacts: Vec<BoardEvent>,
    pub recent: Vec<LeaseRecord>,
    /// Boards out of service.
    #[serde(default)]
    pub maintenance: Vec<crate::Maintenance>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeviceStatus {
    /// `None` for journal records that predate device identities.
    pub mac: Option<String>,
    pub label: String,
    /// The port it is attached at now.
    pub port: Option<String>,
    pub firmware: Option<BoardEvent>,
    /// `None` for journal records without a device identity.
    pub health: Option<crate::health::Health>,
}

#[derive(Clone, Debug, Serialize)]
pub struct HolderStatus {
    pub id: u64,
    pub owner: String,
    pub work: String,
    pub pid: u32,
    pub elapsed_secs: u64,
    /// Expected duration from earlier leases; never a limit.
    pub estimate_secs: u64,
    pub balance_ms: i64,
    pub claims: String,
    /// The `cargo hil` job the lease belongs to.
    pub job: Option<String>,
    /// The HIL run the lease executes, when a runner requested it.
    pub run: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BalanceStatus {
    pub owner: String,
    pub balance_ms: i64,
    pub holding: bool,
    pub waiting: bool,
    pub last_active_unix_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct QueuedStatus {
    pub id: u64,
    pub owner: String,
    pub work: String,
    pub pid: u32,
    pub waiting_secs: u64,
    pub estimate_secs: u64,
    pub balance_ms: i64,
    /// Conflicting waiting requests served before this one.
    pub ahead: usize,
    pub expected_start_secs: u64,
    pub claims: String,
    /// The `cargo hil` job the request belongs to.
    pub job: Option<String>,
}

impl Arbiter {
    /// Holders whose claims conflict with `claims`.
    pub fn conflicting_holders(
        &self,
        claims: &[oer_stand_claims::Claim],
    ) -> crate::Result<Vec<HolderStatus>> {
        let claims = oer_stand_claims::normalize(claims);
        let conflicting = self.transaction(|state| {
            Ok(state
                .holders
                .iter()
                .filter(|holder| oer_stand_claims::conflict(&holder.ticket.claims, &claims))
                .map(|holder| holder.ticket.id)
                .collect::<Vec<_>>())
        })?;
        Ok(self
            .status()?
            .holders
            .into_iter()
            .filter(|holder| conflicting.contains(&holder.id))
            .collect())
    }

    pub fn status(&self) -> crate::Result<Status> {
        let now = oer_durable::unix_seconds();
        let state = self.transaction(|state| Ok(state.clone()))?;
        let mut starts = queue::expected_starts(&state, now, crate::grant::SHUTDOWN_GRACE);
        let order = |id: u64| state.queue.iter().find(|ticket| ticket.id == id);
        starts.sort_by(|(a, ..), (b, ..)| match (order(*a), order(*b)) {
            (Some(a), Some(b)) if balance::before(&state, a, b) => std::cmp::Ordering::Less,
            _ => std::cmp::Ordering::Greater,
        });
        let queue = starts
            .iter()
            .filter_map(|(id, ahead, start)| {
                let ticket = state.queue.iter().find(|ticket| ticket.id == *id)?;
                Some(QueuedStatus {
                    id: ticket.id,
                    owner: ticket.owner.clone(),
                    work: ticket.work.clone(),
                    pid: ticket.process.pid,
                    waiting_secs: now.saturating_sub(ticket.enqueued_unix),
                    estimate_secs: ticket.estimate_secs,
                    balance_ms: balance::of(&state.balances, &ticket.owner),
                    ahead: *ahead,
                    expected_start_secs: *start,
                    claims: crate::grant::describe_claims(&ticket.claims),
                    job: ticket.job.clone(),
                })
            })
            .collect();
        let events = self.journal().events()?;
        let (flashes, startup_artifacts) = latest(&events);
        let stand = self.stand()?;
        let attached = oer_devices::discovery::attached();
        let mut macs: Vec<Option<String>> = stand
            .board
            .iter()
            .filter_map(|board| board.mac().ok().map(|mac| Some(mac.to_string())))
            .chain(
                attached
                    .iter()
                    .filter_map(|port| port.mac.as_ref().map(|mac| Some(mac.to_string()))),
            )
            .chain(flashes.iter().map(|flash| flash.device.clone()))
            .collect();
        let mut seen = Vec::new();
        macs.retain(|mac| {
            let new = !seen.contains(mac);
            seen.push(mac.clone());
            new
        });
        let maintenance = self.maintenance()?;
        let devices = macs
            .into_iter()
            .map(|mac| {
                let port = attached
                    .iter()
                    .find(|port| {
                        port.mac.is_some() && port.mac.as_ref().map(ToString::to_string) == mac
                    })
                    .map(|port| port.port.clone());
                DeviceStatus {
                    label: device_label(mac.as_deref(), Some(&stand)),
                    health: mac.as_deref().map(|mac| {
                        crate::health::of(mac, port.is_some(), &maintenance, &events, now)
                    }),
                    port,
                    firmware: flashes
                        .iter()
                        .find(|flash| flash.device == mac)
                        .map(|flash| (*flash).clone()),
                    mac,
                }
            })
            .collect();
        let history = self.history()?;
        let mut balances = state
            .balances
            .iter()
            .map(|(owner, balance)| BalanceStatus {
                owner: owner.clone(),
                balance_ms: balance.balance_ms,
                holding: state.holders.iter().any(|h| &h.ticket.owner == owner),
                waiting: state.queue.iter().any(|t| &t.owner == owner),
                last_active_unix_ms: balance.last_active_unix_ms,
            })
            .collect::<Vec<_>>();
        balances.sort_by_key(|balance| std::cmp::Reverse(balance.balance_ms));
        Ok(Status {
            maintenance,
            balances,
            holders: state
                .holders
                .into_iter()
                .map(|holder| HolderStatus {
                    id: holder.ticket.id,
                    work: holder.ticket.work,
                    pid: holder.ticket.process.pid,
                    elapsed_secs: now.saturating_sub(holder.granted_unix),
                    estimate_secs: holder.ticket.estimate_secs,
                    balance_ms: balance::of(&state.balances, &holder.ticket.owner),
                    owner: holder.ticket.owner,
                    claims: crate::grant::describe_claims(&holder.ticket.claims),
                    job: holder.ticket.job,
                    run: holder.ticket.run,
                })
                .collect(),
            queue,
            devices,
            startup_artifacts: startup_artifacts.into_iter().cloned().collect(),
            recent: history.iter().rev().take(RECENT_LEASES).cloned().collect(),
        })
    }
}

fn duration(seconds: u64) -> String {
    format_duration(Duration::from_secs(seconds))
}

/// A holder's estimate, and how far it has run past it.
fn estimate(elapsed_secs: u64, estimate_secs: u64) -> String {
    match elapsed_secs.checked_sub(estimate_secs) {
        Some(overdue) if overdue > 0 => format!(
            "estimated {}, overdue by {}",
            duration(estimate_secs),
            duration(overdue)
        ),
        _ => format!("estimated {}", duration(estimate_secs)),
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut text = String::new();
        if self.holders.is_empty() {
            writeln!(text, "held:    free")?;
        }
        for holder in &self.holders {
            writeln!(
                text,
                "held:    #{} {} `{}` on {} pid {}, {} ({}), balance {}",
                holder.id,
                holder.owner,
                holder.work,
                holder.claims,
                holder.pid,
                duration(holder.elapsed_secs),
                estimate(holder.elapsed_secs, holder.estimate_secs),
                signed_duration(holder.balance_ms)
            )?;
        }
        if self.queue.is_empty() {
            writeln!(text, "queue:   empty")?;
        }
        for (position, entry) in self.queue.iter().enumerate() {
            writeln!(
                text,
                "queue {}: #{} {} `{}` on {} pid {}, balance {}, behind {}, waiting {}, \
                 estimated {}, start in ~{}",
                position + 1,
                entry.id,
                entry.owner,
                entry.work,
                entry.claims,
                entry.pid,
                signed_duration(entry.balance_ms),
                entry.ahead,
                duration(entry.waiting_secs),
                duration(entry.estimate_secs),
                duration(entry.expected_start_secs)
            )?;
        }
        if let Some(next) = self.queue.first() {
            writeln!(
                text,
                "next:    {} (balance {}) is served first among conflicting requests; the \
                 highest balance goes first",
                next.owner,
                signed_duration(next.balance_ms)
            )?;
        }
        if !self.balances.is_empty() {
            writeln!(
                text,
                "balances (held time is charged, blocked waiting is credited, halving every 2h):"
            )?;
        }
        for balance in &self.balances {
            writeln!(
                text,
                "  {:>8} {}{}",
                signed_duration(balance.balance_ms),
                balance.owner,
                match (balance.holding, balance.waiting) {
                    (true, true) => " (holding, waiting)",
                    (true, false) => " (holding)",
                    (false, true) => " (waiting)",
                    (false, false) => "",
                }
            )?;
        }
        let describe = |event: &Option<BoardEvent>| {
            event
                .as_ref()
                .map_or_else(|| String::from("unknown"), ToString::to_string)
        };
        if self.devices.is_empty() {
            writeln!(text, "boards:  none known")?;
        }
        for device in &self.devices {
            writeln!(
                text,
                "board {} [{}]{}: {}",
                device.label,
                device.port.as_deref().unwrap_or("not attached"),
                device
                    .health
                    .as_ref()
                    .map(|health| format!(" (health: {health})"))
                    .unwrap_or_default(),
                describe(&device.firmware)
            )?;
        }
        for board in &self.maintenance {
            writeln!(
                text,
                "MAINTENANCE {} by {}: {}",
                board.mac, board.owner, board.reason
            )?;
        }
        for artifact in &self.startup_artifacts {
            writeln!(text, "startup artifact (host file): {artifact}")?;
        }
        if !self.recent.is_empty() {
            writeln!(text, "recent leases:")?;
        }
        for record in &self.recent {
            writeln!(
                text,
                "  #{} {} `{}` {} {:?}, balance after {}",
                record.id,
                record.owner,
                record.work,
                duration(record.duration_secs()),
                record.outcome,
                signed_duration(record.balance_after_ms)
            )?;
        }
        f.write_str(text.trim_end())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_holder_past_its_estimate_is_shown_overdue() {
        assert_eq!(estimate(60, 300), format!("estimated {}", duration(300)));
        assert_eq!(estimate(300, 300), format!("estimated {}", duration(300)));
        assert_eq!(
            estimate(420, 300),
            format!("estimated {}, overdue by {}", duration(300), duration(120))
        );
    }
}
