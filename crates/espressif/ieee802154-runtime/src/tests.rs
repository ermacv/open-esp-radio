//! Runtime behavior over the HAL register model, through the port's owner
//! shape: [`IRuntime::install`] returns the backend, and
//! `run_port` runs a body over it with its joined driver. Covered here are
//! the lock, owned events, the bounded queue, the operation permit, the
//! driver's backoffs, enhanced-ACK settings and the port's end. The engine's
//! vendor sequences are covered by the engine tests and the host stand.

use std::{boxed::Box, vec::Vec};

use core::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};

use embassy_futures::{block_on, yield_now};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_esp32s31_hal::coex::{CoexEventId, CoexPti, CoexPtiTable};
use oer_esp32s31_hal::ieee802154::coex::{Ieee802154CoexConfig, resolve_priorities};
use oer_espressif_ieee802154_engine::engine::{
    Ieee802154Engine, Ieee802154EngineBuffers, Ieee802154Interfaces, PENDING_TABLE_SIZE,
};
use oer_espressif_ieee802154_engine::pib::Ieee802154PibDefaults;
use oer_espressif_ieee802154_engine::{
    coex::Ieee802154Coexistence,
    ll::{Ieee802154LlCommand, model::Ieee802154LlModel},
    tx_power::Ieee802154TxPowerLevels,
    types::{Ieee802154Event, Ieee802154TxAbortReason, Ieee802154TxAbortReasonObservation},
};
use oer_ieee802154::{
    Channel, CslReceiver, EnhancedAckGeneration, FrameAddress, FrameCounterUpdate, FrameView,
    Ieee802154Capabilities, Interface, LinkMetrics, ProbingInitiator, RadioCapabilities,
    RadioFault, RadioState, RestingState, TxMode, TxSecurity, TxStatus,
    owner::{
        Has, Ieee802154Caps, Ieee802154Client, Ieee802154Clients, Ieee802154Setup,
        InterfaceResources, Operation, RadioSettings, RecentRssi, SupportedHeaderIes,
        SupportedTransmit, Transmission,
    },
};
use oer_ieee802154_port::{
    CancelError, Disabled, Ended, Owner, PortBody, RadioPortClient, RadioPortEvents, run_port,
};
use oer_radio_port::{EventsLost, Mint, Poisoned, Seat};
use oer_time::Duration;
use oer_time_virtual::VirtualClock;

use super::{
    IEEE802154_CSL_CAPABILITY, IEEE802154_ENHANCED_ACK_IE_CAPACITY, IEEE802154_RADIO_CAPABILITIES,
    Ieee802154PortEvent, Ieee802154RadioEvent, Ieee802154Random, Ieee802154Runtime,
    Ieee802154RuntimeBackend, Ieee802154RuntimeCaps, Ieee802154RuntimeParts,
    Ieee802154TxRxStatistics, RuntimeClient, RuntimeControl, RuntimeEvents, RuntimeResources,
};

static LEVELS: [i8; 1] = [0];

/// A random source that always draws 21.
struct FixedRandom;

impl Ieee802154Random for FixedRandom {
    fn random(&mut self) -> u32 {
        21
    }
}

const fn capabilities(operations: RadioCapabilities, interfaces: u8) -> Ieee802154Capabilities {
    Ieee802154Capabilities {
        operations,
        interfaces,
        transmit_power: None,
        cca_threshold: None,
        receive_sensitivity_dbm: -100,
        csl: Some(IEEE802154_CSL_CAPABILITY),
        enhanced_ack_header_ies: IEEE802154_ENHANCED_ACK_IE_CAPACITY as u8,
    }
}

/// The capabilities of a single-interface runtime.
enum Single {}

impl Ieee802154Caps for Single {
    const CAPS: Ieee802154Capabilities = capabilities(IEEE802154_RADIO_CAPABILITIES, 1);
}

impl Ieee802154RuntimeCaps for Single {
    type Interfaces<'id> = InterfaceResources<'id, 0, PENDING_TABLE_SIZE>;

    fn interfaces<'id>(mint: &mut Mint<'id>) -> Self::Interfaces<'id> {
        InterfaceResources::mint::<Self>(mint)
    }
}

/// The capabilities of a two-interface multi-PAN runtime.
enum Multipan {}

impl Ieee802154Caps for Multipan {
    const CAPS: Ieee802154Capabilities = capabilities(
        IEEE802154_RADIO_CAPABILITIES.union(RadioCapabilities::MULTI_PAN),
        2,
    );
}

impl Ieee802154RuntimeCaps for Multipan {
    type Interfaces<'id> = (
        InterfaceResources<'id, 0, PENDING_TABLE_SIZE>,
        InterfaceResources<'id, 1, PENDING_TABLE_SIZE>,
    );

    fn interfaces<'id>(mint: &mut Mint<'id>) -> Self::Interfaces<'id> {
        (
            InterfaceResources::mint::<Self>(mint),
            InterfaceResources::mint::<Self>(mint),
        )
    }
}

/// 2006 data frame without an ACK request, as MAC bytes.
const MAC: [u8; 10] = [0x41, 0x98, 0x01, 0x34, 0x12, 0xff, 0xff, 0x78, 0x56, 0xaa];

