//! The lifecycle and the actor joined over the model's data port, the test
//! playing the access point, the network stack and the application: the
//! join, the handshake's EAPOL through the actor, the station's frames on
//! the link's numbers, the data both ways without the lifecycle, the
//! group rekey, the end and the restart on the next installation.

use super::*;
use core::cell::{Cell, RefCell};
use core::marker::PhantomData;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use core::task::Waker;
use std::boxed::Box;
use std::rc::Rc;
use std::sync::Arc;
use std::vec::Vec;

use oer_chips_family_ieee80211_policy::rate_selection::EspressifRateSelection;
use oer_portable_memory::{SlotReceiver, SlotSender, WakingSlotQueue, WakingSpscQueue};
use oer_portable_time::Duration;
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_ieee80211_contracts_port::{LowerMacBackend, ReturnedMpdu};
use oer_protocols_ieee80211_core_arbiter::ArbiterPolicy;
use oer_protocols_ieee80211_core_roles_sta::data::EthernetFrame;
use oer_protocols_ieee80211_core_roles_sta::machine::{
    GroupCapacity, StaSecurity, StationResources,
};
use oer_protocols_ieee80211_core_security_rsn::{Pmk, PtkContext, frames::RsnTxFrame};
use oer_protocols_ieee80211_testing_model::{
    AirFrame, ModelAck, ModelAir, ModelAnyBuffer, ModelBackend, ModelBlockAckEvidence, ModelCaps24,
    ModelCommands, ModelInstall, ModelLentRx, ModelPort, ModelResources, ModelSmall, ModelTypes,
};
use oer_protocols_ieee80211_wire::address::IndividualAddress;
use oer_protocols_ieee80211_wire::qos::WmmUserPriority;
use oer_protocols_ieee80211_wire::security::rsn::Akm;
use oer_protocols_radio_port::{Installations, LocalId, with_local_slot};

use crate::datapath::{Actor, ActorEnd, LifecycleEnds, NetworkEnds, PARCELS as ACTOR_PARCELS};
use crate::station::contract::{
    Command as StaCommand, DisconnectReason as StaDisconnectReason, Notice as StaNotice,
};
use crate::station::tests::{
    ANONCE, ASSOCIATION_REQUEST, AUTHENTICATION, BSSID, Callbacks, DATA, DEAUTHENTICATION, LOCAL,
    PMK, PROBE_REQUEST, SNONCE, SSID, ServiceContexts, TestConfig, association_response,
    authentication_response, beacon, channel6, config, decrypted, eapol_frame, kind, message3,
    meta, network_frame, protected_data, protected_group_message1,
};
use crate::station::{
    Control, ControlCommand, NetworkStop, NetworkStopLost, Outgoing, RX_COPIED_DEPTH, Received,
    RxBlockAckStorage, State,
};

const TO: usize = 24;
const FROM: usize = 64;
const RETURNS: usize = 40;
const TX_DEPTH: usize = 4;
const RX_DEPTH: usize = 8;
const COMMANDS: usize = 4;

type Backend<'s> = ModelBackend<LocalId<'s>, ModelCaps24, ServiceContexts>;
type Types<'s> = ModelTypes<Backend<'s>>;
type Memory = oer_protocols_ieee80211_testing_model::ModelNetworkMemory;

/// The model's port as the actor's and the lifecycle's.
struct PairPort<'s>(PhantomData<fn(&'s ()) -> &'s ()>);

impl<'s> DatapathPort<'s> for PairPort<'s> {
    type Port = ModelPort<'s, LocalId<'s>, ModelCaps24, ServiceContexts>;
    type Caps = ModelCaps24;
    type Types = Types<'s>;
    type Small = ModelSmall;
    type TxMemory = Memory;
    type LentRx = ModelLentRx;
    const RX_HELD: u8 = 8;

    fn arena_ack(response: ModelBlockAckEvidence) -> Option<ModelAck> {
        match response {
            ModelBlockAckEvidence::Measured(snr_db) => Some(ModelAck { snr_db }),
            ModelBlockAckEvidence::Unmeasured => None,
        }
    }
}

impl<'s> LifecyclePort<'s> for PairPort<'s> {
    fn tx_buffer(
        buffer: ModelAnyBuffer<'s, Backend<'s>>,
    ) -> Result<LifecycleTxBuffer<'s, Self>, ModelAnyBuffer<'s, Backend<'s>>> {
        match buffer {
            ModelAnyBuffer::Small(buffer) => Ok(ReturnedMpdu::Fresh(buffer)),
            ModelAnyBuffer::SmallSent(buffer) => Ok(ReturnedMpdu::Sent(buffer)),
            buffer @ (ModelAnyBuffer::Large(_) | ModelAnyBuffer::LargeSent(_)) => Err(buffer),
        }
    }
}

type P<'s> = PairPort<'s>;
type Issuer<'s> = ModelCommands<'s, LocalId<'s>, ModelCaps24, ServiceContexts>;
type Contexts<'s> =
    [Option<LifecycleTxContext<'s, P<'s>>>; oer_protocols_ieee80211_testing_model::MODEL_TICKETS];
type TestLifecycle<'s> = Lifecycle<'s, P<'s>, Callbacks, Issuer<'s>, Contexts<'s>>;
type Completed<'s> = LifecycleCompleted<'s, P<'s>, Callbacks, Issuer<'s>, Contexts<'s>, 2, ()>;

/// A network side that takes every frame and stops at once.
struct Network;

