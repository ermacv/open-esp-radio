//! Actual per-VIF tables and publication draws on every physical AC workload.
use super::*;
use oer_ieee80211_port::conformance::{
    Execution,
    edca::{self as contract, Case, Draw, Scenario},
};
use oer_ieee80211_wire::qos::WmmAccessCategory;
pub type EdcaTrace = contract::Trace<Vec<Draw>>;
pub async fn edca(
    case: Case,
    channel: Channel,
    rate: LegacyRate,
    clock: Rc<dyn Clock>,
) -> Result<EdcaTrace, &'static str> {
    if case.execution() != Execution::Hardware {
        return Err("physical EDCA workload required");
    }
    let station = IndividualAddress::new([2, 0, 0, 0, 0, 1]).ok_or("station")?;
    let ap = IndividualAddress::new([2, 0, 0, 0, 0, 2]).ok_or("AP")?;
    let (backend, air) = ModelBackend::new(channel, [station, ap], clock);
    match oer_radio_port::run_port(backend, Exercise { air, case, rate }).await {
        PortEnd::Uninstalled { output, .. } => output,
        _ => Err("EDCA failed to uninstall"),
    }
}
struct Exercise {
    air: Rc<ModelAir>,
    case: Case,
    rate: LegacyRate,
}
fn submit<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id>,
    role: VifRole,
    category: WmmAccessCategory,
    ticket: Slot<'id, ModelTickets, ModelPool<1>>,
    mut buffer: ModelBuffer<'id, ModelSmall>,
    legacy: LegacyRate,
) -> Result<(), &'static str> {
    buffer.bytes_mut().fill(0);
    let rate = phase.rate(PhyRate::Legacy(legacy)).ok_or("EDCA rate")?;
    let sender = match role {
        VifRole::Station => phase.station().address(),
        VifRole::AccessPoint => phase.access_point().address(),
    };
    let receiver = IndividualAddress::new([2, 0, 0, 0, 0, 9]).ok_or("EDCA receiver")?;
    let header = HeaderSpec::new(
        FrameControl::new(0x0040).ok_or("EDCA FC")?,
        0,
        receiver,
        sender.get(),
        receiver.get(),
        0x1230,
        None,
        None,
        None,
    )
    .and_then(AckHeader::new)
    .ok_or("EDCA header")?;
    let frame = ClassFrame::new(TxFrame::Individual {
        frame: SizedFrame::new(header, MpduLength::new(26).ok_or("EDCA length")?)
            .ok_or("EDCA frame")?,
        plan: RetryPlan::new(RetryStage {
            rate,
            transmissions: core::num::NonZeroU8::MIN,
            protection: Protection::None,
        }),
    })
    .map_err(|_| "EDCA class")?;
    macro_rules! submit {
        ($interface:ident,$role:ty) => {{
            let source = <ModelTypes as TxBufferPool<ModelSmall>>::prepare::<$role, ModelCaps>(
                TxSource::Fresh {
                    buffer,
                    frame,
                    body: None,
                    protection: TxProtection::Open,
                }
                .check()
                .map_err(|_| "EDCA layout")?,
            )
            .map_err(|_| "EDCA prepare")?;
            phase
                .$interface()
                .submit(
                    Attempt {
                        context: (),
                        ticket,
                        source,
                        access_category: category,
                        protection: Protection::None,
                        power: SupportedPower::new(TxPower::Calibrated).ok_or("EDCA power")?,
                        coex_operation: TxOperation::Data,
                    }
                    .admit(),
                )
                .map_err(|_| "EDCA submit")?;
        }};
    }
    match role {
        VifRole::Station => submit!(station, Station),
        VifRole::AccessPoint => submit!(access_point, AccessPoint),
    }
    Ok(())
}
async fn exchange<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id>,
    resources: ModelResources<'id>,
    body: &Exercise,
) -> Result<EdcaTrace, &'static str> {
    body.air.set_backoff_entropy(u32::MAX);
    if body.case.scenario != Scenario::Defaults {
        phase.station().set_edca(
            SupportedEdca::new(contract::parameters(VifRole::Station, false))
                .ok_or("station EDCA")?,
        );
        phase.access_point().set_edca(
            SupportedEdca::new(contract::parameters(VifRole::AccessPoint, false))
                .ok_or("AP EDCA")?,
        );
    }
    let mut tickets = resources.tickets.into_iter();
    let mut buffers = resources.small.into_iter();
    let mut access = resources.terminal_access;
    let mut admitted = [0; 3];
    for (index, role) in [VifRole::Station, VifRole::AccessPoint, VifRole::Station]
        .into_iter()
        .enumerate()
    {
        let ticket = tickets.next().ok_or("EDCA ticket")?;
        *admitted.get_mut(index).ok_or("EDCA index")? = ticket.index();
        submit(
            phase,
            role,
            body.case.category,
            ticket,
            buffers.next().ok_or("EDCA buffer")?,
            body.rate,
        )?;
    }
    let mut completed = [0; 3];
    for ticket in &mut completed {
        let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(TxCompleted::Single(tx))))) =
            phase.next_event().await
        else {
            return Err("EDCA terminal");
        };
        let parts = tx.into_parts(&mut access);
        *ticket = parts.ticket.index();
        let (buffer, _) = parts.payload.into_parts();
        if !matches!(buffer, ModelAnyBuffer::SmallSent(_)) {
            return Err("EDCA returned an unpublished or wrong-class buffer");
        }
    }
    Ok(contract::Trace {
        draws: body
            .air
            .contentions()
            .into_iter()
            .map(|draw| Draw {
                role: draw.role,
                category: draw.access_category,
                parameters: draw.parameters,
                window_slots: draw.window_slots,
                backoff_slots: draw.backoff_slots,
            })
            .collect(),
        installed: [
            body.air.edca(VifRole::Station),
            body.air.edca(VifRole::AccessPoint),
        ],
        admitted,
        completed,
        held_draws: None,
    })
}
impl PortBody<ModelBackend> for Exercise {
    type Output = Result<EdcaTrace, &'static str>;
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, ModelBackend, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend, Self::Output> {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(failed) => return Ended::fail_stopped(failed.fail_stop(), Err("EDCA enable")),
        };
        let mut result = exchange(&mut owner.phase(), resources, &self).await;
        let disabled = owner
            .disable(&mut |_| {
                result = Err("extra EDCA terminal");
                core::ops::ControlFlow::Continue(())
            })
            .await;
        match disabled {
            Ok(owner) => Ended::uninstall(owner, (), result),
            Err(failed) => Ended::fail_stopped(failed.fail_stop(), Err("EDCA disable")),
        }
    }
}
