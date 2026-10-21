//! VIF records and draws captured from the installed policy at publication.
use super::*;
use oer_hil_protocol::lower_mac::{EdcaDraw, EdcaHeader, EdcaTrace, Trace, edca_parameters};
use oer_ieee80211_port::conformance::{
    Execution,
    edca::{self as contract, Case, Scenario},
};
use oer_ieee80211_wire::qos::WmmAccessCategory;
type Ticket<'id, Id> = radio::Slot<'id, Tickets, <Family<Id> as PhaseTypes<'id>>::TicketPath>;
fn submit<'p, 'id: 'p, Id: radio::SlotIdentity + 'id>(
    phase: &mut Esp32s31Phase<'p, 'id, Id, Device, TX>,
    role: VifRole,
    category: WmmAccessCategory,
    ticket: Ticket<'id, Id>,
    mut buffer: Buffer<'id, Id, Device, TX>,
    peer: Peer,
) -> Result<(), &'static str> {
    buffer.bytes_mut().fill(0);
    let rate = phase.rate(PhyRate::Legacy(peer.rate)).ok_or("EDCA rate")?;
    let sender = match role {
        VifRole::Station => phase.station().address(),
        VifRole::AccessPoint => phase.access_point().address(),
    };
    let header = HeaderSpec::new(
        FrameControl::new(0x0040).ok_or("EDCA FC")?,
        0,
        peer.address,
        sender.get(),
        peer.address.get(),
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
            let source = <Family<Id> as TxBufferPool<BufferClass<TX>>>::prepare::<$role, Caps>(
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
                        context: Context::Plain,
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
fn captured(admitted: [u8; 3], completed: [u8; 3]) -> Result<EdcaTrace, &'static str> {
    let mut trace = EdcaTrace {
        draws: [None; 4],
        installed: [[None; 4]; 2],
        headers: [None; 2],
        admitted,
        completed,
        held_draws: None,
    };
    let mut count = 0;
    for record in TRACE.records() {
        if let Some(header) = record.decode::<port::trace::EdcaTable>() {
            if header.role == VifRole::Station {
                trace.installed = [[None; 4]; 2];
                trace.headers = [None; 2];
            }
            *trace
                .headers
                .get_mut(header.role as usize)
                .ok_or("EDCA header role")? = Some(EdcaHeader {
                count: header.count,
                uapsd: header.uapsd,
            });
        } else if let Some(record) = record.decode::<port::trace::EdcaRecord>() {
            *trace
                .installed
                .get_mut(record.role as usize)
                .and_then(|row| row.get_mut(record.category as usize))
                .ok_or("EDCA record coordinate")? = edca_parameters(record.parameters);
        } else if let Some(draw) = record.decode::<port::trace::EdcaDraw>() {
            let parameters = *trace
                .installed
                .get(draw.role as usize)
                .and_then(|row| row.get(draw.category as usize))
                .ok_or("EDCA draw coordinate")?;
            *trace.draws.get_mut(count).ok_or("EDCA draw capacity")? = Some(EdcaDraw {
                role: draw.role as u8,
                category: draw.category as u8,
                parameters,
                window: 1_u16
                    .checked_shl(u32::from(draw.exponent))
                    .and_then(|window| window.checked_sub(1))
                    .ok_or("EDCA exponent")?,
                backoff: draw.backoff,
            });
            count = count.checked_add(1).ok_or("EDCA draw count")?;
        }
    }
    Ok(trace)
}
async fn exchange<'p, 'id: 'p, Id: radio::SlotIdentity + 'id>(
    phase: &mut Esp32s31Phase<'p, 'id, Id, Device, TX>,
    resources: <Backend<Id> as PortBackend<Id>>::Resources<'id>,
    peer: Peer,
    case: Case,
) -> Result<Trace, &'static str> {
    if case.execution() != Execution::Hardware {
        return Err("physical EDCA workload required");
    }
    if case.scenario != Scenario::Defaults {
        phase.station().set_edca(
            SupportedEdca::new(contract::parameters(VifRole::Station, false))
                .ok_or("station EDCA")?,
        );
        phase.access_point().set_edca(
            SupportedEdca::new(contract::parameters(VifRole::AccessPoint, false))
                .ok_or("AP EDCA")?,
        );
    }
    let [first, second, last, ..] = resources.tickets;
    let [a, b, c, ..] = resources.buffers;
    let admitted = [first.index(), second.index(), last.index()];
    oer_trace::start(port::trace::Publication::CHANNEL.mask());
    for (ticket, buffer, role) in [
        (first, a, VifRole::Station),
        (second, b, VifRole::AccessPoint),
        (last, c, VifRole::Station),
    ] {
        submit(phase, role, case.category, ticket, buffer, peer)?;
    }
    let mut access = resources.terminal_access;
    let mut completed = [0; 3];
    for ticket in &mut completed {
        loop {
            match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                    TxCompleted::Single(tx),
                )))) => {
                    let parts = tx.into_parts(&mut access);
                    *ticket = parts.ticket.index();
                    let (buffer, _) = parts.payload.into_parts();
                    if !matches!(buffer, AnyBuffer::Sent(_)) {
                        return Err("EDCA returned an unpublished buffer");
                    }
                    break;
                }
                Ok(Ok(LowerMacEvent::Received { frame, .. })) => phase.recycle(frame),
                Ok(Ok(LowerMacEvent::Tbtt(_))) => {}
                _ => return Err("EDCA terminal"),
            }
        }
    }
    captured(admitted, completed).map(Trace::Edca)
}
pub(super) async fn run<'p, 'id, Id: radio::SlotIdentity + 'id>(
    mut owner: Owner<'p, 'id, Id, Backend<Id>, radio::Enabled>,
    resources: <Backend<Id> as PortBackend<Id>>::Resources<'id>,
    peer: Peer,
    case: Case,
) -> Ended<Id, Backend<Id>, Result<Trace, &'static str>> {
    let mut result = exchange(&mut owner.phase(), resources, peer, case).await;
    let disabled = owner
        .disable(&mut |_| {
            result = Err("extra EDCA terminal");
            core::ops::ControlFlow::Continue(())
        })
        .await;
    match disabled {
        Ok(owner) => Ended::uninstall(owner, (), result),
        Err(failed) => Ended::fail_stop(failed.fail_stop(), Err("EDCA disable")),
    }
}