/// `MAC` as the receive DMA writes it, RSSI -55 and LQI 180 after it.
fn received_image() -> [u8; 13] {
    let mut image = [0; 13];
    image[0] = 12;
    image[1..11].copy_from_slice(&MAC);
    image[11] = -55i8 as u8;
    image[12] = 180;
    image
}

fn channel(number: u8) -> Channel {
    Channel::new(number).unwrap()
}

type Runtime<const EVENTS: usize> =
    Ieee802154Runtime<'static, NoopRawMutex, Ieee802154LlModel, VirtualClock, FixedRandom, EVENTS>;

type Backend<K, const EVENTS: usize> = Ieee802154RuntimeBackend<
    'static,
    'static,
    K,
    NoopRawMutex,
    Ieee802154LlModel,
    VirtualClock,
    FixedRandom,
    EVENTS,
>;

type Parts = Ieee802154RuntimeParts<'static, Ieee802154LlModel>;

type Resources<'id, K, const EVENTS: usize> =
    RuntimeResources<'static, 'id, <K as Ieee802154RuntimeCaps>::Interfaces<'id>, Runtime<EVENTS>>;

fn parts() -> Parts {
    let buffers = Box::leak(Box::new(Ieee802154EngineBuffers::new()));
    let levels = Ieee802154TxPowerLevels::new(&LEVELS).unwrap();
    Ieee802154RuntimeParts {
        engine: Ieee802154Engine::new(buffers, levels, Ieee802154PibDefaults::default()),
        hardware: Ieee802154LlModel::default(),
    }
}

/// A new runtime for the test's whole life.
fn runtime<const EVENTS: usize>() -> &'static Runtime<EVENTS> {
    Box::leak(Box::new(Runtime::new(VirtualClock::new())))
}

/// Install `parts` in `runtime`: the backend of a disabled radio.
fn install<K, const EVENTS: usize>(
    runtime: &'static Runtime<EVENTS>,
    parts: Parts,
) -> Backend<K, EVENTS> {
    let Ok(backend) = runtime.install(parts, FixedRandom, Ieee802154PibDefaults::default()) else {
        panic!("the runtime installs");
    };
    backend
}

/// A port body written as an async closure.
struct Body<F>(F);

impl<K, const EVENTS: usize, F> PortBody<Backend<K, EVENTS>> for Body<F>
where
    K: Ieee802154RuntimeCaps,
    F: for<'p, 'id> AsyncFnOnce(
        Owner<'p, 'id, Backend<K, EVENTS>, Disabled>,
        Resources<'id, K, EVENTS>,
    ) -> Ended<'id, Backend<K, EVENTS>>,
{
    fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Backend<K, EVENTS>, Disabled>,
        resources: Resources<'id, K, EVENTS>,
    ) -> impl Future<Output = Ended<'id, Backend<K, EVENTS>>> {
        (self.0)(owner, resources)
    }
}

/// Run `body` over `backend` to its uninstall and return the parts.
fn run<K, const EVENTS: usize>(
    backend: Backend<K, EVENTS>,
    body: impl for<'p, 'id> AsyncFnOnce(
        Owner<'p, 'id, Backend<K, EVENTS>, Disabled>,
        Resources<'id, K, EVENTS>,
    ) -> Ended<'id, Backend<K, EVENTS>>,
) -> Parts
where
    K: Ieee802154RuntimeCaps,
{
    block_on(run_port(backend, Body(body))).expect("the body uninstalls")
}

/// One running phase of a single-interface port, as [`enabled`] lends it.
struct Phase<'s, 'a, 'id, const EVENTS: usize> {
    runtime: &'static Runtime<EVENTS>,
    events: &'s mut RuntimeEvents<'a, 'id, Runtime<EVENTS>>,
    control: &'s mut RuntimeControl<'a, Runtime<EVENTS>, Single>,
    client: &'s RuntimeClient<'s, 'id, Runtime<EVENTS>, Single, 0>,
    seat: &'s mut Seat<'id, Operation>,
}

/// Install a single-interface radio in a new runtime, configure its
/// interface, enable it and run `scenario` in one phase; then disable and
/// uninstall it. Returns the runtime, the parts and the terminals the
/// disable handed out.
fn enabled<const EVENTS: usize>(
    scenario: impl for<'s, 'a, 'id> AsyncFnOnce(Phase<'s, 'a, 'id, EVENTS>),
) -> (&'static Runtime<EVENTS>, Parts, Vec<Ieee802154PortEvent>) {
    enabled_with(parts(), scenario)
}

fn enabled_with<const EVENTS: usize>(
    parts: Parts,
    scenario: impl for<'s, 'a, 'id> AsyncFnOnce(Phase<'s, 'a, 'id, EVENTS>),
) -> (&'static Runtime<EVENTS>, Parts, Vec<Ieee802154PortEvent>) {
    let runtime = runtime::<EVENTS>();
    let mut leftovers = Vec::new();
    let parts = run(
        install::<Single, EVENTS>(runtime, parts),
        async |mut owner, resources| {
            let RuntimeResources {
                interfaces,
                mut operation,
                ..
            } = resources;
            let configured = owner.setup().configure(interfaces.interface);
            let mut owner = enabled_owner(owner.enable().await);
            {
                let mut phase = owner.split();
                let client = phase.clients.client(&configured);
                scenario(Phase {
                    runtime,
                    events: &mut phase.events,
                    control: &mut phase.control,
                    client: &client,
                    seat: &mut operation[0],
                })
                .await;
            }
            let owner = disabled_owner(owner.disable(&mut |event| leftovers.push(event)).await);
            owner.uninstall(())
        },
    );
    (runtime, parts, leftovers)
}

