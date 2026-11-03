//! The S31 TX arenas over the core's retained A-MPDU chains: the shared
//! arena suite and the S31's own edges (descriptor words bound at intake,
//! the Retry bit in the buffer, the key right held until the last frame
//! leaves). The port's phase lends the rates; a lower-MAC core of its own,
//! over the host device model, serves each case. The device model supplies
//! what real air would: ACK, BlockAck, CTS-timeout and invalid statuses.
use super::*;
use crate::lower_mac::arena::{ArenaPort, Esp32s31TxArena};
use crate::lower_mac::tests as core_fixture;
use crate::lower_mac::{KeyHandle, TransmitKeyScope};
use crate::tx::WifiTxWake;
use core::marker::PhantomData;
use oer_chips_esp32s31_hal::types::MacTxCompletionObservation;
use oer_portable_memory::StableDmaBacking as _;
use oer_protocols_ieee80211_contracts_port::arena::{
    Arena, ArenaFormat, ArenaId, ArenaPublication, AttemptEnd, FrameMask, PublishError,
    PublishRefused, TxArena,
};
use oer_protocols_ieee80211_testing_conformance::arena::{
    self as suite, Answer, Fixture, Published,
};
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;
use oer_protocols_radio_port::Root;

type Path<'core> = Root<radio::LocalId<'core>>;
type Port<'a, 'core> = ArenaPort<
    'a,
    'core,
    'static,
    core_fixture::Power,
    fn() -> u32,
    oer_portable_time_virtual::SkipClock,
    core_fixture::RadioNine,
    core_fixture::Hardware,
    core_fixture::Events<'core>,
    (),
    core_fixture::Backing,
    Caps,
    512,
    Path<'core>,
    4,
    { core_fixture::SUBFRAMES },
>;
type Arenas<'core> = Esp32s31TxArena<
    'core,
    'static,
    core_fixture::Backing,
    Path<'core>,
    { core_fixture::SUBFRAMES },
>;

/// A body over the port phase's rates for a single MPDU (OFDM 24 Mb/s) and
/// an A-MPDU (HT20 MCS 0, whose vendor ceiling is 6 490 octets).
trait WithRates {
    fn run<'p, 'id: 'p>(
        self,
        single: PhaseRate<'p, 'id, Caps>,
        aggregate: AmpduRate<'p, 'id, Caps>,
    );
}

struct RatesBody<W>(W);

impl<Id: radio::SlotIdentity, W: WithRates> PortBody<Id, Backend<Id>> for RatesBody<W> {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        _: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, ()>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let Ok(mut owner) = owner.enable().await else {
            panic!("enable")
        };
        {
            let mut phase = owner.phase();
            let single = phase
                .rate(PhyRate::Legacy(LegacyRate::Ofdm24M))
                .expect("a single rate");
            let ht = phase
                .rate(PhyRate::Ht(
                    oer_protocols_ieee80211_wire::phy::HtRate::new(
                        oer_protocols_ieee80211_wire::phy::HtMcs::new(0).expect("MCS"),
                        oer_protocols_ieee80211_wire::phy::PpduBandwidth::Mhz20,
                        false,
                    )
                    .expect("HT rate"),
                ))
                .expect("an HT rate");
            let aggregate = phase
                .station()
                .ampdu_rate(ht)
                .unwrap_or_else(|_| panic!("an aggregate rate"));
            self.0.run(single, aggregate);
        }
        let Ok(owner) = owner
            .disable(None, &mut |_| core::ops::ControlFlow::Continue(()))
            .await
        else {
            panic!("disable")
        };
        radio::Ended::uninstall(owner, (), ())
    }
}

fn with_rates(run: impl WithRates) {
    let (parts, _, _) = installed(Vec::new());
    let Report::Uninstalled { .. } = block_on(run_body(parts, RatesBody(run))) else {
        panic!("clean uninstall")
    };
}

/// A QoS Data MPDU of 30 octets to the BSS whose sequence number is `tag`.
fn write_tagged(tag: u8, region: &mut [u8]) -> Option<u16> {
    let frame = region.get_mut(..30)?;
    frame.fill(0);
    frame[..26].copy_from_slice(&core_fixture::data_frame(core_fixture::BSSID));
    frame[22..24].copy_from_slice(&(u16::from(tag) << 4).to_le_bytes());
    Some(30)
}

fn tag_of(frame: &[u8]) -> Option<u8> {
    let control = u16::from_le_bytes([*frame.get(22)?, *frame.get(23)?]);
    u8::try_from(control >> 4).ok()
}

