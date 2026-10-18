//! Independent offered-load state with bounded, event-driven socket service.
use core::task::{Context, Poll};
use oer_hil_protocol::{
    network::FlowTransportEvidence, network::Ipv4Endpoint, network::SESSION_FLOW_CAPACITY,
    network::SessionConfig,
};

#[derive(Clone, Copy)]
pub struct Publication {
    pub peer: Ipv4Endpoint,
    pub payload_bytes: usize,
    pub sequence: u32,
}

struct Flow {
    publication: Publication,
    evidence: FlowTransportEvidence,
    rate: Option<u64>,
    group: u8,
    next_send: u64,
    failed: bool,
}

pub struct Producer {
    flows: [Option<Flow>; SESSION_FLOW_CAPACITY],
    started: u64,
    end: u64,
    cursor: usize,
    burst: u8,
    remaining: u8,
    poll_budget: u8,
    wake_at: u64,
}

impl Producer {
    pub fn new(config: SessionConfig, started: u64, duration: u64, group: u8, burst: u8) -> Self {
        assert!(group != 0 && burst != 0);
        Self {
            flows: config.flows.map(|flow| {
                flow.map(|flow| {
                    let tx = flow.target_tx.expect("validated TX flow");
                    Flow {
                        publication: Publication {
                            peer: flow.peer.expect("validated TX peer"),
                            payload_bytes: usize::from(tx.payload_bytes),
                            sequence: 0,
                        },
                        evidence: FlowTransportEvidence {
                            rx_maximum_silence_micros: None,
                            flow_id: flow.flow_id,
                            rx_bytes: 0,
                            tx_bytes: 0,
                            rx_units: 0,
                            tx_units: 0,
                            rx_late_bytes: 0,
                            rx_late_units: 0,
                            elapsed_micros: 0,
                            transport_errors: 0,
                        },
                        rate: tx.offered_rate_bps,
                        group: tx.pacing_group_datagrams.unwrap_or(group),
                        next_send: started,
                        failed: false,
                    }
                })
            }),
            started,
            end: started.saturating_add(duration),
            cursor: 0,
            burst,
            remaining: 0,
            // Limit successful publications between executor service points.
            poll_budget: group,
            wake_at: started.saturating_add(duration),
        }
    }

    /// Each socket owns its own wait registration. `Pending` skips that flow
    /// for this poll; a ready peer continues. Self-wake occurs only after a
    /// full quantum of actual publications, never to retry blocked sockets.
    pub fn poll<E>(
        &mut self,
        cx: &mut Context<'_>,
        mut now: impl FnMut() -> u64,
        mut publish: impl FnMut(usize, Publication, &mut Context<'_>) -> Poll<Result<(), E>>,
    ) -> Poll<()> {
        let mut blocked = [false; SESSION_FLOW_CAPACITY];
        let mut completed = 0;
        loop {
            let time = now();
            if time >= self.end || self.flows.iter().flatten().all(|flow| flow.failed) {
                return Poll::Ready(());
            }
            if completed == self.poll_budget {
                self.wake_at = self.end;
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            let selected = (0..SESSION_FLOW_CAPACITY)
                .map(|offset| (self.cursor + offset) % SESSION_FLOW_CAPACITY)
                .find(|&index| {
                    !blocked[index]
                        && self.flows[index]
                            .as_ref()
                            .is_some_and(|flow| !flow.failed && flow.next_send <= time)
                });
            let Some(index) = selected else {
                // Preserve the decision time: rechecking deadlines later in
                // the timer wrapper could miss a pacing edge that just elapsed.
                self.wake_at = self
                    .flows
                    .iter()
                    .flatten()
                    .filter(|flow| !flow.failed && flow.next_send > time)
                    .map(|flow| flow.next_send)
                    .min()
                    .unwrap_or(self.end)
                    .min(self.end);
                return Poll::Pending;
            };
            if index != self.cursor || self.remaining == 0 {
                self.remaining = self.burst;
            }
            self.cursor = index;
            let flow = self.flows[index].as_mut().expect("selected flow");
            match publish(index, flow.publication, cx) {
                Poll::Pending => {
                    blocked[index] = true;
                    self.remaining = 0;
                }
                Poll::Ready(Err(_)) => {
                    flow.evidence.transport_errors =
                        flow.evidence.transport_errors.saturating_add(1);
                    flow.failed = true;
                    self.remaining = 0;
                }
                Poll::Ready(Ok(())) => {
                    completed += 1;
                    flow.evidence.tx_bytes = flow
                        .evidence
                        .tx_bytes
                        .saturating_add(flow.publication.payload_bytes as u64);
                    flow.evidence.tx_units = flow.evidence.tx_units.saturating_add(1);
                    flow.publication.sequence = flow.publication.sequence.wrapping_add(1);
                    self.remaining -= 1;
                    if let Some(rate) = flow.rate
                        && flow.evidence.tx_units.is_multiple_of(u64::from(flow.group))
                    {
                        let interval = (u64::from(flow.group)
                            * flow.publication.payload_bytes as u64
                            * 8_000_000)
                            .div_ceil(rate);
                        flow.next_send = flow.next_send.saturating_add(interval);
                        let time = now();
                        if time.saturating_sub(flow.next_send) > interval.saturating_mul(4) {
                            flow.next_send = time;
                        }
                    }
                }
            }
            if self.remaining == 0 {
                self.cursor = (index + 1) % SESSION_FLOW_CAPACITY;
            }
        }
    }

    /// Only future pacing edges and the session deadline need a timer.
    /// Ready-but-blocked flows wait for their socket event instead.
    pub fn deadline(&self) -> u64 {
        self.wake_at
    }

    pub fn evidence(self, now: u64) -> [Option<FlowTransportEvidence>; SESSION_FLOW_CAPACITY] {
        self.flows.map(|flow| {
            flow.map(|flow| FlowTransportEvidence {
                rx_maximum_silence_micros: None,
                rx_late_bytes: 0,
                rx_late_units: 0,
                elapsed_micros: now.saturating_sub(self.started).max(1),
                ..flow.evidence
            })
        })
    }
}
