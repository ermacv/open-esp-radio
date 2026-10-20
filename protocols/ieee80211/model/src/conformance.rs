//! Actual port operations for host comparison with a physical backend.
use super::*;
use oer_ieee80211_port::{
    conformance::{MpduState, outcomes},
    *,
};
use oer_ieee80211_wire::{frame::FrameControl, phy::LegacyRate};
use oer_radio_port::{Disabled, Ended, Owner, PortBody, PortEnd};

mod ownership;
pub use ownership::{OwnershipTrace, ownership};
mod barrier;
pub use barrier::barrier;

/// A trace whose owners and terminals were read from ModelBackend.
pub type OutcomeTrace = outcomes::Trace<ModelAck, ModelBlockAckEvidence, ModelAirDenied>;

/// Execute an elementary single-MPDU air case on the model with owner time.
pub async fn outcome(
    case: outcomes::Case,
    channel: Channel,
    rate: LegacyRate,
    clock: Rc<dyn Clock>,
) -> Result<OutcomeTrace, &'static str> {
    if case.form() != outcomes::Form::Single {
        return Err("aggregate fixture required");
    }
    let station = IndividualAddress::new([2, 0, 0, 0, 0, 1]).ok_or("fixture station")?;
    let ap = IndividualAddress::new([2, 0, 0, 0, 0, 2]).ok_or("fixture AP")?;
    let (backend, air) = ModelBackend::<ModelCaps>::new(channel, [station, ap], clock);
    let answer = match case {
        outcomes::Case::Ack => AirAnswer::Ack,
        outcomes::Case::NoResponse(_) => AirAnswer::NoResponse,
        outcomes::Case::Collision(_) => AirAnswer::Collision,
        outcomes::Case::AirDenied(_) => AirAnswer::AirDenied,
        outcomes::Case::HardwareTimeout(_) => AirAnswer::HardwareTimeout,
        _ => return Err("single outcome required"),
    };
    air.answer(answer);
    match oer_radio_port::run_port(backend, OutcomeBody { rate }).await {
        PortEnd::Uninstalled { output, .. } => output,
        _ => Err("model failed to uninstall"),
    }
}
struct OutcomeBody {
    rate: LegacyRate,
}
impl PortBody<ModelBackend> for OutcomeBody {
    type Output = Result<OutcomeTrace, &'static str>;
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, ModelBackend, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend, Self::Output> {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(failed) => return Ended::fail_stopped(failed.fail_stop(), Err("model enable")),
        };
        let result = exchange(&mut owner.phase(), resources, self.rate).await;
        let owner = match owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        {
            Ok(owner) => owner,
            Err(failed) => return Ended::fail_stopped(failed.fail_stop(), Err("model disable")),
        };
        Ended::uninstall(owner, (), result)
    }
}
async fn exchange<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id>,
    resources: ModelResources<'id>,
    legacy: LegacyRate,
) -> Result<OutcomeTrace, &'static str> {
    let ModelResources {
        tickets,
        small,
        mut terminal_access,
        ..
    } = resources;
    let ticket = tickets.into_iter().next().ok_or("model ticket")?;
    let buffer = small.into_iter().next().ok_or("model buffer")?;
    let admitted = outcomes::Identity {
        ticket: ticket.index(),
        buffer: buffer.slot.index(),
    };
    let rate = phase.rate(PhyRate::Legacy(legacy)).ok_or("model rate")?;
    let receiver = IndividualAddress::new([2, 0, 0, 0, 0, 9]).ok_or("fixture peer")?;
    let header = HeaderSpec::new(
        FrameControl::new(0x0040).ok_or("probe control")?,
        0,
        receiver,
        [2, 0, 0, 0, 0, 1],
        receiver.get(),
        0x1230,
        None,
        None,
        None,
    )
    .ok_or("model header")?;
    let frame = ClassFrame::new(TxFrame::Individual {
        frame: SizedFrame::new(
            AckHeader::new(header).ok_or("model ack header")?,
            MpduLength::new(26).ok_or("model length")?,
        )
        .ok_or("model frame")?,
        plan: RetryPlan::new(RetryStage {
            rate,
            transmissions: core::num::NonZeroU8::MIN,
            protection: Protection::None,
        }),
    })
    .map_err(|_| "model class")?;
    let source = TxSource::Fresh {
        buffer,
        frame,
        body: None,
        protection: TxProtection::Open,
    }
    .check()
    .map_err(|_| "model layout")?;
    let source = prepare_model(source).map_err(|_| "model preparation")?;
    phase
        .station()
        .submit(
            Attempt {
                context: (),
                ticket,
                source,
                access_category: oer_ieee80211_wire::qos::WmmAccessCategory::BestEffort,
                protection: Protection::None,
                power: SupportedPower::new(TxPower::Calibrated).ok_or("model power")?,
                coex_operation: TxOperation::Connection(ConnectionFrame::ProbeRequest),
            }
            .admit(),
        )
        .map_err(|_| "model submit")?;
    let completed = loop {
        match phase
            .next_event()
            .await
            .map_err(|_| "model poisoned")?
            .map_err(|_| "model drained")?
        {
            LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(TxCompleted::Single(
                completed,
            ))) => break completed,
            LowerMacEvent::Received { frame, .. } => phase.recycle(frame),
            _ => return Err("unexpected model event"),
        }
    };
    let parts = completed.into_parts(&mut terminal_access);
    let (buffer, _) = parts.payload.into_parts();
    let ModelAnyBuffer::SmallSent(sent) = buffer else {
        return Err("model returned Fresh");
    };
    let returned = outcomes::Identity {
        ticket: parts.ticket.index(),
        buffer: sent.buffer.slot.index(),
    };
    let sequence = SequenceNumber::from_sequence_control(u16::from_le_bytes(
        sent.buffer
            .memory
            .get(22..24)
            .ok_or("model sequence")?
            .try_into()
            .map_err(|_| "model sequence")?,
    ));
    Ok(outcomes::Trace {
        admitted,
        returned,
        state: MpduState::Sent,
        rate: PhyRate::Legacy(legacy),
        sequence,
        publications: phase.held.air.sent().len(),
        outcome: outcomes::Outcome::Single(parts.outcome),
    })
}