fn retried(frame: &[u8]) -> bool {
    frame.get(1).is_some_and(|flags| flags & 0x08 != 0)
}

/// The arenas over two fresh chains, with two of the core's tickets.
fn arenas<'core>(core: &mut core_fixture::Core<'core>) -> Arenas<'core> {
    let tickets = [core.take_ticket(), core.take_ticket()];
    Esp32s31TxArena::new(
        [core_fixture::ampdu_owner(), core_fixture::ampdu_owner()],
        tickets,
    )
    .unwrap_or_else(|_| panic!("idle chains"))
}

/// One step's view of `arenas` over `core`.
fn port<'a, 'core>(
    arenas: &'a mut Arenas<'core>,
    core: &'a mut core_fixture::Core<'core>,
    hardware: &'a mut core_fixture::Hardware,
    events: &'a mut core_fixture::Events<'core>,
    pairwise: Option<&'a KeyHandle<'core>>,
) -> Port<'a, 'core> {
    let core_fixture::Core { inner, free, .. } = core;
    Port {
        arenas,
        core: &mut **inner,
        hardware,
        queues: free,
        role: VifRole::Station,
        pairwise,
        sink: events,
        observations: Box::leak(Box::new(())),
        caps: PhantomData,
    }
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
        power: SupportedPower::new(TxPower::Calibrated).expect("calibrated power"),
    }
}

fn aggregate_format<'p, 'id>(rate: AmpduRate<'p, 'id, Caps>) -> ArenaFormat<'p, 'id, Caps> {
    ArenaFormat::Aggregate {
        rate,
        spacing: MpduStartSpacing::new(0).expect("spacing"),
    }
}

/// The S31 arenas under the shared suite.
struct S31Fixture<'a, 'core, 'p, 'id> {
    port: Port<'a, 'core>,
    source: &'static core_fixture::Backings,
    single: PhaseRate<'p, 'id, Caps>,
    aggregate: AmpduRate<'p, 'id, Caps>,
    micros: u64,
    /// What the last publication carried, read once it went out.
    last: Option<(usize, Published)>,
}

impl S31Fixture<'_, '_, '_, '_> {
    /// Read what the publication on the air carries, once per publication.
    fn observe(&mut self) {
        let publications = self.port.hardware.publications();
        if self
            .last
            .as_ref()
            .is_some_and(|(seen, _)| *seen == publications)
        {
            return;
        }
        let Some((id, count, single)) = self.port.arenas.on_air_shape() else {
            return;
        };
        let Some(chain) = self.port.arenas.chain_mut(id) else {
            return;
        };
        let mut published = Published {
            tags: [0; oer_protocols_ieee80211_contracts_port::arena::ARENA_FRAMES as usize],
            count,
            retried: FrameMask::EMPTY,
            aggregate: !single,
        };
        for (position, index) in FrameMask::first(count)
            .unwrap_or(FrameMask::EMPTY)
            .into_iter()
            .enumerate()
        {
            let Some(frame) = chain.observed_frame(position) else {
                return;
            };
            published.tags[position] = tag_of(frame).unwrap_or(u8::MAX);
            if retried(frame) {
                published.retried.insert(index);
            }
        }
        self.last = Some((publications, published));
    }

    fn on_air_single(&self) -> bool {
        self.last
            .as_ref()
            .is_some_and(|(_, published)| !published.aggregate)
    }
}

