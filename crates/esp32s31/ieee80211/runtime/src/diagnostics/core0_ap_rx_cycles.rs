//! Low-overhead phase timing for successful AP protected-data ingress.

pub(crate) use super::core0_paths::Core0ApRxTurnExit;

use core::sync::atomic::{AtomicU32, Ordering};

use super::core0_rx_cycles::cycle_count;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Core0ApRxCycleSnapshot {
    pub calls: u32,
    pub total: u32,
    pub view: u32,
    pub dispatch: u32,
    pub dispatch_leaf: u32,
    pub reorder_key: u32,
    pub leaf_peer: u32,
    pub leaf_publish_check: u32,
    pub leaf_body: u32,
    pub leaf_admission: u32,
    pub leaf_observe: u32,
    pub publication: u32,
    pub activity_tail: u32,
    pub telemetry: u32,
    pub in_place_eligible: u32,
    pub in_place_published: u32,
    pub deferred_published: u32,
    pub reorder_buffered: u32,
    pub turn_calls: u32,
    pub turn_frames: u32,
    pub turn_initial_batch: u32,
    pub turn_initial_reorder: u32,
    pub turn_mailbox_blocked: u32,
    pub turn_tx_blocked: u32,
    pub turn_batch_pending: u32,
    pub turn_reorder_pending: u32,
    pub turn_drained: u32,
    pub turn_budget_exhausted: u32,
}

impl Core0ApRxCycleSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            calls: self.calls.wrapping_sub(earlier.calls),
            total: self.total.wrapping_sub(earlier.total),
            view: self.view.wrapping_sub(earlier.view),
            dispatch: self.dispatch.wrapping_sub(earlier.dispatch),
            dispatch_leaf: self.dispatch_leaf.wrapping_sub(earlier.dispatch_leaf),
            reorder_key: self.reorder_key.wrapping_sub(earlier.reorder_key),
            leaf_peer: self.leaf_peer.wrapping_sub(earlier.leaf_peer),
            leaf_publish_check: self
                .leaf_publish_check
                .wrapping_sub(earlier.leaf_publish_check),
            leaf_body: self.leaf_body.wrapping_sub(earlier.leaf_body),
            leaf_admission: self.leaf_admission.wrapping_sub(earlier.leaf_admission),
            leaf_observe: self.leaf_observe.wrapping_sub(earlier.leaf_observe),
            publication: self.publication.wrapping_sub(earlier.publication),
            activity_tail: self.activity_tail.wrapping_sub(earlier.activity_tail),
            telemetry: self.telemetry.wrapping_sub(earlier.telemetry),
            in_place_eligible: self
                .in_place_eligible
                .wrapping_sub(earlier.in_place_eligible),
            in_place_published: self
                .in_place_published
                .wrapping_sub(earlier.in_place_published),
            deferred_published: self
                .deferred_published
                .wrapping_sub(earlier.deferred_published),
            reorder_buffered: self.reorder_buffered.wrapping_sub(earlier.reorder_buffered),
            turn_calls: self.turn_calls.wrapping_sub(earlier.turn_calls),
            turn_frames: self.turn_frames.wrapping_sub(earlier.turn_frames),
            turn_initial_batch: self
                .turn_initial_batch
                .wrapping_sub(earlier.turn_initial_batch),
            turn_initial_reorder: self
                .turn_initial_reorder
                .wrapping_sub(earlier.turn_initial_reorder),
            turn_mailbox_blocked: self
                .turn_mailbox_blocked
                .wrapping_sub(earlier.turn_mailbox_blocked),
            turn_tx_blocked: self.turn_tx_blocked.wrapping_sub(earlier.turn_tx_blocked),
            turn_batch_pending: self
                .turn_batch_pending
                .wrapping_sub(earlier.turn_batch_pending),
            turn_reorder_pending: self
                .turn_reorder_pending
                .wrapping_sub(earlier.turn_reorder_pending),
            turn_drained: self.turn_drained.wrapping_sub(earlier.turn_drained),
            turn_budget_exhausted: self
                .turn_budget_exhausted
                .wrapping_sub(earlier.turn_budget_exhausted),
        }
    }
}

pub struct Core0ApRxCycleCounters {
    calls: AtomicU32,
    total: AtomicU32,
    view: AtomicU32,
    dispatch: AtomicU32,
    dispatch_leaf: AtomicU32,
    reorder_key: AtomicU32,
    leaf_peer: AtomicU32,
    leaf_publish_check: AtomicU32,
    leaf_body: AtomicU32,
    leaf_admission: AtomicU32,
    leaf_observe: AtomicU32,
    publication: AtomicU32,
    activity_tail: AtomicU32,
    telemetry: AtomicU32,
    in_place_eligible: AtomicU32,
    in_place_published: AtomicU32,
    deferred_published: AtomicU32,
    reorder_buffered: AtomicU32,
    turn_calls: AtomicU32,
    turn_frames: AtomicU32,
    turn_initial_batch: AtomicU32,
    turn_initial_reorder: AtomicU32,
    turn_mailbox_blocked: AtomicU32,
    turn_tx_blocked: AtomicU32,
    turn_batch_pending: AtomicU32,
    turn_reorder_pending: AtomicU32,
    turn_drained: AtomicU32,
    turn_budget_exhausted: AtomicU32,
}

