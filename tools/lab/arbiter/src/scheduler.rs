//! The service's decisions over the queue: which waiting request is granted,
//! what the end of a lease leaves to recover, and what a restart finds.
//!
//! A request is granted when the queue serves it (no conflicting holder, no
//! conflicting request served before it) and every stateful resource it
//! holds is ready. A request that uses the air also needs every other board
//! of the stand quiet: a board the service has not confirmed in an end state
//! may still radiate. The scheduler runs no hardware and reads no clock: the
//! service passes the time, runs the recoveries it returns and reports each
//! driver's confirmation.

use std::{path::Path, time::Duration};

use crate::{
    balance,
    claims::{Claim, conflict, normalize},
    history::{GrantReason, LeaseOutcome, LeaseRecord, OwnerBalance},
    process::ProcessIdentity,
    readiness::{self, Confirmation, Readiness, Recovery},
    state::{Holder, Priority, STATE_SCHEMA, State, Ticket},
};

/// A request as the service queues it.
#[derive(Clone, Debug)]
pub struct Queued {
    pub owner: String,
    pub work: String,
    pub scenarios: Vec<String>,
    pub run: Option<String>,
    pub claims: Vec<Claim>,
    pub priority: Priority,
    /// The expected duration, for waiting-time estimates only.
    pub estimate_secs: u64,
    /// The client that asked, for the status report.
    pub client: ProcessIdentity,
}

/// Why a held lease must end or yield.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Due {
    /// It reached the hard limit every lease has.
    HardLimit,
    /// Divisible work yields at its next boundary to `waiter`.
    Yield { waiter: String, balance_ms: i64 },
}

/// A lease the scheduler just granted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Granted {
    pub lease: u64,
    pub token: String,
    /// The owner's balance at the grant, in milliseconds.
    pub balance_ms: i64,
    /// Each stateful resource the lease holds, with its generation.
    pub resources: Vec<(String, u64)>,
}

/// A lease that ended: its history record and the recoveries it leaves.
#[derive(Clone, Debug)]
pub struct Ended {
    pub record: LeaseRecord,
    pub recoveries: Vec<Recovery>,
}

/// What a restart found: the leases it ended and the recoveries to run.
#[derive(Clone, Debug, Default)]
pub struct Resumed {
    pub abandoned: Vec<LeaseRecord>,
    pub recoveries: Vec<Recovery>,
}

/// The queue state and the stand's stateful resources.
#[derive(Debug)]
pub struct Scheduler {
    state: State,
    /// The stand's stateful resources: its boards and the router.
    stand: Vec<String>,
}

impl Scheduler {
    /// Resume from the persisted `state` of a service that stopped, at
    /// `now_ms`. The waiting requests left with their clients; every lease
    /// still held ends as abandoned. Its resources, those whose recovery did
    /// not finish and those never confirmed are recovered before they are
    /// granted again.
    pub(crate) fn resume(mut state: State, stand: Vec<String>, now_ms: u64) -> (Self, Resumed) {
        state.queue.clear();
        let mut recoveries = Vec::new();
        for (resource, readiness) in &mut state.readiness {
            if let Readiness::Recovering { lease, generation } = readiness {
                let generation = *generation + 1;
                *readiness = Readiness::Recovering {
                    lease: *lease,
                    generation,
                };
                recoveries.push(Recovery {
                    resource: resource.clone(),
                    generation,
                });
            }
        }
        let mut scheduler = Self {
            state,
            stand: Vec::new(),
        };
        let leases = scheduler
            .state
            .holders
            .iter()
            .map(|holder| holder.ticket.id)
            .collect::<Vec<_>>();
        let mut abandoned = Vec::new();
        for lease in leases {
            if let Some(ended) = scheduler.end(lease, LeaseOutcome::Abandoned, now_ms) {
                abandoned.push(ended.record);
                recoveries.extend(ended.recoveries);
            }
        }
        recoveries.extend(scheduler.set_stand(stand));
        (
            scheduler,
            Resumed {
                abandoned,
                recoveries,
            },
        )
    }