impl<'a, 'core, 'p, 'id: 'p> Fixture<'p, 'id> for S31Fixture<'a, 'core, 'p, 'id> {
    type Port = Port<'a, 'core>;
    fn port(&mut self) -> &mut Port<'a, 'core> {
        self.observe();
        &mut self.port
    }
    fn memory(&mut self, tag: u8) -> Option<core_fixture::Backing> {
        let mut memory = self.source.backing()?;
        let region = memory.stable_dma_region().into_mut_slice();
        write_tagged(tag, region.get_mut(crate::ordinary_tx::TX_METADATA_SIZE..)?)?;
        Some(memory)
    }
    fn write(tag: u8, region: &mut [u8]) -> Option<u16> {
        write_tagged(tag, region)
    }
    fn tag(memory: &core_fixture::Backing) -> Option<u8> {
        tag_of(
            memory
                .as_slice()
                .get(crate::ordinary_tx::TX_METADATA_SIZE..)?,
        )
    }
    fn single(&self, arena: Arena) -> ArenaPublication<'p, 'id, Caps> {
        publication(arena, ArenaFormat::Single(self.single))
    }
    fn aggregate(&self, arena: Arena) -> ArenaPublication<'p, 'id, Caps> {
        publication(arena, aggregate_format(self.aggregate))
    }
    fn answer(&mut self, answer: Answer) {
        self.observe();
        let single = self.on_air_single();
        let hardware = &mut *self.port.hardware;
        let status = |code: u8, detail: u8| MacTxCompletionObservation::new_model(code, detail);
        let lost = |code: u8| core_fixture::block_ack_completion(code, 0, 0, false);
        match answer {
            Answer::BlockAck(received) => {
                let tags = self.last.as_ref().map(|(_, published)| published.tags);
                let start = tags.map_or(0, |tags| tags[0]);
                let bitmap = received.iter().fold(0_u64, |bitmap, index| {
                    let tag = tags.map_or(0, |tags| tags[usize::from(index.get())]);
                    bitmap | 1_u64 << tag.wrapping_sub(start)
                });
                hardware.block_ack_completion[core_fixture::BE] = Some(
                    core_fixture::block_ack_completion(0, u16::from(start), bitmap, true),
                );
            }
            Answer::Ack => hardware.completion[core_fixture::BE] = Some(status(0, 0)),
            Answer::NoResponse if single => {
                hardware.completion[core_fixture::BE] = Some(status(5, 0));
            }
            Answer::NoResponse => hardware.block_ack_completion[core_fixture::BE] = Some(lost(5)),
            Answer::ProtectionFailure if single => {
                hardware.completion[core_fixture::BE] = Some(status(2, 0));
            }
            Answer::ProtectionFailure => {
                hardware.block_ack_completion[core_fixture::BE] = Some(lost(2));
            }
            // Status 3 stops the vendor's completion dispatch.
            Answer::Fault if single => hardware.completion[core_fixture::BE] = Some(status(3, 0)),
            Answer::Fault => hardware.block_ack_completion[core_fixture::BE] = Some(lost(3)),
        }
    }
    fn events(&mut self) -> WifiTxWake {
        WifiTxWake::Interrupt {
            events: crate::mac::irq::EVENT_TX_COMPLETE,
        }
    }
    fn now(&mut self) -> Instant {
        self.micros += 1;
        Instant::from_micros(self.micros)
    }
    fn published(&self) -> Option<Published> {
        self.last.as_ref().map(|(_, published)| *published)
    }
}

#[test]
fn the_s31_arenas_pass_every_shared_arena_case() {
    struct Cases;
    impl WithRates for Cases {
        fn run<'p, 'id: 'p>(
            self,
            single: PhaseRate<'p, 'id, Caps>,
            aggregate: AmpduRate<'p, 'id, Caps>,
        ) {
            // The device model answers every case, the injected ones too.
            for case in suite::Case::ALL {
                core_fixture::installed(|pools| {
                    let mut hardware = core_fixture::Hardware::default();
                    let mut core = core_fixture::enabled(&mut hardware, pools);
                    let mut arenas = arenas(&mut core);
                    let mut events = core_fixture::Events::default();
                    let mut fixture = S31Fixture {
                        port: port(&mut arenas, &mut core, &mut hardware, &mut events, None),
                        source: core_fixture::backings(),
                        single,
                        aggregate,
                        micros: 0,
                        last: None,
                    };
                    assert_eq!(suite::exercise(&mut fixture, case), Ok(()), "{case:?}");
                });
            }
        }
    }
    with_rates(Cases);
}