/// The enabled owner; a failed enable fail-stops the port and panics.
fn enabled_owner<'p, 'id, K: Ieee802154RuntimeCaps, const EVENTS: usize>(
    enabled: Result<
        Owner<'p, 'id, Backend<K, EVENTS>, oer_ieee802154_port::Enabled>,
        oer_ieee802154_port::Failed<'p, 'id, Backend<K, EVENTS>>,
    >,
) -> Owner<'p, 'id, Backend<K, EVENTS>, oer_ieee802154_port::Enabled> {
    match enabled {
        Ok(owner) => owner,
        Err(oer_ieee802154_port::Failed::Poisoned(poisoned)) => {
            let cause = poisoned.cause();
            let _ = poisoned.fail_stop();
            panic!("the enable found the port poisoned: {cause:?}");
        }
        Err(failed) => {
            let _ = failed.fail_stop();
            panic!("the radio refused the enable");
        }
    }
}

/// The disabled owner; a poisoned disable fail-stops the port and panics.
fn disabled_owner<'p, 'id, K: Ieee802154RuntimeCaps, const EVENTS: usize>(
    disabled: Result<
        Owner<'p, 'id, Backend<K, EVENTS>, Disabled>,
        oer_ieee802154_port::PoisonedOwner<'p, 'id, Backend<K, EVENTS>>,
    >,
) -> Owner<'p, 'id, Backend<K, EVENTS>, Disabled> {
    match disabled {
        Ok(owner) => owner,
        Err(poisoned) => {
            let cause = poisoned.cause();
            let _ = poisoned.fail_stop();
            panic!("the disable found the port poisoned: {cause:?}");
        }
    }
}

/// Poll `future` once, as an executor does after a wake.
fn poll_once<F: Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

/// Drive `future`, which the runtime answers at once, to its end.
fn ready<F: Future>(future: F) -> F::Output {
    match poll_once(pin!(future)) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the runtime answers at once"),
    }
}

/// Take the next event, which is already queued.
fn next<const EVENTS: usize>(
    events: &mut RuntimeEvents<'_, '_, Runtime<EVENTS>>,
) -> Result<Ieee802154PortEvent, EventsLost> {
    ready(events.next_event()).unwrap_or_else(|_| panic!("not poisoned"))
}

/// The radio state of the installed radio.
fn state<const EVENTS: usize>(runtime: &Runtime<EVENTS>) -> RadioState {
    runtime.with_installed(|radio, _, _| radio.state())
}

fn transmission(frame: &[u8], mode: TxMode, retries: u8) -> SupportedTransmit<'_, Single> {
    SupportedTransmit::new(Transmission {
        frame: FrameView::new(frame).unwrap(),
        channel: channel(20),
        mode,
        power: None,
        max_frame_retries: retries,
        security: TxSecurity::default(),
        time_sync: None,
    })
    .unwrap_or_else(|_| panic!("the runtime performs the transmission"))
}

fn received(event: &Ieee802154PortEvent) -> bool {
    event.interface.is_none() && matches!(event.event, Ieee802154RadioEvent::Received(_))
}

/// A transmission's terminal for interface 0.
fn transmit_done(id: oer_ieee802154::RequestId, status: TxStatus) -> Ieee802154PortEvent {
    Ieee802154PortEvent {
        interface: Some(Interface::PRIMARY),
        event: Ieee802154RadioEvent::TransmitDone {
            id,
            status,
            acknowledgement: None,
            security: None,
        },
    }
}

/// An installed radio is disabled until its owner enables it, and a second
/// install is refused while it is installed; enabled, it sleeps.
#[test]
fn a_radio_is_installed_once_and_sleeps_once_enabled() {
    let runtime = runtime::<4>();
    let backend = install::<Single, 4>(runtime, parts());
    assert!(
        runtime
            .install::<Single>(parts(), FixedRandom, Ieee802154PibDefaults::default())
            .is_err()
    );
    assert_eq!(state(runtime), RadioState::Disabled);
    run(backend, async |owner, _| {
        let owner = enabled_owner(owner.enable().await);
        assert_eq!(state(runtime), RadioState::Resting(RestingState::Sleeping));
        let owner = disabled_owner(owner.disable(&mut |_| panic!("nothing ran")).await);
        assert_eq!(state(runtime), RadioState::Disabled);
        owner.uninstall(())
    });
    // The uninstalled runtime installs again.
    let _ = install::<Single, 4>(runtime, parts());
}

#[test]
fn a_received_frame_arrives_as_an_owned_portable_event() {
    enabled::<4>(async |phase| {
        ready(phase.client.receive(channel(15))).unwrap();
        let runtime = phase.runtime;
        runtime.model_interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
        let Ok(Ieee802154PortEvent {
            interface: None,
            event: Ieee802154RadioEvent::Received(frame),
        }) = next(phase.events)
        else {
            panic!("a frame was received");
        };
        assert_eq!(frame.frame.as_bytes(), MAC);
        assert_eq!(frame.metadata.channel, channel(15));
        assert_eq!(frame.metadata.rssi_dbm, -55);
        assert_eq!(frame.metadata.link_quality, 180);
    });
}

