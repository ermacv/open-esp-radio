//! Image-owned mapping; the station service and IP workload run on P1.
use crate::chip::{
    Binding, Config, InitializationError, Profile, ServiceRunner, StationExit, StationRunner,
};
use crate::radio::{self, Caps};
use core::{convert::Infallible, future::Future, marker::PhantomData, mem::MaybeUninit};
use oer_chips_esp32s31_radio_ieee80211_backend::port::{self, device::Composition};
use oer_chips_family_radio_ieee80211_policy::rate_selection::EspressifRateSelection;
use oer_portable_embassy_time::EmbassyClock;
use oer_protocols_ieee80211_contracts_port::*;
use oer_protocols_ieee80211_runtime::datapath::ActorRxMeta;
use oer_protocols_ieee80211_runtime::lifecycle::{
    ActorEnds, Lifecycle, LifecycleArbiter, LifecycleCompleted, LifecycleEnd, LifecycleIo,
    LifecycleTxContext, RetiredLifecycle,
};

use oer_portable_memory::{Placed, StaticPlace};
#[cfg(oer_trace)]
use oer_portable_time::Clock;
use oer_portable_time::Instant;
use oer_protocols_ieee80211_core_roles_sta::{
    epoch::{EpochContext, LinkEpochSource},
    machine::{self, Station, StationResources},
};
use oer_protocols_ieee80211_core_upper_mac::{
    rate_control::RatePeer,
    rate_selection::{FrameRates, RateSelection},
};
use oer_protocols_ieee80211_wire::{
    ht::{HtAmpduParameters, HtCapabilityInfo, HtLocalCapabilities, ht_capability_ie},
    station::AssociationCapabilities,
};
use oer_protocols_radio_port::{self as rp, Child, Root, SlotIdentity};

pub type Epoch<Id> = Child<Root<Id>, 4>;
pub struct ImageComposition<W: Binding>(PhantomData<W>);
impl<W: Binding> Composition for ImageComposition<W> {
    type Coex = radio::Coex;
    type Entropy = radio::Entropy;
    type Body = Infallible;
    // The station keeps each transmission's context at its ticket's place:
    // the port carries none.
    type OwnedContext<Id: SlotIdentity> = ();
    type Context<'p, Id: SlotIdentity + 'p> = ();
    type OwnedConfigureContext<Id: SlotIdentity> =
        rp::ContextChoice<(), EpochContext<Epoch<Id>, ()>>;
    type ConfigureContext<'p, Id: SlotIdentity + 'p> =
        machine::StaConfigurationContext<'p, Epoch<Id>>;
}
/// The pool places the station's management frames take, holding the
/// port's own buffers. Data goes through the port's data arenas, which hold
/// the network's memory itself.
pub(crate) const MANAGEMENT: u8 = crate::supply::PORT_BUFFERS;
#[cfg(oer_trace)]
pub(crate) static TARGET: oer_portable_memory::OnceValue<([u8; 6], Option<[u8; 6]>)> =
    oer_portable_memory::OnceValue::new();
