use super::*;

use crate::diagnostics::core0_rx_performance::Core0PathCount;

impl<
    'queue,
    'pool,
    'scratch,
    'irq,
    M: RawMutex,
    S,
    K,
    const DEPTH: usize,
    const CAPACITY: usize,
    const SLOTS: usize,
    const REORDER_SLOTS: usize,
>
    ConnectedReceiveProtocol<
        'queue,
        'pool,
        'scratch,
        'irq,
        M,
        S,
        K,
        DEPTH,
        CAPACITY,
        SLOTS,
        REORDER_SLOTS,
    >
where
    S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
    K: Timer,
{
    /// Whether this owner can make protocol progress without a fresh MAC IRQ.
    pub fn has_ready_work(&self) -> bool {
        !self.frames.is_empty()
            || self
                .processor
                .reorder_commands
                .as_ref()
                .is_some_and(|commands| !commands.is_empty())
            || self
                .processor
                .next_gap_deadline()
                .is_some_and(|(_, deadline)| deadline <= self.processor.clock.now())
    }

    /// Wait only for the next finite reorder deadline.
    ///
    /// Staging publications already carry the physical RX wake and reorder
    /// commands are emitted by the connected control owner. Keeping this
    /// future timer-only avoids consuming a queue item merely to wake the
    /// common DATAPATH scheduler.
    pub fn wait_ready(&mut self) -> impl Future<Output = ()> + '_ {
        let deadline = self
            .processor
            .next_gap_deadline()
            .map(|(_, deadline)| deadline);
        let clock = &self.processor.clock;
        async move {
            match deadline {
                Some(deadline) => clock.wait_until(deadline).await,
                None => pending().await,
            }
        }
    }

    /// Drain ready commands/deadlines and at most `maximum_frames` staging
    /// leases without waiting for another producer edge.
    ///
    /// Ready command and gap processing stays ahead of each synchronous frame
    /// batch. The direct path cannot await, create a reorder gap, or let the
    /// same Core0 control owner run, so repeating those scans between adjacent
    /// direct frames would only add per-MPDU scheduler work. Any asynchronous
    /// fallback crosses a scheduling boundary and therefore re-enables the
    /// command/deadline preflight before the next frame. The independent action
    /// bound prevents a continuously publishing control plane from turning one
    /// radio poll into an unbounded protocol loop.
    pub async fn service_bounded(&mut self, maximum_frames: usize) -> ConnectedRxProtocolTurn {
        assert!(
            self.processor.runtime.dispatcher_configured(),
            "stopped connected RX protocol cannot service another turn"
        );
        let maximum_frames = maximum_frames.max(1);
        let maximum_actions = maximum_frames
            .saturating_add(crate::datapath::rx::reorder::RX_REORDER_COMMAND_CAPACITY)
            .saturating_add(RX_BLOCK_ACK_BANK_COUNT);
        let mut actions = 0_usize;
        let mut consumed_frames = 0_usize;
        let mut control_preflight_needed = true;
        let mut direct_frames = Core0PathCount::default();
        let mut asynchronous_frames = Core0PathCount::default();

        while consumed_frames < maximum_frames && actions < maximum_actions {
            if control_preflight_needed {
                if let Some(command) = self
                    .processor
                    .reorder_commands
                    .as_ref()
                    .and_then(try_receive_rx_reorder_command)
                {
                    actions = actions.saturating_add(1);
                    let _ = self.processor.apply_reorder_command(command).await;
                    continue;
                }

                if let Some((bank, deadline)) = self.processor.next_gap_deadline()
                    && deadline <= self.processor.clock.now()
                {
                    actions = actions.saturating_add(1);
                    let _ = self.processor.expire_reorder_gap(bank).await;
                    continue;
                }
                control_preflight_needed = false;
            }

            let Ok(frame) = self.frames.try_receive() else {
                break;
            };
            crate::diagnostics::core0_rx_cycles::CORE0_RX_CYCLES
                .record_protocol_frame_dequeued(crate::diagnostics::core0_rx_cycles::cycle_count());
            actions = actions.saturating_add(1);
            consumed_frames = consumed_frames.saturating_add(1);
            // The sink's stable ownership capability decides whether direct
            // publication is possible. Rejection returns this exact staging
            // owner to the capacity-aware asynchronous path.
            let frame = match self.processor.try_dispatch_frame_direct(frame) {
                Ok(_) => {
                    direct_frames.increment();
                    continue;
                }
                Err(frame) => frame,
            };
            asynchronous_frames.increment();
            let _ = self.processor.dispatch_frame(frame).await;
            // The fallback may await network capacity or create/release a BA
            // gap. A same-core control action can become visible across that
            // boundary, so restore strict control-before-frame ordering.
            control_preflight_needed = true;
        }

        ConnectedRxProtocolTurn {
            consumed_frames,
            work_remaining: self.has_ready_work(),
            direct_frames,
            asynchronous_frames,
        }
    }
}