/// A transmission's terminal names its interface, and its permit is back
/// before the terminal is handed out.
#[test]
fn a_transmission_completes_through_the_event_queue() {
    enabled::<4>(async |phase| {
        let runtime = phase.runtime;
        let permit = ready(phase.client.operation(phase.seat));
        let work = phase
            .client
            .transmit(permit, transmission(&MAC, TxMode::Direct, 0))
            .unwrap();
        let id = work.id();
        // The one permit is out.
        assert!(poll_once(pin!(phase.client.operation(phase.seat))).is_pending());
        runtime.model_interrupt(None, &[Ieee802154Event::TxDone]);
        assert_eq!(next(phase.events), Ok(transmit_done(id, TxStatus::Success)));
        assert!(poll_once(pin!(phase.client.operation(phase.seat))).is_ready());
        assert_eq!(state(runtime), RadioState::Resting(RestingState::Sleeping));
    });
}

/// A client's addressing applies while its transmission runs, as the
/// vendor's setters do, and the port stays usable.
#[test]
fn a_configuration_during_an_operation_applies_without_poisoning() {
    enabled::<4>(async |phase| {
        let runtime = phase.runtime;
        let permit = ready(phase.client.operation(phase.seat));
        let id = phase
            .client
            .transmit(permit, transmission(&MAC, TxMode::Direct, 0))
            .unwrap()
            .id();
        phase.client.set_pan_id(0x1234);
        phase.client.set_short_address(0x0042);
        phase.control.set_promiscuous(Has::from_const(), true);
        assert_eq!(
            runtime.with_model(|hardware| (hardware.panid[0], hardware.short_address[0])),
            (0x1234, 0x0042)
        );
        runtime.model_interrupt(None, &[Ieee802154Event::TxDone]);
        assert_eq!(next(phase.events), Ok(transmit_done(id, TxStatus::Success)));
        ready(phase.client.receive(channel(11))).unwrap();
    });
}

/// Overflow reports the loss once, after the events queued before it.
#[test]
fn an_overflowing_queue_reports_the_loss_once() {
    // Of four slots, the fault of the enabled period holds one and the next
    // operation's terminal keeps one free: two frames fit.
    enabled::<4>(async |phase| {
        let runtime = phase.runtime;
        ready(phase.client.receive(channel(11))).unwrap();
        for _ in 0..4 {
            runtime.model_interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
        }
        for _ in 0..2 {
            assert!(received(&next(phase.events).unwrap()));
        }
        assert_eq!(next(phase.events), Err(EventsLost));
        // Events after the gap follow the marker.
        runtime.model_interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
        assert!(received(&next(phase.events).unwrap()));
    });
}

/// The enabled port keeps the slot of the fault that may end it: a queue
/// full of received frames still delivers the fault, which poisons the
/// port after the events before it.
#[test]
fn a_fault_reaches_a_queue_full_of_frames_and_poisons_the_port() {
    let runtime = runtime::<4>();
    run(
        install::<Single, 4>(runtime, parts()),
        async |mut owner, resources| {
            let configured = owner.setup().configure(resources.interfaces.interface);
            let mut owner = enabled_owner(owner.enable().await);
            {
                let mut phase = owner.split();
                let client = phase.clients.client(&configured);
                ready(client.receive(channel(11))).unwrap();
                for _ in 0..3 {
                    runtime.model_interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
                }
                // The role reports a broken event sequence through the sink.
                assert!(runtime.events.push(Ieee802154RadioEvent::Fault {
                    id: None,
                    fault: RadioFault::InvalidEventSequence,
                }));
                for _ in 0..2 {
                    assert!(received(&next(&mut phase.events).unwrap()));
                }
                assert_eq!(next(&mut phase.events), Err(EventsLost));
                assert!(matches!(
                    ready(phase.events.next_event()),
                    Err(Poisoned {
                        cause: RadioFault::InvalidEventSequence
                    })
                ));
                assert!(matches!(
                    ready(client.receive(channel(11))),
                    Err(Poisoned { .. })
                ));
            }
            let poisoned = match owner.disable(&mut |_| {}).await {
                Err(poisoned) => poisoned,
                Ok(owner) => {
                    let _ = owner.fail_stop();
                    panic!("the port is poisoned");
                }
            };
            assert_eq!(poisoned.cause(), RadioFault::InvalidEventSequence);
            poisoned.uninstall(())
        },
    );
}

/// The disable hands the queued frames to its sink, and the uninstall ends
/// the port's stream: the next install starts empty ().
#[test]
fn disable_hands_out_the_queued_frames_and_the_next_install_starts_empty() {
    let (runtime, parts, leftovers) = enabled::<4>(async |phase| {
        ready(phase.client.receive(channel(11))).unwrap();
        phase
            .runtime
            .model_interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
    });
    assert!(matches!(leftovers.as_slice(), [frame] if received(frame)));
    let _backend = install::<Single, 4>(runtime, parts);
    assert!(runtime.events.take().is_none());
}

