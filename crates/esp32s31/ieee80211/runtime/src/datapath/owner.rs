use super::*;

impl<'irq, M: RawMutex, N, B, R, K> DatapathRunner<'irq, M, N, B, R, K> {
    /// Run a fallible owner transition, retaining the entire runner on either
    /// branch. The error service owns any partially transitioned resources.
    #[allow(clippy::type_complexity, clippy::result_large_err)]
    pub fn try_map_services<T, E>(
        self,
        map: impl FnOnce(B) -> Result<T, E>,
    ) -> Result<DatapathRunner<'irq, M, N, T, R, K>, DatapathRunner<'irq, M, N, E, R, K>> {
        // Destructure once. Building an intermediate Runner<()> and moving it
        // twice inflates target stack frames for large affine service graphs.
        let Self {
            services,
            irq,
            clock,
            network,
            interfaces,
            network_rx,
            active_tx_interface,
            active_tx_origin,
            prepared_tx_interface,
            prepared_tx_completion,
            control_ready_latched,
            rx_progress,
            recycled_rx_probe_deadline,
            recycled_rx_probe_coalescing_level,
            fairness,
            pair_tx_served_frames,
        } = self;
        match map(services) {
            Ok(services) => Ok(DatapathRunner {
                services,
                irq,
                clock,
                network,
                interfaces,
                network_rx,
                active_tx_interface,
                active_tx_origin,
                prepared_tx_interface,
                prepared_tx_completion,
                control_ready_latched,
                rx_progress,
                recycled_rx_probe_deadline,
                recycled_rx_probe_coalescing_level,
                fairness,
                pair_tx_served_frames,
            }),
            Err(services) => Err(DatapathRunner {
                services,
                irq,
                clock,
                network,
                interfaces,
                network_rx,
                active_tx_interface,
                active_tx_origin,
                prepared_tx_interface,
                prepared_tx_completion,
                control_ready_latched,
                rx_progress,
                recycled_rx_probe_deadline,
                recycled_rx_probe_coalescing_level,
                fairness,
                pair_tx_served_frames,
            }),
        }
    }

    pub const fn services(&self) -> &B {
        &self.services
    }

    /// The monotonic clock of this owner's scheduling deadlines.
    pub const fn clock(&self) -> &K {
        &self.clock
    }

    pub fn services_mut(&mut self) -> &mut B {
        &mut self.services
    }

    /// Transfer a service owner across a lifecycle boundary without rebuilding
    /// the runner. Preserve prepared-frame identity, deadlines, RX progress and
    /// fairness accounting. A service state without `DatapathServices` cannot
    /// run until its original live state has been restored.
    ///
    /// This transfers ownership only. The caller must establish the scheduler
    /// and hardware boundaries required by the service transition itself.
    pub fn map_services<T>(self, map: impl FnOnce(B) -> T) -> DatapathRunner<'irq, M, N, T, R, K> {
        DatapathRunner {
            irq: self.irq,
            clock: self.clock,
            network: self.network,
            interfaces: self.interfaces,
            network_rx: self.network_rx,
            services: map(self.services),
            active_tx_interface: self.active_tx_interface,
            active_tx_origin: self.active_tx_origin,
            prepared_tx_interface: self.prepared_tx_interface,
            prepared_tx_completion: self.prepared_tx_completion,
            control_ready_latched: self.control_ready_latched,
            rx_progress: self.rx_progress,
            recycled_rx_probe_deadline: self.recycled_rx_probe_deadline,
            recycled_rx_probe_coalescing_level: self.recycled_rx_probe_coalescing_level,
            fairness: self.fairness,
            pair_tx_served_frames: self.pair_tx_served_frames,
        }
    }
}

fn select_pair_tx_slot(pending: [bool; 2], served: [u64; 2]) -> Option<usize> {
    match pending {
        [true, true] => Some(usize::from(served[0] > served[1])),
        [true, false] => Some(0),
        [false, true] => Some(1),
        [false, false] => None,
    }
}

/// Charge `frames` to `slot`. Only a VIF whose peer also waits builds a lead:
/// frames served while the peer is idle would otherwise accumulate a lead
/// the returning peer then claims turn after turn.
fn charge_pair_tx_frames(served: &mut [u64; 2], slot: usize, frames: usize, peer_pending: bool) {
    if !peer_pending {
        *served = [0; 2];
        return;
    }
    served[slot] = served[slot].saturating_add(u64::try_from(frames.max(1)).unwrap_or(u64::MAX));
    let shared = served[0].min(served[1]);
    served[0] -= shared;
    served[1] -= shared;
}

