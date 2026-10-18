use super::*;

#[test]
fn batch_target_uses_negotiated_window_instead_of_physical_arena_capacity() {
    assert_eq!(access_point_tx_batch_target(Some(16), 32), 16);
    assert_eq!(access_point_tx_batch_target(Some(8), 32), 8);
    assert_eq!(access_point_tx_batch_target(None, 32), 1);
}

#[test]
fn active_physical_tx_overrides_an_idle_ap_mac_pending_bit() {
    assert!(!AccessPointRxTxDomain::IdleBoundary.tx_pending(false));
    assert!(AccessPointRxTxDomain::IdleBoundary.tx_pending(true));
    assert!(AccessPointRxTxDomain::ActiveTransaction.tx_pending(false));
    assert!(AccessPointRxTxDomain::ActiveTransaction.is_externally_owned());
}

#[test]
fn tx_blocked_protocol_head_is_not_reported_as_a_hardware_probe() {
    assert_eq!(
        ap_rx_progress_while_protocol_tx_blocked(DatapathRxProgress::Drained),
        DatapathRxProgress::ProtocolBlockedByTx
    );
    assert_eq!(
        ap_rx_progress_while_protocol_tx_blocked(DatapathRxProgress::ProbePending),
        DatapathRxProgress::ProbePending
    );
    assert_eq!(
        ap_rx_progress_while_protocol_tx_blocked(DatapathRxProgress::StageCapacityBlocked),
        DatapathRxProgress::StageCapacityBlocked
    );
}

#[test]
fn active_physical_tx_preserves_the_staged_head_until_mailbox_capacity_returns() {
    let active = AccessPointRxTxDomain::ActiveTransaction;
    assert!(!active.protocol_mailbox_ready(AP_PROTOCOL_ACTIONS_PER_RX_FRAME - 1));
    assert!(active.protocol_mailbox_ready(AP_PROTOCOL_ACTIONS_PER_RX_FRAME));
    assert!(AccessPointRxTxDomain::IdleBoundary.protocol_mailbox_ready(0));
}

#[test]
fn active_rx_quantum_keeps_a_staged_backlog_runnable_after_dma_refill() {
    assert_eq!(AP_ACTIVE_TX_PROTOCOL_QUANTUM_FRAMES, 4);

    let mut queued = FusedRxTurn::new(AP_ACTIVE_TX_PROTOCOL_QUANTUM_FRAMES);
    queued.observe_dma(DatapathRxProgress::StageCapacityBlocked);
    assert_eq!(queued.finish(true), DatapathRxProgress::BudgetExhausted);

    let mut idle = FusedRxTurn::new(AP_ACTIVE_TX_PROTOCOL_QUANTUM_FRAMES);
    idle.observe_dma(DatapathRxProgress::StageCapacityBlocked);
    assert_eq!(idle.finish(false), DatapathRxProgress::StageCapacityBlocked);
}

#[derive(Default)]
struct RecordingObserver(std::sync::Mutex<std::vec::Vec<AggregateTxObservation>>);

impl AggregateTxObserver for RecordingObserver {
    fn now_micros(&self) -> u64 {
        0
    }

    fn observe(&self, observation: AggregateTxObservation) {
        self.0.lock().unwrap().push(observation);
    }
}

#[test]
fn staged_dma_burst_does_not_spend_unprocessed_protocol_budget() {
    let mut turn = FusedRxTurn::new(32);

    // The producer may have staged all 32 DMA descriptors, but this turn
    // accounts only the one staged owner actually consumed by AP RX.
    turn.observe_protocol(1, false);

    assert!(turn.has_protocol_budget());
}

#[test]
fn queued_tx_caps_ap_protocol_turn_at_datapath_frame_credit() {
    assert_eq!(
        FusedRxTurn::from_context(
            DatapathRxServiceContext {
                maximum_protocol_frames: None,
            },
            32,
        )
        .remaining_protocol_frames(),
        32
    );
    assert_eq!(
        FusedRxTurn::from_context(
            DatapathRxServiceContext {
                maximum_protocol_frames: Some(4),
            },
            32,
        )
        .remaining_protocol_frames(),
        4
    );
    assert_eq!(
        FusedRxTurn::from_context(
            DatapathRxServiceContext {
                maximum_protocol_frames: Some(0),
            },
            32,
        )
        .remaining_protocol_frames(),
        1
    );
}

#[test]
fn block_ack_readiness_is_published_only_on_live_state_edges() {
    let observer = RecordingObserver::default();
    let mut state = BlockAckObservationState::default();

    state.update(false, Some(&observer));
    state.update(true, Some(&observer));
    state.update(true, Some(&observer));
    state.update(false, Some(&observer));

    assert_eq!(
        *observer.0.lock().unwrap(),
        [
            AggregateTxObservation::BlockAckOperational {
                tid: 0,
                operational: true,
            },
            AggregateTxObservation::BlockAckOperational {
                tid: 0,
                operational: false,
            },
        ]
    );
}