/// The disable ends the transmission in flight with its aborted terminal,
/// takes its permit back, and the enabled port that follows admits an
/// operation at once.
#[test]
fn disable_ends_the_operation_in_flight_and_enable_admits_again() {
    let runtime = runtime::<4>();
    run(
        install::<Single, 4>(runtime, parts()),
        async |mut owner, resources| {
            let RuntimeResources {
                interfaces,
                mut operation,
                ..
            } = resources;
            let configured = owner.setup().configure(interfaces.interface);
            let mut owner = enabled_owner(owner.enable().await);
            let id = {
                let phase = owner.split();
                let client = phase.clients.client(&configured);
                let permit = ready(client.operation(&mut operation[0]));
                client
                    .transmit(permit, transmission(&MAC, TxMode::Direct, 0))
                    .unwrap()
                    .id()
            };
            let mut leftovers = Vec::new();
            let owner = disabled_owner(owner.disable(&mut |event| leftovers.push(event)).await);
            assert_eq!(leftovers, [transmit_done(id, TxStatus::Aborted)]);
            assert_eq!(state(runtime), RadioState::Disabled);
            let mut owner = enabled_owner(owner.enable().await);
            {
                let phase = owner.split();
                let client = phase.clients.client(&configured);
                let permit = ready(client.operation(&mut operation[0]));
                let _work = client
                    .transmit(permit, transmission(&MAC, TxMode::Direct, 0))
                    .unwrap();
            }
            let owner = disabled_owner(owner.disable(&mut |_| {}).await);
            owner.uninstall(())
        },
    );
}

/// Received frames never take the slot an operation's terminal needs: the
/// next operation is admitted beside a full queue, and its terminal follows
/// the frames and the loss.
#[test]
fn received_frames_leave_the_operation_its_terminal_slot() {
    enabled::<4>(async |phase| {
        let runtime = phase.runtime;
        ready(phase.client.receive(channel(20))).unwrap();
        // Two frames fill the free slots, and a third is lost.
        for _ in 0..3 {
            runtime.model_interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
        }
        let permit = ready(phase.client.operation(phase.seat));
        let id = phase
            .client
            .transmit(permit, transmission(&MAC, TxMode::Direct, 0))
            .unwrap()
            .id();
        runtime.model_interrupt(None, &[Ieee802154Event::TxDone]);
        for _ in 0..2 {
            assert!(received(&next(phase.events).unwrap()));
        }
        assert_eq!(next(phase.events), Err(EventsLost));
        assert_eq!(next(phase.events), Ok(transmit_done(id, TxStatus::Success)));
    });
}

/// The engine's software-coexistence PTIs are published while installed
/// and return to the disabled foundation image when the parts leave.
#[test]
fn uninstall_returns_the_coexistence_ptis_to_the_foundation_image() {
    let mut parts = parts();
    parts
        .engine
        .set_coexistence(Ieee802154Coexistence::Software(resolve_priorities(
            Ieee802154CoexConfig::VENDOR,
            &CoexPtiTable::VENDOR,
        )));
    let (_, parts, _) = enabled_with::<4>(parts, async |phase| {
        let runtime = phase.runtime;
        runtime.with_model(|hardware| assert_eq!((hardware.txrx_pti, hardware.ack_pti), (1, 8)));
        // A new table snapshot reaches the next scene switch.
        let mut table = CoexPtiTable::VENDOR;
        table.set(CoexEventId::new(43).unwrap(), CoexPti::new(6).unwrap());
        runtime.set_coexistence(Ieee802154Coexistence::Software(resolve_priorities(
            Ieee802154CoexConfig::VENDOR,
            &table,
        )));
        ready(phase.client.receive(channel(11))).unwrap();
        assert_eq!(runtime.with_model(|hardware| hardware.txrx_pti), 6);
    });
    assert_eq!((parts.hardware.txrx_pti, parts.hardware.ack_pti), (3, 3));
}

#[test]
fn the_pending_mode_reaches_the_pib_of_the_installed_radio() {
    enabled::<4>(async |phase| {
        phase
            .client
            .set_pending_mode(Has::from_const(), oer_ieee802154::AutoPendingMode::Enhanced);
        let mode = phase.runtime.with_installed(|radio, _, _| {
            radio.engine().pib().pending_mode(
                oer_espressif_ieee802154_engine::types::Ieee802154MultipanIndex::CONTEXT0,
            )
        });
        assert_eq!(mode, oer_ieee802154::AutoPendingMode::Enhanced);
    });
}

/// An installed generator answers a 2015 frame with an enhanced ACK; the
/// frame reaches the queue once the ACK is sent.
#[test]
fn an_installed_enhanced_ack_generator_answers_2015_frames() {
    enabled::<4>(async |phase| {
        let runtime = phase.runtime;
        phase
            .control
            .set_enhanced_ack(Some(EnhancedAckGeneration { noise_floor_dbm: 0 }));
        ready(phase.client.receive(channel(11))).unwrap();
        // 2015 data frame requesting an ACK, short addresses, compressed PAN.
        let mac = [0x61, 0xa8, 0x05, 0x34, 0x12, 0x01, 0x00, 0x02, 0x00, 0xaa];
        let mut image = [0; 13];
        image[0] = 12;
        image[1..11].copy_from_slice(&mac);
        runtime.model_interrupt(Some(&image), &[Ieee802154Event::RxDone]);
        assert!(
            runtime.events.take().is_none(),
            "the frame waits for its ACK"
        );
        runtime.model_interrupt(None, &[Ieee802154Event::AckTxDone]);
        assert!(matches!(
            next(phase.events),
            Ok(Ieee802154PortEvent {
                event: Ieee802154RadioEvent::Received(frame),
                ..
            }) if frame.frame.as_bytes() == mac
        ));
    });
}