impl NetworkStop for Network {
    fn takes_frames(&self) -> bool {
        true
    }
    fn request_stop(&self) {}
    fn poll_stopped(&self, _: &mut core::task::Context<'_>) -> Poll<Result<(), NetworkStopLost>> {
        Poll::Ready(Ok(()))
    }
}

/// A task's waker: it marks the task woken and counts its wakes.
#[derive(Default)]
struct Flag {
    woken: AtomicBool,
    wakes: AtomicUsize,
}

impl std::task::Wake for Flag {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.woken.store(true, Ordering::Relaxed);
        self.wakes.fetch_add(1, Ordering::Relaxed);
    }
}

/// One task of the test's executor.
struct Task<'a> {
    future: Pin<&'a mut dyn Future<Output = ()>>,
    flag: Arc<Flag>,
    done: bool,
}

/// Run `tasks` until each ended: a woken task is polled with its own
/// waker; when none is woken, the clock moves to its next deadline.
fn run_tasks(tasks: &mut [Task<'_>], clock: &VirtualClock, air: &ModelAir) {
    for task in tasks.iter() {
        task.flag.woken.store(true, Ordering::Relaxed);
    }
    let mut idle = 0;
    loop {
        if tasks.iter().all(|task| task.done) {
            return;
        }
        let mut polled = false;
        for task in tasks.iter_mut() {
            if task.done || !task.flag.woken.swap(false, Ordering::Relaxed) {
                continue;
            }
            polled = true;
            let waker = Waker::from(task.flag.clone());
            let mut cx = Context::from_waker(&waker);
            if task.future.as_mut().poll(&mut cx).is_ready() {
                task.done = true;
            }
        }
        if polled {
            idle = 0;
            continue;
        }
        idle += 1;
        assert!(idle < 10_000, "the tasks never end");
        if clock.advance_to_next_deadline().is_none() {
            clock.advance(Duration::from_millis(1)).unwrap();
        }
        air.clock_advanced();
    }
}

/// What the application heard.
#[derive(Default)]
struct Heard {
    notices: RefCell<Vec<StaNotice>>,
    tx_refused: Cell<usize>,
    refusals: RefCell<Vec<crate::station::contract::TransmitRefusal>>,
    completed: Cell<usize>,
}

/// The world the script plays: the air, the clock, the application and
/// the network stack.
struct World<'b> {
    air: Rc<ModelAir>,
    clock: Rc<VirtualClock>,
    heard: Rc<Heard>,
    control: RefCell<Option<Control<'b, ModelCaps24, COMMANDS>>>,
    security: StaSecurity,
    stack: RefCell<SlotSender<'b, Option<Outgoing<Memory>>, TX_DEPTH>>,
    returned: RefCell<SlotReceiver<'b, Option<Memory>, TX_DEPTH>>,
    delivered: RefCell<WakingSpscReceiver<'b, Received<ModelLentRx>, RX_DEPTH>>,
    copies: RefCell<WakingSpscReceiver<'b, EthernetFrame, RX_COPIED_DEPTH>>,
    /// The lifecycle task's waker.
    lifecycle: Arc<Flag>,
}