#[test]
fn bounded_rx_turn_yields_at_or_beyond_its_staged_frame_quota() {
    let mut exact = FusedRxTurn::new(4);
    exact.observe_protocol(4, false);
    assert!(!exact.has_protocol_budget());

    let mut overshoot = FusedRxTurn::new(4);
    overshoot.observe_protocol(5, false);
    assert!(!overshoot.has_protocol_budget());
}

#[test]
fn bounded_rx_turn_does_not_charge_empty_protocol_probes_as_frames() {
    let mut turn = FusedRxTurn::new(2);

    turn.observe_protocol(0, false);
    turn.observe_protocol(0, false);

    assert!(turn.has_protocol_budget());
    assert_eq!(turn.remaining_protocol_frames(), 2);
}

struct NetworkRx {
    capacity: usize,
    frames: std::vec::Vec<std::vec::Vec<u8>>,
}

impl DatapathNetworkRx for NetworkRx {
    fn queue_len(&self) -> usize {
        self.frames.len()
    }

    fn try_send(&mut self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        if self.frames.len() == self.capacity {
            return Err(RxEnqueueError::QueueFull);
        }
        self.frames.push(frame.to_vec());
        Ok(())
    }

    fn try_send_parts(&mut self, frame: EthernetFrameParts<'_>) -> Result<(), RxEnqueueError> {
        if self.frames.len() == self.capacity {
            return Err(RxEnqueueError::QueueFull);
        }
        let mut storage = std::vec![0; frame.length()];
        frame.copy_to(&mut storage).expect("test frame fits");
        self.frames.push(storage);
        Ok(())
    }

    fn poll_ready(&mut self, _context: &mut core::task::Context<'_>) -> core::task::Poll<()> {
        if self.frames.len() < self.capacity {
            core::task::Poll::Ready(())
        } else {
            core::task::Poll::Pending
        }
    }

    fn try_send_observed(
        &mut self,
        frame: &[u8],
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        let result = self.try_send(frame);
        if result.is_ok() {
            before_publish();
        }
        result
    }

    fn try_send_parts_observed(
        &mut self,
        frame: EthernetFrameParts<'_>,
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        let result = self.try_send_parts(frame);
        if result.is_ok() {
            before_publish();
        }
        result
    }
}

fn pack(storage: &mut [u8], payloads: &[&[u8]]) -> usize {
    let mut writer = crate::datapath::rx::ethernet::PackedEthernetWriter::new(storage);
    for (index, payload) in payloads.iter().enumerate() {
        writer
            .push(EthernetFrameParts {
                destination: [2, 0, 0, 0, 0, 1],
                source: [2, 0, 0, 0, 0, 2],
                ether_type: 0x0800 + index as u16,
                payload,
            })
            .unwrap();
    }
    writer.used()
}

#[test]
fn ap_batch_publishes_every_amsdu_subframe() {
    let mut storage = [0_u8; 128];
    let used = pack(&mut storage, &[&[0; 20], &[1; 20]]);
    let mut network = NetworkRx {
        capacity: 2,
        frames: std::vec::Vec::new(),
    };
    let mut offset = 0;
    while let Some(record) =
        crate::datapath::rx::ethernet::record_at(&storage, used, offset).unwrap()
    {
        network.try_send_parts(record.frame).unwrap();
        offset = record.next_offset;
    }

    assert_eq!(offset, used);
    assert_eq!(network.frames.len(), 2);
    assert_eq!(&network.frames[0][14..], &[0; 20]);
    assert_eq!(&network.frames[1][14..], &[1; 20]);
}

#[test]
fn ap_batch_retries_the_same_record_after_backpressure() {
    let mut storage = [0_u8; 128];
    let used = pack(&mut storage, &[&[0; 20], &[1; 20]]);
    let mut network = NetworkRx {
        capacity: 1,
        frames: std::vec::Vec::new(),
    };
    let first = crate::datapath::rx::ethernet::record_at(&storage, used, 0)
        .unwrap()
        .unwrap();
    network.try_send_parts(first.frame).unwrap();
    let retained_offset = first.next_offset;
    let second = crate::datapath::rx::ethernet::record_at(&storage, used, retained_offset)
        .unwrap()
        .unwrap();
    assert_eq!(
        network.try_send_parts(second.frame),
        Err(RxEnqueueError::QueueFull)
    );

    // Network consumption releases capacity. The publication cursor was
    // not advanced by QueueFull, so the exact second record is retried.
    network.frames.clear();
    let retry = crate::datapath::rx::ethernet::record_at(&storage, used, retained_offset)
        .unwrap()
        .unwrap();
    assert_eq!(retry.frame.payload, &[1; 20]);
    network.try_send_parts(retry.frame).unwrap();

    assert_eq!(network.frames.len(), 1);
    assert_eq!(&network.frames[0][14..], &[1; 20]);
    assert_eq!(retry.next_offset, used);
}
