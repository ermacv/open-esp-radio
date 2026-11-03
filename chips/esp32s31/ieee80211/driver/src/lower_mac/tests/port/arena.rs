//! The station's data arenas through the S31 port: lent by the station
//! phase, served in the driver's turn, ended by their arena terminal and
//! settled through the same phase; the gate refuses them, a single of the
//! same access category waits behind the arena on the air, disable
//! finishes a published arena, and retirement takes the chains back only
//! from idle, empty arenas.
use super::*;
use crate::mac::tx::TxBacking;
use oer_protocols_ieee80211_contracts_port::arena::{
    Arena, ArenaFormat, ArenaPublication, AttemptEnd, PublishError, PublishRefused, TxArena,
};
use oer_protocols_ieee80211_wire::phy::{HtMcs, HtRate, PpduBandwidth};
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;

/// How a scenario uses the arenas.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scenario {
    /// Three frames go as one A-MPDU; a BlockAck ends it.
    Aggregate,
    /// A closed gate refuses the publication; the frames are withdrawn.
    ClosedGate,
    /// An arena single holds the best-effort queue; an ordinary MPDU of
    /// the same category waits until the arena's end returns it.
    SingleWaits,
    /// Disable finishes a published arena whose completion never comes.
    Disable,
    /// One frame stays in an arena when the port stops.
    LeftBehind,
    /// A single MPDU under a plan of three transmissions: no ACK answers
    /// its first, one its second.
    SingleRetries,
}

/// What a scenario saw.
#[derive(Debug, Default)]
struct Seen {
    /// The arena terminal's end, by name.
    end: Option<&'static str>,
    /// The transmissions the arena's end reports.
    transmissions: Option<u16>,
    /// Whether each released memory carries the Retry bit, in order.
    retried: Vec<bool>,
    /// The tags of the memories that came back, in order.
    returned: Vec<u8>,
    /// Publications when the ordinary MPDU was submitted and when the
    /// arena ended.
    publications: Vec<usize>,
    /// Terminals in the order they arrived.
    order: Vec<&'static str>,
    refused: Option<PublishRefused>,
    /// The disable sink's arena terminals.
    disabled: usize,
}

struct ArenaBody {
    scenario: Scenario,
    observed: Rc<RefCell<Observed>>,
    timer: TestTimer,
    seen: Rc<RefCell<Seen>>,
}

/// A QoS Data MPDU of 30 octets to the BSS whose sequence number is `tag`.
fn write_tagged(tag: u8, region: &mut [u8]) -> Option<u16> {
    let frame = region.get_mut(..30)?;
    frame.fill(0);
    frame[..26].copy_from_slice(&crate::lower_mac::tests::data_frame(BSSID));
    frame[22..24].copy_from_slice(&(u16::from(tag) << 4).to_le_bytes());
    Some(30)
}

/// Whether the MPDU in `memory` carries the Retry bit.
fn retry_of(memory: &mut TxBacking<512>) -> bool {
    use oer_portable_memory::StableDmaBacking as _;
    let bytes = memory.stable_dma_region().into_mut_slice();
    oer_protocols_ieee80211_wire::frame::FrameControl::new(u16::from_le_bytes([bytes[8], bytes[9]]))
        .is_some_and(|control| control.retry())
}

fn tag_of(memory: &mut TxBacking<512>) -> u8 {
    use oer_portable_memory::StableDmaBacking as _;
    let bytes = memory.stable_dma_region().into_mut_slice();
    let control = u16::from_le_bytes([bytes[8 + 22], bytes[8 + 23]]);
    (control >> 4) as u8
}

fn memory() -> TxBacking<512> {
    crate::mac::tx::test_support::model_backing().expect("model memory")
}

fn publication<'p, 'id>(
    arena: Arena,
    format: ArenaFormat<'p, 'id, Caps>,
) -> ArenaPublication<'p, 'id, Caps> {
    ArenaPublication {
        arena,
        format,
        access_category: WmmAccessCategory::BestEffort,
        protection: Protection::None,
        encryption: MpduProtection::Open,
        power: SupportedPower::new(TxPower::Calibrated).expect("power"),
    }
}

impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for ArenaBody {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        resources: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, Self::Output>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let Ok(mut owner) = owner.enable().await else {
            panic!("enable")
        };
        let mut resources = resources;
        let seen = &self.seen;
        {
            let mut phase = owner.phase();
            let permission = resources
                .station_commands
                .configure
                .with_context(Tag(42, None));
            phase.station().configure(
                permission,
                StationReceive::Bss {
                    bssid: IndividualAddress::new(BSSID).expect("BSSID"),
                    filter: SupportedFilter::new(ReceiveFilter::BSS_MEMBER).expect("filter"),
                },
            );
            let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(_)))) =
                phase.next_event().await
            else {
                panic!("configuration terminal")
            };
            let single = phase
                .rate(PhyRate::Legacy(LegacyRate::Ofdm24M))
                .expect("a single rate");
            let ht = phase
                .rate(PhyRate::Ht(
                    HtRate::new(HtMcs::new(7).expect("MCS"), PpduBandwidth::Mhz20, false)
                        .expect("HT rate"),
                ))
                .expect("an HT rate");
            let aggregate = phase
                .station()
                .ampdu_rate(ht)
                .unwrap_or_else(|_| panic!("an aggregate rate"));
            let [first, second] = phase.station().data_arena().arenas().expect("the arenas");
            assert!(
                phase.station().data_arena().arenas().is_none(),
                "the rights are minted once"
            );
            let tags: &[u8] = match self.scenario {
                Scenario::Aggregate | Scenario::ClosedGate => &[10, 11, 12],
                _ => &[10],
            };
            for &tag in tags {
                phase
                    .station()
                    .data_arena()
                    .intake(&first, memory(), |region| write_tagged(tag, region))
                    .unwrap_or_else(|_| panic!("intake"));
            }
            if self.scenario == Scenario::ClosedGate {
                phase.close_gate(resources.gate);
                let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GateClosed(gate)))) =
                    phase.next_event().await
                else {
                    panic!("the gate closes")
                };
                let refused = phase.station().data_arena().publish(
                    publication(first, ArenaFormat::Single(once(single))),
                    self.timer.now(),
                );
                let Err(PublishError::Refused {
                    publication: refused,
                    reason,
                }) = refused
                else {
                    panic!("a closed gate publishes nothing")
                };
                seen.borrow_mut().refused = Some(reason);
                resources.gate = phase.open_gate(gate);
                let mut returned = Vec::new();
                phase
                    .station()
                    .data_arena()
                    .withdraw(&refused.arena, |mut memory| {
                        returned.push(tag_of(&mut memory))
                    })
                    .unwrap_or_else(|_| panic!("withdrawn"));
                seen.borrow_mut().returned = returned;
            } else if self.scenario == Scenario::LeftBehind {
                // The frame stays in the arena: the port cannot take the
                // chains back.
                drop(second);
            } else {
                let format = match self.scenario {
                    Scenario::Aggregate => ArenaFormat::Aggregate {
                        rate: aggregate,
                        spacing: MpduStartSpacing::new(0).expect("spacing"),
                    },
                    Scenario::SingleRetries => ArenaFormat::Single(RetryPlan::new(RetryStage {
                        rate: single,
                        transmissions: core::num::NonZeroU8::new(3).expect("plan"),
                        protection: Protection::None,
                    })),
                    _ => ArenaFormat::Single(once(single)),
                };
                match self.scenario {
                    Scenario::SingleWaits => self.observed.borrow_mut().hold_completion = true,
                    // No completion comes: the hardware's TX-timeout edge
                    // starts the abort that ends the arena.
                    Scenario::Disable => self.observed.borrow_mut().timeout_edge = true,
                    _ => {}
                }
                phase
                    .station()
                    .data_arena()
                    .publish(publication(first, format), self.timer.now())
                    .unwrap_or_else(|_| panic!("published"));
                if self.scenario == Scenario::SingleWaits {
                    let [buffer, ..] = resources.buffers;
                    let [ticket, ..] = resources.tickets;
                    submit_mpdu(&mut phase, buffer, ticket, None, None);
                    for _ in 0..8 {
                        barriers::yield_once().await;
                    }
                    seen.borrow_mut()
                        .publications
                        .push(self.observed.borrow().publications);
                    self.observed.borrow_mut().hold_completion = false;
                }
                if self.scenario != Scenario::Disable {
                    let mut ended = None;
                    let wanted = if self.scenario == Scenario::SingleWaits {
                        2
                    } else {
                        1
                    };
                    while ended.is_none() || seen.borrow().order.len() < wanted {
                        match phase.next_event().await {
                            Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                                TxCompleted::Arena(done),
                            )))) => {
                                let mut seen = seen.borrow_mut();
                                seen.order.push("arena");
                                seen.publications.push(self.observed.borrow().publications);
                                seen.transmissions =
                                    done.end().air().map(|air| air.transmissions.get());
                                seen.end = Some(match done.end() {
                                    AttemptEnd::Answered { .. } => "answered",
                                    AttemptEnd::Acked { .. } => "acked",
                                    _ => "other",
                                });
                                ended = Some(done);
                            }
                            Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                                TxCompleted::Single(completed),
                            )))) => {
                                let _ = completed.into_parts(&mut resources.terminal_access);
                                seen.borrow_mut().order.push("single");
                            }
                            Ok(Ok(_)) => {}
                            _ => panic!("the arena's terminal"),
                        }
                    }
                    let done = ended.expect("the arena ended");
                    let mut returned = Vec::new();
                    let mut retried = Vec::new();
                    let _first = phase
                        .station()
                        .data_arena()
                        .release(done, |mut memory| {
                            returned.push(tag_of(&mut memory));
                            retried.push(retry_of(&mut memory));
                        })
                        .unwrap_or_else(|_| panic!("released"));
                    seen.borrow_mut().returned = returned;
                    seen.borrow_mut().retried = retried;
                }
            }
        }
        // The station waits for nothing more: a published arena finishes
        // at disable, its watchdog's abort settling in virtual time.
        let timer = self.timer.clone();
        let mut sink = |terminal| {
            if let LowerMacTerminal::TxCompleted(TxCompleted::Arena(_)) = terminal {
                seen.borrow_mut().disabled += 1;
            }
            core::ops::ControlFlow::Continue(())
        };
        let mut disable = pin!(owner.disable(None, &mut sink));
        let disabled = core::future::poll_fn(|cx| {
            let ready = disable.as_mut().poll(cx);
            if ready.is_pending() {
                // Short of the watchdog: the abort settles first.
                let _ = timer.0.advance(crate::ordinary_tx::TX_ABORT_SETTLE);
                cx.waker().wake_by_ref();
            }
            ready
        })
        .await;
        let Ok(owner) = disabled else {
            panic!("disable")
        };
        radio::Ended::uninstall(owner, (), ())
    }
}