/// Let the other tasks run once.
async fn yield_now() {
    let mut yielded = false;
    poll_fn(|cx| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;
}

impl World<'_> {
    fn profile(&self) -> oer_protocols_ieee80211_core_roles_sta::machine::StaProfile {
        let security = match &self.security {
            StaSecurity::Open => StaSecurity::Open,
            StaSecurity::Wpa2Personal { pmk } => StaSecurity::Wpa2Personal {
                pmk: pmk.duplicate(),
            },
            StaSecurity::Personal { password, pmk } => StaSecurity::Personal {
                password: oer_protocols_ieee80211_core_security_rsn::sae::SaePassword::new(
                    password.as_bytes(),
                )
                .unwrap(),
                pmk: pmk.duplicate(),
            },
        };
        oer_protocols_ieee80211_core_roles_sta::machine::StaProfile {
            ssid: heapless::Vec::from_slice(SSID).unwrap(),
            security,
        }
    }

    async fn station_command(&self, command: StaCommand) {
        let mut control = self.control.borrow_mut().take().unwrap();
        let operation = match command {
            StaCommand::Connect => control.connect(self.profile()).await,
            StaCommand::Disconnect => control.disconnect().await,
            StaCommand::BackgroundScan => control.scan().await,
        }
        .expect("the request is admitted");
        drop(operation);
        *self.control.borrow_mut() = Some(control);
    }

    async fn stop(&self) {
        let mut control = self.control.borrow_mut().take().unwrap();
        // A lifecycle that already ended refuses it.
        let _refused = control.command(ControlCommand::Stop).await;
        *self.control.borrow_mut() = Some(control);
    }

    /// The next frame of `subtype` the station sent after the first `seen`.
    async fn sent(&self, seen: &mut usize, subtype: u16) -> AirFrame {
        for round in 0..2000 {
            yield_now().await;
            if round % 20 == 19 {
                self.advance();
            }
            let sent = self.air.sent();
            while *seen < sent.len() {
                let frame = sent[*seen].clone();
                *seen += 1;
                if kind(&frame) == subtype {
                    return frame;
                }
            }
        }
        panic!("the station never sent {subtype:#06x}");
    }

    async fn notice(&self, notice: StaNotice) {
        for round in 0..2000 {
            yield_now().await;
            if round % 20 == 19 {
                self.advance();
            }
            if self.heard.notices.borrow().contains(&notice) {
                return;
            }
        }
        panic!("no {notice:?}");
    }

    /// Move the clock to the machines' next deadline.
    fn advance(&self) {
        if self.clock.advance_to_next_deadline().is_some() {
            self.air.clock_advanced();
        }
    }

    async fn settle(&self) {
        for _ in 0..50 {
            yield_now().await;
        }
    }

    async fn associate(&self, seen: &mut usize, protected: bool) {
        self.associate_with(seen, beacon(protected), protected)
            .await;
    }

    async fn associate_with(&self, seen: &mut usize, beacon: Vec<u8>, protected: bool) {
        self.station_command(StaCommand::Connect).await;
        self.sent(seen, PROBE_REQUEST).await;
        self.air.receive(&beacon, meta());
        let authentication = self.sent(seen, AUTHENTICATION).await;
        let response = authentication_response(&authentication.plaintext).unwrap();
        self.air.receive(&response, meta());
        self.sent(seen, ASSOCIATION_REQUEST).await;
        self.air.receive(&association_response(protected), meta());
    }

    async fn handshake(&self, seen: &mut usize) -> oer_protocols_ieee80211_core_security_rsn::Ptk {
        let ptk = self.start_handshake(seen).await;
        self.air.receive(&message3(&ptk), meta());
        let message4 = self.sent(seen, DATA).await;
        assert_eq!(message4.key, None, "Message 4 goes unprotected");
        self.notice(StaNotice::Connected {
            bssid: BSSID,
            channel: 6,
        })
        .await;
        ptk
    }

    /// Message 1 and the station's Message 2: the derived key.
    async fn start_handshake(
        &self,
        seen: &mut usize,
    ) -> oer_protocols_ieee80211_core_security_rsn::Ptk {
        let message1 = RsnTxFrame::<512>::message1(Akm::Psk, LOCAL, 1, ANONCE).unwrap();
        self.air.receive(&eapol_frame(message1.as_bytes()), meta());
        self.sent(seen, DATA).await;
        Pmk::from_bytes(PMK).derive_ptk(
            Akm::Psk,
            PtkContext {
                authenticator_address: BSSID,
                supplicant_address: LOCAL,
                authenticator_nonce: ANONCE,
                supplicant_nonce: SNONCE,
            },
        )
    }

    fn send_msdu(&self, destination: [u8; 6]) -> bool {
        self.send_msdu_at(destination, 0)
    }

    /// The network stack sends an MSDU of user `priority`, if its queue
    /// has room.
    fn send_msdu_at(&self, destination: [u8; 6], priority: u8) -> bool {
        while let Ok(Some(_memory)) = self.returned.borrow_mut().try_receive_with(Option::take) {}
        let packet = network_frame(
            oer_protocols_ieee80211_wire::data::MsduParts {
                destination,
                source: LOCAL,
                ether_type: 0x0800,
                payload: &[0xab; 100],
            },
            WmmUserPriority::new(priority).unwrap(),
        );
        self.stack
            .borrow_mut()
            .try_send_with(|slot| {
                *slot = Some(packet);
                Ok::<_, ()>(())
            })
            .is_ok()
    }

    /// The MSDUs the network stack received.
    fn delivered(&self) -> usize {
        let mut count = 0;
        while let Some(received) = self.delivered.borrow().try_receive() {
            if matches!(received, Received::Copied(_)) {
                let _copy = self.copies.borrow().try_receive();
            }
            count += 1;
        }
        count
    }

    fn lifecycle_wakes(&self) -> usize {
        self.lifecycle.wakes.load(Ordering::Relaxed)
    }
}

/// The script of a case: the access point, the network and the
/// application.
type Script<'w> = Pin<Box<dyn Future<Output = ()> + 'w>>;

/// A script that plays one installation.
type Scripted = Box<dyn for<'a, 'b> FnOnce(&'a World<'b>) -> Script<'a>>;

/// Box `script`, naming its borrow of the world.
fn script(script: impl for<'a, 'b> FnOnce(&'a World<'b>) -> Script<'a> + 'static) -> Scripted {
    Box::new(script)
}

/// The lifecycle of a fresh installation's `resources`.
/// A lifecycle's parts before it is built: the station, its arbiter, the
/// issuer, the first channel and the terminal access.
struct Parts<'s> {
    station: crate::lifecycle::LifecycleConstructed<'s, P<'s>, Callbacks, 2>,
    arbiter: LifecycleArbiter<'s, P<'s>>,
    issuer: Issuer<'s>,
    channel: SupportedChannel<ModelCaps24>,
    terminal_access: oer_protocols_ieee80211_contracts_port::TerminalAccess<
        's,
        <Types<'s> as PhaseTypes<'s>>::TicketKind,
        <Types<'s> as PhaseTypes<'s>>::TicketPath,
    >,
}