#[cfg(oer_trace)]
pub(crate) fn observe_terminal<'id, W: Binding, Id: SlotIdentity + 'id>(
    terminal: &LowerMacTerminal<'id, radio::Family<Id, W>>,
) {
    if let LowerMacTerminal::TxCompleted(TxCompleted::Single(completed)) = terminal
        && matches!(completed.outcome(), TxOutcome::Indeterminate(_))
    {
        let observer = W::DriverObserver::default();
        oer_portable_trace::observe!(observer, crate::observation::TxIndeterminate);
    }
}
/// Report a data MPDU addressed to `local`: its PPDU's rate and whether it
/// came in an A-MPDU, as `meta` tells.
#[cfg(oer_trace)]
fn observe_received_data<W: Binding>(bytes: &[u8], meta: &RxMeta, local: [u8; 6]) {
    if oer_protocols_ieee80211_wire::frame::DecodedHeader::decode(bytes)
        .is_some_and(|header| header.control().is_data() && header.receiver() == local)
    {
        let observer = W::DriverObserver::default();
        oer_portable_trace::observe!(
            observer,
            crate::observation::RxData {
                rate: crate::observation::RateClass::of(match meta.rate {
                    RxEvidence::HardwareObserved(rate) | RxEvidence::ProtocolValidated(rate) => {
                        Some(rate)
                    }
                    RxEvidence::Unavailable => None,
                }),
                containment: crate::observation::Containment::of(meta.ampdu),
            }
        );
    }
}
/// Report a unit the port gave: a data MPDU addressed to the station
/// ([`observe_received_data`]), and the target's successful open
/// authentication and its beacons.
#[cfg(oer_trace)]
pub(crate) fn observe_received<W: Binding>(bytes: &[u8], meta: &RxMeta) {
    if let Some((local, _)) = TARGET.get() {
        observe_received_data::<W>(bytes, meta, local);
    }
    if let Some((local, Some(target))) = TARGET.get() {
        if oer_protocols_ieee80211_wire::station::parse_open_authentication_response(
            bytes, local, target,
        )
        .is_some_and(|r| r.status_code == 0)
        {
            let observer = W::DriverObserver::default();
            oer_portable_trace::observe!(observer, crate::observation::StationAuthenticated);
        }
        if oer_protocols_ieee80211_wire::frame::DecodedHeader::decode(bytes)
            .is_some_and(|h| h.control().subtype() == 8)
            && oer_protocols_ieee80211_wire::scan::parse_management(bytes)
                .is_some_and(|r| r.bssid.get() == target)
        {
            let observer = W::DriverObserver::default();
            oer_portable_trace::observe!(
                observer,
                crate::observation::StationBeacon {
                    at_millis: (EmbassyClock.now().as_micros() / 1000) as u32
                }
            );
        }
    }
}
pub struct Rates<W: Binding>(
    pub(crate) EspressifRateSelection<Caps>,
    pub(crate) W::Observer,
);
impl<W: Binding, M: SignalMetadata> RateSelection<M, port::Ack, port::BlockAckAnswer> for Rates<W> {
    fn packet_number_step(
        &self,
    ) -> oer_protocols_ieee80211_core_upper_mac::ccmp::CcmpPacketNumberStep {
        <EspressifRateSelection<Caps> as RateSelection<M, port::Ack, port::BlockAckAnswer>>::packet_number_step(&self.0)
    }
    fn plan<R>(
        &self,
        frame: FrameRates,
        carried: impl Fn(oer_protocols_ieee80211_wire::phy::PhyRate) -> Option<R>,
    ) -> Option<RetryPlan<R>> {
        <EspressifRateSelection<Caps> as RateSelection<M, port::Ack, port::BlockAckAnswer>>::plan(
            &self.0, frame, carried,
        )
    }
    fn associated(&mut self, peer: &RatePeer, response: &M) {
        oer_portable_trace::observe!(self.1, crate::observation::StationAssociated);
        <EspressifRateSelection<Caps> as RateSelection<M, port::Ack, port::BlockAckAnswer>>::associated(
            &mut self.0,
            peer,
            response,
        );
        oer_portable_trace::observe!(self.1, selected_rate::<M>(&self.0));
    }
    fn disconnected(&mut self) {
        <EspressifRateSelection<Caps> as RateSelection<M, port::Ack, port::BlockAckAnswer>>::disconnected(&mut self.0);
    }
    fn completed<D>(
        &mut self,
        now: Instant,
        frame: FrameRates,
        completion: &SingleOutcome<port::Ack, D>,
    ) {
        #[cfg(oer_trace)]
        if frame == FrameRates::Control
            && matches!(completion, TxOutcome::Ended(SingleEnd::Delivered { .. }))
        {
            oer_portable_trace::observe!(self.1, crate::observation::ControlDelivered);
        }
        #[cfg(oer_trace)]
        if matches!(
            completion,
            TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout)
        ) {
            oer_portable_trace::observe!(self.1, crate::observation::TxHardwareTimeout);
        }
        #[cfg(oer_trace)]
        if frame == FrameRates::Data {
            oer_portable_trace::observe!(self.1, crate::observation::TxData::of(completion));
        }
        <EspressifRateSelection<Caps> as RateSelection<M, port::Ack, port::BlockAckAnswer>>::completed(
            &mut self.0,
            now,
            frame,
            completion,
        );
        #[cfg(oer_trace)]
        if frame == FrameRates::Data {
            oer_portable_trace::observe!(self.1, selected_rate::<M>(&self.0));
        }
    }
    fn completed_aggregate<D>(
        &mut self,
        now: Instant,
        tally: oer_protocols_ieee80211_core_upper_mac::rate_control::BlockAckTally,
        completion: &oer_protocols_ieee80211_contracts_port::AggregateOutcome<
            port::BlockAckAnswer,
            D,
        >,
    ) {
        <EspressifRateSelection<Caps> as RateSelection<M, port::Ack, port::BlockAckAnswer>>::completed_aggregate(
            &mut self.0,
            now,
            tally,
            completion,
        );
        #[cfg(oer_trace)]
        oer_portable_trace::observe!(self.1, selected_rate::<M>(&self.0));
    }
}