/// The port's joined driver runs the CSMA-CA backoff and then starts the
/// CCA attempt; a busy channel backs off again, and the outcome arrives as
/// usual. Awaiting events alone does not progress the backoff.
#[test]
fn the_driver_runs_csma_ca_backoffs() {
    enabled::<4>(async |phase| {
        let runtime = phase.runtime;
        let permit = ready(phase.client.operation(phase.seat));
        let id = phase
            .client
            .transmit(
                permit,
                transmission(&MAC, TxMode::CsmaCa { max_backoffs: 3 }, 0),
            )
            .unwrap()
            .id();
        let command = || runtime.with_model(|hardware| hardware.command);
        assert_ne!(command(), Some(Ieee802154LlCommand::CcaTxStart));
        // Taking events only dequeues them.
        assert!(poll_once(pin!(phase.events.next_event())).is_pending());
        // The backoff (21 % 8 unit periods, 1.6 ms) ends only once its time
        // has passed; each yield lets `run_port` poll the driver.
        yield_now().await;
        runtime.timer.advance(Duration::from_micros(1_599)).unwrap();
        yield_now().await;
        assert_ne!(command(), Some(Ieee802154LlCommand::CcaTxStart));
        runtime.timer.advance(Duration::from_micros(1)).unwrap();
        yield_now().await;
        assert_eq!(command(), Some(Ieee802154LlCommand::CcaTxStart));

        runtime.with_model(|hardware| {
            hardware.tx_abort =
                Ieee802154TxAbortReasonObservation::Named(Ieee802154TxAbortReason::CcaBusy);
        });
        runtime.model_interrupt(None, &[Ieee802154Event::TxAbort]);
        assert_ne!(command(), Some(Ieee802154LlCommand::CcaTxStart));
        yield_now().await;
        runtime.timer.advance(Duration::from_millis(50)).unwrap();
        yield_now().await;
        assert_eq!(command(), Some(Ieee802154LlCommand::CcaTxStart));

        runtime.model_interrupt(None, &[Ieee802154Event::TxDone]);
        assert_eq!(next(phase.events), Ok(transmit_done(id, TxStatus::Success)));
    });
}

/// The port's joined driver runs the delay before a retry after a missing
/// acknowledgement, then the retry goes out.
#[test]
fn the_driver_runs_retry_delays() {
    enabled::<4>(async |phase| {
        let runtime = phase.runtime;
        let mut acknowledged = MAC;
        acknowledged[0] |= 0x20;
        let permit = ready(phase.client.operation(phase.seat));
        let id = phase
            .client
            .transmit(permit, transmission(&acknowledged, TxMode::Direct, 1))
            .unwrap()
            .id();
        // The last MAC command, cleared once read.
        let take_command = || runtime.with_model(|hardware| hardware.command.take());
        let no_ack = || {
            runtime.model_interrupt(None, &[Ieee802154Event::TxDone]);
            runtime.with_model(|hardware| {
                hardware.tx_abort = Ieee802154TxAbortReasonObservation::Named(
                    Ieee802154TxAbortReason::RxAckTimeout,
                );
            });
            runtime.model_interrupt(None, &[Ieee802154Event::TxAbort]);
        };
        assert_eq!(take_command(), Some(Ieee802154LlCommand::TxStart));
        no_ack();
        assert_ne!(take_command(), Some(Ieee802154LlCommand::TxStart));
        yield_now().await;
        runtime.timer.advance(Duration::from_millis(50)).unwrap();
        yield_now().await;
        assert_eq!(take_command(), Some(Ieee802154LlCommand::TxStart));
        no_ack();
        assert_eq!(
            next(phase.events),
            Ok(transmit_done(id, TxStatus::NoAcknowledgement))
        );
    });
}

/// The port reads the radio clock, the platform's monotonic clock, and the
/// CSL state of the installed radio.
#[test]
fn the_clock_and_csl_state_belong_to_the_installed_radio() {
    enabled::<4>(async |phase| {
        assert_eq!(
            ready(phase.client.now())
                .map(|now| now.map(oer_ieee802154::Ieee802154Instant::as_micros)),
            Ok(Ok(0)),
            "the radio clock is the runtime's virtual clock, at its epoch"
        );
        phase.control.set_csl(CslReceiver {
            period: 100,
            sample_time: 5_000,
        });
        assert_eq!(
            phase
                .runtime
                .with_installed(|radio, _, _| (radio.csl().period, radio.csl().sample_time)),
            (100, 5_000)
        );
    });
}

/// The recent-RSSI reader reads the hardware live in any radio state, and
/// the lowest RSSI while no radio is installed.
#[test]
fn recent_rssi_reads_the_hardware_live() {
    let (runtime, _, _) = enabled::<4>(async |phase| {
        let runtime = phase.runtime;
        let rssi = runtime.recent_rssi_reader();
        let set = |rssi: i8| runtime.with_model(|hardware| hardware.recent_rssi = rssi);
        set(-42);
        assert_eq!(rssi.recent_rssi(), -42);
        ready(phase.client.receive(channel(11))).unwrap();
        set(-87);
        assert_eq!(rssi.recent_rssi(), -87);
        phase.client.sleep();
        assert_eq!(rssi.recent_rssi(), -87);
    });
    assert_eq!(runtime.recent_rssi_reader().recent_rssi(), i8::MIN);
}

