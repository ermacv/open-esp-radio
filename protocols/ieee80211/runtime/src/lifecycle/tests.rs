//! The lifecycle and the actor joined over the model's data port, the test
//! playing the access point, the network stack and the application: the
//! join, the handshake's EAPOL through the actor, the station's frames on
//! the link's numbers, the data both ways without the lifecycle, the
//! group rekey, the end and the restart on the next installation.

use super::*;
use core::cell::{Cell, RefCell};
use core::convert::Infallible;
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
    GroupCapacity, StaConfig, StaSecurity, StationResources,
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
    authentication_response, beacon, channel6, config, decrypted, eapol_frame, group_message1,
    kind, message3, meta, network_frame, protected_data,
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
        self.station_command(StaCommand::Connect).await;
        self.sent(seen, PROBE_REQUEST).await;
        self.air.receive(&beacon(protected), meta());
        let authentication = self.sent(seen, AUTHENTICATION).await;
        let response = authentication_response(&authentication.plaintext).unwrap();
        self.air.receive(&response, meta());
        self.sent(seen, ASSOCIATION_REQUEST).await;
        self.air.receive(&association_response(protected), meta());
    }

    async fn handshake(&self, seen: &mut usize) -> oer_protocols_ieee80211_core_security_rsn::Ptk {
        let message1 = RsnTxFrame::<512>::message1(Akm::Psk, LOCAL, 1, ANONCE).unwrap();
        self.air.receive(&eapol_frame(message1.as_bytes()), meta());
        self.sent(seen, DATA).await;
        let ptk = Pmk::from_bytes(PMK).derive_ptk(
            Akm::Psk,
            PtkContext {
                authenticator_address: BSSID,
                supplicant_address: LOCAL,
                authenticator_nonce: ANONCE,
                supplicant_nonce: SNONCE,
            },
        );
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

    fn send_msdu(&self, destination: [u8; 6]) -> bool {
        while let Ok(Some(_memory)) = self.returned.borrow_mut().try_receive_with(Option::take) {}
        let packet = network_frame(
            oer_protocols_ieee80211_wire::data::MsduParts {
                destination,
                source: LOCAL,
                ether_type: 0x0800,
                payload: &[0xab; 100],
            },
            WmmUserPriority::new(0).unwrap(),
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
fn lifecycle<'s>(
    config: TestConfig,
    resources: ModelResources<'s, Backend<'s>>,
    issuer: Issuer<'s>,
) -> (TestLifecycle<'s>, RxBlockAckStorage<'s, Types<'s>, 2, 4>) {
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
    let lifecycle = Lifecycle::new(
        station,
        arbiter,
        issuer,
        CcmpPacketNumberStep::ONE,
        channel,
        terminal_access,
        (),
    );
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
                crate::station::ServiceOutput::TxRefused { .. } => {
                    heard.tx_refused.set(heard.tx_refused.get() + 1)
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
                            .run(&mut lifecycle_ends, &mut io, &*clock)
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