#[cfg(oer_trace)]
fn selected_rate<M: SignalMetadata>(
    selection: &EspressifRateSelection<Caps>,
) -> crate::observation::TxRateSelected {
    use oer_protocols_ieee80211_wire::phy::PhyRate;
    crate::observation::TxRateSelected {
        kbps: <EspressifRateSelection<Caps> as RateSelection<M, port::Ack, port::BlockAckAnswer>>::plan(
            selection,
            FrameRates::Data,
            Some::<PhyRate>,
        )
        .map(|plan| plan.first().rate.nominal_kbps()),
    }
}

// IEEE 802.11 HT20, one spatial stream, MCS 0..7; no HE advertisement.
// WMM Information: QoS station, U-APSD disabled (IEEE 802.11 WMM OUI type 2).
const HT20: [u8; 28] = ht_capability_ie(
    HtLocalCapabilities::new(0, 0, 0xff, 1),
    match Channel::ghz2_4(
        1,
        oer_protocols_ieee80211_wire::channel::WifiChannelWidth::Mhz20,
    ) {
        Ok(channel) => channel,
        Err(_) => panic!("HT20 advertisement geometry"),
    },
);
// IEEE 802.11 HT40, one spatial stream, MCS 0..7 with both short guard
// intervals; the encoder sets the width and guard-interval bits from the
// 40 MHz geometry. SM power save is disabled and the station receives
// A-MPDUs of up to 65 535 octets with MPDUs at least 4 us apart: an AP's
// 32-MPDU aggregate of 1 500-octet MSDUs needs the 65 535-octet limit.
//
// SOURCE: the vendor station's HT Capabilities beside its HE elements,
// `libnet80211.a[ieee80211_ht.o]::ieee80211_add_htcap_body`: SM power save
// disabled (bits 2..3 = 0b11), A-MPDU Parameters 0x17 (exponent 3, spacing
// code 5). Recovered in open-esp-radio-rs da26f64c3
// `crates/roles/esp32s31/ieee80211/sta/src/profile.rs` (SOURCE
// HIL_VENDOR_HE20_NDPA_CBF_2026_07_24, frame 7624), where a static SM power
// save HT40 request was observed to be dropped by a Linux HT40 AP.
const HT40: [u8; 28] = ht_capability_ie(
    HtLocalCapabilities::new(
        u16::from_le_bytes(
            HtCapabilityInfo::from_bytes([0; 2])
                .with_sm_power_save(HtCapabilityInfo::SM_POWER_SAVE_DISABLED)
                .to_bytes(),
        ),
        {
            let [parameters] = HtAmpduParameters::from_bytes([0])
                .with_exponent(HtAmpduParameters::EXPONENT_65535_OCTETS)
                .with_spacing(HtAmpduParameters::SPACING_4_US)
                .to_bytes();
            parameters
        },
        0xff,
        1,
    ),
    match Channel::ghz2_4(
        13,
        oer_protocols_ieee80211_wire::channel::WifiChannelWidth::Mhz40Below,
    ) {
        Ok(channel) => channel,
        Err(_) => panic!("HT40 advertisement geometry"),
    },
);
pub(crate) static ASSOCIATION: AssociationCapabilities = AssociationCapabilities {
    ht20: HT20,
    ht40: HT40,
    he20_ht: [0; 28],
    he20: [0; 24],
    he20_extended: [0; 14],
    wmm: [221, 7, 0, 0x50, 0xf2, 2, 0, 1, 0],
};
/// The station's contexts at their tickets' places.
type Contexts<W> = [Option<LifecycleTxContext<'static, radio::ActorPort<W>>>; port::TX_TICKETS];
/// The station's lifecycle of `W`.
type LifecycleOf<W> = Lifecycle<
    'static,
    radio::ActorPort<W>,
    crate::chip::CoreCallbacks<<W as Binding>::Callbacks>,
    radio::Commands<W>,
    Contexts<W>,
    1,
    <W as Binding>::Sink,
>;
/// A finished lifecycle of `W` and what it keeps.
type CompletedOf<W> = LifecycleCompleted<
    'static,
    radio::ActorPort<W>,
    crate::chip::CoreCallbacks<<W as Binding>::Callbacks>,
    radio::Commands<W>,
    Contexts<W>,
    1,
    <W as Binding>::Sink,
>;
/// The station's configuration and callbacks, which every installation's
/// station takes again.
type StationConfig<W> =
    machine::StaConfig<Caps, crate::chip::CoreCallbacks<<W as Binding>::Callbacks>>;

/// The lifecycle's places: one lifecycle and its end at a time, reused by
/// every installation.
pub(crate) struct ServiceStorage<W: Binding> {
    lifecycle: StaticPlace<LifecycleOf<W>>,
    completed: StaticPlace<CompletedOf<W>>,
}
impl<W: Binding> ServiceStorage<W> {
    pub(crate) const fn new() -> Self {
        Self {
            lifecycle: StaticPlace::new(),
            completed: StaticPlace::new(),
        }
    }
}

/// Build the lifecycle of one installation in `place`: the station of
/// `station` on the installation's `tokens`, its arbiter, and the port's
/// `issuer`.
fn initialize<'s, W: Binding>(
    place: &'s mut MaybeUninit<LifecycleOf<W>>,
    station: StationConfig<W>,
    sink: W::Sink,
    config: &Config,
    issuer: radio::Commands<W>,
    tokens: radio::Tokens<W>,
) -> Placed<'s, LifecycleOf<W>> {
    let radio::Tokens {
        tickets,
        terminal_access,
        buffers,
        connection_epoch,
        station_association,
        station_commands,
        station_group_permission,
        gate,
        arbiter_authority,
    } = tokens;
    let (arbiter, place_back, absence_place) = LifecycleArbiter::<radio::ActorPort<W>>::new(
        crate::policy::arbiter(port::MOVE_PLAN)
            .unwrap_or_else(|_| W::fail(InitializationError::Arbiter)),
        config.supported,
        arbiter_authority,
        gate,
    );
    // The station's frames take four tickets and the pool's first four
    // buffers; the port's other tickets and buffer places have no owner
    // here: data goes through the port's data arenas.
    let [t0, t1, t2, t3, ..] = tickets;
    let [b0, b1, b2, b3, ..] = buffers;
    const {
        if MANAGEMENT != 4 {
            panic!("four management buffers");
        }
    }
    let step = <EspressifRateSelection<Caps> as RateSelection<
        ActorRxMeta<'static, radio::ActorPort<W>>,
        port::Ack,
        port::BlockAckAnswer,
    >>::packet_number_step(&EspressifRateSelection::new());
    LifecycleOf::<W>::init_in_place(
        place,
        |station_storage| {
            let station_resources = StationResources::<_, 4, 4, 1> {
                configure: station_commands.configure,
                epochs: LinkEpochSource::new(connection_epoch),
                tickets: [t0, t1, t2, t3],
                buffers: [b0, b1, b2, b3],
                place: place_back,
                absence_place,
                keys: station_association,
                group_permission: station_group_permission,
            };
            Station::init(
                station_storage,
                station,
                station_resources,
                machine::GroupCapacity::new(),
            )
        },
        arbiter,
        issuer,
        step,
        config.supported,
        terminal_access,
        sink,
    )
}