/// The runtime collects, lends and clears the engine's TX/RX statistics.
#[test]
fn txrx_statistics_are_collected_on_request() {
    enabled::<4>(async |phase| {
        let runtime = phase.runtime;
        assert_eq!(runtime.txrx_statistics(), None);
        runtime.set_txrx_statistics(true);
        ready(phase.client.receive(channel(15))).unwrap();
        runtime.model_interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
        let statistics = runtime.txrx_statistics().unwrap();
        assert_eq!(statistics.rx.done_nums, 1);
        runtime.clear_txrx_statistics();
        assert_eq!(
            runtime.txrx_statistics(),
            Some(Ieee802154TxRxStatistics::default())
        );
    });
}

/// A multi-PAN runtime keeps each interface's keys and frame counter apart:
/// keys start from the zeroed ones and keep the counter, and the counter
/// keeps the keys.
#[test]
fn interface_keys_belong_to_their_interface() {
    let buffers = Box::leak(Box::new(Ieee802154EngineBuffers::new()));
    let levels = Ieee802154TxPowerLevels::new(&LEVELS).unwrap();
    let parts = Ieee802154RuntimeParts {
        engine: Ieee802154Engine::new_multipan(
            buffers,
            levels,
            Ieee802154PibDefaults::default(),
            Ieee802154Interfaces::new(2).unwrap(),
        ),
        hardware: Ieee802154LlModel::default(),
    };
    let runtime = runtime::<4>();
    run(
        install::<Multipan, 4>(runtime, parts),
        async |mut owner, resources| {
            let (zero, one) = resources.interfaces;
            let zero = owner.setup().configure(zero.interface);
            let one = owner.setup().configure(one.interface);
            assert!(runtime.with_installed(|radio, _, _| {
                radio.capabilities().contains(RadioCapabilities::MULTI_PAN)
            }));
            let mut owner = enabled_owner(owner.enable().await);
            {
                let phase = owner.split();
                let first = phase.clients.client(&zero);
                let second = phase.clients.client(&one);
                second.set_mac_keys(1, [1; 16], [2; 16], [3; 16]);
                assert_eq!(second.frame_counter(), Some(0));
                second.update_frame_counter(FrameCounterUpdate::Set(9));
                second.update_frame_counter(FrameCounterUpdate::SetIfLarger(5));
                assert_eq!(second.frame_counter(), Some(9));
                assert_eq!(first.frame_counter(), None);
                let security = runtime.with_installed(|radio, _, _| {
                    radio
                        .interface_mac_keys(Interface::new(1))
                        .and_then(|keys| *keys)
                        .map(|mut keys| keys.transmit_security(false))
                });
                let security = security.unwrap();
                assert_eq!((security.key_id, security.key), (Some(1), [2; 16]));
                second.remove_mac_keys();
                assert_eq!(second.frame_counter(), None);
            }
            let mut owner = disabled_owner(owner.disable(&mut |_| {}).await);
            let _ = owner.setup().remove(one);
            let _ = owner.setup().remove(zero);
            owner.uninstall(())
        },
    );
}

/// A received frame the full queue drops is recorded as dropped.
#[test]
fn a_dropped_frame_is_traced() {
    use oer_ieee802154_trace::{RxDrop, RxOutcome, RxResult};

    // The fault slot, the next operation's slot and two free ones.
    enabled::<4>(async |phase| {
        let runtime = phase.runtime;
        ready(phase.client.receive(channel(15))).unwrap();
        super::trace::take_recorded();
        for _ in 0..3 {
            runtime.model_interrupt(Some(&received_image()), &[Ieee802154Event::RxDone]);
        }
        let drops: Vec<RxOutcome> = super::trace::take_recorded()
            .iter()
            .filter_map(|record| record.decode::<RxOutcome>())
            .filter(|outcome| matches!(outcome.result, RxResult::Dropped(_)))
            .collect();
        assert_eq!(
            drops,
            [RxOutcome {
                length: 12,
                result: RxResult::Dropped(RxDrop::QueueFull),
            }]
        );
    });
}

/// The installed role states the runtime's capabilities.
#[test]
fn the_role_states_the_runtime_capabilities() {
    enabled::<4>(async |phase| {
        assert_eq!(
            phase
                .runtime
                .with_installed(|radio, _, _| radio.capabilities()),
            IEEE802154_RADIO_CAPABILITIES
        );
    });
}

/// A cancelled transmission's aborted end arrives through the queue; a
/// cancel after the radio ended the work finds it no longer running.
#[test]
fn a_cancellation_ends_the_transmission_through_the_queue() {
    enabled::<4>(async |phase| {
        let runtime = phase.runtime;
        let permit = ready(phase.client.operation(phase.seat));
        let work = phase
            .client
            .transmit(permit, transmission(&MAC, TxMode::Direct, 0))
            .unwrap();
        let id = work.id();
        assert_eq!(ready(work.cancel()), Ok(Ok(())));
        assert_eq!(next(phase.events), Ok(transmit_done(id, TxStatus::Aborted)));
        assert_eq!(state(runtime), RadioState::Resting(RestingState::Sleeping));

        let permit = ready(phase.client.operation(phase.seat));
        let work = phase
            .client
            .transmit(permit, transmission(&MAC, TxMode::Direct, 0))
            .unwrap();
        let id = work.id();
        runtime.model_interrupt(None, &[Ieee802154Event::TxDone]);
        assert_eq!(ready(work.cancel()), Ok(Err(CancelError::NotRunning)));
        assert_eq!(next(phase.events), Ok(transmit_done(id, TxStatus::Success)));
    });
}

