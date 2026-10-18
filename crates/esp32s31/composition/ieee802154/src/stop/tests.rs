//! Exercise the stop dispatch used by System::stop with a real model MAC
//! and fake shared-PHY/clock owners. MMIO release sequences belong to HAL.

use core::cell::Cell;
use std::{boxed::Box, rc::Rc};

use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_espressif_ieee802154_engine::{
    coex::Ieee802154Coexistence,
    engine::{Ieee802154Engine, Ieee802154EngineBuffers, Ieee802154State},
    ll::model::Ieee802154LlModel,
    pib::Ieee802154PibDefaults,
    tx_power::Ieee802154TxPowerLevels,
    types::Ieee802154Event,
};
use oer_espressif_ieee802154_runtime::{
    Ieee802154Random, Ieee802154Runtime, Ieee802154RuntimeBackend, Ieee802154RuntimeParts,
    Uninstalled,
};
use oer_ieee802154::{
    Channel,
    owner::{Ieee802154Client, Ieee802154Clients, Ieee802154Setup},
};
use oer_radio_port::{
    Disabled, Ended, Owner, PortBackend, PortBody, PortEnd, Quarantined, run_port,
};
use oer_time_virtual::VirtualClock;

use super::{Released, StoppedMac};

struct Random;
impl Ieee802154Random for Random {
    fn random(&mut self) -> u32 {
        21
    }
}
type Runtime = Ieee802154Runtime<'static, NoopRawMutex, Ieee802154LlModel, VirtualClock, Random, 4>;
type Backend = Ieee802154RuntimeBackend<
    'static,
    'static,
    crate::Esp32s31Ieee802154,
    NoopRawMutex,
    Ieee802154LlModel,
    VirtualClock,
    Random,
    4,
>;

struct Reject(&'static Runtime);
impl PortBody<Backend> for Reject {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        mut owner: Owner<'p, 'id, Backend, Disabled>,
        resources: <Backend as PortBackend>::Resources<'id>,
    ) -> Ended<'id, Backend, ()> {
        let configured = owner.setup().configure(resources.interfaces.interface);
        let mut owner = owner.enable().await.unwrap_or_else(|_| panic!("enable"));
        owner
            .phase()
            .clients
            .client(&configured)
            .receive(Channel::new(11).unwrap())
            .await
            .unwrap();
        self.0.model_interrupt(
            Some(&[
                12, 0x41, 0x98, 1, 0x34, 0x12, 0xff, 0xff, 0x78, 0x56, 0xaa, 201, 180,
            ]),
            &[Ieee802154Event::RxDone],
        );
        let owner = owner
            .disable(&mut core::ops::ControlFlow::Break)
            .await
            .err()
            .unwrap();
        Ended::uninstall(owner, (), ())
    }
}

struct DropCount(Rc<Cell<u32>>);
impl Drop for DropCount {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn terminal_quarantine_still_releases_composition_clocks_and_phy() {
    let runtime = Box::leak(Box::new(Runtime::new(VirtualClock::new())));
    let buffers = Box::leak(Box::new(Ieee802154EngineBuffers::new()));
    let mut engine = Ieee802154Engine::new(
        buffers,
        Ieee802154TxPowerLevels::new(&[0]).unwrap(),
        Ieee802154PibDefaults::default(),
    );
    engine.set_coexistence(Ieee802154Coexistence::Software(
        oer_esp32s31_hal::ieee802154::coex::resolve_priorities(
            oer_esp32s31_hal::ieee802154::coex::Ieee802154CoexConfig::VENDOR,
            &oer_esp32s31_hal::coex::CoexPtiTable::VENDOR,
        ),
    ));
    let parts = Ieee802154RuntimeParts::new(
        engine,
        Ieee802154LlModel::default(),
        runtime.take_queue().unwrap(),
    );
    let backend = runtime
        .install::<crate::Esp32s31Ieee802154>(parts, Random, Ieee802154PibDefaults::default())
        .unwrap_or_else(|_| panic!("install"));
    let PortEnd::Uninstalled {
        uninstalled: Uninstalled::TerminalsQuarantined { parts, terminals },
        ..
    } = block_on(run_port(backend.installed(), Reject(runtime)))
    else {
        panic!("terminal quarantine")
    };
    let drops = Rc::new(Cell::new(0));
    let stopped = StoppedMac::take(Uninstalled::TerminalsQuarantined {
        parts,
        terminals: Quarantined::new((terminals, DropCount(drops.clone()))),
    })
    .unwrap_or_else(|_| panic!("MAC stopped"));
    // A refused coex leave must preserve the exact terminal-retention path
    // when composition retries its stop with the returned owners.
    let stopped = StoppedMac::take(stopped.into_uninstalled()).unwrap_or_else(|_| panic!("retry"));
    let clocks = Cell::new(true);
    let phy = Cell::new(true);
    let released = block_on(stopped.release(async |engine, hardware| {
        assert_eq!(engine.state(), Ieee802154State::Disable);
        assert_eq!((hardware.txrx_pti, hardware.ack_pti), (3, 3));
        assert!(phy.replace(false));
        assert!(clocks.replace(false));
        Ok::<_, ()>((engine, hardware))
    }))
    .unwrap();
    let Released::TerminalsQuarantined {
        parts: _hardware,
        terminals,
    } = released
    else {
        panic!("teardown must not return permission to reuse the old queue");
    };
    assert!(!clocks.get());
    assert!(!phy.get());
    drop(terminals);
    assert_eq!(drops.get(), 0);
    assert!(runtime.take_queue().is_none());
}

#[test]
fn reusable_stop_returns_the_same_queue_permission() {
    let runtime = Box::leak(Box::new(Runtime::new(VirtualClock::new())));
    let buffers = Box::leak(Box::new(Ieee802154EngineBuffers::new()));
    let engine = Ieee802154Engine::new(
        buffers,
        Ieee802154TxPowerLevels::new(&[0]).unwrap(),
        Ieee802154PibDefaults::default(),
    );
    let parts = Ieee802154RuntimeParts::new(
        engine,
        Ieee802154LlModel::default(),
        runtime.take_queue().unwrap(),
    );
    let backend = runtime
        .install::<crate::Esp32s31Ieee802154>(parts, Random, Ieee802154PibDefaults::default())
        .unwrap_or_else(|_| panic!("install"));
    let Uninstalled::Reusable((parts, _)) = backend.uninstall_unused() else {
        panic!("unused")
    };
    let stopped = StoppedMac::take(Uninstalled::<_, _, ()>::Reusable(parts))
        .unwrap_or_else(|_| panic!("stopped"));
    let Released::Reusable {
        parts: (engine, hardware),
        queue,
    } = block_on(stopped.release(async |engine, hardware| Ok::<_, ()>((engine, hardware))))
        .unwrap()
    else {
        panic!("reusable")
    };
    assert!(
        runtime
            .install::<crate::Esp32s31Ieee802154>(
                Ieee802154RuntimeParts::new(engine, hardware, queue),
                Random,
                Ieee802154PibDefaults::default(),
            )
            .is_ok()
    );
}