impl<'irq, M: RawMutex, N, B, R, K> DatapathRunner<'irq, M, N, B, R, K>
where
    N: DatapathNetwork,
    B: DatapathServices<N::TxFrame, N::PhysicalTxFrame>,
    R: DatapathNetworkRxSet,
{
    pub fn new_with_rx_set(
        irq: &'irq EmbassyMacIrqRuntime<M>,
        network: N,
        interface: NetworkInterfaceId,
        network_rx: R,
        services: B,
        clock: K,
    ) -> Self {
        Self::new_with_scope(
            irq,
            network,
            DatapathInterfaceScope::Single(interface),
            network_rx,
            services,
            clock,
        )
    }

    /// Construct one physical DATAPATH scheduling two permanent logical VIFs.
    pub fn new_with_scope(
        irq: &'irq EmbassyMacIrqRuntime<M>,
        network: N,
        interfaces: DatapathInterfaceScope,
        network_rx: R,
        services: B,
        clock: K,
    ) -> Self {
        Self {
            irq,
            clock,
            network,
            interfaces,
            network_rx,
            services,
            active_tx_interface: None,
            active_tx_origin: None,
            prepared_tx_interface: None,
            prepared_tx_completion:
                crate::diagnostics::core0_rx_performance::Core0PreparedTxMark::new(),
            // Every control machine receives one initial finite step so it can
            // establish its first absolute deadline and publish startup work.
            control_ready_latched: true,
            rx_progress: DatapathRxProgress::Drained,
            recycled_rx_probe_deadline: None,
            recycled_rx_probe_coalescing_level: 0,
            fairness: super::fairness::RxTxFairness::new(),
            pair_tx_served_frames: [0; 2],
        }
    }

    pub(super) fn begin_active_tx(
        &mut self,
        interface: NetworkInterfaceId,
        origin: DatapathTxOrigin,
    ) {
        debug_assert!(self.active_tx_interface.is_none());
        debug_assert!(self.active_tx_origin.is_none());
        self.active_tx_interface = Some(interface);
        self.active_tx_origin = Some(origin);
    }

    pub(super) fn finish_active_tx(&mut self) -> DatapathTxOrigin {
        let interface = self
            .active_tx_interface
            .take()
            .expect("terminal TX retains its VIF owner");
        debug_assert!(self.interfaces.contains(interface));
        self.active_tx_origin
            .take()
            .expect("terminal TX retains its semantic origin")
    }

    /// Return the network and hardware owners after the runner exits.
    ///
    /// The outer role lifecycle must be able to reclaim these values after
    /// [`DatapathRunnerExit::Role`] or [`DatapathRunnerExit::Stopped`] in order to
    /// stop DMA, clear role state and construct a later epoch. Keeping them
    /// recoverable also makes it impossible for `run` to hide teardown behind
    /// task-local globals.
    pub fn into_parts(self) -> (N, B) {
        (self.network, self.services)
    }

    /// Return every runner owner, including an addressed multi-VIF RX set.
    ///
    /// Standalone callers normally recreate their cheap publisher at the
    /// next epoch and use [`Self::into_parts`]. A combined owner must retain
    /// both addressed publishers across partial role transitions and uses
    /// this complete frontier instead.
    pub fn into_complete_parts(self) -> (N, R, B) {
        (self.network, self.network_rx, self.services)
    }

    pub(super) fn network_tx_queue_len(&self) -> usize {
        if let Some(interface) = self.prepared_tx_interface {
            return self.network.tx_queue_len(interface);
        }
        match self.interfaces {
            DatapathInterfaceScope::Single(interface) => self.network.tx_queue_len(interface),
            DatapathInterfaceScope::Pair { first, second } => self
                .network
                .tx_queue_len(first)
                .saturating_add(self.network.tx_queue_len(second)),
        }
    }

    /// Select the VIF that owns the next network TX boundary without claiming
    /// its frame or advancing the paired fairness accounting.
    pub(super) fn next_network_tx_interface(&self) -> Option<NetworkInterfaceId> {
        if let Some(interface) = self.prepared_tx_interface {
            return Some(interface);
        }
        if self.services.has_prepared_tx() {
            return Some(self.retained_prepared_tx_interface());
        }
        match self.interfaces {
            DatapathInterfaceScope::Single(interface) => {
                (self.network.tx_queue_len(interface) != 0).then_some(interface)
            }
            DatapathInterfaceScope::Pair { first, second } => {
                let pending = [
                    self.network.tx_queue_len(first) != 0,
                    self.network.tx_queue_len(second) != 0,
                ];
                match select_pair_tx_slot(pending, self.pair_tx_served_frames) {
                    Some(0) => Some(first),
                    Some(1) => Some(second),
                    None => None,
                    Some(_) => unreachable!("paired TX has exactly two slots"),
                }
            }
        }
    }

    pub(super) fn try_receive_network_tx(&mut self) -> Option<N::TxFrame> {
        let interface = self.next_network_tx_interface()?;
        if let DatapathInterfaceScope::Pair { first, second } = self.interfaces
            && self.prepared_tx_interface.is_none()
        {
            let first_pending = self.network.tx_queue_len(first) != 0;
            let second_pending = self.network.tx_queue_len(second) != 0;
            if !first_pending || !second_pending {
                self.pair_tx_served_frames = [0; 2];
            }
        }
        self.network.try_receive_tx(interface)
    }

    pub(super) fn account_pair_tx_frames(&mut self, interface: NetworkInterfaceId, frames: usize) {
        let DatapathInterfaceScope::Pair { first, second } = self.interfaces else {
            return;
        };
        let (slot, peer) = if interface == first {
            (0, second)
        } else {
            assert_eq!(interface, second);
            (1, first)
        };
        let peer_pending = self.network.tx_queue_len(peer) != 0;
        charge_pair_tx_frames(&mut self.pair_tx_served_frames, slot, frames, peer_pending);
    }

    pub(super) fn tx_interface_for(&self, frame: &N::TxFrame) -> NetworkInterfaceId {
        let interface = frame.interface();
        assert!(
            self.interfaces.contains(interface),
            "tagged TX lease does not belong to this DATAPATH scope"
        );
        interface
    }

    /// Whether the other VIF already has published work while `interface`
    /// owns the physical transmitter.
    ///
    /// A paired owner may build one software standby aggregate, but it must
    /// not keep extending that reservation across an already-published frame
    /// from its peer.  This is the finite ownership boundary used for VIF
    /// fairness; a single-interface runner has no competing frontier.
    pub(super) fn competing_tx_pending(&self, interface: NetworkInterfaceId) -> bool {
        match self.interfaces {
            DatapathInterfaceScope::Single(_) => false,
            DatapathInterfaceScope::Pair { first, second } => {
                let peer = if interface == first {
                    second
                } else {
                    assert_eq!(interface, second);
                    first
                };
                self.network.tx_queue_len(peer) != 0
            }
        }
    }

    pub(super) fn set_scope_link_state(&self, state: LinkState) {
        match self.interfaces {
            DatapathInterfaceScope::Single(interface) => {
                self.network.set_link_state(interface, state)
            }
            DatapathInterfaceScope::Pair { first, second } => {
                self.network.set_link_state(first, state);
                self.network.set_link_state(second, state);
            }
        }
    }

    pub(super) fn reported_active_tx_interface(&self) -> NetworkInterfaceId {
        let reported = self.services.active_tx_interface();
        match (self.interfaces, reported) {
            (DatapathInterfaceScope::Single(interface), None) => interface,
            (_, Some(interface)) if self.interfaces.contains(interface) => interface,
            (DatapathInterfaceScope::Pair { .. }, None) => {
                panic!("paired DATAPATH services must identify role-generated TX")
            }
            (_, Some(_)) => panic!("services reported TX outside the DATAPATH scope"),
        }
    }

    pub(super) fn retained_prepared_tx_interface(&self) -> NetworkInterfaceId {
        if let Some(interface) = self.prepared_tx_interface {
            return interface;
        }
        let reported = self.services.prepared_tx_interface();
        match (self.interfaces, reported) {
            (DatapathInterfaceScope::Single(interface), None) => interface,
            (_, Some(interface)) if self.interfaces.contains(interface) => interface,
            (DatapathInterfaceScope::Pair { .. }, None) => {
                panic!("paired DATAPATH services must identify prepared TX")
            }
            (_, Some(_)) => panic!("services reported prepared TX outside the DATAPATH scope"),
        }
    }

    /// Cancel the retained software transaction with the exact logical-VIF
    /// capability that owns any out-of-core materialization request.
    pub(super) fn cancel_prepared_network_tx(&mut self) -> Result<(), B::Error> {
        if !self.services.has_prepared_tx() {
            return Ok(());
        }
        let interface = self.retained_prepared_tx_interface();
        let network = self.network.tx_consumer(interface);
        self.services.cancel_prepared_tx(&network)
    }
}

impl<'irq, M: RawMutex, N, B, K> DatapathRunner<'irq, M, N, B, N::RxPublisher, K>
where
    N: DatapathNetwork,
    B: DatapathServices<N::TxFrame, N::PhysicalTxFrame>,
{
    pub fn new(
        irq: &'irq EmbassyMacIrqRuntime<M>,
        network: N,
        interface: NetworkInterfaceId,
        services: B,
        clock: K,
    ) -> Self {
        let network_rx = network.rx_publisher(interface);
        Self::new_with_rx_set(irq, network, interface, network_rx, services, clock)
    }
}

#[cfg(test)]
mod fairness_tests;