impl<'s> Parts<'s> {
    /// Run the lifecycle to its end: placed, as a composition places it,
    /// when placement is built.
    async fn run(
        self,
        actor: &mut crate::lifecycle::ActorEnds<'_, 's, P<'s>, TO, FROM, RETURNS>,
        io: &mut LifecycleIo<
            '_,
            ModelCaps24,
            impl crate::station::ServiceOutputSink<ModelCaps24>,
            COMMANDS,
            SameHart,
        >,
        clock: &VirtualClock,
    ) -> Completed<'s> {
        let Self {
            station,
            arbiter,
            issuer,
            channel,
            terminal_access,
        } = self;
        #[cfg(feature = "placement")]
        {
            let (station, credits) = station;
            let placed = Lifecycle::init_in_place(
                Box::leak(Box::new(core::mem::MaybeUninit::uninit())),
                |place| (place.write(station), credits),
                arbiter,
                issuer,
                CcmpPacketNumberStep::ONE,
                channel,
                terminal_access,
                (),
            );
            let output = Box::leak(Box::new(core::mem::MaybeUninit::uninit()));
            Lifecycle::run_placed(placed, output, actor, io, clock)
                .await
                .read()
        }
        #[cfg(not(feature = "placement"))]
        {
            let lifecycle: TestLifecycle<'s> = Lifecycle::new(
                station,
                arbiter,
                issuer,
                CcmpPacketNumberStep::ONE,
                channel,
                terminal_access,
                (),
            );
            lifecycle.run(actor, io, clock).await
        }
    }
}

fn lifecycle<'s>(
    config: TestConfig,
    resources: ModelResources<'s, Backend<'s>>,
    issuer: Issuer<'s>,
) -> (Parts<'s>, RxBlockAckStorage<'s, Types<'s>, 2, 4>) {
    let ModelResources {
        connection_epoch,
        block_acks,
        station_commands,
        terminal_access,
        tickets,
        small,
        station_association,
        station_pairwise,
        station_group,
        station_group_permission,
        gate,
        arbiter_authority,
        ..
    } = resources;
    let mut tickets = tickets.into_iter();
    let station_tickets: [_; 4] = core::array::from_fn(|_| tickets.next().unwrap());
    let small: [_; 4] = small.try_into().ok().unwrap();
    let channel = config.0.channels[0];
    let (arbiter, place, absence) = LifecycleArbiter::<P<'s>>::new(
        ArbiterPolicy::DEFAULT
            .checked(<Backend<'s> as LowerMacBackend<LocalId<'s>>>::PUBLICATION_BOUND)
            .unwrap(),
        channel,
        arbiter_authority,
        gate,
    );
    let (sta_config, _security) = config;
    let resources = StationResources::<_, 4, 4, 2> {
        configure: station_commands.configure,
        epochs: oer_protocols_ieee80211_core_roles_sta::epoch::LinkEpochSource::new(
            connection_epoch,
        ),
        tickets: station_tickets,
        buffers: small,
        place,
        absence_place: absence,
        keys: oer_protocols_ieee80211_contracts_port::association::reunite_vacant(
            station_association,
            oer_protocols_ieee80211_contracts_port::association::VacantKeyBank::new(
                station_pairwise,
                station_group,
            ),
        ),
        group_permission: station_group_permission,
    };
    let station = Station::new(sta_config, resources, GroupCapacity::new());
    let lifecycle = Parts {
        station,
        arbiter,
        issuer,
        channel,
        terminal_access,
    };
    let storage = RxBlockAckStorage::new(block_acks, Duration::from_millis(100));
    (lifecycle, storage)
}

/// How a case's run ended.
struct Ends<'s> {
    lifecycle: Option<Completed<'s>>,
    actor: Option<ActorEnd<oer_protocols_ieee80211_testing_model::ModelPortFault>>,
}