/// The next news of the actor's task.
async fn news<W: Binding>(
    news: &mut oer_portable_memory::WakingSpscReceiver<'static, radio::News<W>, 1>,
) -> radio::News<W> {
    loop {
        if let Some(news) = news.try_receive() {
            return news;
        }
        news.ready().await;
    }
}

/// The actor's next news, once it came.
fn poll_news<W: Binding>(
    news: &mut oer_portable_memory::WakingSpscReceiver<'static, radio::News<W>, 1>,
    cx: &mut core::task::Context<'_>,
) -> core::task::Poll<radio::News<W>> {
    if let Some(news) = news.try_receive() {
        return core::task::Poll::Ready(news);
    }
    if core::pin::pin!(news.ready()).poll(cx).is_ready() {
        match news.try_receive() {
            Some(news) => return core::task::Poll::Ready(news),
            None => cx.waker().wake_by_ref(),
        }
    }
    core::task::Poll::Pending
}

/// The station lifecycle's task: it builds the station on each
/// installation the actor's task hands it, runs the lifecycle, and on a
/// restart waits for the next installation. The exit is published to
/// Control once final: after an ordinary stop, once the radio's release
/// ended.
pub async fn serve<W: Binding>(runner: StationRunner<W>, profile: Profile<W>) -> StationExit {
    let StationRunner {
        service,
        mut actor,
        news: mut mailbox,
        stop,
    } = runner;
    let exit = lifecycles::<W>(service, &mut actor, &mut mailbox, profile).await;
    stop.publish(exit);
    exit
}

