//! Atomic handoff from prepared connected owners to the Embassy radio runner.
//!
//! Board applications and HIL select storage, sinks and observation policy,
//! but they must not independently choose the order in which the production
//! connected drivers and network runner are joined.  This transaction keeps
//! the network owner beside every failed driver handoff and exposes one
//! statically dispatched service-decoration hook for qualification faults.

use crate::{
    datapath::{DatapathRunner, DatapathServices, PinnedTxFrame},
    roles::station::{
        connected::port::{
            ConnectedStaCompositionFailure, ConnectedStaControlResources, ConnectedStaPlan,
            ConnectedStaPort, ConnectedStaReport, ConnectedStaRxProtocolResources,
            ConnectedStaTxHandoffFailure, ConnectedStaTxResources,
        },
        control_slot::PlacedConnectedControl,
        rx_protocol::{ConnectedReceiveProtocol, ConnectedRxProtocolSink},
        tx::ConnectedTx,
    },
};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_esp32s31_ieee80211::ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile};

/// Complete running frontier returned by connected driver assembly.
pub struct ConnectedDriverAssembly<R> {
    pub runner: R,
    pub report: ConnectedStaReport,
}

/// Failed driver assembly retaining the persistent network owner and the
/// caller's not-yet-applied service decoration.
pub struct ConnectedDriverAssemblyFailure<N, C, F> {
    pub network: N,
    pub composition: C,
    pub map_services: F,
}

/// Coherent owner set consumed by one connected-driver assembly transaction.
///
/// Named ownership domains keep application and HIL call sites independent of
/// a positional argument sequence while the type system still proves the
/// exact network, IRQ, protocol and TX geometries.
pub struct ConnectedDriverAssemblyResources<'irq, M: RawMutex, N, H, R, P, T, C, F, K> {
    pub plan: ConnectedStaPlan,
    pub irq: &'irq crate::datapath::irq::EmbassyMacIrqRuntime<M>,
    pub network: N,
    pub hardware: H,
    pub rx: R,
    pub protocol: P,
    pub tx: T,
    pub control: C,
    pub map_services: F,
    /// Monotonic clock of the returned runner's scheduling deadlines.
    pub clock: K,
}

/// Compose the connected MAC graph and join it to the persistent network
/// owner in one owner-preserving transaction.
#[allow(clippy::type_complexity, clippy::result_large_err)]
#[inline]
pub fn assemble_esp32s31_connected_driver<
    'slot,
    'resources,
    'queue,
    'pool,
    'scratch,
    'irq,
    'control,
    M,
    S,
    P,
    E,
    T,
    H,
    R,
    B,
    F,
    N,
    K,
    const RX_DEPTH: usize,
    const RX_CAPACITY: usize,
    const RX_SLOTS: usize,
    const REORDER_SLOTS: usize,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
    const AGGREGATE_SLOTS: usize,
    const AGGREGATE_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
    const CONTROL_CAPACITY: usize,
>(
    resources: ConnectedDriverAssemblyResources<
        'irq,
        M,
        N,
        H,
        R,
        ConnectedStaRxProtocolResources<
            'queue,
            'pool,
            'scratch,
            'irq,
            M,
            S,
            K,
            RX_DEPTH,
            RX_CAPACITY,
            RX_SLOTS,
            REORDER_SLOTS,
        >,
        ConnectedStaTxResources<
            'slot,
            'resources,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
            AGGREGATE_SLOTS,
            AGGREGATE_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        ConnectedStaControlResources<'control, M, CONTROL_CAPACITY>,
        F,
        K,
    >,
) -> Result<
    ConnectedDriverAssembly<DatapathRunner<'irq, M, N, B, N::RxPublisher, K>>,
    ConnectedDriverAssemblyFailure<
        N,
        ConnectedStaCompositionFailure<
            H,
            R,
            ConnectedStaRxProtocolResources<
                'queue,
                'pool,
                'scratch,
                'irq,
                M,
                S,
                K,
                RX_DEPTH,
                RX_CAPACITY,
                RX_SLOTS,
                REORDER_SLOTS,
            >,
            ConnectedStaControlResources<'control, M, CONTROL_CAPACITY>,
            ConnectedStaTxHandoffFailure<
                'slot,
                'resources,
                PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
                P,
                E,
                T,
                AGGREGATE_SLOTS,
                AGGREGATE_BUFFER_SIZE,
                ORDINARY_BUFFER_SIZE,
            >,
        >,
        F,
    >,
>
where
    M: RawMutex,
    N: crate::datapath::network::DatapathNetwork,
    S: ConnectedRxProtocolSink<RX_CAPACITY, RX_SLOTS>,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    H: oer_esp32s31_ieee80211_mac::init::StaEspNowRxPolicyHardware,
    F: FnOnce(
        crate::datapath::services::SingleRoleServices<
            H,
            crate::roles::station::connected::ConnectedStaRxService<
                R,
                ConnectedReceiveProtocol<
                    'queue,
                    'pool,
                    'scratch,
                    'irq,
                    M,
                    S,
                    K,
                    RX_DEPTH,
                    RX_CAPACITY,
                    RX_SLOTS,
                    REORDER_SLOTS,
                >,
            >,
            ConnectedTx<
                'slot,
                'resources,
                crate::datapath::PinnedTxFrame<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    TX_QUEUE_DEPTH,
                >,
                P,
                E,
                T,
                AGGREGATE_SLOTS,
                AGGREGATE_BUFFER_SIZE,
                ORDINARY_BUFFER_SIZE,
            >,
            PlacedConnectedControl<'control, M, CONTROL_CAPACITY>,
        >,
    ) -> B,
    B: DatapathServices<N::TxFrame, N::PhysicalTxFrame>,
{
    let ConnectedDriverAssemblyResources {
        plan,
        irq,
        network,
        hardware,
        rx,
        protocol,
        tx,
        control,
        map_services,
        clock,
    } = resources;
    let drivers = match ConnectedStaPort::compose(plan, hardware, rx, protocol, tx, control) {
        Ok(drivers) => drivers,
        Err(composition) => {
            return Err(ConnectedDriverAssemblyFailure {
                network,
                composition,
                map_services,
            });
        }
    };
    let services = map_services(drivers.services);
    Ok(ConnectedDriverAssembly {
        runner: DatapathRunner::new(
            irq,
            network,
            crate::datapath::network::STA_NETWORK_INTERFACE_ID,
            services,
            clock,
        ),
        report: drivers.report,
    })
}