/// Run the station of `config` with `script` on a fresh model, for as many
/// installations as `script` runs: each script after the first plays the
/// next installation after a restart. Returns the ends of every
/// installation and the installations' generations.
#[allow(clippy::type_complexity)]
fn pair(
    config: impl Fn() -> TestConfig,
    scripts: Vec<Scripted>,
    check: impl for<'s> FnOnce(&[Ends<'s>], &ModelAir),
) {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let clock = Rc::new(VirtualClock::new());
        let mut installations = Installations::claim(claim);
        let (install, air) = ModelPort::install(
            &mut installations,
            channel6(),
            [
                IndividualAddress::new(LOCAL).unwrap(),
                IndividualAddress::new(BSSID).unwrap(),
            ],
            clock.clone(),
        )
        .unwrap();
        let ModelInstall {
            port,
            commands,
            resources,
        } = install;
        let (first, storage) = lifecycle(config(), resources, commands);
        let mut life = Some(first);
        let storage = Box::leak(Box::new(storage));
        let mut actor = Some(Actor::<P<'_>, _, _>::new(
            port,
            EspressifRateSelection::<ModelCaps24>::new(),
            storage.view(),
            (),
        ));
        let mut to_queue = WakingSpscQueue::<ToActor<'_, P<'_>>, TO>::new();
        let mut parcel_queue = WakingSpscQueue::<Parcel<'_, P<'_>>, ACTOR_PARCELS>::new();
        let mut from_queue = WakingSpscQueue::<FromActor<'_, P<'_>>, FROM>::new();
        let mut return_queue = WakingSpscQueue::<Returned<'_, P<'_>>, RETURNS>::new();
        let mut link_queue = WakingSpscQueue::<ActorLink<'_, P<'_>>, 1>::new();
        let mut group_queue =
            WakingSpscQueue::<GroupPending<'_, ActorEpochPath<'_, P<'_>>>, 1>::new();
        let mut link_return_queue = WakingSpscQueue::<LinkReturn, 1>::new();
        let mut transmit_queue = WakingSlotQueue::<Option<Outgoing<Memory>>, TX_DEPTH>::new();
        let mut returned_queue = WakingSlotQueue::<Option<Memory>, TX_DEPTH>::new();
        let mut receive_queue = WakingSpscQueue::<Received<ModelLentRx>, RX_DEPTH>::new();
        let mut copies_queue = WakingSpscQueue::<EthernetFrame, RX_COPIED_DEPTH>::new();
        let mut command_queue =
            WakingSpscQueue::<crate::station::ServiceCommand<ModelCaps24>, COMMANDS>::new();
        let (to_actor, to_actor_end) = to_queue.split();
        let (parcels, parcels_end) = parcel_queue.split();
        let (from_actor_end, from_actor) = from_queue.split();
        let (returns_end, returns) = return_queue.split();
        let (link, link_end) = link_queue.split();
        let (groups_end, groups) = group_queue.split();
        let (link_return_end, link_return) = link_return_queue.split();
        let (stack, transmit_end) = transmit_queue.split(|| None);
        let (returned_end, returned) = returned_queue.split(|| None);
        let (receive_end, delivered) = receive_queue.split();
        let (copies_end, copies) = copies_queue.split();
        let (application, commands) = command_queue.split();
        let state = State::new();
        let (application, control) = Control::new(application, &state).unwrap();
        let mut actor_ends = LifecycleEnds {
            to_actor: to_actor_end,
            parcels: parcels_end,
            from_actor: from_actor_end,
            returns: returns_end,
            groups: groups_end,
            link: link_end,
            link_return: link_return_end,
        };
        let mut lifecycle_ends = ActorEnds {
            to_actor,
            parcels,
            from_actor,
            returns,
            groups,
            link,
            link_return,
        };
        let mut network = NetworkEnds::<_, _, _, TX_DEPTH, RX_DEPTH, SameHart> {
            transmit: transmit_end,
            returned: returned_end,
            receive: receive_end,
            receive_copied: copies_end,
            network: &Network,
        };
        let heard = Rc::new(Heard::default());
        let lifecycle_flag = Arc::new(Flag::default());
        let world = World {
            air: air.clone(),
            clock: clock.clone(),
            heard: heard.clone(),
            control: RefCell::new(Some(application)),
            security: config().1,
            stack: RefCell::new(stack),
            returned: RefCell::new(returned),
            delivered: RefCell::new(delivered),
            copies: RefCell::new(copies),
            lifecycle: lifecycle_flag.clone(),
        };
        let record = {
            let heard = heard.clone();
            move |output: crate::station::ServiceOutput<ModelCaps24>| match output {
                crate::station::ServiceOutput::Notice(notice) => {
                    heard.notices.borrow_mut().push(notice)
                }
                crate::station::ServiceOutput::TxRefused { reason } => {
                    heard.tx_refused.set(heard.tx_refused.get() + 1);
                    heard.refusals.borrow_mut().push(reason);
                }
                crate::station::ServiceOutput::CommandCompleted(_) => {
                    heard.completed.set(heard.completed.get() + 1)
                }
                _ => {}
            }
        };
        let mut output = world.control.borrow().as_ref().unwrap().events(record);
        let mut io = LifecycleIo {
            commands,
            output: &mut output,
            control,
            stop: StopPolicy {
                drain: Duration::from_secs(1),
            },
        };
        let mut ends = Vec::new();
        let mut scripts = scripts.into_iter();
        let mut next = scripts.next();
        while let Some(script) = next.take() {
            let mut actor_now = actor.take().unwrap();
            let completed = RefCell::new(None);
            let actor_end = RefCell::new(None);
            {
                let actor_task = async {
                    *actor_end.borrow_mut() =
                        Some(actor_now.run(&mut actor_ends, &mut network, &*clock).await);
                };
                let lifecycle_task = async {
                    *completed.borrow_mut() = Some(
                        life.take()
                            .unwrap()
                            .run(&mut lifecycle_ends, &mut io, &clock)
                            .await,
                    );
                };
                let script_task = script(&world);
                let mut actor_task = core::pin::pin!(actor_task);
                let mut lifecycle_task = core::pin::pin!(lifecycle_task);
                let mut script_task = script_task;
                let mut tasks = [
                    Task {
                        future: actor_task.as_mut(),
                        flag: Arc::new(Flag::default()),
                        done: false,
                    },
                    Task {
                        future: lifecycle_task.as_mut(),
                        flag: lifecycle_flag.clone(),
                        done: false,
                    },
                    Task {
                        future: script_task.as_mut(),
                        flag: Arc::new(Flag::default()),
                        done: false,
                    },
                ];
                run_tasks(&mut tasks, &clock, &air);
            }
            let completed = completed.into_inner();
            let actor_end = actor_end.into_inner();
            let restart = matches!(
                completed.as_ref().map(|completed| &completed.end),
                Some(LifecycleEnd::Restart(_))
            ) && actor_end == Some(ActorEnd::Reinstall);
            ends.push(Ends {
                lifecycle: completed,
                actor: actor_end,
            });
            next = scripts.next();
            if restart && next.is_some() {
                // The next installation under the same actor, and the
                // service built again on its resources.
                let mut minted = None;
                #[expect(
                    clippy::result_large_err,
                    reason = "the model's refusal returns its port"
                )]
                let reinstalled = actor_now.reinstall(|port| {
                    port.reinstall(&mut installations)
                        .map(|install| {
                            let ModelInstall {
                                port,
                                commands,
                                resources,
                            } = install;
                            minted = Some((commands, resources));
                            port
                        })
                        .map_err(|refused| {
                            let (port, reason) = *refused;
                            (port, reason)
                        })
                });
                let actor_next = match reinstalled {
                    Ok(actor) => actor,
                    Err((_, reason)) => panic!("the stopped port reinstalls: {reason:?}"),
                };
                let (commands, resources) = minted.unwrap();
                let (next_life, storage) = lifecycle(config(), resources, commands);
                // The actor keeps its receive agreements' storage; the new
                // installation's stays owned.
                let _kept = Box::leak(Box::new(storage));
                life = Some(next_life);
                actor = Some(actor_next);
            }
        }
        check(&ends, &air);
    }));
}