async fn lifecycles<W: Binding>(
    service: ServiceRunner<W>,
    actor: &mut ActorEnds<
        'static,
        'static,
        radio::ActorPort<W>,
        { radio::TO },
        { radio::FROM },
        { radio::RETURNS },
    >,
    mailbox: &mut oer_portable_memory::WakingSpscReceiver<'static, radio::News<W>, 1>,
    profile: Profile<W>,
) -> StationExit {
    let ServiceRunner {
        config,
        connection,
        storage,
        mut output,
    } = service;
    let crate::ServiceConnection {
        start,
        restarts,
        commands,
        runner: control,
    } = connection;
    let mut io = LifecycleIo {
        commands,
        output: &mut output,
        control,
        stop: config.stop,
    };
    let ServiceStorage {
        lifecycle: lifecycle_place,
        completed: completed_place,
    } = storage;
    let lifecycle_place = lifecycle_place.uninit();
    let completed_place = completed_place.uninit();
    let Profile {
        station: mut station_config,
        mut sink,
    } = profile;
    let mut start = Some(start);
    loop {
        let (issuer, tokens) = match news(mailbox).await {
            radio::News::Installed(issuer, tokens) => (issuer, tokens),
            radio::News::Unavailable(error) => {
                if let Some(start) = start.take() {
                    start.complete(Err(crate::StartError::Unavailable));
                }
                return StationExit::Unavailable(error);
            }
            radio::News::Reinstall(cause) => {
                return StationExit::Failed(crate::failure::LifecycleFailure::Reinstall(cause));
            }
            radio::News::Released(_) => {
                return StationExit::Failed(crate::failure::LifecycleFailure::Mailbox);
            }
        };
        let lifecycle = initialize::<W>(
            &mut *lifecycle_place,
            station_config,
            sink,
            &config,
            issuer,
            tokens,
        );
        if let Some(start) = start.take() {
            start.complete(Ok(()));
        }
        let completed = LifecycleOf::<W>::run_placed(
            lifecycle,
            &mut *completed_place,
            actor,
            &mut io,
            &EmbassyClock,
        )
        .await;
        let (end, retired) =
            oer_portable_memory::unpack_placed!(completed, LifecycleCompleted { end, retired });
        let end = end.read();
        match end {
            LifecycleEnd::Stopped => {
                drop(retired);
                // The exit is published only after the radio's release
                // ended, or once the stop's drain passed first.
                let deadline =
                    oer_portable_time::Clock::now(&EmbassyClock).saturating_add(config.stop.drain);
                let released = crate::release::within(&EmbassyClock, Some(deadline), |cx| {
                    poll_news(mailbox, cx)
                })
                .await;
                return match released {
                    Some(radio::News::Released(Ok(()))) => StationExit::Stopped,
                    Some(radio::News::Released(Err(cause))) => StationExit::ReleaseFailed(cause),
                    None => StationExit::ReleaseFailed(crate::failure::ReleaseCause::Deadline),
                    Some(
                        radio::News::Installed(..)
                        | radio::News::Unavailable(_)
                        | radio::News::Reinstall(_),
                    ) => StationExit::Failed(crate::failure::LifecycleFailure::Mailbox),
                };
            }
            LifecycleEnd::StoppedReturning(_) => {
                drop(retired);
                // The radio stopped; the network returns what it holds of
                // the station late. The actor retires once that return
                // came and the radio's release follows: the exit waits for
                // them, with no deadline of its own, as the return is the
                // network's software and no radio owner is unconfirmed.
                return match news(mailbox).await {
                    radio::News::Released(Ok(())) => StationExit::Stopped,
                    radio::News::Released(Err(cause)) => StationExit::ReleaseFailed(cause),
                    radio::News::Installed(..)
                    | radio::News::Unavailable(_)
                    | radio::News::Reinstall(_) => {
                        StationExit::Failed(crate::failure::LifecycleFailure::Mailbox)
                    }
                };
            }
            LifecycleEnd::Restart(failure) => {
                let failure = crate::failure::convert::restart(failure);
                restarts.record(failure);
                if let Some(failure) = failure {
                    W::restarted(&failure);
                }
                // The next installation's station takes the configuration
                // and the events' sink back; this installation's owners
                // drop with the retired banks.
                let (station, _places, _arbiter, _issuer, _contexts, _rejected, events) = oer_portable_memory::unpack_placed!(
                    retired,
                    RetiredLifecycle {
                        station,
                        places,
                        arbiter,
                        issuer,
                        tx_contexts,
                        rejected,
                        events
                    }
                );
                station_config = station.read().into_config();
                sink = events.read();
            }
            end => {
                drop(retired);
                return match crate::failure::convert::lifecycle(end) {
                    Some(failure) => StationExit::Failed(failure),
                    None => StationExit::Stopped,
                };
            }
        }
    }
}
