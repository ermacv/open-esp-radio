//! The station's own frames: the actor numbers a frame the link numbers,
//! rates it, prepares it in the station's pool and submits it. A frame that
//! does not go comes back to the lifecycle with its ticket and buffer.

use super::turn::Failed;
use super::*;
use oer_protocols_ieee80211_contracts_port::{
    Attempt, Protection, Station, SubmitRefused, TxProtection, TxSource,
};
use oer_protocols_ieee80211_core_roles_sta::machine::calibrated_power;

impl<'id, D: DatapathPort<'id>, S, O> Actor<'id, D, S, O>
where
    S: RateSelection<
            ActorRxMeta<'id, D>,
            <Ty<'id, D> as PhaseTypes<'id>>::AckEvidence,
            <Ty<'id, D> as PhaseTypes<'id>>::BlockAckEvidence,
        >,
    O: ActorEvents,
{
    /// Send one station frame: a frame the link numbers takes its Sequence
    /// Number from the link's space and, protected, its packet number from
    /// the link's pairwise key once its rate is known. A refusal returns the
    /// frame's ticket and buffer to the lifecycle.
    pub(super) fn station_tx<const TO: usize, const FROM: usize, const RETURNS: usize>(
        &mut self,
        transmit: StationTx<'id, D>,
        lifecycle: &LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
    ) -> Result<(), Failed<'id, D>> {
        let StationTx {
            ticket,
            mut buffer,
            mut frame,
            class,
            plan,
            access_category,
            number,
        } = transmit;
        let erase = <Ty<'id, D> as TxBufferPool<'id, D::Small>>::erase;
        if !self.radio.running() {
            // The radio stops: the frame does not go.
            return self.give(
                lifecycle,
                Returned::Station {
                    ticket,
                    buffer: erase(buffer),
                    reason: TxRefusal::LinkChanged,
                },
            );
        }
        let protected = match number {
            FrameNumber::Open => false,
            FrameNumber::Link { protected, .. } => {
                let numbered = match self.link.as_mut() {
                    Some(link) => {
                        link.numbers_mut()
                            .number_frame(number, &mut frame, &mut buffer, plan)
                    }
                    None => Err(TxRefusal::LinkChanged),
                };
                if let Err(reason) = numbered {
                    return self.give(
                        lifecycle,
                        Returned::Station {
                            ticket,
                            buffer: erase(buffer),
                            reason,
                        },
                    );
                }
                protected
            }
        };
        let admitted = {
            let Self {
                port, rates, link, ..
            } = &*self;
            let rated = frame.rated(plan, rates, |rate| port.rate(rate).map(|rate| rate.phase()));
            // The packet number follows the rate: a frame without one draws
            // none.
            let protection = match (rated.is_some(), protected) {
                (false, _) => Err(TxRefusal::NoRate),
                (true, false) => Ok(TxProtection::Open),
                (true, true) => match link.as_ref().and_then(|link| link.numbers().key()) {
                    Some(key) => key
                        .number()
                        .map(TxProtection::Ccmp)
                        .ok_or(TxRefusal::PacketNumberExhausted),
                    // A protected frame of an open link does not go.
                    None => Err(TxRefusal::LinkChanged),
                },
            };
            match (rated, protection) {
                (Some(rated), Ok(protection)) => {
                    let source = TxSource::Fresh {
                        buffer,
                        frame: rated,
                        body: None,
                        protection,
                    };
                    let prepared = match source.check() {
                        Ok(source) => <Ty<'id, D> as TxBufferPool<'id, D::Small>>::prepare::<
                            Station,
                            D::Caps,
                        >(source)
                        .map_err(|refused| {
                            (TxRefusal::Preparation(refused.reason), refused.operation)
                        }),
                        Err(refused) => Err((TxRefusal::Layout(refused.reason), refused.operation)),
                    };
                    match prepared {
                        Ok(prepared) => Ok(port.admit(
                            Attempt {
                                context: (),
                                ticket,
                                source: prepared,
                                access_category,
                                protection: Protection::None,
                                power: calibrated_power::<D::Caps>(),
                                coex_operation: class.coex_operation(),
                            }
                            .admit(),
                        )),
                        Err((reason, operation)) => {
                            let buffer = match operation {
                                TxSource::Fresh { buffer, body, .. } => {
                                    if let Some(body) = body {
                                        match body {}
                                    }
                                    erase(buffer)
                                }
                                TxSource::Retransmission { mpdu, .. } => {
                                    <Ty<'id, D> as TxBufferPool<'id, D::Small>>::erase_sent(mpdu)
                                }
                            };
                            Err((ticket, buffer, reason))
                        }
                    }
                }
                (_, Err(reason)) => Err((ticket, erase(buffer), reason)),
                (None, Ok(_)) => Err((ticket, erase(buffer), TxRefusal::NoRate)),
            }
        };
        let admitted = match admitted {
            Ok(admitted) => admitted,
            Err((ticket, buffer, reason)) => {
                return self.give(
                    lifecycle,
                    Returned::Station {
                        ticket,
                        buffer,
                        reason,
                    },
                );
            }
        };
        match self.port.submit(admitted, self.now) {
            Ok(()) => Ok(()),
            Err(SubmitRefused { attempt, .. }) => {
                // The running port refused it: its owners stay with the
                // actor, sealed, and the run ends.
                self.retained.admitted = Some(attempt);
                Err(ActorFault::Contract(ActorViolation::SubmitRefused))
            }
        }
    }
}