#[test]
fn intake_binds_each_descriptor_word_once_and_a_retry_relinks_it_with_the_retry_bit() {
    struct Case;
    impl WithRates for Case {
        fn run<'p, 'id: 'p>(self, _: PhaseRate<'p, 'id, Caps>, rate: AmpduRate<'p, 'id, Caps>) {
            core_fixture::installed(|pools| {
                let mut hardware = core_fixture::Hardware::default();
                let mut core = core_fixture::enabled(&mut hardware, pools);
                let mut arenas = arenas(&mut core);
                let mut events = core_fixture::Events::default();
                let source = core_fixture::backings();
                let mut port = port(&mut arenas, &mut core, &mut hardware, &mut events, None);
                let [first, _] = port.arenas().expect("the arenas");
                for tag in [10, 11, 12] {
                    port.intake(&first, source.backing().expect("memory"), |region| {
                        write_tagged(tag, region)
                    })
                    .unwrap_or_else(|_| panic!("intake"));
                }
                let chain = port.arenas.chain_mut(ArenaId::First).expect("chain");
                let words: Vec<_> = (0..3)
                    .map(|at| chain.bound_word0(at).expect("bound"))
                    .collect();
                port.publish(
                    publication(first, aggregate_format(rate)),
                    Instant::from_micros(1),
                )
                .unwrap_or_else(|_| panic!("published"));
                port.hardware.block_ack_completion[core_fixture::BE] =
                    Some(core_fixture::block_ack_completion(0, 10, 0b010, true));
                let done = port
                    .service(
                        WifiTxWake::Interrupt {
                            events: crate::mac::irq::EVENT_TX_COMPLETE,
                        },
                        Instant::from_micros(2),
                    )
                    .ok()
                    .flatten()
                    .expect("the BlockAck ends it");
                assert!(matches!(done.end(), AttemptEnd::Answered { .. }));
                let mut returned = Vec::new();
                let first = port
                    .retain(done, FrameMask::from_bits(0b101).expect("mask"), |memory| {
                        returned.push(tag_of(&memory.as_slice()[8..]))
                    })
                    .unwrap_or_else(|_| panic!("retained"));
                assert_eq!(returned, [Some(11)]);
                let chain = port.arenas.chain_mut(ArenaId::First).expect("chain");
                // The kept frames' bound words moved with them, unchanged.
                assert_eq!(chain.bound_word0(0), Some(words[0]));
                assert_eq!(chain.bound_word0(1), Some(words[2]));
                for position in 0..2 {
                    let frame = chain.observed_frame(position).expect("kept frame");
                    assert!(retried(frame), "a missing frame goes again as a retry");
                }
                port.publish(
                    publication(first, aggregate_format(rate)),
                    Instant::from_micros(3),
                )
                .unwrap_or_else(|_| panic!("republished"));
                let chain = port.arenas.chain_mut(ArenaId::First).expect("chain");
                assert_eq!(chain.bound_word0(1), Some(words[2]));
                let tags: Vec<_> = (0..2)
                    .map(|position| chain.observed_frame(position).and_then(tag_of))
                    .collect();
                assert_eq!(tags, [Some(10), Some(12)]);
            });
        }
    }
    with_rates(Case);
}

#[test]
fn the_pairwise_key_stays_until_the_last_frame_it_encrypted_leaves() {
    struct Case;
    impl WithRates for Case {
        fn run<'p, 'id: 'p>(self, _: PhaseRate<'p, 'id, Caps>, rate: AmpduRate<'p, 'id, Caps>) {
            core_fixture::installed(|pools| {
                let mut hardware = core_fixture::Hardware::default();
                let mut core = core_fixture::enabled(&mut hardware, pools);
                let key = core
                    .install_key(
                        &mut hardware,
                        TransmitKeyScope::StationPairwise {
                            peer: core_fixture::BSSID,
                        },
                        &[7; 16],
                    )
                    .expect("installed key");
                let mut arenas = arenas(&mut core);
                let mut events = core_fixture::Events::default();
                let source = core_fixture::backings();
                let protected = |tag| {
                    move |region: &mut [u8]| {
                        let length = write_tagged(tag, region)?;
                        region[1] |= 0x40;
                        Some(length)
                    }
                };
                let encrypted = |arena| ArenaPublication {
                    encryption: MpduProtection::Ccmp,
                    ..publication(arena, aggregate_format(rate))
                };
                let wake = WifiTxWake::Interrupt {
                    events: crate::mac::irq::EVENT_TX_COMPLETE,
                };
                let first = {
                    let mut port = port(
                        &mut arenas,
                        &mut core,
                        &mut hardware,
                        &mut events,
                        Some(&key),
                    );
                    let [first, _] = port.arenas().expect("the arenas");
                    for tag in [20, 21] {
                        port.intake(&first, source.backing().expect("memory"), protected(tag))
                            .unwrap_or_else(|_| panic!("intake"));
                    }
                    // An open publication of protected frames is refused.
                    let Err(PublishError::Refused {
                        publication: refused,
                        reason: PublishRefused::Protection,
                    }) = port.publish(
                        publication(first, aggregate_format(rate)),
                        Instant::from_micros(1),
                    )
                    else {
                        panic!("the encryption contradicts the frames");
                    };
                    port.publish(encrypted(refused.arena), Instant::from_micros(2))
                        .unwrap_or_else(|_| panic!("published"));
                    port.hardware.block_ack_completion[core_fixture::BE] =
                        Some(core_fixture::block_ack_completion(0, 20, 0b01, true));
                    let done = port
                        .service(wake, Instant::from_micros(3))
                        .ok()
                        .flatten()
                        .expect("ended");
                    port.retain(done, FrameMask::from_bits(0b10).expect("mask"), drop)
                        .unwrap_or_else(|_| panic!("retained"))
                };
                assert!(
                    arenas.holds_key(ArenaId::First),
                    "a retained frame keeps it"
                );
                core.inner
                    .remove_checked_key(&mut hardware, key, &mut events);
                assert!(events.removed.is_empty(), "the removal waits for the arena");
                let mut port = port(&mut arenas, &mut core, &mut hardware, &mut events, None);
                port.publish(encrypted(first), Instant::from_micros(4))
                    .unwrap_or_else(|_| panic!("the retained frame goes under its key"));
                port.hardware.block_ack_completion[core_fixture::BE] =
                    Some(core_fixture::block_ack_completion(0, 21, 0b1, true));
                let done = port
                    .service(wake, Instant::from_micros(5))
                    .ok()
                    .flatten()
                    .expect("ended");
                let first = port
                    .release(done, drop)
                    .unwrap_or_else(|_| panic!("released"));
                port.withdraw(&first, drop)
                    .unwrap_or_else(|_| panic!("empty"));
                assert!(!arenas.holds_key(ArenaId::First));
                assert_eq!(events.removed.len(), 1, "the last use finished the removal");
                assert!(arenas.into_parts().is_ok(), "both tickets are back");
            });
        }
    }
    with_rates(Case);
}

