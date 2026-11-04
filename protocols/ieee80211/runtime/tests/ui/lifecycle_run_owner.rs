#![no_std]
use oer_portable_time::Timer;
use oer_protocols_ieee80211_contracts_port::{PhaseTypes, PortCommands};
use oer_protocols_ieee80211_core_roles_sta::machine::{StaCallbacks, StaConfigurationContext};
use oer_protocols_ieee80211_runtime::datapath::ActorEpochPath;
use oer_protocols_ieee80211_runtime::lifecycle::{
    ActorEnds, Lifecycle, LifecycleIo, LifecyclePort, LifecycleTxContext,
};
use oer_protocols_ieee80211_runtime::station::{
    ServiceOutputSink, StationEvents, TxContextStorage,
};

pub fn run_once<'id, D, C, X, T, O>(
    lifecycle: Lifecycle<'id, D, C, X, T, 2, O>,
    actor: &mut ActorEnds<'_, 'id, D, 1, 1, 1>,
    io: &mut LifecycleIo<'_, D::Caps, impl ServiceOutputSink<D::Caps>, 1>,
    timer: &impl Timer,
) where
    D: LifecyclePort<'id>,
    D::Types: PhaseTypes<
            'id,
            StationConfigureContext = StaConfigurationContext<'id, ActorEpochPath<'id, D>>,
        >,
    C: StaCallbacks,
    X: PortCommands<'id, Types = D::Types, Caps = D::Caps>,
    T: TxContextStorage<LifecycleTxContext<'id, D>>,
    O: StationEvents,
{
    let run = lifecycle.run(actor, io, timer);
    #[cfg(ui_reject)]
    drop(lifecycle); // ui-error: E0382
    drop(run);
}