/// Run `scenario` and whether the stopped port reinstalls.
fn exercise(scenario: Scenario, statuses: Vec<(u8, u8)>) -> (Seen, Rc<RefCell<Observed>>, bool) {
    let (parts, observed, timer) = installed(statuses);
    if scenario == Scenario::Aggregate {
        observed.borrow_mut().block_ack = Some((10, 0b111, true));
    }
    let seen = Rc::new(RefCell::new(Seen::default()));
    let body = ArenaBody {
        scenario,
        observed: observed.clone(),
        timer,
        seen: seen.clone(),
    };
    let reinstalled = block_on(radio::with_local_slot(async |claim| {
        let mut slot = claim.place();
        let backend = Backend::new(parts).ok().expect("valid construction");
        let registered = slot.register(backend).ok().expect("fresh local slot");
        let end = radio::LocalExecution
            .scope(crate::port::Registration::guarded(registered), |frontend| {
                radio::run_port(frontend, body)
            })
            .await;
        let radio::PortEnd::Uninstalled { joined, .. } = end else {
            panic!(
                "clean uninstall, poisoned by {:?}",
                crate::port::FIRST_POISON.with(Cell::get)
            )
        };
        let stopped = slot.retire(joined).ok().expect("the joined slot retires");
        stopped.reinstall(18).is_ok()
    }));
    let seen = seen.take();
    (seen, observed, reinstalled)
}