fn open() -> TestConfig {
    config(StaSecurity::Open)
}

fn wpa2() -> TestConfig {
    config(StaSecurity::Wpa2Personal {
        pmk: Pmk::from_bytes(PMK),
    })
}

/// An open data frame from the access point, from `source`.
fn open_data(sequence: u16, source: [u8; 6]) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 24];
    frame[0..2].copy_from_slice(&0x0208_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&source);
    frame[22..24].copy_from_slice(&(sequence << 4).to_le_bytes());
    frame.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
    frame.extend_from_slice(&[0xcd; 40]);
    frame
}

fn stopped(ends: &[Ends<'_>]) -> bool {
    matches!(
        ends.last()
            .and_then(|ends| ends.lifecycle.as_ref())
            .map(|completed| &completed.end),
        Some(LifecycleEnd::Stopped)
    ) && ends.last().and_then(|ends| ends.actor.as_ref()) == Some(&ActorEnd::Retired)
}

#[test]
fn an_open_station_joins_through_the_actor_carries_data_both_ways_and_stops() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, false).await;
                world
                    .notice(StaNotice::Connected {
                        bssid: BSSID,
                        channel: 6,
                    })
                    .await;
                // The management frames went to the lifecycle and back.
                world.settle().await;
                assert_eq!(world.air.pending_rx(), 0, "every frame recycled");
                // TX: the network's MSDU goes on the link.
                let destination = [0x02, 0, 0, 0, 0, 0x07];
                assert!(world.send_msdu(destination));
                let data = world.sent(&mut seen, DATA).await;
                assert_eq!(data.plaintext[4..10], BSSID);
                assert_eq!(data.plaintext[16..22], destination);
                assert_eq!(data.key, None);
                // RX: the access point's data reaches the network.
                world
                    .air
                    .receive(&open_data(1, [0x02, 0, 0, 0, 0, 0x08]), meta());
                world.settle().await;
                assert_eq!(world.delivered(), 1);
                world.stop().await;
            })
        })],
        |ends, air| {
            assert!(stopped(ends), "an ordinary stop retires the actor");
            assert!(
                ends[0]
                    .lifecycle
                    .as_ref()
                    .unwrap()
                    .retired
                    .rejected
                    .is_none()
            );
            assert_eq!(air.pending_rx(), 0);
        },
    );
}

#[test]
fn a_wpa2_station_handshakes_through_the_actor_and_its_pairwise_key_comes_back_at_the_end() {
    pair(
        wpa2,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                let _ptk = world.handshake(&mut seen).await;
                // The link's data goes under the pairwise key the actor
                // holds.
                let destination = [0x02, 0, 0, 0, 0, 0x07];
                assert!(world.send_msdu(destination));
                let first = world.sent(&mut seen, DATA).await;
                assert!(matches!(first.key, Some((_, true))), "{first:?}");
                assert!(world.send_msdu(destination));
                let second = world.sent(&mut seen, DATA).await;
                assert_eq!(
                    second.packet_number,
                    first.packet_number.map(|number| number + 1)
                );
                // RX: a decrypted MPDU reaches the stack; its replay does
                // not.
                let source = [0x02, 0, 0, 0, 0, 0x08];
                world
                    .air
                    .receive(&protected_data(1, source, &[0xcd; 40]), decrypted());
                world
                    .air
                    .receive(&protected_data(1, source, &[0xcd; 40]), decrypted());
                world.settle().await;
                assert_eq!(world.delivered(), 1);
                // The disconnection: the link comes back, the station's
                // Deauthentication goes under the key the actor kept, then
                // the key comes back for its removal.
                world.station_command(StaCommand::Disconnect).await;
                let deauthentication = world.sent(&mut seen, DEAUTHENTICATION).await;
                assert_eq!(deauthentication.plaintext[4..10], BSSID);
                world
                    .notice(StaNotice::Disconnected(StaDisconnectReason::LocalRequest))
                    .await;
                world.settle().await;
                world.stop().await;
            })
        })],
        |ends, air| {
            assert!(stopped(ends));
            assert!(
                ends[0]
                    .lifecycle
                    .as_ref()
                    .unwrap()
                    .retired
                    .rejected
                    .is_none()
            );
            assert_eq!(air.pending_rx(), 0);
        },
    );
}

/// `config` with a second channel the scan visits.
fn two_channels(mut config: TestConfig) -> TestConfig {
    let channel1 = SupportedChannel::new(
        oer_protocols_radio_regulatory::TEST_DOMAIN
            .permit(
                oer_protocols_ieee80211_contracts_port::RegulatedChannel::from(
                    oer_protocols_ieee80211_contracts_port::Channel::ghz2_4(
                        1,
                        oer_protocols_ieee80211_contracts_port::ChannelWidth::Mhz20,
                    )
                    .unwrap(),
                ),
            )
            .unwrap(),
    )
    .unwrap();
    config.0.channels.push(channel1).unwrap();
    config
}