impl Core0ApRxCycleCounters {
    const fn new() -> Self {
        Self {
            calls: AtomicU32::new(0),
            total: AtomicU32::new(0),
            view: AtomicU32::new(0),
            dispatch: AtomicU32::new(0),
            dispatch_leaf: AtomicU32::new(0),
            reorder_key: AtomicU32::new(0),
            leaf_peer: AtomicU32::new(0),
            leaf_publish_check: AtomicU32::new(0),
            leaf_body: AtomicU32::new(0),
            leaf_admission: AtomicU32::new(0),
            leaf_observe: AtomicU32::new(0),
            publication: AtomicU32::new(0),
            activity_tail: AtomicU32::new(0),
            telemetry: AtomicU32::new(0),
            in_place_eligible: AtomicU32::new(0),
            in_place_published: AtomicU32::new(0),
            deferred_published: AtomicU32::new(0),
            reorder_buffered: AtomicU32::new(0),
            turn_calls: AtomicU32::new(0),
            turn_frames: AtomicU32::new(0),
            turn_initial_batch: AtomicU32::new(0),
            turn_initial_reorder: AtomicU32::new(0),
            turn_mailbox_blocked: AtomicU32::new(0),
            turn_tx_blocked: AtomicU32::new(0),
            turn_batch_pending: AtomicU32::new(0),
            turn_reorder_pending: AtomicU32::new(0),
            turn_drained: AtomicU32::new(0),
            turn_budget_exhausted: AtomicU32::new(0),
        }
    }

    pub(crate) fn record_ingress_path(
        &self,
        in_place_eligible: bool,
        in_place_published: bool,
        deferred_published: bool,
        reorder_buffered: bool,
    ) {
        self.in_place_eligible
            .fetch_add(u32::from(in_place_eligible), Ordering::Relaxed);
        self.in_place_published
            .fetch_add(u32::from(in_place_published), Ordering::Relaxed);
        self.deferred_published
            .fetch_add(u32::from(deferred_published), Ordering::Relaxed);
        self.reorder_buffered
            .fetch_add(u32::from(reorder_buffered), Ordering::Relaxed);
    }