#[test]
fn an_aggregate_past_its_rate_limit_is_refused_and_a_single_carries_position_0() {
    struct Case;
    impl WithRates for Case {
        fn run<'p, 'id: 'p>(
            self,
            single: PhaseRate<'p, 'id, Caps>,
            rate: AmpduRate<'p, 'id, Caps>,
        ) {
            core_fixture::installed(|pools| {
                let mut hardware = core_fixture::Hardware::default();
                let mut core = core_fixture::enabled(&mut hardware, pools);
                let mut arenas = arenas(&mut core);
                let mut events = core_fixture::Events::default();
                let source = core_fixture::backings();
                let mut port = port(&mut arenas, &mut core, &mut hardware, &mut events, None);
                let [first, _] = port.arenas().expect("the arenas");
                // Four MPDUs of 1 900 octets pass the HT MCS 0 ceiling.
                for tag in [30, 31, 32, 33] {
                    port.intake(&first, source.backing().expect("memory"), |region| {
                        write_tagged(tag, region)?;
                        Some(1_900)
                    })
                    .unwrap_or_else(|_| panic!("intake"));
                }
                let Err(PublishError::Refused {
                    publication: refused,
                    reason: PublishRefused::Length,
                }) = port.publish(
                    publication(first, aggregate_format(rate)),
                    Instant::from_micros(1),
                )
                else {
                    panic!("the port holds the rate's octet limit");
                };
                port.publish(
                    publication(refused.arena, ArenaFormat::Single(single)),
                    Instant::from_micros(2),
                )
                .unwrap_or_else(|_| panic!("a single MPDU of position 0"));
                assert_eq!(port.hardware.legacy.len(), 1, "one legacy program");
                port.hardware.completion[core_fixture::BE] =
                    Some(MacTxCompletionObservation::new_model(0, 0));
                let done = port
                    .service(
                        WifiTxWake::Interrupt {
                            events: crate::mac::irq::EVENT_TX_COMPLETE,
                        },
                        Instant::from_micros(3),
                    )
                    .ok()
                    .flatten()
                    .expect("the ACK ends it");
                assert_eq!(done.frames(), 4);
                assert!(matches!(done.end(), AttemptEnd::Acked { .. }));
                let mut returned = Vec::new();
                let first = port
                    .release(done, |memory| {
                        returned.push(tag_of(&memory.as_slice()[8..]))
                    })
                    .unwrap_or_else(|_| panic!("released"));
                assert_eq!(returned, [Some(30), Some(31), Some(32), Some(33)]);
                port.withdraw(&first, drop)
                    .unwrap_or_else(|_| panic!("empty"));
                assert!(arenas.into_parts().is_ok(), "both tickets are back");
            });
        }
    }
    with_rates(Case);
}