#[test]
fn a_station_whose_radio_cannot_move_restarts_on_the_next_installation_without_a_reset() {
    pair(
        || two_channels(open()),
        std::vec![
            script(|world| {
                Box::pin(async move {
                    let mut seen = 0;
                    world.station_command(StaCommand::Connect).await;
                    world.sent(&mut seen, PROBE_REQUEST).await;
                    // The scan's move to the next channel and the return
                    // home are refused: the arbiter fails the station.
                    world.air.refuse_enables(2);
                })
            }),
            script(|world| {
                Box::pin(async move {
                    // The next installation's service joins on the same
                    // air and carries data.
                    let mut seen = world.air.sent().len();
                    world.associate(&mut seen, false).await;
                    world
                        .notice(StaNotice::Connected {
                            bssid: BSSID,
                            channel: 6,
                        })
                        .await;
                    assert!(world.send_msdu([0x02, 0, 0, 0, 0, 0x07]));
                    world.sent(&mut seen, DATA).await;
                    world.stop().await;
                })
            }),
        ],
        |ends, air| {
            assert_eq!(ends.len(), 2);
            let first = ends[0].lifecycle.as_ref().unwrap();
            assert!(
                matches!(
                    first.end,
                    LifecycleEnd::Restart(StationFailure::RetuneRefused(_))
                ),
                "{:?}",
                first.end
            );
            assert!(!first.end.is_terminal(), "a failed station is no reset");
            assert_eq!(ends[0].actor, Some(ActorEnd::Reinstall));
            assert!(first.retired.rejected.is_none());
            assert!(stopped(&ends[1..]));
            assert_eq!(air.pending_rx(), 0);
        },
    );
}

#[test]
fn steady_transmission_does_not_wake_the_lifecycle_per_ppdu() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, false).await;
                world
                    .notice(StaNotice::Connected {
                        bssid: BSSID,
                        channel: 6,
                    })
                    .await;
                world.settle().await;
                let wakes = world.lifecycle_wakes();
                let before = world
                    .air
                    .sent()
                    .iter()
                    .filter(|frame| kind(frame) == DATA)
                    .count();
                for _ in 0..20 {
                    assert!(world.send_msdu([0x02, 0, 0, 0, 0, 0x07]));
                    world.settle().await;
                    world.clock.advance(Duration::from_micros(500)).unwrap();
                    world.air.clock_advanced();
                    world.settle().await;
                }
                let data = world
                    .air
                    .sent()
                    .iter()
                    .filter(|frame| kind(frame) == DATA)
                    .count();
                assert_eq!(data - before, 20, "every MSDU went");
                let woken = world.lifecycle_wakes() - wakes;
                // The first MSDU tells the TID's traffic once; the later
                // ones within the refresh span tell nothing.
                assert!(woken <= 2, "{woken} lifecycle wakes for 20 PPDUs");
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

#[test]
fn message4_waits_for_the_group_installations_terminal_through_the_actor() {
    pair(
        wpa2,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                let ptk = world.start_handshake(&mut seen).await;
                let before = world.air.sent().len();
                world.air.hold_group_commands(true);
                world.air.receive(&message3(&ptk), meta());
                world.settle().await;
                assert_eq!(world.air.group_writes(), 0);
                assert_eq!(world.air.sent().len(), before, "no M4 before the terminal");
                assert!(world.heard.notices.borrow().is_empty());
                world.air.hold_group_commands(false);
                let message4 = world.sent(&mut seen, DATA).await;
                assert_eq!(message4.key, None);
                assert_eq!(world.air.group_writes(), 1);
                world
                    .notice(StaNotice::Connected {
                        bssid: BSSID,
                        channel: 6,
                    })
                    .await;
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

#[test]
fn a_group_rekey_installs_by_its_terminal_and_answers_under_the_links_key() {
    pair(
        wpa2,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                let ptk = world.handshake(&mut seen).await;
                assert_eq!(world.air.group_writes(), 1);
                world
                    .air
                    .receive(&protected_group_message1(&ptk, 3, 2), decrypted());
                let response = world.sent(&mut seen, DATA).await;
                assert!(matches!(response.key, Some((_, true))), "{response:?}");
                assert!(response.packet_number.is_some());
                assert_eq!(world.air.group_writes(), 2);
                world.stop().await;
            })
        })],
        |ends, _| {
            assert!(stopped(ends));
            assert!(
                ends[0]
                    .lifecycle
                    .as_ref()
                    .unwrap()
                    .retired
                    .rejected
                    .is_none()
            );
        },
    );
}