    pub(crate) fn record_turn(&self, frames: usize, exit: Core0ApRxTurnExit) {
        self.turn_calls.fetch_add(1, Ordering::Relaxed);
        self.turn_frames
            .fetch_add(u32::try_from(frames).unwrap_or(u32::MAX), Ordering::Relaxed);
        let counter = match exit {
            Core0ApRxTurnExit::InitialBatch => &self.turn_initial_batch,
            Core0ApRxTurnExit::InitialReorder => &self.turn_initial_reorder,
            Core0ApRxTurnExit::MailboxBlocked => &self.turn_mailbox_blocked,
            Core0ApRxTurnExit::TxBlocked => &self.turn_tx_blocked,
            Core0ApRxTurnExit::BatchPending => &self.turn_batch_pending,
            Core0ApRxTurnExit::ReorderPending => &self.turn_reorder_pending,
            Core0ApRxTurnExit::Drained => &self.turn_drained,
            Core0ApRxTurnExit::BudgetExhausted => &self.turn_budget_exhausted,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    fn record(&self, profile: Core0ApRxCycleProfile, ended: u32) {
        let telemetry_started = cycle_count();
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.total
            .fetch_add(ended.wrapping_sub(profile.started), Ordering::Relaxed);
        self.view.fetch_add(profile.view, Ordering::Relaxed);
        self.dispatch.fetch_add(profile.dispatch, Ordering::Relaxed);
        self.dispatch_leaf
            .fetch_add(profile.dispatch_leaf, Ordering::Relaxed);
        self.reorder_key
            .fetch_add(profile.reorder_key, Ordering::Relaxed);
        self.leaf_peer
            .fetch_add(profile.leaf_peer, Ordering::Relaxed);
        self.leaf_publish_check
            .fetch_add(profile.leaf_publish_check, Ordering::Relaxed);
        self.leaf_body
            .fetch_add(profile.leaf_body, Ordering::Relaxed);
        self.leaf_admission
            .fetch_add(profile.leaf_admission, Ordering::Relaxed);
        self.leaf_observe
            .fetch_add(profile.leaf_observe, Ordering::Relaxed);
        self.publication
            .fetch_add(profile.publication, Ordering::Relaxed);
        self.activity_tail
            .fetch_add(ended.wrapping_sub(profile.last), Ordering::Relaxed);
        self.telemetry.fetch_add(
            cycle_count().wrapping_sub(telemetry_started),
            Ordering::Relaxed,
        );
    }

    pub fn snapshot(&self) -> Core0ApRxCycleSnapshot {
        Core0ApRxCycleSnapshot {
            calls: self.calls.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
            view: self.view.load(Ordering::Relaxed),
            dispatch: self.dispatch.load(Ordering::Relaxed),
            dispatch_leaf: self.dispatch_leaf.load(Ordering::Relaxed),
            reorder_key: self.reorder_key.load(Ordering::Relaxed),
            leaf_peer: self.leaf_peer.load(Ordering::Relaxed),
            leaf_publish_check: self.leaf_publish_check.load(Ordering::Relaxed),
            leaf_body: self.leaf_body.load(Ordering::Relaxed),
            leaf_admission: self.leaf_admission.load(Ordering::Relaxed),
            leaf_observe: self.leaf_observe.load(Ordering::Relaxed),
            publication: self.publication.load(Ordering::Relaxed),
            activity_tail: self.activity_tail.load(Ordering::Relaxed),
            telemetry: self.telemetry.load(Ordering::Relaxed),
            in_place_eligible: self.in_place_eligible.load(Ordering::Relaxed),
            in_place_published: self.in_place_published.load(Ordering::Relaxed),
            deferred_published: self.deferred_published.load(Ordering::Relaxed),
            reorder_buffered: self.reorder_buffered.load(Ordering::Relaxed),
            turn_calls: self.turn_calls.load(Ordering::Relaxed),
            turn_frames: self.turn_frames.load(Ordering::Relaxed),
            turn_initial_batch: self.turn_initial_batch.load(Ordering::Relaxed),
            turn_initial_reorder: self.turn_initial_reorder.load(Ordering::Relaxed),
            turn_mailbox_blocked: self.turn_mailbox_blocked.load(Ordering::Relaxed),
            turn_tx_blocked: self.turn_tx_blocked.load(Ordering::Relaxed),
            turn_batch_pending: self.turn_batch_pending.load(Ordering::Relaxed),
            turn_reorder_pending: self.turn_reorder_pending.load(Ordering::Relaxed),
            turn_drained: self.turn_drained.load(Ordering::Relaxed),
            turn_budget_exhausted: self.turn_budget_exhausted.load(Ordering::Relaxed),
        }
    }
}

pub static CORE0_AP_RX_CYCLES: Core0ApRxCycleCounters = Core0ApRxCycleCounters::new();

pub(crate) struct Core0ApRxCycleProfile {
    started: u32,
    last: u32,
    view: u32,
    dispatch: u32,
    dispatch_leaf: u32,
    reorder_key: u32,
    leaf_peer: u32,
    leaf_publish_check: u32,
    leaf_body: u32,
    leaf_admission: u32,
    leaf_observe: u32,
    publication: u32,
}

impl Core0ApRxCycleProfile {
    pub(crate) fn begin() -> Self {
        let now = cycle_count();
        Self {
            started: now,
            last: now,
            view: 0,
            dispatch: 0,
            dispatch_leaf: 0,
            reorder_key: 0,
            leaf_peer: 0,
            leaf_publish_check: 0,
            leaf_body: 0,
            leaf_admission: 0,
            leaf_observe: 0,
            publication: 0,
        }
    }

    pub(crate) fn view_complete(&mut self) {
        let now = cycle_count();
        self.view = now.wrapping_sub(self.last);
        self.last = now;
    }

    pub(crate) fn dispatch_complete(&mut self, leaf_cycles: u32) {
        let now = cycle_count();
        self.dispatch = now.wrapping_sub(self.last);
        self.dispatch_leaf = leaf_cycles;
        self.last = now;
    }

    pub(crate) fn record_reorder_key(&mut self, cycles: u32) {
        self.reorder_key = self.reorder_key.wrapping_add(cycles);
    }

    pub(crate) fn record_leaf_peer(&mut self, cycles: u32) {
        self.leaf_peer = self.leaf_peer.wrapping_add(cycles);
    }

    pub(crate) fn record_leaf_publish_check(&mut self, cycles: u32) {
        self.leaf_publish_check = self.leaf_publish_check.wrapping_add(cycles);
    }

    pub(crate) fn record_leaf_body(&mut self, cycles: u32) {
        self.leaf_body = self.leaf_body.wrapping_add(cycles);
    }

    pub(crate) fn record_leaf_admission(&mut self, cycles: u32) {
        self.leaf_admission = self.leaf_admission.wrapping_add(cycles);
    }

    pub(crate) fn record_leaf_observe(&mut self, cycles: u32) {
        self.leaf_observe = self.leaf_observe.wrapping_add(cycles);
    }

    pub(crate) fn publication_complete(&mut self) {
        let now = cycle_count();
        self.publication = now.wrapping_sub(self.last);
        self.last = now;
    }

    pub(crate) fn finish(self) {
        let ended = cycle_count();
        CORE0_AP_RX_CYCLES.record(self, ended);
    }
}
