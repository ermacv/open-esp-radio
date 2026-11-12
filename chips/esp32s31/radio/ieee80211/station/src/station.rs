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
    Lifecycle, LifecycleArbiter, LifecycleCompleted, LifecycleEnd, LifecycleIo, LifecycleTxContext,
    RetiredLifecycle,
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
    // The receiver address is the header's first address
    // (IEEE Std 802.11-2020, 9.3.2.1).
    if bytes.get(4..10).is_some_and(|receiver| receiver == local) {
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
/// authentication and its beacons. The frame control field decides which
/// of them a frame can be, so a data frame is read no further than its
/// receiver.
#[cfg(oer_trace)]
pub(crate) fn observe_received<W: Binding>(bytes: &[u8], meta: &RxMeta) {
    use oer_protocols_ieee80211_wire::frame::{FrameControlFields, FrameType, ManagementSubtype};
    let Some(&control) = bytes.first_chunk::<2>() else {
        return;
    };
    let control = FrameControlFields::from_bytes(control);
    let Some((local, target)) = TARGET.get() else {
        return;
    };
    match control.kind() {
        FrameType::Data => observe_received_data::<W>(bytes, meta, local),
        FrameType::Management => {
            let Some(target) = target else {
                return;
            };
            if oer_protocols_ieee80211_wire::station::parse_open_authentication_response(
                bytes, local, target,
            )
            .is_some_and(|r| r.status_code == 0)
            {
                let observer = W::DriverObserver::default();
                oer_portable_trace::observe!(observer, crate::observation::StationAuthenticated);
            }
            if control.management_subtype() == ManagementSubtype::Beacon
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
        _ => {}
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
        oer_protocols_ieee80211_wire::channel::ChannelWidth::Mhz20,
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
        oer_protocols_ieee80211_wire::channel::ChannelWidth::Mhz40Below,
    ) {
        Ok(channel) => channel,
        Err(_) => panic!("HT40 advertisement geometry"),
    },
);
pub(crate) static ASSOCIATION: AssociationCapabilities = AssociationCapabilities {
    ht20: HT20,
    ht40: HT40,
    he20_ht: crate::he::HE20_HT,
    he20: crate::he::HE20,
    he20_extended: crate::he::HE20_EXTENDED,
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
/// The retired lifecycle of `W`: the station, its places, the arbiter, the
/// issuer, the contexts, a rejected owner and the events' sink.
pub(crate) type RetiredOf<W> = RetiredLifecycle<
    'static,
    radio::ActorPort<W>,
    crate::chip::CoreCallbacks<<W as Binding>::Callbacks>,
    radio::Commands<W>,
    Contexts<W>,
    1,
    <W as Binding>::Sink,
>;

/// The retired station's bank of `W`.
pub type RetiredStationOf<W> = oer_protocols_ieee80211_core_roles_sta::machine::RetiredStation<
    'static,
    oer_protocols_ieee80211_runtime::lifecycle::LifecycleTokens<'static, radio::ActorPort<W>, 1>,
    Caps,
    crate::chip::CoreCallbacks<<W as Binding>::Callbacks>,
>;

/// The gate path of `W`'s actor port.
type GatePathOf<W> =
    oer_protocols_ieee80211_runtime::datapath::ActorGatePath<'static, radio::ActorPort<W>>;

/// The places `W`'s retired station returned.
pub type StationPlacesOf<W> =
    oer_protocols_ieee80211_core_arbiter::StationPlaceReturns<'static, GatePathOf<W>>;

/// The retired arbiter's bank of `W`.
pub type RetiredArbiterOf<W> = oer_protocols_ieee80211_core_arbiter::RetiredArbiter<
    'static,
    Caps,
    oer_protocols_ieee80211_core_arbiter::NoReservation,
    GatePathOf<W>,
>;

/// The first owner a broken contract left in `W`'s lifecycle.
pub type RejectedOf<W> =
    oer_protocols_ieee80211_runtime::lifecycle::Rejected<'static, radio::ActorPort<W>>;

/// The station lifecycle's half of an installation that did not retire:
/// its retired lifecycle, whole, in the place that keeps it for the
/// application, the lifecycle's ends of the queues to the actor with every
/// owner in transit in them, and the actor task's news that came after
/// the lifecycle stopped waiting. The place the lifecycle ran in is free
/// once the half left it; the half stays here for good.
#[must_use = "the lifecycle's half holds the station's owners of the installation"]
pub struct UnfinishedLifecycle<W: Binding>(Placed<'static, HalfOf<W>>);

/// The generic half of `W`'s lifecycle.
type HalfOf<W> = crate::station_end::UnfinishedLifecycleOf<
    'static,
    Placed<'static, RetiredOf<W>>,
    radio::LifecycleQueues<W>,
    radio::Commands<W>,
    radio::Tokens<W>,
    InitializationError,
    <W as Binding>::Id,
>;

/// What a finished lifecycle half of `W` hands back.
pub type FinishedLifecycle<W> = crate::station_end::FinishedLifecycleOf<
    radio::LifecycleQueues<W>,
    radio::Commands<W>,
    radio::Tokens<W>,
    InitializationError,
    <W as Binding>::Id,
>;

/// One installation of `W`'s port.
pub type InstallationOf<W> = crate::station_end::InstallationOf<<W as Binding>::Id>;

/// A proof that `W`'s lifecycle half refused: the half and the proof,
/// unchanged, and why.
pub type RefusedProofOf<W> =
    crate::station_end::RefusedProof<UnfinishedLifecycle<W>, <W as Binding>::Id>;

impl<W: Binding> UnfinishedLifecycle<W> {
    /// The lifecycle's ends of the queues to the actor: the owners the
    /// actor sent and the lifecycle had not taken, each once, in order.
    pub fn queues(&mut self) -> &mut radio::LifecycleQueues<W> {
        self.0.queues()
    }

    /// The events' sink the retired lifecycle keeps.
    pub fn events(&mut self) -> &mut W::Sink {
        &mut self.0.retired_mut().events
    }

    /// The retired station's bank: its configuration, callbacks and the
    /// tokens it held.
    pub fn station(&mut self) -> &mut RetiredStationOf<W> {
        &mut self.0.retired_mut().station
    }

    /// The places the station returned.
    pub fn places(&mut self) -> &mut StationPlacesOf<W> {
        &mut self.0.retired_mut().places
    }

    /// The arbiter's bank, once: `None` once taken, or when a broken
    /// contract ended the lifecycle while it moved the arbiter.
    pub fn take_arbiter(&mut self) -> Option<RetiredArbiterOf<W>> {
        self.0.retired_mut().arbiter.take()
    }

    /// The installation's commands issuer the lifecycle held.
    pub fn issuer(&mut self) -> &mut radio::Commands<W> {
        &mut self.0.retired_mut().issuer
    }

    /// The context still waiting at the place of ticket `ticket`, once:
    /// `None` once taken or when none waits there (or no such place).
    pub fn take_context(
        &mut self,
        ticket: usize,
    ) -> Option<LifecycleTxContext<'static, radio::ActorPort<W>>> {
        self.0
            .retired_mut()
            .tx_contexts
            .get_mut(ticket)
            .and_then(Option::take)
    }

    /// The first owner a broken contract left, once.
    pub fn take_rejected(&mut self) -> Option<RejectedOf<W>> {
        self.0.retired_mut().rejected.take()
    }

    /// How far the actor task's end after the lifecycle stopped waiting for
    /// it (a release past the stop's drain, the stop after an abandon, a
    /// refused reinstall) is read, kept once read
    /// ([`crate::station_end::LateRead`]).
    pub fn late_end(&mut self) -> crate::station_end::LateRead<InitializationError> {
        self.0.late_end()
    }

    /// Wait until the actor's task tells how it ended: the continuation of
    /// an abandon whose wait stopped on full places, once a kept news was
    /// taken ([`Self::take_installed`]).
    ///
    /// # Errors
    ///
    /// Every place for installations' news is full: nothing more was read.
    pub async fn confirm_end(
        &mut self,
    ) -> Result<crate::station_end::LateEnd<InitializationError>, crate::station_end::KeptFull>
    {
        self.0.confirm_end().await
    }

    /// The commands, tokens and installation a late news of an
    /// installation carried, each once, in order.
    pub fn take_installed(
        &mut self,
    ) -> Option<(radio::Commands<W>, radio::Tokens<W>, InstallationOf<W>)> {
        self.0.take_installed()
    }

    /// The installation the actor's task runs on now, as far as its news
    /// was read: only its proof finishes the half.
    pub fn installation(&self) -> InstallationOf<W> {
        self.0.installation()
    }

    /// Finish the half of an installation whose datapath half retired
    /// ([`radio::DatapathEnd::Retired`]): the retired lifecycle's owners end
    /// with the retired installation and no release runs again. The actual
    /// end of the actor's task, beside the exit the station's task
    /// returned, and every owner still in transit come back.
    ///
    /// # Errors
    ///
    /// The proof names another installation: the half and the proof come
    /// back unchanged.
    #[allow(
        clippy::result_large_err,
        reason = "a refused finish returns the half and the proof whole, by value"
    )]
    pub fn finish(
        mut self,
        retired: crate::station_end::DatapathRetired<<W as Binding>::Id>,
    ) -> Result<FinishedLifecycle<W>, RefusedProofOf<W>> {
        // A refused proof leaves the half in its place.
        if let Some(cause) = self.0.refusal(&retired) {
            return Err(crate::station_end::RefusedProof {
                half: self,
                proof: retired,
                cause,
            });
        }
        // The proof is spent: its installation retired with this half.
        let _spent = retired;
        Ok(self.0.read().into_finished())
    }
}

/// How the station's lifecycle task ended: its exit, which Control keeps,
/// and the lifecycle's half of an installation that did not retire. Its
/// datapath half comes from the actor's task
/// ([`crate::radio::DatapathEnd`]).
#[must_use = "an unfinished installation's lifecycle half holds its owners"]
pub struct StationEnd<W: Binding> {
    pub exit: StationExit,
    pub lifecycle: Option<UnfinishedLifecycle<W>>,
    /// After a completed retire, the lifecycle's ends of the queues to the
    /// actor when an owner was still in transit in them once the actor's
    /// task had ended: the owners, each once, in order. `None` when the
    /// queues were empty (they end with the retired installation) or the
    /// half holds them.
    pub in_transit: Option<radio::LifecycleQueues<W>>,
}

/// The station's configuration and callbacks, which every installation's
/// station takes again.
type StationConfig<W> =
    machine::StaConfig<Caps, crate::chip::CoreCallbacks<<W as Binding>::Callbacks>>;

/// The lifecycle's places: one lifecycle and its end at a time, reused by
/// every installation once the previous one's end settled, and the home
/// of an unfinished installation's lifecycle half.
pub(crate) struct ServiceStorage<W: Binding> {
    lifecycle: StaticPlace<LifecycleOf<W>>,
    completed: StaticPlace<CompletedOf<W>>,
    unfinished: StaticPlace<RetiredOf<W>>,
    /// The unfinished installation's lifecycle half: it moves as its owner,
    /// not by value, through the task's end.
    half: StaticPlace<HalfOf<W>>,
}
impl<W: Binding> ServiceStorage<W> {
    pub(crate) const fn new() -> Self {
        Self {
            lifecycle: StaticPlace::new(),
            completed: StaticPlace::new(),
            unfinished: StaticPlace::new(),
            half: StaticPlace::new(),
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
/// Control once final: after a stop or a refused start, once the radio's
/// release ended. Each lifecycle's retired half stays in its place until
/// the actor's task told how its end settled: a confirmed release or
/// reinstall ends it, and only then is the place reused; any other end
/// moves it, whole, to the home it leaves in.
pub async fn serve<W: Binding>(runner: StationRunner<W>, profile: Profile<W>) -> StationEnd<W> {
    let StationRunner {
        service,
        actor,
        news: mailbox,
        abandon,
        stop,
    } = runner;
    let end = lifecycles::<W>(service, actor, mailbox, abandon, profile).await;
    stop.publish(end.exit);
    end
}

/// The actor task's news to the station's task.
type Mailbox<W> = oer_portable_memory::WakingSpscReceiver<'static, radio::News<W>, 1>;

/// `retired`, which ran on `installation`, moved whole to `home`, with the
/// lifecycle's ends of the queues and the mailbox: the half of an
/// installation that did not retire, for the application.
fn unfinished<W: Binding>(
    retired: Placed<'_, RetiredOf<W>>,
    (home, half): Homes<W>,
    queues: radio::LifecycleQueues<W>,
    installation: InstallationOf<W>,
    mailbox: Mailbox<W>,
    late: Option<crate::station_end::LateEnd<InitializationError>>,
) -> UnfinishedLifecycle<W> {
    let retired = retired.move_to(home);
    UnfinishedLifecycle(Placed::init(half, |half| {
        half.write(crate::station_end::UnfinishedLifecycleOf::new(
            retired,
            queues,
            installation,
            mailbox,
            late,
        ))
    }))
}

/// The homes an unfinished installation's lifecycle half leaves in: its
/// retired lifecycle's, and its own.
type Homes<W> = (
    &'static mut MaybeUninit<RetiredOf<W>>,
    &'static mut MaybeUninit<HalfOf<W>>,
);

/// The end of a lifecycle that ended on `failure` without retiring the
/// actor: the actor's task hears it once and ends with its half
/// ([`crate::station_end::DatapathCause::Lifecycle`]); once it told that
/// its task ended, `retired` and the lifecycle's ends of the queues go out
/// whole beside it.
async fn abandoned<W: Binding>(
    failure: crate::failure::LifecycleFailure,
    mut half: UnfinishedLifecycle<W>,
    abandon: &mut radio::Abandon,
) -> StationEnd<W> {
    half.0.abandoned(failure, abandon).await;
    StationEnd {
        exit: StationExit::Failed(failure),
        lifecycle: Some(half),
        in_transit: None,
    }
}

async fn lifecycles<W: Binding>(
    service: ServiceRunner<W>,
    mut actor: radio::LifecycleQueues<W>,
    mut mailbox: Mailbox<W>,
    mut abandon: radio::Abandon,
    profile: Profile<W>,
) -> StationEnd<W> {
    use crate::station_end::LifecycleHalf;
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
        unfinished: home,
        half: half_home,
    } = storage;
    let lifecycle_place = lifecycle_place.uninit();
    let completed_place = completed_place.uninit();
    let homes: Homes<W> = (home.uninit(), half_home.uninit());
    let Profile {
        station: mut station_config,
        mut sink,
    } = profile;
    let mut start = Some(start);
    let mut next = match crate::station_end::installed(news(&mut mailbox).await) {
        Ok(installed) => installed,
        Err(exit) => {
            if let StationExit::Unavailable(error) = exit
                && let Some(start) = start.take()
            {
                start.complete(Err(crate::StartError::Unavailable(error)));
            }
            return StationEnd {
                exit,
                lifecycle: None,
                in_transit: None,
            };
        }
    };
    loop {
        let (issuer, tokens, installation) = next;
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
            &mut actor,
            &mut io,
            &EmbassyClock,
        )
        .await;
        let (end, retired) =
            oer_portable_memory::unpack_placed!(completed, LifecycleCompleted { end, retired });
        let end = end.read();
        // The actor retired (a stop, a refused start): the exit and the
        // half wait for the radio's release, or for the stop's drain
        // first. A network that returned late or was lost waits without a
        // deadline of its own, as no radio owner is unconfirmed.
        let (deadline, failure) = match end {
            LifecycleEnd::Stopped => (Some(config.stop.drain), None),
            LifecycleEnd::StoppedReturning(_) | LifecycleEnd::StoppedNetworkLost(_) => (None, None),
            LifecycleEnd::StartRefused(refused) => (
                Some(config.stop.drain),
                Some(crate::failure::LifecycleFailure::StartRefused(
                    crate::failure::convert::start_refused(refused),
                )),
            ),
            LifecycleEnd::Restart(failure) => {
                let failure = crate::failure::convert::restart(failure);
                restarts.record(failure);
                if let Some(failure) = failure {
                    W::restarted(&failure);
                }
                // The retired half stays whole until the reinstall's
                // answer: a refusal sends it out with the actor's half.
                match crate::station_end::installed(news(&mut mailbox).await) {
                    Ok(installed) => {
                        // The next installation's station takes the
                        // configuration and the events' sink back; this
                        // installation's other owners end with it, and
                        // its place is free.
                        let (
                            station,
                            _places,
                            _arbiter,
                            _issuer,
                            _contexts,
                            _rejected,
                            _unsent,
                            _confirmed,
                            events,
                        ) = oer_portable_memory::unpack_placed!(
                            retired,
                            RetiredLifecycle {
                                station,
                                places,
                                arbiter,
                                issuer,
                                tx_contexts,
                                rejected,
                                unsent,
                                confirmed,
                                events
                            }
                        );
                        station_config = station.read().into_config();
                        sink = events.read();
                        next = installed;
                        continue;
                    }
                    // The news read is the actor task's end: the half
                    // keeps it too.
                    Err(exit) => {
                        return StationEnd {
                            exit,
                            lifecycle: Some(unfinished(
                                retired,
                                homes,
                                actor,
                                installation,
                                mailbox,
                                Some(crate::station_end::LateEnd::Ended(exit)),
                            )),
                            in_transit: None,
                        };
                    }
                }
            }
            // The lifecycle ended without retiring the actor: the actor's
            // task ends with its half, and this one goes out beside it.
            LifecycleEnd::StopUnconfirmed(observed) => {
                let failure = crate::failure::LifecycleFailure::StopUnconfirmed(
                    crate::failure::convert::stop_observed(observed),
                );
                let half = unfinished(retired, homes, actor, installation, mailbox, None);
                return abandoned(failure, half, &mut abandon).await;
            }
            LifecycleEnd::RetuneUnconfirmed(unconfirmed) => {
                let failure = crate::failure::LifecycleFailure::RetuneUnconfirmed(
                    crate::failure::convert::stop_unconfirmed(unconfirmed),
                );
                let half = unfinished(retired, homes, actor, installation, mailbox, None);
                return abandoned(failure, half, &mut abandon).await;
            }
            LifecycleEnd::Faulted(fault) => {
                let failure = crate::failure::convert::actor_fault(fault);
                let half = unfinished(retired, homes, actor, installation, mailbox, None);
                return abandoned(failure, half, &mut abandon).await;
            }
            LifecycleEnd::Contract(violation) => {
                let failure = crate::failure::LifecycleFailure::Contract(violation);
                let half = unfinished(retired, homes, actor, installation, mailbox, None);
                return abandoned(failure, half, &mut abandon).await;
            }
        };
        let news = match deadline {
            Some(drain) => {
                let deadline = oer_portable_time::Clock::now(&EmbassyClock).saturating_add(drain);
                crate::release::within(&EmbassyClock, Some(deadline), |cx| {
                    poll_news(&mut mailbox, cx)
                })
                .await
            }
            None => Some(news(&mut mailbox).await),
        };
        let (exit, half, late) = crate::station_end::release_settled(news, failure);
        return match half {
            // The actor's task ended with its release: nothing produces
            // into the queues any more, and an owner still in them comes
            // back.
            LifecycleHalf::Retired => {
                drop(retired);
                StationEnd {
                    exit,
                    lifecycle: None,
                    in_transit: crate::station_end::in_transit(actor).await,
                }
            }
            LifecycleHalf::Unfinished => StationEnd {
                exit,
                lifecycle: Some(unfinished(
                    retired,
                    homes,
                    actor,
                    installation,
                    mailbox,
                    late,
                )),
                in_transit: None,
            },
        };
    }
}
