//! What the worker shares with the callers: the running boot with its kept
//! messages, the requests waiting for their responses and the counters.
//!
//! A message of the running boot has a position: its `message_sequence`
//! extended past wrap-around. A response goes to its request's slot; every
//! other message is kept in a bounded ring, where a newer one displaces the
//! oldest. Reading leaves the ring as it is: each cursor keeps its own
//! position. A position the device skipped or the ring displaced is lost;
//! lost positions only grow, so the highest one tells a cursor whether the
//! part it has not read lost one.

use std::collections::VecDeque;

use oer_hil_protocol::{Key, Message, base::Hello};

use crate::{Boot, BootStats, Health, LinkError, Received};

/// Device messages of the running boot the link keeps for a cursor; the
/// oldest one makes room for a newer and is lost.
pub const EVENT_CAPACITY: usize = 4096;

/// Requests that may wait for their responses at once.
pub const MAX_PENDING: usize = 16;

/// Diagnostic lines kept until taken; the oldest one makes room for a newer
/// and is counted as lost.
pub const NOTES_CAPACITY: usize = 1024;

/// Ended boots whose counters [`Health`] keeps.
pub const PAST_BOOTS: usize = 4;

/// A request waiting for its response.
struct Slot {
    boot_id: u64,
    request_id: u32,
    response: Key,
    message: Option<Received>,
}

/// The running boot as the link saw it.
struct Running {
    stats: BootStats,
    hello: Option<Hello>,
    /// The device sequence number and position the next message carries.
    sequence: u32,
    position: u64,
    /// The highest position the device skipped or the ring displaced.
    lost_through: Option<u64>,
    kept: VecDeque<(u64, Received)>,
}

impl Running {
    fn new(message: &Received) -> Self {
        Self {
            stats: BootStats {
                boot_id: message.boot_id,
                ..BootStats::default()
            },
            hello: None,
            sequence: message.message_sequence,
            position: u64::from(message.message_sequence),
            lost_through: None,
            kept: VecDeque::new(),
        }
    }

    /// Lose `position`.
    fn lose(&mut self, position: u64) {
        self.lost_through = self.lost_through.max(Some(position));
    }
}

#[derive(Default)]
pub(crate) struct State {
    running: Option<Running>,
    past: VecDeque<BootStats>,
    pending: Vec<Slot>,
    boots: u32,
    pub(crate) counters: oer_hil_protocol::DecodeCounters,
    notes: VecDeque<String>,
    notes_lost: u64,
    /// Lines lost since the last [`Self::take_notes`].
    notes_unreported: u64,
    failure: Option<LinkError>,
}

/// Whether a cursor's interval of its boot holds every message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Completeness {
    /// Every message since the cursor started is kept or was taken.
    Complete,
    /// A message since the cursor started is lost: the device skipped it or
    /// the link displaced it before anyone took it.
    Lossy,
    /// The device runs boot `boot_id` now: the cursor's boot ended, and no
    /// later message of it can come.
    Ended { boot_id: u64 },
}

impl State {
    pub(crate) fn fail(&mut self, error: LinkError) {
        self.failure.get_or_insert(error);
    }

    pub(crate) fn failure(&self) -> Option<&LinkError> {
        self.failure.as_ref()
    }

    /// The running boot's identity, once a frame showed it.
    pub(crate) fn boot_id(&self) -> Option<u64> {
        self.running.as_ref().map(|running| running.stats.boot_id)
    }

    /// The running boot, once its Hello arrived.
    pub(crate) fn hello(&self) -> Option<Boot> {
        let running = self.running.as_ref()?;
        Some(Boot {
            boot_id: running.stats.boot_id,
            hello: running.hello?,
        })
    }

    pub(crate) fn health(&self) -> Health {
        Health {
            counters: self.counters,
            boots: self.boots,
            running: self.running.as_ref().map(|running| running.stats),
            past: self.past.iter().copied().collect(),
            notes_lost: self.notes_lost,
        }
    }