#[test]
fn an_arena_goes_through_the_phase_ends_as_its_terminal_and_settles_through_it() {
    let (seen, observed, reinstalled) = exercise(Scenario::Aggregate, std::vec![(0, 0)]);
    assert_eq!(seen.end, Some("answered"));
    assert_eq!(seen.returned, [10, 11, 12], "every memory back in order");
    assert_eq!(observed.borrow().publications, 1, "one PPDU");
    assert_eq!(seen.disabled, 0);
    assert!(reinstalled, "idle, empty arenas give their chains back");
}

#[test]
fn a_closed_gate_refuses_an_arena_and_its_frames_are_withdrawn() {
    let (seen, observed, reinstalled) = exercise(Scenario::ClosedGate, Vec::new());
    assert_eq!(seen.refused, Some(PublishRefused::Busy));
    assert_eq!(seen.returned, [10, 11, 12]);
    assert_eq!(observed.borrow().publications, 0);
    assert!(reinstalled);
}

#[test]
fn a_single_waits_while_an_arena_holds_its_access_category() {
    let (seen, observed, reinstalled) = exercise(Scenario::SingleWaits, std::vec![(0, 0), (0, 0)]);
    assert_eq!(
        seen.publications.first(),
        Some(&1),
        "the MPDU waits behind the arena on the air"
    );
    assert_eq!(seen.end, Some("acked"));
    assert_eq!(seen.order, ["arena", "single"]);
    assert_eq!(observed.borrow().publications, 2);
    assert_eq!(seen.returned, [10]);
    assert!(reinstalled);
}

#[test]
fn a_single_arena_runs_its_retry_plan_until_an_ack() {
    // AckTimeout, then success: the frame goes again with its Retry bit.
    let (seen, observed, reinstalled) =
        exercise(Scenario::SingleRetries, std::vec![(5, 0), (0, 0)]);
    assert_eq!(seen.end, Some("acked"));
    assert_eq!(
        seen.transmissions,
        Some(2),
        "both transmissions reached the air"
    );
    assert_eq!(observed.borrow().publications, 2, "the plan sent it again");
    assert_eq!(seen.returned, [10]);
    assert_eq!(
        seen.retried,
        [true],
        "the retransmission carried the Retry bit"
    );
    assert!(reinstalled);
}

#[test]
fn disable_finishes_a_published_arena_and_hands_its_end_to_the_sink() {
    let (seen, observed, reinstalled) = exercise(Scenario::Disable, std::vec![(0, 0)]);
    assert_eq!(observed.borrow().publications, 1);
    assert_eq!(seen.disabled, 1, "the arena's end reaches the disable sink");
    assert!(
        !reinstalled,
        "an unsettled arena keeps its chains, and the port refuses reinstall"
    );
}

#[test]
fn a_frame_left_in_an_arena_keeps_the_chains_from_a_new_installation() {
    let (_, observed, reinstalled) = exercise(Scenario::LeftBehind, Vec::new());
    assert_eq!(observed.borrow().publications, 0);
    assert!(!reinstalled);
}

/// A single MPDU's plan of one transmission at `rate`.
fn once<R>(rate: R) -> oer_protocols_ieee80211_contracts_port::RetryPlan<R> {
    oer_protocols_ieee80211_contracts_port::RetryPlan::new(
        oer_protocols_ieee80211_contracts_port::RetryStage {
            rate,
            transmissions: core::num::NonZeroU8::MIN,
            protection: oer_protocols_ieee80211_contracts_port::Protection::None,
        },
    )
}