    /// Resume from the state persisted at `path`, a new one when there is
    /// none (see [`Scheduler::resume`]).
    pub fn open(path: &Path, stand: Vec<String>, now_ms: u64) -> crate::Result<(Self, Resumed)> {
        let state = match std::fs::read(path) {
            Ok(bytes) => {
                let value: serde_json::Value = serde_json::from_slice(&bytes)?;
                let schema = value.get("schema").cloned().unwrap_or_default();
                if schema.as_u64() != Some(u64::from(STATE_SCHEMA)) {
                    return Err(crate::state::unreadable(path, &schema).into());
                }
                serde_json::from_value(value)?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self::resume(state, stand, now_ms))
    }

    /// Persist the state at `path`, atomically.
    pub fn save(&self, path: &Path) -> crate::Result<()> {
        oer_os_durable::atomic_write(path, &serde_json::to_vec_pretty(&self.state)?)?;
        Ok(())
    }

    /// Take the stand's stateful resources as they are now: one never seen
    /// is recovered before its first grant.
    pub fn set_stand(&mut self, stand: Vec<String>) -> Vec<Recovery> {
        let mut recoveries = Vec::new();
        for resource in &stand {
            if !self.state.readiness.contains_key(resource) {
                self.state.readiness.insert(
                    resource.clone(),
                    Readiness::Recovering {
                        lease: None,
                        generation: 1,
                    },
                );
                recoveries.push(Recovery {
                    resource: resource.clone(),
                    generation: 1,
                });
            }
        }
        self.stand = stand;
        recoveries
    }

    pub fn readiness(&self, resource: &str) -> Option<&Readiness> {
        self.state.readiness.get(resource)
    }

    pub(crate) fn state(&self) -> &State {
        &self.state
    }

    /// The boards, or the stand, owners took out of service.
    pub fn maintenance(&self) -> &[crate::Maintenance] {
        &self.state.maintenance
    }

    /// Take `entry.mac` (a board's MAC, or the stand) out of service for
    /// every owner but `entry.owner`, replacing an earlier entry for it.
    pub fn set_maintenance(&mut self, entry: crate::Maintenance) {
        self.state
            .maintenance
            .retain(|known| known.mac != entry.mac);
        self.state.maintenance.push(entry);
    }

    /// Return `mac` (or the stand) to service; false when it was in service.
    pub fn clear_maintenance(&mut self, mac: &str) -> bool {
        let before = self.state.maintenance.len();
        self.state.maintenance.retain(|entry| entry.mac != mac);
        self.state.maintenance.len() != before
    }

    /// Charge `old`'s balance to `new`, for an owner known under another
    /// name; the service renames the history.
    pub fn merge_owner(&mut self, old: &str, new: &str) {
        if let Some(balance) = self.state.balances.remove(old) {
            let merged = self.state.balances.entry(new.to_owned()).or_default();
            merged.balance_ms += balance.balance_ms;
            merged.last_active_unix_ms =
                merged.last_active_unix_ms.max(balance.last_active_unix_ms);
        }
    }

    /// Drop `name`'s balance; false when it had none.
    pub fn forget_owner(&mut self, name: &str) -> bool {
        self.state.balances.remove(name).is_some()
    }

    /// Every stateful resource's readiness.
    pub fn resources(&self) -> Vec<(String, Readiness)> {
        self.state
            .readiness
            .iter()
            .map(|(resource, readiness)| (resource.clone(), readiness.clone()))
            .collect()
    }

    /// The owner, work and claims of held lease `lease`.
    pub fn holder(&self, lease: u64) -> Option<(&str, &str, &[Claim])> {
        self.state
            .holders
            .iter()
            .find(|holder| holder.ticket.id == lease)
            .map(|holder| {
                (
                    holder.ticket.owner.as_str(),
                    holder.ticket.work.as_str(),
                    holder.ticket.claims.as_slice(),
                )
            })
    }

    /// Queue `request` at `now_unix` and return its ticket.
    pub fn enqueue(&mut self, request: Queued, now_unix: u64) -> u64 {
        let id = self.state.next_id;
        self.state.next_id += 1;
        self.state.queue.push(Ticket {
            id,
            owner: request.owner,
            work: request.work,
            estimate_secs: request.estimate_secs,
            process: request.client,
            enqueued_unix: now_unix,
            claims: normalize(&request.claims),
            priority: request.priority,
            run: request.run,
            scenarios: request.scenarios,
            unknown: Default::default(),
        });
        id
    }

    /// Take ticket `id` out of the queue; false when it is not waiting.
    pub fn leave(&mut self, id: u64) -> bool {
        let before = self.state.queue.len();
        self.state.queue.retain(|ticket| ticket.id != id);
        self.state.queue.len() != before
    }

    /// Grant, in service order, every waiting request that can be granted
    /// at `now_ms`; `token` names each new lease for the commands that join
    /// it.
    pub fn grant(&mut self, now_ms: u64, token: &mut dyn FnMut() -> String) -> Vec<Granted> {
        balance::settle(&mut self.state, now_ms);
        let mut order = self.state.queue.iter().collect::<Vec<_>>();
        order.sort_by(|a, b| {
            if balance::before(&self.state, a, b) {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            }
        });
        let order = order
            .into_iter()
            .map(|ticket| ticket.id)
            .collect::<Vec<_>>();
        let mut granted = Vec::new();
        for id in order {
            // A request maintenance keeps waiting holds back no other one.
            let mut view = self.state.clone();
            view.queue.retain(|ticket| {
                ticket.id == id
                    || crate::maintenance::refusal(&view.maintenance, &ticket.owner, &ticket.claims)
                        .is_none()
            });
            if !crate::queue::grantable(&view, id) || !self.ready(id) {
                continue;
            }
            let Some(index) = self.state.queue.iter().position(|ticket| ticket.id == id) else {
                continue;
            };
            let ticket = self.state.queue.remove(index);
            let balance_ms = balance::of(&self.state.balances, &ticket.owner);
            let over = self
                .state
                .queue
                .iter()
                .filter(|other| {
                    other.owner != ticket.owner && conflict(&other.claims, &ticket.claims)
                })
                .map(|other| OwnerBalance {
                    owner: other.owner.clone(),
                    balance_ms: balance::of(&self.state.balances, &other.owner),
                })
                .collect();
            let mut resources = Vec::new();
            for resource in readiness::held(&ticket.claims, &self.stand) {
                let generation = self
                    .state
                    .readiness
                    .get(resource)
                    .map_or(0, Readiness::generation)
                    + 1;
                self.state.readiness.insert(
                    resource.to_owned(),
                    Readiness::InUse {
                        lease: id,
                        generation,
                    },
                );
                resources.push((resource.to_owned(), generation));
            }
            let token = token();
            self.state.holders.push(Holder {
                ticket,
                token: token.clone(),
                granted_unix: now_ms / 1000,
                reason: Some(GrantReason { balance_ms, over }),
                preempted: None,
                unknown: Default::default(),
            });
            granted.push(Granted {
                lease: id,
                token,
                balance_ms,
                resources,
            });
        }
        granted
    }

    /// Whether every stateful resource ticket `id` holds is ready and, when
    /// it uses the air, every other board of the stand is quiet.
    fn ready(&self, id: u64) -> bool {
        let Some(ticket) = self.state.queue.iter().find(|ticket| ticket.id == id) else {
            return false;
        };
        if crate::maintenance::refusal(&self.state.maintenance, &ticket.owner, &ticket.claims)
            .is_some()
        {
            return false;
        }
        let held = readiness::held(&ticket.claims, &self.stand);
        let ready = held.iter().all(|resource| {
            matches!(
                self.state.readiness.get(*resource),
                Some(Readiness::Ready { .. })
            )
        });
        let quiet = !readiness::uses_air(&ticket.claims)
            || self
                .stand
                .iter()
                .filter(|resource| {
                    readiness::is_board(resource) && !held.contains(&resource.as_str())
                })
                .all(|resource| {
                    self.state
                        .readiness
                        .get(resource)
                        .is_some_and(Readiness::quiet)
                });
        ready && quiet
    }

    /// End lease `lease` at `now_ms` with `outcome`: its resources start
    /// their recovery, and none is granted again before its driver confirms
    /// an end state. `None` when no such lease is held.
    pub fn end(&mut self, lease: u64, outcome: LeaseOutcome, now_ms: u64) -> Option<Ended> {
        balance::settle(&mut self.state, now_ms);
        let index = self
            .state
            .holders
            .iter()
            .position(|holder| holder.ticket.id == lease)?;
        let holder = self.state.holders.remove(index);
        let mut recoveries = Vec::new();
        for (resource, readiness) in &mut self.state.readiness {
            if let Readiness::InUse {
                lease: holding,
                generation,
            } = readiness
                && *holding == lease
            {
                let generation = *generation + 1;
                *readiness = Readiness::Recovering {
                    lease: Some(lease),
                    generation,
                };
                recoveries.push(Recovery {
                    resource: resource.clone(),
                    generation,
                });
            }
        }
        let held_ms = now_ms.saturating_sub(holder.granted_unix.saturating_mul(1000));
        let record = LeaseRecord {
            id: lease,
            balance_after_ms: balance::of(&self.state.balances, &holder.ticket.owner),
            charged_ms: crate::preempt::charged_ms(&holder, held_ms),
            owner: holder.ticket.owner,
            work: holder.ticket.work,
            granted_unix: holder.granted_unix,
            released_unix: now_ms / 1000,
            outcome: if holder.preempted.is_some() {
                LeaseOutcome::PreemptedOnRequest
            } else {
                outcome
            },
            reason: holder.reason,
            preempted: holder.preempted,
            scenarios: holder.ticket.scenarios,
            run: holder.ticket.run,
            unknown: Default::default(),
        };
        Some(Ended { record, recoveries })
    }

    /// What held leases are due at `now_ms`: a lease held for `limit` ends,
    /// and divisible work held for `slice` yields to a conflicting waiter
    /// whose owner's balance is higher.
    pub fn due(&mut self, now_ms: u64, slice: Duration, limit: Duration) -> Vec<(u64, Due)> {
        balance::settle(&mut self.state, now_ms);
        let now_unix = now_ms / 1000;
        self.state
            .holders
            .iter()
            .filter_map(|holder| {
                let id = holder.ticket.id;
                if now_unix.saturating_sub(holder.granted_unix) >= limit.as_secs() {
                    return Some((id, Due::HardLimit));
                }
                crate::queue::outranked_by(&self.state, id, now_unix, slice)
                    .map(|(waiter, balance_ms)| (id, Due::Yield { waiter, balance_ms }))
            })
            .collect()
    }

    /// Mark held lease `lease` preempted by `by` for `reason` at `now_ms`:
    /// it is charged no longer. A repeated preemption keeps the first.
    pub fn preempt(
        &mut self,
        lease: u64,
        by: &str,
        reason: &str,
        now_ms: u64,
    ) -> crate::Result<()> {
        if reason.trim().is_empty() {
            return Err("a preemption needs a reason".into());
        }
        balance::settle(&mut self.state, now_ms);
        let Some(holder) = self
            .state
            .holders
            .iter_mut()
            .find(|holder| holder.ticket.id == lease)
        else {
            return Err(
                if self.state.queue.iter().any(|ticket| ticket.id == lease) {
                    format!("lease #{lease} is waiting, not held; nothing to preempt")
                } else {
                    format!("no lease #{lease} is held")
                }
                .into(),
            );
        };
        holder
            .preempted
            .get_or_insert_with(|| crate::preempt::Preemption {
                by: by.to_owned(),
                reason: reason.to_owned(),
                unix_ms: now_ms,
                unknown: crate::Unknown::default(),
            });
        Ok(())
    }

    /// The held leases a waiting maintenance request conflicts with and
    /// that are not preempted yet, each with that request's ticket.
    pub fn maintenance_conflicts(&self) -> Vec<(u64, u64)> {
        self.state
            .holders
            .iter()
            .filter(|holder| holder.preempted.is_none())
            .filter_map(|holder| {
                self.state
                    .queue
                    .iter()
                    .find(|ticket| {
                        ticket.priority == Priority::Maintenance
                            && conflict(&ticket.claims, &holder.ticket.claims)
                    })
                    .map(|ticket| (holder.ticket.id, ticket.id))
            })
            .collect()
    }

    /// Apply a driver's confirmation of `recovery`. A confirmation of an
    /// older generation changes nothing and returns false.
    pub fn confirm(&mut self, recovery: &Recovery, confirmation: Confirmation) -> bool {
        let Some(readiness) = self.state.readiness.get_mut(&recovery.resource) else {
            return false;
        };
        if !matches!(readiness, Readiness::Recovering { generation, .. } if *generation == recovery.generation)
        {
            return false;
        }
        *readiness = match confirmation {
            Confirmation::Ready => Readiness::Ready {
                generation: recovery.generation,
            },
            Confirmation::Unavailable { reason, quiet } => Readiness::Unavailable {
                reason,
                quiet,
                generation: recovery.generation,
            },
        };
        true
    }

    /// A person returns `resource`: recover it again. `None` when it is not
    /// unavailable.
    pub fn release(&mut self, resource: &str) -> Option<Recovery> {
        let readiness = self.state.readiness.get_mut(resource)?;
        let Readiness::Unavailable { generation, .. } = readiness else {
            return None;
        };
        let generation = *generation + 1;
        *readiness = Readiness::Recovering {
            lease: None,
            generation,
        };
        Some(Recovery {
            resource: resource.to_owned(),
            generation,
        })
    }
}

#[cfg(test)]
mod tests;