    /// Takes `message` into the boot accounting, then into its request's
    /// slot or the kept messages.
    pub(crate) fn receive(&mut self, message: Received) {
        if message.boot_id == 0 {
            self.fail(LinkError::Protocol(
                "the device sent the reserved boot identity 0".to_owned(),
            ));
            return;
        }
        let position = match self.running.as_mut() {
            Some(running) if running.stats.boot_id == message.boot_id => {
                let missing = message.message_sequence.wrapping_sub(running.sequence);
                if missing > u32::MAX / 2 {
                    let error = LinkError::Protocol(format!(
                        "device message {} repeats or reorders boot {:016x}'s sequence",
                        message.message_sequence, message.boot_id
                    ));
                    self.fail(error);
                    return;
                }
                let position = running.position.saturating_add(u64::from(missing));
                if missing > 0 {
                    running.stats.missing = running.stats.missing.saturating_add(missing.into());
                    running.lose(position.saturating_sub(1));
                }
                position
            }
            _ => {
                self.boots = self.boots.saturating_add(1);
                let running = Running::new(&message);
                let position = running.position;
                if let Some(ended) = self.running.replace(running) {
                    if self.past.len() == PAST_BOOTS {
                        self.past.pop_front();
                    }
                    self.past.push_back(ended.stats);
                }
                position
            }
        };
        let Some(running) = self.running.as_mut() else {
            return;
        };
        running.sequence = message.message_sequence.wrapping_add(1);
        running.position = position.saturating_add(1);
        let hello = message.decode::<Hello>().and_then(Result::ok);
        if hello.is_some() {
            running.hello = hello;
        }
        if let Some(slot) = self.pending.iter_mut().find(|slot| {
            slot.message.is_none()
                && slot.boot_id == message.boot_id
                && slot.request_id == message.request_id
                && message.session_id == 0
                && (message.key == slot.response
                    || message.key == <oer_hil_protocol::base::Rejected as Message>::KEY)
        }) {
            slot.message = Some(message);
            return;
        }
        // A Hello outside a request is the boot's, which the link holds.
        if hello.is_some() {
            return;
        }
        if running.kept.len() == EVENT_CAPACITY
            && let Some((displaced, _)) = running.kept.pop_front()
        {
            running.stats.displaced = running.stats.displaced.saturating_add(1);
            running.lose(displaced);
        }
        running.kept.push_back((position, message));
    }

    /// Reserve a slot for the response to `request_id` of `boot_id`;
    /// `false` when [`MAX_PENDING`] requests wait already.
    pub(crate) fn expect(&mut self, boot_id: u64, request_id: u32, response: Key) -> bool {
        if self.pending.len() >= MAX_PENDING {
            return false;
        }
        self.pending.push(Slot {
            boot_id,
            request_id,
            response,
            message: None,
        });
        true
    }

    /// The response to `request_id`, once it arrived; the slot stays
    /// reserved until [`Self::forget`].
    pub(crate) fn response(&mut self, request_id: u32) -> Option<Received> {
        self.pending
            .iter_mut()
            .find(|slot| slot.request_id == request_id)?
            .message
            .take()
    }

    /// Release `request_id`'s slot: a later response is kept as any other
    /// message.
    pub(crate) fn forget(&mut self, request_id: u32) {
        self.pending.retain(|slot| slot.request_id != request_id);
    }

    /// A cursor over the messages of `boot_id` from now on.
    pub(crate) fn cursor(&self, boot_id: u64) -> crate::Cursor {
        crate::Cursor {
            boot_id,
            next: self
                .running
                .as_ref()
                .filter(|running| running.stats.boot_id == boot_id)
                .map_or(u64::MAX, |running| running.position),
            lossy: false,
        }
    }

    pub(crate) fn completeness(&self, cursor: &crate::Cursor) -> Completeness {
        match &self.running {
            Some(running) if running.stats.boot_id == cursor.boot_id => {
                if cursor.lossy || running.lost_through >= Some(cursor.next) {
                    Completeness::Lossy
                } else {
                    Completeness::Complete
                }
            }
            Some(running) => Completeness::Ended {
                boot_id: running.stats.boot_id,
            },
            // No frame yet: the cursor's boot never showed.
            None => Completeness::Lossy,
        }
    }

    /// The first kept message `cursor` has not read that `matches` accepts.
    /// The cursor moves past it and past every message it passed over; the
    /// ring keeps them all for other cursors.
    pub(crate) fn take(
        &self,
        cursor: &mut crate::Cursor,
        matches: impl Fn(&Received) -> bool,
    ) -> Option<Received> {
        let running = self
            .running
            .as_ref()
            .filter(|running| running.stats.boot_id == cursor.boot_id)?;
        let unread = running
            .kept
            .partition_point(|(position, _)| *position < cursor.next);
        let found = running
            .kept
            .range(unread..)
            .find(|(_, message)| matches(message));
        let next = found.map_or(running.position, |(position, _)| position.saturating_add(1));
        // Lost positions only grow: one at or past `cursor.next` now is
        // either among those the cursor passes over or still ahead of it.
        if running.lost_through >= Some(cursor.next) {
            cursor.lossy = true;
        }
        cursor.next = cursor.next.max(next);
        found.map(|(_, message)| message.clone())
    }

    /// Keep diagnostic `lines`, displacing the oldest beyond
    /// [`NOTES_CAPACITY`].
    pub(crate) fn note(&mut self, lines: Vec<String>) {
        for line in lines {
            if self.notes.len() == NOTES_CAPACITY {
                self.notes.pop_front();
                self.notes_lost = self.notes_lost.saturating_add(1);
                self.notes_unreported = self.notes_unreported.saturating_add(1);
            }
            self.notes.push_back(line);
        }
    }

    /// The kept diagnostic lines and how many were lost since the last call.
    pub(crate) fn take_notes(&mut self) -> crate::DeviceNotes {
        crate::DeviceNotes {
            lines: self.notes.drain(..).collect(),
            lost: std::mem::take(&mut self.notes_unreported),
        }
    }
}