#[test]
fn the_actor_numbers_the_stations_frames_and_the_data_of_one_tid_without_a_gap() {
    use crate::station::tests::{QOS_DATA, WMM, carries_eapol, qos_sequence};
    pair(
        wpa2,
        std::vec![script(|world| {
            Box::pin(async move {
                // Voice is closed, so the station's EAPOL goes as Video, in
                // TID 5, as data of priority 5 does.
                let mut wmm = WMM;
                wmm[22] |= 0x10;
                let mut beacon = beacon(true);
                beacon.extend_from_slice(&wmm);
                let mut seen = 0;
                world.associate_with(&mut seen, beacon, true).await;
                let ptk = world.handshake(&mut seen).await;
                assert!(world.send_msdu_at([2, 0, 0, 0, 0, 9], 5));
                let first = world.sent(&mut seen, QOS_DATA).await;
                assert!(!carries_eapol(&first));
                world
                    .air
                    .receive(&protected_group_message1(&ptk, 3, 2), decrypted());
                let response = world.sent(&mut seen, QOS_DATA).await;
                assert!(carries_eapol(&response));
                assert!(world.send_msdu_at([2, 0, 0, 0, 0, 9], 5));
                let second = world.sent(&mut seen, QOS_DATA).await;
                assert!(!carries_eapol(&second));
                for frame in [&first, &response, &second] {
                    assert_eq!(frame.plaintext[24] & 0x0f, 5, "TID 5");
                    assert!(matches!(frame.key, Some((_, true))), "{frame:?}");
                }
                assert_eq!(qos_sequence(&response), qos_sequence(&first) + 1);
                assert_eq!(qos_sequence(&second), qos_sequence(&response) + 1);
                let numbers =
                    [&first, &response, &second].map(|frame| frame.packet_number.unwrap());
                assert!(
                    numbers.windows(2).all(|pair| pair[0] < pair[1]),
                    "{numbers:?}"
                );
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

#[test]
fn a_receive_agreement_answers_through_the_station_and_reorders_in_the_actor() {
    use crate::station::tests::block_ack::{addba, data, ht_beacon, ht_config, response};
    use oer_protocols_ieee80211_wire::block_ack::frame::BlockAckAction;
    pair(
        ht_config,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate_with(&mut seen, ht_beacon(), false).await;
                world
                    .notice(StaNotice::Connected {
                        bssid: BSSID,
                        channel: 6,
                    })
                    .await;
                world.air.receive(&addba(7, 0, 4094, 0), meta());
                let ack = world.sent(&mut seen, 0xd0).await;
                assert!(matches!(
                    response(&ack),
                    BlockAckAction::AddbaResponse {
                        dialog_token: 7,
                        status: 0,
                        tid: 0,
                        ..
                    }
                ));
                assert_eq!(world.air.receive_agreements().len(), 1);
                world.air.receive(&data(4095, 0, 2), meta());
                world.settle().await;
                assert_eq!(world.delivered(), 0, "the hole holds the later frame");
                world.air.receive(&data(4094, 0, 1), meta());
                world.settle().await;
                assert_eq!(world.delivered(), 2);
                world.stop().await;
            })
        })],
        |ends, air| {
            assert!(stopped(ends));
            assert_eq!(air.pending_rx(), 0);
        },
    );
}

#[test]
fn a_transmit_agreement_names_the_exact_next_number_and_ends_idle() {
    use crate::station::tests::block_ack::{
        addba_response, ht_beacon, ht_config, response, tx_block_ack_config,
    };
    use oer_protocols_ieee80211_wire::block_ack::frame::BlockAckAction;
    pair(
        || tx_block_ack_config(ht_config()),
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate_with(&mut seen, ht_beacon(), false).await;
                world
                    .notice(StaNotice::Connected {
                        bssid: BSSID,
                        channel: 6,
                    })
                    .await;
                assert!(world.send_msdu(BSSID));
                let request = world.sent(&mut seen, 0xd0).await;
                let BlockAckAction::AddbaRequest {
                    dialog_token,
                    tid: 0,
                    starting_sequence,
                    ..
                } = response(&request)
                else {
                    panic!("an ADDBA Request of TID 0: {:?}", response(&request));
                };
                // The MSDU that started it took Sequence Number 0.
                assert_eq!(starting_sequence.get(), 1);
                world
                    .air
                    .receive(&addba_response(dialog_token, 0, 32), meta());
                // The next MSDU takes the number the request named.
                assert!(world.send_msdu(BSSID));
                let next = world.sent(&mut seen, crate::station::tests::QOS_DATA).await;
                assert_eq!(crate::station::tests::qos_sequence(&next), 1);
                // No more data: the idle agreement ends with a DELBA.
                let delba = world.sent(&mut seen, 0xd0).await;
                assert!(
                    matches!(response(&delba), BlockAckAction::Delba { tid: 0, .. }),
                    "{:?}",
                    response(&delba)
                );
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

#[test]
fn a_station_frame_the_port_refuses_comes_back_withdrawn_and_the_station_goes_on() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, false).await;
                world
                    .notice(StaNotice::Connected {
                        bssid: BSSID,
                        channel: 6,
                    })
                    .await;
                world.settle().await;
                // The port closes its admission: the Deauthentication ends
                // withdrawn at its submission, and the station hears it as
                // any withdrawn frame's end.
                world.air.fail_after_terminals();
                let before = world.air.sent().len();
                world.station_command(StaCommand::Disconnect).await;
                world
                    .notice(StaNotice::Disconnected(StaDisconnectReason::LocalRequest))
                    .await;
                assert_eq!(world.air.sent().len(), before, "nothing went on the air");
                // Its retry, numbered on the link the station gave back
                // meanwhile, is refused before the port as before.
                assert!(
                    world
                        .heard
                        .refusals
                        .borrow()
                        .iter()
                        .all(|reason| *reason
                            == crate::station::contract::TransmitRefusal::LinkChanged),
                    "{:?}",
                    world.heard.refusals.borrow()
                );
                world.stop().await;
            })
        })],
        |ends, _| {
            let completed = ends[0].lifecycle.as_ref().unwrap();
            assert!(completed.retired.rejected.is_none());
            assert!(
                !matches!(
                    completed.end,
                    LifecycleEnd::Contract(_) | LifecycleEnd::Faulted(_)
                ),
                "{:?}",
                completed.end
            );
        },
    );
}