/// Header IEs set while generation is off are kept and go out once it is
/// on; turning generation off and on again, or changing its noise floor,
/// keeps them (the owner shape's `set_enhanced_ack`).
#[test]
fn enhanced_ack_header_ies_are_kept_while_generation_is_off() {
    enabled::<4>(async |phase| {
        let runtime = phase.runtime;
        let held = || {
            runtime.with_installed(|radio, _, _| {
                radio.enhanced_ack().as_mut().map(|generator| {
                    (
                        generator.header_ies().to_vec(),
                        generator.probing().noise_floor(),
                    )
                })
            })
        };
        assert!(SupportedHeaderIes::<Single>::new(&[0; 17]).is_none());
        phase
            .control
            .set_enhanced_ack_header_ies(SupportedHeaderIes::new(&[1, 2]).unwrap());
        assert_eq!(held(), None, "generation is off");
        phase.control.set_enhanced_ack(Some(EnhancedAckGeneration {
            noise_floor_dbm: -100,
        }));
        assert_eq!(held(), Some((std::vec![1, 2], -100)));
        phase.control.set_enhanced_ack(Some(EnhancedAckGeneration {
            noise_floor_dbm: -90,
        }));
        assert_eq!(held(), Some((std::vec![1, 2], -90)));
        phase.control.set_enhanced_ack(None);
        assert_eq!(held(), None);
        phase
            .control
            .set_enhanced_ack_header_ies(SupportedHeaderIes::new(&[3]).unwrap());
        phase.control.set_enhanced_ack(Some(EnhancedAckGeneration {
            noise_floor_dbm: -80,
        }));
        assert_eq!(held(), Some((std::vec![3], -80)));
    });
}

/// Probing initiators set while generation is off survive enabling,
/// reconfiguration and repeated disabling; replacement and removal while
/// off reach the next enabled generator.
#[test]
fn enhanced_ack_probing_is_kept_while_generation_is_off() {
    enabled::<4>(async |phase| {
        let initiator = ProbingInitiator {
            short_address: 0x1234,
            extended_address: [1, 2, 3, 4, 5, 6, 7, 8],
            metrics: LinkMetrics {
                lqi: true,
                ..LinkMetrics::NONE
            },
        };
        let held = || {
            phase.runtime.with_installed(|radio, _, _| {
                radio.enhanced_ack().as_mut().map(|generator| {
                    let probing = generator.probing();
                    (
                        probing.metrics(FrameAddress::Short(initiator.short_address.to_le_bytes())),
                        probing.metrics(FrameAddress::Extended(initiator.extended_address)),
                        probing.noise_floor(),
                    )
                })
            })
        };
        phase.control.set_enhanced_ack_probing(&[initiator]);
        assert_eq!(held(), None, "generation is off");
        for noise_floor_dbm in [-100, -90] {
            phase
                .control
                .set_enhanced_ack(Some(EnhancedAckGeneration { noise_floor_dbm }));
            assert_eq!(
                held(),
                Some((
                    Some(initiator.metrics),
                    Some(initiator.metrics),
                    noise_floor_dbm
                ))
            );
        }
        phase.control.set_enhanced_ack(None);
        phase.control.set_enhanced_ack(None);
        phase.control.set_enhanced_ack(Some(EnhancedAckGeneration {
            noise_floor_dbm: -80,
        }));
        assert_eq!(
            held(),
            Some((Some(initiator.metrics), Some(initiator.metrics), -80))
        );

        let replacement = ProbingInitiator {
            metrics: LinkMetrics {
                rssi: true,
                ..LinkMetrics::NONE
            },
            ..initiator
        };
        for (initiators, metrics) in [
            (&[replacement][..], Some(replacement.metrics)),
            (&[][..], None),
        ] {
            phase.control.set_enhanced_ack(None);
            phase.control.set_enhanced_ack_probing(initiators);
            assert_eq!(held(), None, "generation stays off");
            phase.control.set_enhanced_ack(Some(EnhancedAckGeneration {
                noise_floor_dbm: -80,
            }));
            assert_eq!(held(), Some((metrics, metrics, -80)));
        }
    });
}

/// Only the first fault of an enabled period holds a reserved slot; a
/// further one, as a disabled radio may still report, takes a free slot or
/// is lost instead of overrunning the queue.
#[test]
fn a_fault_beyond_the_reserved_one_never_overruns_the_queue() {
    let queue = super::EventQueue::<NoopRawMutex, 2>::new();
    queue.owed(|owed| owed.fault = true);
    let fault = || Ieee802154RadioEvent::Fault {
        id: None,
        fault: RadioFault::InvalidEventSequence,
    };
    assert!(queue.push(fault()));
    assert!(queue.push(fault()));
    // Both slots are taken: the third fault is lost, not a panic.
    assert!(!queue.push(fault()));
    assert_eq!(queue.take(), Some(Ok(fault())));
    assert_eq!(queue.take(), Some(Ok(fault())));
    assert_eq!(queue.take(), Some(Err(EventsLost)));
    assert_eq!(queue.take(), None);
}
